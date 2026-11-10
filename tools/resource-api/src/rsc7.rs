//! RSC7 raw-deflate envelope and bounded virtual system/graphics address spaces.
//! Page-flag reference: CodeWalker.Core/GameFiles/RpfFile.cs (CodeWalker).
//! Does not parse YTD objects, GPU texture compression, scripts or resource ciphers.
use crate::{MAX_BYTES, Result};
use flate2::{Decompress, FlushDecompress, Status};
pub fn page_bytes(flags: u32) -> u64 {
    let units = ((flags >> 27) & 1) as u64
        + (((flags >> 26) & 1) as u64 * 2)
        + (((flags >> 25) & 1) as u64 * 4)
        + (((flags >> 24) & 1) as u64 * 8)
        + (((flags >> 17) & 127) as u64 * 16)
        + (((flags >> 11) & 63) as u64 * 32)
        + (((flags >> 7) & 15) as u64 * 64)
        + (((flags >> 5) & 3) as u64 * 128)
        + (((flags >> 4) & 1) as u64 * 256);
    (512u64 << (flags & 15)) * units
}
fn word(bytes: &[u8], at: usize) -> u32 { u32::from_le_bytes(bytes[at..at+4].try_into().unwrap()) }
pub struct Resource {
    pub version: u32,
    pub system_flags: u32,
    pub graphics_flags: u32,
    pub system_bytes: usize,
    pub graphics_bytes: usize,
    bytes: Vec<u8>,
}
impl Resource {
    pub fn open(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 16 || bytes.len() as u64 > MAX_BYTES || &bytes[..4] != b"RSC7" {
            return Err("expected bounded RSC7 envelope".into());
        }
        let system_flags = word(bytes,8);
        let graphics_flags = word(bytes,12);
        let system = page_bytes(system_flags);
        let graphics = page_bytes(graphics_flags);
        let expected = system + graphics;
        if expected == 0 || expected > MAX_BYTES { return Err("RSC7 pages exceed decoded byte budget".into()); }
        let mut decoded = Vec::with_capacity(expected as usize + 1);
        let mut state = Decompress::new(false);
        let status = state.decompress_vec(&bytes[16..],&mut decoded,FlushDecompress::Finish).map_err(|e|e.to_string())?;
        if status != Status::StreamEnd || state.total_in() != (bytes.len()-16) as u64 || decoded.len() as u64 != expected {
            return Err("RSC7 deflate stream or declared page sizes disagree".into());
        }
        Ok(Self { version:word(bytes,4),system_flags,graphics_flags,system_bytes:system as usize,
            graphics_bytes:graphics as usize,bytes:decoded })
    }
    /// Resolve only exact virtual domains; never dereference a host pointer.
    pub fn slice(&self, address: u64, length: usize) -> Result<&[u8]> {
        let (base,size,offset) = match address {
            0x50000000..=0x5fffffff => (0x50000000,self.system_bytes,0),
            0x60000000..=0x6fffffff => (0x60000000,self.graphics_bytes,self.system_bytes),
            _ => return Err("RSC7 pointer outside system/graphics domains".into()),
        };
        let at = (address-base) as usize;
        let end = at.checked_add(length).ok_or("RSC7 pointer range overflow")?;
        if end > size { return Err("RSC7 pointer crosses page-space boundary".into()); }
        Ok(&self.bytes[offset+at..offset+end])
    }
    pub fn system(&self) -> &[u8] { &self.bytes[..self.system_bytes] }
    pub fn graphics(&self) -> &[u8] { &self.bytes[self.system_bytes..] }
}
