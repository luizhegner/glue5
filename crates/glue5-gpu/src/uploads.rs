//! Data from the engine to the GPU: written into pooled staging memory and
//! copied by an encoder of its own, submitted just before the commands
//! recorded meanwhile (what `Queue::write_buffer` does, without creating a
//! staging buffer per call: thousands of uploads per frame).
//!
//! Staging chunks travel with the batch that copies from them and come back
//! mapped once it is submitted (see [`recycle`]): batches are submitted by
//! another thread, so nothing here waits for a submit.

use std::sync::{Arc, Mutex};

/// A chunk's size: each batch takes the one being filled with it, however
/// little is in it (4-7 batches a frame), so chunks are small. A larger
/// upload (a texture's top mips) goes in pieces of a chunk at most: a
/// buffer made for it would be zeroed by wgpu on the command thread
/// (megabytes) and freed after.
pub const CHUNK: u64 = 1 << 20;
/// Bytes of free chunks kept for reuse, at most: loading needs many at
/// once, play few, and memory is scarce on phones.
const KEEP: u64 = 32 << 20;

/// Mapped chunks, ready to be written.
pub type FreeChunks = Arc<Mutex<Vec<wgpu::Buffer>>>;

pub struct Uploads {
    device: wgpu::Device,
    encoder: Option<wgpu::CommandEncoder>,
    /// The chunk being filled and its next free byte.
    current: Option<(wgpu::Buffer, u64)>,
    /// Chunks written since the last batch.
    used: Vec<wgpu::Buffer>,
    free: FreeChunks,
    /// Bytes uploaded and chunks made, for the statistics.
    pub bytes: u64,
    pub made: u64,
}

impl Uploads {
    pub fn new(device: wgpu::Device) -> Uploads {
        Uploads {
            device,
            encoder: None,
            current: None,
            used: Vec::new(),
            free: FreeChunks::default(),
            bytes: 0,
            made: 0,
        }
    }

    pub fn free_chunks(&self) -> FreeChunks {
        self.free.clone()
    }

    /// Takes out the free chunks (memory is short).
    pub fn take_free(&self) -> Vec<wgpu::Buffer> {
        std::mem::take(&mut *self.free.lock().unwrap())
    }

    fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        self.encoder.get_or_insert_with(|| {
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("uploads") })
        })
    }

    /// `size` bytes of staging memory, `align`-aligned: (chunk, offset).
    fn allocate(&mut self, size: u64, align: u64) -> (wgpu::Buffer, u64) {
        // Mapped ranges start at multiples of MAP_ALIGNMENT.
        let align = align.max(wgpu::MAP_ALIGNMENT);
        if let Some((buffer, next)) = &mut self.current {
            let at = next.next_multiple_of(align);
            if at + size <= buffer.size() {
                *next = at + size;
                return (buffer.clone(), at);
            }
        }
        if let Some((full, _)) = self.current.take() {
            self.used.push(full);
        }
        // Uploads come in pieces of a chunk at most, but for a texture row
        // larger than that (none is: 256 KiB at most).
        let reused = {
            let mut free = self.free.lock().unwrap();
            free.iter().position(|b| b.size() >= size).map(|i| free.swap_remove(i))
        };
        let buffer = reused.unwrap_or_else(|| {
            self.made += 1;
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("upload staging"),
                size: size.max(CHUNK),
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            })
        });
        self.current = Some((buffer.clone(), size));
        (buffer, 0)
    }

    /// `data` into `target` at `offset`; both 4-byte aligned in length and offset
    /// (a shorter tail is padded with zeros).
    pub fn buffer(&mut self, target: &wgpu::Buffer, offset: u64, data: &[u8]) {
        for (i, piece) in data.chunks(CHUNK as usize).enumerate() {
            self.buffer_piece(target, offset + i as u64 * CHUNK, piece);
        }
    }

    fn buffer_piece(&mut self, target: &wgpu::Buffer, offset: u64, data: &[u8]) {
        let size = (data.len() as u64).next_multiple_of(4);
        self.bytes += size;
        let (chunk, at) = self.allocate(size, wgpu::COPY_BUFFER_ALIGNMENT);
        {
            let Some(mut view) = self.mapped(&chunk, at, size) else { return };
            view.slice(..data.len()).copy_from_slice(data);
            view.slice(data.len()..).fill(0);
        }
        self.encoder().copy_buffer_to_buffer(&chunk, at, target, offset, size);
    }

    /// Rows of texel blocks into a texture region: `rows` rows of `row_bytes`
    /// per layer, `bpr` bytes apart in `data` and `src_rows` rows per layer.
    #[allow(clippy::too_many_arguments)]
    pub fn texture(
        &mut self,
        target: wgpu::TexelCopyTextureInfo,
        data: &[u8],
        bpr: u32,
        src_rows: u32,
        rows: u32,
        extent: wgpu::Extent3d,
        row_bytes: u32,
    ) {
        let pitch = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let layers = extent.depth_or_array_layers;
        let size = pitch as u64 * rows as u64 * layers as u64;
        if size == 0 {
            return;
        }
        self.bytes += size;
        // In one copy if it fits a chunk, else per layer in bands of rows
        // that do: (layers, rows) per copy.
        let (per_layers, per_rows) =
            if size <= CHUNK { (layers, rows) } else { (1, (CHUNK / pitch as u64).clamp(1, rows as u64) as u32) };
        // Texel rows per row of data (of blocks, for compressed formats).
        let texel_rows = extent.height.div_ceil(rows);
        for first_layer in (0..layers).step_by(per_layers as usize) {
            for first_row in (0..rows).step_by(per_rows as usize) {
                let n = per_rows.min(rows - first_row);
                let bytes = pitch as u64 * n as u64 * per_layers as u64;
                let (chunk, base) = self.allocate(bytes, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64);
                {
                    let Some(mut view) = self.mapped(&chunk, base, bytes) else { return };
                    for layer in 0..per_layers {
                        for row in 0..n {
                            let src = (((first_layer + layer) * src_rows + first_row + row) * bpr) as usize;
                            let dst = ((layer * n + row) * pitch) as usize;
                            let len = (row_bytes as usize).min(data.len().saturating_sub(src));
                            view.slice(dst..dst + len).copy_from_slice(&data[src..src + len]);
                        }
                    }
                }
                let o = target.origin;
                let y = first_row * texel_rows;
                self.encoder().copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &chunk,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: base,
                            bytes_per_row: Some(pitch),
                            rows_per_image: Some(n),
                        },
                    },
                    wgpu::TexelCopyTextureInfo {
                        origin: wgpu::Origin3d { x: o.x, y: o.y + y, z: o.z + first_layer },
                        ..target
                    },
                    wgpu::Extent3d {
                        width: extent.width,
                        height: (n * texel_rows).min(extent.height - y),
                        depth_or_array_layers: per_layers,
                    },
                );
            }
        }
    }

    /// `size` bytes of `chunk` at `at` to write into. None when the chunk
    /// could not be made (memory ran out, the device was lost): it is
    /// forgotten and the upload dropped, the game going on without it.
    fn mapped(&mut self, chunk: &wgpu::Buffer, at: u64, size: u64) -> Option<wgpu::BufferViewMut> {
        match chunk.slice(at..at + size).get_mapped_range_mut() {
            Ok(view) => Some(view),
            Err(e) => {
                crate::log_once("staging", || format!("gpu: no staging memory ({e}): uploads dropped"));
                if self.current.as_ref().is_some_and(|(c, _)| c == chunk) {
                    self.current = None;
                }
                None
            }
        }
    }

    /// The uploads to submit before the next commands, and the chunks they
    /// copy from (unmapped; to [`recycle`] after the submit).
    pub fn finish(&mut self) -> (Option<wgpu::CommandEncoder>, Vec<wgpu::Buffer>) {
        if let Some((chunk, _)) = self.current.take() {
            self.used.push(chunk);
        }
        let chunks = std::mem::take(&mut self.used);
        for chunk in &chunks {
            chunk.unmap();
        }
        (self.encoder.take(), chunks)
    }
}

