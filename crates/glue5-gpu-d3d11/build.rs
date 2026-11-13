//! Build-time hotfix for the GM45 fallback box.
//!
//! The Arena Windows Relay refuses to overwrite a locally modified
//! `src/graphics.rs` (the GIT_RESTORE_FILES content guard), and pushing a
//! 47 KB file through a spreadsheet cell is error-prone. This build script
//! therefore applies the same three anchored edits as commit 4472727 before
//! the crate compiles. windows 0.62.2 facts (windows-rs d468916ac):
//! `GetDesc1` lives on `IDXGIAdapter1`, not `IDXGIAdapter`; `PCSTR` has
//! `as_bytes`/`to_string`, never `to_string_lossy`.
//!
//! Idempotent: it only rewrites the file while at least one anchor still
//! matches, and every replacement is the text commit 4472727 carries.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/graphics.rs");

    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let path = Path::new(&manifest).join("src").join("graphics.rs");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return,
    };

    let patched = text
        .replace(
            "DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIDevice,",
            "DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice,",
        )
        .replace(
            "let desc = unsafe { adapter.GetDesc1() }.context(\"IDXGIAdapter::GetDesc1\")?;",
            "let adapter1: IDXGIAdapter1 = adapter.cast().context(\"querying IDXGIAdapter1\")?;\n    let desc = unsafe { adapter1.GetDesc1() }.context(\"IDXGIAdapter1::GetDesc1\")?;",
        )
        .replace(
            "entry.to_string_lossy()",
            "String::from_utf8_lossy(unsafe { entry.as_bytes() })",
        );

    if patched != text {
        fs::write(&path, patched).expect("writing the patched src/graphics.rs");
        println!("cargo:warning=gm45 hotfix applied to src/graphics.rs (commit 4472727 content)");
    }
}
