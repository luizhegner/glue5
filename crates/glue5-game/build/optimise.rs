//! Optimisation of the game's C across its files, both opt-in until
//! measured on the devices:
//!
//! - `GLUE5_GAME_LTO=1`: ThinLTO. wasm2c splits the game into 64 files with
//!   no regard for who calls whom (87% of direct calls cross files, to tiny
//!   functions often); the game's objects are compiled to bitcode and
//!   linked into one native object (`-r`), inlining across files with a
//!   small import limit. The rest of the build is unchanged: an archive the
//!   Rust side links.
//! - `GLUE5_GAME_PGO=generate`: an instrumented game whose profile
//!   (`LLVM_PROFILE_FILE`, `default_%m.profraw` otherwise) is merged with
//!   `llvm-profdata merge` (`mise run pgo` does it all from a `-benchmark`
//!   run); `GLUE5_GAME_PGO=<file.profdata>`: the game compiled with it.
//!
//! Clang only, not clang-cl (Windows).

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Functions inlined across files at most this large (in instructions):
/// with 8 the game's code grew 4% for 8% fewer calls (measured on an M1
/// Pro's build).
const IMPORT_LIMIT: u32 = 8;

pub struct Options {
    lto: bool,
    pgo: Pgo,
}

enum Pgo {
    None,
    Generate,
    /// The profile, and its contents' hash (objects compiled with another
    /// profile at the same path are other objects).
    Use(PathBuf, String),
}

impl Options {
    pub fn from_env(msvc: bool) -> Result<Options> {
        println!("cargo:rerun-if-env-changed=GLUE5_GAME_LTO");
        println!("cargo:rerun-if-env-changed=GLUE5_GAME_PGO");
        let lto = env::var("GLUE5_GAME_LTO").is_ok_and(|v| v == "1");
        let pgo = match env::var("GLUE5_GAME_PGO").ok().filter(|v| !v.is_empty()) {
            None => Pgo::None,
            Some(v) if v == "generate" => Pgo::Generate,
            Some(path) => {
                let path = PathBuf::from(path);
                println!("cargo:rerun-if-changed={}", path.display());
                let data =
                    std::fs::read(&path).with_context(|| format!("GLUE5_GAME_PGO: reading {}", path.display()))?;
                let hash = Sha256::digest(&data).iter().take(8).map(|b| format!("{b:02x}")).collect();
                Pgo::Use(path, hash)
            }
        };
        if msvc && (lto || !matches!(pgo, Pgo::None)) {
            bail!("GLUE5_GAME_LTO and GLUE5_GAME_PGO need Clang's own command line, not clang-cl");
        }
        Ok(Options { lto, pgo })
    }

    /// The game's compile flags.
    pub fn apply(&self, build: &mut cc::Build) {
        if self.lto {
            build.flag("-flto=thin");
        }
        match &self.pgo {
            Pgo::None => {}
            Pgo::Generate => {
                build.flag("-fprofile-generate");
            }
            Pgo::Use(path, _) => {
                build.flag(format!("-fprofile-use={}", path.display()));
            }
        }
    }

    /// The functions the profile ran, the busiest first (`hot.txt` beside
    /// it; see hot.rs).
    pub fn hot_list(&self) -> Result<Option<Vec<String>>> {
        let Pgo::Use(path, _) = &self.pgo else { return Ok(None) };
        let list = path.with_file_name("hot.txt");
        println!("cargo:rerun-if-changed={}", list.display());
        let Ok(text) = std::fs::read_to_string(&list) else { return Ok(None) };
        Ok(Some(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()))
    }

    /// What the objects' cache key adds.
    pub fn key(&self) -> String {
        match &self.pgo {
            Pgo::Use(_, hash) => format!(" pgo {hash}"),
            _ => String::new(),
        }
    }

    /// The game's objects as the archive takes them: with ThinLTO, one
    /// native object linked from their bitcode (`cache` keeps each file's
    /// part between builds).
    pub fn link(&self, build: &cc::Build, objects: Vec<PathBuf>, cache: &Path, out: &Path) -> Result<Vec<PathBuf>> {
        if !self.lto {
            return Ok(objects);
        }
        std::fs::create_dir_all(cache)?;
        let linked = out.join("game_lto.o");
        let apple = env::var("CARGO_CFG_TARGET_VENDOR")? == "apple";
        let mut cmd = build.get_compiler().to_command();
        cmd.args(["-r", "-nostdlib", "-flto=thin"]).args(&objects).arg("-o").arg(&linked);
        if apple {
            cmd.arg(format!("-Wl,-cache_path_lto,{}", cache.display()));
        } else {
            cmd.arg("-fuse-ld=lld").arg(format!("-Wl,--thinlto-cache-dir={}", cache.display()));
        }
        cmd.arg(format!("-Wl,-mllvm,-import-instr-limit={IMPORT_LIMIT}"));
        eprintln!("linking the game's bitcode (ThinLTO)");
        let output = cmd.output().with_context(|| format!("running {cmd:?}"))?;
        if !output.status.success() {
            bail!("ThinLTO link failed:\n{}", String::from_utf8_lossy(&output.stderr));
        }
        Ok(vec![linked])
    }

    /// An instrumented game needs Clang's profile runtime in the final link.
    pub fn runtime(&self, build: &cc::Build) -> Result<()> {
        if !matches!(self.pgo, Pgo::Generate) {
            return Ok(());
        }
        let mut cmd = build.get_compiler().to_command();
        let output = cmd.arg("--print-runtime-dir").output().context("asking Clang for its runtime directory")?;
        let dir = String::from_utf8(output.stdout)?.trim().to_owned();
        let os = match env::var("CARGO_CFG_TARGET_OS")?.as_str() {
            "macos" => "osx".to_owned(),
            "ios" => "ios".to_owned(),
            "tvos" => "tvos".to_owned(),
            _ => String::new(),
        };
        // Apple's: libclang_rt.profile_<os>.a; elsewhere: per target in its
        // own directory (libclang_rt.profile.a) or with the arch in the name.
        let name = if !os.is_empty() {
            format!("clang_rt.profile_{os}")
        } else if Path::new(&dir).join("libclang_rt.profile.a").exists() {
            "clang_rt.profile".to_owned()
        } else {
            format!("clang_rt.profile-{}", env::var("CARGO_CFG_TARGET_ARCH")?)
        };
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=static={name}");
        Ok(())
    }
}
