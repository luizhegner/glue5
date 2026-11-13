use std::{collections::HashSet, thread, time::Duration};

use anyhow::bail;
use glue5_ring::{self as gpu, GpuBackend, GpuRing, State, RingMemory, Reader};

use crate::D3D11Command;
use crate::pipeline::{GraphicsExecutor, PipelineCommand, PipelineKind};
use crate::shader_resolver::{Artifact, ShaderResolver};
use crate::ring_decode::{self, Command as PayloadCommand, Kind};

const RELEASE_BATCH: usize = 512; // Bound ring back-pressure while commands are being consumed.

/// Returns an opt-in backend that consumes the engine's GpuRing.
///
/// Executes the resource allowlist plus explicit-catalog shader/layout creation,
/// graphics bindings and six-word DRAW kind 0/1. Unsupported states, instancing,
/// UAV/compute, MRT and depth remain fail-closed at this increment.
/// FENCE wakes the engine only after a reusable D3D11 event query completes.
pub fn backend<M: RingMemory>() -> GpuBackend<M> { backend_with_artifacts(Vec::new()) }

/// Explicit producer catalog injection. Empty/default catalogs fail closed.
pub fn backend_with_artifacts<M: RingMemory>(artifacts: Vec<(u64, Artifact)>) -> GpuBackend<M> {
    Box::new(move |ring| {
        let artifacts = artifacts.clone();
        let thread_ring = ring;
        let spawned = thread::Builder::new().name("gpu (native d3d11 gm45)".into()).spawn(move || run(thread_ring, artifacts));

        if let Err(error) = spawned {
            thread_ring.mem().backend_failed(&format!("no thread for the native D3D11 renderer: {error}"));
        }
    })
}

fn run<M: RingMemory>(ring: GpuRing<M>, artifacts: Vec<(u64, Artifact)>) {
    // Keep this reader for the worker's entire lifetime. Creating another
    // Reader resets the shared tail to RING_DATA and would replay old bytes.
    let mut reader = ring.reader();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<()> {
        let mut executor = open_with_artifacts(artifacts)?;
        ring.set_state(State::Running);
        log::info!("gpu: native D3D11 worker running ({:?})", executor.resources().device().feature_level());
        let mut warned = HashSet::new();
        worker_loop(ring, &mut reader, &mut executor, &mut warned)
    }));

    match outcome {
        Ok(Ok(())) => fail_and_drain(ring, &mut reader, "worker returned unexpectedly"),
        Ok(Err(error)) => fail_and_drain(ring, &mut reader, &format!("{error:#}")),
        Err(_) => fail_and_drain(ring, &mut reader, "worker panicked"),
    }
}

/// Construct the same catalog/executor used by the asynchronous worker.
pub fn open_with_artifacts(artifacts: Vec<(u64, Artifact)>) -> anyhow::Result<GraphicsExecutor> {
    let mut resolver=ShaderResolver::default();
    for (hash,artifact) in artifacts {resolver.register(hash,artifact)?;}
    GraphicsExecutor::open_hardware(resolver)
}

