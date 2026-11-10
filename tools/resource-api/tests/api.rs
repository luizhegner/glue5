use aes::{Aes256, cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray}};
use flate2::{Compression, write::DeflateEncoder};
use glue5_resource_api::Resources;
use serde_json::json;
use std::{fs, io::Write, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}, time::{SystemTime, UNIX_EPOCH}};
static SERIAL: AtomicUsize = AtomicUsize::new(0);
const KEY: [u8; 32] = [0x35; 32]; // Synthetic test key, not game material.
const PAYLOAD: &[u8] = b"GFX\x09synthetic resource fixture";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let ticks = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("glue5-resource-test-{ticks}-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("source")).unwrap();
        fs::create_dir(path.join("cache")).unwrap();
        Self(path)
    }
    fn api(&self, key: Option<[u8;32]>) -> Resources { Resources::open(&self.0.join("source"), &self.0.join("cache"), key).unwrap() }
}
impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

fn fixture(shift: u32, encrypted: bool, compressed: bool) -> Vec<u8> {
    let mut names = vec![0; 1 << shift];
    names.extend(b"sample.gfx\0");
    names.resize((names.len() + 15) / 16 * 16, 0);
    let mut payload = if compressed {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(PAYLOAD).unwrap(); encoder.finish().unwrap()
    } else { PAYLOAD.to_vec() };
    let mut table = Vec::new();
    for word in [0u32, 0x7fffff00, 1, 1] { table.extend(word.to_le_bytes()); }
    let mut entry = [0;16];
    entry[0] = 1;
    if compressed { entry[2..5].copy_from_slice(&(payload.len() as u32).to_le_bytes()[..3]); }
    entry[5] = 1; // 512-byte sector.
    entry[8..12].copy_from_slice(&(PAYLOAD.len() as u32).to_le_bytes());
    if encrypted {
        entry[12..16].copy_from_slice(&1u32.to_le_bytes());
        let cipher = Aes256::new_from_slice(&KEY).unwrap();
        for block in payload.chunks_exact_mut(16) { cipher.encrypt_block(GenericArray::from_mut_slice(block)); }
    }
    table.extend(entry);
    table.extend(&names);
    if encrypted {
        let cipher = Aes256::new_from_slice(&KEY).unwrap();
        for block in table.chunks_exact_mut(16) { cipher.encrypt_block(GenericArray::from_mut_slice(block)); }
    }
    let mut bytes = b"7FPR".to_vec();
    for word in [2, (names.len() as u32) | (shift << 28), if encrypted {0x0ffffff9} else {0x4e45504f}] { bytes.extend(word.to_le_bytes()); }
    bytes.extend(table);
    bytes.resize(512, 0);
    bytes.extend(payload);
    bytes
}

#[test]
fn open_and_aes_archives_resolve_both_name_shifts_and_deflate() {
    for shift in [0,1] { for encrypted in [false,true] { for compressed in [false,true] {
        let temp = Temp::new();
        fs::write(temp.0.join("source/test.rpf"), fixture(shift,encrypted,compressed)).unwrap();
        let api = temp.api(Some(KEY));
        assert_eq!(api.read("test.rpf::sample.gfx").unwrap(), PAYLOAD);
        let listing = api.list("test.rpf", 0, 1).unwrap();
        assert_eq!(listing["total"], 1);
        assert_eq!(listing["metadata"]["name_shift"], shift);
    } } }
}

#[test]
fn wrong_shift_wrong_key_and_ng_are_refused_without_writes() {
    let temp = Temp::new();
    let path = temp.0.join("source/test.rpf");
    let mut data = fixture(0,true,false);
    data[11] |= 0x10;
    fs::write(&path, &data).unwrap();
    assert!(temp.api(Some(KEY)).read("test.rpf::sample.gfx").is_err());
    fs::write(&path, fixture(0,true,false)).unwrap();
    assert!(temp.api(None).list("test.rpf",0,10).is_err());
    assert!(temp.api(Some([0;32])).list("test.rpf",0,10).is_err());
    data[12..16].copy_from_slice(&0x0fefffffu32.to_le_bytes());
    fs::write(&path, &data).unwrap();
    assert!(temp.api(Some(KEY)).list("test.rpf",0,10).is_err());
    assert_eq!(fs::read(&path).unwrap(),data);
    assert_eq!(fs::read_dir(temp.0.join("cache")).unwrap().count(),0);
}

