//! FL10_0 hardware smoke test for VS/PS, vertex/index buffers and readback.
use anyhow::{Context, Result, ensure};
use glue5_gpu_d3d11::{D3D11Device, graphics::run_draw_probe};
fn main() -> Result<()> {
    let device = D3D11Device::open_hardware().context("open physical D3D11 device")?;
    let report = run_draw_probe(&device).context("GM45 shader/draw/readback probe")?;
    ensure!(report.all_expectations_met(), "unexpected hardware pixels");
    println!(
        "DRAW OK adapter={} FL={:?} size={}x{} draw={:?} corner={:?} indexed={:?} indexed_corner={:?}",
        report.adapter.description,
        report.feature_level,
        report.width,
        report.height,
        report.centre,
        report.corner,
        report.indexed_centre,
        report.indexed_corner,
    );
    Ok(())
}
