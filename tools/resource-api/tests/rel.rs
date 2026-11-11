use glue5_resource_api::rel::{Sounds, name_hash};

fn record(kind: u8, flags: u32, header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend(flags.to_le_bytes()); out.extend(header); out.extend(body); out
}
fn set(event: &str, child: u32) -> Vec<u8> {
    let mut body = 1u32.to_le_bytes().to_vec();
    body.extend(name_hash(event).unwrap().to_le_bytes()); body.extend(child.to_le_bytes());
    record(32, 0xaaaaaaaa, &[], &body)
}
fn simple(flags: u32, header: &[u8]) -> Vec<u8> {
    let mut body = 123u32.to_le_bytes().to_vec();
    body.extend(0xe1234567u32.to_le_bytes()); body.push(7);
    record(12, flags, header, &body)
}
fn fixture(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut data = 1u32.to_le_bytes().to_vec();
    let mut index = Vec::new();
    for (name, body) in records {
        index.extend(name.to_le_bytes()); index.extend((data.len() as u32).to_le_bytes());
        index.extend((body.len() as u32).to_le_bytes()); data.extend(body);
    }
    let mut bytes = 54u32.to_le_bytes().to_vec();
    bytes.extend((data.len() as u32).to_le_bytes()); bytes.extend(data);
    bytes.extend(4u32.to_le_bytes()); bytes.extend(0u32.to_le_bytes());
    bytes.extend((records.len() as u32).to_le_bytes()); bytes.extend(index);
    bytes.extend([0;8]); bytes
}
#[test]
fn resolves_set_and_event_to_actual_child_not_event_hash() {
    let bytes = fixture(&[(name_hash("HUD_FRONTEND_DEFAULT_SOUNDSET").unwrap(),set("SELECT",42)),(42,simple(0xaaaaaaaa,&[]))]);
    let resolved = Sounds::open(&bytes).unwrap().resolve("hud_frontend_default_soundset","select").unwrap();
    assert_eq!(resolved.child_sound_hash,42);
    assert_eq!(resolved.container_hash,123);
    assert_eq!(resolved.file_hash,0xe1234567);
    assert_eq!(resolved.stream_id,0x01234567);
    assert_eq!(resolved.wave_slot,7);
    assert_ne!(resolved.event_hash,resolved.stream_id);
    assert!(Sounds::open(&bytes).unwrap().resolve("OTHER_SET","SELECT").is_err());
    assert!(Sounds::open(&bytes).unwrap().resolve("HUD_FRONTEND_DEFAULT_SOUNDSET","BACK").is_err());
}
#[test]
fn all_optional_header_fields_and_aa_sentinels_preserve_body_offset() {
    // All 32 optional fields occupy 75 bytes. AA bytes suppress eight flags.
    for (flags,size) in [(0u32,0usize),(0xaaaaaaaa,0),(0xffffffff,75),(0xaaaaaa01,4),(0xaaaa01aa,2),(0xaa01aaaa,2),(0x01aaaaaa,1)] {
        let bytes = fixture(&[(name_hash("SET").unwrap(),set("EVENT",42)),(42,simple(flags,&vec![0;size]))]);
        assert_eq!(Sounds::open(&bytes).unwrap().resolve("SET","EVENT").unwrap().container_hash,123,"flags {flags:x}");
    }
}
#[test]
fn malformed_or_ambiguous_records_never_guess_a_waveform() {
    let base = fixture(&[(name_hash("SET").unwrap(),set("EVENT",42)),(42,simple(0,&[]))]);
    for end in 0..base.len() { assert!(Sounds::open(&base[..end]).is_err(),"truncation {end}"); }
    let dup = fixture(&[(42,simple(0,&[])),(42,simple(0,&[]))]);
    assert!(Sounds::open(&dup).is_err());
    let complex = fixture(&[(name_hash("SET").unwrap(),set("EVENT",42)),(42,record(14,0,&[],&[0;9]))]);
    assert!(Sounds::open(&complex).unwrap().resolve("SET","EVENT").unwrap_err().contains("graph rendering"));
    let truncated = fixture(&[(name_hash("SET").unwrap(),set("EVENT",42)),(42,simple(u32::MAX,&[]))]);
    assert!(Sounds::open(&truncated).unwrap().resolve("SET","EVENT").is_err());
    let mut extra = base.clone(); extra.push(0); assert!(Sounds::open(&extra).is_err());
    for invalid in ["","é","SELECT\n"] { assert!(name_hash(invalid).is_err()); }
    assert_eq!(name_hash("WEAPON_PISTOL").unwrap(),0x1b06d571);
}

