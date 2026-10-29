//! The temporal upscalers against a known image: a pattern drawn at the
//! scene's jitter (as scene.rs moves the game's draws), 48 frames scaled up
//! twice by the real upscaling paths, compared with the same pattern
//! supersampled at the output's size. Jittered, they must come much closer
//! to it than without: what decides it is the sign of the offset they are
//! told, which this pins down. Needs a GPU, so ignored by default:
//! `cargo test -r -p glue5-gpu -- --ignored upscale`.

use crate::scene::{Camera, SceneInputs, halton, phases};
use crate::submit::Present;

const INPUT: (u32, u32) = (640, 360);
const OUTPUT: (u32, u32) = (1280, 720);
const FRAMES: u32 = 48;

/// Rings and spokes with hard edges: aliased at one sample per pixel.
const PATTERN_WGSL: &str = "
struct U { jitter: vec2f, size: vec2f, samples: f32, pad: f32 }
@group(0) @binding(0) var<uniform> u: U;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  var p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3));
  return vec4f(p[i], 0, 1);
}
fn pattern(q: vec2f) -> f32 {
  let c = q - vec2f(0.5, 0.5);
  let r = length(c * vec2f(16.0 / 9.0, 1.0));
  let spoke = select(0.0, 1.0, fract(atan2(c.y, c.x) * 6.0 / 3.14159265) < 0.5);
  return spoke * select(0.2, 1.0, fract(r * 40.0) < 0.5);
}
@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {
  let n = i32(u.samples);
  var v = 0.0;
  for (var y = 0; y < n; y++) {
    for (var x = 0; x < n; x++) {
      let o = (vec2f(f32(x), f32(y)) + 0.5) / u.samples - 0.5;
      // Moved right and down by the jitter: a pixel sees what is that far
      // up and left of it.
      v += pattern((p.xy + o - u.jitter) / u.size);
    }
  }
  v /= f32(n * n);
  return vec4f(v, v, v, 1.0);
}";

struct Bench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pattern: wgpu::RenderPipeline,
    /// The pattern at the output's size, 8x8 samples a pixel.
    truth: Vec<f32>,
}

impl Bench {
    fn new() -> Bench {
        let adapter = pollster::block_on(crate::instance().request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: adapter.features() & crate::WANTED_FEATURES,
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .unwrap();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pattern"),
            source: wgpu::ShaderSource::Wgsl(PATTERN_WGSL.into()),
        });
        let pattern = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pattern"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Bgra8Unorm.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mut bench = Bench { device, queue, pattern, truth: Vec::new() };
        let truth = bench.texture(OUTPUT, wgpu::TextureFormat::Bgra8Unorm);
        bench.draw(&truth, [0.0; 2], 8);
        bench.truth = bench.read(&truth);
        bench
    }

    fn texture(&self, (w, h): (u32, u32), format: wgpu::TextureFormat) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn draw(&self, target: &wgpu::Texture, jitter: [f32; 2], samples: u32) {
        let size = [target.width() as f32, target.height() as f32];
        let data = [jitter[0], jitter[1], size[0], size[1], samples as f32, 0.0, 0.0, 0.0];
        let uniforms = wgpu::util::DeviceExt::create_buffer_init(
            &self.device,
            &wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&data),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pattern.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let view = target.create_view(&Default::default());
        let mut pass = crate::present::begin_pass(&mut encoder, &view, wgpu::Color::BLACK);
        pass.set_pipeline(&self.pattern);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        self.queue.submit([encoder.finish()]);
    }

    /// The first channel of `texture` (8-bit unorm or half float).
    fn read(&self, texture: &wgpu::Texture) -> Vec<f32> {
        let (w, h) = (texture.width(), texture.height());
        let texel = texture.format().block_copy_size(None).unwrap();
        let bpr = (w * texel).next_multiple_of(256);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            texture.size(),
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        let half = texture.format() == wgpu::TextureFormat::Rgba16Float;
        let mut values = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let at = (y * bpr + x * texel) as usize;
                values.push(if half {
                    half::f16::from_le_bytes([data[at], data[at + 1]]).to_f32()
                } else {
                    data[at] as f32 / 255.0
                });
            }
        }
        values
    }

    /// The frames an upscaler gets: the pattern (jittered or not) at the
    /// input's size, a flat depth, no motion.
    fn frames(&self, jittered: bool) -> impl Iterator<Item = Present> + '_ {
        let color = self.texture(INPUT, wgpu::TextureFormat::Bgra8Unorm);
        let depth = self.texture(INPUT, wgpu::TextureFormat::Depth32Float);
        let motion = self.texture(INPUT, wgpu::TextureFormat::Rg16Float);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &motion.create_view(&Default::default()),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.create_view(&Default::default()),
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.5), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        }));
        self.queue.submit([encoder.finish()]);
        (0..FRAMES).map(move |i| {
            let phase = i % phases(2.0) + 1;
            let jitter = if jittered { [halton(phase, 2) - 0.5, halton(phase, 3) - 0.5] } else { [0.0; 2] };
            self.draw(&color, jitter, 1);
            let camera = Camera { near: 0.15, far: 1000.0, fov_y: 60.0, aspect: 16.0 / 9.0, reversed: true };
            Present {
                view: color.create_view(&Default::default()),
                texture: color.clone(),
                width: INPUT.0,
                height: INPUT.1,
                camera: None,
                scene: Some(SceneInputs {
                    color: color.clone(),
                    depth: depth.clone(),
                    motion: motion.clone(),
                    jitter,
                    reset: i == 0,
                    camera,
                }),
            }
        })
    }

    /// Mean squared error from the truth of what `upscale` made of the
    /// frames, jittered or not.
    fn error(&self, jittered: bool, mut upscale: impl FnMut(&Present) -> Option<crate::present::Upscaled>) -> f32 {
        let mut last = None;
        for frame in self.frames(jittered) {
            let up = ready(&mut upscale, &frame);
            let up = up.expect("upscaled");
            self.queue.submit(up.commands);
            let crate::present::Scaled::Scene(view) = up.image else { panic!("not the scene") };
            last = Some(view.texture().clone());
        }
        let out = self.read(&last.unwrap());
        out.iter().zip(&self.truth).map(|(a, b)| (a - b) * (a - b)).sum::<f32>() / out.len() as f32
    }
}

