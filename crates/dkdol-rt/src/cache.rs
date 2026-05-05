//! Cache management for the Gekko/Broadway PowerPC CPU.
//!
//! The Gekko has split L1 caches:
//! - 32 KB 8-way set-associative **data cache**
//! - 32 KB 8-way set-associative **instruction cache**
//! - 256 KB unified **L2 cache** (optional, controlled by HID0[L2E])
//!
//! Cache line size is **32 bytes** for both L1 caches.
//!
//! ## Why This Matters
//!
//! The Video Interface (VI) hardware DMA reads directly from physical RAM to
//! display the XFB (external framebuffer). If the CPU writes pixel data through
//! the L1 data cache without flushing, the hardware will read stale data.
//!
//! Similarly, the GX GPU FIFO is a write-gather pipe that bypasses the cache
//! entirely, but any DMA source buffers (textures, display lists) must be
//! flushed before submitting to the GPU.

/// Size of a single cache line in bytes. All cache operations are granular to
/// this boundary.
pub const CACHE_LINE_SIZE: usize = 32;

/// Flush (write-back and invalidate) a single data cache line containing `addr`.
///
/// Use this after writing to memory that will be read by hardware DMA (VI, GX, DSP).
///
/// # Safety
///
/// `addr` must point to memory that is valid and mapped with write-back caching.
/// Flushing a cache-inhibited region (WIMG bit 1 set) is undefined behaviour.
#[inline(always)]
pub unsafe fn dcbf(addr: *const u8) {
    core::arch::asm!(
        "dcbf 0, {r}",
        r = in(reg) addr,
        options(nostack, preserves_flags)
    );
}

/// Flush (write-back and invalidate) a range of data cache lines covering
/// `[ptr, ptr + len)`.
///
/// The range is rounded outward to cache-line boundaries automatically.
///
/// # Safety
///
/// `ptr` must be valid for `len` bytes of reads and the memory must use
/// write-back caching.
pub unsafe fn dcbf_range(ptr: *const u8, len: usize) {
    if len == 0 { return; }
    let start = ptr as usize & !(CACHE_LINE_SIZE - 1);
    let end   = (ptr as usize + len + CACHE_LINE_SIZE - 1) & !(CACHE_LINE_SIZE - 1);
    let mut addr = start;
    while addr < end {
        dcbf(addr as *const u8);
        addr += CACHE_LINE_SIZE;
    }
    // Ensure all stores are visible to hardware before returning.
    sync();
}

/// Store (write-back without invalidate) a single data cache line.
///
/// Cheaper than `dcbf` when you don't need the line evicted from cache.
#[inline(always)]
pub unsafe fn dcbst(addr: *const u8) {
    core::arch::asm!(
        "dcbst 0, {r}",
        r = in(reg) addr,
        options(nostack, preserves_flags)
    );
}

/// Invalidate (without write-back) a single data cache line.
///
/// Use only when you know the cached data is stale and you don't need to
/// write it back (e.g. a DMA destination buffer that was just filled by hardware).
#[inline(always)]
pub unsafe fn dcbi(addr: *const u8) {
    core::arch::asm!(
        "dcbi 0, {r}",
        r = in(reg) addr,
        options(nostack, preserves_flags)
    );
}

/// Invalidate a data cache range. No write-back is performed.
///
/// # Safety
///
/// Any dirty cache lines in this range will be silently discarded. Only safe
/// when you know the CPU hasn't written data to this range since the last flush.
pub unsafe fn dcbi_range(ptr: *const u8, len: usize) {
    if len == 0 { return; }
    let start = ptr as usize & !(CACHE_LINE_SIZE - 1);
    let end   = (ptr as usize + len + CACHE_LINE_SIZE - 1) & !(CACHE_LINE_SIZE - 1);
    let mut addr = start;
    while addr < end {
        dcbi(addr as *const u8);
        addr += CACHE_LINE_SIZE;
    }
}

/// Invalidate a single instruction cache line.
///
/// Required after writing self-modifying code or loading code into RAM via DMA.
#[inline(always)]
pub unsafe fn icbi(addr: *const u8) {
    core::arch::asm!(
        "icbi 0, {r}",
        r = in(reg) addr,
        options(nostack, preserves_flags)
    );
}

/// Emit a `sync` instruction (heavyweight memory barrier).
///
/// Ensures all previous stores are globally visible before this point.
/// Required after `dcbf` / `dcbst` sequences that precede hardware DMA.
#[inline(always)]
pub fn sync() {
    unsafe {
        core::arch::asm!("sync", options(nostack, preserves_flags));
    }
}

/// Emit an `isync` instruction (instruction synchronisation barrier).
///
/// Flushes the instruction pipeline. Required after modifying MSR, HID0, or
/// other special-purpose registers that affect instruction fetch.
#[inline(always)]
pub fn isync() {
    unsafe {
        core::arch::asm!("isync", options(nostack, preserves_flags));
    }
}
