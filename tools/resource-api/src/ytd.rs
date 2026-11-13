//! Bounded legacy (RSC7 version 13, 64-bit PC layout) texture dictionaries.
//! Layout reference: CodeWalker.Core/GameFiles/Resources/Texture.cs.
//! BC1/2/3: Microsoft Direct3D block-compression specification. No GPU needed.
use crate::{MAX_BYTES, Result, gfx::Bitmap, rsc7::Resource};
use serde::Serialize;

const SYSTEM: u64 = 0x50000000;
const MAX_TEXTURES: usize = 4096;
const BC1: u32 = u32::from_le_bytes(*b"DXT1");
const BC2: u32 = u32::from_le_bytes(*b"DXT3");
const BC3: u32 = u32::from_le_bytes(*b"DXT5");
fn u16_at(b: &[u8], at: usize) -> u16 { u16::from_le_bytes(b[at..at+2].try_into().unwrap()) }
fn u32_at(b: &[u8], at: usize) -> u32 { u32::from_le_bytes(b[at..at+4].try_into().unwrap()) }
fn u64_at(b: &[u8], at: usize) -> u64 { u64::from_le_bytes(b[at..at+8].try_into().unwrap()) }
fn system(resource: &Resource, address: u64, len: usize) -> Result<&[u8]> {
    if !(SYSTEM..=0x5fffffff).contains(&address) { return Err("YTD object pointer outside system pages".into()); }
    resource.slice(address, len)
}
fn name(resource: &Resource, address: u64) -> Result<String> {
    let mut bytes = Vec::new();
    for i in 0..=255 {
        let b = system(resource, address.checked_add(i).ok_or("YTD name pointer overflow")?, 1)?[0];
        if b == 0 {
            let text = String::from_utf8(bytes).map_err(|_| "YTD texture name is not UTF-8")?;
            if text.is_empty() || text.chars().any(char::is_control) { return Err("invalid YTD texture name".into()); }
            return Ok(text);
        }
        bytes.push(b);
    }
    Err("YTD texture name exceeds 255 bytes".into())
}
fn list_header(root: &[u8], at: usize) -> Result<(u64, usize)> {
    let count = u16_at(root, at+8) as usize;
    let capacity = u16_at(root, at+10) as usize;
    if count > capacity || capacity > MAX_TEXTURES { return Err("invalid/excessive YTD list count or capacity".into()); }
    Ok((u64_at(root, at), count))
}
fn format_name(code: u32) -> &'static str {
    match code { 21 => "BGRA8", 22 => "BGRX8", 32 => "RGBA8", 28 => "A8", 50 => "L8",
        BC1 => "BC1/DXT1", BC2 => "BC2/DXT3", BC3 => "BC3/DXT5", _ => "unsupported" }
}

#[derive(Debug, Serialize)]
pub struct Texture {
    pub index: usize,
    pub name: String,
    pub name_hash: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub stride: u16,
    pub mip_levels: u8,
    pub format_code: u32,
    pub format: &'static str,
    pub decode_supported: bool,
    pub unsupported_reason: Option<String>,
    #[serde(skip)] data_address: u64,
    #[serde(skip)] flags: u8,
}
impl Texture {
    /// Base mip only. Unknown pitches/dimensions are refused, never guessed.
    fn layout(&self) -> Result<(usize, usize, usize)> {
        let (w,h) = (self.width as usize,self.height as usize);
        if w == 0 || h == 0 || self.depth != 1 || self.flags != 0 { return Err("only ordinary 2D legacy textures are supported".into()); }
        if self.mip_levels == 0 || self.mip_levels as u32 > 1 + w.max(h).ilog2() {
            return Err("invalid YTD mip count".into());
        }
        if (w as u64 * 4 + 1) * h as u64 > MAX_BYTES { return Err("YTD decoded image exceeds byte budget".into()); }
        if !(0x60000000..=0x6fffffff).contains(&self.data_address) { return Err("YTD pixels must be in graphics pages".into()); }
        let block = match self.format_code { BC1 => 8, BC2|BC3 => 16, _ => 0 };
        if block != 0 {
            let row = w.div_ceil(4) * block;
            // Legacy writers store either a block-row pitch or its nominal
            // per-pixel-row equivalent. Both describe the same packed blocks.
            if self.stride as usize != row && self.stride as usize != row/4 { return Err("unsupported YTD compressed pitch".into()); }
            return Ok((row * h.div_ceil(4),row,block));
        }
        let components = match self.format_code { 21|22|32 => 4, 28|50 => 1, _ => return Err("unsupported YTD pixel format".into()) };
        let row = self.stride as usize;
        if row < w * components { return Err("YTD row pitch is shorter than its pixels".into()); }
        Ok((row*h,row,0))
    }
}

