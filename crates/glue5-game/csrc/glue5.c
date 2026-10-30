#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "game.h"
#include "glue5.h"
#include "wasm-rt-exceptions.h"
#include "wasm-rt-impl.h"

void glue5_init(void) {
  wasm_rt_init();
}

static glue5_wait32_fn glue5_wait32;
static glue5_wait64_fn glue5_wait64;
static glue5_notify_fn glue5_notify;

void glue5_set_atomics(glue5_wait32_fn wait32, glue5_wait64_fn wait64, glue5_notify_fn notify) {
  glue5_wait32 = wait32;
  glue5_wait64 = wait64;
  glue5_notify = notify;
}

uint32_t wasm_rt_atomic_wait32(void* memory, uint64_t addr, uint32_t expected, int64_t timeout_ns) {
  return glue5_wait32(memory, addr, expected, timeout_ns);
}

uint32_t wasm_rt_atomic_wait64(void* memory, uint64_t addr, uint64_t expected, int64_t timeout_ns) {
  return glue5_wait64(memory, addr, expected, timeout_ns);
}

uint32_t wasm_rt_atomic_notify(void* memory, uint64_t addr, uint32_t count) {
  return glue5_notify(memory, addr, count);
}

size_t glue5_memory_struct_size(void) {
  return sizeof(wasm_rt_shared_memory_t);
}

/* The host owns the allocation (reserved up front, so the base never moves
 * while other threads run); this just describes it to the generated code. */
void glue5_memory_init(wasm_rt_shared_memory_t* memory,
                       uint8_t* data,
                       uint64_t pages,
                       uint64_t max_pages) {
  memset(memory, 0, sizeof *memory);
  memory->data = (_Atomic volatile uint8_t*)data;
  memory->data_end = (_Atomic volatile uint8_t*)(data + max_pages * WASM_DEFAULT_PAGE_SIZE);
  memory->page_size = WASM_DEFAULT_PAGE_SIZE;
  memory->pages = pages;
  memory->max_pages = max_pages;
  memory->size = pages * WASM_DEFAULT_PAGE_SIZE;
  memory->is64 = true;
#if defined(_WIN32)
  InitializeCriticalSection(&memory->mem_lock);
#else
  pthread_mutex_init(&memory->mem_lock, NULL);
#endif
}

wasm_rt_shared_memory_t* w2c_env_memory(struct w2c_env* env) {
  return env->memory;
}

static const void* const* glue5_host_fns;
static uint32_t glue5_host_count;

void glue5_set_host_functions(const void* const* fns, uint32_t count) {
  glue5_host_fns = fns;
  glue5_host_count = count;
}

/* The one function table every thread's instance uses: indirect calls pass
 * the caller's instance (WASM_RT_NONCONFORMING_CALLER_INSTANCE), so the
 * entries' instances do not matter and the per-instance copies (~2.9 MB
 * each, one per engine thread) need not be kept. The first instance's
 * table, made by the main thread before any other thread starts. */
static wasm_rt_funcref_t* glue5_shared_table;
static uint32_t glue5_shared_table_size;

/* The host's functions after the module's in the table, each called with the
 * calling thread's instance first: the game reaches them through function
 * pointers, e.g. in vtables the host builds. Returns the first one's index. */
static uint32_t glue5_append_host_functions(wasm_rt_funcref_table_t* table) {
  uint32_t base = table->size;
  if (!glue5_host_count) {
    return base;
  }
  wasm_rt_funcref_t* data = realloc(table->data, (base + glue5_host_count) * sizeof *data);
  if (!data) {
    abort();
  }
  for (uint32_t i = 0; i < glue5_host_count; i++) {
    data[base + i] = (wasm_rt_funcref_t){NULL, (wasm_rt_function_ptr_t)glue5_host_fns[i], {NULL}, NULL};
  }
  table->data = data;
  table->size = table->max_size = base + glue5_host_count;
  return base;
}

uint32_t glue5_host_functions_base;

/* Points `instance` at the shared table, freeing the copy instantiation made
 * (the first instance's becomes the shared one, host functions appended). */
