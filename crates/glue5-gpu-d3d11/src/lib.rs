//! Native D3D11 primitives for the GLUE5 GM45 investigation.
//!
//! This crate is intentionally separate from the existing wgpu executor.
//! COM objects remain owned by this backend and are never mixed with wgpu
//! handles. The first milestone covers hardware-device creation, format
//! capability checks, 2D texture/view ownership, a typed executor, and an
//! opt-in GpuRing adapter for a deliberately small command allowlist.

#[cfg(target_os = "windows")]
mod d3d11;
#[cfg(target_os = "windows")]
mod executor;
#[cfg(all(target_os = "windows", any(test, feature = "native-ring")))]
mod ring_decode;
#[cfg(all(target_os = "windows", feature = "native-ring"))]
mod ring;
#[cfg(target_os = "windows")]
pub mod graphics;
#[cfg(target_os = "windows")]
pub mod presentation;

#[cfg(target_os = "windows")]
pub use d3d11::{D3D11Buffer, D3D11Device, D3D11Texture, FeatureLevel, FormatSupport, TextureBindings};
#[cfg(target_os = "windows")]
pub use executor::{D3D11Command, D3D11Executor, ExecutorStats};
#[cfg(all(target_os = "windows", feature = "native-ring"))]
pub use ring::backend;
#[cfg(target_os = "windows")]
pub use graphics::{
    AdapterIdentity, ConstantBuffer, DrawProbeReport, GraphicsPipeline, IndexBuffer,
    InputElement, RenderTarget, VertexBuffer, adapter_identity, draw, draw_indexed, pixel_at,
    pixel_close, run_draw_probe,
};

#[cfg(not(target_os = "windows"))]
compile_error!("glue5-gpu-d3d11 is a Windows-only crate");

#[cfg(target_os = "windows")]
pub mod shader_resolver;

#[cfg(target_os = "windows")]
pub mod pipeline;

#[cfg(all(target_os = "windows", feature = "native-ring"))]
pub use ring::backend_with_artifacts;

#[cfg(all(target_os = "windows", feature = "native-ring"))]
pub use ring::{open_with_artifacts, drain_available, wire_fingerprint, BatchProof};
#[cfg(all(target_os = "windows", feature = "wgsl-catalog"))]
pub mod wgsl_catalog;

// Opt-in concrete adapter: no alternate runtime memory or ring protocol.
#[cfg(all(target_os = "windows", feature = "runtime-integration"))]
pub fn runtime_backend_with_artifacts(artifacts: Vec<(u64, shader_resolver::Artifact)>) -> glue5_runtime::gpu::GpuBackend {
    ring::backend_with_artifacts(artifacts)
}
