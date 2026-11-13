//! Resources placed in more pages than their file left room for.
//!
//! A resource starts with `rage::pgBase`, whose page map (in the file,
//! after the object) lists the memory pages the resource was placed in, so
//! that `pgBase::Destroy` can free them. The file reserves an entry per page
//! (the counts at +8 and +9, entries of 8 bytes from +16), and
//! `pgBase::pgBase()` writes one per page the resource was really placed in.
//! In this game data some resources are placed in more physical (GPU data)
//! pages than they reserved, one or two (about one resource in 300): the
//! extra entries overwrote what follows the map. For the two fences of the
//! Vinewood Hills (`ch2_07d_fencing_02`, `ch2_07_fence_03`, at the end of
//! Marriage Counseling) that is their texture dictionary's own page map
//! pointer, and the game stopped with "Invalid fixup".
//!
//! Such a resource gets a page map of its size in the engine's heap,
//! tagged just before it; `pgBase::Destroy` frees it once it has read it.
//! The engine never moves resources here (`-nodefrag`): a move would place
//! the resource again and look for the map inside it.

use std::fs;
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::mangle::game_symbol;

/// The memory arguments every access in the wasm2c output starts with.
const M: &str = "wasm_rt_local_memory_base, wasm_rt_local_memory_size, instance->w2c_env_memory";

/// The helpers, defined before the first function they are used in.
const HELPERS: &str = r"
/* A page map outside its resource (build/page_map.rs): 8 bytes before it,
   its address with this tag, so that it is told apart from one in place. */
#define GLUE5_PAGE_MAP_TAG 0x67357061676d6170ull

/* pgBase::pgBase(): the map at `map` has room for fewer pages than the
   resource being placed (`resource`, a datResource) has: a map with room
   for them all, in the heap, becomes the resource's (`base` + 8). */
static u64 glue5_page_map_room(w2c_game* instance, u64 base, u64 map, u64 resource) {
  uint8_t* const wasm_rt_local_memory_base = NULL;
  uint64_t wasm_rt_local_memory_size = 0;
  (void)wasm_rt_local_memory_base;
  (void)wasm_rt_local_memory_size;
  if (!map || i32_load8_u_shared({M}, map + 11)) return map;
  u64 pages = i64_load_shared({M}, resource);
  u32 placed = i32_load8_u_shared({M}, pages) + i32_load8_u_shared({M}, pages + 1);
  u32 room = i32_load8_u_shared({M}, map + 8) + i32_load8_u_shared({M}, map + 9);
  if (placed <= room) return map;
  u64 block = w2c_game_emscripten_builtin_malloc(instance, 8 + 16 + 8 * (u64)placed);
  if (!block) return map;
  i64_store_shared({M}, block, (block + 8) ^ GLUE5_PAGE_MAP_TAG);
  i64_store_shared({M}, block + 8, i64_load_shared({M}, map));
  i64_store_shared({M}, block + 16, i64_load_shared({M}, map + 8));
  i64_store_shared({M}, base + 8, block + 8);
  return block + 8;
}

/* pgBase::Destroy(): its pages are known (RegenerateMap): a map in the heap
   is freed. */
static void glue5_page_map_free(w2c_game* instance, u64 base) {
  uint8_t* const wasm_rt_local_memory_base = NULL;
  uint64_t wasm_rt_local_memory_size = 0;
  (void)wasm_rt_local_memory_base;
  (void)wasm_rt_local_memory_size;
  u64 map = i64_load_shared({M}, base + 8);
  if (map < 8 || i64_load_shared({M}, map - 8) != (map ^ GLUE5_PAGE_MAP_TAG)) return;
  w2c_game_emscripten_builtin_free(instance, map - 8);
}

";

/// `function`'s text `from` gets `insert` after it, once.
struct Insert {
    function: &'static str,
    ret: &'static str,
    from: &'static str,
    insert: &'static str,
}

const INSERTS: &[Insert] = &[
    // The map's address is known (fixed up and stored at +8); the counts
    // and entries are written next.
    Insert {
        function: "rage::pgBase::pgBase()",
        ret: "u64",
        from: "  var_l1 = var_j1;
  i64_store_shared({M}, (u64)checked_add_u64(var_j0, 8u), var_j1);
",
        insert: "  var_l1 = glue5_page_map_room(instance, var_p0, var_l1, var_l3);\n",
    },
    // The pages are listed: the map is no longer needed.
    Insert {
        function: "rage::pgBase::Destroy()",
        ret: "void",
        from: "  w2c_game_rage0x3A0x3ApgBase0x3A0x3ARegenerateMap0x28rage0x3A0x3AdatResourceMap0x260x290x20const(instance, var_j0, var_j1);
",
        insert: "  glue5_page_map_free(instance, var_p0);\n",
    },
];

/// Applies the fix to `texts` (each source's text, possibly already
/// patched), keyed like `sources`.
pub fn apply(sources: &[PathBuf], texts: &mut [Option<String>]) -> Result<()> {
    let mut helpers_in: Option<usize> = None;
    for insert in INSERTS {
        let symbol = game_symbol(insert.function);
        let needle = format!("\n{} {symbol}(w2c_game* instance", insert.ret);
        let from = insert.from.replace("{M}", M);
        let mut done = false;
        for (i, (source, text)) in sources.iter().zip(texts.iter_mut()).enumerate() {
            let mut current = match text {
                Some(t) => std::mem::take(t),
                None => fs::read_to_string(source)?,
            };
            let Some(start) =
                current.find(&needle).or_else(|| current.find(&needle.replace("(w2c_game*", "_orig(w2c_game*")))
            else {
                if text.is_some() {
                    *text = Some(current);
                }
                continue;
            };
            let Some(len) = current[start..].find("\n}\n") else {
                bail!("{}: end of the function not found", insert.function);
            };
            let body = &current[start..start + len];
            if body.matches(&from).count() != 1 {
                bail!("{}: the text to patch was not found once: the game changed", insert.function);
            }
            let body = body.replacen(&from, &format!("{from}{}", insert.insert), 1);
            current = format!("{}{body}{}", &current[..start], &current[start + len..]);
            if helpers_in.is_none_or(|f| f != i) {
                // Before the first function of the file.
                let at = current
                    .find("\nu32 w2c_game_")
                    .into_iter()
                    .chain(current.find("\nu64 w2c_game_"))
                    .chain(current.find("\nvoid w2c_game_"))
                    .min();
                let Some(at) = at.filter(|&at| at <= start) else {
                    bail!("{}: no function before it to put the helpers ahead of", insert.function);
                };
                current.insert_str(at + 1, &HELPERS.replace("{M}", M));
                helpers_in = Some(i);
            }
            *text = Some(current);
            done = true;
            break;
        }
        if !done {
            bail!("{}: definition not found in the wasm2c output", insert.function);
        }
    }
    Ok(())
}
