//! # gc-alloc — GameCube/Wii Heap Allocator
//!
//! **Status: Stub — Milestone 1**
//!
//! This crate will provide a `#[global_allocator]` implementation backed by
//! the MEM1 heap region (`__heap_start` … `__heap_end` from the linker script).
//!
//! ## Planned Design
//!
//! A simple linked-list allocator will be implemented first:
//! - `alloc`: walk free list, split best-fit block.
//! - `dealloc`: insert block back into free list, coalesce neighbours.
//! - Thread safety: single-core (Gekko), so a simple critical-section
//!   (disable/restore interrupts) is sufficient.
//!
//! Once this is in place, `extern crate alloc` and the standard `Vec`, `String`,
//! `Box` types become available in application code.
//!
//! ## See Also
//!
//! - `WIP.md` Milestone 1
//! - `crates/gc-alloc/TODO.md`

#![no_std]

// TODO (Milestone 1): Implement GlobalAlloc

use core::alloc::{GlobalAlloc, Layout};

/// Placeholder allocator — panics on any allocation attempt.
///
/// Replace with a real implementation in Milestone 1.
pub struct GcAllocator;

unsafe impl GlobalAlloc for GcAllocator {
    unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
        // TODO: implement linked-list allocator
        core::ptr::null_mut()
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // TODO: implement
    }
}

/// Set this as the global allocator in your application:
///
/// ```rust,no_run
/// use gc_alloc::GcAllocator;
/// #[global_allocator]
/// static ALLOCATOR: GcAllocator = GcAllocator;
/// ```
pub static ALLOCATOR: GcAllocator = GcAllocator;
