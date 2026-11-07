//! End-to-end draw probe for the GLUE5 D3D11 backend on Intel GM45.
//!
//! Unlike `gm45_probe.rs`, which exercises resource creation, format
//! capabilities and the fence, this example is the first harness in the crate
//! that actually *renders*: it compiles `vs_4_0`/`ps_4_0` shaders on the real
//! device, binds a pipeline, issues `Draw` and `DrawIndexed`, copies the render
//! target back to the CPU and checks the pixels.
//!
//! Run it on the target machine with:
//!
//! ```text
//! cargo run -p glue5-gpu-d3d11 --example gm45_draw_probe --release
//! ```
//!
//! Exit code 0 means every expectation was met; any failure propagates
//! through `main`, prints the reason and exits non-zero so a script can gate
//! on it.

use anyhow::{Result, ensure};
use glue5_gpu_d3d11::graphics::{
    PROBE_CLEAR_RGBA8, PROBE_DRAW_RGBA8, PROBE_TOLERANCE, run_draw_probe,
};
use glue5_gpu_d3d11::{D3D11Device, FeatureLevel};

fn main() -> Result<()> {
    println!("== GLUE5 D3D11 draw probe (GM45 / FL10_0) ==");

    let device = D3D11Device::open_hardware()?;
    println!("[ok] hardware device opened (no WARP fallback)");
    println!("     feature level: {:?}", device.feature_level());

    let report = run_draw_probe(&device)?;

    println!(
        "[ok] adapter: vendor=0x{:04x} device=0x{:04x} software={} \"{}\"",
        report.adapter.vendor_id, report.adapter.device_id, report.adapter.software, report.adapter.description
    );
    if report.adapter.is_gm45() {
        println!("     identity: Mobile Intel 4 Series (GM45, PCI 8086:2A42)");
    } else if report.adapter.is_intel() {
        println!("     identity: Intel adapter, but not the GM45 PCI id");
    } else {
        println!("     identity: non-Intel adapter; the probe still validates the pipeline");
    }

    println!(
        "[ok] render target: {}x{} R8G8B8A8_UNORM",
        report.width, report.height
    );
    println!(
        "[ok] stage 1 (SV_VertexID, Draw):      centre={:?} corner={:?}",
        report.centre, report.corner
    );
    println!(
        "[ok] stage 2 (VB+IB+IL+cbuffer, DrawIndexed): centre={:?}",
        report.indexed_centre
    );
    println!(
        "     expected: centre={PROBE_DRAW_RGBA8:?} corner={PROBE_CLEAR_RGBA8:?} tolerance=±{PROBE_TOLERANCE}"
    );

    ensure!(
        report.adapter.is_intel(),
        "the draw path passed, but the adapter is not Intel (vendor=0x{:04x}); this run cannot be used as GM45 evidence",
        report.adapter.vendor_id
    );
    ensure!(
        report.feature_level == FeatureLevel::Level10_0
            || report.feature_level == FeatureLevel::Level10_1
            || report.feature_level == FeatureLevel::Level11_0,
        "unexpected feature level {:?}",
        report.feature_level
    );
    ensure!(
        report.all_expectations_met(),
        "pixel expectations failed: {report:?}"
    );

    println!("PASS: real Draw reached the render target and the pixels matched");
    Ok(())
}