#[test]
fn duplicate_event_and_overlapping_record_extents_are_rejected() {
    let mut body=2u32.to_le_bytes().to_vec();
    for _ in 0..2 { body.extend(name_hash("EVENT").unwrap().to_le_bytes()); body.extend(42u32.to_le_bytes()); }
    let bytes=fixture(&[(name_hash("SET").unwrap(),record(32,0,&[],&body)),(42,simple(0,&[]))]);
    assert!(Sounds::open(&bytes).unwrap().resolve("SET","EVENT").is_err());
    let mut bytes=fixture(&[(1,simple(0,&[])),(2,simple(0,&[]))]);
    let len=u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let index=8+len+8+4;
    bytes[index+12+4..index+12+8].copy_from_slice(&4u32.to_le_bytes());
    assert!(Sounds::open(&bytes).is_err());
}

#[test]
fn original_bank_identity_is_explicit_and_confined() {
    use glue5_resource_api::rel::bank_name;
    assert_eq!(bank_name("x64/audio/sfx/RESIDENT.rpf::frontend.awc").unwrap(),"resident/frontend");
    assert_eq!(bank_name("x64/audio/sfx/RESIDENT/FRONTEND.awc").unwrap(),"resident/frontend");
    for invalid in ["frontend.awc","../RESIDENT/frontend.awc","RESIDENT.rpf::../frontend.awc","RESIDENT.rpf::other.rpf::frontend.awc","RESIDENT.rpf::frontend.wav"] {
        assert!(bank_name(invalid).is_err(),"{invalid}");
    }
}

#[test]
fn event_export_checks_both_inputs_and_records_both_hashes() {
    use glue5_resource_api::Resources;
    use serde_json::json;
    let ticks=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir=std::env::temp_dir().join(format!("glue5-rel-event-{}-{ticks}",std::process::id()));
    std::fs::create_dir_all(dir.join("source/RESIDENT")).unwrap();
    std::fs::create_dir_all(dir.join("source/OTHER")).unwrap();
    std::fs::create_dir(dir.join("cache")).unwrap();
    let mut sound=simple(0,&[]);
    sound[5..9].copy_from_slice(&name_hash("RESIDENT/frontend").unwrap().to_le_bytes());
    let rel=fixture(&[(name_hash("SET").unwrap(),set("EVENT",42)),(42,sound)]);
    std::fs::write(dir.join("source/sounds.rel"),&rel).unwrap();
    let mut awc=vec![0u8;70]; awc[..4].copy_from_slice(b"ADAT");
    for (at,value) in [(4,0xff020001u32),(8,1),(12,48),(16,(2<<29)|0x01234567),(48,1)] {
        awc[at..at+4].copy_from_slice(&value.to_le_bytes());
    }
    awc[20..28].copy_from_slice(&((0xfau64<<56)|(20<<28)|48).to_le_bytes());
    awc[28..36].copy_from_slice(&((0x55u64<<56)|(2<<28)|68).to_le_bytes());
    awc[56..58].copy_from_slice(&48000u16.to_le_bytes()); awc[67]=1;
    awc[68..70].copy_from_slice(&1234i16.to_le_bytes());
    for pack in ["RESIDENT","OTHER"] { std::fs::write(dir.join(format!("source/{pack}/frontend.awc")),&awc).unwrap(); }
    let api=Resources::open(&dir.join("source"),&dir.join("cache"),None).unwrap();
    assert!(api.audio_extract_event("sounds.rel","OTHER/frontend.awc","SET","EVENT").is_err());
    for method in ["audio.resolve","audio.extract_event"] {
        let response=api.dispatch(json!({"jsonrpc":"2.0","id":1,"method":method,"params":{"uri":"sounds.rel","soundset":"SET"}}));
        assert_eq!(response["error"]["code"],-32602);
    }
    assert_eq!(std::fs::read_dir(dir.join("cache")).unwrap().count(),0);
    let result=api.dispatch(json!({"jsonrpc":"2.0","id":2,"method":"audio.extract_event","params":{"uri":"sounds.rel","bank_uri":"RESIDENT/frontend.awc","soundset":"SET","event":"EVENT"}}));
    assert!(result.get("error").is_none(),"{result}");
    let result=&result["result"];
    assert_eq!(result["receipt"]["selection"]["reference"]["stream_id"],0x01234567);
    for key in ["rel_sha256","source_sha256"] { assert_eq!(result["receipt"]["selection"][key].as_str().unwrap().len(),64); }
    let output=dir.join("cache").join(result["bundle"].as_str().unwrap()).join("audio.wav");
    assert_eq!(&std::fs::read(output).unwrap()[44..],1234i16.to_le_bytes());
    assert_eq!(std::fs::read(dir.join("source/sounds.rel")).unwrap(),rel);
    assert_eq!(std::fs::read(dir.join("source/RESIDENT/frontend.awc")).unwrap(),awc);
    std::fs::remove_dir_all(dir).unwrap();
}
