//! Native shader boundary: never treats a WGSL hash as HLSL or DXBC.
//! The upstream compiler/catalog must explicitly associate protocol identity,
//! DXBC bytes and semantic names. No guessed filenames or semantic hash rule.
use std::{collections::{HashMap, HashSet}, ffi::CString, sync::Arc};
use anyhow::{Context, Result, bail, ensure};
use windows::{core::PCSTR, Win32::Graphics::{Direct3D11::{
    ID3D11ClassLinkage, ID3D11VertexShader, ID3D11PixelShader, ID3D11InputLayout,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_CLASSIFICATION,
}, Dxgi::Common::DXGI_FORMAT}};
use crate::D3D11Device;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage { Vertex, Pixel }
impl Stage {
    fn wire(value: u32) -> Result<Self> {
        match value { 0 => Ok(Self::Vertex), 1 => Ok(Self::Pixel),
            _ => bail!("unsupported GM45 shader stage {value}; graphics VS/PS only") }
    }
}
/// Preserve all four words. The fourth is opaque until the producer defines it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature(pub [u32; 4]);
#[derive(Clone, Debug)]
pub struct ShaderRequest { pub id: u32, pub hash: u64, pub stage: Stage, pub signature: Vec<Signature> }
impl ShaderRequest {
    pub fn decode(words: &[u32]) -> Result<Self> {
        ensure!(words.len() >= 5, "CREATE_SHADER truncated header");
        let n = words[4] as usize;
        ensure!(n <= 32 && words.len() == 5 + n * 4, "CREATE_SHADER invalid signature length");
        ensure!(words[0] != 0, "shader id zero is reserved for unbind");
        let signature: Vec<_> = words[5..].chunks_exact(4)
            .map(|s| Signature([s[0], s[1], s[2], s[3]])).collect();
        unique_signature(&signature)?;
        Ok(Self { id: words[0], hash: u64::from(words[2]) | (u64::from(words[3]) << 32),
            stage: Stage::wire(words[1])?, signature })
    }
}
fn unique_signature(signature: &[Signature]) -> Result<()> {
    let (mut names, mut registers) = (HashSet::new(), HashSet::new());
    for Signature(s) in signature {
        ensure!(names.insert((s[0], s[1])) && registers.insert(s[2]), "duplicate signature semantic/register");
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct LayoutRequest { pub id: u32, pub elements: Vec<[u32; 7]> }
impl LayoutRequest {
    pub fn decode(words: &[u32]) -> Result<Self> {
        ensure!(words.len() >= 2, "CREATE_LAYOUT truncated header");
        let n = words[1] as usize;
        ensure!(words[0] != 0 && n <= 32 && words.len() == 2 + n * 7, "CREATE_LAYOUT invalid id/length");
        Ok(Self { id: words[0], elements: words[2..].chunks_exact(7)
            .map(|s| [s[0], s[1], s[2], s[3], s[4], s[5], s[6]]).collect() })
    }
}
#[derive(Clone, Debug)]
pub struct Semantic {
    pub wire: Signature,
    /// From native shader reflection/compiler metadata, not an invented hash lookup.
    pub name: String,
}
#[derive(Clone, Debug)]
pub struct Artifact {
    /// Association supplied by the authoritative compiler/catalog. This is NOT
    /// the DXBC container checksum, and cannot be recomputed from WGSL text.
    pub protocol_hash: u64,
    pub stage: Stage,
    pub signature: Vec<Semantic>,
    pub dxbc: Arc<[u8]>,
}
#[derive(Clone, Debug)]
struct Reflected { name: String, index: u32, register: u32, mask: u8, component: u32 }
#[derive(Clone, Debug)]
struct Validated { artifact: Artifact, inputs: Vec<Reflected> }
fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).context("DXBC offset overflow")?;
    let b = bytes.get(offset..end).context("truncated DXBC word")?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn inspect(bytes: &[u8]) -> Result<(Stage, Vec<Reflected>)> {
    ensure!(bytes.get(..4) == Some(b"DXBC"), "native artifact is not DXBC (WGSL/HLSL are not bytecode)");
    ensure!(bytes.len() >= 32 && word(bytes, 24)? as usize == bytes.len(), "invalid DXBC container length");
    let n = word(bytes, 28)? as usize;
    ensure!(n <= 64 && 32 + n * 4 <= bytes.len(), "invalid DXBC chunk table");
    let (mut stage, mut signature) = (None, None);
    let mut ranges = Vec::new();
    for i in 0..n {
        let offset = word(bytes, 32 + i * 4)? as usize;
        let header_end = offset.checked_add(8).context("chunk offset overflow")?;
        let tag = bytes.get(offset..offset.checked_add(4).context("tag overflow")?).context("truncated DXBC tag")?;
        let size = word(bytes, offset.checked_add(4).context("size overflow")?)? as usize;
        let end = header_end.checked_add(size).context("chunk size overflow")?;
        ensure!(offset >= 32 + n * 4 && end <= bytes.len(), "invalid DXBC chunk bounds");
        ensure!(ranges.iter().all(|&(a, b)| end <= a || offset >= b), "overlapping DXBC chunks");
        ranges.push((offset, end));
        let data = &bytes[header_end..end];
        if tag == b"SHDR" || tag == b"SHEX" {
            ensure!(stage.is_none() && size >= 8 && size % 4 == 0, "duplicate/invalid shader chunk");
            let version = word(data, 0)?;
            ensure!(version & 0xffff == 0x40, "GM45 requires Shader Model 4_0");
            ensure!(word(data, 4)? as usize == size / 4, "invalid shader token length");
            stage = Some(match version >> 16 { 1 => Stage::Vertex, 0 => Stage::Pixel,
                _ => bail!("native shader stage unsupported on this graphics path") });
        } else if tag == b"ISGN" {
            ensure!(signature.is_none(), "duplicate input signature");
            let count = word(data, 0)? as usize;
            ensure!(count <= 32 && 8 + count * 24 <= size, "invalid input signature table");
            let mut inputs = Vec::new();
            for j in 0..count {
                let at = 8 + j * 24;
                if word(data, at + 8)? != 0 { continue; } // System values do not belong to IA layouts.
                let name_at = word(data, at)? as usize;
                ensure!(name_at >= 8 + count * 24 && name_at < size, "invalid semantic name offset");
                let tail = &data[name_at..];
                let len = tail.iter().position(|&b| b == 0).context("unterminated semantic name")?;
                let name = std::str::from_utf8(&tail[..len]).context("semantic is not UTF-8")?.to_owned();
                ensure!(!name.is_empty() && name.is_ascii(), "invalid native semantic name");
                let mask = data[at + 20];
                let component = word(data, at + 12)?;
                ensure!(mask != 0 && mask <= 15 && (1..=3).contains(&component), "invalid signature component/mask");
                inputs.push(Reflected { name, index: word(data, at + 4)?, register: word(data, at + 16)?, mask, component });
            }
            signature = Some(inputs);
        } else if tag == b"ISG1" || tag == b"ISG5" {
            bail!("extended input signature not supported by this SM4 resolver");
        }
    }
    Ok((stage.context("DXBC has no shader chunk")?, signature.context("DXBC has no input signature")?))
}
/// Explicit artifact registry. No implicit hash-to-filename or hash-to-HLSL rule.
#[derive(Default)]
pub struct ShaderResolver { programs: HashMap<(u64, Stage), Arc<Validated>> }
impl ShaderResolver {
    pub fn register(&mut self, catalog_hash: u64, artifact: Artifact) -> Result<()> {
        ensure!(catalog_hash == artifact.protocol_hash, "catalog/artifact hash mismatch");
        ensure!(!self.programs.contains_key(&(catalog_hash, artifact.stage)), "shader identity already registered");
        let (stage, inputs) = inspect(&artifact.dxbc)?;
        ensure!(stage == artifact.stage, "DXBC/metadata stage mismatch");
        let wire: Vec<_> = artifact.signature.iter().map(|s| s.wire).collect();
        unique_signature(&wire)?;
        if stage == Stage::Vertex { ensure!(inputs.len() == wire.len(), "vertex signature metadata incomplete"); }
        for semantic in &artifact.signature {
            let s = semantic.wire.0;
            ensure!(!semantic.name.contains('\0'), "semantic contains NUL");
            ensure!(inputs.iter().any(|r| r.name == semantic.name && r.index == s[1] && r.register == s[2]),
                "semantic metadata does not match DXBC reflection");
        }
        self.programs.insert((catalog_hash, stage), Arc::new(Validated { artifact, inputs }));
        Ok(())
    }
    pub fn resolve(&self, request: &ShaderRequest) -> Result<ResolvedShader> {
        ensure!(request.id != 0, "shader id zero is reserved");
        unique_signature(&request.signature)?;
        let program = self.programs.get(&(request.hash, request.stage)).with_context(|| {
            if self.programs.keys().any(|&(h, _)| h == request.hash) { "shader stage mismatch" }
            else { "native shader artifact missing: upstream WGSL catalog is not an SM4 artifact provider" }
        })?;
        let expected: Vec<_> = program.artifact.signature.iter().map(|s| s.wire).collect();
        ensure!(expected == request.signature, "CREATE_SHADER signature mismatch (including opaque fourth word)");
        Ok(ResolvedShader { id: request.id, program: Arc::clone(program) })
    }
    /// Existing resolved objects retain their artifact through Arc ownership.
    pub fn remove(&mut self, hash: u64, stage: Stage) -> bool { self.programs.remove(&(hash, stage)).is_some() }
}
#[derive(Clone, Debug)]
pub struct ResolvedShader { pub id: u32, program: Arc<Validated> }
impl ResolvedShader {
    pub fn hash(&self) -> u64 { self.program.artifact.protocol_hash }
    pub fn stage(&self) -> Stage { self.program.artifact.stage }
    /// Calls the real D3D11 driver. No test shader/source substitution.
    pub fn create_native(&self, device: &D3D11Device) -> Result<NativeShader> {
        let bytes = &self.program.artifact.dxbc;
        unsafe { match self.stage() {
            Stage::Vertex => { let mut out = None;
                device.raw_device().CreateVertexShader(bytes, None::<&ID3D11ClassLinkage>, Some(&mut out))?;
                Ok(NativeShader::Vertex(out.context("CreateVertexShader returned no object")?)) },
            Stage::Pixel => { let mut out = None;
                device.raw_device().CreatePixelShader(bytes, None::<&ID3D11ClassLinkage>, Some(&mut out))?;
                Ok(NativeShader::Pixel(out.context("CreatePixelShader returned no object")?)) },
        } }
    }
    pub fn resolve_layout(&self, request: &LayoutRequest) -> Result<ResolvedLayout> {
        ensure!(self.stage() == Stage::Vertex, "input layout requires a vertex shader");
        ensure!(request.id != 0 && request.elements.len() == self.program.artifact.signature.len(), "layout signature element count mismatch");
        let (mut seen, mut slots) = (HashSet::new(), HashMap::new());
        let mut elements = Vec::new();
        let mut minimum_strides = [0u32; 16];
        for e in &request.elements {
            ensure!(seen.insert((e[0], e[1])), "duplicate layout semantic");
            let semantic = self.program.artifact.signature.iter().find(|s| s.wire.0[..2] == e[..2])
                .context("layout semantic absent from vertex signature")?;
            let reflected = self.program.inputs.iter().find(|r| r.name == semantic.name && r.index == e[1])
                .context("native semantic absent")?;
            let (bytes, component, channels, alignment) = format_info(e[2])?;
            ensure!(component == reflected.component && channels == 32 - u32::from(reflected.mask).leading_zeros(), "layout format incompatible with shader signature");
            ensure!(e[3] < 16 && e[4] != u32::MAX && e[4] % alignment == 0, "invalid layout slot/offset (append-aligned unsupported)");
            ensure!(e[5] <= 1 && ((e[5] == 0 && e[6] == 0) || (e[5] == 1 && e[6] > 0)), "invalid input classification/step rate");
            let end = e[4].checked_add(bytes).context("layout offset overflow")?;
            ensure!(end <= 2048, "layout element exceeds supported stride limit");
            let class = (e[5], e[6]);
            ensure!(slots.insert(e[3], class).is_none_or(|old| old == class), "inconsistent input-slot classification");
            minimum_strides[e[3] as usize] = minimum_strides[e[3] as usize].max(end);
            elements.push((CString::new(semantic.name.as_str())?, *e));
        }
        Ok(ResolvedLayout { id: request.id, shader: self.clone(), elements, minimum_strides })
    }
}
pub enum NativeShader { Vertex(ID3D11VertexShader), Pixel(ID3D11PixelShader) }
pub struct ResolvedLayout { pub id: u32, shader: ResolvedShader, elements: Vec<(CString, [u32; 7])>, minimum_strides: [u32; 16] }
impl ResolvedLayout {
    pub fn validate_strides(&self, strides: &[u32; 16]) -> Result<()> {
        for (need, stride) in self.minimum_strides.iter().zip(strides) {
            ensure!(*need == 0 || (*stride >= *need && *stride <= 2048), "vertex buffer stride too small/large");
        }
        Ok(())
    }
    pub fn create_native(&self, device: &D3D11Device) -> Result<Option<ID3D11InputLayout>> {
        if self.elements.is_empty() { return Ok(None); }
        let descs: Vec<_> = self.elements.iter().map(|(name, e)| D3D11_INPUT_ELEMENT_DESC {
            SemanticName: PCSTR(name.as_ptr().cast()), SemanticIndex: e[1], Format: DXGI_FORMAT(e[2] as i32),
            InputSlot: e[3], AlignedByteOffset: e[4], InputSlotClass: D3D11_INPUT_CLASSIFICATION(e[5] as i32),
            InstanceDataStepRate: e[6],
        }).collect();
        let mut out = None;
        unsafe { device.raw_device().CreateInputLayout(&descs, &self.shader.program.artifact.dxbc, Some(&mut out))?; }
        Ok(Some(out.context("CreateInputLayout returned no object")?))
    }
}
/// Deliberately restricted, explicit DXGI allowlist; not a claim of all FL10 formats.
fn format_info(format: u32) -> Result<(u32, u32, u32, u32)> {
    let (size, component, channels) = match format {
        2 => (16, 3, 4), 6 => (12, 3, 3), 16 => (8, 3, 2), 41 => (4, 3, 1),
        3 => (16, 1, 4), 7 => (12, 1, 3), 17 => (8, 1, 2), 42 => (4, 1, 1),
        4 => (16, 2, 4), 8 => (12, 2, 3), 18 => (8, 2, 2), 43 => (4, 2, 1),
        28 => return Ok((4, 3, 4, 1)),
        _ => bail!("unsupported input-layout DXGI format {format}"),
    };
    Ok((size, component, channels, 4))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Structural DXBC fixtures for parser/metadata tests ONLY. Not executable
    // shader programs; never passed to a driver or used as GPU evidence.
    fn container(stage: Stage) -> Vec<u8> {
        let mut sig = vec![0u8; 32];
        sig[0..4].copy_from_slice(&1u32.to_le_bytes());
        sig[4..8].copy_from_slice(&8u32.to_le_bytes());
        sig[8..12].copy_from_slice(&32u32.to_le_bytes());
        sig[20..24].copy_from_slice(&3u32.to_le_bytes());
        sig[28] = 7;
        sig.extend_from_slice(b"POSITION\0");
        let mut b = vec![0u8; 40];
        b[..4].copy_from_slice(b"DXBC");
        b[20..24].copy_from_slice(&1u32.to_le_bytes());
        b[28..32].copy_from_slice(&2u32.to_le_bytes());
        b[32..36].copy_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(b"SHDR"); b.extend_from_slice(&8u32.to_le_bytes());
        let token = if stage == Stage::Vertex { 0x10040u32 } else { 0x40 };
        b.extend_from_slice(&token.to_le_bytes()); b.extend_from_slice(&2u32.to_le_bytes());
        let at = b.len() as u32; b[36..40].copy_from_slice(&at.to_le_bytes());
        b.extend_from_slice(b"ISGN"); b.extend_from_slice(&(sig.len() as u32).to_le_bytes());
        b.extend_from_slice(&sig); let size = b.len() as u32;
        b[24..28].copy_from_slice(&size.to_le_bytes()); b
    }
    fn artifact() -> Artifact {
        Artifact { protocol_hash: 0x1234, stage: Stage::Vertex,
            signature: vec![Semantic { wire: Signature([77, 0, 0, 91]), name: "POSITION".into() }],
            dxbc: container(Stage::Vertex).into() }
    }
    fn request() -> ShaderRequest { ShaderRequest::decode(&[5, 0, 0x1234, 0, 1, 77, 0, 0, 91]).unwrap() }
    fn resolved() -> ResolvedShader {
        let mut r = ShaderResolver::default(); r.register(0x1234, artifact()).unwrap(); r.resolve(&request()).unwrap()
    }
    fn layout(e: [u32; 7]) -> LayoutRequest { LayoutRequest { id: 6, elements: vec![e] } }
    fn base() -> [u32; 7] { [77, 0, 6, 0, 0, 0, 0] }
    #[test] fn real_wire_header_and_opaque_signature_word_are_preserved() {
        let r = request(); assert_eq!((r.id, r.hash, r.stage), (5, 0x1234, Stage::Vertex));
        assert_eq!(r.signature, vec![Signature([77, 0, 0, 91])]);
    }
    #[test] fn malformed_wire_and_compute_are_rejected() {
        for n in 0..9 { assert!(ShaderRequest::decode(&[5, 0, 0x1234, 0, 1, 77, 0, 0, 91][..n]).is_err()); }
        assert!(ShaderRequest::decode(&[5, 2, 0, 0, 0]).is_err());
        assert!(ShaderRequest::decode(&[0, 0, 0, 0, 0]).is_err());
        assert!(LayoutRequest::decode(&[1, u32::MAX]).is_err());
    }
    #[test] fn missing_shader_has_no_fallback() { assert!(ShaderResolver::default().resolve(&request()).is_err()); }
    #[test] fn catalog_hash_stage_and_native_model_are_checked() {
        let mut r = ShaderResolver::default(); assert!(r.register(7, artifact()).is_err());
        let mut a = artifact(); a.stage = Stage::Pixel; assert!(r.register(0x1234, a).is_err());
        let mut a = artifact(); let mut bytes = a.dxbc.to_vec(); bytes[48..52].copy_from_slice(&0x10050u32.to_le_bytes());
        a.dxbc = bytes.into(); assert!(r.register(0x1234, a).is_err());
        assert!(r.programs.is_empty());
    }
    #[test] fn wrong_stage_and_signature_are_not_cached_as_success() {
        let mut r = ShaderResolver::default(); r.register(0x1234, artifact()).unwrap();
        let mut q = request(); q.stage = Stage::Pixel; assert!(r.resolve(&q).is_err());
        q = request(); q.signature[0].0[3] ^= 1; assert!(r.resolve(&q).is_err());
        assert!(r.resolve(&request()).is_ok());
    }
    #[test] fn registration_is_atomic_and_resolved_identity_survives_removal() {
        let mut r = ShaderResolver::default(); r.register(0x1234, artifact()).unwrap();
        let s = r.resolve(&request()).unwrap(); assert!(r.register(0x1234, artifact()).is_err());
        assert!(r.remove(0x1234, Stage::Vertex)); assert!(r.resolve(&request()).is_err());
        assert_eq!(s.hash(), 0x1234); assert!(s.resolve_layout(&layout(base())).is_ok());
    }
    #[test] fn semantic_names_must_match_native_reflection() {
        let mut a = artifact(); a.signature[0].name = "NORMAL".into();
        assert!(ShaderResolver::default().register(0x1234, a).is_err());
    }
    #[test] fn truncated_or_overlapping_dxbc_is_rejected() {
        let b = container(Stage::Vertex);
        for n in 0..b.len() { assert!(inspect(&b[..n]).is_err()); }
        let mut b = b; b[36..40].copy_from_slice(&40u32.to_le_bytes()); assert!(inspect(&b).is_err());
    }
    #[test] fn layout_wire_and_stride_validation() {
        let l = LayoutRequest::decode(&[6, 1, 77, 0, 6, 0, 4, 0, 0]).unwrap();
        let resolved = resolved().resolve_layout(&l).unwrap(); let mut strides = [0; 16];
        assert!(resolved.validate_strides(&strides).is_err()); strides[0] = 16;
        assert!(resolved.validate_strides(&strides).is_ok());
    }
    #[test] fn missing_duplicate_and_incompatible_elements_are_rejected() {
        let s = resolved(); assert!(s.resolve_layout(&LayoutRequest { id: 6, elements: vec![] }).is_err());
        assert!(s.resolve_layout(&LayoutRequest { id: 6, elements: vec![base(), base()] }).is_err());
        for (at, value) in [(0, 88), (1, 1), (2, 7), (2, 16), (2, 0)] {
            let mut e = base(); e[at] = value; assert!(s.resolve_layout(&layout(e)).is_err());
        }
    }
    #[test] fn slot_offset_class_and_step_rate_are_checked() {
        let s = resolved();
        for (at, value) in [(3, 16), (4, u32::MAX), (4, 1), (4, 2048), (5, 2), (6, 1)] {
            let mut e = base(); e[at] = value; assert!(s.resolve_layout(&layout(e)).is_err());
        }
        let mut e = base(); e[5] = 1; assert!(s.resolve_layout(&layout(e)).is_err());
        e[6] = 1; assert!(s.resolve_layout(&layout(e)).is_ok());
        assert!(s.resolve_layout(&layout(base())).is_ok()); // A rejected request leaves no state.
    }
    #[test] fn registry_does_not_accept_source_text_as_native_bytecode() {
        let mut a = artifact(); a.dxbc = Arc::from(&b"@vertex fn main() {}"[..]);
        assert!(ShaderResolver::default().register(0x1234, a).is_err());
    }
}

