use std::collections::{HashMap, HashSet};

use anyhow::{Context, ensure};

use crate::{D3D11Buffer, D3D11Device, D3D11Texture, TextureBindings};
use windows::Win32::{
    Graphics::Direct3D11::{ID3D11Query, ID3D11SamplerState, ID3D11ShaderResourceView},
    Graphics::Dxgi::Common::DXGI_FORMAT,
};

/// The deliberately small command vocabulary implemented by the native
/// executor and populated by the opt-in GpuRing adapter.
#[derive(Debug)]
pub enum D3D11Command {
    CreateTexture2D {
        id: u32,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        bindings: TextureBindings,
    },
    CreateBuffer {
        id: u32,
        size: u32,
        bind: u32,
        misc: u32,
        stride: u32,
    },
    UploadBuffer {
        id: u32,
        offset: u32,
        data: Vec<u8>,
    },
    CreateSrv {
        id: u32,
        resource_id: u32,
        is_buffer: bool,
        format: u32,
        view_dim: u32,
        a: u32,
        b: u32,
        c: u32,
        d: u32,
    },
    SetSrvs {
        stage: u32,
        start: u32,
        views: Vec<u32>,
    },
    CreateSampler {
        id: u32,
        words: Vec<u32>,
    },
    SetSamplers {
        stage: u32,
        start: u32,
        samplers: Vec<u32>,
    },
    UploadTexture2D {
        id: u32,
        mip: u32,
        slice: u32,
        x: u32,
        y: u32,
        z: u32,
        width: u32,
        height: u32,
        depth: u32,
        bytes_per_row: u32,
        rows: u32,
        data: Vec<u8>,
    },
    DestroyObject {
        id: u32,
    },
    ClearRenderTarget {
        id: u32,
        color: [f32; 4],
    },
    Flush,
}

/// Counters for the commands accepted by the native executor.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExecutorStats {
    pub commands: u64,
    pub textures_created: u64,
    pub buffers_created: u64,
    pub buffers_uploaded: u64,
    pub buffer_bytes_uploaded: u64,
    pub textures_uploaded: u64,
    pub texture_bytes_uploaded: u64,
    pub srvs_created: u64,
    pub srv_bind_calls: u64,
    pub srv_slots_bound: u64,
    pub srv_slots_unbound_on_destroy: u64,
    pub srv_slots_rebound_after_recreate: u64,
    pub samplers_created: u64,
    pub sampler_bind_calls: u64,
    pub sampler_slots_bound: u64,
    pub sampler_slots_unbound_on_destroy: u64,
    pub objects_destroyed: u64,
    pub render_targets_cleared: u64,
    pub flushes: u64,
    pub fences_completed: u64,
}

#[derive(Clone, Copy)]
struct SrvDescriptor {
    resource_id: u32,
    is_buffer: bool,
    format: u32,
    view_dim: u32,
    a: u32,
    b: u32,
    c: u32,
    d: u32,
}

struct SrvObject {
    descriptor: SrvDescriptor,
    view: Option<ID3D11ShaderResourceView>,
}

struct BindingState {
    /// Object IDs requested by the engine, retained across object destruction.
    srvs: [[u32; 128]; 3],
    samplers: [[u32; 16]; 3],
    /// Object IDs that currently have a live COM object in the native context.
    bound_srvs: [[u32; 128]; 3],
    bound_samplers: [[u32; 16]; 3],
}

impl Default for BindingState {
    fn default() -> Self {
        Self { srvs: [[0; 128]; 3], samplers: [[0; 16]; 3], bound_srvs: [[0; 128]; 3], bound_samplers: [[0; 16]; 3] }
    }
}

/// A first reusable D3D11 command executor.
///
/// The opt-in GpuRing adapter translates a deliberately small opcode
/// allowlist into this typed command vocabulary.
pub struct D3D11Executor {
    device: D3D11Device,
    textures: HashMap<u32, D3D11Texture>,
    buffers: HashMap<u32, D3D11Buffer>,
    srvs: HashMap<u32, SrvObject>,
    samplers: HashMap<u32, ID3D11SamplerState>,
    bindings: BindingState,
    fence_query: Option<ID3D11Query>,
    stats: ExecutorStats,
}

impl D3D11Executor {
    pub fn open_hardware() -> anyhow::Result<Self> {
        Ok(Self {
            device: D3D11Device::open_hardware()?,
            textures: HashMap::new(),
            buffers: HashMap::new(),
            srvs: HashMap::new(),
            samplers: HashMap::new(),
            bindings: BindingState::default(),
            fence_query: None,
            stats: ExecutorStats::default(),
        })
    }

