//! Memory ordering the game's x86 code takes for granted.
//!
//! On x86 a core's stores become visible to the others in the order it made
//! them, so code that publishes data with plain stores (fill an entry, then
//! raise a count) works there. ARM makes no such promise, and wasm's plain
//! accesses to shared memory have no ordering either: wasm2c turns them into
//! plain loads and stores. Where the game relies on that order, the plain
//! accesses that publish and the ones that read are made atomic (sequentially
//! consistent: `stlr`/`ldar` on ARM), in a copy of the function's file.

use std::fs;
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::mangle::game_symbol;

/// The memory arguments every access in the wasm2c output starts with.
const M: &str = "wasm_rt_local_memory_base, wasm_rt_local_memory_size, instance->w2c_env_memory";

/// In `function` (its name in the wasm module), `count` occurrences of a
/// plain access become atomic: `from` (with `{M}` for the memory arguments)
/// is replaced by itself with `plain` changed to `atomic`.
struct Fix {
    function: &'static str,
    from: &'static str,
    plain: &'static str,
    atomic: &'static str,
    count: usize,
}

/// `phSimulator::Update` adds the frame's collision pairs to a list (an entry,
/// its manifold, then the list's count at +32, and the list closed at +116)
/// while the physics worker threads already process it (`ProcessPairListTask`,
/// taking the pairs up to that count). On ARM a worker saw the count before
/// the entry and processed a stale manifold, another worker's: its contacts
/// changed under it and the engine's bounds checks trapped (`unreachable`;
/// a Pixel 8a, now and then after minutes of play).
const FIXES: &[Fix] = &[
    // The pairs' count, raised after each entry (the list, then its second
    // list at +64).
    Fix {
        function: "rage::phSimulator::Update(float, bool)",
        from: "var_i1 = i32_load_shared({M}, (u64)checked_add_u64(var_j1, 32u));
        var_i2 = 1u;
        var_i1 += var_i2;
        i32_store_shared({M}, (u64)checked_add_u64(var_j0, 32u), var_i1);",
        plain: "i32_store_shared(",
        atomic: "i32_atomic_store_shared(",
        count: 2,
    },
    // The list closed: no more pairs to wait for.
    Fix {
        function: "rage::phSimulator::Update(float, bool)",
        from: "var_j0 = var_l5;
    var_i1 = 1u;
    i32_store8_shared({M}, (u64)checked_add_u64(var_j0, 116u), var_i1);",
        plain: "i32_store8_shared(",
        atomic: "i32_atomic_store8_shared(",
        count: 1,
    },
    // The workers: the count, then whether the list is closed.
    Fix {
        function: "rage::phSimulator::ProcessPairListTask(rage::sysTaskParameters&)",
        from: "var_i1 = i32_load_shared({M}, (u64)checked_add_u64(var_j1, 32u));",
        plain: "i32_load_shared(",
        atomic: "i32_atomic_load_shared(",
        count: 1,
    },
    Fix {
        function: "rage::phSimulator::ProcessPairListTask(rage::sysTaskParameters&)",
        from: "var_i0 = i32_load8_u_shared({M}, (u64)checked_add_u64(var_j0, 116u));",
        plain: "i32_load8_u_shared(",
        atomic: "i32_atomic_load8_u_shared(",
        count: 1,
    },
];

/// Applies the fixes to `texts` (each source's text, possibly already
/// patched by the hooks), keyed like `sources`.
pub fn apply(sources: &[PathBuf], texts: &mut [Option<String>]) -> Result<()> {
    for fix in FIXES {
        let symbol = game_symbol(fix.function);
        let needle = format!("\n{} {symbol}(w2c_game* instance", "void");
        let from = fix.from.replace("{M}", M);
        let mut done = false;
        for (source, text) in sources.iter().zip(texts.iter_mut()) {
            let current = match text {
                Some(t) => t.clone(),
                None => fs::read_to_string(source)?,
            };
            // The definition (the original's, or `_orig` once hooked).
            let Some(start) =
                current.find(&needle).or_else(|| current.find(&needle.replace("(w2c_game*", "_orig(w2c_game*")))
            else {
                continue;
            };
            let Some(len) = current[start..].find("\n}\n") else {
                bail!("{}: end of the function not found", fix.function);
            };
            let body = &current[start..start + len];
            let found = body.matches(&from).count();
            if found != fix.count {
                bail!("{}: {} found {found} times instead of {}", fix.function, fix.plain, fix.count);
            }
            let body = body.replace(&from, &from.replacen(fix.plain, fix.atomic, 1));
            *text = Some(format!("{}{body}{}", &current[..start], &current[start + len..]));
            done = true;
            break;
        }
        if !done {
            bail!("{}: definition not found in the wasm2c output", fix.function);
        }
    }
    Ok(())
}
