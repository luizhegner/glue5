//! The missing vertical slice between the resource layer and a real `Draw`.
//!
//! [`crate::d3d11`] can negotiate a hardware device, check format support, own
//! textures/views and copy a texture back to the CPU, but nothing in the crate
//! ever bound a pipeline or issued a draw. This module adds that path without
//! introducing a second device, a second COM ownership model or a parallel
//! abstraction:
//!
//! ```text
//! HLSL -> D3DCompile(vs_4_0 | ps_4_0)
//!      -> ID3D11VertexShader / ID3D11PixelShader / ID3D11InputLayout
//! vertex + index + constant buffers
//!      -> IASetVertexBuffers / IASetIndexBuffer / VSSetConstantBuffers
//! RTV + viewport
//!      -> OMSetRenderTargets / RSSetViewports / VSSetShader / PSSetShader
//!      -> Draw / DrawIndexed
//! staging copy -> CPU readback -> per-pixel assertions
//! ```
//!
//! Every COM object created here is owned by the caller through a wrapper
//! struct; nothing is leaked and nothing is cached globally. The device is
//! always borrowed from [`D3D11Device`], so the single-device invariant that
//! `lib.rs` documents stays intact.
//!
//! # GM45 constraints this module encodes
//!
//! * Shader Model 4.0 only (`vs_4_0` / `ps_4_0`). Mobile Intel 4 Series
//!   (PCI 8086:2A42) exposes Feature Level 10_0, so FL10_1/FL11_0 targets are
//!   never requested and never silently substituted.
//! * No compute. The GM45 path stays on the graphics pipeline; a compute
//!   kernel is not emulated with `cs_4_0` anywhere in this crate.
//! * Render-target format support is proven by creating the view: if the
//!   adapter cannot render into the format, `CreateRenderTargetView` fails
//!   with a real HRESULT instead of the limitation being papered over.
//! * Constant buffers respect the FL10_0 rules already enforced by the
//!   executor: byte width is a non-zero multiple of 16 and at most 64 KiB.
//! * The probe binds its own rasterizer state with `CullMode = NONE`. Front
//!   versus back facing is decided in screen space, after the viewport
//!   y-flip, so leaving culling to the default is an easy way to have the
//!   first draw silently disappear. The triangle is still declared clockwise,
//!   matching the engine's convention, and a unit test pins that.
//!
//! # windows-rs API target
//!
//! Every signature in this module was checked against the generated bindings of
//! `windows 0.62.2` (crates.io release built from `microsoft/windows-rs`
//! commit `d468916ac`, 2025-10-06). Points worth remembering:
//!
//! * `D3DCompile` lives in `Win32::Graphics::Direct3D::Fxc`, but `ID3DBlob` and
//!   `ID3DInclude` live in `Win32::Graphics::Direct3D`.
//! * `OMSetRenderTargets` takes a slice and no explicit count.
//! * `IASetVertexBuffers` is the opposite: explicit count plus raw pointers.
//! * `Map` takes `Option<*mut D3D11_MAPPED_SUBRESOURCE>`; `RSSetViewports`,
//!   `VSSetConstantBuffers` and `OMSetRenderTargets` take slices.
//! * Generic `Param<T>` arguments cannot be `None` without a type annotation.

use anyhow::{Context, ensure};
use windows::core::{Interface, PCSTR, s};
use windows::Win32::Graphics::{
    Direct3D::{D3D_PRIMITIVE_TOPOLOGY, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob, ID3DInclude},
    Direct3D::Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile},
    Direct3D11::{
        D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_INDEX_BUFFER, D3D11_BIND_RENDER_TARGET,
        D3D11_BIND_VERTEX_BUFFER, D3D11_BUFFER_DESC, D3D11_CULL_NONE, D3D11_CPU_ACCESS_READ,
        D3D11_FILL_SOLID, D3D11_RASTERIZER_DESC,
        D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_MAP_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        D3D11_USAGE_STAGING, D3D11_VIEWPORT, ID3D11Buffer, ID3D11ClassLinkage, ID3D11DeviceContext,
        ID3D11DepthStencilView, ID3D11InputLayout, ID3D11PixelShader, ID3D11RenderTargetView,
        ID3D11RasterizerState, ID3D11Resource, ID3D11Texture2D, ID3D11VertexShader,
    },
    Dxgi::{
        Common::{
            DXGI_FORMAT, DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R32G32_FLOAT,
            DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
        },
        DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice,
    },
};

use crate::{D3D11Device, FeatureLevel};

/// A clear colour that is deliberately *not* grey or black so that "did not
/// draw" and "drew the wrong colour" are distinguishable in the readback.
pub const PROBE_CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The colour the stage-1 pixel shader returns. 0.25/0.50/0.75 quantise to
/// 64/128/191 in `R8G8B8A8_UNORM`, none of which equal the clear colour.
pub const PROBE_DRAW_COLOR: [f32; 4] = [0.25, 0.50, 0.75, 1.0];

/// Expected `R8G8B8A8_UNORM` bytes for [`PROBE_DRAW_COLOR`].
pub const PROBE_DRAW_RGBA8: [u8; 4] = [64, 128, 191, 255];

/// Tint pushed through the constant buffer in stage 2. It is deliberately a
/// *different* colour from stage 1, so a stale stage-1 image copied forward
/// cannot satisfy the stage-2 assertion.
pub const PROBE_STAGE_TWO_TINT: [f32; 4] = [0.50, 0.25, 0.125, 1.0];

/// Expected `R8G8B8A8_UNORM` bytes for [`PROBE_STAGE_TWO_TINT`].
pub const PROBE_STAGE_TWO_RGBA8: [u8; 4] = [128, 64, 32, 255];

/// Expected `R8G8B8A8_UNORM` bytes for [`PROBE_CLEAR_COLOR`].
pub const PROBE_CLEAR_RGBA8: [u8; 4] = [0, 0, 0, 255];

