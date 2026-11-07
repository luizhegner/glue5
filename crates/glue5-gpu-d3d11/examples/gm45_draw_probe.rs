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
//! By default the run is *strict*: it requires the real Mobile Intel 4 Series
//! adapter (PCI 8086:2A42) at Feature Level 10_0, because that is the milestone
//! being certified. On a different machine pass `--allow-any-adapter`, which
//! still refuses a software adapter and still runs both draw stages.
//!
//! Exit code 0 means every expectation was met; any failure propagates through
//! `main`, prints the reason and exits non-zero so a script can gate on it.

use anyhow::{Result, ensure};
use glue5_gpu_d3d11::graphics::{
    PROBE_CLEAR_RGBA8, PROBE_DRAW_RGBA8, PROBE_STAGE_TWO_RGBA8, PROBE_TOLERANCE,
    PROBE_STAGE_TWO_TINT, pixel_close, run_draw_probe,
};
use glue5_gpu_d3d11::{D3D11Device, FeatureLevel};

fn main() -> Result<()> {
    let strict = !std::env::args().any(|argument| argument == "--allow-any-adapter");

    println!("[GLUE5 D3D11 draw probe - GM45 / FL10_0]");

    let device = D3D11Device::open_hardware()?;
    println!("[ok] hardware device opened (no WARP fallback)");
    println!("     feature level: {:?}", device.feature_level());

    let report = run_draw_probe(&device)?;

    println!(
        "[ok] adapter: vendor=0x{:04x} device=0x{:04x} software={} \"{}\"",
        report.adapter.vendor_id,
        report.adapter.device_id,
        report.adapter.software,
        report.adapter.description
    );
    if report.adapter.is_gm45() {
        println!("     identity: Mobile Intel 4 Series (GM45, PCI 8086:2A42)");
    } else if report.adapter.is_intel() {
        println!("     identity: Intel adapter, but not the GM45 PCI id");
    } else {
        println!("     identity: non-Intel adapter");
    }

    println!("[ok] render target: {}x{} R8G8B8A8_UNORM", report.width, report.height);
    println!(
        "[ok] stage 1 (SV_VertexID, Draw): centre={:?} corner={:?}",
        report.centre, report.corner
    );
    println!(
        "[ok] stage 2 (VB+IB+IL+cbuffer, DrawIndexed): centre={:?} corner={:?}",
        report.indexed_centre, report.indexed_corner
    );
    println!(
        "     expected: stage1 centre={PROBE_DRAW_RGBA8:?} stage2 centre={PROBE_STAGE_TWO_RGBA8:?} corners={PROBE_CLEAR_RGBA8:?} tolerance=±{PROBE_TOLERANCE}"
    );

    // ---- what the milestone requires, checked in the order that produces the
    // ---- most useful error message ----
    ensure!(
        !report.adapter.software,
        "the device was created on a software adapter; this run is not hardware evidence"
    );

    if strict {
        ensure!(
            report.adapter.is_gm45(),
            "expected the GM45 (PCI 8086:2A42) but found vendor=0x{:04x} device=0x{:04x}; re-run with --allow-any-adapter to exercise the pipeline on another adapter",
            report.adapter.vendor_id,
            report.adapter.device_id
        );
    }

    if report.adapter.is_gm45() {
        ensure!(
            report.feature_level == FeatureLevel::Level10_0,
            "the GM45 must negotiate Feature Level 10_0, but the device reported {:?}",
            report.feature_level
        );
    } else {
        ensure!(
            matches!(
                report.feature_level,
                FeatureLevel::Level10_0 | FeatureLevel::Level10_1 | FeatureLevel::Level11_0
            ),
            "unexpected feature level {:?}",
            report.feature_level
        );
        println!("     note: non-GM45 adapter, FL10_0 is not enforced for this run");
    }

    ensure!(
        pixel_close(report.centre, PROBE_DRAW_RGBA8, PROBE_TOLERANCE),
        "stage 1 centre mismatch: got {:?}, expected {:?}",
        report.centre,
        PROBE_DRAW_RGBA8
    );
    ensure!(
        pixel_close(report.indexed_centre, PROBE_STAGE_TWO_RGBA8, PROBE_TOLERANCE),
        "stage 2 centre mismatch: got {:?}, expected the constant-buffer tint {:?} (tint was {:?})",
        report.indexed_centre,
        PROBE_STAGE_TWO_RGBA8,
        PROBE_STAGE_TWO_TINT
    );
    ensure!(
        report.all_expectations_met(),
        "pixel expectations failed: {report:?}"
    );

    println!("PASS: real Draw reached the render target and the pixels matched");
    Ok(())
}
