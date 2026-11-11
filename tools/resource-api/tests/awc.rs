use glue5_resource_api::{awc::Bank, Resources, MAX_BYTES};
use serde_json::json;
use std::{fs, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};
static SERIAL: AtomicUsize = AtomicUsize::new(0);
fn put(bytes: &mut [u8], at: usize, n: u64, count: usize, be: bool) {
    for i in 0..count { bytes[at + i] = (n >> (8 * if be { count - i - 1 } else { i })) as u8; }
}
fn fixture(codec: u8, samples: u32, pcm: &[u8], be: bool, index: bool) -> Vec<u8> {
    let ids = 16 + if index { 2 } else { 0 };
    let mut data = vec![0; 68 + pcm.len()];
    data[..4].copy_from_slice(if be { b"TADA" } else { b"ADAT" });
    put(&mut data, 4, 0xff020001 | if index { 0x10000 } else { 0 }, 4, be);
    put(&mut data, 8, 1, 4, be);
    put(&mut data, 12, 48, 4, be);
    put(&mut data, ids, (2u64 << 29) | 123, 4, be);
    put(&mut data, ids + 4, (0xfau64 << 56) | (20 << 28) | 48, 8, be);
    put(&mut data, ids + 12, (0x55u64 << 56) | ((pcm.len() as u64) << 28) | 68, 8, be);
    put(&mut data, 48, samples as u64, 4, be);
    put(&mut data, 56, 48000, 2, be);
    data[67] = codec;
    data[68..].copy_from_slice(pcm);
    data
}
fn samples(wav: &[u8]) -> Vec<i16> {
    wav[44..].chunks_exact(2).map(|p| i16::from_le_bytes([p[0],p[1]])).collect()
}
#[test]
fn pcm_bank_and_wav_sizes_endianness_and_optional_index() {
    for be in [false,true] { for index in [false,true] {
        let raw: Vec<u8> = [-32768i16,-1,0,32767].iter().flat_map(|n| if be {n.to_be_bytes()}else{n.to_le_bytes()}).collect();
        let bytes = fixture(1,4,&raw,be,index);
        let bank = Bank::open(&bytes).unwrap();
        assert_eq!(bank.streams[0].id,123);
        let wav = bank.wav(123).unwrap();
        assert_eq!(&wav[..4],b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()),44);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()),8);
        assert_eq!(samples(&wav),[-32768,-1,0,32767]);
        assert!(bank.wav(124).is_err());
    } }
}
#[test]
fn ima_uses_inverted_header_low_nibble_first_and_no_extra_predictor() {
    for be in [false,true] {
        let bytes = fixture(4,4,&[0,0,0xe8,3,0x71,0x08],be,false);
        let wav = Bank::open(&bytes).unwrap().wav(123).unwrap();
        assert_eq!(samples(&wav),[1001,1012,1010,1011]);
    }
}
#[test]
fn ima_resets_state_at_2048_byte_boundary_and_clamps_pcm() {
    let mut raw = vec![0;2048];
    raw.extend([0,0,0xd2,4,0]);
    let bytes = fixture(4,4089,&raw,false,false);
    let wav = Bank::open(&bytes).unwrap().wav(123).unwrap();
    let pcm = samples(&wav);
    assert!(pcm[..4088].iter().all(|&n| n==0));
    assert_eq!(pcm[4088],1234);
    let bytes = fixture(4,2,&[88,0,0xff,0x7f,0xf7],false,false);
    assert_eq!(samples(&Bank::open(&bytes).unwrap().wav(123).unwrap()),[32767,-28669]);
}
#[test]
fn malformed_banks_and_samples_are_errors_not_partial_audio() {
    let bytes = fixture(1,2,&[0,0,1,0],false,true);
    for end in 0..bytes.len() { assert!(Bank::open(&bytes[..end]).is_err(),"accepted truncation {end}"); }
    for (at,n,size) in [(12,1,4),(8,5000,4),(16,1,2),(22,0,8)] {
        let mut bad = bytes.clone(); put(&mut bad,at,n,size,false);
        assert!(Bank::open(&bad).is_err());
    }
    for (codec,count,payload) in [(4,1,vec![89,0,0,0,0]),(4,2,vec![0,0,0,0]),(1,3,vec![0,0]),(1,(MAX_BYTES/2) as u32,vec![0,0]),(12,1,vec![0,0])] {
        let bad = fixture(codec,count,&payload,false,false);
        assert!(Bank::open(&bad).unwrap().wav(123).is_err());
    }
    for flag in [0x40000,0x80000] {
        let mut bad = fixture(1,1,&[0,0],false,false);
        put(&mut bad,4,0xff020001 | flag,4,false);
        assert!(Bank::open(&bad).unwrap().wav(123).is_err());
    }
}
struct Temp(PathBuf);
impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
#[test]
fn audio_api_is_explicit_paginated_and_keeps_originals_and_provenance() {
    let ticks = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let temp = Temp(std::env::temp_dir().join(format!("glue5-awc-{ticks}-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed))));
    fs::create_dir_all(temp.0.join("source")).unwrap();
    fs::create_dir(temp.0.join("cache")).unwrap();
    let bytes = fixture(0,2,&[1,0,2,0],false,false);
    fs::write(temp.0.join("source/bank.awc"),&bytes).unwrap();
    let api = Resources::open(&temp.0.join("source"),&temp.0.join("cache"),None).unwrap();
    assert_eq!(api.audio_list("bank.awc",0,1).unwrap()["streams"][0]["id"],123);
    assert_eq!(api.audio_list("bank.awc",1,1).unwrap()["streams"],json!([]));
    for id in [json!(null),json!(-1),json!(0x20000000u32),json!("123")] {
        let result = api.dispatch(json!({"jsonrpc":"2.0","id":1,"method":"audio.extract","params":{"uri":"bank.awc","stream_id":id}}));
        assert_eq!(result["error"]["code"],-32602);
    }
    assert_eq!(fs::read_dir(temp.0.join("cache")).unwrap().count(),0);
    let result = api.audio_extract("bank.awc",123).unwrap();
    let bundle = temp.0.join("cache").join(result["bundle"].as_str().unwrap());
    assert_eq!(samples(&fs::read(bundle.join("audio.wav")).unwrap()),[1,2]);
    assert_eq!(result["receipt"]["selection"]["stream_id"],123);
    assert_eq!(result["receipt"]["selection"]["source_sha256"].as_str().unwrap().len(),64);
    assert_eq!(fs::read(temp.0.join("source/bank.awc")).unwrap(),bytes);
}
