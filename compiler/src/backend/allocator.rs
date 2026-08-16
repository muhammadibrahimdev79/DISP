//! Native runtime allocation boundary.
//!
//! Owned standard-library values use this API rather than binding their
//! representation directly to the platform allocator.

pub const C_ALLOCATOR: &str = r#"
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

static void dv_panic(const char *message, int line, int column);

typedef struct DispAllocationHeader DispAllocationHeader;
typedef struct DispRollbackHook DispRollbackHook;
struct DispAllocationHeader {
    void *base;
    size_t size;
    size_t align;
    DispAllocationHeader *boundary_previous;
    DispAllocationHeader *boundary_next;
    bool boundary_owned;
};

struct DispRollbackHook {
    DispRollbackHook *previous;
    DispRollbackHook *next;
    void (*cleanup)(void *);
    void *context;
    bool active;
};

static _Thread_local bool disp_ffi_allocation_boundary_active;
static _Thread_local DispAllocationHeader *disp_ffi_boundary_allocations;
static _Thread_local DispRollbackHook *disp_ffi_boundary_rollbacks;
static _Thread_local size_t disp_ffi_boundary_call_depth;

static atomic_size_t disp_runtime_memory_bytes;
static atomic_size_t disp_runtime_steps;
static atomic_size_t disp_runtime_output_bytes;
static atomic_size_t disp_runtime_live_tasks;
static atomic_size_t disp_runtime_live_threads;
static atomic_size_t disp_runtime_process_starts;
static atomic_size_t disp_runtime_live_handles;
static _Thread_local size_t disp_runtime_call_depth;

static size_t disp_runtime_limit(const char *name, size_t fallback) {
    const char *text = getenv(name);
    if (!text || !*text) return fallback;
    size_t value = 0;
    for (const unsigned char *at = (const unsigned char *)text; *at; at++) {
        if (*at < '0' || *at > '9' || value > (SIZE_MAX - (size_t)(*at - '0')) / 10) {
            fprintf(stderr, "DISP runtime resource configuration error: %s must be a positive decimal integer\n", name);
            exit(101);
        }
        value = value * 10 + (size_t)(*at - '0');
    }
    if (!value) {
        fprintf(stderr, "DISP runtime resource configuration error: %s must be greater than zero\n", name);
        exit(101);
    }
    return value;
}

static void disp_resource_failure(const char *resource) {
    char message[160];
    snprintf(message, sizeof(message), "DISP runtime resource limit exceeded: %s", resource);
    dv_panic(message, 0, 0);
}

static void disp_runtime_charge(atomic_size_t *counter, size_t amount, size_t limit, const char *resource) {
    size_t current = atomic_load_explicit(counter, memory_order_relaxed);
    for (;;) {
        if (amount > limit - (current > limit ? limit : current)) disp_resource_failure(resource);
        if (atomic_compare_exchange_weak_explicit(counter, &current, current + amount, memory_order_relaxed, memory_order_relaxed)) return;
    }
}

static void disp_runtime_charge_steps(size_t amount) {
    disp_runtime_charge(&disp_runtime_steps, amount, disp_runtime_limit("DISP_MAX_STEPS", (size_t)DISP_DEFAULT_MAX_STEPS), "execution steps");
}

static void disp_runtime_charge_output(size_t amount) {
    disp_runtime_charge(&disp_runtime_output_bytes, amount, disp_runtime_limit("DISP_MAX_OUTPUT_BYTES", (size_t)DISP_DEFAULT_MAX_OUTPUT_BYTES), "printed output bytes");
}

static void disp_runtime_enter_call(void) {
    size_t limit = disp_runtime_limit("DISP_MAX_CALL_DEPTH", (size_t)DISP_DEFAULT_MAX_CALL_DEPTH);
    if (disp_runtime_call_depth >= limit) disp_resource_failure("call depth");
    disp_runtime_call_depth++;
}

static void disp_runtime_leave_call(void) {
    if (!disp_runtime_call_depth) disp_resource_failure("call-depth accounting");
    disp_runtime_call_depth--;
}

static void disp_runtime_acquire_task(void) {
    disp_runtime_charge(&disp_runtime_live_tasks, 1, disp_runtime_limit("DISP_MAX_TASKS", (size_t)DISP_DEFAULT_MAX_TASKS), "live tasks");
}

