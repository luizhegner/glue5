// Hooks for the runtime's `memory.rs` (see ../hooks.rs).
&[
    // The resource heap grows whenever the streaming engine asks how much
    // memory is left; on low-memory devices the host lets it grow only for
    // allocations (and only calls GetHeapSize, to know the heap's size).
    ("grow_heap", "rage::sysMemGrowBuddyAllocator::GrowHeap()"),
    ("heap_size", "rage::sysMemGrowBuddyAllocator::GetHeapSize() const"),
    ("memory_available", "rage::sysMemGrowBuddyAllocator::GetMemoryAvailable()"),
    ("largest_block", "rage::sysMemGrowBuddyAllocator::GetLargestAvailableBlock()"),
    // Freed memory past the budget goes back to the system (Apple: and
    // comes back when allocated); the size of a block.
    ("buddy_free", "rage::sysMemGrowBuddyAllocator::Free(void const*)"),
    ("buddy_try_allocate", "rage::sysMemGrowBuddyAllocator::TryAllocate(unsigned long, unsigned long, int)"),
    ("buddy_block_size", "rage::sysMemGrowBuddyAllocator::GetSize(void const*) const"),
    // The engine's allocator, for what the host hands the engine (the
    // online stand-ins, the cloud's files).
    ("builtin_malloc", "emscripten_builtin_malloc"),
    ("builtin_free", "emscripten_builtin_free"),
    // libc++'s nothrow operator new (std::stable_sort's buffers: cutscenes'
    // load events, HUD markers), whose blocks rage's operator delete frees:
    // made in rage's heap, through zlib's default allocator (operator new;
    // a hook on operator new itself would be checked at every allocation).
    ("nothrow_new", "operator_new_impl(unsigned long)"),
    ("zcalloc", "zcalloc(void*, unsigned int, unsigned int)"),
]
