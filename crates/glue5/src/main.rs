//! Desktop entry point: `glue5 [game dir]`.

/// The renderer and wgpu allocate and free many blocks every frame, and
/// Windows' heap is slower at it: the game's benchmark ran 4-7 % faster with
/// mimalloc.
#[cfg(windows)]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    glue5_runtime::exit::process(glue5::main());
}
