//! AMD's FSR 2 upscaler (temporal), through sp-fidelity: the FidelityFX
//! SDK's FSR 2.3 ported to Rust and WGSL, with a wgpu backend. It takes the
//! same inputs as MetalFX's temporal scaler (the scene before the HUD, its
//! depth and motion, jittered; see scene.rs), on the submit thread before
//! the present pass, which puts the HUD back over its image.
//!
//! It needs the device features `sp_fidelity_wgpu::required_features` lists
//! (subgroups, texture atomics, 16-bit normalized formats...); where the GPU
//! lacks one it is not offered. Making it compiles its passes (5 s on an M1
//! Pro): a thread of its own does, and frames take the bicubic filter
//! meanwhile.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use sp_fidelity::fsr2::{
    FFX_FSR2_ENABLE_DEPTH_INVERTED, FfxFsr2Context, FfxFsr2ContextDescription, FfxFsr2DispatchDescription,
    ffx_fsr2_context_create, ffx_fsr2_context_destroy, ffx_fsr2_context_dispatch,
};
use sp_fidelity::types::{
    FFX_RESOURCE_STATE_COMPUTE_READ, FFX_RESOURCE_STATE_UNORDERED_ACCESS, FfxDimensions2D, FfxFloatCoords2D,
};
use sp_fidelity_wgpu::FfxWgpuBackend;

use crate::present::{Scaled, Upscaled};
use crate::submit::Present;

/// Sizes hold this long before FSR is made for them.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// What a context is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    render: (u32, u32),
    output: (u32, u32),
    reversed: bool,
}

/// FSR made for one render size, output size and depth direction.
struct Made {
    key: Key,
    backend: Rc<RefCell<FfxWgpuBackend>>,
    context: FfxFsr2Context,
    image: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Drop for Made {
    fn drop(&mut self) {
        let _ = ffx_fsr2_context_destroy(&mut self.context);
    }
}

/// A `Made` handed from the thread that made it to the submit thread: it
/// holds every `Rc` of its backend (the context's interface is one), and
/// sp-fidelity keeps no state of the thread, so it moves whole.
struct Moved(Made);

unsafe impl Send for Moved {}

#[derive(Default)]
pub struct Fsr {
    made: Option<Made>,
    /// FSR being made, for that key.
    making: Option<(Key, std::thread::JoinHandle<Option<Moved>>)>,
    /// The sizes last asked for, and since when.
    wanted: Option<(Key, Instant)>,
    /// Sizes FSR could not be made for.
    failed: Option<Key>,
}

// Made empty with the window's output and moved once to the submit thread,
// which alone makes and uses FSR's backend (its `Rc`s are not `Send`).
unsafe impl Send for Fsr {}

impl Fsr {
    /// Whether `device` has what FSR needs.
    pub fn supported(device: &wgpu::Device) -> bool {
        device.features().contains(sp_fidelity_wgpu::required_features())
    }

    /// FSR's memory back while it is not used.
    pub fn release(&mut self) {
        self.made = None;
        self.wanted = None;
        // (One being made goes once made.)
        self.making = None;
    }