static void disp_runtime_release_task(void) {
    atomic_fetch_sub_explicit(&disp_runtime_live_tasks, 1, memory_order_relaxed);
}

static void disp_runtime_acquire_thread(void) {
    disp_runtime_charge(&disp_runtime_live_threads, 1, disp_runtime_limit("DISP_MAX_THREADS", (size_t)DISP_DEFAULT_MAX_THREADS), "live threads");
}

static void disp_runtime_release_thread(void) {
    atomic_fetch_sub_explicit(&disp_runtime_live_threads, 1, memory_order_relaxed);
}

static void disp_runtime_charge_process_start(void) {
    disp_runtime_charge(&disp_runtime_process_starts, 1, disp_runtime_limit("DISP_MAX_PROCESS_STARTS", (size_t)DISP_DEFAULT_MAX_PROCESS_STARTS), "child-process launch attempts");
}

static void disp_runtime_acquire_handle(void) {
    disp_runtime_charge(&disp_runtime_live_handles, 1, disp_runtime_limit("DISP_MAX_HANDLES", (size_t)DISP_DEFAULT_MAX_HANDLES), "live resource handles");
}

static void disp_runtime_release_handle(void) {
    atomic_fetch_sub_explicit(&disp_runtime_live_handles, 1, memory_order_relaxed);
}

static FILE *disp_fopen_metered(const char *path, const char *mode) {
    disp_runtime_acquire_handle();
    FILE *file = fopen(path, mode);
    if (!file) disp_runtime_release_handle();
    return file;
}

static int disp_fclose_metered(FILE *file) {
    int status = fclose(file);
    disp_runtime_release_handle();
    return status;
}

#define fopen disp_fopen_metered
#define fclose disp_fclose_metered

static void disp_allocation_failure(const char *message) {
    dv_panic(message, 0, 0);
}

static void disp_runtime_reserve_memory(size_t size) {
    disp_runtime_charge(&disp_runtime_memory_bytes, size, disp_runtime_limit("DISP_MAX_MEMORY_BYTES", (size_t)DISP_DEFAULT_MAX_MEMORY_BYTES), "managed memory bytes");
}

static void disp_runtime_release_memory(size_t size) {
    if (size) atomic_fetch_sub_explicit(&disp_runtime_memory_bytes, size, memory_order_relaxed);
}

static void *disp_alloc_unmetered(size_t size, size_t align) {
    if (align == 0 || (align & (align - 1)) != 0) disp_allocation_failure("alignment must be a non-zero power of two");
    if (align < sizeof(void *)) align = sizeof(void *);
    size_t extra;
    if (__builtin_add_overflow(sizeof(DispAllocationHeader), align - 1, &extra) ||
        __builtin_add_overflow(size, extra, &extra)) disp_allocation_failure("allocation size overflow");
    void *base = malloc(extra ? extra : 1);
    if (!base) disp_allocation_failure("out of memory");
    uintptr_t address = ((uintptr_t)base + sizeof(DispAllocationHeader) + align - 1) & ~((uintptr_t)align - 1);
    DispAllocationHeader *header = (DispAllocationHeader *)(address - sizeof(DispAllocationHeader));
    header->base = base; header->size = size; header->align = align;
    header->boundary_previous = NULL; header->boundary_next = NULL; header->boundary_owned = false;
    return (void *)address;
}

static void disp_ffi_track_allocation(void *value) {
    if (!value || !disp_ffi_allocation_boundary_active) return;
    DispAllocationHeader *header = (DispAllocationHeader *)((uintptr_t)value - sizeof(DispAllocationHeader));
    header->boundary_owned = true;
    header->boundary_previous = NULL;
    header->boundary_next = disp_ffi_boundary_allocations;
    if (disp_ffi_boundary_allocations) disp_ffi_boundary_allocations->boundary_previous = header;
    disp_ffi_boundary_allocations = header;
}

static void disp_ffi_untrack_allocation(DispAllocationHeader *header) {
    if (!header || !header->boundary_owned) return;
    if (header->boundary_previous) header->boundary_previous->boundary_next = header->boundary_next;
    else disp_ffi_boundary_allocations = header->boundary_next;
    if (header->boundary_next) header->boundary_next->boundary_previous = header->boundary_previous;
    header->boundary_previous = NULL; header->boundary_next = NULL; header->boundary_owned = false;
}