static void glue5_share_table(w2c_game* instance) {
  wasm_rt_funcref_table_t* table = w2c_game_0x5F_indirect_function_table(instance);
  if (!glue5_shared_table) {
    glue5_host_functions_base = glue5_append_host_functions(table);
    glue5_shared_table = table->data;
    glue5_shared_table_size = table->size;
    return;
  }
  free(table->data);
  table->data = glue5_shared_table;
  table->size = table->max_size = glue5_shared_table_size;
}

/* Creates this thread's instance; runs the start function (which initialises
 * memory only on the first instance). Returns the trap code. */
uint32_t glue5_instance_new(struct w2c_env* env) {
  w2c_game* instance = calloc(1, sizeof *instance);
  if (!instance) {
    abort();
  }
  env->instance = instance;
  GLUE5_TRY(wasm2c_game_instantiate(instance, env, (struct w2c_wasi__snapshot__preview1*)env);
            glue5_share_table(instance));
}

void glue5_instance_free(struct w2c_env* env) {
  if (env->instance) {
    /* The shared table stays (other threads use it). */
    wasm_rt_funcref_table_t* table = w2c_game_0x5F_indirect_function_table(env->instance);
    if (table->data == glue5_shared_table) {
      table->data = NULL;
    }
    wasm2c_game_free(env->instance);
    free(env->instance);
    env->instance = NULL;
  }
}

/* Calls table function `index` as `void* f(void*)` (a pthread start routine). */
uint32_t glue5_call_ptr_ptr(struct w2c_env* env, uint64_t index, uint64_t arg, uint64_t* out) {
  wasm_rt_funcref_table_t* table = w2c_game_0x5F_indirect_function_table(env->instance);
  if (index >= table->size || !table->data[index].func) {
    fprintf(stderr, "glue5: bad function index %llu\n", (unsigned long long)index);
    abort();
  }
  const wasm_rt_funcref_t* f = &table->data[index];
  GLUE5_TRY(*out = ((u64(*)(void*, u64))f->func)(env->instance, arg));
}

/* Calls table function `index` as a script command's handler,
 * `void f(rage::scrThread::Info&)`. A trap inside returns its code, with
 * the wasm stack pointer put back: the frames it skipped never popped
 * theirs. */
uint32_t glue5_call_native(struct w2c_env* env, uint64_t index, uint64_t info) {
  wasm_rt_funcref_table_t* table = w2c_game_0x5F_indirect_function_table(env->instance);
  if (index >= table->size || !table->data[index].func) {
    return WASM_RT_TRAP_CALL_INDIRECT;
  }
  const wasm_rt_funcref_t* f = &table->data[index];
  u64 sp = env->instance->w2c_0x5F_stack_pointer;
  void* longjmp_target = env->longjmp_target;
  wasm_rt_jmp_buf saved = g_wasm_rt_jmp_buf;
  wasm_rt_trap_t trap = wasm_rt_impl_try();
  if (trap == WASM_RT_TRAP_NONE) {
    ((void (*)(void*, u64))f->func)(env->instance, info);
  } else {
    env->instance->w2c_0x5F_stack_pointer = sp;
    env->longjmp_target = longjmp_target;
  }
  g_wasm_rt_jmp_buf = saved;
  return (uint32_t)trap;
}

/* The module's thread-local storage (`__tls_base`): the engine keeps the
 * running script thread there (`rage::scrThread::GetActiveThread`). */
uint64_t glue5_tls_base(struct w2c_env* env) {
  return env->instance->w2c_0x5F_tls_base;
}

/* Emscripten's longjmp: back to the innermost invoke_* frame. */
_Noreturn void glue5_longjmp(struct w2c_env* env) {
  if (!env->longjmp_target) {
    fprintf(stderr, "glue5: longjmp without a setjmp frame\n");
    abort();
  }
  GLUE5_LONGJMP(*(jmp_buf*)env->longjmp_target, 1);
}

_Noreturn void glue5_unwind(void) {
  WASM_RT_LONGJMP(g_wasm_rt_jmp_buf, GLUE5_TRAP_UNWIND);
  abort();
}
