//! The engine's GPU objects, by id. Ids come from one counter on the engine
//! side and are never reused, so caches can key on them.

use std::cell::Cell;
use std::sync::{Arc, OnceLock};

use rustc_hash::FxHashMap;
use wgpu::{TextureDimension, TextureFormat};

use crate::format::Upload;
use crate::pipeline::{Element, SigElement};
use crate::shader::{Program, Stage};

pub struct Tex {
    pub tex: wgpu::Texture,
    pub format: TextureFormat,
    pub upload: Upload,
    pub dim: TextureDimension,
    pub width: u32,
    pub height: u32,
    /// Depth (3D) or array layers.
    pub layers: u32,
    /// Size and mips as the engine sees them; the GPU texture lacks the
    /// first `dropped` levels (BC textures decoded on the CPU, see
    /// `create_texture`): engine mip `m` is GPU level `m - dropped`.
    pub mips: u32,
    pub dropped: u32,
    pub samples: u32,
    pub bytes: u64,
    /// Its description, to be reused for a texture like it once the engine
    /// destroys it (see `recycle.rs`); `None`: not reused.
    pub recycle: Option<crate::recycle::TexKey>,
    /// Render-target / clear views, by (format, mip, layer).
    pub views: Vec<((TextureFormat, u32, u32), wgpu::TextureView)>,
    /// Encoder epoch of the last command that used it.
    pub used: Cell<u64>,
    /// Encoder epoch of the last upload into it (uploads run before the commands).
    pub uploaded: Cell<u64>,
    /// A cached bind group may hold a view of it.
    pub in_bind_group: Cell<bool>,
    /// Copies of a depth texture readable as float (0) / uint stencil (1),
    /// and of a float texture usable as depth for comparisons (2).
    pub proxies: [Option<Box<Proxy>>; 3],
    /// GenerateMips passes (made once).
    pub mip_chain: Option<Vec<(wgpu::TextureView, wgpu::BindGroup)>>,
    /// Its view for presenting (made once).
    pub present: Option<wgpu::TextureView>,
}

pub struct Proxy {
    pub tex: Tex,
    pub dirty: bool,
    pub passes: Vec<(wgpu::TextureView, wgpu::BindGroup)>,
}

impl Tex {
    pub fn is_3d(&self) -> bool {
        self.dim == TextureDimension::D3
    }

    /// The GPU level of engine mip `mip`, unless dropped.
    pub fn level(&self, mip: u32) -> Option<u32> {
        mip.checked_sub(self.dropped)
    }

    pub fn mip_size(&self, mip: u32) -> (u32, u32, u32) {
        let d = if self.is_3d() { (self.layers >> mip).max(1) } else { 1 };
        ((self.width >> mip).max(1), (self.height >> mip).max(1), d)
    }

    /// A single-subresource view in `format` (render targets, clears).
    pub fn target_view(&mut self, format: TextureFormat, mip: u32, layer: u32) -> wgpu::TextureView {
        if let Some((_, v)) = self.views.iter().find(|(k, _)| *k == (format, mip, layer)) {
            return v.clone();
        }
        let view = self.tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(format),
            dimension: Some(if self.is_3d() { wgpu::TextureViewDimension::D3 } else { wgpu::TextureViewDimension::D2 }),
            base_mip_level: mip,
            mip_level_count: Some(1),
            base_array_layer: if self.is_3d() { 0 } else { layer },
            array_layer_count: Some(1),
            ..Default::default()
        });
        self.views.push(((format, mip, layer), view.clone()));
        view
    }

    pub fn mark_written(&mut self) {
        for p in self.proxies.iter_mut().flatten() {
            p.dirty = true;
        }
    }

    /// A pass writes these aspects of it (a depth target): the copies of
    /// the others stay good (a pass with depth read-only redid its copy at
    /// every draw that read it, breaking the pass each time).
    pub fn mark_aspects_written(&mut self, depth: bool, stencil: bool) {
        // Proxies: depth as float, stencil as uint, float as depth.
        for (p, written) in self.proxies.iter_mut().zip([depth, stencil, true]) {
            if let Some(p) = p
                && written
            {
                p.dirty = true;
            }
        }
    }
}

pub struct Buf {
    pub size: u64,
    pub stride: u32,
    /// The GPU buffer (vertex, index, structured, indirect args); constant
    /// buffers live in `shadow` only.
    pub gpu: Option<wgpu::Buffer>,
    /// Where in `gpu` the buffer starts: small vertex and index buffers
    /// share GPU buffers (`slot`, see slabs.rs).
    pub base: u64,
    pub slot: Option<crate::slabs::Slot>,
    /// Size of `gpu` (4-byte multiple).
    pub bytes: u64,
    /// Identity of the current GPU allocation (a full overwrite while in use
    /// moves the buffer to a new allocation, as D3D's map-discard does).
    pub uid: u32,
    pub usage: wgpu::BufferUsages,
    pub shadow: Option<Vec<u8>>,
    pub version: u32,
    pub used: Cell<u64>,
    pub uploaded: Cell<u64>,
    pub in_bind_group: Cell<bool>,
    /// A constant buffer's copy for the stages that bind it on their own
    /// (`shader::Pack::own`): (frame, version, first constant, size, where).
    pub constants: Cell<Option<(u64, u32, u32, u32, crate::uniforms::Alloc)>>,
    /// The UAV counter of append/consume buffers.
    pub counter: Option<wgpu::Buffer>,
}

