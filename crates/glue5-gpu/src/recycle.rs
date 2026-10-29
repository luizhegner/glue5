//! Buffers and textures the engine destroyed, kept for its next ones of the
//! same description once the GPU is done with them. Creating one is a call
//! into the kernel driver on the command thread (about a tenth of its time
//! on an iPhone 13 Pro while the world streams in), and the engine destroys
//! and creates the same sizes over and over: while loading, 95% of new
//! buffers matched one destroyed earlier.
//!
//! An object is reused once the GPU has finished the batch it was destroyed
//! in (the submit thread counts finished batches). What is kept is bounded
//! in bytes and in age; the rest is destroyed as before (the graveyard).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustc_hash::FxHashMap;
use wgpu::{BufferUsages, Extent3d, TextureDimension, TextureFormat, TextureUsages};

use crate::submit::Grave;

/// Kept at most, in bytes.
const MAX_BYTES: u64 = 32 << 20;
/// Kept at most for this many batches (a few seconds).
const MAX_AGE: u64 = 1000;

/// What a texture is made of; equal descriptions are interchangeable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TexKey {
    pub format: TextureFormat,
    pub view: Option<TextureFormat>,
    pub dim: TextureDimension,
    pub size: Extent3d,
    pub mips: u32,
    pub samples: u32,
    pub usage: TextureUsages,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Buffer(u64, BufferUsages),
    Texture(TexKey),
}

enum Object {
    Buffer(wgpu::Buffer),
    Texture(wgpu::Texture),
}

impl Object {
    fn grave(self) -> Grave {
        match self {
            Object::Buffer(b) => Grave::Buffer(b),
            Object::Texture(t) => Grave::Texture(t),
        }
    }
}

#[derive(Default)]
pub struct Recycler {
    /// Batches (epochs) the GPU has finished, from the submit thread.
    pub finished: Arc<AtomicU64>,
    /// Kept objects by description, oldest first: (epoch destroyed, object, bytes).
    kept: FxHashMap<Key, VecDeque<(u64, Object, u64)>>,
    /// Everything kept, in the order it came (some taken since).
    order: VecDeque<(u64, Key)>,
    bytes: u64,
    pub reused: u64,
}

impl Recycler {
    /// A buffer of `bytes` and `usage` destroyed in an epoch the GPU has finished.
    pub fn buffer(&mut self, bytes: u64, usage: BufferUsages) -> Option<wgpu::Buffer> {
        match self.take(&Key::Buffer(bytes, usage))? {
            Object::Buffer(b) => Some(b),
            Object::Texture(_) => None,
        }
    }

    pub fn texture(&mut self, key: &TexKey) -> Option<wgpu::Texture> {
        match self.take(&Key::Texture(key.clone()))? {
            Object::Texture(t) => Some(t),
            Object::Buffer(_) => None,
        }
    }

    /// Keeps a buffer the commands of `epoch` may still use.
    pub fn keep_buffer(&mut self, epoch: u64, buffer: wgpu::Buffer) {
        let (bytes, usage) = (buffer.size(), buffer.usage());
        self.keep(epoch, Key::Buffer(bytes, usage), Object::Buffer(buffer), bytes);
    }

    pub fn keep_texture(&mut self, epoch: u64, key: TexKey, texture: wgpu::Texture, bytes: u64) {
        self.keep(epoch, Key::Texture(key), Object::Texture(texture), bytes);
    }

    /// Hands what is too old, or over the budget, to the graveyard.
    pub fn trim(&mut self, epoch: u64, graveyard: &mut Vec<Grave>) {
        while let Some(&(at, _)) = self.order.front() {
            if self.bytes <= MAX_BYTES && epoch.saturating_sub(at) <= MAX_AGE {
                break;
            }
            let (at, key) = self.order.pop_front().unwrap();
            let Some(list) = self.kept.get_mut(&key) else { continue };
            // Taken since: its record is stale.
            if list.front().is_none_or(|(e, ..)| *e != at) {
                continue;
            }
            let (_, object, bytes) = list.pop_front().unwrap();
            if list.is_empty() {
                self.kept.remove(&key);
            }
            self.bytes -= bytes;
            graveyard.push(object.grave());
        }
    }

    /// Hands everything kept to the graveyard (memory is short); returns
    /// its bytes.
    pub fn release(&mut self, graveyard: &mut Vec<Grave>) -> u64 {
        self.order.clear();
        graveyard.extend(self.kept.drain().flat_map(|(_, list)| list).map(|(_, object, _)| object.grave()));
        std::mem::take(&mut self.bytes)
    }

    fn take(&mut self, key: &Key) -> Option<Object> {
        let finished = self.finished.load(Ordering::Acquire);
        let list = self.kept.get_mut(key)?;
        if list.front().is_none_or(|(epoch, ..)| *epoch > finished) {
            return None;
        }
        let (_, object, bytes) = list.pop_front().unwrap();
        if list.is_empty() {
            self.kept.remove(key);
        }
        self.bytes -= bytes;
        self.reused += 1;
        Some(object)
    }

    fn keep(&mut self, epoch: u64, key: Key, object: Object, bytes: u64) {
        self.kept.entry(key.clone()).or_default().push_back((epoch, object, bytes));
        self.order.push_back((epoch, key));
        self.bytes += bytes;
    }
}
