// Hooks for the runtime's `lod_lights.rs` (see ../hooks.rs).
&[
    // The far lights' instance buffers: their writes are named to the D3D
    // layer's upload scan before each unlock (see lod_lights.rs).
    ("render_lod_lights", "CLODLights::RenderLODLights()"),
    ("vertex_buffer_unlock", "rage::grcVertexBufferD3D11::UnlockInternal() const"),
    ("d3d_map_hint", "wasm_d3d_map_hint"),
]