    /// The frame's scene upscaled to `size` (sharpened by `sharpness`, 0 to
    /// 1, `dt` seconds after the last), when it has one and FSR works here.
    pub fn upscale(
        &mut self,
        device: &wgpu::Device,
        src: &Present,
        size: (u32, u32),
        sharpness: f32,
        dt: f32,
    ) -> Option<Upscaled> {
        let scene = src.scene.as_ref()?;
        let camera = scene.camera;
        let key = Key { render: (src.width, src.height), output: size, reversed: camera.reversed };
        if self.made.as_ref().is_none_or(|m| m.key != key) {
            match self.wanted {
                Some((k, since)) if k == key && since.elapsed() >= SETTLE && self.failed != Some(key) => {}
                Some((k, _)) if k == key => return None,
                _ => {
                    self.wanted = Some((key, Instant::now()));
                    return None;
                }
            }
            match self.making.take() {
                Some((k, thread)) if k == key && thread.is_finished() => match thread.join().ok().flatten() {
                    Some(Moved(made)) => self.made = Some(made),
                    None => {
                        self.failed = Some(key);
                        return None;
                    }
                },
                Some((k, thread)) if k == key => {
                    self.making = Some((k, thread));
                    return None;
                }
                _ => {
                    self.made = None;
                    let device = device.clone();
                    let thread = glue5_runtime::qos::spawn("gpu fsr", glue5_runtime::qos::Qos::Initiated, move || {
                        // Failing, it leaves the bicubic filter.
                        glue5_runtime::exit::panic_recoverable();
                        let t = Instant::now();
                        let made = Made::new(&device, key)?;
                        log::info!(
                            "gpu: FSR {}x{} -> {}x{} ({:.0} ms to make)",
                            key.render.0,
                            key.render.1,
                            key.output.0,
                            key.output.1,
                            t.elapsed().as_secs_f64() * 1000.0
                        );
                        Some(Moved(made))
                    })
                    .expect("spawning the FSR thread");
                    self.making = Some((key, thread));
                    return None;
                }
            }
        }
        let made = self.made.as_mut()?;
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fsr") });
        let mut backend = made.backend.borrow_mut();
        let command_list = backend.ffx_get_command_list_wgpu(encoder);
        let mut resource =
            |texture: &wgpu::Texture, name: &str, state| backend.ffx_get_resource_wgpu(texture, name, state);
        let description = FfxFsr2DispatchDescription {
            command_list,
            color: resource(&scene.color, "Color", FFX_RESOURCE_STATE_COMPUTE_READ),
            depth: resource(&scene.depth, "Depth", FFX_RESOURCE_STATE_COMPUTE_READ),
            motion_vectors: resource(&scene.motion, "Motion", FFX_RESOURCE_STATE_COMPUTE_READ),
            output: resource(&made.image, "Output", FFX_RESOURCE_STATE_UNORDERED_ACCESS),
            jitter_offset: FfxFloatCoords2D { x: scene.jitter[0], y: scene.jitter[1] },
            // Already in the scene's pixels.
            motion_vector_scale: FfxFloatCoords2D { x: 1.0, y: 1.0 },
            render_size: FfxDimensions2D { width: src.width, height: src.height },
            enable_sharpening: sharpness > 0.0,
            sharpness: sharpness.clamp(0.0, 1.0),
            frame_time_delta: (dt * 1000.0).max(1.0),
            pre_exposure: 1.0,
            reset: scene.reset,
            camera_near: camera.near,
            camera_far: camera.far,
            camera_fov_angle_vertical: camera.fov_y.to_radians(),
            view_space_to_meters_factor: 1.0,
            ..Default::default()
        };
        // (The dispatch borrows the backend through the context.)
        drop(backend);
        let dispatched = ffx_fsr2_context_dispatch(&mut made.context, &description);
        let mut backend = made.backend.borrow_mut();
        let encoder = backend.ffx_take_command_list_wgpu(command_list)?;
        let failed = match (dispatched, backend.take_job_error()) {
            (Err(code), _) => Some(format!("error {code:#x}")),
            (Ok(()), Some(e)) => Some(format!("{e:?}")),
            (Ok(()), None) => None,
        };
        if let Some(why) = failed {
            crate::log_once("fsr dispatch", || format!("FSR: {why}"));
            return None;
        }
        Some(Upscaled { commands: vec![encoder.finish()], image: Scaled::Scene(made.view.clone()), between: None })
    }
}

impl Made {
    fn new(device: &wgpu::Device, key: Key) -> Option<Made> {
        let (backend, interface) = sp_fidelity_wgpu::ffx_get_interface_wgpu(device);
        let description = FfxFsr2ContextDescription {
            flags: if key.reversed { FFX_FSR2_ENABLE_DEPTH_INVERTED } else { 0 },
            max_render_size: FfxDimensions2D { width: key.render.0, height: key.render.1 },
            display_size: FfxDimensions2D { width: key.output.0, height: key.output.1 },
            fp_message: None,
            backend_interface: interface,
        };
        let mut context = FfxFsr2Context::default();
        if let Err(code) = ffx_fsr2_context_create(&mut context, &description) {
            log::warn!("gpu: FSR could not be made (error {code:#x}), bicubic instead");
            return None;
        }
        let image = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fsr output"),
            size: wgpu::Extent3d { width: key.output.0, height: key.output.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            // (Copied out by the tests.)
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = image.create_view(&Default::default());
        Some(Made { key, backend, context, image, view })
    }
}
