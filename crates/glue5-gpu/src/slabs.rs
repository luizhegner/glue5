//! Small vertex and index buffers, many to a GPU buffer. The engine makes
//! tens of thousands of them (a model's meshes), most under 16 KiB, and on
//! iOS each GPU buffer of its own takes 7-16 KiB of memory however small it
//! is: 20,000 buffers of 512 bytes took 288 MB on an iPhone 13 Pro. Here they
//! are slots of 4 MiB GPU buffers instead. On Vulkan too, where the
//! allocator places small buffers in large blocks anyway, but finds room by
//! scanning a block's free list: with tens of thousands of buffers streamed
//! in and out, that scan took 7 % of the command thread on a Pixel 8a, and
//! slots took a sixth off its recording time (its slabs keep 60-90 MiB of
//! freed slots for their next buffers).
//!
//! Slots come in size classes (four per power of two, so at most a fifth
//! is lost to rounding) and go back to their class's free list once the GPU
//! has finished the batch they were freed in. Buffers bound as storage or
//! indirect arguments keep buffers of their own (alignment, and usage
//! tracking per buffer).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use wgpu::BufferUsages;

const CHUNK: u64 = 4 << 20;
/// Largest size given a slot.
pub const MAX: u64 = 64 << 10;

/// Size classes: 256 bytes, then four per power of two up to `MAX`. All
/// multiples of 64 bytes, which is enough alignment for vertex and index
/// buffers and for copies.
const CLASSES: [u64; 33] = {
    let mut sizes = [0; 33];
    sizes[0] = 256;
    let (mut i, mut p) = (1, 256);
    while i < 33 {
        sizes[i] = p + p / 4 * ((i as u64 - 1) % 4 + 1);
        if (i - 1) % 4 == 3 {
            p *= 2;
        }
        i += 1;
    }
    sizes
};

/// Where a buffer lives in the slabs.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    chunk: u32,
    class: u8,
    pub offset: u64,
}

pub struct Slabs {
    device: wgpu::Device,
    chunks: Vec<wgpu::Buffer>,
    /// Next byte of the last chunk never handed out.
    tail: u64,
    /// Free slots by class.
    free: Vec<Vec<Slot>>,
    /// Slots freed in a batch the GPU may not have finished: (epoch, slot).
    pending: VecDeque<(u64, Slot)>,
    /// Batches (epochs) the GPU has finished, from the submit thread.
    finished: Arc<AtomicU64>,
    /// Bytes in slots handed out (class sizes).
    pub used: u64,
}

impl Slabs {
    pub fn new(device: wgpu::Device, finished: Arc<AtomicU64>) -> Slabs {
        Slabs {
            device,
            chunks: Vec::new(),
            tail: CHUNK,
            free: vec![Vec::new(); CLASSES.len()],
            pending: VecDeque::new(),
            finished,
            used: 0,
        }
    }

    /// Whether a buffer of `bytes` and `usage` goes in a slot.
    pub fn takes(bytes: u64, usage: BufferUsages) -> bool {
        let allowed = BufferUsages::VERTEX | BufferUsages::INDEX | BufferUsages::COPY_SRC | BufferUsages::COPY_DST;
        bytes <= MAX && allowed.contains(usage)
    }

    /// A slot of at least `bytes` (at most `MAX`), and the GPU buffer it is in.
    pub fn alloc(&mut self, bytes: u64) -> (Slot, wgpu::Buffer) {
        self.reclaim();
        let class = CLASSES.partition_point(|&c| c < bytes);
        let size = CLASSES[class];
        self.used += size;
        let slot = self.free[class].pop().unwrap_or_else(|| {
            if self.tail + size > CHUNK {
                self.chunks.push(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("slabs"),
                    size: CHUNK,
                    usage: BufferUsages::VERTEX | BufferUsages::INDEX | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                self.tail = 0;
            }
            let slot = Slot { chunk: self.chunks.len() as u32 - 1, class: class as u8, offset: self.tail };
            self.tail += size;
            slot
        });
        (slot, self.chunks[slot.chunk as usize].clone())
    }

    /// Frees a slot the commands of `epoch` may still use.
    pub fn free(&mut self, epoch: u64, slot: Slot) {
        self.used -= CLASSES[slot.class as usize];
        self.pending.push_back((epoch, slot));
    }

    /// Slots freed in batches the GPU has finished, back to their classes.
    fn reclaim(&mut self) {
        let finished = self.finished.load(Ordering::Acquire);
        while let Some(&(epoch, slot)) = self.pending.front() {
            if epoch > finished {
                break;
            }
            self.pending.pop_front();
            self.free[slot.class as usize].push(slot);
        }
    }

    /// The GPU memory taken, in bytes.
    pub fn bytes(&self) -> u64 {
        self.chunks.len() as u64 * CHUNK
    }
}

#[test]
fn classes() {
    assert_eq!(&CLASSES[..6], &[256, 320, 384, 448, 512, 640]);
    assert_eq!(CLASSES[CLASSES.len() - 1], MAX);
    assert!(CLASSES.windows(2).all(|w| w[0] < w[1] && w[1] % 64 == 0 && w[1] * 4 <= w[0] * 5 + 1));
}