#[test]
fn extract_is_explicit_and_records_hash_without_overwriting_sources_or_prior_exports() {
    let temp = Temp::new();
    let source = temp.0.join("source/test.rpf");
    let original = fixture(1,true,true);
    fs::write(&source, &original).unwrap();
    let api = temp.api(Some(KEY));
    let first = api.extract("test.rpf::sample.gfx").unwrap();
    let second = api.extract("test.rpf::sample.gfx").unwrap();
    assert_ne!(first["bundle"],second["bundle"]);
    assert_eq!(first["receipt"]["sha256"],api.inspect("test.rpf::sample.gfx").unwrap()["sha256"]);
    let bundle = first["bundle"].as_str().unwrap();
    assert_eq!(fs::read(temp.0.join("cache").join(bundle).join("resource.bin")).unwrap(), PAYLOAD);
    assert!(temp.0.join("cache").join(bundle).join("receipt.json").is_file());
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(!serde_json::to_string(&first).unwrap().contains(&format!("{:?}",KEY)));
}

#[test]
fn traversal_absolute_paths_overlapping_sources_and_excessive_limits_are_refused() {
    let temp = Temp::new();
    let api = temp.api(None);
    for path in ["../outside", "/etc/passwd", "C:/secret", "a\\b", "a//b", "a/../b", "a.", "x::y::z", "NUL", "COM1.log"] {
        assert!(api.read(path).is_err(), "accepted {path}");
    }
    assert!(Resources::open(&temp.0, &temp.0.join("cache"),None).is_err());
    assert!(api.list("",0,513).is_err());
    assert!(api.list("",0,0).is_err());
}

#[test]
fn cyclic_directory_tables_and_payloads_outside_the_file_are_refused() {
    let temp = Temp::new();
    let path = temp.0.join("source/bad.rpf");
    let mut bytes = fixture(0,false,false);
    bytes[24..28].copy_from_slice(&0u32.to_le_bytes());
    fs::write(&path,&bytes).unwrap();
    assert!(temp.api(None).list("bad.rpf",0,10).is_err());
    let mut bytes = fixture(0,false,false);
    bytes[37..40].copy_from_slice(&[255,255,127]);
    fs::write(&path,&bytes).unwrap();
    assert!(temp.api(None).list("bad.rpf",0,10).is_err());
}

#[test]
fn protocol_keeps_request_ids_and_reports_capabilities_and_unknown_methods() {
    let temp = Temp::new();
    let api = temp.api(None);
    let reply = api.dispatch(json!({"jsonrpc":"2.0","id":"agent-1","method":"api.capabilities"}));
    assert_eq!(reply["id"],"agent-1");
    assert_eq!(reply["result"]["api_version"],1);
    let reply = api.dispatch(json!({"jsonrpc":"2.0","id":7,"method":"run_shell"}));
    assert_eq!(reply["error"]["code"],-32601);
    assert!(api.dispatch(json!({"jsonrpc":"wrong"}))["error"].is_object());
    for params in [json!({"offset":-1}), json!({"limit":0}), json!({"path":7}), json!([])] {
        let reply = api.dispatch(json!({"jsonrpc":"2.0","id":9,"method":"resources.list","params":params}));
        assert_eq!(reply["error"]["code"],-32602);
    }
}

#[test]
fn command_line_emits_only_json_and_survives_malformed_request() {
    use std::process::{Command, Stdio};
    let temp = Temp::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_glue5-resource-api"))
        .args(["--source",temp.0.join("source").to_str().unwrap(),"--cache",temp.0.join("cache").to_str().unwrap()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"not-json\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"api.capabilities\"}\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let lines: Vec<serde_json::Value> = std::str::from_utf8(&output.stdout).unwrap().lines().map(|s|serde_json::from_str(s).unwrap()).collect();
    assert_eq!(lines.len(),2);
    assert_eq!(lines[0]["error"]["code"],-32700);
    assert_eq!(lines[1]["result"]["api_version"],1);
}

#[cfg(unix)]
#[test]
fn links_cannot_escape_source() {
    let temp = Temp::new();
    fs::write(temp.0.join("private"),b"not a resource").unwrap();
    std::os::unix::fs::symlink(temp.0.join("private"),temp.0.join("source/link")).unwrap();
    assert!(temp.api(None).read("link").is_err());
}
