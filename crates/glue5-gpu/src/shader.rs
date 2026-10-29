//! The engine's shaders: WGSL translated ahead of time from the D3D11
//! bytecode (`shaders/<hash>.wgsl`, `index.json`), adapted to wgpu and
//! reflected.
//!
//! The translator's conventions: group 0 is the vertex (or compute) stage,
//! group 1 the pixel stage; binding = D3D register + 0 (constant buffers,
//! `b15` = the special-constant table of `<hash>.consts.json`), 16
//! (samplers), 32 (SRVs), 160 (UAVs), 176 (UAV counters). The constant
//! buffers move to groups of their own, two above their stage's (see
//! [`pack_uniforms`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use regex::Regex;
use wgpu::util::DeviceExt;

use crate::format;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Vertex = 0,
    Pixel = 1,
    Compute = 2,
}

impl Stage {
    pub fn from_d3d(v: u32) -> Option<Stage> {
        match v {
            0 => Some(Stage::Vertex),
            1 => Some(Stage::Pixel),
            2 => Some(Stage::Compute),
            _ => None,
        }
    }

    pub fn group(self) -> u32 {
        if self == Stage::Pixel { 1 } else { 0 }
    }

    /// The group of the stage's constant buffers.
    pub fn constants_group(self) -> u32 {
        self.group() + CONSTANTS_GROUPS
    }

    fn visibility(self) -> wgpu::ShaderStages {
        match self {
            Stage::Vertex => wgpu::ShaderStages::VERTEX,
            Stage::Pixel => wgpu::ShaderStages::FRAGMENT,
            Stage::Compute => wgpu::ShaderStages::COMPUTE,
        }
    }
}

/// The constant buffers' groups are this many above their stage's: their
/// bind groups, made of the uniform ring's buffers, are not those of the
/// textures, which then do not depend on where the constants went.
pub const CONSTANTS_GROUPS: u32 = 2;

/// Binding numbers.
pub const PACKED_CBUFFERS: u32 = 0;
pub const CONSTANT_TABLE: u32 = 15;
pub const SAMPLERS: u32 = 16;
pub const SRVS: u32 = 32;
pub const UAVS: u32 = 160;
pub const UAV_COUNTERS: u32 = 176;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    Float,
    Depth,
    Uint,
    Sint,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Uniform,
    Storage { read_only: bool },
    Sampler { comparison: bool },
    Texture { sample: Sample, dim: wgpu::TextureViewDimension, multisampled: bool },
    StorageTexture { access: wgpu::StorageTextureAccess, format: wgpu::TextureFormat, dim: wgpu::TextureViewDimension },
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub binding: u32,
    pub kind: Kind,
}

impl Binding {
    pub fn layout_entry(&self, stage: Stage) -> wgpu::BindGroupLayoutEntry {
        use wgpu::{BindingType, BufferBindingType, TextureSampleType};
        let ty = match &self.kind {
            // The constant table (the others are in the constants group).
            Kind::Uniform => BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            Kind::Storage { read_only } => BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: *read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            Kind::Sampler { comparison: true } => BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            Kind::Sampler { comparison: false } => BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            Kind::Texture { sample, dim, multisampled } => BindingType::Texture {
                sample_type: match sample {
                    Sample::Float => TextureSampleType::Float { filterable: !multisampled },
                    Sample::Depth => TextureSampleType::Depth,
                    Sample::Uint => TextureSampleType::Uint,
                    Sample::Sint => TextureSampleType::Sint,
                },
                view_dimension: *dim,
                multisampled: *multisampled,
            },
            Kind::StorageTexture { access, format, dim } => {
                BindingType::StorageTexture { access: *access, format: *format, view_dimension: *dim }
            }
        };
        wgpu::BindGroupLayoutEntry { binding: self.binding, visibility: stage.visibility(), ty, count: None }
    }
}

/// The constant buffers a stage reads, merged into one uniform struct at
/// binding 0 (one dynamic offset per stage instead of up to 14 bindings),
/// but for the largest (the bones, lighting, shadows): those keep their own
/// binding (at their slot), copied once per version instead of into each
/// draw's pack. All in the stage's constants group.
#[derive(Clone, Debug)]
pub struct Pack {
    /// (D3D slot, size, offset), by slot.
    pub members: Vec<PackMember>,
    pub size: u32,
    /// Bound on their own (offset unused), by slot.
    pub own: Vec<PackMember>,
}

/// Constant buffers this large get a binding of their own, at most this
/// many a stage (a pipeline has two stages: its dynamic uniform buffers,
/// with the packs, stay within the 8 every GPU has).
pub const OWN_SIZE: u32 = 512;
pub const OWN_MAX: usize = 3;

#[derive(Clone, Copy, Debug)]
pub struct PackMember {
    pub slot: u32,
    pub size: u32,
    pub offset: u32,
}

impl Pack {
    /// The constants group's bindings, in binding order: (binding, size).
    pub fn bindings(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        std::iter::once((PACKED_CBUFFERS, self.size)).chain(self.own.iter().map(|m| (m.slot, m.size)))
    }
}

/// One translated shader, ready to use.
pub struct Program {
    /// Unique per program (keys of caches).
    pub id: u32,
    pub hash: u64,
    pub stage: Stage,
    pub label: String,
    /// The WGSL as compiled (constant buffers packed).
    pub text: Wgsl,
    /// Modules made from it, kept while in use (see [`Modules`]); shared by
    /// the copies of a program with the same text.
    pub modules: Arc<Modules>,
    /// The stage's bindings, by binding number.
    pub bindings: Vec<Binding>,
    /// Layout of the stage's group (compute: per pipeline, see ops.rs).
    pub layout: Option<wgpu::BindGroupLayout>,
    pub pack: Option<Pack>,
    /// Layout of its constants group (with a pack).
    pub constants: Option<wgpu::BindGroupLayout>,
    /// The special-constant table (binding 15).
    pub consts: Option<wgpu::Buffer>,
    /// Pixel shader: colour outputs written (bit per location).
    pub outputs: u8,
    /// Pixel shader: interpolation attributes of its inputs, by location.
    pub interpolation: Vec<(u32, String)>,
    /// Vertex shader: reads vertex_index / instance_index.
    pub vertex_index: bool,
    pub instance_index: bool,
    pub clip_distances: bool,
    /// Pixel shader: writes `frag_depth` (D3D's SV_Depth).
    pub frag_depth: bool,
    /// Pixel shader: reads `SV_Position` (see [`reads_position`]).
    pub frag_coord: bool,
    /// Indices into `bindings` of the sampled textures.
    pub textures: Vec<usize>,
    /// The same as bits by SRV slot, by how they are read.
    pub sampled: Sampled,
    /// Inter-stage locations (vertex outputs / pixel inputs).
    pub locations: Vec<u32>,
    /// (texture binding, sampler binding) sampled together.
    pub pairs: Vec<(u32, u32)>,
    /// Textures declared unfilterable (bit: binding - SRVS), for GPUs that
    /// cannot filter the 32-bit float textures bound there, or read as depth
    /// textures (`depth`); the samplers used with them are non-filtering.
    pub unfiltered: u128,
    /// Textures read as depth textures (bit: binding - SRVS): depth buffers
    /// the shader reads as floats, read directly (see [`with_depth_reads`]).
    pub depth: u128,
    /// This pixel program draws the HUD's normalized minimap mask; its
    /// blit vertex stage needs normalized coordinates converted to clip space.
    pub hud_mask: bool,
    mask: OnceLock<Option<Arc<Program>>>,
    /// Copies for other bindings, made once each: (unfiltered, depth).
    copies: Mutex<Vec<((u128, u128), Arc<Program>)>>,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

static NEXT_PROGRAM: AtomicU32 = AtomicU32::new(1);

pub fn next_program_id() -> u32 {
    NEXT_PROGRAM.fetch_add(1, Ordering::Relaxed)
}

/// Shader modules made from a program's text, by variant key ("" for the
/// text itself). wgpu's Metal and Vulkan backends keep each module's naga
/// IR, ~470 KB for the game's larger shaders, while it lives; a module is
/// needed only to make pipelines, so idle ones are released and made again
/// from the text when a new pipeline needs them (on the compile threads).
#[derive(Default)]
pub struct Modules(Mutex<HashMap<String, (Option<wgpu::ShaderModule>, u64)>>);

fn now_secs() -> u64 {
    static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
    START.elapsed().as_secs()
}

impl Modules {
    pub fn with(module: wgpu::ShaderModule) -> Modules {
        Modules(Mutex::new(HashMap::from([(String::new(), (Some(module), now_secs()))])))
    }

    /// The module for `key`, made by `make` if missing. Failures are
    /// remembered (not retried).
    fn get_or_make(&self, key: &str, make: impl FnOnce() -> Option<wgpu::ShaderModule>) -> Option<wgpu::ShaderModule> {
        if let Some((module, used)) = self.0.lock().unwrap().get_mut(key) {
            *used = now_secs();
            return module.clone();
        }
        // Made outside the lock: compiling takes a while.
        let module = make();
        self.0.lock().unwrap().insert(key.to_owned(), (module.clone(), now_secs()));
        module
    }

    /// Takes out the modules unused for `idle` seconds (until `out` holds
    /// `max`). Dropping them frees their naga IR, which takes a while for
    /// thousands: the caller does it off the recording thread.
    pub fn take_idle(&self, idle: u64, max: usize, out: &mut Vec<wgpu::ShaderModule>) {
        let now = now_secs();
        let mut map = self.0.lock().unwrap();
        // (A module may have been used after `now`, while this waited.)
        let idle_keys: Vec<String> = map
            .iter()
            .filter(|(_, (module, used))| module.is_some() && now.saturating_sub(*used) >= idle)
            .map(|(k, _)| k.clone())
            .take(max.saturating_sub(out.len()))
            .collect();
        for key in idle_keys {
            if let Some((Some(module), _)) = map.remove(&key) {
                out.push(module);
            }
        }
    }
}

/// The translated shaders on disk.
pub struct Library {
    dir: PathBuf,
    index: HashMap<u64, Entry>,
    /// Programs by hash: prepared once, shared by every shader object with
    /// that hash. `None`: not usable.
    programs: Mutex<HashMap<u64, Arc<OnceLock<Option<Arc<Program>>>>>>,
    /// Hashes of the programs asked for by name (a scan of the index each
    /// time otherwise: the pause map asks at every draw).
    names: Mutex<HashMap<String, Option<u64>>>,
    /// Special-constant tables by content: ~1800 programs have one, of 17
    /// different ones; a buffer each took 6 MiB on a Mac (3.5 KiB apiece).
    tables: Mutex<HashMap<Vec<u64>, wgpu::Buffer>>,
}

struct Entry {
    label: String,
    ok: bool,
    consts: bool,
    error: Option<String>,
}

impl Library {
    pub fn open(dir: &Path) -> Result<Library> {
        let path = dir.join("index.json");
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let json: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut index = HashMap::new();
        for (key, v) in json.as_object().context("index.json is not an object")? {
            let Ok(hash) = u64::from_str_radix(key, 16) else {
                continue;
            };
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
            index.insert(
                hash,
                Entry {
                    label: format!("{}::{}", s("effect"), s("program")),
                    ok: v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
                    consts: v.get("consts").is_some(),
                    error: v.get("error").and_then(|x| x.as_str()).map(String::from),
                },
            );
        }
        log::info!("gpu: {} translated shaders in {}", index.len(), dir.display());
        Ok(Library {
            dir: dir.to_owned(),
            index,
            programs: Mutex::default(),
            names: Mutex::default(),
            tables: Mutex::default(),
        })
    }

