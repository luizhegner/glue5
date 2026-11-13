//! Pure payload decoding; numeric wire opcodes remain owned by glue5-runtime.
use anyhow::bail;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use crate::{D3D11Command, TextureBindings};

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    NOP,
    INIT,
    LOG,
    SCENE_DONE,
    CREATE_BUFFER,
    UPLOAD_BUFFER,
    UPLOAD_BUFFER_NO_OVERWRITE,
    CREATE_SRV,
    SET_SRVS,
    CREATE_STATE,
    SET_SAMPLERS,
    CREATE_UAV,
    SET_UAVS,
    UPLOAD_TEXTURE,
    CREATE_TEXTURE,
    DESTROY_OBJECT,
    CLEAR_RT,
    FENCE,
}
impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
}

#[derive(Clone, Copy)]
pub(crate) struct Command<'a> {
    pub(crate) op: Kind,
    pub(crate) payload: &'a [u32],
}
impl Command<'_> {
    fn w(&self, index: usize) -> u32 { self.payload[index] }
    fn f(&self, index: usize) -> f32 { f32::from_bits(self.w(index)) }
    fn addr(&self, index: usize) -> u64 { u64::from(self.w(index)) | (u64::from(self.w(index + 1)) << 32) }
    fn bytes_from(&self, index: usize) -> Vec<u8> {
        self.payload[index..].iter().flat_map(|word| word.to_le_bytes()).collect()
    }
}

/// Decodes the completion target and sequence word from a FENCE command.
pub(crate) fn decode_fence(command: Command<'_>) -> anyhow::Result<(u64, u32)> {
    if command.payload.len() < 3 {
        bail!("FENCE payload is truncated: expected 3 words, got {}", command.payload.len());
    }
    Ok((command.addr(0), command.w(2)))
}

