use glue5_resource_api::{rsc7::{Resource,page_bytes}, Archive, Resources, MAX_BYTES};
use flate2::{Compression,write::DeflateEncoder};
use std::{io::Write,fs};
use aes::{Aes256,cipher::{BlockEncrypt,KeyInit,generic_array::GenericArray}};
const FLAGS:u32=1<<27; // One 512-byte page.
fn envelope(system:u32, graphics:u32, bytes:&[u8]) -> Vec<u8> {
    let mut out=b"RSC7".to_vec();
    for n in [13u32,system,graphics] {out.extend(n.to_le_bytes());}
    let mut z=DeflateEncoder::new(Vec::new(),Compression::default());
    z.write_all(bytes).unwrap();out.extend(z.finish().unwrap());out
}
#[test]
fn all_page_count_fields_and_base_shifts_ignore_version_nibbles() {
    for shift in 0..=3 { for (bit,count,multiplier) in [(27,1,1),(26,1,2),(25,1,4),(24,1,8),(17,127,16),(11,63,32),(7,15,64),(5,3,128),(4,1,256)] {
        let flags=(count<<bit)|shift;
        assert_eq!(page_bytes(flags),(512u64<<shift)*count as u64*multiplier);
        assert_eq!(page_bytes(flags|0xf0000000),page_bytes(flags));
    } }
}
#[test]
fn resource_separates_system_graphics_and_checks_every_virtual_boundary() {
    let bytes:Vec<u8>=(0..1024).map(|i|(i/512+10) as u8).collect();
    let raw=envelope(FLAGS,FLAGS,&bytes);let r=Resource::open(&raw).unwrap();
    assert_eq!(r.version,13);assert_eq!(r.system(),vec![10;512]);assert_eq!(r.graphics(),vec![11;512]);
    assert_eq!(r.slice(0x500001ff,1).unwrap(),[10]);
    assert_eq!(r.slice(0x600001ff,1).unwrap(),[11]);
    for (address,len) in [(0,0),(0x500001ff,2),(0x600001ff,2),(0x70000000,1),(0x150000000,1),(0x50000001,usize::MAX)] {
        assert!(r.slice(address,len).is_err());
    }
}
#[test]
fn truncated_deflate_page_bombs_wrong_sizes_and_trailing_data_are_rejected() {
    let original=envelope(FLAGS,0,&[7;512]);
    for end in 0..original.len(){assert!(Resource::open(&original[..end]).is_err(),"truncation {end}");}
    let mut bad=original.clone();bad.push(0);assert!(Resource::open(&bad).is_err());
    let mut bad=original.clone();bad[8..12].copy_from_slice(&0x0800000fu32.to_le_bytes());assert!(Resource::open(&bad).is_err());
    let mut bomb=original.clone();bomb[8..12].copy_from_slice(&0x0000001fu32.to_le_bytes());assert!(Resource::open(&bomb).is_err());
    assert!(Resource::open(&envelope(FLAGS,0,&[8;1024])).is_err());
    assert!(Resource::open(&envelope(0,0,&[])).is_err());
}
fn archive(raw:&[u8], name:&str, aes:bool) -> Vec<u8> {
    let mut names=vec![0];names.extend(name.as_bytes());names.push(0);names.resize((names.len()+15)/16*16,0);
    let mut table=Vec::new();for n in [0u32,0x7fffff00,1,1]{table.extend(n.to_le_bytes());}
    let mut e=[0u8;16];e[0]=1;e[2..5].copy_from_slice(&(raw.len() as u32).to_le_bytes()[..3]);
    e[5]=1;e[7]=0x80;e[8..12].copy_from_slice(&FLAGS.to_le_bytes());e[12..16].copy_from_slice(&FLAGS.to_le_bytes());
    table.extend(e);table.extend(&names);
    if aes {let cipher=Aes256::new_from_slice(&[0x23;32]).unwrap();for b in table.chunks_exact_mut(16){cipher.encrypt_block(GenericArray::from_mut_slice(b));}}
    let mut out=b"7FPR".to_vec();for n in [2u32,names.len() as u32,if aes{0x0ffffff9}else{0x4e45504f}]{out.extend(n.to_le_bytes());}
    out.extend(table);out.resize(512,0);out.extend(raw);out
}
struct Temp(std::path::PathBuf);
impl Temp {fn new()->Self{
    let ticks=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let p=std::env::temp_dir().join(format!("glue5-rsc7-{ticks}-{}",std::process::id()));
    fs::create_dir_all(p.join("source")).unwrap();fs::create_dir(p.join("cache")).unwrap();Self(p)
}}
impl Drop for Temp {fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
#[test]
fn rpf_resource_keeps_envelope_does_not_apply_binary_cipher_flags_and_inspects() {
    for aes in [false,true] {
        let t=Temp::new();let raw=envelope(FLAGS,FLAGS,&[7;1024]);
        let path=t.0.join("source/textures.rpf");let original=archive(&raw,"sample.ytd",aes);
        fs::write(&path,&original).unwrap();
        let api=Resources::open(&t.0.join("source"),&t.0.join("cache"),Some([0x23;32])).unwrap();
        assert_eq!(api.read("textures.rpf::sample.ytd").unwrap(),raw);
        let result=api.rsc7_inspect("textures.rpf::sample.ytd").unwrap();
        assert_eq!(result["system_bytes"],512);assert_eq!(result["graphics_bytes"],512);
        assert_eq!(fs::read(&path).unwrap(),original);
        assert_eq!(fs::read_dir(t.0.join("cache")).unwrap().count(),0);
        let arc=Archive::open(fs::File::open(&path).unwrap(),Some(&[0x23;32])).unwrap();
        assert_eq!(arc.entries[0].decoded_bytes,1024);assert_eq!(arc.entries[0].system_flags,Some(FLAGS));
    }
}
#[test]
fn rpf_page_flag_disagreement_scripts_and_protocol_missing_uri_are_errors() {
    let t=Temp::new();let raw=envelope(FLAGS,FLAGS,&[7;1024]);
    let mut bad=archive(&raw,"sample.ytd",false);
    bad[40..44].copy_from_slice(&(1u32<<26).to_le_bytes()); // Resource TOC system flags.
    fs::write(t.0.join("source/bad.rpf"),&bad).unwrap();
    fs::write(t.0.join("source/script.rpf"),archive(&raw,"sample.ysc",false)).unwrap();
    let api=Resources::open(&t.0.join("source"),&t.0.join("cache"),None).unwrap();
    assert!(api.read("bad.rpf::sample.ytd").is_err());
    assert!(api.read("script.rpf::sample.ysc").is_err());
    let result=api.dispatch(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"rsc7.inspect","params":{}}));
    assert_eq!(result["error"]["code"],-32602);
    assert_eq!(MAX_BYTES,32*1024*1024);
}
