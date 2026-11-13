use std::{
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use windows::Win32::{
    Foundation::HMODULE,
    Graphics::{
        Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_11_0, D3D_SRV_DIMENSION_BUFFER, D3D_SRV_DIMENSION_TEXTURE2D,
        },
        Direct3D11::{
            D3D11_ASYNC_GETDATA_DONOTFLUSH, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_INDEX_BUFFER,
            D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER, D3D11_BOX,
            D3D11_BUFFER_DESC, D3D11_BUFFER_SRV, D3D11_BUFFER_SRV_0, D3D11_BUFFER_SRV_1, D3D11_COMPARISON_ALWAYS,
            D3D11_COMPARISON_FUNC, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_FILTER,
            D3D11_FORMAT_SUPPORT_RENDER_TARGET, D3D11_FORMAT_SUPPORT_SHADER_LOAD, D3D11_FORMAT_SUPPORT_TEXTURE2D,
            D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
            D3D11_RESOURCE_MISC_DRAWINDIRECT_ARGS, D3D11_SAMPLER_DESC, D3D11_SDK_VERSION,
            D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0, D3D11_TEX2D_SRV,
            D3D11_TEXTURE_ADDRESS_MODE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
            D3D11CreateDevice, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11Query, ID3D11RenderTargetView,
            ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D,
        },
        Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC},
    },
};

/// The D3D feature level returned by the real hardware device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeatureLevel {
    Level11_0,
    Level10_1,
    Level10_0,
    Other(i32),
}

impl FeatureLevel {
    fn from_raw(value: D3D_FEATURE_LEVEL) -> Self {
        match value {
            D3D_FEATURE_LEVEL_11_0 => Self::Level11_0,
            D3D_FEATURE_LEVEL_10_1 => Self::Level10_1,
            D3D_FEATURE_LEVEL_10_0 => Self::Level10_0,
            other => Self::Other(other.0),
        }
    }
}

/// Validates the engine's D3D11 sampler descriptor before translating it.
///
/// The WGPU renderer tolerates some unknown enum values by substituting
/// defaults. The native backend must not silently turn malformed values into
/// a different D3D11 state, so unsupported bits and enum values are rejected.
fn validate_sampler_words(words: &[u32]) -> anyhow::Result<()> {
    ensure!(words.len() >= 13, "sampler payload is truncated: expected 13 words, got {}", words.len());

    let filter = words[0];
    ensure!(filter & !0xD5 == 0, "unsupported D3D11 sampler filter bits {filter:#x}");
    let filter_bits = filter & 0x7f;
    let anisotropic = filter_bits == 0x55;
    ensure!(anisotropic || filter_bits & !0x15 == 0, "unsupported D3D11 sampler filter value {filter_bits:#x}");

    // Engine zero/default is WGPU Repeat, represented by D3D11 WRAP (1).
    // The other values are the native D3D11 address-mode enum values.
    for (index, name) in [(1, "U"), (2, "V"), (3, "W")] {
        let value = words[index];
        ensure!(value <= 5, "unsupported D3D11 sampler Address{name} value {value}");
    }

    if filter & 0x80 != 0 {
        let comparison = words[6];
        ensure!((1..=8).contains(&comparison), "invalid D3D11 sampler comparison function {comparison}");
    }

    for index in [4usize, 7, 8, 9, 10, 11, 12] {
        let value = f32::from_bits(words[index]);
        ensure!(value.is_finite(), "sampler float word {index} must be finite");
    }

    Ok(())
}

/// Required capabilities for a 2D texture's views.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextureBindings {
    pub shader_resource: bool,
    pub render_target: bool,
}

impl TextureBindings {
    pub const fn shader_resource() -> Self {
        Self { shader_resource: true, render_target: false }
    }

    pub const fn render_target() -> Self {
        Self { shader_resource: false, render_target: true }
    }

    pub const fn render_target_and_shader_resource() -> Self {
        Self { shader_resource: true, render_target: true }
    }

    fn bind_flags(self) -> u32 {
        let mut flags = 0;
        if self.shader_resource {
            flags |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        }
        if self.render_target {
            flags |= D3D11_BIND_RENDER_TARGET.0 as u32;
        }
        flags
    }
}

/// D3D11 format capabilities queried from this specific adapter.
#[derive(Clone, Copy, Debug)]
pub struct FormatSupport {
    pub raw: u32,
}

impl FormatSupport {
    pub fn supports(self, required: u32) -> bool {
        self.raw & required == required
    }

    pub fn texture_2d_shader_load(self) -> bool {
        self.supports(D3D11_FORMAT_SUPPORT_TEXTURE2D.0 as u32 | D3D11_FORMAT_SUPPORT_SHADER_LOAD.0 as u32)
    }