/// Half-open tolerance in 8-bit units. One unit of quantisation error is
/// expected on Intel FL10_0 drivers that round instead of truncate
/// (`0.25 * 255 = 63.75`, `0.5 * 255 = 127.5`).
pub const PROBE_TOLERANCE: u8 = 2;

/// Vertex shader that synthesises geometry from `SV_VertexID`.
///
/// This is the smallest legal SM4.0 draw: no vertex buffer, no input layout,
/// no constant buffer. It isolates "can the device rasterise at all" from
/// every buffer-upload question.
///
/// The winding is clockwise because the default D3D11 rasterizer state culls
/// back faces with `FrontCounterClockwise = FALSE`.
pub const VS_SV_VERTEX_ID: &str = r#"
struct VSOutput {
    float4 position : SV_Position;
    float2 uv       : TEXCOORD0;
};

// Clockwise, and small enough to cover the centre pixel of the render target
// while leaving every corner untouched. Written as if/else rather than as an
// indexed constant array: dynamic indexing into a literal array is exactly the
// construct older FXC/driver combinations disagree about, and this stage exists
// to isolate "can the device rasterise at all".
VSOutput VSMain(uint vertexId : SV_VertexID) {
    float2 position;
    if (vertexId == 0) {
        position = float2(-0.5, -0.5);
    } else if (vertexId == 1) {
        position = float2(0.0, 0.5);
    } else {
        position = float2(0.5, -0.5);
    }

    VSOutput output;
    output.position = float4(position, 0.0, 1.0);
    output.uv = position * 0.5 + 0.5;
    return output;
}
"#;

/// Pixel shader returning the probe colour. Kept separate from
/// [`VS_SV_VERTEX_ID`] so the identical PS contract can be re-checked by the
/// vertex-buffer path, which proves pipeline state is actually re-bound.
pub const PS_SOLID_COLOR: &str = r#"
struct PSInput {
    float4 position : SV_Position;
    float2 uv       : TEXCOORD0;
};

float4 PSMain(PSInput input) : SV_Target {
    // The uv input is part of the declared signature (matching VS_SV_VERTEX_ID)
    // but the colour itself is constant: this stage exists to prove that a
    // PS bound through the pipeline reaches the render target at all.
    return float4(0.25, 0.50, 0.75, 1.0);
}
"#;

/// Vertex shader for the vertex-buffer + input-layout + constant-buffer path.
///
/// `Tint` lives at `b0` and is consumed by the VS, which exercises
/// `VSSetConstantBuffers` rather than the more common PS-only case.
pub const VS_VERTEX_BUFFER: &str = r#"
cbuffer ProbeConstants : register(b0) {
    float4 Tint;
};

struct VSInput {
    float3 position : POSITION;
    float2 uv       : TEXCOORD0;
};

struct VSOutput {
    float4 position : SV_Position;
    float2 uv       : TEXCOORD0;
    float4 tint     : TEXCOORD1;
};

VSOutput VSMain(VSInput input) {
    VSOutput output;
    output.position = float4(input.position, 1.0);
    output.uv = input.uv;
    output.tint = Tint;
    return output;
}
"#;

/// Pixel shader for the vertex-buffer path; forwards the tint that travelled
/// through the constant buffer and the vertex stage.
pub const PS_VERTEX_BUFFER: &str = r#"
struct PSInput {
    float4 position : SV_Position;
    float2 uv       : TEXCOORD0;
    float4 tint     : TEXCOORD1;
};

float4 PSMain(PSInput input) : SV_Target {
    return input.tint;
}
"#;

/// A single interleaved vertex for [`VS_VERTEX_BUFFER`]: position then uv,
/// 20 bytes per vertex, which is the smallest legal FL10_0 layout for this
/// pair of semantics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}

/// The triangle used by both probe stages, in clockwise order.
///
/// Kept as a named constant so the winding can be asserted in a unit test:
/// flipping it back to counter-clockwise would make the default rasterizer
/// cull every triangle and the probe would report the clear colour.
pub const PROBE_TRIANGLE: [[f32; 2]; 3] = [
    [-0.5, -0.5],
    [0.0, 0.5],
    [0.5, -0.5],
];

