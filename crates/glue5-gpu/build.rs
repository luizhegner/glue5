//! On Android, libadrenotools (third_party/libadrenotools, from `mise run
//! setup`) as the shared libraries the app ships, as its own CMake build
//! makes them: `libadrenotools.so`, which the renderer opens only for a GPU
//! driver the user added (its linkernsbypass patches the linker's pages as it
//! loads), and the hooks it loads into its copy of the Vulkan loader,
//! `libhook_impl.so` and `libmain_hook.so`. They go next to the crate's
//! library (as sdl3-sys does with libSDL3.so), where the Android tasks take
//! them for the APK.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../third_party/libadrenotools");
    println!("cargo::rerun-if-changed={}", src.join(".glue5-stamp").display());
    assert!(src.join(".glue5-stamp").exists(), "{} is missing: run `mise run setup`", src.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let lnb = src.join("lib/linkernsbypass");

    // Linked into both shared libraries and exported by neither.
    cc::Build::new()
        .cpp(true)
        .std("c++20")
        .files([lnb.join("android_linker_ns.cpp"), lnb.join("elf_soname_patcher.cpp")])
        .include(&lnb)
        .warnings(false)
        .cargo_metadata(false)
        .compile("linkernsbypass");

    let cpp = |name: &str, hidden: bool, sources: &[PathBuf]| {
        let mut c = shared(true, name, hidden);
        c.arg("-std=c++20").arg("-static-libstdc++").args(sources);
        c.arg(format!("-I{}", src.join("include").display())).arg(format!("-I{}", lnb.display()));
        c.arg(format!("-L{}", out.display())).args(["-Wl,--exclude-libs,ALL", "-llinkernsbypass", "-llog"]);
        c.args(["-ldl", "-landroid"]);
        c
    };
    run(cpp("libadrenotools.so", false, &[src.join("src/driver.cpp")]), &out);
    run(cpp("libhook_impl.so", true, &[src.join("src/hook/hook_impl.cpp")]), &out);
    // Interposes on the loader's dlopen calls (-z global).
    let mut main_hook = shared(false, "libmain_hook.so", true);
    main_hook.arg(src.join("src/hook/main_hook.c"));
    main_hook.arg(format!("-L{}", out.display())).args(["-lhook_impl", "-Wl,-z,global"]);
    run(main_hook, &out);

    // target/<triple>/<profile>, above build/glue5-gpu-<hash>/out.
    let profile = out.ancestors().nth(3).unwrap();
    for lib in ["libadrenotools.so", "libhook_impl.so", "libmain_hook.so"] {
        fs::copy(out.join(lib), profile.join(lib)).unwrap();
    }
}

/// The NDK's compiler (C++ or C), linking the shared library `name`.
fn shared(cpp: bool, name: &str, hidden: bool) -> Command {
    let mut c = cc::Build::new().cpp(cpp).get_compiler().to_command();
    c.args(["-shared", "-fPIC", "-O2", "-Wl,-s", "-Wl,--no-undefined"]);
    c.arg(format!("-Wl,-soname,{name}")).arg("-o").arg(name);
    if hidden {
        c.arg("-fvisibility=hidden");
    }
    c
}

fn run(mut c: Command, dir: &Path) {
    let status = c.current_dir(dir).status().unwrap();
    assert!(status.success(), "{c:?} failed");
}