    pub fn label(&self, hash: u64) -> String {
        self.index.get(&hash).map_or_else(|| format!("{hash:016x}"), |e| e.label.clone())
    }

    /// A named translated program, prepared once like engine-created shaders.
    pub fn named(&self, device: &wgpu::Device, label: &str, stage: Stage) -> Option<Arc<Program>> {
        let mut names = self.names.lock().unwrap();
        let hash = match names.get(label) {
            Some(&hash) => hash,
            None => {
                let hash = self.index.iter().find(|(_, e)| e.label == label).map(|(h, _)| *h);
                names.insert(label.to_owned(), hash);
                hash
            }
        }?;
        drop(names);
        self.slot(hash).get_or_init(|| self.prepare(device, hash, stage)).clone()
    }

    /// Takes out, to be dropped elsewhere, up to `max` shader modules
    /// unused for `idle` seconds.
    pub fn take_idle_modules(&self, idle: u64, max: usize, out: &mut Vec<wgpu::ShaderModule>) {
        let programs: Vec<Arc<Program>> =
            self.programs.lock().unwrap().values().filter_map(|slot| slot.get().cloned().flatten()).collect();
        for p in programs {
            if out.len() >= max {
                break;
            }
            p.take_idle_modules(idle, max, out);
        }
    }

    /// The slot of a program: filled by [`Library::prepare`], once.
    pub fn slot(&self, hash: u64) -> Arc<OnceLock<Option<Arc<Program>>>> {
        self.programs.lock().unwrap().entry(hash).or_default().clone()
    }

    /// Loads, adapts and compiles a shader (any thread). Errors are logged.
    pub fn prepare(&self, device: &wgpu::Device, hash: u64, stage: Stage) -> Option<Arc<Program>> {
        match self.load(device, hash, stage) {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                log::warn!("gpu: {} shader {hash:016x} ({}) unusable: {e:#}", stage_name(stage), self.label(hash));
                None
            }
        }
    }

    fn load(&self, device: &wgpu::Device, hash: u64, stage: Stage) -> Result<Program> {
        let entry = self.index.get(&hash).context("not in the shader index")?;
        if !entry.ok {
            bail!("not translated: {}", entry.error.as_deref().unwrap_or("?"));
        }
        let name = format!("{hash:016x}");
        let source = std::fs::read_to_string(self.dir.join(format!("{name}.wgsl")))?;
        let source = if stage == Stage::Pixel && entry.label == "radar::PS_TexturedUnlitAA" {
            with_radar_coverage(&source).context("unexpected radar edge input")?
        } else {
            source
        };
        let source = match stage {
            Stage::Pixel => with_opaque_sample_inputs(&source).unwrap_or(source),
            Stage::Compute if device.features().contains(wgpu::Features::SUBGROUP) => {
                with_subgroup_reduction(&source).unwrap_or(source)
            }
            _ => source,
        };
        // Two stages a pipeline, each with its pack and own buffers.
        let dynamic = device.limits().max_dynamic_uniform_buffers_per_pipeline_layout as usize;
        let (text, pack) = pack_uniforms_owning(&source, OWN_MAX.min((dynamic / 2).saturating_sub(1)));
        let clip_distances = text.contains("@builtin(clip_distances)");
        let text = if clip_distances && !device.features().contains(wgpu::Features::CLIP_DISTANCES) {
            without_clip_distances(&text).context("unexpected clip distances output")?
        } else {
            text
        };
        if let Some(pack) = &pack {
            let limit = device.limits().max_uniform_buffer_binding_size as u32;
            if pack.size > limit || pack.own.iter().any(|m| m.size > limit) {
                bail!("constant buffers of {} bytes, over the uniform binding limit", pack.size);
            }
        }
        // Validated now, made again when a pipeline needs it: the engine
        // creates all its shaders (thousands) while loading.
        drop(create_module(device, &entry.label, &text)?);
        let modules = Arc::new(Modules::default());
        let consts = if entry.consts {
            let json = std::fs::read_to_string(self.dir.join(format!("{name}.consts.json")))?;
            let values: Vec<u64> = serde_json::from_str(&json)?;
            let mut tables = self.tables.lock().unwrap();
            let table = tables.entry(values).or_insert_with_key(|values| {
                // One u32 per 16-byte slot (x of a uvec4).
                let mut data = vec![0u32; values.len().max(1) * 4];
                for (i, v) in values.iter().enumerate() {
                    data[i * 4] = *v as u32;
                }
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("constant table"),
                    contents: bytemuck::cast_slice(&data),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
            });
            Some(table.clone())
        } else {
            None
        };

        let bindings = stage_bindings(&text, stage);
        let (textures, sampled) = sampled_textures(&bindings);
        let (outputs, interpolation, locations) = match stage {
            Stage::Pixel => (fragment_outputs(&text), interpolations(&text), ps_input_locations(&text)),
            Stage::Vertex => (0, Vec::new(), vs_output_locations(&text)),
            Stage::Compute => (0, Vec::new(), Vec::new()),
        };
        let mut program = Program {
            id: next_program_id(),
            hash,
            stage,
            label: entry.label.clone(),
            modules,
            bindings,
            layout: None,
            pack,
            constants: None,
            consts,
            outputs,
            interpolation,
            vertex_index: RE_VERTEX_INDEX.is_match(&text),
            instance_index: RE_INSTANCE_INDEX.is_match(&text),
            clip_distances,
            frag_depth: text.contains("@builtin(frag_depth)"),
            frag_coord: reads_position(&text),
            textures,
            sampled,
            locations,
            pairs: sampling_pairs(&text),
            unfiltered: 0,
            depth: 0,
            hud_mask: false,
            mask: OnceLock::new(),
            copies: Mutex::default(),
            text: Wgsl::new(&text),
        };
        program.layout = program.make_layout(device);
        program.constants = program.make_constants_layout(device);
        Ok(program)
    }
}

/// A program's sampled textures, bits by SRV slot (binding - SRVS).
#[derive(Clone, Copy, Debug, Default)]
pub struct Sampled {
    /// Read as floats, single-sampled.
    pub float: u128,
    /// Read as colours (floats or integers).
    pub color: u128,
    /// Read as depth.
    pub depth: u128,
}

/// The sampled textures among `bindings`: their indices, and as bits.
fn sampled_textures(bindings: &[Binding]) -> (Vec<usize>, Sampled) {
    let (mut textures, mut sampled) = (Vec::new(), Sampled::default());
    for (i, b) in bindings.iter().enumerate() {
        let Kind::Texture { sample, multisampled, .. } = b.kind else { continue };
        textures.push(i);
        let bit = 1u128 << (b.binding - SRVS);
        if sample == Sample::Depth {
            sampled.depth |= bit;
        } else {
            sampled.color |= bit;
        }
        if sample == Sample::Float && !multisampled {
            sampled.float |= bit;
        }
    }
    (textures, sampled)
}

/// A stage's resource bindings (its group), by binding number.
pub fn stage_bindings(text: &str, stage: Stage) -> Vec<Binding> {
    let group = stage.group();
    let mut bindings: Vec<Binding> =
        reflect_bindings(text).into_iter().filter(|(g, _)| *g == group).map(|(_, b)| b).collect();
    bindings.sort_by_key(|b| b.binding);
    bindings
}

pub fn stage_name(stage: Stage) -> &'static str {
    ["vs", "ps", "cs"][stage as usize]
}

/// Compiles WGSL; a module that fails validation is an error (an invalid
/// module would invalidate every pipeline and command buffer using it).
pub fn create_module(device: &wgpu::Device, label: &str, text: &str) -> Result<wgpu::ShaderModule> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    // The translated code is trusted: no loop bounding (loops come from D3D
    // bytecode). Bounds checks stay: D3D reads zeros out of bounds.
    let checks = wgpu::ShaderRuntimeChecks { force_loop_bounding: false, ..wgpu::ShaderRuntimeChecks::checked() };
    let module = unsafe {
        device.create_shader_module_trusted(
            wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(text.into()) },
            checks,
        )
    };
    match pollster::block_on(scope.pop()) {
        None => Ok(module),
        Some(e) => bail!("{e}"),
    }
}

impl Program {
    /// The same program under a new id (with its own pipelines and bind
    /// groups); the caller changes what differs.
    pub fn copy(&self) -> Program {
        Program {
            id: next_program_id(),
            hash: self.hash,
            stage: self.stage,
            label: self.label.clone(),
            text: self.text.clone(),
            modules: self.modules.clone(),
            bindings: self.bindings.clone(),
            layout: self.layout.clone(),
            pack: self.pack.clone(),
            constants: self.constants.clone(),
            consts: self.consts.clone(),
            outputs: self.outputs,
            interpolation: self.interpolation.clone(),
            vertex_index: self.vertex_index,
            instance_index: self.instance_index,
            clip_distances: self.clip_distances,
            frag_depth: self.frag_depth,
            frag_coord: self.frag_coord,
            textures: self.textures.clone(),
            sampled: self.sampled,
            locations: self.locations.clone(),
            pairs: self.pairs.clone(),
            unfiltered: self.unfiltered,
            depth: self.depth,
            hud_mask: self.hud_mask,
            mask: OnceLock::new(),
            copies: Mutex::default(),
        }
    }