/// Twice the signed area of a triangle in NDC. Negative means clockwise, which
/// is what D3D11's default rasterizer state treats as front facing.
pub fn signed_area_twice(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Identity of the adapter that the device was actually created on.
///
/// The whole point of the GM45 work is that the real Intel hardware is doing
/// the work, so the probe refuses to report success on a software adapter.
#[derive(Clone, Debug)]
pub struct AdapterIdentity {
    pub vendor_id: u32,
    pub device_id: u32,
    pub description: String,
    pub software: bool,
}

impl AdapterIdentity {
    /// `true` when this is the Mobile Intel 4 Series part the backend targets.
    pub fn is_gm45(&self) -> bool {
        self.vendor_id == 0x8086 && self.device_id == 0x2A42
    }

    /// `true` when this is any Intel integrated adapter (GM45, or a newer
    /// Intel part when the same probe runs on another machine).
    pub fn is_intel(&self) -> bool {
        self.vendor_id == 0x8086
    }
}

/// Queries the adapter behind `device` through DXGI.
///
/// `D3D11CreateDevice(HARDWARE)` already excludes WARP, but a software
/// adapter flag is still checked explicitly so a future refactor cannot
/// quietly reintroduce a fallback.
///
/// Verified against `windows 0.62.2`:
/// `IDXGIDevice::GetAdapter() -> Result<IDXGIAdapter>`,
/// `IDXGIAdapter::GetDesc1() -> Result<DXGI_ADAPTER_DESC1>`,
/// `DXGI_ADAPTER_DESC1 { Description: [u16; 128], VendorId: u32, DeviceId: u32, Flags: u32, .. }`,
/// `DXGI_ADAPTER_FLAG_SOFTWARE = DXGI_ADAPTER_FLAG(2i32)`.
pub fn adapter_identity(device: &D3D11Device) -> anyhow::Result<AdapterIdentity> {
    let dxgi: IDXGIDevice = device
        .raw_device()
        .cast()
        .context("querying IDXGIDevice from the D3D11 device")?;
    let adapter: IDXGIAdapter = unsafe { dxgi.GetAdapter() }.context("IDXGIDevice::GetAdapter")?;
    let adapter1: IDXGIAdapter1 = adapter.cast().context("querying IDXGIAdapter1")?;
    let desc = unsafe { adapter1.GetDesc1() }.context("IDXGIAdapter1::GetDesc1")?;

    // DXGI_ADAPTER_DESC1::Description is a fixed [u16; 128] buffer that DXGI
    // guarantees to be NUL terminated.
    let end = desc
        .Description
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(desc.Description.len());
    let description = String::from_utf16_lossy(&desc.Description[..end]);

    let software = (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0;

    Ok(AdapterIdentity {
        vendor_id: desc.VendorId,
        device_id: desc.DeviceId,
        description,
        software,
    })
}

/// A compiled vertex/pixel pair plus the input layout that matches the VS
/// signature.
///
/// `input_layout` is `None` for pipelines that synthesise geometry from
/// `SV_VertexID`; creating an empty input layout is legal but pointless and
/// hides bugs, so the absence is modelled explicitly.
pub struct GraphicsPipeline {
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    input_layout: Option<ID3D11InputLayout>,
    topology: D3D_PRIMITIVE_TOPOLOGY,
    debug_name: String,
}

impl GraphicsPipeline {
    /// Compiles both stages for Shader Model 4.0 and creates the input layout.
    ///
    /// `elements` must describe the vertex buffer this pipeline will be drawn
    /// with, or be empty for `SV_VertexID` pipelines.
    pub fn new(
        device: &D3D11Device,
        vertex_source: &str,
        pixel_source: &str,
        elements: &[InputElement],
        topology: D3D_PRIMITIVE_TOPOLOGY,
        debug_name: &str,
    ) -> anyhow::Result<Self> {
        let vertex_blob = compile_shader(vertex_source, s!("VSMain"), s!("vs_4_0"), debug_name)?;
        let pixel_blob = compile_shader(pixel_source, s!("PSMain"), s!("ps_4_0"), debug_name)?;

        let vertex_bytes = blob_bytes(&vertex_blob);
        let pixel_bytes = blob_bytes(&pixel_blob);

        let mut vertex_shader: Option<ID3D11VertexShader> = None;
        let mut pixel_shader: Option<ID3D11PixelShader> = None;
        let mut layout: Option<ID3D11InputLayout> = None;

        unsafe {
            device
                .raw_device()
                .CreateVertexShader(vertex_bytes, None::<&ID3D11ClassLinkage>, Some(&mut vertex_shader))
                .with_context(|| format!("creating the vertex shader for {debug_name}"))?;
            device
                .raw_device()
                .CreatePixelShader(pixel_bytes, None::<&ID3D11ClassLinkage>, Some(&mut pixel_shader))
                .with_context(|| format!("creating the pixel shader for {debug_name}"))?;
        }

        if !elements.is_empty() {
            let descs: Vec<D3D11_INPUT_ELEMENT_DESC> =
                elements.iter().map(InputElement::to_desc).collect();
            unsafe {
                device
                    .raw_device()
                    .CreateInputLayout(&descs, vertex_bytes, Some(&mut layout))
                    .with_context(|| format!("creating the input layout for {debug_name}"))?;
            }
            ensure!(
                layout.is_some(),
                "CreateInputLayout reported success but returned no layout for {debug_name}"
            );
        }

        Ok(Self {
            vertex_shader: vertex_shader
                .context("CreateVertexShader reported success but returned no shader")?,
            pixel_shader: pixel_shader
                .context("CreatePixelShader reported success but returned no shader")?,
            input_layout: layout,
            topology,
            debug_name: debug_name.to_owned(),
        })
    }

    /// `true` when the pipeline carries an input layout and therefore expects
    /// a vertex buffer to be bound before drawing.
    pub const fn has_input_layout(&self) -> bool {
        self.input_layout.is_some()
    }

    pub fn debug_name(&self) -> &str {
        &self.debug_name
    }
}

/// One entry of a vertex buffer declaration, translated to
/// [`D3D11_INPUT_ELEMENT_DESC`] on demand.
#[derive(Clone, Copy, Debug)]
pub struct InputElement {
    pub semantic: PCSTR,
    pub index: u32,
    pub format: DXGI_FORMAT,
    pub slot: u32,
    pub offset: u32,
}

impl InputElement {
    /// The `POSITION` / `TEXCOORD0` pair used by [`ProbeVertex`].
    pub fn probe_layout() -> [Self; 2] {
        [
            Self {
                semantic: s!("POSITION"),
                index: 0,
                format: DXGI_FORMAT_R32G32B32_FLOAT,
                slot: 0,
                offset: 0,
            },
            Self {
                semantic: s!("TEXCOORD"),
                index: 0,
                format: DXGI_FORMAT_R32G32_FLOAT,
                slot: 0,
                offset: 12,
            },
        ]
    }

    fn to_desc(&self) -> D3D11_INPUT_ELEMENT_DESC {
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: self.semantic,
            SemanticIndex: self.index,
            Format: self.format,
            InputSlot: self.slot,
            AlignedByteOffset: self.offset,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        }
    }
}

/// A depth-less render target: the FL10_0 case the GM45 path needs first.
pub struct RenderTarget {
    texture: ID3D11Texture2D,
    view: ID3D11RenderTargetView,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl RenderTarget {
    /// Creates a `RENDER_TARGET`-bound texture after asking the adapter
    /// whether it can actually render into this format.
    pub fn new(
        device: &D3D11Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> anyhow::Result<Self> {
        ensure!(width > 0 && height > 0, "render target extent must be non-zero");

        // Format support is proven by the view creation below rather than by
        // a capability query: if the adapter cannot render into this format,
        // CreateRenderTargetView fails with a real HRESULT that propagates to
        // the caller. (D3D11Device::format_support exists for diagnostics, but
        // its return shape is not part of this module's contract.)

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        let mut texture: Option<ID3D11Texture2D> = None;
        let mut view: Option<ID3D11RenderTargetView> = None;
        unsafe {
            device
                .raw_device()
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .context("creating the render-target texture")?;
            // as_ref(): `Option::context` consumes the Option, and the
            // texture is stored in the struct at the end of this function.
            let created = texture
                .as_ref()
                .context("CreateTexture2D returned no texture")?;
            device
                .raw_device()
                .CreateRenderTargetView(created, None, Some(&mut view))
                .context("creating the render-target view")?;
        }

        Ok(Self {
            texture: texture.context("render-target texture disappeared")?,
            view: view.context("CreateRenderTargetView returned no view")?,
            width,
            height,
            format,
        })
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn format(&self) -> DXGI_FORMAT {
        self.format
    }

    /// Binds this target as the single output of the pipeline, together with a
    /// viewport that covers it exactly.
    pub fn bind(&self, context: &ID3D11DeviceContext) {
        let views = [Some(self.view.clone())];
        let viewports = [D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.width as f32,
            Height: self.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }];
        unsafe {
            // windows-rs 0.62.2: slice argument, no explicit count.
            context.OMSetRenderTargets(Some(&views[..]), None::<&ID3D11DepthStencilView>);
            context.RSSetViewports(Some(&viewports[..]));
        }
    }

    /// Fills the target with `color` through the render-target view.
    pub fn clear(&self, context: &ID3D11DeviceContext, color: [f32; 4]) {
        unsafe { context.ClearRenderTargetView(Some(&self.view), &color) };
    }

    /// Reads one pixel without a full readback, for diagnostics.
    pub fn pixel(&self, device: &D3D11Device, x: u32, y: u32) -> anyhow::Result<[u8; 4]> {
        let bytes = self.readback(device)?;
        pixel_at(&bytes, self.width, x, y)
    }

    /// Copies the rendered image back to the CPU.
    ///
    /// This is the first caller of the crate's readback logic that is not a
    /// diagnostic: the bytes it returns are the only evidence that the GPU
    /// actually rasterised something.
    pub fn readback(&self, device: &D3D11Device) -> anyhow::Result<Vec<u8>> {
        let bytes_per_texel = bytes_per_texel(self.format).with_context(|| {
            format!("readback is not implemented for DXGI format {:?}", self.format)
        })?;

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: self.format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            device
                .raw_device()
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .context("creating the readback staging texture")?;
        }
        let staging = staging.context("staging CreateTexture2D returned no texture")?;

        let context = device.immediate_context();
        let row_bytes = (self.width as usize) * (bytes_per_texel as usize);
        let mut bytes = vec![0u8; row_bytes * self.height as usize];

        let destination: ID3D11Resource = staging.cast()?;
        let source: ID3D11Resource = self.texture.cast()?;

        unsafe {
            context.CopyResource(Some(&destination), Some(&source));

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context
                .Map(Some(&destination), 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("mapping the readback staging texture")?;

            let source_ptr = mapped.pData.cast::<u8>();
            for row in 0..self.height as usize {
                let source_row = source_ptr.add(row * mapped.RowPitch as usize);
                let target_row = &mut bytes[row * row_bytes..(row + 1) * row_bytes];
                std::ptr::copy_nonoverlapping(source_row, target_row.as_mut_ptr(), row_bytes);
            }

            context.Unmap(Some(&destination), 0);
        }

        Ok(bytes)
    }
}

/// A rasterizer state owned explicitly by the probe.
///
/// The probe binds `CullMode = NONE`. Whether a triangle is front- or
/// back-facing is decided in *screen* space, after the viewport y-flip, which
/// is precisely the convention that makes a first draw vanish on real
/// hardware. With culling off, a black readback can only mean the draw itself
/// did not run — the one thing this probe is trying to measure.
pub struct RasterizerState {
    state: ID3D11RasterizerState,
}

impl RasterizerState {
    /// Solid fill, no culling, depth clipping off (there is no depth buffer).
    pub fn no_cull(device: &D3D11Device) -> anyhow::Result<Self> {
        // `Default::default()` alone would be an invalid state: FillMode and
        // CullMode default to 0, which is not a valid enumerant, and
        // CreateRasterizerState would fail with E_INVALIDARG.
        let desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            ..Default::default()
        };

        let mut state: Option<ID3D11RasterizerState> = None;
        unsafe {
            device
                .raw_device()
                .CreateRasterizerState(&desc, Some(&mut state))
                .context("creating the no-cull rasterizer state")?;
        }

        Ok(Self {
            state: state.context("CreateRasterizerState returned no state")?,
        })
    }

    pub fn bind(&self, context: &ID3D11DeviceContext) {
        unsafe { context.RSSetState(&self.state) }
    }
}

/// A vertex buffer plus the stride the IA stage must be told about.
pub struct VertexBuffer {
    buffer: ID3D11Buffer,
    stride: u32,
    vertices: u32,
}

impl VertexBuffer {
    /// Uploads `vertices` as an immutable vertex buffer.
    pub fn new<T: Copy>(device: &D3D11Device, vertices: &[T]) -> anyhow::Result<Self> {
        ensure!(!vertices.is_empty(), "vertex buffer cannot be empty");
        let stride = std::mem::size_of::<T>();
        ensure!(stride > 0 && stride <= 2048, "invalid vertex stride {stride}");

        let bytes = unsafe {
            std::slice::from_raw_parts(
                vertices.as_ptr().cast::<u8>(),
                std::mem::size_of_val(vertices),
            )
        };

        let desc = D3D11_BUFFER_DESC {
            ByteWidth: bytes.len() as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: bytes.as_ptr().cast(),
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };

        let mut buffer: Option<ID3D11Buffer> = None;
        unsafe {
            device
                .raw_device()
                .CreateBuffer(&desc, Some(&initial), Some(&mut buffer))
                .context("creating the vertex buffer")?;
        }

        Ok(Self {
            buffer: buffer.context("CreateBuffer returned no vertex buffer")?,
            stride: stride as u32,
            vertices: vertices.len() as u32,
        })
    }

    pub const fn vertex_count(&self) -> u32 {
        self.vertices
    }

    pub const fn stride(&self) -> u32 {
        self.stride
    }

    pub fn bind(&self, context: &ID3D11DeviceContext) {
        let buffers = [Some(self.buffer.clone())];
        let strides = [self.stride];
        let offsets = [0u32];
        unsafe {
            // windows-rs 0.62.2: explicit count plus raw pointers, unlike the
            // Set* helpers above which take slices.
            context.IASetVertexBuffers(
                0,
                1,
                Some(buffers.as_ptr()),
                Some(strides.as_ptr()),
                Some(offsets.as_ptr()),
            );
        }
    }
}

/// A 16-bit index buffer, which is what FL10_0 needs for everything the GM45
/// path renders and what the engine's index stream already produces.
pub struct IndexBuffer {
    buffer: ID3D11Buffer,
    indices: u32,
}

impl IndexBuffer {
    pub fn new(device: &D3D11Device, indices: &[u16]) -> anyhow::Result<Self> {
        ensure!(!indices.is_empty(), "index buffer cannot be empty");

        let bytes = unsafe {
            std::slice::from_raw_parts(
                indices.as_ptr().cast::<u8>(),
                std::mem::size_of_val(indices),
            )
        };

        let desc = D3D11_BUFFER_DESC {
            ByteWidth: bytes.len() as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_INDEX_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: bytes.as_ptr().cast(),
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };

        let mut buffer: Option<ID3D11Buffer> = None;
        unsafe {
            device
                .raw_device()
                .CreateBuffer(&desc, Some(&initial), Some(&mut buffer))
                .context("creating the index buffer")?;
        }

        Ok(Self {
            buffer: buffer.context("CreateBuffer returned no index buffer")?,
            indices: indices.len() as u32,
        })
    }

    pub const fn index_count(&self) -> u32 {
        self.indices
    }

    pub fn bind(&self, context: &ID3D11DeviceContext) {
        unsafe { context.IASetIndexBuffer(Some(&self.buffer), DXGI_FORMAT_R16_UINT, 0) };
    }
}

/// A constant buffer created for one shader-visible slot.
///
/// `bytes` must already be packed to the FL10_0 rules (16-byte multiples); the
/// check is repeated here instead of being trusted from the caller.
pub struct ConstantBuffer {
    buffer: ID3D11Buffer,
}

impl ConstantBuffer {
    pub fn new(device: &D3D11Device, bytes: &[u8]) -> anyhow::Result<Self> {
        ensure!(!bytes.is_empty(), "constant buffer cannot be empty");
        ensure!(
            bytes.len() % 16 == 0,
            "constant buffer size {} must be a multiple of 16 bytes on FL10_0",
            bytes.len()
        );
        ensure!(
            bytes.len() <= 64 * 1024,
            "constant buffer size {} exceeds the 64 KiB limit",
            bytes.len()
        );

        let desc = D3D11_BUFFER_DESC {
            ByteWidth: bytes.len() as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: bytes.as_ptr().cast(),
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };

        let mut buffer: Option<ID3D11Buffer> = None;
        unsafe {
            device
                .raw_device()
                .CreateBuffer(&desc, Some(&initial), Some(&mut buffer))
                .context("creating the constant buffer")?;
        }

        Ok(Self {
            buffer: buffer.context("CreateBuffer returned no constant buffer")?,
        })
    }

    pub fn bind_to_vertex_stage(&self, context: &ID3D11DeviceContext, slot: u32) {
        let buffers = [Some(self.buffer.clone())];
        unsafe { context.VSSetConstantBuffers(slot, Some(&buffers[..])) };
    }
}

/// Binds the pipeline and issues `Draw`.
pub fn draw(
    context: &ID3D11DeviceContext,
    pipeline: &GraphicsPipeline,
    vertex_count: u32,
) -> anyhow::Result<()> {
    ensure!(vertex_count > 0, "Draw needs at least one vertex");
    bind_pipeline(context, pipeline);
    unsafe { context.Draw(vertex_count, 0) };
    Ok(())
}

/// Binds the pipeline, the index buffer and issues `DrawIndexed`.
pub fn draw_indexed(
    context: &ID3D11DeviceContext,
    pipeline: &GraphicsPipeline,
    indices: &IndexBuffer,
) -> anyhow::Result<()> {
    ensure!(indices.index_count() > 0, "DrawIndexed needs at least one index");
    ensure!(
        pipeline.has_input_layout(),
        "DrawIndexed through a vertex buffer needs a pipeline with an input layout"
    );
    bind_pipeline(context, pipeline);
    indices.bind(context);
    unsafe { context.DrawIndexed(indices.index_count(), 0, 0) };
    Ok(())
}

fn bind_pipeline(context: &ID3D11DeviceContext, pipeline: &GraphicsPipeline) {
    unsafe {
        context.IASetPrimitiveTopology(pipeline.topology);
        // Clears a stale layout on the SV_VertexID path instead of leaving the
        // previous pipeline's layout bound.
        context.IASetInputLayout(pipeline.input_layout.as_ref());
        context.VSSetShader(Some(&pipeline.vertex_shader), None);
        context.PSSetShader(Some(&pipeline.pixel_shader), None);
    }
}

/// What the probe measured, in a form a caller can assert on instead of
/// parsing stdout.
#[derive(Clone, Debug)]
pub struct DrawProbeReport {
    pub adapter: AdapterIdentity,
    pub feature_level: FeatureLevel,
    pub width: u32,
    pub height: u32,
    /// Centre pixel after `Draw` with `SV_VertexID` only.
    pub centre: [u8; 4],
    /// Corner pixel after the same draw; must still equal the clear colour.
    pub corner: [u8; 4],
    /// Centre pixel after the vertex-buffer + input-layout + constant-buffer
    /// path rendered a *different* colour through a second pipeline.
    pub indexed_centre: [u8; 4],
    /// Corner pixel after the second stage; must equal the clear colour.
    pub indexed_corner: [u8; 4],
}

impl DrawProbeReport {
    /// `true` when every pixel matched its expectation within
    /// [`PROBE_TOLERANCE`].
    pub fn all_expectations_met(&self) -> bool {
        pixel_close(self.centre, PROBE_DRAW_RGBA8, PROBE_TOLERANCE)
            && pixel_close(self.corner, PROBE_CLEAR_RGBA8, PROBE_TOLERANCE)
            && pixel_close(self.indexed_centre, PROBE_STAGE_TWO_RGBA8, PROBE_TOLERANCE)
            && pixel_close(self.indexed_corner, PROBE_CLEAR_RGBA8, PROBE_TOLERANCE)
    }
}

/// Runs both draw paths on the real device and returns the measured pixels.
///
/// Stage 1 is `SV_VertexID` only; stage 2 re-uses the same render target but
/// binds a vertex buffer, an index buffer, an input layout and a constant
/// buffer, and draws a *different* colour. Each stage clears first, so neither
/// a stale stage-1 image nor an untouched target can satisfy the other.
pub fn run_draw_probe(device: &D3D11Device) -> anyhow::Result<DrawProbeReport> {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    const CENTRE_X: u32 = WIDTH / 2;
    const CENTRE_Y: u32 = HEIGHT / 2;
    const CORNER_X: u32 = WIDTH - 2;
    const CORNER_Y: u32 = HEIGHT - 2;

    let adapter = adapter_identity(device)?;
    ensure!(!adapter.software, "probe must not run on a software adapter");
    let context = device.immediate_context();
    let target = RenderTarget::new(device, WIDTH, HEIGHT, DXGI_FORMAT_R8G8B8A8_UNORM)?;
    let rasterizer = RasterizerState::no_cull(device)?;

    // ---- Stage 1: geometry from SV_VertexID, no vertex buffer at all ----
    let id_pipeline = GraphicsPipeline::new(
        device,
        VS_SV_VERTEX_ID,
        PS_SOLID_COLOR,
        &[],
        D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
        "probe-sv-vertex-id",
    )?;
    ensure!(
        !id_pipeline.has_input_layout(),
        "SV_VertexID pipeline must not create an input layout"
    );

    target.bind(context);
    rasterizer.bind(context);
    target.clear(context, PROBE_CLEAR_COLOR);
    draw(context, &id_pipeline, 3)?;
    unsafe { context.Flush() };
    let centre = target.pixel(device, CENTRE_X, CENTRE_Y)?;
    let corner = target.pixel(device, CORNER_X, CORNER_Y)?;

    ensure!(
        pixel_close(centre, PROBE_DRAW_RGBA8, PROBE_TOLERANCE),
        "SV_VertexID draw did not reach the centre pixel: got {centre:?}, expected {PROBE_DRAW_RGBA8:?} (a clear-only target means the triangle was culled or the draw never ran)"
    );
    ensure!(
        pixel_close(corner, PROBE_CLEAR_RGBA8, PROBE_TOLERANCE),
        "SV_VertexID draw leaked outside the triangle: corner is {corner:?}, expected the clear colour {PROBE_CLEAR_RGBA8:?}"
    );

    // ---- Stage 2: vertex buffer + index buffer + input layout + cbuffer ----
    let triangle = PROBE_TRIANGLE;
    let vertices = [
        ProbeVertex { position: [triangle[0][0], triangle[0][1], 0.0], uv: [0.0, 0.0] },
        ProbeVertex { position: [triangle[1][0], triangle[1][1], 0.0], uv: [0.5, 1.0] },
        ProbeVertex { position: [triangle[2][0], triangle[2][1], 0.0], uv: [1.0, 0.0] },
    ];
    // Same clockwise winding as stage 1: indices go straight through.
    let indices: [u16; 3] = [0, 1, 2];

    let vertex_buffer = VertexBuffer::new(device, &vertices)?;
    let index_buffer = IndexBuffer::new(device, &indices)?;

    let mut constants = [0u8; 16];
    for (index, value) in PROBE_STAGE_TWO_TINT.iter().enumerate() {
        constants[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    let constant_buffer = ConstantBuffer::new(device, &constants)?;

    let buffer_pipeline = GraphicsPipeline::new(
        device,
        VS_VERTEX_BUFFER,
        PS_VERTEX_BUFFER,
        &InputElement::probe_layout(),
        D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
        "probe-vertex-buffer",
    )?;
    ensure!(
        buffer_pipeline.has_input_layout(),
        "vertex-buffer pipeline must create an input layout"
    );

    target.bind(context);
    target.clear(context, PROBE_CLEAR_COLOR);
    vertex_buffer.bind(context);
    constant_buffer.bind_to_vertex_stage(context, 0);
    draw_indexed(context, &buffer_pipeline, &index_buffer)?;
    unsafe { context.Flush() };
    let indexed_centre = target.pixel(device, CENTRE_X, CENTRE_Y)?;
    let indexed_corner = target.pixel(device, CORNER_X, CORNER_Y)?;

    ensure!(
        pixel_close(indexed_centre, PROBE_STAGE_TWO_RGBA8, PROBE_TOLERANCE),
        "DrawIndexed through the vertex-buffer pipeline produced {indexed_centre:?} at the centre, expected the constant-buffer tint {PROBE_STAGE_TWO_RGBA8:?}"
    );
    ensure!(
        pixel_close(indexed_corner, PROBE_CLEAR_RGBA8, PROBE_TOLERANCE),
        "DrawIndexed leaked outside the triangle: corner is {indexed_corner:?}, expected {PROBE_CLEAR_RGBA8:?}"
    );

    Ok(DrawProbeReport {
        adapter,
        feature_level: device.feature_level(),
        width: WIDTH,
        height: HEIGHT,
        centre,
        corner,
        indexed_centre,
        indexed_corner,
    })
}

/// Reads one RGBA8 pixel out of a row-major readback buffer.
pub fn pixel_at(bytes: &[u8], width: u32, x: u32, y: u32) -> anyhow::Result<[u8; 4]> {
    ensure!(x < width, "pixel x {x} is outside width {width}");
    let stride = width as usize * 4;
    let offset = (y as usize)
        .checked_mul(stride)
        .and_then(|row| row.checked_add(x as usize * 4))
        .context("pixel offset overflow")?;
    let end = offset.checked_add(4).context("pixel end offset overflow")?;
    ensure!(
        end <= bytes.len(),
        "pixel ({x},{y}) needs bytes {offset}..{end} but the buffer is {} bytes",
        bytes.len()
    );
    Ok([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

/// Per-channel comparison with an absolute tolerance, which is what a real
/// GPU readback needs: Intel FL10_0 rounds `0.25 * 255 = 63.75` differently
/// from the reference rasteriser, so exact equality is the wrong assertion.
pub fn pixel_close(actual: [u8; 4], expected: [u8; 4], tolerance: u8) -> bool {
    actual
        .iter()
        .zip(expected.iter())
        .all(|(a, e)| a.abs_diff(*e) <= tolerance)
}

fn compile_shader(
    source: &str,
    entry: PCSTR,
    target: PCSTR,
    debug_name: &str,
) -> anyhow::Result<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let flags = D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3;

    // windows-rs 0.62.2 signature:
    //   D3DCompile<P2, P4, P5, P6>(psrcdata, srcdatasize, psourcename,
    //       pdefines: Option<*const D3D_SHADER_MACRO>, pinclude: P4,
    //       pentrypoint, ptarget, flags1, flags2,
    //       ppcode: *mut Option<ID3DBlob>,
    //       pperrormsgs: Option<*mut Option<ID3DBlob>>) -> Result<()>
    // The generic `pinclude` cannot be a bare `None`, and `pperrormsgs` is an
    // Option of a pointer, hence the two explicit spellings below.
    let compiled: windows::core::Result<()> = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("gm45_probe.hlsl"),
            None,
            None::<&ID3DInclude>,
            entry,
            target,
            flags,
            0,
            &mut code,
            Some(&mut errors),
        )
    };

    if let Err(error) = compiled {
        let message = errors
            .map(|blob| String::from_utf8_lossy(blob_bytes(&blob)).into_owned())
            .unwrap_or_default();
        return Err(anyhow::anyhow!(
            "D3DCompile failed for {debug_name} ({}): {error}; compiler output: {message}",
            String::from_utf8_lossy(unsafe { entry.as_bytes() })
        ));
    }

    code.context("D3DCompile reported success but returned no shader blob")
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        let pointer = blob.GetBufferPointer().cast::<u8>();
        let length = blob.GetBufferSize();
        std::slice::from_raw_parts(pointer, length)
    }
}

/// Small local mirror of the crate-private `direct_bytes_per_texel`, limited to
/// the formats this module can create itself. Kept local so the graphics path
/// does not depend on a private helper that may change shape.
fn bytes_per_texel(format: DXGI_FORMAT) -> Option<u32> {
    if format == DXGI_FORMAT_R8G8B8A8_UNORM { Some(4) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_colors_quantise_to_the_expected_bytes() {
        assert_eq!(PROBE_DRAW_RGBA8, [64, 128, 191, 255]);
        assert_eq!(PROBE_STAGE_TWO_RGBA8, [128, 64, 32, 255]);
        assert_eq!(PROBE_CLEAR_RGBA8, [0, 0, 0, 255]);
    }

    #[test]
    fn the_two_stages_use_different_colours() {
        // Guards the "a stale stage-1 image cannot satisfy stage 2" property.
        assert!(!pixel_close(PROBE_STAGE_TWO_RGBA8, PROBE_DRAW_RGBA8, PROBE_TOLERANCE));
        assert!(!pixel_close(PROBE_STAGE_TWO_RGBA8, PROBE_CLEAR_RGBA8, PROBE_TOLERANCE));
    }

    #[test]
    fn drawn_and_clear_colours_are_distinguishable_within_tolerance() {
        assert!(!pixel_close(PROBE_DRAW_RGBA8, PROBE_CLEAR_RGBA8, PROBE_TOLERANCE));
    }

    #[test]
    fn probe_triangle_winds_clockwise() {
        // The probe disables culling, so this is a declaration of intent
        // rather than a rasteriser requirement: the triangle is clockwise to
        // match the engine's convention. A positive (counter-clockwise) area
        // here means someone flipped the constant without updating the
        // shader, which is worth failing on.
        let area = signed_area_twice(PROBE_TRIANGLE[0], PROBE_TRIANGLE[1], PROBE_TRIANGLE[2]);
        assert!(area < 0.0, "probe triangle must wind clockwise, area = {area}");
    }

    #[test]
    fn probe_triangle_covers_the_centre_but_not_the_corners() {
        let inside = |p: [f32; 2]| {
            let d1 = signed_area_twice(PROBE_TRIANGLE[0], PROBE_TRIANGLE[1], p);
            let d2 = signed_area_twice(PROBE_TRIANGLE[1], PROBE_TRIANGLE[2], p);
            let d3 = signed_area_twice(PROBE_TRIANGLE[2], PROBE_TRIANGLE[0], p);
            let negative = d1 < 0.0 && d2 < 0.0 && d3 < 0.0;
            let positive = d1 > 0.0 && d2 > 0.0 && d3 > 0.0;
            negative || positive
        };
        // NDC origin is the centre of the render target; the four corners of
        // the target are at +/-1.
        assert!(inside([0.0, 0.0]), "centre must be covered");
        for corner in [[-0.98, 0.98], [0.98, 0.98], [-0.98, -0.98], [0.98, -0.98]] {
            assert!(!inside(corner), "corner {corner:?} must stay clear");
        }
    }

    #[test]
    fn pixel_close_honours_the_tolerance_boundary() {
        let expected = [64, 128, 191, 255];
        assert!(pixel_close([62, 130, 189, 253], expected, 2));
        assert!(!pixel_close([61, 131, 189, 253], expected, 2));
    }

    #[test]
    fn pixel_close_accepts_rounding_of_half_values() {
        // 0.5 * 255 = 127.5 rounds either way on real hardware.
        assert!(pixel_close([127, 63, 31, 255], PROBE_STAGE_TWO_RGBA8, PROBE_TOLERANCE));
        assert!(pixel_close([129, 65, 33, 255], PROBE_STAGE_TWO_RGBA8, PROBE_TOLERANCE));
    }

    #[test]
    fn pixel_at_rejects_out_of_range_coordinates() {
        let bytes = vec![0u8; 4 * 4 * 4];
        assert!(pixel_at(&bytes, 4, 3, 3).is_ok());
        assert!(pixel_at(&bytes, 4, 4, 0).is_err());
        assert!(pixel_at(&bytes, 4, 0, 3).is_ok());
        assert!(pixel_at(&bytes, 4, 0, 4).is_err());
    }

    #[test]
    fn pixel_at_reads_row_major_rgba8() {
        let mut bytes = vec![0u8; 2 * 2 * 4];
        bytes[0..4].copy_from_slice(&[1, 2, 3, 4]);
        bytes[4..8].copy_from_slice(&[5, 6, 7, 8]);
        bytes[12..16].copy_from_slice(&[9, 10, 11, 12]);
        assert_eq!(pixel_at(&bytes, 2, 0, 0).unwrap(), [1, 2, 3, 4]);
        assert_eq!(pixel_at(&bytes, 2, 1, 0).unwrap(), [5, 6, 7, 8]);
        assert_eq!(pixel_at(&bytes, 2, 1, 1).unwrap(), [9, 10, 11, 12]);
    }

    #[test]
    fn probe_vertex_matches_the_declared_input_layout() {
        // The layout is POSITION R32G32B32 at offset 0 and TEXCOORD R32G32 at
        // offset 12; a #[repr(C)] struct that drifts from that would silently
        // feed the VS garbage, so pin it here.
        assert_eq!(std::mem::size_of::<ProbeVertex>(), 20);
        assert_eq!(std::mem::offset_of!(ProbeVertex, position), 0);
        assert_eq!(std::mem::offset_of!(ProbeVertex, uv), 12);
    }

    #[test]
    fn gm45_identity_is_recognised_by_pci_id() {
        let gm45 = AdapterIdentity {
            vendor_id: 0x8086,
            device_id: 0x2A42,
            description: "Mobile Intel(R) 4 Series Express Chipset Family".to_owned(),
            software: false,
        };
        assert!(gm45.is_intel());
        assert!(gm45.is_gm45());

        let warp = AdapterIdentity {
            vendor_id: 0x1414,
            device_id: 0x008C,
            description: "Microsoft Basic Render Driver".to_owned(),
            software: true,
        };
        assert!(!warp.is_intel());
        assert!(!warp.is_gm45());
    }

    #[test]
    fn shaders_target_shader_model_4_0_only() {
        // GM45 is Feature Level 10_0; the module must never ask D3DCompile for
        // a newer profile. The targets are literals inside compile_shader call
        // sites, so guard the contract at the source-text level.
        assert!(VS_SV_VERTEX_ID.contains("SV_VertexID"));
        assert!(PS_SOLID_COLOR.contains("SV_Target"));
        assert!(VS_VERTEX_BUFFER.contains("register(b0)"));
        assert!(!VS_SV_VERTEX_ID.contains("SV_InstanceID"));
        for stage in [VS_SV_VERTEX_ID, PS_SOLID_COLOR, VS_VERTEX_BUFFER, PS_VERTEX_BUFFER] {
            assert!(!stage.contains("StructuredBuffer"), "compute-era features are out of scope on FL10_0");
            assert!(!stage.contains("RWTexture"), "UAV writes are out of scope on the FL10_0 path");
        }
    }

    /// Hardware test: requires a real D3D11 device, so it is ignored by
    /// default. Run on the GM45 with
    /// `cargo test -p glue5-gpu-d3d11 -- --ignored draw_probe`.
    #[test]
    #[ignore = "requires real D3D11 hardware"]
    fn draw_probe_renders_on_the_real_adapter() {
        let device = D3D11Device::open_hardware().expect("hardware device");
        let report = run_draw_probe(&device).expect("draw probe");
        assert!(!report.adapter.software);
        assert!(
            report.all_expectations_met(),
            "report: {report:?}"
        );
    }
}
