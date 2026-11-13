use glue5_resource_api::{awc::Bank, Resources};
use serde_json::json;

fn put(bytes: &mut [u8], at: usize, value: u64, n: usize, be: bool) {
    for i in 0..n { bytes[at+i]=(value >> (8*if be {n-1-i}else{i})) as u8; }
}
fn fixture(be: bool) -> Vec<u8> {
    // Three channels, two blocks. Sorted stream IDs deliberately do not
    // match the header channel order. Third channel is an independent stem.
    let mut bytes=vec![0;128+2*0x2000];
    bytes[..4].copy_from_slice(if be { b"TADA" } else { b"ADAT" });
    put(&mut bytes,4,0xff040001,4,be); put(&mut bytes,8,4,4,be);
    put(&mut bytes,12,48,4,be); put(&mut bytes,16,2<<29,4,be);
    for (i,id) in [1,2,3].into_iter().enumerate() { put(&mut bytes,20+i*4,id,4,be); }
    put(&mut bytes,32,(0x48u64<<56)|(60<<28)|48,8,be);
    put(&mut bytes,40,(0x55u64<<56)|((2*0x2000)<<28)|128,8,be);
    put(&mut bytes,48,2,4,be); put(&mut bytes,52,0x2000,4,be); put(&mut bytes,56,3,4,be);
    for (i,id) in [3,1,2].into_iter().enumerate() {
        let at=60+i*16;
        put(&mut bytes,at,id,4,be); put(&mut bytes,at+4,4,4,be);
        put(&mut bytes,at+10,48000,2,be); bytes[at+12]=4;
    }
    for block in 0..2 {
        let base=128+block*0x2000;
        for ch in 0..3 {
            put(&mut bytes,base+ch*0x18+4,1,4,be);
            put(&mut bytes,base+ch*0x18+12,2,4,be);
            // IMA frame header remains LE, including in TADA containers.
            let predictor=(1000+ch*100+block*10) as i16;
            bytes[base+0x800+ch*0x800+2..base+0x800+ch*0x800+4].copy_from_slice(&predictor.to_le_bytes());
        }
    }
    bytes
}
fn pcm(wav: &[u8]) -> Vec<i16> { wav[44..].chunks_exact(2).map(|b|i16::from_le_bytes([b[0],b[1]])).collect() }
#[test]
fn blocked_music_keeps_header_channel_order_and_does_not_mix_stems() {
    for be in [false,true] {
        let bytes=fixture(be); let bank=Bank::open(&bytes).unwrap();
        let music=bank.music().unwrap();
        assert_eq!(music.channels.iter().map(|c|c.stream_id).collect::<Vec<_>>(),[3,1,2]);
        let wav=bank.music_wav(&[1,0]).unwrap();
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()),2);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()),16);
        assert_eq!(pcm(&wav),[1100,1000,1100,1000,1110,1010,1110,1010]);
        assert_eq!(pcm(&bank.music_wav(&[2]).unwrap()),[1200,1200,1210,1210]);
        for selected in [vec![],vec![0,0],vec![3],vec![0,1,2]] { assert!(bank.music_wav(&selected).is_err()); }
        assert!(bank.wav(1).is_err());
    }
}
#[test]
fn unsupported_codec_skip_layout_and_incorrect_totals_fail_closed() {
    for (at,n,size) in [(72,12,1),(52,1,4),(48,3,4),(64,5,4),(128+8,1,4),
        (128+20,1,4),(128+4,50,4),(128+12,5000,4),(128+0x800,89,2)] {
        let mut bytes=fixture(false); put(&mut bytes,at,n,size,false);
        assert!(Bank::open(&bytes).unwrap().music_wav(&[0,1]).is_err(),"accepted change at {at}");
    }
    let mut bytes=fixture(false); put(&mut bytes,4,0xff0c0001,4,false);
    assert!(Bank::open(&bytes).unwrap().music_wav(&[0]).is_err());
    let bytes=fixture(false);
    for end in [0,15,31,47,100,127,bytes.len()-1] { assert!(Bank::open(&bytes[..end]).is_err()); }
}
#[test]
fn music_protocol_export_preserves_source_and_channel_selection() {
    let ticks=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir=std::env::temp_dir().join(format!("glue5-music-{}-{ticks}",std::process::id()));
    std::fs::create_dir_all(dir.join("source")).unwrap(); std::fs::create_dir(dir.join("cache")).unwrap();
    let bytes=fixture(false); std::fs::write(dir.join("source/music.awc"),&bytes).unwrap();
    let api=Resources::open(&dir.join("source"),&dir.join("cache"),None).unwrap();
    let list=api.audio_list("music.awc",0,1).unwrap(); assert_eq!(list["music"]["channels"][0]["stream_id"],3);
    for channels in [json!(null),json!([]),json!([0,1,2]),json!([32]),json!([-1]),json!(["0"])] {
        let result=api.dispatch(json!({"jsonrpc":"2.0","id":1,"method":"audio.extract_music","params":{"uri":"music.awc","channels":channels}}));
        assert_eq!(result["error"]["code"],-32602);
    }
    assert_eq!(std::fs::read_dir(dir.join("cache")).unwrap().count(),0);
    let result=api.dispatch(json!({"jsonrpc":"2.0","id":2,"method":"audio.extract_music","params":{"uri":"music.awc","channels":[1,0]}}));
    assert!(result.get("error").is_none(),"{result}");
    let result=&result["result"];
    assert_eq!(result["receipt"]["selection"]["channels"],json!([1,0]));
    let output=dir.join("cache").join(result["bundle"].as_str().unwrap()).join("audio.wav");
    assert_eq!(pcm(&std::fs::read(output).unwrap()),[1100,1000,1100,1000,1110,1010,1110,1010]);
    assert_eq!(std::fs::read(dir.join("source/music.awc")).unwrap(),bytes);
    std::fs::remove_dir_all(dir).unwrap();
}
