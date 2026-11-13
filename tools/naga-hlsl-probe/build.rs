// Build provenance only: never generates/replaces the reader or game assets.
use sha2::{Digest,Sha256};
use std::{env,fs,path::PathBuf};
fn main() {
    let root=PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    inspect_game_input(root.join("../../../game.wasm"));
    for (name,path) in [("shared-reader",root.join("src/lib.rs")),
        ("runtime-gpu",root.join("../../crates/glue5-runtime/src/gpu.rs")),
        ("runtime-memory",root.join("../../crates/glue5-runtime/src/memory.rs"))] {
        println!("cargo:rerun-if-changed={}",path.display());
        if let Ok(bytes)=fs::read(&path) {
            let hex:String=Sha256::digest(&bytes).iter().map(|b|format!("{b:02x}")).collect();
            println!("cargo:warning={name}-source-sha256={hex}");
        }
    }
}

// Optional, read-only provenance for the operator-supplied real WASM. Never
// stages files, bypasses the game build check, or invents a supported build.
fn inspect_game_input(path:PathBuf) {
    use std::io::Read;
    println!("cargo:rerun-if-changed={}",path.display());
    let Ok(mut file)=fs::File::open(&path) else {return};
    let mut h=Sha256::new();let mut buf=[0u8;65536];let mut size=0u64;
    let mut header=[0u8;8];
    loop {
        let n=match file.read(&mut buf) {Ok(n)=>n,Err(e)=>{
            println!("cargo:warning=operator-wasm-read-error={e}");return;
        }};
        if n==0 {break;}
        if size<8 {let at=size as usize;let end=(at+n).min(8);header[at..end].copy_from_slice(&buf[..end-at]);}
        h.update(&buf[..n]);size+=n as u64;
    }
    let hex:String=h.finalize().iter().map(|b|format!("{b:02x}")).collect();
    let valid=header==[0,97,115,109,1,0,0,0];
    let supported=valid && hex=="11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0";
    println!("cargo:warning=operator-wasm-bytes={size} header-v1={valid} sha256={hex} supported-build-8b0b5899ed={supported}");
}
