//! Constant buffers: D3D11 copies them per draw, so every draw's constants go
//! into a ring of uniform buffers and are bound with a dynamic offset. Writes
//! land in a CPU mirror and reach the GPU with one write per chunk before
//! each submit.
//!
//! Two sets of chunks alternate by frame, so a frame's constants never wait
//! for the GPU to finish reading the previous frame's. The constants' bind
//! groups name a chunk's buffer: a few per program layout and chunk (those
//! of the textures do not depend on the chunks). The two sets' chunks of an
//! index share a mirror: a frame's writes are uploaded before it is
//! presented, and the next frame writes the other set.

pub const CHUNK: u32 = 4 << 20;

pub struct Chunk {
    pub buffer: wgpu::Buffer,
    /// Unique (bind group keys).
    pub id: u32,
}

/// Where a chunk's constants are written, and the range not uploaded yet.
struct Mirror {
    bytes: Vec<u8>,
    dirty: Option<(u32, u32)>,
}

pub struct Uniforms {
    sets: [Vec<Chunk>; 2],
    /// By chunk index, for both sets.
    mirrors: Vec<Mirror>,
    set: usize,
    chunk: usize,
    offset: u32,
    align: u32,
    next_id: u32,
    /// Frame counter: packed constants can be reused within a frame.
    pub frame: u64,
}

/// Where a pack of constants went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alloc {
    /// Index of the chunk in the current set.
    pub chunk: usize,
    pub chunk_id: u32,
    pub offset: u32,
}

impl Uniforms {
    pub fn new(align: u32) -> Uniforms {
        Uniforms {
            sets: [Vec::new(), Vec::new()],
            mirrors: Vec::new(),
            set: 0,
            chunk: 0,
            offset: 0,
            align: align.max(16),
            next_id: 1,
            frame: 0,
        }
    }

    /// Room for `size` bytes; the caller fills them through [`Uniforms::bytes`].
    pub fn alloc(&mut self, device: &wgpu::Device, size: u32) -> Alloc {
        let size = size.next_multiple_of(self.align);
        if self.offset + size > CHUNK {
            self.chunk += 1;
            self.offset = 0;
        }
        let set = &mut self.sets[self.set];
        if self.chunk == set.len() {
            let id = self.next_id;
            self.next_id += 1;
            set.push(Chunk {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("constants"),
                    size: CHUNK as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                id,
            });
            if self.mirrors.len() == self.chunk {
                self.mirrors.push(Mirror { bytes: vec![0; CHUNK as usize], dirty: None });
            }
        }
        let offset = self.offset;
        self.offset += size;
        Alloc { chunk: self.chunk, chunk_id: set[self.chunk].id, offset }
    }

    /// The bytes of an allocation of `len` bytes, marked for upload.
    pub fn bytes(&mut self, a: Alloc, len: u32) -> &mut [u8] {
        let m = &mut self.mirrors[a.chunk];
        let (lo, hi) = (a.offset, a.offset + len);
        m.dirty = Some(m.dirty.map_or((lo, hi), |(l, h)| (l.min(lo), h.max(hi))));
        &mut m.bytes[lo as usize..hi as usize]
    }

    pub fn buffer(&self, chunk: usize) -> &wgpu::Buffer {
        &self.sets[self.set][chunk].buffer
    }

    /// Uploads what was written since the last flush (before a submit).
    pub fn flush(&mut self, uploads: &mut crate::uploads::Uploads) {
        for (c, m) in self.sets[self.set].iter().zip(&mut self.mirrors) {
            if let Some((lo, hi)) = m.dirty.take() {
                let hi = hi.next_multiple_of(4).min(CHUNK);
                uploads.buffer(&c.buffer, lo as u64, &m.bytes[lo as usize..hi as usize]);
            }
        }
    }

    /// After a present (its flush uploaded the frame's writes): the other
    /// set, from its start.
    pub fn next_frame(&mut self) {
        debug_assert!(self.mirrors.iter().all(|m| m.dirty.is_none()), "constants not uploaded");
        self.set ^= 1;
        self.chunk = 0;
        self.offset = 0;
        self.frame += 1;
    }

    pub fn bytes_allocated(&self) -> u64 {
        (self.sets[0].len() + self.sets[1].len()) as u64 * CHUNK as u64
    }
}
