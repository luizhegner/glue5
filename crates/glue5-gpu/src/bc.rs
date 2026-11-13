//! BCn texture data decoded on the CPU, for devices without BC compression
//! (iPhones before the A17 Pro, most Android GPUs).
//!
//! Decoded formats: BC1/2/3/7 -> RGBA8, BC4 -> R8, BC5 -> RG8, BC6H -> RGBA16F.
//! Where the GPU has ETC2 (all of those), the RGBA8 texels are encoded again
//! as ETC2 RGBA ([`etc2_rgba`], etcpak's fast encoder): a quarter of the
//! memory, which is most of a phone's texture memory otherwise (BC1 decoded
//! takes 8 times its size). ETC2 RGBA, not RGB: BC1 may hold 1-bit alpha.

use crate::format::Bc;

/// Blocks worth a thread of their own (a 512x512 texture). Streaming uploads
/// many 256x256 textures: split across 8 threads of 512 blocks each, a
/// quarter of the decoding threads' time on a Pixel 8a went to starting and
/// ending threads (stacks mapped, named and unmapped).
const BLOCKS_PER_THREAD: usize = 16384;

/// Threads for `blocks` blocks of work: one per [`BLOCKS_PER_THREAD`], at
/// most 8.
pub(crate) fn threads_for(blocks: usize) -> usize {
    (blocks / BLOCKS_PER_THREAD).clamp(1, glue5_runtime::cores().min(8))
}

impl Bc {
    /// Bytes per compressed 4x4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            Bc::Bc1 | Bc::Bc4 => 8,
            _ => 16,
        }
    }

    /// Bytes per decoded texel.
    pub fn texel_bytes(self) -> usize {
        match self {
            Bc::Bc4 => 1,
            Bc::Bc5 => 2,
            Bc::Bc6h => 8,
            _ => 4,
        }
    }
}

/// Decodes `cols` x `rows` blocks per layer (`layers` layers of `src_rows`
/// block rows, `src_bpr` bytes per block row) into tightly packed texels:
/// `cols * 4` texels per row, `rows * 4` rows per layer.
pub fn decode(bc: Bc, src: &[u8], src_bpr: usize, src_rows: usize, cols: usize, rows: usize, layers: usize) -> Vec<u8> {
    let texel = bc.texel_bytes();
    let pitch = cols * 4 * texel;
    let band = pitch * 4; // one row of blocks
    let mut out = vec![0u8; band * rows * layers];
    // Block rows, all layers: split into bands across threads for big uploads.
    let jobs: Vec<(usize, &mut [u8])> = out.chunks_mut(band).enumerate().collect();
    let threads = threads_for(cols * rows * layers);
    let decode_band = |(i, dst): (usize, &mut [u8])| {
        let (layer, row) = (i / rows, i % rows);
        let src_row = (layer * src_rows + row) * src_bpr;
        for col in 0..cols {
            let at = src_row + col * bc.block_bytes();
            let Some(block) = src.get(at..at + bc.block_bytes()) else { return };
            decode_block(bc, block, &mut dst[col * 4 * texel..], pitch);
        }
    };
    if threads <= 1 {
        jobs.into_iter().for_each(decode_band);
    } else {
        let per = jobs.len().div_ceil(threads);
        let mut jobs = jobs;
        std::thread::scope(|s| {
            while !jobs.is_empty() {
                let rest = jobs.split_off(per.min(jobs.len()));
                let chunk = std::mem::replace(&mut jobs, rest);
                s.spawn(move || chunk.into_iter().for_each(decode_band));
            }
        });
    }
    out
}

/// Bytes per ETC2 RGBA block (4x4 texels).
pub const ETC2_RGBA_BLOCK_BYTES: u32 = 16;

/// `width` x `height` RGBA8 texels (both multiples of 4, tightly packed)
/// encoded as ETC2 RGBA blocks, row by row.
pub fn etc2_rgba(mut texels: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
    // etcpak takes BGRA.
    for texel in texels.as_chunks_mut::<4>().0 {
        texel.swap(0, 2);
    }
    let pitch = width as usize * 4 * 4; // one row of blocks
    let band_bytes = (width / 4) as usize * ETC2_RGBA_BLOCK_BYTES as usize;
    let mut out = vec![0u8; band_bytes * (height / 4) as usize];
    // Big uploads in bands of block rows across threads, as decoding.
    let threads = threads_for((width / 4) as usize * (height / 4) as usize);
    let per = (height as usize / 4).div_ceil(threads);
    let encode = |(src, dst): (&[u8], &mut [u8])| {
        let rows = (src.len() / pitch) as u32 * 4;
        let surface = ctt_etcpak::Surface::new(src, width, rows);
        ctt_etcpak::etc2_rgba::compress_blocks_into(&surface, true, dst);
    };
    let bands = texels.chunks(pitch * per).zip(out.chunks_mut(band_bytes * per));
    if threads <= 1 {
        bands.for_each(encode);
    } else {
        std::thread::scope(|s| {
            for band in bands {
                s.spawn(move || encode(band));
            }
        });
    }
    out
}

/// One block into `dst` (its top-left texel), `pitch` bytes per texel row.
fn decode_block(bc: Bc, block: &[u8], dst: &mut [u8], pitch: usize) {
    match bc {
        Bc::Bc1 => bcdec_rs::bc1(block, dst, pitch),
        Bc::Bc2 => bcdec_rs::bc2(block, dst, pitch),
        Bc::Bc3 => bcdec_rs::bc3(block, dst, pitch),
        Bc::Bc4 => bcdec_rs::bc4(block, dst, pitch, false),
        Bc::Bc5 => bcdec_rs::bc5(block, dst, pitch, false),
        Bc::Bc7 => bcdec_rs::bc7(block, dst, pitch),
        Bc::Bc6h => {
            // RGB halves, then RGBA16F with alpha 1.0.
            let mut rgb = [0u16; 4 * 4 * 3];
            bcdec_rs::bc6h_half(block, &mut rgb, 4 * 3, false);
            for y in 0..4 {
                for x in 0..4 {
                    let s = (y * 4 + x) * 3;
                    let d = y * pitch + x * 8;
                    for (c, v) in [rgb[s], rgb[s + 1], rgb[s + 2], 0x3c00].into_iter().enumerate() {
                        dst[d + c * 2..d + c * 2 + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn etc2_rgba_keeps_colours_and_alpha() {
        // 8x8: four flat 4x4 blocks (opaque red, green, blue; half-transparent white).
        let colours = [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 128]];
        let mut texels = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                let c = colours[(y / 4) * 2 + x / 4];
                texels[(y * 8 + x) * 4..][..4].copy_from_slice(&c);
            }
        }
        let blocks = super::etc2_rgba(texels.clone(), 8, 8);
        assert_eq!(blocks.len(), 4 * super::ETC2_RGBA_BLOCK_BYTES as usize);
        // etcpak decodes to RGBA (its encoder takes BGRA).
        let decoded = ctt_etcpak::decode::decode_rgba(&blocks, 8, 8);
        for (got, want) in decoded.chunks(4).zip(texels.chunks(4)) {
            assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 8), "{got:?} for {want:?}");
        }
    }
}