    pub fn device(&self) -> &D3D11Device {
        &self.device
    }

    pub fn stats(&self) -> ExecutorStats {
        self.stats
    }

    /// Waits for all immediate-context work submitted before this call.
    /// The event query is created lazily and reused after each completion.
    pub fn wait_for_gpu(&mut self) -> anyhow::Result<()> {
        if self.fence_query.is_none() {
            self.fence_query = Some(self.device.create_event_query()?);
        }

        let query = self.fence_query.as_ref().context("D3D11 event query was not created")?;
        self.device.wait_event_query(query)?;
        self.stats.fences_completed += 1;
        Ok(())
    }

    pub fn texture(&self, id: u32) -> Option<&D3D11Texture> {
        self.textures.get(&id)
    }

    pub fn buffer(&self, id: u32) -> Option<&D3D11Buffer> {
        self.buffers.get(&id)
    }

    pub fn execute(&mut self, command: D3D11Command) -> anyhow::Result<()> {
        self.stats.commands += 1;

        match command {
            D3D11Command::CreateTexture2D { id, width, height, format, bindings } => {
                ensure!(
                    !self.textures.contains_key(&id)
                        && !self.buffers.contains_key(&id)
                        && !self.srvs.contains_key(&id)
                        && !self.samplers.contains_key(&id),
                    "D3D11 object ID {id} already exists"
                );
                let texture = self
                    .device
                    .create_texture_2d(width, height, format, bindings)
                    .with_context(|| format!("creating D3D11 texture object {id}"))?;
                self.textures.insert(id, texture);
                self.stats.textures_created += 1;
                self.refresh_bound_srvs_for_resource(id)?;
            }
            D3D11Command::CreateBuffer { id, size, bind, misc, stride } => {
                ensure!(
                    !self.textures.contains_key(&id)
                        && !self.buffers.contains_key(&id)
                        && !self.srvs.contains_key(&id)
                        && !self.samplers.contains_key(&id),
                    "D3D11 object ID {id} already exists"
                );
                let buffer = self
                    .device
                    .create_buffer(size, bind, misc, stride)
                    .with_context(|| format!("creating D3D11 buffer object {id}"))?;
                self.buffers.insert(id, buffer);
                self.stats.buffers_created += 1;
                self.refresh_bound_srvs_for_resource(id)?;
            }
            D3D11Command::UploadBuffer { id, offset, data } => {
                let buffer = self.buffers.get(&id).with_context(|| format!("unknown D3D11 buffer object {id}"))?;
                self.device.update_buffer(buffer, offset, &data)?;
                self.stats.buffers_uploaded += 1;
                self.stats.buffer_bytes_uploaded += data.len() as u64;
            }
            D3D11Command::CreateSrv { id, resource_id, is_buffer, format, view_dim, a, b, c, d } => {
                ensure!(
                    !self.textures.contains_key(&id)
                        && !self.buffers.contains_key(&id)
                        && !self.srvs.contains_key(&id)
                        && !self.samplers.contains_key(&id),
                    "D3D11 object ID {id} already exists"
                );
                let descriptor = SrvDescriptor { resource_id, is_buffer, format, view_dim, a, b, c, d };
                let view = self.create_srv_view(descriptor)?;
                self.srvs.insert(id, SrvObject { descriptor, view });
                self.stats.srvs_created += 1;
                self.refresh_bound_srv_ids(&HashSet::from([id]))?;
            }
            D3D11Command::SetSrvs { stage, start, views } => {
                let mut resolved = Vec::with_capacity(views.len());
                for id in &views {
                    if *id == 0 {
                        resolved.push(None);
                    } else {
                        resolved.push(self.resolve_srv_view(*id)?);
                    }
                }
                self.device.set_shader_resources(stage, start, &resolved)?;
                let stage_index = stage as usize;
                let start_index = start as usize;
                let end_index = start_index + views.len();
                self.bindings.srvs[stage_index][start_index..end_index].copy_from_slice(&views);
                for (offset, view) in resolved.iter().enumerate() {
                    self.bindings.bound_srvs[stage_index][start_index + offset] =
                        if view.is_some() { views[offset] } else { 0 };
                }
                self.stats.srv_bind_calls += 1;
                self.stats.srv_slots_bound += resolved.len() as u64;
            }
            D3D11Command::CreateSampler { id, words } => {
                ensure!(
                    !self.textures.contains_key(&id)
                        && !self.buffers.contains_key(&id)
                        && !self.srvs.contains_key(&id)
                        && !self.samplers.contains_key(&id),
                    "D3D11 object ID {id} already exists"
                );
                let sampler = self
                    .device
                    .create_sampler_state(&words)
                    .with_context(|| format!("creating D3D11 sampler state {id}"))?;
                self.samplers.insert(id, sampler);
                self.stats.samplers_created += 1;
                self.refresh_bound_sampler_id(id)?;
            }
            D3D11Command::SetSamplers { stage, start, samplers } => {
                let mut resolved = Vec::with_capacity(samplers.len());
                for id in &samplers {
                    if *id == 0 {
                        resolved.push(None);
                    } else {
                        // Keep the logical ID but unbind a missing/destroyed state.
                        // Unlike WGPU, this native backend has no dummy sampler object.
                        resolved.push(self.samplers.get(id).cloned());
                    }
                }
                self.device.set_samplers(stage, start, &resolved)?;
                let stage_index = stage as usize;
                let start_index = start as usize;
                let end_index = start_index + samplers.len();
                self.bindings.samplers[stage_index][start_index..end_index].copy_from_slice(&samplers);
                for (offset, sampler) in resolved.iter().enumerate() {
                    self.bindings.bound_samplers[stage_index][start_index + offset] =
                        if sampler.is_some() { samplers[offset] } else { 0 };
                }
                self.stats.sampler_bind_calls += 1;
                self.stats.sampler_slots_bound += resolved.len() as u64;
            }
            D3D11Command::UploadTexture2D {
                id,
                mip,
                slice,
                x,
                y,
                z,
                width,
                height,
                depth,
                bytes_per_row,
                rows,
                data,
            } => {
                let texture = self.textures.get(&id).with_context(|| format!("unknown D3D11 texture object {id}"))?;
                self.device.update_texture_2d(
                    texture,
                    mip,
                    slice,
                    z,
                    width,
                    height,
                    depth,
                    x,
                    y,
                    bytes_per_row,
                    rows,
                    &data,
                )?;
                self.stats.textures_uploaded += 1;
                self.stats.texture_bytes_uploaded += data.len() as u64;
            }
            D3D11Command::DestroyObject { id } => {
                let destroys_resource = self.textures.contains_key(&id) || self.buffers.contains_key(&id);
                if destroys_resource {
                    let dependent = self
                        .srvs
                        .iter()
                        .filter_map(|(&srv_id, object)| (object.descriptor.resource_id == id).then_some(srv_id))
                        .collect::<HashSet<_>>();
                    self.unbind_srv_ids(&dependent)?;
                    for srv_id in dependent {
                        if let Some(object) = self.srvs.get_mut(&srv_id) {
                            object.view = None;
                        }
                    }
                }

                if self.srvs.contains_key(&id) {
                    self.unbind_srv_ids(&HashSet::from([id]))?;
                }
                if self.samplers.contains_key(&id) {
                    self.unbind_sampler_ids(&HashSet::from([id]))?;
                }

                self.textures.remove(&id);
                self.buffers.remove(&id);
                self.srvs.remove(&id);
                self.samplers.remove(&id);
                self.stats.objects_destroyed += 1;
            }
            D3D11Command::ClearRenderTarget { id, color } => {
                let texture = self.textures.get(&id).with_context(|| format!("unknown D3D11 texture object {id}"))?;
                self.device.clear_render_target(texture, color)?;
                self.stats.render_targets_cleared += 1;
            }
            D3D11Command::Flush => {
                self.device.flush();
                self.stats.flushes += 1;
            }
        }

        Ok(())
    }

