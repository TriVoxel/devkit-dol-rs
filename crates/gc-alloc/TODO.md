# TODO: gc-alloc — Heap Allocator

## What This Is

`gc-alloc` provides a `#[global_allocator]` implementation backed by MEM1's
heap region (the area between the end of BSS and the bottom of the stack).

The heap region is defined by the linker script symbols:
- `__heap_start` = end of BSS (address after all static data)
- `__heap_end`   = `__stack_bottom` (0x817F0000)

## Implementation Plan (Milestone 1)

### Phase 1: Bump Allocator

Simple, fast, no deallocation:

```rust
static HEAP_PTR: AtomicUsize = AtomicUsize::new(0); // 0 = uninitialised

fn alloc(layout: Layout) -> *mut u8 {
    let start = HEAP_PTR.fetch_add(layout.size(), Ordering::SeqCst);
    if start + layout.size() > HEAP_END { null_mut() } else { start as *mut u8 }
}
```

Sufficient for applications that allocate once at startup and never free.

### Phase 2: Linked-List Allocator

Full alloc+free:
- Free list sorted by address
- First-fit or best-fit block selection
- Coalesce adjacent free blocks on deallocation
- Thread safety: disable/restore interrupts around free-list operations

### Implementation Steps

- [ ] Read `__heap_start` and `__heap_end` from linker symbols via `extern "C"`
- [ ] Implement bump allocator as Phase 1
- [ ] Implement linked-list on top as Phase 2
- [ ] Export `GcAllocator` and document `#[global_allocator]` usage
- [ ] Test: allocate Vec<u8>, fill it, deallocate, verify no memory leak

## Memory Availability

For a typical hello_world DOL (~50 KB code + data + BSS):
- Heap start: ≈ 0x80010000
- Heap end:   0x817F0000
- Available:  ≈ 23 MB — plenty for game assets and data structures
