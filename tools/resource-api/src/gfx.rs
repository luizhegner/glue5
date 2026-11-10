//! Bounded Scaleform/SWF framing, tag inventory and embedded lossless bitmaps.
//! Format reference: https://github.com/ruffle-rs/ruffle/blob/master/swf/src/read.rs
//! This does NOT execute ActionScript or render movies/fonts/vector shapes.
use crate::{MAX_BYTES, Result};
use flate2::{Decompress, FlushDecompress, Status, Compression, write::ZlibEncoder};
use serde::Serialize;
use std::io::Write;

fn slice(data: &[u8], at: usize, size: usize) -> Result<&[u8]> {
    data.get(at..at.checked_add(size).ok_or("movie range overflow")?).ok_or_else(|| "truncated movie".into())
}
fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(slice(data, at, 2)?.try_into().unwrap()))
}
fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(slice(data, at, 4)?.try_into().unwrap()))
}
fn inflate(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    if expected as u64 > MAX_BYTES { return Err("movie expansion exceeds budget".into()); }
    let mut decoded = Vec::with_capacity(expected + 1);
    let mut state = Decompress::new(true);
    let status = state.decompress_vec(data, &mut decoded, FlushDecompress::Finish).map_err(|e|e.to_string())?;
    if status != Status::StreamEnd || state.total_in() != data.len() as u64 || decoded.len() != expected {
        return Err("invalid/truncated zlib stream or declared size mismatch".into());
    }
    Ok(decoded)
}
#[derive(Debug, Serialize)]
pub struct Tag {
    pub code: u16,
    /// Offset in the decompressed movie body (after its 8-byte file header).
    pub offset: usize,
    pub bytes: usize,
    pub depth: usize,
    pub character_id: Option<u16>,
}
pub struct Movie {
    pub version: u8,
    pub scaleform: bool,
    pub frame_rate_fixed8: u16,
    pub frames: u16,
    pub tags: Vec<Tag>,
    body: Vec<u8>,
}
impl Movie {
    pub fn open(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 || bytes.len() as u64 > MAX_BYTES { return Err("movie input outside byte budget".into()); }
        let (scaleform, compressed) = match &bytes[..3] {
            b"GFX" => (true,false), b"CFX" => (true,true),
            b"FWS" => (false,false), b"CWS" => (false,true),
            _ => return Err("unsupported movie signature (LZMA is not implemented)".into()),
        };
        let declared = u32_at(bytes,4)? as usize;
        if declared < 8 || declared as u64 > MAX_BYTES { return Err("invalid/excessive movie length".into()); }
        let body = if compressed { inflate(&bytes[8..],declared-8)? } else {
            if bytes.len() != declared { return Err("movie size differs from header".into()); }
            bytes[8..].to_vec()
        };
        let first = *body.first().ok_or("missing movie rectangle")?;
        let rect_bytes = (5 + 4 * (first as usize >> 3)).div_ceil(8);
        let frame_rate_fixed8 = u16_at(&body,rect_bytes)?;
        let frames = u16_at(&body,rect_bytes+2)?;
        let mut tags = Vec::new();
        walk(&body,rect_bytes+4,body.len(),0,&mut tags)?;
        Ok(Self { version:bytes[3],scaleform,frame_rate_fixed8,frames,tags,body })
    }
    /// Decode one embedded DefineBitsLossless(2) bitmap, not external GFX images.
    pub fn bitmap(&self, id: u16) -> Result<Bitmap> {
        let mut found = self.tags.iter().filter(|t| matches!(t.code,20|36) && t.character_id == Some(id));
        let tag = found.next().ok_or("embedded lossless bitmap ID not found")?;
        if found.next().is_some() { return Err("ambiguous duplicate bitmap ID".into()); }
        let data = slice(&self.body,tag.offset,tag.bytes)?;
        let format = *data.get(2).ok_or("missing bitmap format")?;
        let width = u16_at(data,3)? as usize;
        let height = u16_at(data,5)? as usize;
        let pixels = width.checked_mul(height).ok_or("bitmap dimension overflow")?;
        if pixels == 0 || pixels as u64 * 4 + height as u64 > MAX_BYTES {
            return Err("bitmap exceeds decoded pixel budget".into());
        }
        let alpha = tag.code == 36;
        // Only these layouts are implemented; do not interpret JPEG/YTD/external
        // image tags as raw pixels or invent transparent layers.
        if format != 5 { return Err("only lossless bitmap format 5 (XRGB/ARGB32) is implemented".into()); }
        let raw = inflate(slice(data,7,data.len()-7)?, pixels*4)?;
        let mut rgba = Vec::with_capacity(pixels*4);
        for p in raw.chunks_exact(4) {
            let a = if alpha { p[0] } else { 255 };
            let straight = |c: u8| -> u8 {
                if !alpha { c } else if a == 0 { 0 } else { ((c as u32 * 255 + a as u32/2) / a as u32).min(255) as u8 }
            };
            rgba.extend([straight(p[1]),straight(p[2]),straight(p[3]),a]);
        }
        Ok(Bitmap { width:width as u32,height:height as u32,rgba })
    }
}
fn walk(body: &[u8], mut at: usize, end: usize, depth: usize, tags: &mut Vec<Tag>) -> Result<()> {
    if depth > 16 { return Err("movie sprite nesting exceeds budget".into()); }
    while at < end {
        if tags.len() >= 100_000 { return Err("movie tag count exceeds budget".into()); }
        if end-at < 2 { return Err("truncated movie tag header".into()); }
        let word = u16_at(body,at)?;
        at += 2;
        let code = word >> 6;
        let mut len = (word & 63) as usize;
        if len == 63 {
            if end-at < 4 { return Err("truncated long movie tag".into()); }
            len = u32_at(body,at)? as usize; at += 4;
        }
        let stop = at.checked_add(len).ok_or("movie tag overflow")?;
        if stop > end { return Err("movie tag escapes its container".into()); }
        if code == 0 {
            if len != 0 || body[stop..end].iter().any(|&b| b != 0) { return Err("invalid End tag/trailing movie bytes".into()); }
            return Ok(());
        }
        let character_id = if matches!(code,10|20|36|39|48|75|91) {
            if len < 2 { return Err("missing movie character ID".into()); }
            Some(u16_at(body,at)?)
        } else { None };
        tags.push(Tag { code,offset:at,bytes:len,depth,character_id });
        if code == 39 {
            if len < 4 { return Err("truncated DefineSprite".into()); }
            walk(body,at+4,stop,depth+1,tags)?;
        }
        at = stop;
    }
    Err("movie container has no End tag".into())
}
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8, suitable for PNG and egui's image loader.
    pub rgba: Vec<u8>,
}
impl Bitmap {
    pub fn png(&self) -> Result<Vec<u8>> {
        let stride = self.width as u64 * 4;
        let size = (stride+1).checked_mul(self.height as u64).ok_or("PNG dimensions overflow")?;
        if self.width == 0 || self.height == 0 || size > MAX_BYTES
            || stride * self.height as u64 != self.rgba.len() as u64 {
            return Err("invalid/excessive PNG image".into());
        }
        let mut encoder = ZlibEncoder::new(Vec::new(),Compression::fast());
        for row in self.rgba.chunks_exact(stride as usize) {
            encoder.write_all(&[0]).map_err(|e|e.to_string())?;
            encoder.write_all(row).map_err(|e|e.to_string())?;
        }
        let compressed = encoder.finish().map_err(|e|e.to_string())?;
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend(self.width.to_be_bytes()); header.extend(self.height.to_be_bytes());
        header.extend([8,6,0,0,0]); // RGBA8, no interlace.
        chunk(&mut png,b"IHDR",&header);
        chunk(&mut png,b"IDAT",&compressed);
        chunk(&mut png,b"IEND",&[]);
        if png.len() as u64 > MAX_BYTES { return Err("PNG output exceeds budget".into()); }
        Ok(png)
    }
}
fn chunk(out: &mut Vec<u8>, kind: &[u8;4], bytes: &[u8]) {
    out.extend((bytes.len() as u32).to_be_bytes()); out.extend(kind); out.extend(bytes);
    let mut table = [0u32;256];
    for (i,v) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 { c = (c >> 1) ^ if c & 1 != 0 {0xedb88320}else{0}; }
        *v = c;
    }
    let mut crc = !0u32;
    for &b in kind.iter().chain(bytes) { crc = table[((crc ^ b as u32) & 255) as usize] ^ (crc >> 8); }
    out.extend((!crc).to_be_bytes());
}