/// Native object plus the immutable identity/reflection that created it.
/// This is an owned factory result, not an entry in the executor's ID table yet.
pub struct ShaderObject {
    source: ResolvedShader,
    native: NativeShader,
    owner: windows::Win32::Graphics::Direct3D11::ID3D11Device,
}
impl ShaderObject {
    pub fn id(&self) -> u32 { self.source.id }
    pub fn hash(&self) -> u64 { self.source.hash() }
    pub fn stage(&self) -> Stage { self.source.stage() }
    pub fn native(&self) -> &NativeShader { &self.native }
}
pub struct LayoutObject {
    layout: ResolvedLayout,
    native: Option<ID3D11InputLayout>,
    shader: Arc<ShaderObject>,
}
impl LayoutObject {
    pub fn id(&self) -> u32 { self.layout.id }
    pub fn shader(&self) -> &Arc<ShaderObject> { &self.shader }
    pub fn native(&self) -> Option<&ID3D11InputLayout> { self.native.as_ref() }
    pub fn validate_strides(&self, strides: &[u32; 16]) -> Result<()> { self.layout.validate_strides(strides) }
}
impl crate::D3D11Executor {
    /// Decode, resolve and create on this executor's real device. The caller
    /// must reserve the resource ID in its unified object table before use.
    pub fn create_shader_from_payload(&self, resolver: &ShaderResolver, payload: &[u32]) -> Result<Arc<ShaderObject>> {
        let source = resolver.resolve(&ShaderRequest::decode(payload)?)?;
        let native = source.create_native(self.device())?;
        Ok(Arc::new(ShaderObject { source, native, owner: self.device().raw_device().clone() }))
    }
    /// Does not bind or publish partial state if validation/driver creation fails.
    pub fn create_layout_from_payload(&self, shader: Arc<ShaderObject>, payload: &[u32]) -> Result<LayoutObject> {
        use windows::core::Interface;
        ensure!(shader.owner.as_raw() == self.device().raw_device().as_raw(), "shader belongs to a different D3D11 device");
        let request = LayoutRequest::decode(payload)?;
        ensure!(request.id != shader.id(), "layout id collides with its vertex shader id");
        let layout = shader.source.resolve_layout(&request)?;
        let native = layout.create_native(self.device())?;
        Ok(LayoutObject { layout, native, shader })
    }
}

/// Native input metadata for the compiler/catalog bridge, validated by the
/// same container reader used by registration. Never derives engine hashes.
#[derive(Clone,Debug)]
pub struct NativeInput {pub name:String,pub index:u32,pub register:u32,pub mask:u8,pub component:u32}
pub fn native_inputs(dxbc:&[u8])->Result<(Stage,Vec<NativeInput>)> {
    let (stage,inputs)=inspect(dxbc)?;
    Ok((stage,inputs.into_iter().map(|i|NativeInput{name:i.name,index:i.index,register:i.register,mask:i.mask,component:i.component}).collect()))
}