    fn create_srv_view(&self, descriptor: SrvDescriptor) -> anyhow::Result<Option<ID3D11ShaderResourceView>> {
        if descriptor.is_buffer {
            let Some(buffer) = self.buffers.get(&descriptor.resource_id) else {
                return Ok(None);
            };
            return self
                .device
                .create_buffer_srv(buffer, descriptor.format, descriptor.view_dim, descriptor.a, descriptor.b)
                .map(Some);
        }

        let Some(texture) = self.textures.get(&descriptor.resource_id) else {
            return Ok(None);
        };
        self.device
            .create_texture_srv_2d(
                texture,
                descriptor.format,
                descriptor.view_dim,
                descriptor.a,
                descriptor.b,
                descriptor.c,
                descriptor.d,
            )
            .map(Some)
    }

    fn resolve_srv_view(&mut self, id: u32) -> anyhow::Result<Option<ID3D11ShaderResourceView>> {
        let Some(object) = self.srvs.get(&id) else {
            // The engine keeps logical slot IDs across object destruction.
            // Bind NULL until this ID is created again; shader-level dummy parity is separate.
            return Ok(None);
        };
        let (descriptor, cached) = (object.descriptor, object.view.clone());
        if cached.is_some() {
            return Ok(cached);
        }

        let Some(view) = self.create_srv_view(descriptor)? else {
            return Ok(None);
        };
        let object = self.srvs.get_mut(&id).context("SRV disappeared while resolving its resource")?;
        object.view = Some(view.clone());
        Ok(Some(view))
    }

