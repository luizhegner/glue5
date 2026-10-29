//! The pipelines known before the game starts (the app's seed and this
//! device's recordings, see `pipeline.rs`), compiled on every core while the
//! start page shows how far it got, then handed to the renderer: the world
//! never shows with them missing.
//!
//! Compiling a pipeline the driver has not seen takes long on a phone
//! (~200 ms of CPU on an iPhone 13 Pro, 100 times a warm start's): the first
//! start waits minutes, later ones about a second. Metal keeps what it
//! compiled in its system cache, Vulkan in the pipeline cache saved in the
//! cache directory. Pipelines the game asks for that were never recorded
//! still compile in the background while playing; their draws are skipped
//! meanwhile.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use anyhow::Result;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ops::{Blits, Computes};
use crate::pipeline::{self, Cell, Compiler, ComputeRecipe, Recipe};
use crate::shader::{Library, Program, Stage};
use crate::{Device, Settings};

/// What the renderer takes over: the shaders, the compiler, the pipelines
/// compiled (render ones by recipe for this device) and the recipes known
/// (as recorded).
pub struct Prepared {
    pub library: Arc<Library>,
    pub compiler: Arc<Compiler>,
    pub pipes: FxHashMap<Arc<Recipe>, Arc<Cell>>,
    pub known: FxHashSet<Arc<Recipe>>,
    pub computes: Arc<Computes>,
    pub known_computes: FxHashSet<ComputeRecipe>,
    pub blits: Blits,
}

/// Pipelines compiled so far, of how many.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// Opens the game's shaders and compiles the known pipelines, on all cores.
pub(crate) fn prepare(gpu: &Device, settings: &Settings, progress: &Progress) -> Result<Prepared> {
    let t0 = Instant::now();
    let library = Arc::new(Library::open(&settings.shader_dir)?);
    let compiler = Arc::new(crate::compiler(gpu, settings));
    let known = pipeline::known_recipes(settings.cache_dir.as_deref());
    // Recipes from other devices can make the same pipeline here.
    let mut recipes: Vec<Arc<Recipe>> = known.render.iter().map(|r| Arc::new(r.for_device(&compiler.caps))).collect();
    let mut seen = FxHashSet::default();
    recipes.retain(|r| seen.insert(r.clone()));
    let mut compute_recipes: Vec<ComputeRecipe> = known.compute.iter().map(|r| r.for_device(&compiler.caps)).collect();
    let mut seen = FxHashSet::default();
    compute_recipes.retain(|r| seen.insert(r.clone()));
    progress.total.store(recipes.len() + compute_recipes.len(), Ordering::Relaxed);
    let cells: Vec<Arc<Cell>> = recipes.iter().map(|_| Arc::new(OnceLock::new())).collect();
    let computes = Arc::new(Computes::default());
    let next = AtomicUsize::new(0);
    let threads = glue5_runtime::cores();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if let Some(r) = recipes.get(i) {
                        let pipe = programs(&library, &gpu.device, r)
                            .and_then(|(vs, ps)| compiler.compile(&vs, ps.as_deref(), r));
                        let _ = cells[i].set(pipe);
                    } else if let Some(r) = compute_recipes.get(i - recipes.len()) {
                        compute(&library, &compiler, &computes, r);
                    } else {
                        break;
                    }
                    progress.done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    let blits = Blits::new(&compiler);
    // The modules were needed to compile only; the game makes them again
    // for new pipelines (see `shader::Modules`).
    let mut modules = Vec::new();
    library.take_idle_modules(0, usize::MAX, &mut modules);
    drop(modules);
    let failed = cells.iter().filter(|c| matches!(c.get(), Some(None))).count();
    log::info!(
        "gpu: {} known pipelines prepared in {:.1} s ({} recipes, {failed} not usable; {} compute; {threads} threads)",
        recipes.len(),
        t0.elapsed().as_secs_f64(),
        known.render.len(),
        compute_recipes.len(),
    );
    // Saved now rather than while playing, where writing it takes long.
    if let Some(dir) = &settings.cache_dir {
        crate::save_pipeline_cache(&compiler, &gpu.adapter, dir);
    }
    Ok(Prepared {
        library,
        compiler,
        pipes: recipes.into_iter().zip(cells).collect(),
        known: known.render.into_iter().collect(),
        computes,
        known_computes: known.compute.into_iter().collect(),
        blits,
    })
}

/// The programs of a recipe, prepared as when the engine creates them and
/// made into the variants the renderer picks for what is bound (the same
/// objects: `Program::for_bindings` makes each once).
fn programs(library: &Library, device: &wgpu::Device, r: &Recipe) -> Option<(Arc<Program>, Option<Arc<Program>>)> {
    let program =
        |hash: u64, stage: Stage| library.slot(hash).get_or_init(|| library.prepare(device, hash, stage)).clone();
    let [vs_unfiltered, vs_depth, ps_unfiltered, ps_depth] = r.variant;
    let vs = program(r.vs, Stage::Vertex)?.for_bindings(device, vs_unfiltered & !vs_depth, vs_depth);
    let ps = match r.ps {
        Some(hash) => {
            let ps = program(hash, Stage::Pixel)?;
            let ps = if r.hud_mask { ps.for_hud_mask(device)? } else { ps };
            Some(ps.for_bindings(device, ps_unfiltered & !ps_depth, ps_depth))
        }
        None => None,
    };
    Some((vs, ps))
}

/// A compute recipe's pipeline, made as a dispatch makes it (the same
/// program variant, the same storage formats).
fn compute(library: &Library, compiler: &Compiler, computes: &Computes, r: &ComputeRecipe) {
    let device = &compiler.device;
    let Some(cs) = library.slot(r.cs).get_or_init(|| library.prepare(device, r.cs, Stage::Compute)).clone() else {
        return;
    };
    let [unfiltered, depth] = r.variant;
    let cs = cs.for_bindings(device, unfiltered & !depth, depth);
    let subs = cs.storage_formats(|binding| r.storage.iter().find(|(b, _)| *b == binding).map(|&(_, f)| f));
    computes.get(compiler, &cs, &subs);
}