pub type ProgramSlot = Arc<OnceLock<Option<Arc<Program>>>>;

pub struct Shader {
    pub stage: Stage,
    pub hash: u64,
    pub sig: Arc<[SigElement]>,
    pub slot: ProgramSlot,
}

pub struct Layout {
    pub elements: Vec<Element>,
    /// (slot, class) of each slot used, in element order.
    pub slots: Vec<(u32, u32)>,
}

/// No input layout: draws whose vertex shader reads no vertex attributes.
pub static NO_LAYOUT: Layout = Layout { elements: Vec::new(), slots: Vec::new() };

pub struct State {
    pub words: Box<[u32]>,
    /// Sampler states (kind 3).
    pub sampler: Option<wgpu::Sampler>,
    pub comparison: bool,
    /// The sampler with nearest filtering, made when a program needs a
    /// non-filtering one (see `Program::unfiltered`).
    pub nearest: Option<wgpu::Sampler>,
}

/// A shader resource view or unordered access view.
pub struct View {
    pub res: u32,
    pub is_buffer: bool,
    pub format: u32,
    pub view_dim: u32,
    /// SRV: most detailed mip / first element, mip count / element count,
    /// first slice, slice count. UAV: mip / first element, first slice /
    /// element count, slice count, flags.
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
    pub views: Vec<(u64, wgpu::TextureView)>,
}

pub struct Query {
    pub kind: u32,
    pub at: u64,
    /// Occlusion: query-set slots of the passes between Begin and End.
    pub slots: Vec<u32>,
}

pub enum Obj {
    Texture(Box<Tex>),
    Buffer(Box<Buf>),
    Shader(Box<Shader>),
    Layout(Box<Layout>),
    State(Box<State>),
    Srv(Box<View>),
    Uav(Box<View>),
    Query(Box<Query>),
}

#[derive(Default)]
pub struct Objects {
    map: FxHashMap<u32, Obj>,
    /// The textures' and buffers' bytes, kept as objects come and go: adding
    /// them up walked every object, a stall at each of the overlay's updates.
    /// A texture's `bytes`, and a buffer's `bytes` and whether it has a `gpu`
    /// buffer, do not change while it is here.
    bytes: [u64; 2],
}

/// What an object counts for in [`Objects::bytes`].
fn counted(obj: &Obj) -> [u64; 2] {
    match obj {
        Obj::Texture(t) => [t.bytes, 0],
        Obj::Buffer(b) => [0, b.gpu.as_ref().map_or(0, |_| b.bytes)],
        _ => [0, 0],
    }
}

macro_rules! getters {
    ($($get:ident, $get_mut:ident, $variant:ident, $ty:ty;)*) => {$(
        pub fn $get(&self, id: u32) -> Option<&$ty> {
            match self.map.get(&id) {
                Some(Obj::$variant(x)) => Some(x),
                _ => None,
            }
        }
        #[allow(dead_code)]
        pub fn $get_mut(&mut self, id: u32) -> Option<&mut $ty> {
            match self.map.get_mut(&id) {
                Some(Obj::$variant(x)) => Some(x),
                _ => None,
            }
        }
    )*};
}

impl Objects {
    getters! {
        tex, tex_mut, Texture, Tex;
        buf, buf_mut, Buffer, Buf;
        shader, shader_mut, Shader, Shader;
        layout, layout_mut, Layout, Layout;
        state, state_mut, State, State;
        srv, srv_mut, Srv, View;
        uav, uav_mut, Uav, View;
        query, query_mut, Query, Query;
    }

    pub fn insert(&mut self, id: u32, obj: Obj) {
        let [textures, buffers] = counted(&obj);
        self.bytes[0] += textures;
        self.bytes[1] += buffers;
        if let Some(old) = self.map.insert(id, obj) {
            self.uncount(&old);
        }
    }

    pub fn remove(&mut self, id: u32) -> Option<Obj> {
        let obj = self.map.remove(&id);
        if let Some(obj) = &obj {
            self.uncount(obj);
        }
        obj
    }

    fn uncount(&mut self, obj: &Obj) {
        let [textures, buffers] = counted(obj);
        self.bytes[0] -= textures;
        self.bytes[1] -= buffers;
    }

    /// The textures' and buffers' bytes.
    pub fn bytes(&self) -> [u64; 2] {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Two different objects at once (`a != b`).
    pub fn two_mut(&mut self, a: u32, b: u32) -> Option<(&mut Obj, &mut Obj)> {
        let [x, y] = self.map.get_disjoint_mut([&a, &b]);
        Some((x?, y?))
    }
}