    fn refresh_bound_srvs_for_resource(&mut self, resource_id: u32) -> anyhow::Result<()> {
        let ids = self
            .srvs
            .iter()
            .filter_map(|(&id, object)| (object.descriptor.resource_id == resource_id).then_some(id))
            .collect::<HashSet<_>>();
        self.refresh_bound_srv_ids(&ids)
    }

    fn refresh_bound_srv_ids(&mut self, ids: &HashSet<u32>) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        for stage in 0..self.bindings.srvs.len() {
            for slot in 0..self.bindings.srvs[stage].len() {
                let id = self.bindings.srvs[stage][slot];
                if !ids.contains(&id) || id == 0 {
                    continue;
                }
                let view = self.resolve_srv_view(id)?;
                let rebound = view.is_some();
                self.device.set_shader_resources(stage as u32, slot as u32, &[view])?;
                self.bindings.bound_srvs[stage][slot] = if rebound { id } else { 0 };
                if rebound {
                    self.stats.srv_slots_rebound_after_recreate += 1;
                }
            }
        }
        Ok(())
    }

    fn refresh_bound_sampler_id(&mut self, id: u32) -> anyhow::Result<()> {
        let Some(sampler) = self.samplers.get(&id).cloned() else {
            return Ok(());
        };
        for stage in 0..self.bindings.samplers.len() {
            for slot in 0..self.bindings.samplers[stage].len() {
                if self.bindings.samplers[stage][slot] == id {
                    self.device.set_samplers(stage as u32, slot as u32, &[Some(sampler.clone())])?;
                    self.bindings.bound_samplers[stage][slot] = id;
                }
            }
        }
        Ok(())
    }

    /// Unbinds native SRV references but retains logical object IDs so a
    /// destroyed/recreated object can be rebound in the same slots.
    fn unbind_srv_ids(&mut self, ids: &HashSet<u32>) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let mut unbound = 0u64;
        for stage in 0..self.bindings.srvs.len() {
            let mut slot = 0;
            while slot < self.bindings.bound_srvs[stage].len() {
                if !ids.contains(&self.bindings.bound_srvs[stage][slot]) {
                    slot += 1;
                    continue;
                }

                let start = slot;
                while slot < self.bindings.bound_srvs[stage].len()
                    && ids.contains(&self.bindings.bound_srvs[stage][slot])
                {
                    slot += 1;
                }
                let nulls = vec![None; slot - start];
                self.device.set_shader_resources(stage as u32, start as u32, &nulls)?;
                self.bindings.bound_srvs[stage][start..slot].fill(0);
                unbound += (slot - start) as u64;
            }
        }
        self.stats.srv_slots_unbound_on_destroy += unbound;
        Ok(())
    }

    /// Unbinds native sampler references while preserving their logical IDs.
    fn unbind_sampler_ids(&mut self, ids: &HashSet<u32>) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let mut unbound = 0u64;
        for stage in 0..self.bindings.samplers.len() {
            let mut slot = 0;
            while slot < self.bindings.bound_samplers[stage].len() {
                if !ids.contains(&self.bindings.bound_samplers[stage][slot]) {
                    slot += 1;
                    continue;
                }

                let start = slot;
                while slot < self.bindings.bound_samplers[stage].len()
                    && ids.contains(&self.bindings.bound_samplers[stage][slot])
                {
                    slot += 1;
                }
                let nulls = vec![None; slot - start];
                self.device.set_samplers(stage as u32, start as u32, &nulls)?;
                self.bindings.bound_samplers[stage][start..slot].fill(0);
                unbound += (slot - start) as u64;
            }
        }
        self.stats.sampler_slots_unbound_on_destroy += unbound;
        Ok(())
    }
}
