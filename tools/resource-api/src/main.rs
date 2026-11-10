use glue5_resource_api::Resources;
use serde_json::json;
use std::{io::{self, BufRead, Read, Write}, path::PathBuf};

fn main() {
    if let Err(message) = run() { eprintln!("glue5-resource-api: {message}"); std::process::exit(1); }
}
fn run() -> Result<(), String> {
    let mut source = None;
    let mut cache = None;
    let mut key = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source" => source = Some(PathBuf::from(args.next().ok_or("missing source")?)),
            "--cache" => cache = Some(PathBuf::from(args.next().ok_or("missing export cache")?)),
            "--aes-key" => {
                let path = args.next().ok_or("missing AES key file")?;
                let mut bytes = Vec::new();
                std::fs::File::open(path).map_err(|_| "could not open key file")?.take(33).read_to_end(&mut bytes).map_err(|_| "could not read key file")?;
                key = Some(bytes.try_into().map_err(|_| "key file must contain exactly 32 bytes")?);
            }
            "--help" => { println!("glue5-resource-api --source DIRECTORY --cache SEPARATE_EXISTING_DIRECTORY [--aes-key FILE]\nJSON-RPC 2.0 requests on stdin, one JSON object per line. No server or game writes."); return Ok(()); }
            _ => return Err("unknown argument (see --help)".into()),
        }
    }
    let api = Resources::open(&source.ok_or("--source is required")?, &cache.ok_or("--cache is required")?, key)?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut line = Vec::new();
        let size = (&mut input).take(65537).read_until(b'\n', &mut line).map_err(|e|e.to_string())?;
        if size == 0 { break; }
        if size > 65536 { return Err("request exceeds 64 KiB; input closed".into()); }
        let response = match serde_json::from_slice(&line) {
            Ok(request) => api.dispatch(request),
            Err(_) => json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"invalid JSON"}}),
        };
        serde_json::to_writer(&mut output, &response).map_err(|e|e.to_string())?;
        output.write_all(b"\n").and_then(|_|output.flush()).map_err(|e|e.to_string())?;
    }
    Ok(())
}