/// Converts a non-fence ring command without touching the GPU. Keeping
/// protocol decoding pure makes the payload layout and allowlist unit-testable.
pub(crate) fn decode(command: Command<'_>) -> anyhow::Result<Option<D3D11Command>> {
    match command.op {
        Kind::NOP | Kind::INIT | Kind::LOG | Kind::SCENE_DONE => Ok(None),
        Kind::CREATE_BUFFER => {
            if command.payload.len() < 5 {
                bail!("CREATE_BUFFER payload is truncated: expected 5 words, got {}", command.payload.len());
            }
            Ok(Some(D3D11Command::CreateBuffer {
                id: command.w(0),
                size: command.w(1),
                bind: command.w(2),
                misc: command.w(3),
                stride: command.w(4),
            }))
        }
        Kind::UPLOAD_BUFFER | Kind::UPLOAD_BUFFER_NO_OVERWRITE => {
            if command.payload.len() < 3 {
                bail!("UPLOAD_BUFFER payload is truncated: expected 3 header words, got {}", command.payload.len());
            }
            let size = command.w(2) as usize;
            if size == 0 {
                return Ok(None);
            }
            let available = command.bytes_from(3);
            if available.len() < size {
                bail!(
                    "UPLOAD_BUFFER payload is truncated: header says {size} bytes, only {} are present",
                    available.len()
                );
            }
            Ok(Some(D3D11Command::UploadBuffer {
                id: command.w(0),
                offset: command.w(1),
                data: available[..size].to_vec(),
            }))
        }
        Kind::CREATE_SRV => {
            if command.payload.len() < 9 {
                bail!("CREATE_SRV payload is truncated: expected 9 words, got {}", command.payload.len());
            }
            let is_buffer = match command.w(1) {
                0 => false,
                1 => true,
                value => bail!("CREATE_SRV resource kind must be 0 (texture) or 1 (buffer), got {value}"),
            };
            let (format, view_dim, a, b, c, d) =
                (command.w(3), command.w(4), command.w(5), command.w(6), command.w(7), command.w(8));
            if is_buffer {
                if view_dim != 1 {
                    bail!("CREATE_SRV buffer: only D3D11 BUFFER views are supported");
                }
            } else if view_dim != 4
                || a != 0
                || !(b == 0 || b == 1 || b == u32::MAX)
                || c != 0
                || !(d == 0 || d == 1 || d == u32::MAX)
            {
                bail!("CREATE_SRV texture: only a base-mip, single-slice TEXTURE2D view is supported");
            }
            Ok(Some(D3D11Command::CreateSrv {
                id: command.w(0),
                resource_id: command.w(2),
                is_buffer,
                format,
                view_dim,
                a,
                b,
                c,
                d,
            }))
        }
        Kind::SET_SRVS => {
            if command.payload.len() < 3 {
                bail!("SET_SRVS payload is truncated: expected 3 header words, got {}", command.payload.len());
            }
            let (stage, start, count) = (command.w(0), command.w(1), command.w(2) as usize);
            if stage > 2 {
                bail!("SET_SRVS stage {stage} is unsupported (expected vertex=0, pixel=1, compute=2)");
            }
            let end = start.checked_add(count as u32).ok_or_else(|| anyhow::anyhow!("SET_SRVS slot range overflow"))?;
            if end > 128 {
                bail!("SET_SRVS slot range exceeds 128 slots");
            }
            if command.payload.len() < 3 + count {
                bail!(
                    "SET_SRVS payload is truncated: expected {} object IDs, got {}",
                    count,
                    command.payload.len().saturating_sub(3)
                );
            }
            Ok(Some(D3D11Command::SetSrvs { stage, start, views: command.payload[3..3 + count].to_vec() }))
        }
        Kind::CREATE_STATE => {
            if command.payload.len() < 2 {
                bail!("CREATE_STATE payload is truncated: expected id and kind");
            }
            let (id, kind) = (command.w(0), command.w(1));
            if kind != 3 {
                bail!("CREATE_STATE kind {kind} is unsupported; only sampler states (kind 3) are implemented");
            }
            if command.payload.len() < 15 {
                bail!("sampler CREATE_STATE payload is truncated: expected 15 words, got {}", command.payload.len());
            }
            Ok(Some(D3D11Command::CreateSampler { id, words: command.payload[2..15].to_vec() }))
        }
        Kind::SET_SAMPLERS => {
            if command.payload.len() < 3 {
                bail!("SET_SAMPLERS payload is truncated: expected 3 header words, got {}", command.payload.len());
            }
            let (stage, start, count) = (command.w(0), command.w(1), command.w(2) as usize);
            if stage > 2 {
                bail!("SET_SAMPLERS stage {stage} is unsupported (expected vertex=0, pixel=1, compute=2)");
            }
            let end =
                start.checked_add(count as u32).ok_or_else(|| anyhow::anyhow!("SET_SAMPLERS slot range overflow"))?;
            if end > 16 {
                bail!("SET_SAMPLERS slot range exceeds 16 slots");
            }
            if command.payload.len() < 3 + count {
                bail!(
                    "SET_SAMPLERS payload is truncated: expected {} object IDs, got {}",
                    count,
                    command.payload.len().saturating_sub(3)
                );
            }
            Ok(Some(D3D11Command::SetSamplers { stage, start, samplers: command.payload[3..3 + count].to_vec() }))
        }
        Kind::CREATE_UAV => bail!("CREATE_UAV is unsupported by this GM45 Feature Level 10_0 backend"),
        Kind::SET_UAVS => bail!("SET_UAVS is unsupported by this GM45 Feature Level 10_0 backend"),
        Kind::UPLOAD_TEXTURE => {
            if command.payload.len() < 12 {
                bail!("UPLOAD_TEXTURE payload is truncated: expected 12 header words, got {}", command.payload.len());
            }
            let size = command.w(11) as usize;
            if size == 0 {
                return Ok(None);
            }
            let available = command.bytes_from(12);
            if available.len() < size {
                bail!(
                    "UPLOAD_TEXTURE payload is truncated: header says {size} bytes, only {} are present",
                    available.len()
                );
            }
            Ok(Some(D3D11Command::UploadTexture2D {
                id: command.w(0),
                mip: command.w(1),
                slice: command.w(2),
                x: command.w(3),
                y: command.w(4),
                z: command.w(5),
                width: command.w(6),
                height: command.w(7),
                depth: command.w(8),
                bytes_per_row: command.w(9),
                rows: command.w(10),
                data: available[..size].to_vec(),
            }))
        }
        Kind::CREATE_TEXTURE => {
            if command.payload.len() < 10 {
                bail!("CREATE_TEXTURE payload is truncated: expected 10 words, got {}", command.payload.len());
            }
            let (id, dimension, width, height, depth, mip_levels, array_size, format, bind, samples) = (
                command.w(0),
                command.w(1),
                command.w(2),
                command.w(3),
                command.w(4),
                command.w(5),
                command.w(6),
                command.w(7),
                command.w(8),
                command.w(9),
            );

            if dimension != 2 {
                bail!("CREATE_TEXTURE id {id}: only 2D textures are supported (dimension={dimension})");
            }
            if depth != 1 {
                bail!("CREATE_TEXTURE id {id}: 2D texture depth must be 1 (depth={depth})");
            }
            if mip_levels > 1 || array_size > 1 || samples > 1 {
                bail!("CREATE_TEXTURE id {id}: only one mip, one array slice and one sample are supported");
            }
            if bind & (0x40 | 0x80) != 0 {
                bail!("CREATE_TEXTURE id {id}: depth-stencil and UAV bindings are not supported yet");
            }

            Ok(Some(D3D11Command::CreateTexture2D {
                id,
                width: width.max(1),
                height: height.max(1),
                format: DXGI_FORMAT(format as i32),
                bindings: TextureBindings { shader_resource: true, render_target: bind & 0x20 != 0 },
            }))
        }
        Kind::DESTROY_OBJECT => {
            if command.payload.is_empty() {
                bail!("DESTROY_OBJECT payload is truncated: expected 1 word, got 0");
            }
            Ok(Some(D3D11Command::DestroyObject { id: command.w(0) }))
        }
        Kind::CLEAR_RT => {
            if command.payload.len() < 7 {
                bail!("CLEAR_RT payload is truncated: expected 7 words, got {}", command.payload.len());
            }
            if command.w(1) != 0 || command.w(2) != 0 {
                bail!("CLEAR_RT only supports mip 0 and slice 0 (mip={}, slice={})", command.w(1), command.w(2));
            }
            Ok(Some(D3D11Command::ClearRenderTarget {
                id: command.w(0),
                color: [command.f(3), command.f(4), command.f(5), command.f(6)],
            }))
        }
        unsupported => bail!("unsupported GpuRing opcode {unsupported}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

    fn command(op: Kind, payload: &[u32]) -> Command<'_> {
        Command { op, payload }
    }

    #[test]
    fn create_buffer_decodes_the_engine_payload_layout() {
        let words = [91, 256, 0x1 | 0x8, 0x10, 32];
        match decode(command(Kind::CREATE_BUFFER, &words)).unwrap().unwrap() {
            D3D11Command::CreateBuffer { id, size, bind, misc, stride } => {
                assert_eq!((id, size, bind, misc, stride), (91, 256, 0x9, 0x10, 32));
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn upload_buffer_decodes_inline_bytes_and_offset() {
        let words = [7, 4, 6, 0x4433_2211, 0x0000_6655];
        match decode(command(Kind::UPLOAD_BUFFER, &words)).unwrap().unwrap() {
            D3D11Command::UploadBuffer { id, offset, data } => {
                assert_eq!((id, offset), (7, 4));
                assert_eq!(data, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn upload_buffer_rejects_truncated_header_and_bytes() {
        assert!(decode(command(Kind::UPLOAD_BUFFER, &[7, 0])).unwrap_err().to_string().contains("header"));
        assert!(
            decode(command(Kind::UPLOAD_BUFFER, &[7, 0, 5, 0x4433_2211]))
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );
    }

    #[test]
    fn zero_length_buffer_upload_is_a_noop() {
        assert!(decode(command(Kind::UPLOAD_BUFFER_NO_OVERWRITE, &[7, 12, 0])).unwrap().is_none());
    }

    #[test]
    fn create_srv_decodes_texture_and_typed_buffer_views() {
        let texture = [21, 0, 5, 28, 4, 0, 1, 0, 1];
        match decode(command(Kind::CREATE_SRV, &texture)).unwrap().unwrap() {
            D3D11Command::CreateSrv { id, resource_id, is_buffer, format, view_dim, a, b, c, d } => {
                assert_eq!(
                    (id, resource_id, is_buffer, format, view_dim, a, b, c, d),
                    (21, 5, false, 28, 4, 0, 1, 0, 1)
                );
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }

        let buffer = [22, 1, 9, 41, 1, 3, 5, 0, 0];
        match decode(command(Kind::CREATE_SRV, &buffer)).unwrap().unwrap() {
            D3D11Command::CreateSrv { id, resource_id, is_buffer, format, view_dim, a, b, .. } => {
                assert_eq!((id, resource_id, is_buffer, format, view_dim, a, b), (22, 9, true, 41, 1, 3, 5));
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn create_srv_rejects_unsupported_view_shapes() {
        assert!(decode(command(Kind::CREATE_SRV, &[1, 2, 3, 0, 1, 0, 1, 0, 0])).is_err());
        assert!(decode(command(Kind::CREATE_SRV, &[1, 0, 3, 0, 5, 0, 1, 0, 1])).is_err());
        assert!(decode(command(Kind::CREATE_SRV, &[1, 0, 3, 0, 4, 1, 1, 0, 1])).is_err());
    }

    #[test]
    fn set_srvs_decodes_stage_range_and_null_slots() {
        let words = [1, 4, 3, 90, 0, 91];
        match decode(command(Kind::SET_SRVS, &words)).unwrap().unwrap() {
            D3D11Command::SetSrvs { stage, start, views } => {
                assert_eq!((stage, start), (1, 4));
                assert_eq!(views, [90, 0, 91]);
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn set_srvs_rejects_invalid_stage_range_and_truncation() {
        assert!(decode(command(Kind::SET_SRVS, &[3, 0, 1, 5])).is_err());
        assert!(decode(command(Kind::SET_SRVS, &[1, 127, 2, 5, 6])).is_err());
        assert!(decode(command(Kind::SET_SRVS, &[1, 0, 2, 5])).unwrap_err().to_string().contains("truncated"));
    }

    #[test]
    fn create_state_accepts_only_sampler_state_kind_and_full_payload() {
        let words = [17, 3, 0x15, 1, 2, 3, 0, 1, 8, 0, 0, 0, 0, 0, 32.0f32.to_bits()];
        match decode(command(Kind::CREATE_STATE, &words)).unwrap().unwrap() {
            D3D11Command::CreateSampler { id, words } => {
                assert_eq!(id, 17);
                assert_eq!(words.len(), 13);
                assert_eq!(words[0], 0x15);
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
        assert!(decode(command(Kind::CREATE_STATE, &[17, 1])).unwrap_err().to_string().contains("kind"));
        assert!(decode(command(Kind::CREATE_STATE, &[17, 3, 0])).unwrap_err().to_string().contains("truncated"));
    }

    #[test]
    fn set_samplers_decodes_slots_and_null_unbinds() {
        let words = [1, 3, 2, 20, 0];
        match decode(command(Kind::SET_SAMPLERS, &words)).unwrap().unwrap() {
            D3D11Command::SetSamplers { stage, start, samplers } => {
                assert_eq!((stage, start), (1, 3));
                assert_eq!(samplers, [20, 0]);
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn set_samplers_rejects_invalid_stage_range_and_truncation() {
        assert!(decode(command(Kind::SET_SAMPLERS, &[3, 0, 1, 5])).is_err());
        assert!(decode(command(Kind::SET_SAMPLERS, &[1, 15, 2, 5, 6])).is_err());
        assert!(decode(command(Kind::SET_SAMPLERS, &[1, 0, 2, 5])).unwrap_err().to_string().contains("truncated"));
    }

    #[test]
    fn uav_commands_are_explicitly_rejected_on_feature_level_10() {
        assert!(decode(command(Kind::CREATE_UAV, &[1; 9])).unwrap_err().to_string().contains("Feature Level 10_0"));
        assert!(decode(command(Kind::SET_UAVS, &[0; 3])).unwrap_err().to_string().contains("Feature Level 10_0"));
    }

    #[test]
    fn upload_texture_decodes_the_engine_payload_and_inline_bytes() {
        let mut words = vec![5, 0, 0, 1, 2, 0, 3, 2, 1, 12, 2, 24];
        words.extend((0..6).map(|value| 0x0302_0100u32.wrapping_add(value * 0x0404_0404)));
        match decode(command(Kind::UPLOAD_TEXTURE, &words)).unwrap().unwrap() {
            D3D11Command::UploadTexture2D {
                id,
                mip,
                slice,
                x,
                y,
                z,
                width,
                height,
                depth,
                bytes_per_row,
                rows,
                data,
            } => {
                assert_eq!((id, mip, slice, x, y, z, width, height, depth), (5, 0, 0, 1, 2, 0, 3, 2, 1));
                assert_eq!((bytes_per_row, rows), (12, 2));
                assert_eq!(data.len(), 24);
                assert_eq!(&data[..4], &[0, 1, 2, 3]);
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn upload_texture_rejects_truncated_header_and_data() {
        assert!(decode(command(Kind::UPLOAD_TEXTURE, &[5, 0])).unwrap_err().to_string().contains("header"));
        let mut words = vec![5, 0, 0, 0, 0, 0, 2, 2, 1, 8, 2, 16];
        words.push(0);
        assert!(decode(command(Kind::UPLOAD_TEXTURE, &words)).unwrap_err().to_string().contains("truncated"));
    }

    #[test]
    fn create_texture_decodes_the_engine_payload_layout() {
        let words = [17, 2, 640, 480, 1, 1, 1, DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32, 0x20, 1];

        match decode(command(Kind::CREATE_TEXTURE, &words)).unwrap().unwrap() {
            D3D11Command::CreateTexture2D { id, width, height, format, bindings } => {
                assert_eq!(id, 17);
                assert_eq!((width, height), (640, 480));
                assert_eq!(format, DXGI_FORMAT_R8G8B8A8_UNORM);
                assert!(bindings.shader_resource);
                assert!(bindings.render_target);
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn create_texture_rejects_unsupported_shapes_and_bindings() {
        let base = [1, 2, 32, 32, 1, 1, 1, DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32, 0, 1];

        for (index, value) in [(1, 3), (4, 2), (5, 2), (6, 2), (9, 4), (8, 0x40), (8, 0x80)] {
            let mut words = base;
            words[index] = value;
            assert!(
                decode(command(Kind::CREATE_TEXTURE, &words)).is_err(),
                "payload index {index} with value {value:#x} should be rejected"
            );
        }
    }

    #[test]
    fn fence_is_handled_by_the_completion_path_not_the_regular_decoder() {
        let words = [0; 4];
        let error = decode(command(Kind::FENCE, &words)).unwrap_err();
        assert!(error.to_string().contains("unsupported GpuRing opcode"));
    }

    #[test]
    fn fence_payload_decodes_address_and_sequence() {
        let words = [0x89ab_cdef, 0x0123_4567, 0x55aa_7711];
        let (address, sequence) = decode_fence(command(Kind::FENCE, &words)).unwrap();
        assert_eq!(address, 0x0123_4567_89ab_cdef);
        assert_eq!(sequence, 0x55aa_7711);
    }

    #[test]
    fn fence_rejects_truncated_payloads() {
        for words in [&[][..], &[1][..], &[1, 2][..]] {
            assert!(decode_fence(command(Kind::FENCE, words)).unwrap_err().to_string().contains("truncated"));
        }
    }

    #[test]
    fn clear_render_target_decodes_color_words() {
        let words = [4, 0, 0, 0.25f32.to_bits(), 0.5f32.to_bits(), 0.75f32.to_bits(), 1.0f32.to_bits()];
        match decode(command(Kind::CLEAR_RT, &words)).unwrap().unwrap() {
            D3D11Command::ClearRenderTarget { id, color } => {
                assert_eq!(id, 4);
                assert_eq!(color, [0.25, 0.5, 0.75, 1.0]);
            }
            other => panic!("unexpected decoded command: {other:?}"),
        }
    }

    #[test]
    fn clear_render_target_rejects_nonzero_mip_or_slice() {
        let base = [4, 0, 0, 0.0f32.to_bits(), 0.0f32.to_bits(), 0.0f32.to_bits(), 1.0f32.to_bits()];
        for (index, value) in [(1, 1), (2, 1)] {
            let mut words = base;
            words[index] = value;
            assert!(
                decode(command(Kind::CLEAR_RT, &words)).unwrap_err().to_string().contains("only supports mip 0"),
                "payload index {index} should be rejected"
            );
        }
    }

    #[test]
    fn truncated_payloads_are_rejected() {
        assert!(decode(command(Kind::CREATE_TEXTURE, &[1, 2])).unwrap_err().to_string().contains("truncated"));
        assert!(decode(command(Kind::DESTROY_OBJECT, &[])).unwrap_err().to_string().contains("truncated"));
        assert!(decode(command(Kind::CLEAR_RT, &[1, 2])).unwrap_err().to_string().contains("truncated"));
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;
    #[test]
    fn zero_sized_texture_upload_is_a_noop() {
        assert!(decode(Command { op: Kind::UPLOAD_TEXTURE, payload: &[0; 12] }).unwrap().is_none());
    }
    #[test]
    fn all_noop_kinds_ignore_payload() {
        for op in [Kind::NOP, Kind::INIT, Kind::LOG, Kind::SCENE_DONE] {
            assert!(decode(Command { op, payload: &[] }).unwrap().is_none());
        }
    }
    #[test]
    fn every_short_header_is_rejected_without_panicking() {
        for (op, minimum) in [
            (Kind::CREATE_BUFFER, 5), (Kind::UPLOAD_BUFFER, 3), (Kind::CREATE_SRV, 9),
            (Kind::SET_SRVS, 3), (Kind::CREATE_STATE, 2), (Kind::SET_SAMPLERS, 3),
            (Kind::UPLOAD_TEXTURE, 12), (Kind::CREATE_TEXTURE, 10),
            (Kind::DESTROY_OBJECT, 1), (Kind::CLEAR_RT, 7),
        ] {
            let words = [0; 12];
            for length in 0..minimum {
                assert!(decode(Command { op, payload: &words[..length] }).is_err(), "{op:?}: {length}");
            }
        }
    }
    #[test]
    fn slot_arithmetic_overflow_is_rejected() {
        for op in [Kind::SET_SRVS, Kind::SET_SAMPLERS] {
            assert!(decode(Command { op, payload: &[1, u32::MAX, 1, 0] }).is_err());
            assert!(decode(Command { op, payload: &[1, 1, u32::MAX] }).is_err());
        }
    }
}
