//! Synthetic payload-to-executor smoke test, on the draw probe's own device.
//! This is not a live GpuRing worker or an application/presentation test.
use anyhow::{Result, ensure};
use glue5_gpu_d3d11::{D3D11Command, D3D11Executor};
use crate::ring_decode::{decode, Command, Kind};

fn submit(executor: &mut D3D11Executor, op: Kind, payload: &[u32]) -> Result<()> {
    let command = decode(Command { op, payload })?
        .ok_or_else(|| anyhow::anyhow!("expected a native command for {op}"))?;
    executor.execute(command)
}

pub fn run(executor: &mut D3D11Executor) -> Result<()> {
    // Malformed input must fail before it reaches the native executor.
    let before = executor.stats().commands;
    ensure!(decode(Command { op: Kind::UPLOAD_BUFFER, payload: &[101, 0] }).is_err(),
        "truncated upload unexpectedly accepted");
    ensure!(executor.stats().commands == before, "decoder mutated executor");

    submit(executor, Kind::CREATE_BUFFER, &[101, 16, 0x1, 0, 0])?;
    submit(executor, Kind::UPLOAD_BUFFER, &[101, 0, 16,
        0x03020100, 0x07060504, 0x0b0a0908, 0x0f0e0d0c])?;
    // Texture2D, 2x2, one mip/layer/sample, RGBA8, render-target bind.
    submit(executor, Kind::CREATE_TEXTURE, &[102, 2, 2, 2, 1, 1, 1, 28, 0x20, 1])?;
    submit(executor, Kind::UPLOAD_TEXTURE, &[102, 0, 0, 0, 0, 0, 1, 1, 1, 4, 1, 4, 0xff804020])?;
    submit(executor, Kind::CLEAR_RT, &[102, 0, 0,
        0.25f32.to_bits(), 0.5f32.to_bits(), 0.75f32.to_bits(), 1.0f32.to_bits()])?;
    ensure!(executor.buffer(101).is_some() && executor.texture(102).is_some(),
        "decoded resource IDs are missing");
    executor.execute(D3D11Command::Flush)?;
    executor.wait_for_gpu()?; // Real reusable D3D11 event query, not a fake fence.
    submit(executor, Kind::DESTROY_OBJECT, &[101])?;
    submit(executor, Kind::DESTROY_OBJECT, &[102])?;
    ensure!(executor.buffer(101).is_none() && executor.texture(102).is_none(),
        "decoded destroy left live resources");
    let stats = executor.stats();
    ensure!(stats.commands == before + 8, "unexpected native command count");
    ensure!(stats.buffers_uploaded == 1 && stats.buffer_bytes_uploaded == 16,
        "buffer upload counters mismatch");
    ensure!(stats.textures_uploaded == 1 && stats.texture_bytes_uploaded == 4,
        "texture upload counters mismatch");
    ensure!(stats.render_targets_cleared == 1 && stats.objects_destroyed == 2
        && stats.fences_completed == 1, "clear/destroy/fence counters mismatch");
    println!("[ok] decoded payloads -> native executor: buffer upload=16 bytes, texture upload=4 bytes, clear, real fence, destroy");
    println!("     resource probe checks submission/completion and lifetime; pixel assertions belong to the draw stages");
    Ok(())
}