/// Maps chunks again once the GPU has copied from them, back into `free`.
/// Those not kept are dropped: `destroy` would take wgpu's exclusive lock,
/// waiting for the encoding threads.
pub fn recycle(chunks: Vec<wgpu::Buffer>, free: &FreeChunks) {
    for chunk in chunks {
        let (back, free) = (chunk.clone(), free.clone());
        chunk.slice(..).map_async(wgpu::MapMode::Write, move |r| {
            let mut free = free.lock().unwrap();
            if r.is_ok() && free.iter().map(wgpu::Buffer::size).sum::<u64>() + back.size() <= KEEP {
                free.push(back);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    /// Textures larger than a chunk go in bands of rows per layer: what the
    /// GPU holds must be the data given, whole (RGBA8 in two layers at an
    /// offset, from padded rows; BC1 in rows of blocks). Needs a GPU, so
    /// ignored by default: `cargo test -p glue5-gpu -- --ignored uploads`.
    #[test]
    #[ignore]
    fn uploads_larger_than_a_chunk_land_whole() {
        let adapter = pollster::block_on(crate::instance().request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
            ..Default::default()
        }))
        .unwrap();
        // (format, texture size, layers, origin, region, block size and bytes, data rows per layer)
        let mut cases = vec![(wgpu::TextureFormat::Rgba8Unorm, (720, 520), 2, (3, 5), (700, 500), (1, 4), 510)];
        if device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
            cases.push((wgpu::TextureFormat::Bc1RgbaUnorm, (2048, 1600), 1, (0, 0), (2048, 1600), (4, 8), 400));
        }
        for (format, (tw, th), layers, (x, y), (w, h), (block, bytes), src_rows) in cases {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let (rows, row_bytes) = (h / block, w / block * bytes);
            let bpr = row_bytes + 12;
            let data: Vec<u8> = (0..bpr * src_rows * layers).map(|i| (i * 7 + i / 4093) as u8).collect();
            let mut uploads = super::Uploads::new(device.clone());
            let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: layers };
            let target = wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            };
            uploads.texture(target, &data, bpr, src_rows, rows, extent, row_bytes);
            assert!(!uploads.used.is_empty(), "{format:?}: in one chunk");
            let (encoder, _chunks) = uploads.finish();
            let pitch = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let back = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (pitch * rows * layers) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut read = device.create_command_encoder(&Default::default());
            read.copy_texture_to_buffer(
                target,
                wgpu::TexelCopyBufferInfo {
                    buffer: &back,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(pitch),
                        rows_per_image: Some(rows),
                    },
                },
                extent,
            );
            queue.submit([encoder.unwrap().finish(), read.finish()]);
            back.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let got = back.slice(..).get_mapped_range().unwrap();
            for layer in 0..layers {
                for row in 0..rows {
                    let src = ((layer * src_rows + row) * bpr) as usize;
                    let dst = ((layer * rows + row) * pitch) as usize;
                    let n = row_bytes as usize;
                    assert!(got[dst..dst + n] == data[src..src + n], "{format:?}: layer {layer}, row {row}");
                }
            }
        }
    }
}
