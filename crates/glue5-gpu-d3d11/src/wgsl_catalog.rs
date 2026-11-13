//! WGSL -> pinned Naga -> HLSL -> FXC SM4 -> validated native catalog.
//! Explicit location contracts are producer metadata, never attribute-hash guesses.
use std::{ffi::CString,sync::Arc};
use anyhow::{Result,Context,ensure};
use sha2::{Digest,Sha256};
use windows::{core::PCSTR,Win32::Graphics::{Direct3D::{ID3DBlob,ID3DInclude},Direct3D::Fxc::*}};
use crate::shader_resolver::{Artifact,Stage,Semantic,Signature,native_inputs};
#[path="wgsl_source.rs"] mod source;
pub use source::LocationContract;
pub struct ProducedShader {
    pub artifact:Artifact, pub wgsl_sha256:[u8;32], pub dxbc_sha256:[u8;32],
    pub hlsl:String, pub entry:String, pub profile:String,
}
impl ProducedShader {
    pub fn create_payload(&self,id:u32)->Vec<u32> {
        let a=&self.artifact;
        let mut p=vec![id,if a.stage==Stage::Vertex {0}else{1},a.protocol_hash as u32,(a.protocol_hash>>32) as u32,a.signature.len() as u32];
        for s in &a.signature {p.extend(s.wire.0);}p
    }
}
/// Deterministic FIXTURE-producer identity. Not asserted to be the game's hash
/// algorithm. A production caller supplies its original identity to compile().
pub fn fixture_hash(wgsl:&str,vertex:bool,entry:&str,inputs:&[LocationContract])->u64 {
    let mut h=Sha256::new();h.update(b"GLUE5_WGSL_FIXTURE_V1\0");h.update([vertex as u8]);
    h.update((entry.len() as u64).to_le_bytes());h.update(entry.as_bytes());
    for c in inputs {h.update(c.location.to_le_bytes());for w in c.wire {h.update(w.to_le_bytes());}}
    h.update(wgsl.as_bytes());let hash:[u8;32]=h.finalize().into();u64::from_le_bytes(hash[..8].try_into().unwrap())
}
pub fn compile(protocol_hash:u64,wgsl:&str,vertex:bool,entry:&str,inputs:&[LocationContract])->Result<ProducedShader> {
    let generated=source::generate(wgsl,vertex,entry,inputs)?;
    let profile=if vertex {"vs_4_0"} else {"ps_4_0"};
    let ep=CString::new(generated.entry.as_str())?;let target=CString::new(profile)?;
    let mut blob:Option<ID3DBlob>=None;let mut errors:Option<ID3DBlob>=None;
    let outcome=unsafe {D3DCompile(generated.hlsl.as_ptr().cast(),generated.hlsl.len(),PCSTR::null(),None,None::<&ID3DInclude>,PCSTR(ep.as_ptr().cast()),PCSTR(target.as_ptr().cast()),D3DCOMPILE_ENABLE_STRICTNESS,0,&mut blob,Some(&mut errors))};
    if let Err(e)=outcome {
        let msg=errors.as_ref().map(|b|unsafe {String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer().cast(),b.GetBufferSize())).into_owned()}).unwrap_or_default();
        anyhow::bail!("Naga-generated HLSL rejected by FXC {profile}: {e}: {msg}");
    }
    let blob=blob.context("FXC did not return DXBC")?;
    let dxbc:Arc<[u8]>=unsafe {std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(),blob.GetBufferSize())}.to_vec().into();
    let (native_stage,reflected)=native_inputs(&dxbc)?;
    let stage=if vertex {Stage::Vertex} else {Stage::Pixel};ensure!(native_stage==stage,"DXBC stage differs from WGSL");
    let mut signature=Vec::new();
    if vertex {
        ensure!(reflected.len()==inputs.len() && generated.locations.len()==inputs.len(),"DXBC/WGSL input count mismatch");
        for c in inputs {
            // Pinned Naga emits LOCn for @location(n); require that ABI in
            // generated HLSL AND actual DXBC. Native name comes from DXBC.
            let native=reflected.iter().find(|r|r.name.eq_ignore_ascii_case("LOC") && r.index==c.location).context("DXBC input not associated with declared WGSL location")?;
            ensure!(c.wire[1]==native.index && c.wire[2]==native.register,"producer wire index/register differs from compiled input");
            signature.push(Semantic{wire:Signature(c.wire),name:native.name.clone()});
        }
    } else {ensure!(reflected.is_empty(),"PS varying/system inputs outside initial catalog subset");}
    let artifact=Artifact{protocol_hash,stage,signature,dxbc};
    // Validate before publishing to a caller; the native worker still performs
    // its own catalog registration and resolves each CREATE_SHADER from ring.
    let mut check=crate::shader_resolver::ShaderResolver::default();check.register(protocol_hash,artifact.clone())?;
    Ok(ProducedShader{wgsl_sha256:Sha256::digest(wgsl.as_bytes()).into(),dxbc_sha256:Sha256::digest(artifact.dxbc.as_ref()).into(),artifact,hlsl:generated.hlsl,entry:generated.entry,profile:profile.into()})
}
