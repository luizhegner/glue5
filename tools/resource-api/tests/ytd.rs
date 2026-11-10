use glue5_resource_api::{Resources, ytd::Dictionary};
use flate2::{Compression, write::DeflateEncoder, read::ZlibDecoder};
use serde_json::json;
use std::{fs, io::{Read, Write}, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};

const SYSTEM: u64 = 0x50000000;
const GRAPHICS: u64 = 0x60000000;
const TEXTURE: usize = 0x80;
fn put16(b: &mut [u8], at: usize, v: u16) { b[at..at+2].copy_from_slice(&v.to_le_bytes()); }
fn put32(b: &mut [u8], at: usize, v: u32) { b[at..at+4].copy_from_slice(&v.to_le_bytes()); }
fn put64(b: &mut [u8], at: usize, v: u64) { b[at..at+8].copy_from_slice(&v.to_le_bytes()); }
struct Fixture { system: Vec<u8>, graphics: Vec<u8> }
impl Fixture {
    fn new(w: u16, h: u16, format: u32, stride: u16, pixels: &[u8]) -> Self {
        let mut system = vec![0;1024];
        put64(&mut system,0x20,SYSTEM+0x40); put16(&mut system,0x28,1); put16(&mut system,0x2a,1);
        put64(&mut system,0x30,SYSTEM+0x48); put16(&mut system,0x38,1); put16(&mut system,0x3a,1);
        put32(&mut system,0x40,0x12345678); put64(&mut system,0x48,SYSTEM+TEXTURE as u64);
        put64(&mut system,TEXTURE+0x28,SYSTEM+0x120);
        system[0x120..0x12a].copy_from_slice(b"synthetic\0");
        for (at,v) in [(0x50,w),(0x52,h),(0x54,1),(0x56,stride)] { put16(&mut system,TEXTURE+at,v); }
        put32(&mut system,TEXTURE+0x58,format); system[TEXTURE+0x5d] = 1;
        put64(&mut system,TEXTURE+0x70,GRAPHICS);
        let mut graphics = vec![0;512]; graphics[..pixels.len()].copy_from_slice(pixels);
        Self { system,graphics }
    }
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = b"RSC7".to_vec();
        for word in [13u32,1<<26,1<<27] { bytes.extend(word.to_le_bytes()); }
        let mut z = DeflateEncoder::new(Vec::new(),Compression::default());
        z.write_all(&self.system).unwrap(); z.write_all(&self.graphics).unwrap();
        bytes.extend(z.finish().unwrap()); bytes
    }
    fn dictionary(&self) -> Dictionary { Dictionary::open(&self.bytes()).unwrap() }
}
fn color(a: u16, b: u16, indices: u32) -> Vec<u8> {
    let mut out = a.to_le_bytes().to_vec(); out.extend(b.to_le_bytes()); out.extend(indices.to_le_bytes()); out
}
fn rgba_png(png: &[u8]) -> Vec<u8> {
    assert_eq!(&png[..8],b"\x89PNG\r\n\x1a\n");
    let mut encoded = Vec::new(); let mut at = 8;
    while at < png.len() {
        let n = u32::from_be_bytes(png[at..at+4].try_into().unwrap()) as usize;
        if &png[at+4..at+8] == b"IDAT" { encoded.extend_from_slice(&png[at+8..at+8+n]); }
        at += n+12;
    }
    let mut decoded = Vec::new(); ZlibDecoder::new(encoded.as_slice()).read_to_end(&mut decoded).unwrap(); decoded
}