    /// The HUD mask reads a filtered neighbourhood of the radar texture.
    /// The original program remains available for pause-map exploration.
    pub fn for_hud_mask(self: &Arc<Self>, device: &wgpu::Device) -> Option<Arc<Program>> {
        self.mask
            .get_or_init(|| {
                let (text, pack) = pack_uniforms(include_str!("minimap_mask.wgsl"));
                let module = create_module(device, "minimap mask", &text).ok()?;
                let mut p = self.copy();
                p.hud_mask = true;
                p.pack = pack;
                p.bindings = stage_bindings(&text, p.stage);
                (p.textures, p.sampled) = sampled_textures(&p.bindings);
                p.pairs = sampling_pairs(&text);
                p.locations = ps_input_locations(&text);
                p.interpolation = interpolations(&text);
                p.outputs = fragment_outputs(&text);
                p.frag_depth = text.contains("@builtin(frag_depth)");
                p.text = Wgsl::new(&text);
                p.modules = Arc::new(Modules::with(module));
                p.layout = p.make_layout(device);
                p.constants = p.make_constants_layout(device);
                Some(Arc::new(p))
            })
            .clone()
    }

    /// Whether the sampler at `binding` is used with an unfilterable texture.
    pub fn nonfiltering(&self, binding: u32) -> bool {
        self.pairs.iter().any(|&(t, s)| s == binding && self.unfiltered >> (t - SRVS) & 1 != 0)
    }

    /// The layout entries of the stage's group.
    pub fn layout_entries(&self) -> Vec<wgpu::BindGroupLayoutEntry> {
        self.bindings.iter().map(|b| self.layout_entry(b)).collect()
    }

    /// The layout entry of `b`, one of its bindings: as declared, with the
    /// textures and samplers this copy does not filter.
    pub fn layout_entry(&self, b: &Binding) -> wgpu::BindGroupLayoutEntry {
        use wgpu::{BindingType, SamplerBindingType, TextureSampleType};
        let mut entry = b.layout_entry(self.stage);
        match &mut entry.ty {
            BindingType::Texture { sample_type: TextureSampleType::Float { filterable }, .. }
                if self.unfiltered >> (b.binding - SRVS) & 1 != 0 =>
            {
                *filterable = false
            }
            BindingType::Sampler(ty @ SamplerBindingType::Filtering) if self.nonfiltering(b.binding) => {
                *ty = SamplerBindingType::NonFiltering
            }
            _ => {}
        }
        entry
    }

    /// The constants group's layout: dynamic offsets, sizes as bound.
    fn make_constants_layout(&self, device: &wgpu::Device) -> Option<wgpu::BindGroupLayout> {
        let entries: Vec<_> = self
            .pack
            .as_ref()?
            .bindings()
            .map(|(binding, _)| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: self.stage.visibility(),
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        Some(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &entries }))
    }

    /// The group's layout (compute: per pipeline, see ops.rs).
    fn make_layout(&self, device: &wgpu::Device) -> Option<wgpu::BindGroupLayout> {
        (self.stage != Stage::Compute && !self.bindings.is_empty()).then(|| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(&self.label),
                entries: &self.layout_entries(),
            })
        })
    }

    /// This program for what is bound: the textures in `unfiltered`
    /// declared unfilterable (GPUs that cannot filter 32-bit float textures),
    /// those in `depth` read as depth textures. Made once per pair of sets;
    /// when the depth reads cannot be rewritten, it has none (`depth` is 0:
    /// the renderer copies those depth buffers to float textures instead).
    pub fn for_bindings(self: &Arc<Self>, device: &wgpu::Device, unfiltered: u128, depth: u128) -> Arc<Program> {
        if (unfiltered, depth) == (self.unfiltered, self.depth) {
            return self.clone();
        }
        let key = (unfiltered, depth);
        if let Some((_, p)) = self.copies.lock().unwrap().iter().find(|(k, _)| *k == key) {
            return p.clone();
        }
        let mut p = self.copy();
        p.unfiltered = unfiltered | depth;
        if depth != 0 {
            let module =
                with_depth_reads(&self.text.get(), depth).ok_or_else(|| anyhow::anyhow!("not rewritable")).and_then(|text| {
                    create_module(device, &format!("{} [depth reads]", self.label), &text).map(|m| (text, m))
                });
            match module {
                Ok((text, module)) => {
                    p.bindings = stage_bindings(&text, self.stage);
                    (p.textures, p.sampled) = sampled_textures(&p.bindings);
                    p.text = Wgsl::new(&text);
                    p.modules = Arc::new(Modules::with(module));
                    p.depth = depth;
                }
                Err(e) => {
                    crate::log_once(&format!("depth{}", self.label), || {
                        format!("{}: depth read as float through a copy ({e:#})", self.label)
                    });
                    let p = self.for_bindings(device, unfiltered, 0);
                    self.copies.lock().unwrap().push((key, p.clone()));
                    return p;
                }
            }
        }
        p.layout = p.make_layout(device);
        let p = Arc::new(p);
        self.copies.lock().unwrap().push((key, p.clone()));
        p
    }

    /// The program's module (made again if released while idle).
    pub fn module(&self, device: &wgpu::Device) -> Option<wgpu::ShaderModule> {
        self.modules.get_or_make("", || {
            create_module(device, &self.label, &self.text.get()).map_err(|e| log::warn!("gpu: {}: {e:#}", self.label)).ok()
        })
    }

    /// A variant of this program's module (`make` rewrites the text).
    pub fn variant(
        &self,
        device: &wgpu::Device,
        key: &str,
        make: impl FnOnce(&str) -> String,
    ) -> Option<wgpu::ShaderModule> {
        self.modules.get_or_make(key, || {
            let text = make(&self.text.get());
            let label = format!("{} [{key}]", self.label);
            create_module(device, &label, &text).map_err(|e| log::warn!("gpu: variant {label}: {e:#}")).ok()
        })
    }

    /// Takes out the modules of this program and its copies unused for
    /// `idle` seconds (until `out` holds `max`).
    pub fn take_idle_modules(&self, idle: u64, max: usize, out: &mut Vec<wgpu::ShaderModule>) {
        self.modules.take_idle(idle, max, out);
        for (_, p) in self.copies.lock().unwrap().iter() {
            p.modules.take_idle(idle, max, out);
        }
        if let Some(Some(p)) = self.mask.get() {
            p.take_idle_modules(idle, max, out);
        }
    }
}

/// A program's WGSL, compressed (LZ4) and shared by its copies. It is
/// needed again only to make a module released while idle, or a variant;
/// whole, the game's thousands of programs (often 50-130 KB each) took
/// about 150 MB.
#[derive(Clone)]
pub struct Wgsl(Arc<[u8]>);

impl Wgsl {
    pub fn new(text: &str) -> Wgsl {
        Wgsl(lz4_flex::compress_prepend_size(text.as_bytes()).into())
    }

    pub fn get(&self) -> String {
        let bytes = lz4_flex::decompress_size_prepended(&self.0).expect("compressed by Wgsl::new");
        String::from_utf8(bytes).expect("compressed from a str")
    }
}

// ---- WGSL text: the translator emits plain, regular declarations -------------

// ASCII classes (`(?-u)`: the text is ASCII). Unicode `\w`, `\s`, `\d` and
// `\b` made the caches each thread keeps for these 3.3 times larger: 1.4 MiB
// a thread, 23 MiB in all. A negated class stays Unicode (`(?u:[^;])`), as
// a `&str` regex needs.
macro_rules! re {
    ($name:ident, $re:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new(concat!("(?-u)", $re)).unwrap());
    };
}

re!(RE_DECL, r"@group\((\d+)u?\)\s*@binding\((\d+)u?\)\s*var(?:<((?u:[^>])*)>)?\s+(\w+)\s*:\s*((?u:[^;])+);");
re!(RE_UNIFORM, r"@group\((\d+)u?\)\s*@binding\((\d+)u?\)\s*var<uniform>\s+(\w+)\s*:\s*(\w+)\s*;");
re!(RE_ARRAY_LEN, r"array<(?u:[^>])*>,\s*(\d+)u?>");
re!(RE_FIRST_ITEM, r"\n(?:@\w+|fn)\s");
re!(RE_TEXTURE_TYPE, r"^texture_(\w+?)(?:<((?u:[^>])*)>)?$");
re!(RE_FRAGMENT_MAIN, r"@fragment\s*fn\s+main\s*\((?:(?u:[^()])|\((?u:[^)])*\))*\)\s*->\s*((?u:[^{])+)\{");
re!(RE_LOCATION, r"@location\((\d+)u?\)");
re!(RE_INTERPOLATE, r"@location\((\d+)u?\)\s*@interpolate\(((?u:[^)])*)\)");
re!(RE_VS_MAIN, r"@vertex\s*fn\s+main\s*\((?u:[\s\S])*?\)\s*->\s*(\w+)\s*\{");
re!(RE_PS_MAIN, r"(@fragment\s*fn\s+main\s*\()((?u:[\s\S])*?)(\)\s*->)");
re!(RE_VS_OUTPUT_MEMBER, r"@location\((\d+)u\)(\s*\n\s*\w+\s*:)");
re!(RE_MAIN_INNER_CALL, r"(\n\s*main_inner\((?u:[^;])*\);)");
re!(RE_PS_MAIN_BODY, r"(@fragment\s*fn\s+main\s*\()((?:(?u:[^()])|\((?u:[^)])*\))*)(\)(?u:[^{])*\{)");
re!(RE_CLIP_ENABLE, r"enable\s+clip_distances\s*;\s*");
re!(RE_CLIP_MEMBER, r"@builtin\(clip_distances\)\s*(\w+)\s*:\s*array<f32,\s*\d+u?>\s*,?\s*");
re!(RE_DEPTH_MEMBER, r"@builtin\(frag_depth\)\s*(\w+)\s*:\s*f32\s*,?\s*");
re!(
    RE_DEPTH_ONLY_MAIN,
    r"(@fragment\s*fn\s+main\s*\((?:(?u:[^()])|\((?u:[^)])*\))*\))\s*->\s*@builtin\(frag_depth\)\s*f32\s*\{((?u:[^{}])*?)\n\s*return\s+\w+;"
);
re!(RE_SAMPLE_INPUT, r"^(@location\(\d+u?\)\s*@interpolate\(perspective,\s*sample\)\s*)(\w+)(\s*:\s*vec4<f32>)$");
re!(RE_SAMPLE_INDEX, r"^@builtin\(sample_index\)\s*(\w+)\s*:");
re!(RE_VERTEX_INDEX, r"\bvertex_index\b");
re!(RE_INSTANCE_INDEX, r"\binstance_index\b");