pub struct Dictionary {
    resource: Resource,
    pub textures: Vec<Texture>,
}
impl Dictionary {
    pub fn open(bytes: &[u8]) -> Result<Self> {
        let resource = Resource::open(bytes)?;
        if resource.version != 13 { return Err("only legacy YTD version 13 is supported (no Gen9/console auto-detection)".into()); }
        let root = system(&resource, SYSTEM, 64)?;
        let (hash_ptr, count) = list_header(root, 0x20)?;
        let (texture_ptr, texture_count) = list_header(root, 0x30)?;
        if count != texture_count { return Err("YTD hash and texture counts disagree".into()); }
        let mut textures = Vec::with_capacity(count);
        if count != 0 {
            let hashes = system(&resource, hash_ptr, count*4)?;
            let pointers = system(&resource, texture_ptr, count*8)?;
            for index in 0..count {
                let data = system(&resource, u64_at(pointers,index*8), 0x90)?;
                let format_code = u32_at(data,0x58);
                let mut texture = Texture { index, name:name(&resource,u64_at(data,0x28))?,
                    name_hash:u32_at(hashes,index*4), width:u16_at(data,0x50),height:u16_at(data,0x52),
                    depth:u16_at(data,0x54),stride:u16_at(data,0x56),format_code,format:format_name(format_code),
                    mip_levels:data[0x5d],flags:data[0x5c],data_address:u64_at(data,0x70),
                    decode_supported:false,unsupported_reason:None };
                texture.unsupported_reason = texture.layout().and_then(|(size,_,_)| resource.slice(texture.data_address,size).map(|_| ())).err();
                texture.decode_supported = texture.unsupported_reason.is_none();
                textures.push(texture);
            }
        }
        Ok(Self { resource,textures })
    }

    /// Decode a selected texture's base mip. No alpha/gamma guessing or resize.
    pub fn bitmap(&self, index: usize) -> Result<Bitmap> {
        let t = self.textures.get(index).ok_or("YTD texture index not found")?;
        let (size,row,block) = t.layout()?;
        let pixels = self.resource.slice(t.data_address,size)?;
        let (w,h) = (t.width as usize,t.height as usize);
        let mut rgba = vec![0;w*h*4];
        if block == 0 {
            for y in 0..h { for x in 0..w {
                let source = y*row + x*if matches!(t.format_code,28|50) {1}else{4};
                let p = match t.format_code {
                    21 => [pixels[source+2],pixels[source+1],pixels[source],pixels[source+3]],
                    22 => [pixels[source+2],pixels[source+1],pixels[source],255],
                    32 => pixels[source..source+4].try_into().unwrap(),
                    28 => [255,255,255,pixels[source]],
                    50 => [pixels[source],pixels[source],pixels[source],255],
                    _ => unreachable!(), // layout checked the format before allocation.
                };
                rgba[(y*w+x)*4..(y*w+x+1)*4].copy_from_slice(&p);
            } }
        } else {
            for by in 0..h.div_ceil(4) { for bx in 0..w.div_ceil(4) {
                let at = by*row + bx*block;
                let tile = decode_block(&pixels[at..at+block],t.format_code);
                for y in 0..4 { for x in 0..4 {
                    let (dx,dy) = (bx*4+x,by*4+y);
                    if dx < w && dy < h { rgba[(dy*w+dx)*4..(dy*w+dx+1)*4].copy_from_slice(&tile[y*4+x]); }
                } }
            } }
        }
        Ok(Bitmap { width:w as u32,height:h as u32,rgba })
    }
}

fn rgb565(value: u16) -> [u8;4] {
    let (r,g,b) = ((value>>11) as u8,((value>>5)&63) as u8,(value&31) as u8);
    [(r<<3)|(r>>2),(g<<2)|(g>>4),(b<<3)|(b>>2),255]
}
fn decode_block(block: &[u8], format: u32) -> [[u8;4];16] {
    let color = if format == BC1 { block } else { &block[8..] };
    let (a,b) = (u16_at(color,0),u16_at(color,2));
    let mut palette = [rgb565(a),rgb565(b),[0,0,0,255],[0,0,0,255]];
    for c in 0..3 {
        if a > b || format != BC1 {
            palette[2][c] = ((2*palette[0][c] as u16 + palette[1][c] as u16)/3) as u8;
            palette[3][c] = ((palette[0][c] as u16 + 2*palette[1][c] as u16)/3) as u8;
        } else { palette[2][c] = ((palette[0][c] as u16+palette[1][c] as u16)/2) as u8; }
    }
    if format == BC1 && a <= b { palette[3] = [0;4]; }
    let indices = u32_at(color,4);
    let mut out = [[0;4];16];
    for i in 0..16 { out[i] = palette[((indices>>(i*2))&3) as usize]; }
    if format == BC2 {
        let alpha = u64_at(block,0);
        for i in 0..16 { out[i][3] = ((alpha>>(i*4))&15) as u8*17; }
    } else if format == BC3 {
        let (a,b) = (block[0] as u16,block[1] as u16);
        let mut palette = [a as u8,b as u8,0,0,0,0,0,255];
        let steps = if a > b {7}else{5};
        for i in 1..steps { palette[i+1] = (((steps-i) as u16*a+i as u16*b)/steps as u16) as u8; }
        let mut packed = [0;8]; packed[..6].copy_from_slice(&block[2..8]);
        let indices = u64::from_le_bytes(packed);
        for i in 0..16 { out[i][3] = palette[((indices>>(i*3))&7) as usize]; }
    }
    out
}

