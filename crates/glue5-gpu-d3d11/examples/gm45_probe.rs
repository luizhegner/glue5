use anyhow::{Result, ensure};
use glue5_gpu_d3d11::{D3D11Command, D3D11Executor, TextureBindings};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R11G11B10_FLOAT, DXGI_FORMAT_R32_FLOAT,
};

fn main() -> Result<()> {
    let mut executor = D3D11Executor::open_hardware()?;
    println!("D3D11 hardware device opened");
    println!("Feature level: {:?}", executor.device().feature_level());

    let source_support = executor.device().format_support(DXGI_FORMAT_R11G11B10_FLOAT)?;
    println!(
        "R11G11B10_FLOAT: support=0x{:08x}, Texture2D+shader-load={}",
        source_support.raw,
        source_support.texture_2d_shader_load()
    );

    let r32_support = executor.device().format_support(DXGI_FORMAT_R32_FLOAT)?;
    println!(
        "R32_FLOAT: support=0x{:08x}, Texture2D+shader-load={}, Texture2D+RTV={}",
        r32_support.raw,
        r32_support.texture_2d_shader_load(),
        r32_support.texture_2d_render_target()
    );

    // Match the dimensions observed in the S21+ compute audit:
    // 240x108 source -> 15x7 partials -> 1x1 result.
    executor.execute(D3D11Command::CreateTexture2D {
        id: 1,
        width: 240,
        height: 108,
        format: DXGI_FORMAT_R11G11B10_FLOAT,
        bindings: TextureBindings::shader_resource(),
    })?;

    executor.execute(D3D11Command::CreateTexture2D {
        id: 2,
        width: 15,
        height: 7,
        format: DXGI_FORMAT_R32_FLOAT,
        bindings: TextureBindings::render_target_and_shader_resource(),
    })?;

    executor.execute(D3D11Command::CreateTexture2D {
        id: 3,
        width: 1,
        height: 1,
        format: DXGI_FORMAT_R32_FLOAT,
        bindings: TextureBindings::render_target_and_shader_resource(),
    })?;

    executor.execute(D3D11Command::ClearRenderTarget { id: 2, color: [0.05, 0.10, 0.20, 1.0] })?;
    executor.execute(D3D11Command::ClearRenderTarget { id: 3, color: [0.25, 0.25, 0.25, 1.0] })?;
    executor.execute(D3D11Command::Flush)?;
    executor.wait_for_gpu()?;
    assert_eq!(executor.stats().fences_completed, 1);
    println!("PASS: D3D11 event query completed on GM45");

    let source = executor.texture(1).expect("source texture was just created");
    let partials = executor.texture(2).expect("partial texture was just created");
    let final_value = executor.texture(3).expect("final texture was just created");
    assert!(source.srv().is_some());
    assert!(source.rtv().is_none());
    assert!(partials.srv().is_some() && partials.rtv().is_some());
    assert!(final_value.srv().is_some() && final_value.rtv().is_some());

    println!(
        "Resources created: source={}x{}, partials={}x{}, final={}x{}",
        source.width(),
        source.height(),
        partials.width(),
        partials.height(),
        final_value.width(),
        final_value.height()
    );

    let mut expected = vec![0u8; 64];
    let first = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b];
    let second = [0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87];
    expected[8..20].copy_from_slice(&first);
    expected[28..36].copy_from_slice(&second);

    executor.execute(D3D11Command::CreateBuffer {
        id: 4,
        size: 64,
        bind: 0x1, // D3D11 vertex-buffer binding from the engine's BIND_VERTEX bit.
        misc: 0,
        stride: 16,
    })?;
    executor.execute(D3D11Command::CreateBuffer {
        id: 6,
        size: 15, // Logical size is not word-aligned; physical ByteWidth is padded to 16.
        bind: 0x1,
        misc: 0,
        stride: 0,
    })?;
    executor.execute(D3D11Command::CreateBuffer {
        id: 7,
        size: 64,
        bind: 0x8, // Typed shader-resource buffer.
        misc: 0,
        stride: 0,
    })?;
    ensure!(
        executor.execute(D3D11Command::UploadBuffer { id: 6, offset: 1, data: vec![0xAA, 0xBB, 0xCC, 0xDD] }).is_err(),
        "unaligned D3D11 buffer upload offset must be rejected"
    );
    executor.execute(D3D11Command::UploadBuffer { id: 6, offset: 12, data: vec![0xAA, 0xBB, 0xCC] })?;
    executor.execute(D3D11Command::UploadBuffer { id: 4, offset: 8, data: first.to_vec() })?;
    executor.execute(D3D11Command::UploadBuffer { id: 4, offset: 28, data: second.to_vec() })?;

    // An 8x4 RGBA8 texture with four bytes of source padding at the end of each row.
    let row_bytes = 8 * 4;
    let source_pitch = row_bytes + 4;
    let mut texture_source = vec![0xEE; source_pitch * 4];
    let mut texture_expected = vec![0u8; row_bytes * 4];
    for row in 0..4 {
        for column in 0..row_bytes {
            let value = (row * row_bytes + column).wrapping_mul(13) as u8;
            texture_source[row * source_pitch + column] = value;
            texture_expected[row * row_bytes + column] = value;
        }
    }

    executor.execute(D3D11Command::CreateTexture2D {
        id: 5,
        width: 8,
        height: 4,
        format: DXGI_FORMAT_R8G8B8A8_UNORM,
        bindings: TextureBindings::shader_resource(),
    })?;
    executor.execute(D3D11Command::UploadTexture2D {
        id: 5,
        mip: 0,
        slice: 0,
        x: 0,
        y: 0,
        z: 0,
        width: 8,
        height: 4,
        depth: 1,
        bytes_per_row: source_pitch as u32,
        rows: 4,
        data: texture_source,
    })?;

    executor.execute(D3D11Command::CreateSrv {
        id: 8,
        resource_id: 5,
        is_buffer: false,
        format: 0,
        view_dim: 4,
        a: 0,
        b: 1,
        c: 0,
        d: 1,
    })?;
    executor.execute(D3D11Command::CreateSrv {
        id: 9,
        resource_id: 7,
        is_buffer: true,
        format: DXGI_FORMAT_R32_FLOAT.0 as u32,
        view_dim: 1,
        a: 1,
        b: 4,
        c: 0,
        d: 0,
    })?;
    executor.execute(D3D11Command::SetSrvs { stage: 0, start: 2, views: vec![8, 9] })?;
    executor.execute(D3D11Command::SetSrvs { stage: 1, start: 4, views: vec![9, 8] })?;
    executor.execute(D3D11Command::SetSrvs { stage: 0, start: 2, views: vec![0, 0] })?;
    executor.execute(D3D11Command::SetSrvs { stage: 1, start: 4, views: vec![0, 0] })?;
    ensure!(
        executor.device().set_shader_resources(2, 0, &[None]).is_err(),
        "compute-stage SRV binding must be rejected on GM45 Feature Level 10_0"
    );
    println!("PASS: texture and typed-buffer SRVs created and bound to VS/PS on GM45");
    println!("PASS: compute-stage SRV binding rejected on GM45 Feature Level 10_0");

    let sampler_words = vec![
        0x15, // MIN_MAG_MIP_LINEAR
        1,
        2,
        3, // WRAP, MIRROR, CLAMP
        0.0f32.to_bits(),
        1, // MaxAnisotropy (unused for non-anisotropic filter)
        8, // D3D11_COMPARISON_ALWAYS
        0.0f32.to_bits(),
        0.25f32.to_bits(),
        0.5f32.to_bits(),
        1.0f32.to_bits(),
        0.0f32.to_bits(),
        32.0f32.to_bits(),
    ];
    executor.execute(D3D11Command::CreateSampler { id: 12, words: sampler_words })?;
    executor.execute(D3D11Command::SetSamplers { stage: 0, start: 0, samplers: vec![12] })?;
    executor.execute(D3D11Command::SetSamplers { stage: 1, start: 1, samplers: vec![12] })?;
    executor.execute(D3D11Command::SetSamplers { stage: 0, start: 0, samplers: vec![0] })?;
    executor.execute(D3D11Command::SetSamplers { stage: 1, start: 1, samplers: vec![0] })?;
    ensure!(
        executor.device().set_samplers(2, 0, &[None]).is_err(),
        "compute-stage sampler binding must be rejected on GM45 Feature Level 10_0"
    );
    println!("PASS: sampler state created, bound to VS/PS and unbound on GM45");
    println!("PASS: compute-stage sampler binding rejected on GM45 Feature Level 10_0");

    executor.execute(D3D11Command::Flush)?;
    executor.wait_for_gpu()?;

    let buffer = executor.buffer(4).expect("buffer was just created");
    assert_eq!(buffer.size(), 64);
    assert_eq!(buffer.stride(), 16);
    let actual = executor.device().readback_buffer(buffer)?;
    assert_eq!(actual, expected, "D3D11 buffer readback must preserve both uploaded ranges and untouched bytes");
    println!("PASS: partial D3D11 buffer uploads round-tripped through staging on GM45");

    let tail_buffer = executor.buffer(6).expect("tail buffer was just created");
    let tail_actual = executor.device().readback_buffer(tail_buffer)?;
    let mut tail_expected = vec![0u8; 15];
    tail_expected[12..15].copy_from_slice(&[0xAA, 0xBB, 0xCC]);
    assert_eq!(
        tail_actual, tail_expected,
        "unaligned logical buffer tail must be padded without overwriting visible bytes"
    );
    println!("PASS: final partial buffer word is padded safely");

    let uploaded_texture = executor.texture(5).expect("texture was just created");
    let actual_texture = executor.device().readback_texture(uploaded_texture)?;
    assert_eq!(
        actual_texture, texture_expected,
        "D3D11 texture upload must respect padded source pitch and preserve every row"
    );
    println!("PASS: pitched RGBA8 texture upload round-tripped through staging on GM45");

    assert_eq!(executor.stats().fences_completed, 2);
    let stats = executor.stats();
    println!(
        "Commands: {}, texture creates: {}, texture uploads: {}, texture source bytes: {}, buffer creates: {}, buffer uploads: {}, uploaded bytes: {}, SRVs: {}, SRV binds: {}, SRV slots: {}, samplers: {}, sampler binds: {}, sampler slots: {}, clears: {}, flushes: {}",
        stats.commands,
        stats.textures_created,
        stats.textures_uploaded,
        stats.texture_bytes_uploaded,
        stats.buffers_created,
        stats.buffers_uploaded,
        stats.buffer_bytes_uploaded,
        stats.srvs_created,
        stats.srv_bind_calls,
        stats.srv_slots_bound,
        stats.samplers_created,
        stats.sampler_bind_calls,
        stats.sampler_slots_bound,
        stats.render_targets_cleared,
        stats.flushes
    );
    println!("PASS: typed D3D11 executor on GM45");

    executor.execute(D3D11Command::DestroyObject { id: 12 })?;
    executor.execute(D3D11Command::DestroyObject { id: 9 })?;
    executor.execute(D3D11Command::DestroyObject { id: 8 })?;
    executor.execute(D3D11Command::DestroyObject { id: 7 })?;
    executor.execute(D3D11Command::DestroyObject { id: 5 })?;
    executor.execute(D3D11Command::DestroyObject { id: 6 })?;
    executor.execute(D3D11Command::DestroyObject { id: 4 })?;
    executor.execute(D3D11Command::DestroyObject { id: 3 })?;
    executor.execute(D3D11Command::DestroyObject { id: 2 })?;
    executor.execute(D3D11Command::DestroyObject { id: 1 })?;
    println!("PASS: explicit resource destruction");

    Ok(())
}