    pub fn texture_2d_render_target(self) -> bool {
        self.supports(D3D11_FORMAT_SUPPORT_TEXTURE2D.0 as u32 | D3D11_FORMAT_SUPPORT_RENDER_TARGET.0 as u32)
    }
}

/// An owned D3D11 texture and whichever views were requested at creation.
pub struct D3D11Texture {
    texture: ID3D11Texture2D,
    srv: Option<ID3D11ShaderResourceView>,
    rtv: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl D3D11Texture {
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn format(&self) -> DXGI_FORMAT {
        self.format
    }

    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    pub fn srv(&self) -> Option<&ID3D11ShaderResourceView> {
        self.srv.as_ref()
    }

    pub fn rtv(&self) -> Option<&ID3D11RenderTargetView> {
        self.rtv.as_ref()
    }
}

/// The maximum size of a D3D11 constant buffer: 4096 16-byte constants.
pub const MAX_CONSTANT_BUFFER_BYTES: u32 = 65_536;

/// An owned D3D11 buffer, including the size and bindings used at creation.
pub struct D3D11Buffer {
    buffer: ID3D11Buffer,
    size: u32,
    byte_width: u32,
    bind_flags: u32,
    stride: u32,
    /// CPU shadow retained only for constant buffers so shader-specific
    /// packed/offset copies can be materialized on FL10_0.
    constant_shadow: Option<Mutex<Vec<u8>>>,
}

impl D3D11Buffer {
    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn bind_flags(&self) -> u32 {
        self.bind_flags
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }

    pub fn buffer(&self) -> &ID3D11Buffer {
        &self.buffer
    }

    /// Copies size bytes beginning at first 16-byte constants.
    /// Missing source bytes are zero-filled, matching the WGPU executor.
    pub fn copy_constant_range(&self, first: u32, size: u32) -> anyhow::Result<Vec<u8>> {
        let shadow = self
            .constant_shadow
            .as_ref()
            .context("buffer was not created with BIND_CONSTANT")?
            .lock()
            .map_err(|_| anyhow::anyhow!("constant-buffer shadow lock was poisoned"))?;
        copy_constant_range(Some(&shadow), first, size)
    }
}

/// Validates the FL10_0/FL11_0 constant-buffer size constraints.
fn validate_constant_buffer_size(size: u32) -> anyhow::Result<()> {
    ensure!(size > 0, "constant-buffer size must be non-zero");
    ensure!(size <= MAX_CONSTANT_BUFFER_BYTES, "D3D11 constant-buffer size {size} exceeds the 64 KiB limit");
    ensure!(size.is_multiple_of(16), "D3D11 constant-buffer size must be a multiple of 16 bytes");
    Ok(())
}

/// Materializes the exact byte interval requested by a shader pack member.
fn copy_constant_range(source: Option<&[u8]>, first: u32, size: u32) -> anyhow::Result<Vec<u8>> {
    validate_constant_buffer_size(size)?;
    let start = first.checked_mul(16).context("constant-buffer first-constant offset overflow")? as usize;
    let mut output = vec![0; size as usize];

    if let Some(source) = source {
        let copied = source.len().saturating_sub(start).min(output.len());
        if copied != 0 {
            output[..copied].copy_from_slice(&source[start..start + copied]);
        }
    }
    Ok(output)
}

fn direct_bytes_per_texel(format: DXGI_FORMAT) -> Option<u32> {
    Some(match format.0 {
        2 => 16,           // R32G32B32A32_FLOAT
        10 | 11 | 16 => 8, // R16G16B16A16_FLOAT/UNORM, R32G32_FLOAT
        24 | 26 | 28 | 29 | 31 | 34 | 35 | 41 | 42 | 43 | 67 | 87 | 88 | 91 => 4,
        49 | 51 | 54 | 56 => 2,
        61 | 63 | 65 => 1,
        _ => return None,
    })
}

/// Owns the native D3D11 device and its immediate context.
///
/// This is deliberately not a wrapper around wgpu. All COM objects in this
/// type belong to the native backend and stay inside that ownership boundary.
pub struct D3D11Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    feature_level: FeatureLevel,
}

