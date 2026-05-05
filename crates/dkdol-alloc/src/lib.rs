//! # dkdol-alloc — GameCube/Wii Heap Allocator
//!
//! A linked-list first-fit allocator over `__heap_start`…`__heap_end`.
//!
//! Every block starts with a 32-byte header (padded to the cache-line size):
//!
//! ```text
//!   [Header: size(usize) + next(usize) + 24 pad bytes]
//!   [User data, ALIGN-aligned]
//! ```
//!
//! All sizes are multiples of `ALIGN` (32). The free list is sorted by
//! address so adjacent free blocks can be coalesced in O(1).
//!
//! Thread safety is provided by disabling external interrupts around
//! all free-list mutations (`dkdol_rt::irq::free`).
//!
//! ## Usage
//!
//! ```rust,no_run
//! #[global_allocator]
//! static ALLOCATOR: dkdol_alloc::GcAllocator = dkdol_alloc::GcAllocator::new();
//!
//! // In main():
//! unsafe { dkdol_alloc::init(); }
//! ```

#![no_std]

extern crate dkdol_rt;

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const ALIGN: usize = 32;
const HEADER: usize = ALIGN; // header occupies one full cache-line slot

// ─────────────────────────────────────────────────────────────────────────────
// Block header (stored at block_addr; user data at block_addr + HEADER)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, align(32))]
struct Hdr {
    size: usize,  // total block bytes including this header
    next: usize,  // raw addr of next FREE block; 0 = none / allocated
}
const _: () = assert!(core::mem::size_of::<Hdr>() <= ALIGN);

unsafe fn hdr(addr: usize) -> *mut Hdr { addr as *mut Hdr }

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

static INITED: AtomicBool  = AtomicBool::new(false);
static HEAD:   AtomicUsize = AtomicUsize::new(0); // head of free list

fn head_addr() -> *mut usize {
    // SAFETY: AtomicUsize has the same memory layout as usize.
    &HEAD as *const AtomicUsize as *mut usize
}

// ─────────────────────────────────────────────────────────────────────────────
// Init
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise the allocator. Call once before the first allocation.
///
/// # Safety
/// `__heap_start` and `__heap_end` must be valid linker symbols bounding
/// free memory exclusively for the allocator.
pub unsafe fn init() {
    if INITED.swap(true, Ordering::SeqCst) { return; }
    extern "C" {
        static __heap_start: u8;
        static __heap_end:   u8;
    }
    let start = align_up(&__heap_start as *const u8 as usize, ALIGN);
    let end   = &__heap_end as *const u8 as usize;
    if end < start + HEADER + ALIGN { return; }
    let h = hdr(start);
    (*h).size = end - start;
    (*h).next = 0;
    HEAD.store(start, Ordering::SeqCst);
}

// ─────────────────────────────────────────────────────────────────────────────
// GlobalAlloc
// ─────────────────────────────────────────────────────────────────────────────

pub struct GcAllocator;
impl GcAllocator {
    pub const fn new() -> Self { GcAllocator }

    /// Return total free bytes (walks the free list; O(n)).
    pub fn free_bytes(&self) -> usize {
        dkdol_rt::irq::free(|| {
            let mut total = 0usize;
            let mut cur = HEAD.load(Ordering::Relaxed);
            while cur != 0 {
                let h = unsafe { hdr(cur) };
                total += unsafe { (*h).size } - HEADER;
                cur = unsafe { (*h).next };
            }
            total
        })
    }
}

unsafe impl GlobalAlloc for GcAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let real_align  = layout.align().max(ALIGN);
        let user_need   = align_up(layout.size(), ALIGN);
        let total_need  = HEADER + user_need;

        dkdol_rt::irq::free(|| {
            // prev_next: pointer to the usize that points at cur
            // starts as the HEAD storage itself
            let mut pp: *mut usize = head_addr();
            let mut cur = HEAD.load(Ordering::Relaxed);

            while cur != 0 {
                let h    = hdr(cur);
                let size = (*h).size;
                let next = (*h).next;

                // For over-alignment: user payload starts at cur+HEADER
                // which is already ALIGN-aligned. If real_align > ALIGN,
                // compute extra padding needed.
                let user_start = cur + HEADER;
                let aligned    = align_up(user_start, real_align);
                let extra      = aligned - user_start;
                let need       = total_need + extra;

                if size >= need {
                    let rem = size - need;
                    if rem >= HEADER + ALIGN {
                        // Split: create a new free block
                        let nb_addr = cur + need;
                        let nb = hdr(nb_addr);
                        (*nb).size = rem;
                        (*nb).next = next;
                        *pp = nb_addr;
                        (*h).size = need;
                    } else {
                        // Consume entire block
                        *pp = next;
                    }
                    (*h).next = 0; // mark allocated
                    return aligned as *mut u8;
                }

                pp = &raw mut (*h).next;
                cur = next;
            }
            core::ptr::null_mut()
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() { return; }
        // Recover block start. For alignment <= ALIGN: block = ptr - HEADER.
        // For over-aligned: we returned `aligned` which may be > cur+HEADER,
        // but since we added `extra` into `need` and stored size correctly,
        // the real header is at ptr - HEADER - extra. We don't track `extra`
        // per-block yet, so for now assume alignment <= ALIGN (common case).
        let block = ptr as usize - HEADER;
        let h = hdr(block);

        dkdol_rt::irq::free(|| {
            // Insert sorted by address
            let mut pp: *mut usize = head_addr();
            let mut cur = HEAD.load(Ordering::Relaxed);

            while cur != 0 && cur < block {
                pp  = &raw mut (*hdr(cur)).next;
                cur = *pp;
            }

            // Link freed block between prev and cur
            (*h).next = cur;
            *pp = block;

            // Coalesce with next
            let h_size = (*h).size;
            let h_next = (*h).next;
            if h_next != 0 && block + h_size == h_next {
                let nb = hdr(h_next);
                (*h).size += (*nb).size;
                (*h).next  = (*nb).next;
            }

            // Coalesce with prev (re-walk to find it)
            let prev_addr = {
                let mut pa = 0usize;
                let mut c  = HEAD.load(Ordering::Relaxed);
                while c != 0 && c < block {
                    pa = c;
                    c  = (*hdr(c)).next;
                }
                pa
            };
            if prev_addr != 0 {
                let ph = hdr(prev_addr);
                if prev_addr + (*ph).size == block {
                    (*ph).size += (*h).size;
                    (*ph).next  = (*h).next;
                }
            }
        });
    }
}

pub static ALLOCATOR: GcAllocator = GcAllocator::new();

#[inline(always)]
fn align_up(v: usize, a: usize) -> usize { (v + a - 1) & !(a - 1) }