#[derive(Debug)]
pub struct BatchProof {
    pub commands: usize, pub words: usize, pub fingerprint:u64,
    pub payload_offsets: Vec<u32>, pub opcodes: Vec<u32>, pub wrapped:bool,
}
/// FNV-1a over little-endian wire words; provenance checksum, not shader identity.
pub fn wire_fingerprint(words:&[u32])->u64 {
    let mut hash=0xcbf29ce484222325;
    for &w in words {hash_word(&mut hash,w);} hash
}
fn hash_word(hash:&mut u64,word:u32) {
    for b in word.to_le_bytes() {*hash=(*hash ^ u64::from(b)).wrapping_mul(0x100000001b3);}
}
/// Bounded production pump. Its ONLY command source is the shared real Reader.
/// The worker and integration harness both call this function; neither passes
/// decoded commands or a replacement reader. Zero-copy origin is checked before
/// dispatch, while the reader still owns the unreleased memory span.
pub fn drain_available<M: RingMemory>(reader:&mut Reader<M>,executor:&mut GraphicsExecutor)->anyhow::Result<BatchProof> {
    let ring=reader.ring(); let mem=ring.mem(); let head=reader.head();
    let start=mem.ptr(ring.base()) as usize;
    let mut proof=BatchProof{commands:0,words:0,fingerprint:0xcbf29ce484222325,
        payload_offsets:vec![],opcodes:vec![],wrapped:head<mem.u32(ring.base()+4)};
    let mut warned=HashSet::new();
    while let Some(command)=reader.next(head)? {
        let ptr=command.payload.as_ptr() as usize;
        anyhow::ensure!(ptr>=start+gpu::RING_DATA as usize+8 && ptr.checked_add(command.payload.len()*4).is_some_and(|end|end<=start+ring.size() as usize),"reader payload not in ring mapping");
        let offset=(ptr-start) as u64;
        anyhow::ensure!(mem.u32(ring.base()+offset-8)==command.op && mem.u32(ring.base()+offset-4) as usize==command.payload.len(),"reader header/payload origin mismatch");
        proof.payload_offsets.push(offset as u32); proof.opcodes.push(command.op);
        hash_word(&mut proof.fingerprint,command.op);hash_word(&mut proof.fingerprint,command.payload.len() as u32);
        for &word in command.payload {hash_word(&mut proof.fingerprint,word);}
        proof.words+=2+command.payload.len();
        execute(executor,ring,&mut warned,command)?;
        proof.commands+=1;
        if proof.commands.is_multiple_of(RELEASE_BATCH) {reader.release();}
    }
    reader.release();
    if proof.commands!=0 {executor.execute(PipelineCommand::Resource(D3D11Command::Flush))?;}
    Ok(proof)
}
fn worker_loop<M: RingMemory>(
    _ring:GpuRing<M>, reader:&mut Reader<M>, executor:&mut GraphicsExecutor,
    _warned:&mut HashSet<u32>,
)->anyhow::Result<()> {
    loop {
        if reader.is_empty(reader.head()) {reader.wait(Duration::from_millis(100));continue;}
        drain_available(reader,executor)?;
    }
}

/// Publish failure and keep releasing ring space using the existing reader.
/// Do not synthesize successful FENCE notifications after GPU work failed.
fn fail_and_drain<M: RingMemory>(ring: GpuRing<M>, reader: &mut Reader<M>, reason: &str) -> ! {
    log::error!("gpu: native D3D11 backend entering Failed state: {reason}");
    ring.add_errors(1);
    ring.mem().backend_failed(&format!("native D3D11 backend failed: {reason}"));
    ring.set_state(State::Failed);
    loop {
        let head = reader.head();
        if let Err(error) = reader.discard_to_head(head) {
            log::error!("gpu: cannot safely discard malformed ring head: {error:#}");
        }
        reader.wait(Duration::from_millis(100));
    }
}