static void disp_ffi_track_rollback(DispRollbackHook *hook, void (*cleanup)(void *), void *context) {
    if (!hook) return;
    *hook = (DispRollbackHook){0};
    if (!disp_ffi_allocation_boundary_active) return;
    hook->cleanup = cleanup;
    hook->context = context;
    hook->active = true;
    hook->next = disp_ffi_boundary_rollbacks;
    if (disp_ffi_boundary_rollbacks) disp_ffi_boundary_rollbacks->previous = hook;
    disp_ffi_boundary_rollbacks = hook;
}

static void disp_ffi_untrack_rollback(DispRollbackHook *hook) {
    if (!hook || !hook->active) return;
    if (hook->previous) hook->previous->next = hook->next;
    else disp_ffi_boundary_rollbacks = hook->next;
    if (hook->next) hook->next->previous = hook->previous;
    hook->previous = NULL;
    hook->next = NULL;
    hook->active = false;
}

static void disp_ffi_allocation_boundary_begin(void) {
    if (disp_ffi_allocation_boundary_active || disp_ffi_boundary_allocations || disp_ffi_boundary_rollbacks)
        dv_panic("nested DISP allocation boundary", 0, 0);
    disp_ffi_allocation_boundary_active = true;
    disp_ffi_boundary_call_depth = disp_runtime_call_depth;
}

static void disp_ffi_allocation_boundary_abort(void) {
    DispRollbackHook *hook = disp_ffi_boundary_rollbacks;
    disp_ffi_boundary_rollbacks = NULL;
    while (hook) {
        DispRollbackHook *next = hook->next;
        hook->active = false;
        if (hook->cleanup) hook->cleanup(hook->context);
        free(hook);
        hook = next;
    }
    DispAllocationHeader *header = disp_ffi_boundary_allocations;
    disp_ffi_boundary_allocations = NULL;
    disp_ffi_allocation_boundary_active = false;
    disp_runtime_call_depth = disp_ffi_boundary_call_depth;
    while (header) {
        DispAllocationHeader *next = header->boundary_next;
        disp_runtime_release_memory(header->size);
        free(header->base);
        header = next;
    }
}

static void disp_ffi_allocation_boundary_finish(void) {
    if (disp_ffi_boundary_allocations || disp_ffi_boundary_rollbacks) {
        disp_ffi_allocation_boundary_abort();
        dv_panic("DISP export returned with live owned allocations or rollback resources", 0, 0);
    }
    if (disp_runtime_call_depth != disp_ffi_boundary_call_depth) {
        disp_ffi_allocation_boundary_abort();
        dv_panic("DISP export returned with unbalanced call-depth state", 0, 0);
    }
    disp_ffi_allocation_boundary_active = false;
}

static void *disp_alloc(size_t size, size_t align) {
    disp_runtime_reserve_memory(size);
    void *value = disp_alloc_unmetered(size, align);
    disp_ffi_track_allocation(value);
    return value;
}

static void *disp_alloc_zeroed(size_t count, size_t size, size_t align) {
    size_t bytes;
    if (__builtin_mul_overflow(count, size, &bytes)) disp_allocation_failure("allocation size overflow");
    void *value = disp_alloc(bytes, align);
    if (bytes) memset(value, 0, bytes);
    return value;
}

static void disp_dealloc(void *value) {
    if (!value) return;
    DispAllocationHeader *header = (DispAllocationHeader *)((uintptr_t)value - sizeof(DispAllocationHeader));
    disp_ffi_untrack_allocation(header);
    disp_runtime_release_memory(header->size);
    free(header->base);
}

static void *disp_realloc(void *value, size_t new_size, size_t align) {
    if (!value) return disp_alloc(new_size, align);
    DispAllocationHeader *old = (DispAllocationHeader *)((uintptr_t)value - sizeof(DispAllocationHeader));
    if (new_size > old->size) disp_runtime_reserve_memory(new_size - old->size);
    void *replacement = disp_alloc_unmetered(new_size, align);
    memcpy(replacement, value, old->size < new_size ? old->size : new_size);
    if (new_size < old->size) disp_runtime_release_memory(old->size - new_size);
    bool tracked = old->boundary_owned;
    disp_ffi_untrack_allocation(old);
    free(old->base);
    if (tracked) disp_ffi_track_allocation(replacement);
    return replacement;
}
"#;