/// What `upscale` makes of `frame`, once it has made its effects (they
/// wait for their sizes to hold a moment, FSR compiles on a thread).
fn ready(
    upscale: &mut dyn FnMut(&Present) -> Option<crate::present::Upscaled>,
    frame: &Present,
) -> Option<crate::present::Upscaled> {
    for _ in 0..200 {
        if let Some(up) = upscale(frame) {
            return Some(up);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    None
}

#[test]
#[ignore]
fn upscale_fsr_converges_with_the_jitter() {
    let bench = Bench::new();
    let mut errors = Vec::new();
    for jittered in [false, true] {
        let mut fsr = crate::fsr::Fsr::default();
        errors.push(bench.error(jittered, |p| fsr.upscale(&bench.device, p, OUTPUT, 0.0, 1.0 / 60.0)));
    }
    println!("FSR: error {:.5} still, {:.5} jittered", errors[0], errors[1]);
    assert!(errors[1] < errors[0] * 0.5, "{errors:?}");
}

#[cfg(target_vendor = "apple")]
#[test]
#[ignore]
fn upscale_metalfx_converges_with_the_jitter() {
    use crate::metalfx::{Kind, MetalFx};
    let bench = Bench::new();
    let mut errors = Vec::new();
    for jittered in [false, true] {
        let mut fx = MetalFx::new(&bench.device);
        if !fx.supports(Kind::Temporal) {
            return;
        }
        errors.push(bench.error(jittered, |p| fx.upscale(&bench.device, p, OUTPUT, Some(Kind::Temporal), false, 0.0)));
    }
    println!("MetalFX: error {:.5} still, {:.5} jittered", errors[0], errors[1]);
    assert!(errors[1] < errors[0] * 0.5, "{errors:?}");
}

/// What each upscaler costs a frame (the GPU waited for after each), at
/// 1280x720 and 640x360 to 2560x1440: printed, not checked.
#[test]
#[ignore]
fn upscale_cost() {
    let bench = Bench::new();
    let frames = |input: (u32, u32), upscale: &mut dyn FnMut(&Present) -> Option<crate::present::Upscaled>| {
        let color = bench.texture(input, wgpu::TextureFormat::Bgra8Unorm);
        let depth = bench.texture(input, wgpu::TextureFormat::Depth32Float);
        let motion = bench.texture(input, wgpu::TextureFormat::Rg16Float);
        let present = |i: u32| Present {
            view: color.create_view(&Default::default()),
            texture: color.clone(),
            width: input.0,
            height: input.1,
            camera: None,
            scene: Some(SceneInputs {
                color: color.clone(),
                depth: depth.clone(),
                motion: motion.clone(),
                jitter: [halton(i % phases(2.0) + 1, 2) - 0.5, halton(i % phases(2.0) + 1, 3) - 0.5],
                reset: i == 0,
                camera: Camera { near: 0.15, far: 1000.0, fov_y: 60.0, aspect: 16.0 / 9.0, reversed: true },
            }),
        };
        let mut times = Vec::new();
        for i in 0..80 {
            let p = present(i);
            let up = ready(upscale, &p);
            let t = std::time::Instant::now();
            bench.queue.submit(crate::present::timed(&bench.device, up.expect("upscaled").commands));
            bench.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            if i == 19 {
                crate::present::take_upscaling();
            }
            if i >= 20 {
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        println!("  (measured on the GPU: {:?} ms a frame)", crate::present::take_upscaling());
        times[times.len() / 2]
    };
    for input in [(1280, 720), (960, 540)] {
        let mut fsr = crate::fsr::Fsr::default();
        let ms = frames(input, &mut |p| fsr.upscale(&bench.device, p, (2560, 1440), 0.5, 1.0 / 60.0));
        println!("FSR {input:?} -> 2560x1440: {ms:.2} ms");
        #[cfg(target_vendor = "apple")]
        {
            use crate::metalfx::{Kind, MetalFx};
            for kind in [Kind::Spatial, Kind::Temporal] {
                let mut fx = MetalFx::new(&bench.device);
                let ms = frames(input, &mut |p| fx.upscale(&bench.device, p, (2560, 1440), Some(kind), false, 0.0));
                println!("MetalFX {kind:?} {input:?} -> 2560x1440: {ms:.2} ms");
            }
        }
    }
}