/// Whether a pixel program reads D3D's `SV_Position` (WGSL's
/// `@builtin(position)` as a fragment input), taken from its entry point's
/// parameters and not from the whole text: a vertex output or a helper
/// function's parameter is not the fragment stage reading its own pixel.
///
/// Its value is in pixels of the target being drawn into, so a program that
/// derives a texture coordinate, a dither or a `textureLoad` from it computes
/// something else when the draw is redirected to a differently sized target
/// (see [`crate::census`]).
pub fn reads_position(text: &str) -> bool {
    RE_PS_MAIN_BODY.captures(text).is_some_and(|c| c[2].contains("@builtin(position)"))
}

fn struct_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let re = Regex::new(&format!(r"struct\s+{}\s*\{{([^}}]*)\}}", regex::escape(name))).ok()?;
    re.captures(text).map(|c| c.get(1).unwrap().as_str())
}

/// Merges the constant buffers (every `var<uniform>` but the constant table)
/// into one struct at binding 0; members keep their names, 16-byte aligned,
/// in slot order. The runtime copies the bound buffers back to back. The
/// largest keep their own declaration ([`Pack::own`]); at least one is
/// packed. They go to the stage's constants group ([`CONSTANTS_GROUPS`]
/// above its own).
pub fn pack_uniforms(text: &str) -> (String, Option<Pack>) {
    pack_uniforms_owning(text, OWN_MAX)
}

/// [`pack_uniforms`] with at most `own_max` constant buffers on their own.
pub fn pack_uniforms_owning(text: &str, own_max: usize) -> (String, Option<Pack>) {
    let mut members = Vec::new();
    let mut group = 0;
    let mut names = Vec::new();
    let rows = |ty: &str| {
        struct_body(text, ty).and_then(|b| RE_ARRAY_LEN.captures(b)).map_or(1, |a| a[1].parse::<u32>().unwrap())
    };
    // The largest, slot 0 (the pack's binding) aside.
    let mut sizes: Vec<(u32, u32)> = RE_UNIFORM
        .captures_iter(text)
        .filter_map(|c| {
            let slot: u32 = c[2].parse().unwrap();
            (slot != CONSTANT_TABLE).then(|| (rows(&c[4]) * 16, slot))
        })
        .collect();
    sizes.sort_by(|a, b| b.cmp(a));
    let own: Vec<(u32, u32)> = sizes
        .iter()
        .copied()
        // Not `rage_matrices` (slot 1): the jitter goes into each vertex
        // stage's copy (see `scene.rs`).
        .filter(|&(size, slot)| size >= OWN_SIZE && slot != PACKED_CBUFFERS && slot != 1)
        .take(own_max.min(sizes.len().saturating_sub(1)))
        .collect();
    let out = RE_UNIFORM.replace_all(text, |c: &regex::Captures| {
        let slot: u32 = c[2].parse().unwrap();
        group = c[1].parse().unwrap();
        if slot == CONSTANT_TABLE {
            return c[0].to_owned();
        }
        if own.iter().any(|o| o.1 == slot) {
            let (name, ty, group) = (&c[3], &c[4], group + CONSTANTS_GROUPS);
            return format!("@group({group}u) @binding({slot}u) var<uniform> {name} : {ty};");
        }
        let ty = &c[4];
        members.push((slot, c[3].to_owned(), ty.to_owned(), rows(ty) * 16));
        String::new()
    });
    if members.is_empty() {
        return (text.to_owned(), None);
    }
    let mut own: Vec<PackMember> = own.into_iter().map(|(size, slot)| PackMember { slot, size, offset: 0 }).collect();
    own.sort_by_key(|m| m.slot);
    members.sort_by_key(|m| m.0);
    let mut out = out.into_owned();
    for (_, name, _, _) in &members {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(name))).unwrap();
        out = re.replace_all(&out, format!("cbpack_v.{name}").as_str()).into_owned();
        names.push(name.clone());
    }
    let mut decl = String::from("struct cbpack_t {\n");
    let mut offset = 0;
    let mut pack = Vec::new();
    for (slot, name, ty, size) in &members {
        decl += &format!("  {name} : {ty},\n");
        pack.push(PackMember { slot: *slot, size: *size, offset });
        offset += size;
    }
    let group = group + CONSTANTS_GROUPS;
    decl += &format!("}}\n@group({group}u) @binding({PACKED_CBUFFERS}u) var<uniform> cbpack_v : cbpack_t;\n");
    // After the member types, before the first function or entry point.
    match RE_FIRST_ITEM.find(&out).map(|m| m.start()) {
        Some(at) => out.insert_str(at, &format!("\n{decl}")),
        None => out.push_str(&decl),
    }
    (out, Some(Pack { members: pack, size: offset, own }))
}

fn view_dim(name: &str) -> Option<wgpu::TextureViewDimension> {
    use wgpu::TextureViewDimension as D;
    Some(match name {
        "1d" => D::D1,
        "2d" => D::D2,
        "2d_array" => D::D2Array,
        "3d" => D::D3,
        "cube" => D::Cube,
        "cube_array" => D::CubeArray,
        _ => return None,
    })
}

/// Every resource declaration: (group, binding).
pub fn reflect_bindings(text: &str) -> Vec<(u32, Binding)> {
    let mut out = Vec::new();
    for c in RE_DECL.captures_iter(text) {
        let group: u32 = c[1].parse().unwrap();
        let binding: u32 = c[2].parse().unwrap();
        let space: String = c.get(3).map_or("", |m| m.as_str()).chars().filter(|c| !c.is_whitespace()).collect();
        let ty = c[5].trim();
        let kind = if space == "uniform" {
            Kind::Uniform
        } else if space.starts_with("storage") {
            Kind::Storage { read_only: !space.contains("read_write") }
        } else if ty == "sampler" {
            Kind::Sampler { comparison: false }
        } else if ty == "sampler_comparison" {
            Kind::Sampler { comparison: true }
        } else if let Some(t) = RE_TEXTURE_TYPE.captures(ty) {
            let name = &t[1];
            let arg = t.get(2).map_or("f32", |m| m.as_str());
            if let Some(dim) = name.strip_prefix("storage_") {
                let mut parts = arg.split(',').map(str::trim);
                let Some(format) = parts.next().and_then(format::storage) else {
                    log::warn!("gpu: storage texture format {arg} not supported");
                    continue;
                };
                let access = match parts.next() {
                    Some("read") => wgpu::StorageTextureAccess::ReadOnly,
                    Some("read_write") => wgpu::StorageTextureAccess::ReadWrite,
                    _ => wgpu::StorageTextureAccess::WriteOnly,
                };
                Kind::StorageTexture { access, format, dim: view_dim(dim).unwrap_or(wgpu::TextureViewDimension::D2) }
            } else if let Some(rest) = name.strip_prefix("depth_") {
                let (ms, dim) = match rest.strip_prefix("multisampled_") {
                    Some(d) => (true, d),
                    None => (false, rest),
                };
                Kind::Texture {
                    sample: Sample::Depth,
                    dim: view_dim(dim).unwrap_or(wgpu::TextureViewDimension::D2),
                    multisampled: ms,
                }
            } else {
                let sample = match arg {
                    "u32" => Sample::Uint,
                    "i32" => Sample::Sint,
                    _ => Sample::Float,
                };
                let (multisampled, dim) = match name.strip_prefix("multisampled_") {
                    Some(dim) => (true, dim),
                    None => (false, name),
                };
                Kind::Texture { sample, dim: view_dim(dim).unwrap_or(wgpu::TextureViewDimension::D2), multisampled }
            }
        } else {
            log::warn!("gpu: unsupported declaration {}", &c[0]);
            continue;
        };
        out.push((group, Binding { binding, kind }));
    }
    out
}

re!(RE_SAMPLING, r"\btexture(?:Sample|Gather)\w*\(((?u:[^;])*)");

/// (texture binding, sampler binding) of every sampling call: the first two
/// arguments, or the second and third (textureGather's component first).
fn sampling_pairs(text: &str) -> Vec<(u32, u32)> {
    let bindings: HashMap<&str, u32> =
        RE_DECL.captures_iter(text).map(|c| (c.get(4).unwrap().as_str(), c[2].parse().unwrap())).collect();
    let mut pairs = Vec::new();
    for c in RE_SAMPLING.captures_iter(text) {
        let args: Vec<&str> = c[1].splitn(4, ',').map(str::trim).collect();
        for w in args.windows(2).take(2) {
            if let (Some(&t), Some(&s)) = (bindings.get(w[0]), bindings.get(w[1]))
                && (SRVS..UAVS).contains(&t)
                && (SAMPLERS..SRVS).contains(&s)
                && !pairs.contains(&(t, s))
            {
                pairs.push((t, s));
            }
        }
    }
    pairs
}

/// The colour locations `fn main` of a pixel shader returns.
fn fragment_outputs(text: &str) -> u8 {
    let Some(sig) = RE_FRAGMENT_MAIN.captures(text) else {
        return 0;
    };
    let ret = sig[1].trim();
    let locations =
        |s: &str| RE_LOCATION.captures_iter(s).fold(0u8, |m, c| m | 1 << c[1].parse::<u32>().unwrap().min(7));
    if ret.contains("@location") { locations(ret) } else { struct_body(text, ret).map_or(0, locations) }
}

fn interpolations(text: &str) -> Vec<(u32, String)> {
    RE_INTERPOLATE.captures_iter(text).map(|c| (c[1].parse().unwrap(), c[2].to_owned())).collect()
}

fn vs_output_struct(text: &str) -> Option<String> {
    RE_VS_MAIN.captures(text).map(|c| c[1].to_owned())
}

fn vs_output_locations(text: &str) -> Vec<u32> {
    let body = vs_output_struct(text).and_then(|s| struct_body(text, &s).map(String::from)).unwrap_or_default();
    RE_LOCATION.captures_iter(&body).map(|c| c[1].parse().unwrap()).collect()
}

fn ps_input_locations(text: &str) -> Vec<u32> {
    RE_PS_MAIN
        .captures(text)
        .map_or_else(Vec::new, |c| RE_LOCATION.captures_iter(&c[2]).map(|c| c[1].parse().unwrap()).collect())
}

/// Tiled lighting's depth downsample (`DepthDownsampleCS_Default` and
/// `_B`, exactly as translated: their text's FNV-1a) with its reduction made
/// of subgroup operations (see `depth_downsample.wgsl`): the same results.
fn with_subgroup_reduction(text: &str) -> Option<String> {
    const TRANSLATED: [(u64, u32); 2] = [(0x48e4fc2565a175b7, 16), (0x3231d97411b0d134, 8)];
    let fnv = text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let &(_, tile) = TRANSLATED.iter().find(|(h, _)| *h == fnv)?;
    Some(
        include_str!("depth_downsample.wgsl")
            .replace("@TILE@", &tile.to_string())
            .replace("@THREADS@", &(tile * tile).to_string()),
    )
}

