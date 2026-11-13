//! Legacy DXGI presentation using the same D3D11 device as the GM45 executor.
use crate::D3D11Device;
use anyhow::{Context, ensure};
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Direct3D11::{ID3D11RenderTargetView, ID3D11Texture2D},
        Dxgi::{
            Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_RATIONAL, DXGI_SAMPLE_DESC},
            DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice,
            IDXGIFactory, IDXGISwapChain,
        },
    },
};
use windows::core::Interface;

/// Owns a DXGI window swapchain. The supplied HWND must stay alive.
pub struct Presentation {
    swapchain: IDXGISwapChain,
    render_target: ID3D11RenderTargetView,
    width: u32,
    height: u32,
}
impl Presentation {
    /// # Safety
    /// hwnd must be a valid live window handle. Call on the UI thread.
    pub unsafe fn new(device: &D3D11Device, hwnd: HWND, width: u32, height: u32) -> anyhow::Result<Self> {
        ensure!(width > 0 && height > 0, "nonzero dimensions required");
        ensure!(!hwnd.0.is_null(), "valid HWND required");
        let dxgi: IDXGIDevice = device.raw_device().cast().context("IDXGIDevice")?;
        let adapter = unsafe { dxgi.GetAdapter() }.context("GetAdapter")?;
        let factory: IDXGIFactory = unsafe { adapter.GetParent() }.context("IDXGIFactory")?;
        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: width,
                Height: height,
                RefreshRate: DXGI_RATIONAL { Numerator: 0, Denominator: 1 },
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ..Default::default()
            },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 1,
            OutputWindow: hwnd,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
            Flags: 0,
        };
        let mut swapchain = None;
        unsafe { factory.CreateSwapChain(device.raw_device(), &desc, &mut swapchain) }
            .ok()
            .context("CreateSwapChain(HARDWARE)")?;
        let swapchain = swapchain.context("missing DXGI swapchain")?;
        let buffer: ID3D11Texture2D = unsafe { swapchain.GetBuffer(0) }.context("GetBuffer(0)")?;
        let mut render_target = None;
        unsafe { device.raw_device().CreateRenderTargetView(&buffer, None, Some(&mut render_target)) }
            .context("CreateRenderTargetView(backbuffer)")?;
        Ok(Self { swapchain, render_target: render_target.context("missing backbuffer RTV")?, width, height })
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn target(&self) -> &ID3D11RenderTargetView {
        &self.render_target
    }
    pub fn clear(&self, device: &D3D11Device, color: [f32; 4]) {
        unsafe {
            let context = device.immediate_context();
            context.OMSetRenderTargets(Some(&[Some(self.render_target.clone())]), None);
            context.ClearRenderTargetView(&self.render_target, &color);
        }
    }
    pub fn present(&self, sync_interval: u32) -> anyhow::Result<()> {
        ensure!(sync_interval <= 4, "invalid sync interval");
        unsafe { self.swapchain.Present(sync_interval, DXGI_PRESENT(0)) }.ok().context("Present")?;
        Ok(())
    }
    pub fn unbind(&self, device: &D3D11Device) {
        unsafe { device.immediate_context().OMSetRenderTargets(None, None) };
    }
}