fn execute<M: RingMemory>(
    executor: &mut GraphicsExecutor,
    ring: GpuRing<M>,
    warned: &mut HashSet<u32>,
    command: gpu::Command<'_>,
) -> anyhow::Result<()> {
    if command.op == gpu::op::FENCE {
        let (address, sequence) = decode_fence(command)?;
        executor.wait_for_gpu()?;
        ring.mem().store_notify_checked(address, sequence)?;
        return Ok(());
    }

    // Numeric constants remain owned by the authoritative runtime protocol.
    let kind = match command.op {
        gpu::op::CREATE_SHADER => Some(PipelineKind::CreateShader),
        gpu::op::CREATE_LAYOUT => Some(PipelineKind::CreateLayout),
        gpu::op::SET_SHADER => Some(PipelineKind::Shader),
        gpu::op::SET_INPUT_LAYOUT => Some(PipelineKind::Layout),
        gpu::op::SET_TOPOLOGY => Some(PipelineKind::Topology),
        gpu::op::SET_VERTEX_BUFFERS => Some(PipelineKind::VertexBuffers),
        gpu::op::SET_INDEX_BUFFER => Some(PipelineKind::IndexBuffer),
        gpu::op::SET_RENDER_TARGETS => Some(PipelineKind::RenderTargets),
        gpu::op::SET_VIEWPORTS => Some(PipelineKind::Viewport),
        gpu::op::DRAW => Some(PipelineKind::Draw),
        gpu::op::CLEAR_STATE => Some(PipelineKind::Clear),
        _ => None,
    };
    if let Some(kind) = kind { return executor.submit(kind, command.payload); }
    let opcode = command.op;
    let decoded = match decode(command) {
        Ok(decoded) => decoded,
        Err(error) => {
            if !matches!(
                opcode,
                gpu::op::NOP
                    | gpu::op::INIT
                    | gpu::op::CREATE_TEXTURE
                    | gpu::op::CREATE_BUFFER
                    | gpu::op::UPLOAD_BUFFER
                    | gpu::op::UPLOAD_BUFFER_NO_OVERWRITE
                    | gpu::op::UPLOAD_TEXTURE
                    | gpu::op::CREATE_SRV
                    | gpu::op::SET_SRVS
                    | gpu::op::CREATE_STATE
                    | gpu::op::SET_SAMPLERS
                    | gpu::op::DESTROY_OBJECT
                    | gpu::op::CLEAR_RT
            ) && warned.insert(opcode)
            {
                log::warn!("gpu: native D3D11 backend does not implement opcode {opcode}");
            }
            executor.execute(PipelineCommand::Clear)?;
            return Err(error);
        }
    };

    match decoded {
        Some(native_command) => executor.execute(PipelineCommand::Resource(native_command)),
        None => Ok(()),
    }
}

/// Decode the fence payload without coupling payload tests to the engine.
fn decode_fence(command: gpu::Command<'_>) -> anyhow::Result<(u64, u32)> {
    ring_decode::decode_fence(PayloadCommand { op: Kind::FENCE, payload: command.payload })
}

/// Translate authoritative runtime opcode constants into symbolic payload kinds.
fn decode(command: gpu::Command<'_>) -> anyhow::Result<Option<D3D11Command>> {
    let op = match command.op {
        gpu::op::NOP => Kind::NOP,
        gpu::op::INIT => Kind::INIT,
        gpu::op::LOG => Kind::LOG,
        gpu::op::SCENE_DONE => Kind::SCENE_DONE,
        gpu::op::CREATE_BUFFER => Kind::CREATE_BUFFER,
        gpu::op::UPLOAD_BUFFER => Kind::UPLOAD_BUFFER,
        gpu::op::UPLOAD_BUFFER_NO_OVERWRITE => Kind::UPLOAD_BUFFER_NO_OVERWRITE,
        gpu::op::CREATE_SRV => Kind::CREATE_SRV,
        gpu::op::SET_SRVS => Kind::SET_SRVS,
        gpu::op::CREATE_STATE => Kind::CREATE_STATE,
        gpu::op::SET_SAMPLERS => Kind::SET_SAMPLERS,
        gpu::op::CREATE_UAV => Kind::CREATE_UAV,
        gpu::op::SET_UAVS => Kind::SET_UAVS,
        gpu::op::UPLOAD_TEXTURE => Kind::UPLOAD_TEXTURE,
        gpu::op::CREATE_TEXTURE => Kind::CREATE_TEXTURE,
        gpu::op::DESTROY_OBJECT => Kind::DESTROY_OBJECT,
        gpu::op::CLEAR_RT => Kind::CLEAR_RT,
        unsupported => bail!("unsupported GpuRing opcode {unsupported}"),
    };
    ring_decode::decode(PayloadCommand { op, payload: command.payload })
}