/// The radar's geometry shader supplies three edge distances at location 5.
/// WebGPU has no geometry stage: use full coverage, keeping the pixel
/// shader's texture, depth and clipping calculations. Zero distances would
/// make every fragment transparent. This is specific to the radar, not a
/// default for other missing inter-stage inputs.
fn with_radar_coverage(text: &str) -> Option<String> {
    let sig = RE_PS_MAIN.captures(text)?;
    let args = split_args(&sig[2]);
    let edge = args.iter().position(|arg| arg.trim() == "@location(5u) v5 : vec4<f32>")?;
    let args: Vec<_> = args.iter().enumerate().filter(|(i, _)| *i != edge).map(|(_, arg)| arg.trim()).collect();
    let text = RE_PS_MAIN.replace(text, format!("{}{}{}", &sig[1], args.join(", "), &sig[3])).into_owned();
    if !RE_MAIN_INNER_CALL.is_match(&text) {
        return None;
    }
    Some(RE_MAIN_INNER_CALL.replace(&text, "\n  let v5 = vec4<f32>(1.0f);$1").into_owned())
}

/// HUD mask vertices are in [0, 1], with y pointing down. Other blits
/// already use clip space and must keep their original vertex program.
pub fn with_hud_coordinates(text: &str) -> Option<String> {
    let from = "o0 = vec4<f32>(v0.xyz, o0.w);";
    text.contains(from)
        .then(|| text.replace(from, "o0 = vec4<f32>(v0.x * 2.0f - 1.0f, 1.0f - v0.y * 2.0f, v0.z, o0.w);"))
}

/// D3D blend words: enabled, MIN colour/alpha operations, alpha-only writes.
/// This distinguishes world-space pause-map exploration from the HUD mask
/// even though the engine binds the same blit programs for both.
pub fn pause_map_blend(words: &[u32]) -> bool {
    words.get(2) == Some(&1) && words.get(5) == Some(&4) && words.get(8) == Some(&4) && words.get(9) == Some(&8)
}

/// Vertex shader outputs take the pixel shader's interpolation attributes
/// (WGSL requires them to match; HLSL declares them on the pixel shader only).
pub fn with_interpolation(text: &str, interpolation: &[(u32, String)]) -> String {
    RE_VS_OUTPUT_MEMBER
        .replace_all(text, |c: &regex::Captures| {
            let loc: u32 = c[1].parse().unwrap();
            match interpolation.iter().find(|(l, _)| *l == loc) {
                Some((_, attr)) => format!("@location({loc}u) @interpolate({attr}){}", &c[2]),
                None => c[0].to_owned(),
            }
        })
        .into_owned()
}

/// D3D11 alpha-to-coverage on a single-sample target is an alpha test
/// (alpha >= 0.5); the foliage shaders rely on it instead of clip().
/// `None` when the shader's first output is not an RGBA colour.
pub fn with_alpha_test(text: &str) -> Option<String> {
    if !text.contains("var<private> o0 : vec4<f32>;") || !RE_MAIN_INNER_CALL.is_match(text) {
        return None;
    }
    Some(RE_MAIN_INNER_CALL.replace(text, "$1\n  if (o0.w < 0.5f) { discard; }").into_owned())
}

/// Where the translator's vertex shaders also write their four clip
/// distances (D3D's SV_ClipDistance, which pixel shaders may read).
pub const CLIP_LOCATION: u32 = 15;

/// A vertex shader without its `clip_distances` output, for GPUs without
/// clip distances (Mali): the pixel stage clips instead, from the copy at
/// [`CLIP_LOCATION`] ([`with_clipping`]).
pub fn without_clip_distances(text: &str) -> Option<String> {
    let name = vs_output_struct(text)?;
    let text = without_output_member(text, &name, &RE_CLIP_MEMBER)?;
    Some(RE_CLIP_ENABLE.replace(&text, "").into_owned())
}

/// A pixel shader without its depth output, for a draw without a depth
/// target: D3D ignores SV_Depth there, wgpu rejects the pipeline.
pub fn without_frag_depth(text: &str) -> Option<String> {
    if let Some(c) = RE_DEPTH_ONLY_MAIN.captures(text) {
        return Some(text.replacen(&c[0], &format!("{} {{{}", &c[1], &c[2]), 1));
    }
    let name = RE_FRAGMENT_MAIN.captures(text)?[1].trim().to_owned();
    without_output_member(text, &name, &RE_DEPTH_MEMBER)
}

/// The text without the member of output struct `name` that `member`
/// matches (the field name in its first group), nor its argument in the
/// struct's constructor.
fn without_output_member(text: &str, name: &str, member: &Regex) -> Option<String> {
    let body = struct_body(text, name)?;
    let m = member.captures(body)?;
    let field = m[1].to_owned();
    let text = text.replacen(body, &body.replace(&m[0], ""), 1);
    // The output's constructor, fields in order: the member's goes.
    let call = Regex::new(&format!(r"return {}\(([^;]*)\);", regex::escape(name))).ok()?;
    let args = call.captures(&text)?.get(1)?.as_str().to_owned();
    let kept: Vec<&str> = args.split(',').map(str::trim).filter(|a| *a != field).collect();
    if kept.len() + 1 != args.split(',').count() {
        return None;
    }
    Some(call.replace(&text, format!("return {name}({});", kept.join(", ")).as_str()).into_owned())
}

/// Apple's M1 compiler (AGX G13) fails on a sample at a projected
/// per-sample input, `textureSample(t, s, v.xy / v.w)` with `v`
/// `@interpolate(perspective, sample)` ("Undefined symbols:
/// llvm.agx2.sample.itrproj"), as the MSAA light shaders do. Such inputs
/// arrive through an identity it cannot see through (sample indices are
/// below 32). `None` if the pixel shader has none.
pub fn with_opaque_sample_inputs(text: &str) -> Option<String> {
    let main = RE_PS_MAIN_BODY.captures(text)?;
    let mut params: Vec<String> =
        split_args(&main[2]).iter().map(|a| a.trim().to_owned()).filter(|a| !a.is_empty()).collect();
    let mut inputs = Vec::new();
    for p in &mut params {
        if let Some(c) = RE_SAMPLE_INPUT.captures(p) {
            inputs.push(c[2].to_owned());
            *p = format!("{}g5_{}{}", &c[1], &c[2], &c[3]);
        }
    }
    if inputs.is_empty() {
        return None;
    }
    let index = params.iter().find_map(|p| RE_SAMPLE_INDEX.captures(p).map(|c| c[1].to_owned()));
    let index = index.unwrap_or_else(|| {
        params.push("@builtin(sample_index) g5_sample : u32".to_owned());
        "g5_sample".to_owned()
    });
    let lets: String = inputs
        .iter()
        .map(|v| format!("\n  let {v} = bitcast<vec4<f32>>(bitcast<vec4<u32>>(g5_{v}) | vec4<u32>({index} >> 31u));"))
        .collect();
    let head = format!("{}{}{}{lets}", &main[1], params.join(", "), &main[3]);
    Some(text.replacen(&main[0], &head, 1))
}

/// A pixel shader that first discards the fragments outside the clip
/// distances at location `at` (see [`without_clip_distances`]); `None` for
/// a draw without one: a pixel stage that does only that.
pub fn with_clipping(text: Option<&str>, at: u32) -> Option<String> {
    let test = |v: &str| format!("\n  if (any({v} < vec4<f32>(0.0f))) {{\n    discard;\n  }}");
    let Some(text) = text else {
        return Some(format!("@fragment\nfn main(@location({at}u) clip : vec4<f32>) {{{}\n}}\n", test("clip")));
    };
    let main = RE_PS_MAIN_BODY.captures(text)?;
    let params = &main[2];
    let read = Regex::new(&format!(r"@location\({at}u\)\s*(\w+)\s*:")).ok()?;
    let (params, var) = match read.captures(params) {
        Some(c) => (params.to_owned(), c[1].to_owned()),
        None if params.trim().is_empty() => (format!("@location({at}u) clip : vec4<f32>"), "clip".to_owned()),
        None => (format!("{params}, @location({at}u) clip : vec4<f32>"), "clip".to_owned()),
    };
    let head = format!("{}{params}{}{}", &main[1], &main[3], test(&var));
    Some(text.replacen(&main[0], &head, 1))
}

/// Moves inter-stage locations (`map`: from -> to) in a vertex shader's
/// output struct or a pixel shader's inputs.
pub fn with_locations(text: &str, stage: Stage, map: &[(u32, u32)]) -> String {
    let swap = |s: &str| {
        RE_LOCATION
            .replace_all(s, |c: &regex::Captures| {
                let loc: u32 = c[1].parse().unwrap();
                match map.iter().find(|(from, _)| *from == loc) {
                    Some((_, to)) => format!("@location({to}u)"),
                    None => c[0].to_owned(),
                }
            })
            .into_owned()
    };
    match stage {
        Stage::Vertex => {
            let Some(name) = vs_output_struct(text) else {
                return text.to_owned();
            };
            let re = Regex::new(&format!(r"(struct\s+{}\s*\{{)([^}}]*)(\}})", regex::escape(&name))).unwrap();
            re.replace(text, |c: &regex::Captures| format!("{}{}{}", &c[1], swap(&c[2]), &c[3])).into_owned()
        }
        _ => RE_PS_MAIN.replace(text, |c: &regex::Captures| format!("{}{}{}", &c[1], swap(&c[2]), &c[3])).into_owned(),
    }
}

/// Inter-stage locations at or above the device's limit moved to free low
/// locations (vertex outputs and the pixel inputs that read them), or `None`
/// when nothing has to move or there is no room.
pub fn remap_locations(vs: &Program, ps: Option<&Program>, max: u32) -> Option<Vec<(u32, u32)>> {
    let limit = if vs.clip_distances { max.saturating_sub(1) } else { max };
    if !vs.locations.iter().any(|&l| l >= limit) {
        return None;
    }
    let ins = ps.map_or(&[][..], |p| &p.locations);
    let mut used: Vec<u32> = vs.locations.iter().chain(ins).copied().filter(|&l| l < limit).collect();
    let mut map = Vec::new();
    let mut free = 0;
    for &l in &vs.locations {
        if l < limit {
            continue;
        }
        while used.contains(&free) {
            free += 1;
        }
        if free >= limit {
            crate::log_once(&format!("remap{}", vs.label), || {
                format!("{}: vertex outputs do not fit below location {limit}", vs.label)
            });
            return None;
        }
        map.push((l, free));
        used.push(free);
    }
    Some(map)
}