#[test]
fn bgra_inventory_row_padding_and_png_preserve_channels() {
    let f = Fixture::new(2,2,21,12,&[3,2,1,4,30,20,10,40,99,99,99,99, 7,6,5,8,70,60,50,80,88,88,88,88]);
    let d = f.dictionary(); let t = &d.textures[0];
    assert_eq!(t.name,"synthetic"); assert_eq!(t.name_hash,0x12345678);
    assert_eq!(t.format,"BGRA8"); assert!(t.decode_supported);
    let b = d.bitmap(0).unwrap(); assert_eq!(b.rgba,[1,2,3,4,10,20,30,40,5,6,7,8,50,60,70,80]);
    assert_eq!(rgba_png(&b.png().unwrap()),[0,1,2,3,4,10,20,30,40,0,5,6,7,8,50,60,70,80]);
    assert!(d.bitmap(1).is_err());
}
#[test]
fn plain_formats_explicitly_distinguish_alpha_luminance_and_unused_x() {
    for (format,pixels,expected,stride) in [
        (22,vec![3,2,1,0],vec![1,2,3,255],4), (32,vec![1,2,3,4],vec![1,2,3,4],4),
        (28,vec![128],vec![255,255,255,128],1), (50,vec![64],vec![64,64,64,255],1),
    ] { assert_eq!(Fixture::new(1,1,format,stride,&pixels).dictionary().bitmap(0).unwrap().rgba,expected); }
}
#[test]
fn bc1_palette_transparency_and_block_edge_clipping() {
    let four = color(0xf800,0x07e0,0xe4e4e4e4);
    let b = Fixture::new(4,4,u32::from_le_bytes(*b"DXT1"),2,&four).dictionary().bitmap(0).unwrap();
    assert_eq!(&b.rgba[..16],&[255,0,0,255,0,255,0,255,170,85,0,255,85,170,0,255]);
    let transparent = color(0,0xffff,u32::MAX);
    assert_eq!(Fixture::new(4,4,u32::from_le_bytes(*b"DXT1"),8,&transparent).dictionary().bitmap(0).unwrap().rgba,vec![0;64]);
    let mut blocks = color(0xf800,0,0); blocks.extend(color(0x07e0,0,0));
    blocks.extend(color(0x001f,0,0)); blocks.extend(color(0xffff,0,0));
    for stride in [4,16] {
        let b = Fixture::new(5,5,u32::from_le_bytes(*b"DXT1"),stride,&blocks).dictionary().bitmap(0).unwrap();
        assert_eq!(b.rgba.len(),100);
        for (x,y,p) in [(0,0,[255,0,0,255]),(4,0,[0,255,0,255]),(0,4,[0,0,255,255]),(4,4,[255;4])] {
            assert_eq!(&b.rgba[(y*5+x)*4..(y*5+x+1)*4],&p);
        }
    }
}
#[test]
fn bc2_uses_four_color_mode_and_explicit_four_bit_alpha() {
    let alpha = (0..16).fold(0u64,|bits,i| bits|((i as u64)<<(i*4)));
    let mut block = alpha.to_le_bytes().to_vec(); block.extend(color(0,0xffff,u32::MAX));
    let b = Fixture::new(4,4,u32::from_le_bytes(*b"DXT3"),4,&block).dictionary().bitmap(0).unwrap();
    for (i,p) in b.rgba.chunks_exact(4).enumerate() { assert_eq!(p,[170,170,170,i as u8*17]); }
}
#[test]
fn bc3_supports_both_alpha_endpoint_orders() {
    for (a,b,expected) in [(200,10,[200,10,172,145,118,91,64,37]),(10,200,[10,200,48,86,124,162,0,255])] {
        let indices = (0..16).fold(0u64,|bits,i| bits|(((i%8) as u64)<<(i*3)));
        let mut block = vec![a,b]; block.extend(&indices.to_le_bytes()[..6]); block.extend(color(0xffff,0,0));
        let bitmap = Fixture::new(4,4,u32::from_le_bytes(*b"DXT5"),16,&block).dictionary().bitmap(0).unwrap();
        for (i,p) in bitmap.rgba.chunks_exact(4).enumerate() { assert_eq!(p,[255,255,255,expected[i%8]]); }
    }
}
#[test]
fn malformed_dictionary_counts_names_pointers_and_other_versions_are_refused() {
    let mut f = Fixture::new(1,1,21,4,&[1;4]);
    put16(&mut f.system,0x28,2); assert!(Dictionary::open(&f.bytes()).is_err());
    put16(&mut f.system,0x28,1); put16(&mut f.system,0x2a,5000); assert!(Dictionary::open(&f.bytes()).is_err());
    put16(&mut f.system,0x2a,1); put64(&mut f.system,0x20,GRAPHICS); assert!(Dictionary::open(&f.bytes()).is_err());
    put64(&mut f.system,0x20,SYSTEM+0x40); put64(&mut f.system,0x48,SYSTEM+1020); assert!(Dictionary::open(&f.bytes()).is_err());
    put64(&mut f.system,0x48,SYSTEM+TEXTURE as u64); f.system[0x120..0x220].fill(b'A'); assert!(Dictionary::open(&f.bytes()).is_err());
    let mut bytes = Fixture::new(1,1,21,4,&[1;4]).bytes();
    bytes[4..8].copy_from_slice(&5u32.to_le_bytes()); assert!(Dictionary::open(&bytes).is_err());
    for end in [0,3,15,bytes.len()-1] { assert!(Dictionary::open(&bytes[..end]).is_err()); }
}
#[test]
fn unsupported_textures_remain_visible_but_cannot_export_guessed_pixels() {
    for (field,value,wide) in [(0x58,u32::from_le_bytes(*b"BC7 "),true),(0x54,6,false),(0x56,1,false),
        (0x50,0,false),(0x5c,1,false),(0x5d,0,false),(0x5d,99,false),(0x50,65535,false)] {
        let mut f = Fixture::new(4,4,21,16,&[1;64]);
        if wide { put32(&mut f.system,TEXTURE+field,value); }
        else if matches!(field,0x5c|0x5d) { f.system[TEXTURE+field] = value as u8; }
        else { put16(&mut f.system,TEXTURE+field,value as u16); }
        let d = f.dictionary(); assert!(!d.textures[0].decode_supported); assert!(d.textures[0].unsupported_reason.is_some());
        assert!(d.bitmap(0).is_err());
    }
    for pointer in [0,SYSTEM,GRAPHICS+510] {
        let mut f = Fixture::new(1,1,21,4,&[1;4]); put64(&mut f.system,TEXTURE+0x70,pointer);
        let d = f.dictionary(); assert!(!d.textures[0].decode_supported); assert!(d.bitmap(0).is_err());
    }
    let f = Fixture::new(4,4,u32::from_le_bytes(*b"DXT1"),1,&[1;8]);
    assert!(!f.dictionary().textures[0].decode_supported);
}
struct Temp(PathBuf);
impl Temp { fn new() -> Self {
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let ticks = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("glue5-ytd-{ticks}-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
    fs::create_dir_all(path.join("source")).unwrap(); fs::create_dir(path.join("cache")).unwrap(); Self(path)
} }
impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
#[test]
fn protocol_pagination_receipts_and_source_integrity() {
    let t = Temp::new(); let path = t.0.join("source/test.ytd");
    let bytes = Fixture::new(1,1,21,4,&[3,2,1,4]).bytes(); fs::write(&path,&bytes).unwrap();
    let api = Resources::open(&t.0.join("source"),&t.0.join("cache"),None).unwrap();
    let listing = api.dispatch(json!({"jsonrpc":"2.0","id":1,"method":"textures.list","params":{"uri":"test.ytd","limit":1}}));
    assert_eq!(listing["result"]["textures"][0]["name"],"synthetic");
    assert_eq!(listing["result"]["total"],1); assert_eq!(listing["result"]["next_offset"],json!(null));
    assert_eq!(api.textures_list("test.ytd",1,1).unwrap()["textures"],json!([]));
    assert!(api.textures_list("test.ytd",0,513).is_err());
    for params in [json!({}),json!({"uri":"test.ytd"}),json!({"uri":"test.ytd","texture_index":-1}),json!({"uri":"test.ytd","texture_index":"0"})] {
        let r = api.dispatch(json!({"jsonrpc":"2.0","id":2,"method":"textures.extract","params":params}));
        assert_eq!(r["error"]["code"],-32602);
    }
    assert!(api.textures_extract("test.ytd",1).is_err());
    assert_eq!(fs::read_dir(t.0.join("cache")).unwrap().count(),0);
    let r = api.dispatch(json!({"jsonrpc":"2.0","id":3,"method":"textures.extract","params":{"uri":"test.ytd","texture_index":0}}));
    let receipt = &r["result"]["receipt"];
    assert_eq!(receipt["selection"]["mip_level"],0); assert_eq!(receipt["selection"]["width"],1);
    assert_eq!(receipt["selection"]["source_sha256"],listing["result"]["source_sha256"]);
    let bundle = t.0.join("cache").join(r["result"]["bundle"].as_str().unwrap());
    assert_eq!(rgba_png(&fs::read(bundle.join("image.png")).unwrap()),[0,1,2,3,4]);
    assert_eq!(fs::read(path).unwrap(),bytes);
    let capability = api.dispatch(json!({"jsonrpc":"2.0","id":4,"method":"api.capabilities"}));
    assert!(capability["result"]["methods"].as_array().unwrap().contains(&json!("textures.extract")));
}
