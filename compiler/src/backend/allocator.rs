//! Native runtime allocation boundary.
//!
//! Owned standard-library values use this API rather than binding their
//! representation directly to the platform allocator.

pub const C_ALLOCATOR: &str = r#"
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

typedef struct { void *base; size_t size; size_t align; } DispAllocationHeader;

static void disp_allocation_failure(const char *message) {
    fprintf(stderr, "DISP runtime allocation error: %s\n", message);
    exit(101);
}

static void *disp_alloc(size_t size, size_t align) {
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
    return (void *)address;
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
    free(header->base);
}

static void *disp_realloc(void *value, size_t new_size, size_t align) {
    if (!value) return disp_alloc(new_size, align);
    DispAllocationHeader *old = (DispAllocationHeader *)((uintptr_t)value - sizeof(DispAllocationHeader));
    void *replacement = disp_alloc(new_size, align);
    memcpy(replacement, value, old->size < new_size ? old->size : new_size);
    disp_dealloc(value);
    return replacement;
}
"#;