/// The text with the textures in `depth` (bit: binding - SRVS) declared as
/// depth textures, read the way D3D reads a depth format through a float
/// view, (d, 0, 0, 1): no copy of the depth buffer into a float texture.
/// `None` if one is read in a way depth textures cannot be (gradients, bias).
pub fn with_depth_reads(text: &str, depth: u128) -> Option<String> {
    let mut names = Vec::new();
    let mut out = text.to_owned();
    for c in RE_DECL.captures_iter(text) {
        let binding: u32 = c[2].parse().unwrap();
        if !(SRVS..UAVS).contains(&binding) || depth >> (binding - SRVS) & 1 == 0 {
            continue;
        }
        let ty = match c[5].trim() {
            "texture_2d<f32>" => "texture_depth_2d",
            "texture_2d_array<f32>" => "texture_depth_2d_array",
            "texture_cube<f32>" => "texture_depth_cube",
            "texture_cube_array<f32>" => "texture_depth_cube_array",
            _ => return None,
        };
        let decl = &c[0];
        out = out.replacen(decl, &decl.replacen(c[5].trim(), ty, 1), 1);
        names.push(c[4].to_owned());
    }
    if names.is_empty() {
        return None;
    }
    let mut body = rewrite_depth_calls(&out, &names)?;
    body.push_str("\nfn g5_depth4(d : f32) -> vec4<f32> {\n  return vec4<f32>(d, 0.0f, 0.0f, 1.0f);\n}\n");
    Some(body)
}

re!(RE_TEXTURE_CALL, r"\b(texture[A-Za-z]+)\(");

/// Rewrites the texture calls on `names` (see [`with_depth_reads`]).
fn rewrite_depth_calls(text: &str, names: &[String]) -> Option<String> {
    let mut out = String::with_capacity(text.len() + 256);
    let mut at = 0;
    while let Some(m) = RE_TEXTURE_CALL.captures_at(text, at) {
        let whole = m.get(0).unwrap();
        let func = m.get(1).unwrap().as_str();
        let open = whole.end();
        let close = matching_paren(text, open)?;
        let args: Vec<&str> = split_args(&text[open..close]);
        out.push_str(&text[at..whole.start()]);
        // textureGather takes the component first.
        let ti = if func == "textureGather" && args.len() == 4 { 1 } else { 0 };
        let target = args.get(ti).is_some_and(|a| names.iter().any(|n| n == a.trim()));
        if !target {
            // Not ours: copy the call, rewriting inside its arguments.
            out.push_str(&text[whole.start()..open]);
            out.push_str(&rewrite_depth_calls(&text[open..close], names)?);
            out.push(')');
            at = close + 1;
            continue;
        }
        let args: Vec<String> = args.iter().map(|a| rewrite_depth_calls(a.trim(), names)).collect::<Option<_>>()?;
        let call = match func {
            "textureSample" | "textureLoad" => format!("g5_depth4({func}({}))", args.join(", ")),
            "textureSampleLevel" => {
                // The level of a depth texture is an integer.
                let (last, rest) = args.split_last()?;
                format!("g5_depth4(textureSampleLevel({}, i32({})))", rest.join(", "), last.trim())
            }
            "textureGather" if args[0].trim() == "0i" || args[0].trim() == "0" => {
                format!("textureGather({})", args[1..].join(", "))
            }
            "textureGather" => "vec4<f32>(0.0f)".to_owned(),
            "textureDimensions" | "textureNumLevels" | "textureNumLayers" => format!("{func}({})", args.join(", ")),
            _ => return None,
        };
        out.push_str(&call);
        at = close + 1;
    }
    out.push_str(&text[at..]);
    Some(out)
}