impl D3D11Device {
    /// Opens the hardware adapter only. It never silently falls back to WARP.
    pub fn open_hardware() -> anyhow::Result<Self> {
        let requested = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut obtained = D3D_FEATURE_LEVEL(0);

        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&requested),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut obtained),
                Some(&mut context),
            )
            .context("D3D11CreateDevice(HARDWARE)")?;
        }

        let device = device.context("D3D11CreateDevice returned no ID3D11Device")?;
        let context = context.context("D3D11CreateDevice returned no immediate context")?;

        Ok(Self { device, context, feature_level: FeatureLevel::from_raw(obtained) })
    }

    pub fn feature_level(&self) -> FeatureLevel {
        self.feature_level
    }

    pub fn raw_device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn immediate_context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    pub fn format_support(&self, format: DXGI_FORMAT) -> anyhow::Result<FormatSupport> {
        let raw = unsafe { self.device.CheckFormatSupport(format).context("ID3D11Device::CheckFormatSupport")? };
        Ok(FormatSupport { raw })
    }

    /// Uploads one uncompressed 2D texture region through UpdateSubresource.
    ///
    /// Only one mip, one array slice and one depth plane are supported. The
    /// source pitch may contain padding; only the texel bytes in each row are
    /// consumed. Compressed and typeless formats are rejected by design.
    #[allow(clippy::too_many_arguments)]
    pub fn update_texture_2d(
        &self,
        texture: &D3D11Texture,
        mip: u32,
        slice: u32,
        z: u32,
        width: u32,
        height: u32,
        depth: u32,
        x: u32,
        y: u32,
        bytes_per_row: u32,
        rows: u32,
        data: &[u8],
    ) -> anyhow::Result<()> {
        ensure!(mip == 0, "only texture mip 0 is supported");
        ensure!(slice == 0 && z == 0, "only texture array slice 0 / z 0 is supported");
        ensure!(depth == 0 || depth == 1, "2D texture upload depth must be 1");
        ensure!(width > 0 && height > 0, "texture upload extent must be non-zero");

        let right = x.checked_add(width).context("texture upload x range overflow")?;
        let bottom = y.checked_add(height).context("texture upload y range overflow")?;
        ensure!(right <= texture.width, "texture upload x range exceeds texture width");
        ensure!(bottom <= texture.height, "texture upload y range exceeds texture height");

        let bytes_per_texel = direct_bytes_per_texel(texture.format)
            .with_context(|| format!("direct upload is not implemented for DXGI format {:?}", texture.format))?;
        let row_bytes = width.checked_mul(bytes_per_texel).context("texture upload row size overflow")?;
        ensure!(
            bytes_per_row >= row_bytes,
            "texture upload pitch {bytes_per_row} is smaller than row size {row_bytes}"
        );
        ensure!(rows >= height, "texture upload row count {rows} is smaller than extent height {height}");

        let required = (height as usize - 1)
            .checked_mul(bytes_per_row as usize)
            .and_then(|prefix| prefix.checked_add(row_bytes as usize))
            .context("texture upload byte range overflow")?;
        ensure!(
            data.len() >= required,
            "texture upload needs {required} source bytes but only {} are present",
            data.len()
        );

        let depth_pitch = bytes_per_row.checked_mul(height).context("texture upload depth pitch overflow")?;
        let region = D3D11_BOX { left: x, right, top: y, bottom, front: 0, back: 1 };
        unsafe {
            self.context.UpdateSubresource(
                &texture.texture,
                0,
                Some(&region),
                data.as_ptr().cast(),
                bytes_per_row,
                depth_pitch,
            );
        }
        Ok(())
    }

    /// Copies a direct-format texture through a CPU-readable staging resource.
    pub fn readback_texture(&self, texture: &D3D11Texture) -> anyhow::Result<Vec<u8>> {
        let bytes_per_texel = direct_bytes_per_texel(texture.format)
            .with_context(|| format!("texture readback is not implemented for DXGI format {:?}", texture.format))?;
        let row_bytes = texture.width.checked_mul(bytes_per_texel).context("texture readback row size overflow")?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: texture.width,
            Height: texture.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: texture.format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .context("creating D3D11 staging texture for readback")?;
        }
        let staging = staging.context("staging CreateTexture2D returned no texture")?;
        unsafe {
            self.context.CopyResource(&staging, &texture.texture);
            self.context.Flush();
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("mapping D3D11 staging texture")?;
        }
        if mapped.pData.is_null() {
            unsafe { self.context.Unmap(&staging, 0) };
            anyhow::bail!("D3D11 staging texture Map returned a null data pointer");
        }
        if mapped.RowPitch < row_bytes {
            unsafe { self.context.Unmap(&staging, 0) };
            anyhow::bail!("D3D11 staging row pitch {} is smaller than {row_bytes}", mapped.RowPitch);
        }

        let byte_count = (row_bytes as usize)
            .checked_mul(texture.height as usize)
            .context("texture readback byte count overflow")?;
        let mut bytes = vec![0u8; byte_count];
        unsafe {
            let source = mapped.pData.cast::<u8>();
            for row in 0..texture.height as usize {
                let src = std::slice::from_raw_parts(source.add(row * mapped.RowPitch as usize), row_bytes as usize);
                let dst = &mut bytes[row * row_bytes as usize..(row + 1) * row_bytes as usize];
                dst.copy_from_slice(src);
            }
            self.context.Unmap(&staging, 0);
        }
        Ok(bytes)
    }

    /// Creates an SRV for a single-mip 2D texture view.
    #[allow(clippy::too_many_arguments)]
    pub fn create_texture_srv_2d(
        &self,
        texture: &D3D11Texture,
        format: u32,
        view_dim: u32,
        most_detailed_mip: u32,
        mip_levels: u32,
        first_slice: u32,
        slice_count: u32,
    ) -> anyhow::Result<ID3D11ShaderResourceView> {
        ensure!(view_dim == 4, "only D3D11 TEXTURE2D SRVs are supported");
        ensure!(most_detailed_mip == 0, "only texture mip 0 is supported");
        ensure!(mip_levels == 0 || mip_levels == 1 || mip_levels == u32::MAX, "only a single texture mip is supported");
        ensure!(
            first_slice == 0 && (slice_count == 0 || slice_count == 1 || slice_count == u32::MAX),
            "only texture array slice 0 is supported"
        );
        ensure!(
            format == 0 || format == texture.format.0 as u32,
            "SRV format reinterpretation is not supported for this texture"
        );

        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: texture.format,
            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 },
            },
        };
        let mut view = None;
        unsafe {
            self.device
                .CreateShaderResourceView(&texture.texture, Some(&desc), Some(&mut view))
                .context("ID3D11Device::CreateShaderResourceView(texture2D)")?;
        }
        view.context("CreateShaderResourceView returned no texture view")
    }

    /// Creates a D3D11 sampler from the engine's 13-word sampler payload.
    pub fn create_sampler_state(&self, words: &[u32]) -> anyhow::Result<ID3D11SamplerState> {
        validate_sampler_words(words)?;
        let word = |index: usize| words[index];
        let filter = word(0);
        let comparison = filter & 0x80 != 0;
        let anisotropic = filter & 0x7f == 0x55;
        let filter_value = if anisotropic { 0x55 } else { filter & 0x15 } | if comparison { 0x80 } else { 0 };
        let address = |value: u32| {
            D3D11_TEXTURE_ADDRESS_MODE(match value {
                0 | 1 => 1, // Engine default / D3D11_TEXTURE_ADDRESS_WRAP.
                2..=5 => value as i32,
                _ => unreachable!("sampler address modes were validated"),
            })
        };
        let comparison_func = if comparison { D3D11_COMPARISON_FUNC(word(6) as i32) } else { D3D11_COMPARISON_ALWAYS };
        let min_lod = f32::from_bits(word(11)).max(0.0);
        let max_lod = f32::from_bits(word(12)).clamp(0.0, 32.0).max(min_lod);

        let desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER(filter_value as i32),
            AddressU: address(word(1)),
            AddressV: address(word(2)),
            AddressW: address(word(3)),
            MipLODBias: f32::from_bits(word(4)),
            MaxAnisotropy: if anisotropic { word(5).clamp(1, 16) } else { 1 },
            ComparisonFunc: comparison_func,
            BorderColor: [7, 8, 9, 10].map(|index| f32::from_bits(word(index))),
            MinLOD: min_lod,
            MaxLOD: max_lod,
        };
        let mut sampler = None;
        unsafe {
            self.device.CreateSamplerState(&desc, Some(&mut sampler)).context("ID3D11Device::CreateSamplerState")?;
        }
        sampler.context("CreateSamplerState returned no sampler")
    }

    /// Binds a contiguous sampler range. Compute-stage binding requires FL11_0.
    pub fn set_samplers(
        &self,
        stage: u32,
        start_slot: u32,
        samplers: &[Option<ID3D11SamplerState>],
    ) -> anyhow::Result<()> {
        ensure!(stage <= 2, "unsupported shader stage {stage}");
        ensure!(
            stage != 2 || self.feature_level == FeatureLevel::Level11_0,
            "compute-stage sampler binding requires D3D11 Feature Level 11_0"
        );
        let end = start_slot.checked_add(samplers.len() as u32).context("sampler slot range overflow")?;
        ensure!(end <= 16, "sampler slot range exceeds D3D11's 16 slots");
        unsafe {
            match stage {
                0 => self.context.VSSetSamplers(start_slot, Some(samplers)),
                1 => self.context.PSSetSamplers(start_slot, Some(samplers)),
                2 => self.context.CSSetSamplers(start_slot, Some(samplers)),
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    /// Creates a typed SRV for a buffer range. Structured/raw views are not available on GM45 FL10_0.
    pub fn create_buffer_srv(
        &self,
        buffer: &D3D11Buffer,
        format: u32,
        view_dim: u32,
        first_element: u32,
        num_elements: u32,
    ) -> anyhow::Result<ID3D11ShaderResourceView> {
        ensure!(view_dim == 1, "only D3D11 BUFFER SRVs are supported");
        ensure!(buffer.bind_flags & 0x8 != 0, "buffer was not created with BIND_SRV");
        ensure!(buffer.stride == 0, "structured-buffer SRVs are not supported by this GM45 Feature Level 10_0 driver");
        let format = DXGI_FORMAT(format as i32);
        let bytes_per_element = direct_bytes_per_texel(format)
            .context("typed buffer SRV format is not in the supported direct-format table")?;
        ensure!(num_elements > 0, "buffer SRV element count must be non-zero");
        let total_elements = buffer.size / bytes_per_element;
        let end = first_element.checked_add(num_elements).context("buffer SRV element range overflow")?;
        ensure!(end <= total_elements, "buffer SRV element range exceeds {total_elements} elements");

        let view_buffer = D3D11_BUFFER_SRV {
            Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: first_element },
            Anonymous2: D3D11_BUFFER_SRV_1 { NumElements: num_elements },
        };
        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D_SRV_DIMENSION_BUFFER,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: view_buffer },
        };
        let mut view = None;
        unsafe {
            self.device
                .CreateShaderResourceView(&buffer.buffer, Some(&desc), Some(&mut view))
                .context("ID3D11Device::CreateShaderResourceView(typed buffer)")?;
        }
        view.context("CreateShaderResourceView returned no buffer view")
    }

    /// Binds a contiguous SRV range to the requested D3D11 shader stage.
    pub fn set_shader_resources(
        &self,
        stage: u32,
        start_slot: u32,
        views: &[Option<ID3D11ShaderResourceView>],
    ) -> anyhow::Result<()> {
        ensure!(stage <= 2, "unsupported shader stage {stage}");
        ensure!(
            stage != 2 || self.feature_level == FeatureLevel::Level11_0,
            "compute-stage SRV binding requires D3D11 Feature Level 11_0"
        );
        let end = start_slot.checked_add(views.len() as u32).context("SRV slot range overflow")?;
        ensure!(end <= 128, "SRV slot range exceeds D3D11's 128 slots");
        unsafe {
            match stage {
                0 => self.context.VSSetShaderResources(start_slot, Some(views)),
                1 => self.context.PSSetShaderResources(start_slot, Some(views)),
                2 => self.context.CSSetShaderResources(start_slot, Some(views)),
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    /// Creates a DEFAULT buffer for the subset of engine bindings supported so far.
    /// Structured SRV buffers require FL11_0 and are rejected on this GM45 FL10_0 device.
    pub fn create_buffer(&self, size: u32, bind: u32, misc: u32, stride: u32) -> anyhow::Result<D3D11Buffer> {
        const BIND_VERTEX: u32 = 0x1;
        const BIND_INDEX: u32 = 0x2;
        const BIND_CONSTANT: u32 = 0x4;
        const BIND_SHADER_RESOURCE: u32 = 0x8;
        const BIND_UNORDERED_ACCESS: u32 = 0x80;
        const MISC_DRAW_INDIRECT: u32 = 0x10;

        ensure!(size > 0, "buffer size must be non-zero");
        ensure!(
            bind & !(BIND_VERTEX | BIND_INDEX | BIND_CONSTANT | BIND_SHADER_RESOURCE | BIND_UNORDERED_ACCESS) == 0,
            "unsupported buffer bind flags {bind:#x}"
        );
        ensure!(bind & BIND_UNORDERED_ACCESS == 0, "buffer UAV binding is not supported yet");
        ensure!(
            bind & BIND_CONSTANT == 0 || bind == BIND_CONSTANT,
            "D3D11 constant buffers cannot be combined with other bind flags"
        );
        ensure!(misc & !MISC_DRAW_INDIRECT == 0, "unsupported buffer misc flags {misc:#x}");
        if bind & BIND_CONSTANT != 0 {
            validate_constant_buffer_size(size)?;
        }
        if bind & BIND_SHADER_RESOURCE != 0 && stride != 0 {
            ensure!(
                self.feature_level == FeatureLevel::Level11_0,
                "structured buffers are disabled below D3D11 Feature Level 11_0 on this backend"
            );
            ensure!(size.is_multiple_of(stride), "structured buffer size must be a multiple of its stride");
        }

        let byte_width = size.max(4).checked_add(3).context("buffer size overflows D3D11 alignment")? & !3;
        let mut bind_flags = 0;
        if bind & BIND_VERTEX != 0 {
            bind_flags |= D3D11_BIND_VERTEX_BUFFER.0 as u32;
        }
        if bind & BIND_INDEX != 0 {
            bind_flags |= D3D11_BIND_INDEX_BUFFER.0 as u32;
        }
        if bind & BIND_CONSTANT != 0 {
            bind_flags |= D3D11_BIND_CONSTANT_BUFFER.0 as u32;
        }
        if bind & BIND_SHADER_RESOURCE != 0 {
            bind_flags |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        }

        let mut misc_flags =
            if misc & MISC_DRAW_INDIRECT != 0 { D3D11_RESOURCE_MISC_DRAWINDIRECT_ARGS.0 as u32 } else { 0 };
        let structure_stride = if bind & BIND_SHADER_RESOURCE != 0 && stride != 0 {
            misc_flags |= 0x40; // D3D11_RESOURCE_MISC_BUFFER_STRUCTURED
            stride
        } else {
            0
        };

        let desc = D3D11_BUFFER_DESC {
            ByteWidth: byte_width,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flags,
            CPUAccessFlags: 0,
            MiscFlags: misc_flags,
            StructureByteStride: structure_stride,
        };
        let mut buffer = None;
        unsafe {
            self.device.CreateBuffer(&desc, None, Some(&mut buffer)).context("ID3D11Device::CreateBuffer")?;
        }
        let buffer = buffer.context("CreateBuffer returned no buffer")?;
        let constant_shadow = (bind & BIND_CONSTANT != 0).then(|| Mutex::new(vec![0; size as usize]));
        Ok(D3D11Buffer { buffer, size, byte_width, bind_flags: bind, stride, constant_shadow })
    }

    /// Applies an ordered byte-range update to a DEFAULT buffer.
    ///
    /// The no-overwrite engine opcode is deliberately treated as the same
    /// ordered update in this first backend milestone; a dynamic-buffer
    /// Map strategy can replace this without changing the ring payload.
    pub fn update_buffer(&self, buffer: &D3D11Buffer, offset: u32, data: &[u8]) -> anyhow::Result<()> {
        ensure!(!data.is_empty(), "buffer upload must not be empty");
        let length: u32 = data.len().try_into().context("buffer upload exceeds D3D11's 32-bit range")?;
        let end = offset.checked_add(length).context("buffer upload range overflow")?;
        ensure!(end <= buffer.size, "buffer upload range {offset}..{end} exceeds {} bytes", buffer.size);

        let constant_buffer = buffer.bind_flags & 0x4 != 0;
        let (region, upload_data);
        if constant_buffer {
            ensure!(offset == 0 && length == buffer.size, "constant-buffer uploads must replace the entire buffer");
            region = None;
            upload_data = data.to_vec();
        } else {
            ensure!(offset.is_multiple_of(4), "D3D11 buffer upload offset must be 4-byte aligned");
            ensure!(
                length.is_multiple_of(4) || end == buffer.size,
                "a non-four-byte upload length must reach the logical end of the buffer"
            );
            let padded_length = length.checked_add(3).context("buffer upload alignment overflow")? & !3;
            let padded_end = offset.checked_add(padded_length).context("padded buffer upload range overflow")?;
            ensure!(
                padded_end <= buffer.byte_width,
                "padded buffer upload range exceeds allocated byte width {}",
                buffer.byte_width
            );
            let mut padded = vec![0u8; padded_length as usize];
            padded[..data.len()].copy_from_slice(data);
            region = Some(D3D11_BOX { left: offset, right: padded_end, top: 0, bottom: 1, front: 0, back: 1 });
            upload_data = padded;
        }

        unsafe {
            self.context.UpdateSubresource(
                &buffer.buffer,
                0,
                region.as_ref().map(|value| value as *const D3D11_BOX),
                upload_data.as_ptr().cast(),
                0,
                0,
            );
        }
        if let Some(shadow) = &buffer.constant_shadow {
            let mut shadow = shadow.lock().map_err(|_| anyhow::anyhow!("constant-buffer shadow lock was poisoned"))?;
            shadow.copy_from_slice(data);
        }
        Ok(())
    }

    /// Copies a buffer through a CPU-readable staging resource for diagnostics.
    pub fn readback_buffer(&self, buffer: &D3D11Buffer) -> anyhow::Result<Vec<u8>> {
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: buffer.byte_width,
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut staging = None;
        unsafe {
            self.device
                .CreateBuffer(&desc, None, Some(&mut staging))
                .context("creating D3D11 staging buffer for readback")?;
        }
        let staging = staging.context("staging CreateBuffer returned no buffer")?;
        unsafe {
            self.context.CopyResource(&staging, &buffer.buffer);
            self.context.Flush();
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("mapping D3D11 staging buffer")?;
        }
        if mapped.pData.is_null() {
            unsafe { self.context.Unmap(&staging, 0) };
            anyhow::bail!("D3D11 staging Map returned a null data pointer");
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(mapped.pData.cast::<u8>(), buffer.byte_width as usize)[..buffer.size as usize]
                .to_vec()
        };
        unsafe { self.context.Unmap(&staging, 0) };
        Ok(bytes)
    }

    /// Clears a render-target texture through the immediate context.
    pub fn clear_render_target(&self, texture: &D3D11Texture, color: [f32; 4]) -> anyhow::Result<()> {
        let rtv = texture.rtv().context("texture has no render-target view")?;
        unsafe {
            self.context.ClearRenderTargetView(rtv, &color);
        }
        Ok(())
    }

    /// Submits queued immediate-context commands to the driver.
    ///
    /// Flush is submission, not a GPU completion wait.
    pub fn flush(&self) {
        unsafe {
            self.context.Flush();
        }
    }

    /// Creates a reusable D3D11 event query.
    pub fn create_event_query(&self) -> anyhow::Result<ID3D11Query> {
        let desc = D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 };
        let mut query = None;
        unsafe {
            self.device.CreateQuery(&desc, Some(&mut query)).context("ID3D11Device::CreateQuery(EVENT)")?;
        }
        query.context("CreateQuery(EVENT) returned no query")
    }

    /// Waits until all commands submitted before this event query have completed.
    ///
    /// The immediate context is flushed once, then polled without asking
    /// GetData to flush implicitly on every iteration.
    pub fn wait_event_query(&self, query: &ID3D11Query) -> anyhow::Result<()> {
        unsafe {
            self.context.End(query);
            self.context.Flush();
        }

        let started = Instant::now();
        loop {
            let mut complete: i32 = 0;
            unsafe {
                self.context
                    .GetData(
                        query,
                        Some((&mut complete as *mut i32).cast()),
                        std::mem::size_of::<i32>() as u32,
                        D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                    )
                    .context("ID3D11DeviceContext::GetData(EVENT)")?;
            }

            if complete != 0 {
                return Ok(());
            }

            ensure!(started.elapsed() < Duration::from_secs(30), "D3D11 event query timed out after 30 seconds");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Creates a single-mip, single-sample 2D texture with explicit views.
    ///
    /// Format capabilities are checked before allocating the resource. The
    /// returned object owns the texture and its views through COM references.
    pub fn create_texture_2d(
        &self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        bindings: TextureBindings,
    ) -> anyhow::Result<D3D11Texture> {
        ensure!(width > 0 && height > 0, "texture dimensions must be non-zero");
        ensure!(bindings.shader_resource || bindings.render_target, "at least one texture binding must be requested");

        let support = self.format_support(format)?;
        if bindings.shader_resource {
            ensure!(support.texture_2d_shader_load(), "format {:?} does not support Texture2D shader loads", format);
        }
        if bindings.render_target {
            ensure!(
                support.texture_2d_render_target(),
                "format {:?} does not support Texture2D render targets",
                format
            );
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bindings.bind_flags(),
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device.CreateTexture2D(&desc, None, Some(&mut texture)).context("ID3D11Device::CreateTexture2D")?;
        }
        let texture = texture.context("CreateTexture2D returned no texture")?;

        let mut srv = None;
        if bindings.shader_resource {
            unsafe {
                self.device
                    .CreateShaderResourceView(&texture, None, Some(&mut srv))
                    .context("ID3D11Device::CreateShaderResourceView")?;
            }
        }

        let mut rtv = None;
        if bindings.render_target {
            unsafe {
                self.device
                    .CreateRenderTargetView(&texture, None, Some(&mut rtv))
                    .context("ID3D11Device::CreateRenderTargetView")?;
            }
        }

        Ok(D3D11Texture { texture, srv, rtv, width, height, format })
    }
}

#[cfg(test)]
mod sampler_validation_tests {
    use super::validate_sampler_words;

    fn descriptor(filter: u32) -> [u32; 13] {
        let mut words = [0u32; 13];
        words[0] = filter;
        words[1] = 1;
        words[2] = 2;
        words[3] = 3;
        words[6] = 8;
        words[11] = 0.0f32.to_bits();
        words[12] = 32.0f32.to_bits();
        words
    }

    #[test]
    fn accepts_all_supported_filter_combinations_and_address_modes() {
        let filters = [0, 1, 4, 5, 0x10, 0x11, 0x14, 0x15, 0x55];
        for filter in filters {
            let mut words = descriptor(filter);
            assert!(validate_sampler_words(&words).is_ok(), "filter {filter:#x}");
            words[0] |= 0x80;
            assert!(validate_sampler_words(&words).is_ok(), "comparison filter {:#x}", words[0]);
        }

        for address in 0..=5 {
            let mut words = descriptor(0x15);
            words[1] = address;
            words[2] = address;
            words[3] = address;
            assert!(validate_sampler_words(&words).is_ok(), "address mode {address}");
        }
    }

    #[test]
    fn rejects_unknown_filter_bits_and_invalid_filter_shapes() {
        for filter in [0x100, 0x195, 0x25, 0x40, 0xD5 | 0x200] {
            let words = descriptor(filter);
            assert!(validate_sampler_words(&words).is_err(), "filter {filter:#x}");
        }
    }

    #[test]
    fn rejects_invalid_address_modes_in_each_axis() {
        for index in [1usize, 2, 3] {
            for address in [6, u32::MAX] {
                let mut words = descriptor(0x15);
                words[index] = address;
                assert!(validate_sampler_words(&words).is_err(), "word {index} accepted address {address}");
            }
        }
    }

    #[test]
    fn comparison_function_is_checked_only_for_comparison_filters() {
        for comparison in 1..=8 {
            let mut words = descriptor(0xD5);
            words[6] = comparison;
            assert!(validate_sampler_words(&words).is_ok(), "comparison {comparison}");
        }

        for comparison in [0, 9, u32::MAX] {
            let mut words = descriptor(0xD5);
            words[6] = comparison;
            assert!(validate_sampler_words(&words).is_err(), "comparison {comparison} should be rejected");
        }

        let mut non_comparison = descriptor(0x15);
        non_comparison[6] = u32::MAX;
        assert!(validate_sampler_words(&non_comparison).is_ok());
    }

    #[test]
    fn rejects_truncated_payloads_and_non_finite_float_fields() {
        assert!(validate_sampler_words(&[0; 12]).is_err());

        for index in [4usize, 7, 8, 9, 10, 11, 12] {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut words = descriptor(0x15);
                words[index] = value.to_bits();
                assert!(validate_sampler_words(&words).is_err(), "float word {index} accepted {value}");
            }
        }
    }
}

#[cfg(test)]
mod constant_buffer_tests {
    use super::{MAX_CONSTANT_BUFFER_BYTES, copy_constant_range, validate_constant_buffer_size};

    #[test]
    fn constant_buffer_size_obeys_d3d11_limit_and_alignment() {
        assert!(validate_constant_buffer_size(16).is_ok());
        assert!(validate_constant_buffer_size(MAX_CONSTANT_BUFFER_BYTES).is_ok());
        assert!(validate_constant_buffer_size(0).is_err());
        assert!(validate_constant_buffer_size(15).is_err());
        assert!(validate_constant_buffer_size(MAX_CONSTANT_BUFFER_BYTES + 16).is_err());
    }

    #[test]
    fn copies_from_first_constant_and_zero_fills_the_missing_tail() {
        let source: Vec<u8> = (0..64).collect();
        let copied = copy_constant_range(Some(&source), 1, 32).unwrap();
        assert_eq!(copied, source[16..48]);

        let short_source: Vec<u8> = (0..24).collect();
        let copied = copy_constant_range(Some(&short_source), 1, 32).unwrap();
        assert_eq!(&copied[..8], &short_source[16..24]);
        assert!(copied[8..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn missing_constant_buffer_data_is_zero_filled() {
        let copied = copy_constant_range(None, 12, 64).unwrap();
        assert_eq!(copied, vec![0; 64]);
    }

    #[test]
    fn rejects_offset_overflow_and_invalid_pack_member_sizes() {
        assert!(copy_constant_range(Some(&[1; 64]), u32::MAX, 16).is_err());
        assert!(copy_constant_range(Some(&[1; 64]), 0, 0).is_err());
        assert!(copy_constant_range(Some(&[1; 64]), 0, 15).is_err());
        assert!(copy_constant_range(Some(&[1; 64]), 0, MAX_CONSTANT_BUFFER_BYTES + 16).is_err());
    }
}
