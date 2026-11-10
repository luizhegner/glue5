use glue5_resource_api::{gfx::{Movie, Bitmap}, MAX_BYTES};
use flate2::{Compression, write::ZlibEncoder, read::ZlibDecoder};
use std::io::{Read,Write};
fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut z = ZlibEncoder::new(Vec::new(),Compression::default()); z.write_all(bytes).unwrap(); z.finish().unwrap()
}
fn tag(code:u16, data:&[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if data.len()<63 { out.extend(((code<<6)|data.len() as u16).to_le_bytes()); }
    else { out.extend(((code<<6)|63).to_le_bytes());out.extend((data.len() as u32).to_le_bytes()); }
    out.extend(data);out
}
fn movie(tags:&[u8], compressed:bool, scaleform:bool) -> Vec<u8> {
    let mut body = vec![0,0,30,1,0]; // RECT, 30fps fixed8, one frame.
    body.extend(tags);body.extend([0,0]); // End.
    let mut out = if scaleform {b"GFX".to_vec()}else{b"FWS".to_vec()};
    if compressed {out[0]=b'C';}
    out.push(10);out.extend((body.len() as u32 + 8).to_le_bytes());
    out.extend(if compressed {zlib(&body)}else{body});out
}
fn image(width:u16,height:u16,raw:&[u8],alpha:bool) -> Vec<u8> {
    let mut data=17u16.to_le_bytes().to_vec();data.push(5);
    data.extend(width.to_le_bytes());data.extend(height.to_le_bytes());data.extend(zlib(raw));
    tag(if alpha {36}else{20},&data)
}
#[test]
fn compressed_and_plain_scaleform_swf_preserve_metadata_and_rgb() {
    for compressed in [false,true] { for scaleform in [false,true] {
        let bytes=movie(&image(2,1,&[0,1,2,3,99,250,251,252],false),compressed,scaleform);
        let m=Movie::open(&bytes).unwrap();
        assert_eq!(m.scaleform,scaleform);assert_eq!(m.version,10);
        assert_eq!(m.frames,1);assert_eq!(m.frame_rate_fixed8,30*256);
        assert_eq!(m.tags.len(),1);assert_eq!(m.tags[0].character_id,Some(17));
        assert_eq!(m.bitmap(17).unwrap().rgba,[1,2,3,255,250,251,252,255]);
        assert!(m.bitmap(18).is_err());
    } }
}
#[test]
fn argb_is_unpremultiplied_and_transparent_rgb_is_zero() {
    let bytes=movie(&image(3,1,&[128,64,32,0,0,88,44,22,255,1,2,3],true),true,true);
    assert_eq!(Movie::open(&bytes).unwrap().bitmap(17).unwrap().rgba,[128,64,0,128,0,0,0,0,1,2,3,255]);
}
#[test]
fn nested_sprites_long_tags_and_unknown_tags_are_bounded_not_executed() {
    let mut sprite=vec![2,0,1,0];
    sprite.extend(tag(999,&[1;100]));
    sprite.extend(image(1,1,&[0,5,6,7],false));sprite.extend([0,0]);
    let bytes=movie(&tag(39,&sprite),true,true);
    let m=Movie::open(&bytes).unwrap();
    assert_eq!(m.tags.len(),3);
    assert_eq!(m.tags[1].code,999);assert_eq!(m.tags[1].depth,1);
    assert_eq!(m.bitmap(17).unwrap().rgba,[5,6,7,255]);
    let mut nested=tag(1,&[]);
    nested.extend([0,0]);
    for _ in 0..18 { let mut s=vec![1,0,1,0];s.extend(nested);nested=tag(39,&s);nested.extend([0,0]); }
    assert!(Movie::open(&movie(&nested,false,true)).is_err());
}
#[test]
fn malformed_lengths_zlib_truncations_and_bitmap_bombs_are_refused() {
    let original=movie(&image(1,1,&[0,1,2,3],false),true,true);
    for end in 0..original.len() { assert!(Movie::open(&original[..end]).is_err(),"accepted truncation {end}"); }
    let mut extra=original.clone();extra.push(0);assert!(Movie::open(&extra).is_err());
    let mut bad=original.clone();bad[4..8].copy_from_slice(&(MAX_BYTES as u32+1).to_le_bytes());assert!(Movie::open(&bad).is_err());
    let mut bad=movie(&[],false,true);let len=bad.len();bad[len-2]=64;assert!(Movie::open(&bad).is_err());
    for (w,h,raw) in [(65535,65535,vec![0;4]),(1,1,vec![0;3]),(0,1,vec![])] {
        let bytes=movie(&image(w,h,&raw,false),true,true);
        assert!(Movie::open(&bytes).unwrap().bitmap(17).is_err());
    }
    let mut tags=image(1,1,&[0,1,2,3],false);tags.extend(image(1,1,&[0,4,5,6],false));
    assert!(Movie::open(&movie(&tags,false,true)).unwrap().bitmap(17).is_err());
}
fn crc(bytes:&[u8])->u32 {
    let mut c=!0u32;
    for &b in bytes {c^=b as u32;for _ in 0..8 {if c&1!=0 {c=(c>>1)^0xedb88320;}else{c>>=1;}}}
    !c
}
#[test]
fn png_has_valid_chunks_crc_scanlines_and_transparency() {
    let bitmap=Bitmap{width:2,height:1,rgba:vec![255,0,0,255,0,64,128,128]};
    let png=bitmap.png().unwrap();assert_eq!(&png[..8],b"\x89PNG\r\n\x1a\n");
    let mut at=8;let mut pixels=Vec::new();let mut types=Vec::new();
    while at<png.len() {
        let len=u32::from_be_bytes(png[at..at+4].try_into().unwrap()) as usize;
        let kind=&png[at+4..at+8];let data=&png[at+8..at+8+len];
        assert_eq!(crc(&png[at+4..at+8+len]),u32::from_be_bytes(png[at+8+len..at+12+len].try_into().unwrap()));
        if kind==b"IHDR" {assert_eq!(data,&[0,0,0,2,0,0,0,1,8,6,0,0,0]);}
        if kind==b"IDAT" {ZlibDecoder::new(data).read_to_end(&mut pixels).unwrap();}
        types.push(kind.to_vec());at+=12+len;
    }
    assert_eq!(types,[b"IHDR".to_vec(),b"IDAT".to_vec(),b"IEND".to_vec()]);
    assert_eq!(pixels,[0,255,0,0,255,0,64,128,128]);
    assert_eq!(&png[png.len()-4..],&[0xae,0x42,0x60,0x82]);
    assert!(Bitmap{width:1,height:1,rgba:vec![]}.png().is_err());
}

#[test]
fn movie_protocol_requires_typed_selection_and_exports_with_provenance() {
    use glue5_resource_api::Resources;
    use serde_json::json;
    use std::fs;
    struct Temp(std::path::PathBuf);
    impl Drop for Temp { fn drop(&mut self) { let _=fs::remove_dir_all(&self.0); } }
    let ticks=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let temp=Temp(std::env::temp_dir().join(format!("glue5-gfx-{ticks}-{}",std::process::id())));
    fs::create_dir_all(temp.0.join("source")).unwrap();fs::create_dir(temp.0.join("cache")).unwrap();
    let bytes=movie(&image(1,1,&[255,10,20,30],true),true,true);
    fs::write(temp.0.join("source/test.gfx"),&bytes).unwrap();
    let api=Resources::open(&temp.0.join("source"),&temp.0.join("cache"),None).unwrap();
    let list=api.movie_list("test.gfx",0,1).unwrap();
    assert_eq!(list["total"],1);assert_eq!(list["tags"][0]["character_id"],17);
    assert_eq!(api.movie_list("test.gfx",1,1).unwrap()["tags"],json!([]));
    for id in [json!(null),json!(-1),json!(65536),json!("17")] {
        let result=api.dispatch(json!({"jsonrpc":"2.0","id":1,"method":"movie.extract_bitmap","params":{"uri":"test.gfx","character_id":id}}));
        assert_eq!(result["error"]["code"],-32602);
    }
    assert_eq!(fs::read_dir(temp.0.join("cache")).unwrap().count(),0);
    let result=api.dispatch(json!({"jsonrpc":"2.0","id":2,"method":"movie.extract_bitmap","params":{"uri":"test.gfx","character_id":17}}));
    assert!(result.get("error").is_none(),"{result}");
    let result=&result["result"];
    assert_eq!(result["receipt"]["selection"]["character_id"],17);
    assert_eq!(result["receipt"]["selection"]["width"],1);
    assert_eq!(result["receipt"]["selection"]["source_sha256"].as_str().unwrap().len(),64);
    let png=fs::read(temp.0.join("cache").join(result["bundle"].as_str().unwrap()).join("image.png")).unwrap();
    assert_eq!(&png[..8],b"\x89PNG\r\n\x1a\n");
    assert_eq!(fs::read(temp.0.join("source/test.gfx")).unwrap(),bytes);
}