/// The index of the `)` closing the parenthesis opened just before `open`.
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 1;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Arguments separated by top-level commas.
fn split_args(args: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, c) in args.char_indices() {
        match c {
            '(' | '<' => depth += 1,
            ')' | '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&args[start..]);
    out
}

/// A compute shader with its storage textures declared in the formats of
/// the textures bound (the translation guesses them from the component count).
pub fn with_storage_formats(text: &str, subs: &[(u32, wgpu::TextureFormat, wgpu::TextureFormat)]) -> String {
    let mut out = text.to_owned();
    for (binding, from, to) in subs {
        let (Some(from), Some(to)) = (format::wgsl_storage_name(*from), format::wgsl_storage_name(*to)) else {
            continue;
        };
        let re =
            Regex::new(&format!(r"(@binding\({binding}u?\)\s*var\s+\w+\s*:\s*texture_storage_\w+<){from}")).unwrap();
        out = re.replace(&out, format!("${{1}}{to}").as_str()).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const VS: &str = "diagnostic(off, derivative_uniformity);

var<private> r0 : vec4<f32>;

struct cb10_struct {
  tint_symbol : array<vec4<f32>, 10u>,
}

@group(0u) @binding(7u) var<uniform> cb7_7 : cb10_struct;

struct cb2_struct {
  tint_symbol : array<vec4<f32>, 3u>,
}

@group(0u) @binding(2u) var<uniform> cb2_2 : cb2_struct;

var<private> o0 : vec4<f32>;

fn main_inner(v0 : vec2<f32>) {
  o0 = vec4<f32>(v0 * cb7_7.tint_symbol[9u].xy + cb2_2.tint_symbol[0u].xy, 0.0f, 1.0f);
}

struct tint_symbol_1 {
  @builtin(position)
  o0 : vec4<f32>,
  @location(1u)
  o1 : vec4<f32>,
}

@vertex
fn main(@location(0u) v0 : vec2<f32>) -> tint_symbol_1 {
  main_inner(v0);
  return tint_symbol_1(o0, o0);
}
";

    #[test]
    fn packs_constant_buffers_by_slot() {
        let (text, pack) = pack_uniforms(VS);
        let pack = pack.unwrap();
        assert_eq!(pack.size, (3 + 10) * 16);
        assert_eq!((pack.members[0].slot, pack.members[0].offset), (2, 0));
        assert_eq!((pack.members[1].slot, pack.members[1].offset), (7, 48));
        assert!(text.contains("cbpack_v.cb7_7.tint_symbol[9u]"));
        assert!(text.contains("@group(2u) @binding(0u) var<uniform> cbpack_v : cbpack_t;"));
        assert!(!text.contains("@binding(7u)"));
        naga::front::wgsl::parse_str(&text).unwrap();
    }

    /// The depth downsample with subgroup operations gives the translation's
    /// results, bit for bit, on random depths and tiles past the edges
    /// (GLUE5_GAME_DIR, a GPU with subgroups).
    #[test]
    #[ignore]
    fn subgroup_depth_downsample_matches_the_translation() {
        let game = std::env::var_os("GLUE5_GAME_DIR").expect("GLUE5_GAME_DIR");
        let dir = glue5_runtime::layout::shader_dir(Path::new(&game));
        let gpu = crate::Device::open(&crate::instance(), None).unwrap();
        assert!(gpu.device.features().contains(wgpu::Features::SUBGROUP), "no subgroups here");
        for name in ["23a7a3c1c0d38f46", "09596507b8362aed"] {
            let original = std::fs::read_to_string(dir.join(format!("{name}.wgsl"))).unwrap();
            let consts: Vec<u64> =
                serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.consts.json"))).unwrap()).unwrap();
            let replaced = with_subgroup_reduction(&original).expect("the translation as it was");
            let tile = if name.starts_with("23") { 16 } else { 8 };
            let a = run_downsample(&gpu, &original, &consts, tile);
            let b = run_downsample(&gpu, &replaced, &consts, tile);
            assert!(a.chunks(16).any(|t| t != &a[..16]), "{name}: the same everywhere, nothing compared");
            assert_eq!(a.len(), b.len());
            assert!(a == b, "{name}: results differ");
        }
    }

    /// Runs a depth downsample over 1000x600 random depths: its output's bits.
    fn run_downsample(gpu: &crate::Device, text: &str, consts: &[u64], tile: u32) -> Vec<u8> {
        let (device, queue) = (&gpu.device, &gpu.queue);
        let (w, h) = (1000u32, 600u32);
        let (gw, gh) = (w.div_ceil(tile), h.div_ceil(tile));
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (text, pack) = pack_uniforms(text);
        let module = create_module(device, "downsample", &text).unwrap();
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        // Constant buffers: slot 10 row 3 the depth's size (as integers),
        // slot 11 row 17 the projection terms.
        let pack = pack.unwrap();
        let mut cb = vec![0u8; pack.size as usize];
        for m in &pack.members {
            let rows: Vec<[u32; 4]> = match m.slot {
                10 => vec![[0; 4], [0; 4], [0; 4], [w, h, 0, 0]],
                _ => (0..18).map(|_| [0.5f32, 1.0, 0.1, 0.002].map(f32::to_bits)).collect(),
            };
            let bytes: Vec<u8> = rows.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
            let n = bytes.len().min(m.size as usize);
            cb[m.offset as usize..m.offset as usize + n].copy_from_slice(&bytes[..n]);
        }
        let mut table = vec![0u32; consts.len() * 4];
        for (i, v) in consts.iter().enumerate() {
            table[i * 4] = *v as u32;
        }
        use wgpu::util::DeviceExt;
        let uniform = |data: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: data,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let (cb, table) = (uniform(&cb), uniform(&table.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()));
        let texture = |format, texels: Vec<u8>, (tw, th): (u32, u32), usage| {
            device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                },
                Default::default(),
                &texels,
            )
        };
        // Depths, a quarter of them far (1: the sky), and four channels of
        // anything.
        let depth: Vec<u8> = (0..w * h)
            .flat_map(|_| {
                let r = random();
                let d = if r % 4 == 0 { 1.0 } else { (r >> 40) as f32 / (1u64 << 24) as f32 };
                half::f16::from_f32(d).to_le_bytes()
            })
            .collect();
        let other: Vec<u8> = (0..w * h * 4).flat_map(|_| half::f16::from_f32((random() >> 40) as f32 / 1e3).to_le_bytes()).collect();
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING;
        let t7 = texture(wgpu::TextureFormat::R16Float, depth, (w, h), sampled);
        let t8 = texture(wgpu::TextureFormat::Rgba16Float, other, (w, h), sampled);
        let out = texture(
            wgpu::TextureFormat::Rgba32Float,
            vec![0; (gw * gh * 16) as usize],
            (gw, gh),
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let views = [&t7, &t8, &out].map(|t| t.create_view(&Default::default()));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 15, resource: table.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 39, resource: wgpu::BindingResource::TextureView(&views[0]) },
                wgpu::BindGroupEntry { binding: 40, resource: wgpu::BindingResource::TextureView(&views[1]) },
                wgpu::BindGroupEntry { binding: 161, resource: wgpu::BindingResource::TextureView(&views[2]) },
            ],
        });
        let constants = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(Stage::Compute.constants_group()),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: cb.as_entire_binding() }],
        });
        let bpr = (gw * 16).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (bpr * gh) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_bind_group(Stage::Compute.constants_group(), &constants, &[]);
            pass.dispatch_workgroups(gw, gh, 1);
        }
        encoder.copy_texture_to_buffer(
            out.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(gh) },
            },
            out.size(),
        );
        queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = readback.slice(..).get_mapped_range().unwrap();
        (0..gh).flat_map(|y| data[(y * bpr) as usize..(y * bpr + gw * 16) as usize].to_vec()).collect()
    }

    #[test]
    fn large_constant_buffers_keep_their_own_binding() {
        let decl = |slot: u32, rows: u32| {
            format!(
                "struct cb{slot}_struct {{\n  tint_symbol : array<vec4<f32>, {rows}u>,\n}}\n\n\
                 @group(0u) @binding({slot}u) var<uniform> cb{slot}_{slot} : cb{slot}_struct;\n\n"
            )
        };
        // The matrices (256 B), lighting (960 B), the bones (12,240 B).
        let text = format!(
            "{}{}{}@vertex\nfn main() -> @builtin(position) vec4<f32> {{\n  return cb1_1.tint_symbol[0u] + \
             cb3_3.tint_symbol[0u] + cb4_4.tint_symbol[0u];\n}}\n",
            decl(1, 16),
            decl(3, 60),
            decl(4, 765)
        );
        let (out, pack) = pack_uniforms(&text);
        let pack = pack.unwrap();
        assert_eq!(pack.own.iter().map(|m| (m.slot, m.size)).collect::<Vec<_>>(), [(3, 960), (4, 12240)]);
        assert_eq!((pack.members.len(), pack.size), (1, 256));
        assert!(out.contains("@group(2u) @binding(4u) var<uniform> cb4_4 : cb4_struct;"));
        assert!(out.contains("cbpack_v.cb1_1.tint_symbol[0u]") && out.contains("cb4_4.tint_symbol[0u]"));
        naga::front::wgsl::parse_str(&out).unwrap();
        // Never all of them, nor more than asked.
        assert!(pack_uniforms(&format!("{}@vertex fn main() -> @builtin(position) vec4<f32> {{ return cb4_4.tint_symbol[0u]; }}", decl(4, 765))).1.unwrap().own.is_empty());
        assert_eq!(pack_uniforms_owning(&text, 1).1.unwrap().own.len(), 1);
    }

    /// Every translated shader of the game (GLUE5_GAME_DIR) still validates
    /// once adapted: `cargo test -r -p glue5-gpu -- --ignored`.
    #[test]
    #[ignore]
    fn all_shaders_validate_once_packed() {
        let game = std::env::var_os("GLUE5_GAME_DIR").expect("GLUE5_GAME_DIR");
        let dir = glue5_runtime::layout::shader_dir(std::path::Path::new(&game));
        let mut validator =
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
        let mut no_clip = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all() - naga::valid::Capabilities::CLIP_DISTANCES,
        );
        let clipping = naga::front::wgsl::parse_str(&with_clipping(None, CLIP_LOCATION).unwrap()).unwrap();
        no_clip.validate(&clipping).unwrap();
        let (mut n, mut failed) = (0, Vec::new());
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "wgsl") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let (mut text, _) = pack_uniforms(&source);
            if text.contains("@fragment")
                && let Some(opaque) = with_opaque_sample_inputs(&text)
            {
                text = opaque;
            }
            if text.contains("@fragment")
                && let Some(alpha_tested) = with_alpha_test(&text)
            {
                text = alpha_tested;
            }
            if text.contains("@builtin(frag_depth)") {
                let ok = without_frag_depth(&text)
                    .filter(|t| !t.contains("frag_depth"))
                    .ok_or_else(|| "not rewritten".to_owned())
                    .and_then(|t| naga::front::wgsl::parse_str(&t).map_err(|e| e.to_string()))
                    .and_then(|m| validator.validate(&m).map(|_| ()).map_err(|e| format!("{e:?}")));
                if let Err(e) = ok {
                    failed.push(format!("{} (no depth output): {}", path.display(), e.lines().next().unwrap_or("")));
                }
            }
            n += 1;
            let ok = naga::front::wgsl::parse_str(&text)
                .map_err(|e| e.to_string())
                .and_then(|m| validator.validate(&m).map(|_| ()).map_err(|e| format!("{e:?}")));
            if let Err(e) = ok {
                failed.push(format!("{}: {}", path.display(), e.lines().next().unwrap_or("")));
            }
            // As adapted for GPUs without clip distances.
            let unclipped = if text.contains("@builtin(clip_distances)") {
                without_clip_distances(&text)
            } else if text.contains("@fragment") {
                with_clipping(Some(&text), CLIP_LOCATION)
            } else {
                None
            };
            if let Some(adapted) = unclipped {
                // naga's front end takes any `enable`; wgpu's does not.
                if text.contains("@builtin(clip_distances)") && adapted.contains("clip_distances") {
                    failed.push(format!("{}: clip distances left", path.display()));
                }
                let ok = naga::front::wgsl::parse_str(&adapted)
                    .map_err(|e| e.to_string())
                    .and_then(|m| no_clip.validate(&m).map(|_| ()).map_err(|e| format!("{e:?}")));
                if let Err(e) = ok {
                    failed.push(format!("{} (no clip distances): {}", path.display(), e.lines().next().unwrap_or("")));
                }
            } else if text.contains("@builtin(clip_distances)") || text.contains("@fragment") {
                failed.push(format!("{}: not adapted for GPUs without clip distances", path.display()));
            }
            let bindings = reflect_bindings(&text);
            assert!(bindings.iter().all(|(_, b)| b.binding != PACKED_CBUFFERS || b.kind == Kind::Uniform));
        }
        assert!(failed.is_empty(), "{} of {n} failed:\n{}", failed.len(), failed[..failed.len().min(10)].join("\n"));
        assert!(n > 4000);
    }

    #[test]
    fn makes_sample_inputs_opaque() {
        let ps = "@fragment
fn main(@location(1u) @interpolate(perspective, sample) v1 : vec4<f32>, @location(2u) v2 : vec4<f32>) -> @location(0u) vec4<f32> {
  return v1 / v1.w + v2;
}
";
        let out = with_opaque_sample_inputs(ps).unwrap();
        assert!(out.contains("@location(1u) @interpolate(perspective, sample) g5_v1 : vec4<f32>, @location(2u) v2 : vec4<f32>, @builtin(sample_index) g5_sample : u32"));
        assert!(out.contains("{\n  let v1 = bitcast<vec4<f32>>(bitcast<vec4<u32>>(g5_v1) | vec4<u32>(g5_sample >> 31u));\n  return v1 / v1.w + v2;"));
        assert_eq!(interpolations(&out), interpolations(ps));
        naga::front::wgsl::parse_str(&out).unwrap();
        let indexed = ps.replace("v2 : vec4<f32>)", "v2 : vec4<f32>, @builtin(sample_index) v3 : u32)");
        let out = with_opaque_sample_inputs(&indexed).unwrap();
        assert!(out.contains("(g5_v1) | vec4<u32>(v3 >> 31u));") && !out.contains("g5_sample"));
        naga::front::wgsl::parse_str(&out).unwrap();
        assert!(with_opaque_sample_inputs(&ps.replace(", sample", ", center")).is_none());
    }

    #[test]
    fn drops_the_depth_output() {
        let ps = "var<private> o0 : f32;
var<private> oDepth : f32;
fn main_inner() {
  o0 = 1.0f;
  oDepth = 0.5f;
}
struct tint_symbol {
  @location(0u)
  o0 : f32,
  @builtin(frag_depth)
  oDepth : f32,
}
@fragment
fn main() -> tint_symbol {
  main_inner();
  return tint_symbol(o0, oDepth);
}
";
        let out = without_frag_depth(ps).unwrap();
        assert!(!out.contains("frag_depth") && out.contains("return tint_symbol(o0);"));
        naga::front::wgsl::parse_str(&out).unwrap();
        let only = "var<private> oDepth : f32;
fn main_inner(p : vec4<f32>) {
  oDepth = p.z;
}
@fragment
fn main(@builtin(position) v_5 : vec4<f32>) -> @builtin(frag_depth) f32 {
  main_inner(v_5);
  return oDepth;
}
";
        let out = without_frag_depth(only).unwrap();
        assert!(out.contains("fn main(@builtin(position) v_5 : vec4<f32>) {\n  main_inner(v_5);\n}"));
        naga::front::wgsl::parse_str(&out).unwrap();
    }

    #[test]
    fn reads_depth_textures_directly() {
        let text = "@group(1u) @binding(16u) var s0 : sampler;
@group(1u) @binding(32u) var t0 : texture_2d<f32>;
@group(1u) @binding(33u) var t1 : texture_2d<f32>;
fn f(uv : vec2<f32>, r : vec4<f32>) -> f32 {
  let a = textureSample(t0, s0, r.xyxx.xy).x;
  let b = textureSampleLevel(t0, s0, uv, r.w).x;
  let c = textureLoad(t0, bitcast<vec2<i32>>(r.xy), 0i).x;
  let d = textureGather(0i, t0, s0, uv).y;
  let e = textureSample(t1, s0, vec2<f32>(textureLoad(t0, vec2<i32>(0, 0), 0i).x, 0.5f)).x;
  return a + b + c + d + e;
}
";
        let out = with_depth_reads(text, 1).unwrap();
        assert!(out.contains("var t0 : texture_depth_2d;"));
        assert!(out.contains("var t1 : texture_2d<f32>;"));
        assert!(out.contains("g5_depth4(textureSample(t0, s0, r.xyxx.xy)).x"));
        assert!(out.contains("g5_depth4(textureSampleLevel(t0, s0, uv, i32(r.w))).x"));
        assert!(out.contains("g5_depth4(textureLoad(t0, bitcast<vec2<i32>>(r.xy), 0i)).x"));
        assert!(out.contains("textureGather(t0, s0, uv).y"));
        assert!(
            out.contains("textureSample(t1, s0, vec2<f32>(g5_depth4(textureLoad(t0, vec2<i32>(0, 0), 0i)).x, 0.5f))")
        );
        naga::front::wgsl::parse_str(&out).unwrap();
        let grad = "@group(1u) @binding(32u) var t0 : texture_2d<f32>;\n@group(1u) @binding(16u) var s0 : sampler;\nfn f(u : vec2<f32>) -> vec4<f32> { return textureSampleGrad(t0, s0, u, u, u); }\n";
        assert!(with_depth_reads(grad, 1).is_none());
    }

    #[test]
    fn reflects_multisampled_sample_types() {
        let text = "@group(1u) @binding(32u) var t0 : texture_multisampled_2d<f32>;
@group(1u) @binding(33u) var t1 : texture_multisampled_2d<u32>;
";
        let kinds: Vec<Kind> = reflect_bindings(text).into_iter().map(|(_, b)| b.kind).collect();
        let ms = |sample| Kind::Texture { sample, dim: wgpu::TextureViewDimension::D2, multisampled: true };
        assert_eq!(kinds, [ms(Sample::Float), ms(Sample::Uint)]);
    }

    #[test]
    fn finds_sampling_pairs() {
        let text = "@group(1u) @binding(16u) var s0 : sampler;
@group(1u) @binding(17u) var s1 : sampler;
@group(1u) @binding(32u) var t0 : texture_2d<f32>;
@group(1u) @binding(33u) var t1 : texture_2d<f32>;
fn f(uv : vec2<f32>) {
  let a = textureSample(t0, s0, uv);
  let b = textureGather(1i, t1, s1, uv);
  let c = textureLoad(t1, vec2<i32>(0), 0);
}
";
        assert_eq!(sampling_pairs(text), vec![(32, 16), (33, 17)]);
    }

    #[test]
    fn copies_interpolation_to_vertex_outputs() {
        let text = with_interpolation(VS, &[(1, "perspective, sample".into())]);
        assert!(text.contains("@location(1u) @interpolate(perspective, sample)\n  o1"));
        let moved = with_locations(&text, Stage::Vertex, &[(1, 0)]);
        assert!(moved.contains("@location(0u) @interpolate"));
        assert!(moved.contains("@location(0u) v0"), "inputs stay");
    }

    #[test]
    fn radar_uses_full_coverage_without_a_geometry_stage() {
        let source = "fn main_inner(v1 : vec4<f32>, v5 : vec4<f32>) -> vec4<f32> {
  return vec4<f32>(v1.xyz, v1.w * clamp(min(v5.x, min(v5.y, v5.z)), 0.0f, 1.0f));
}
@fragment
fn main(@location(1u) v1 : vec4<f32>, @location(5u) v5 : vec4<f32>) -> @location(0u) vec4<f32> {
  main_inner(v1, v5);
  return v1;
}
";
        let text = with_radar_coverage(source).unwrap();
        assert_eq!(ps_input_locations(&text), [1]);
        assert!(text.contains("let v5 = vec4<f32>(1.0f);"));
        assert!(text.contains("v1.w * clamp(min(v5.x, min(v5.y, v5.z)), 0.0f, 1.0f)"));
        let module = naga::front::wgsl::parse_str(&text).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap();
        assert!(with_radar_coverage(&text).is_none());
        assert!(with_radar_coverage(&source.replace("vec4<f32>) -> @location", "vec4<u32>) -> @location")).is_none());
    }

    #[test]
    fn hud_mask_has_its_own_bindings_and_constants() {
        let (text, pack) = pack_uniforms(include_str!("minimap_mask.wgsl"));
        let pack = pack.unwrap();
        assert_eq!(pack.size, 64);
        assert_eq!((pack.members[0].slot, pack.members[0].offset), (5, 0));
        assert_eq!(stage_bindings(&text, Stage::Pixel).iter().map(|b| b.binding).collect::<Vec<_>>(), [18, 34]);
        assert!(text.contains("@group(3u) @binding(0u) var<uniform> cbpack_v : cbpack_t;"));
        assert_eq!(ps_input_locations(&text), [1, 2]);
        let module = naga::front::wgsl::parse_str(&text).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap();
        assert!(with_hud_coordinates(VS).is_none());
        assert!(!pause_map_blend(&[]));
        assert!(pause_map_blend(&[0, 0, 1, 2, 2, 4, 2, 2, 4, 8]));
        assert!(!pause_map_blend(&[0, 0, 1, 2, 2, 4, 2, 2, 4, 15]));
    }

    #[test]
    #[ignore = "requires the game shaders and a GPU"]
    fn minimap_mask_pipeline_compiles() {
        use crate::pipeline::{Compiler, Recipe};
        let game = std::env::var_os("GLUE5_GAME_DIR").expect("GLUE5_GAME_DIR");
        let library = Library::open(&glue5_runtime::layout::shader_dir(Path::new(&game))).unwrap();
        let gpu = crate::Device::open(&crate::instance(), None).unwrap();
        let vs = library.named(&gpu.device, "im::VS_Blit", Stage::Vertex).unwrap();
        let original = library.named(&gpu.device, "im::PS_BlitDepthToAlpha", Stage::Pixel).unwrap();
        let ps = original.for_hud_mask(&gpu.device).unwrap();
        assert!(!original.hud_mask);
        assert!(ps.hud_mask);
        assert_ne!(ps.id, original.id);
        assert_eq!(ps.id, original.for_hud_mask(&gpu.device).unwrap().id);
        assert_eq!(original.textures.len(), 2);
        assert_eq!(ps.textures.len(), 1);
        let recipe = Recipe::from_json(&serde_json::json!({
            "vs": format!("{:016x}", vs.hash), "ps": format!("{:016x}", ps.hash),
            "sig": [[1, 0, 0], [2, 0, 2], [3, 0, 3]],
            "el": [[1, 0, 6, 0, 0, 0, 0], [2, 0, 2, 0, 12, 0, 0], [3, 0, 16, 0, 28, 0, 0]],
            "st": [36], "tp": 4, "sf": "", "b": null, "d": null, "r": null,
            "rt": ["rgba8unorm"], "n": 1, "ds": "", "ro": 0, "hud_mask": true
        }))
        .unwrap();
        assert_eq!(recipe, Recipe::from_json(&recipe.to_json()).unwrap());
        let compiler = Compiler::new(gpu.device.clone(), crate::format::Caps::new(&gpu.adapter, gpu.features), None);
        assert!(compiler.compile(&vs, Some(&ps), &recipe).is_some());
        let pause = library.named(&gpu.device, "im::VS_TransformUnlit_IM", Stage::Vertex).unwrap();
        assert!(pause.pack.is_some(), "world-space map keeps its transform constants");
        assert!(compiler.compile(&pause, Some(&original), &recipe).is_some());
    }

    /// Reproduces the failing stage pair without depending on a save, a
    /// window or the engine selecting its antialiased radar technique.
    #[test]
    #[ignore = "requires the game shaders and a GPU"]
    fn radar_pipeline_compiles() {
        use crate::pipeline::{Compiler, Recipe};
        let game = std::env::var_os("GLUE5_GAME_DIR").expect("GLUE5_GAME_DIR");
        let library = Library::open(&glue5_runtime::layout::shader_dir(Path::new(&game))).unwrap();
        let gpu = crate::Device::open(&crate::instance(), None).unwrap();
        let vs = library.prepare(&gpu.device, 0x6e3b688101fd34ef, Stage::Vertex).unwrap();
        let ps = library.prepare(&gpu.device, 0x5c5d5123dfb3dd00, Stage::Pixel).unwrap();
        assert_eq!(vs.locations, [1, 2, 3, 4]);
        assert_eq!(ps.locations, [1, 2, 3]);
        assert!(!ps.text.get().contains("@location(5u)"));
        let recipe = Recipe::from_json(&serde_json::json!({
            "vs": "6e3b688101fd34ef", "ps": "5c5d5123dfb3dd00",
            "sig": [[1, 0, 0], [2, 0, 1], [3, 0, 2]],
            "el": [[1, 0, 6, 0, 0, 0, 0], [2, 0, 16, 0, 12, 0, 0], [3, 0, 2, 0, 20, 0, 0]],
            "st": [36], "tp": 4, "sf": "", "b": null, "d": null, "r": null,
            "rt": ["rgba8unorm"], "n": 1, "ds": "", "ro": 0
        }))
        .unwrap();
        let compiler = Compiler::new(gpu.device.clone(), crate::format::Caps::new(&gpu.adapter, gpu.features), None);
        assert!(compiler.compile(&vs, Some(&ps), &recipe).is_some());
    }

    /// A pixel program that reads D3D's `SV_Position` computes in pixels of
    /// the target it draws into, so redirecting that draw to a differently
    /// sized target changes what it computes. Which programs those are is
    /// what the census reports (see `crate::census`).
    #[test]
    fn a_pixel_program_reading_the_position_is_found() {
        let text = "@fragment\nfn main(@builtin(position) p : vec4<f32>, @location(0) v : vec4<f32>) -> \
                    @location(0) vec4<f32> {\n  return v;\n}\n";
        assert!(reads_position(text));
    }

    /// A pixel program without it is unaffected by the target's size, so its
    /// draw can be redirected as it is.
    #[test]
    fn a_pixel_program_without_the_position_is_not() {
        let text = "@fragment\nfn main(@location(0) v : vec4<f32>) -> @location(0) vec4<f32> {\n  return v;\n}\n";
        assert!(!reads_position(text));
    }

    /// Every vertex program *outputs* `@builtin(position)`, so matching the
    /// text as a whole would flag all of them. Only the fragment entry point's
    /// own parameters are a read of it.
    #[test]
    fn a_vertex_program_outputting_the_position_is_not_a_read() {
        let text = "@vertex\nfn main(@builtin(vertex_index) i : u32) -> @builtin(position) vec4<f32> {\n  \
                    return vec4<f32>(0.0);\n}\n";
        assert!(!reads_position(text));
    }

    /// Nor is a helper function's parameter: the entry point is what wgpu
    /// gives the pixel's own coordinates to.
    #[test]
    fn a_helper_taking_the_position_is_not_a_read() {
        let text = "fn helper(@builtin(position) q : vec4<f32>) -> f32 {\n  return q.x;\n}\n\
                    @fragment\nfn main(@location(0) v : vec4<f32>) -> @location(0) vec4<f32> {\n  return v;\n}\n";
        assert!(!reads_position(text));
    }
}
