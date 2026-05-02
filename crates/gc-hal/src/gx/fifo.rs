//! GX FIFO — circular command buffer setup.
//!
//! The GX pipeline is fed by a circular FIFO buffer in MEM1. The CPU writes
//! commands via the write-gather pipe; the GP reads and executes them.
//!
//! In the simplest "linked" mode (CPU FIFO = GP FIFO), both the CPU write
//! pointer and the GP read pointer chase each other around the same buffer.
//! This is what we use here.
//!
//! ## Register layout
//!
//! CP registers (16-bit) at `0xCC000000`:
//! - `[1]`: Control register (bit 0 = read enable, bit 4 = CPU/GP link)
//! - `[2]`: Clear/interrupt register
//! - `[16]/[17]`: FIFO base address (lo/hi u16 halves of physical addr)
//! - `[18]/[19]`: FIFO end address
//! - `[20]/[21]`: High watermark
//! - `[22]/[23]`: Low watermark
//! - `[24]/[25]`: Read-write distance (bytes between wr and rd pointers)
//! - `[26]/[27]`: Write pointer
//! - `[28]/[29]`: Read pointer
//!
//! PI registers (32-bit) at `0xCC003000`:
//! - `[3]`: CPU FIFO base
//! - `[4]`: CPU FIFO end
//! - `[5]`: CPU FIFO write pointer

pub const CP_BASE: usize = crate::mmio::addr(0x000000);
pub const PI_BASE: usize = crate::mmio::addr(0x003000);

/// Minimum FIFO size: 64 KB.
pub const FIFO_MIN_SIZE: usize = 64 * 1024;

/// Default high-watermark: 16 KB from the end of the buffer.
pub const FIFO_HI_WATERMARK: usize = 16 * 1024;

/// Default low-watermark: same as high-watermark (simple setup).
pub const FIFO_LO_WATERMARK: usize = FIFO_HI_WATERMARK;

#[inline(always)]
fn cp(idx: usize) -> *mut u16 { (CP_BASE + idx * 2) as *mut u16 }
#[inline(always)]
fn pi(idx: usize) -> *mut u32 { (PI_BASE + idx * 4) as *mut u32 }

/// Initialise the GX FIFO.
///
/// `buf_phys` must be the **physical** address of the FIFO buffer (i.e.
/// virtual addr minus 0x80000000 for cached MEM1 pointers).
/// `size` must be ≥ [`FIFO_MIN_SIZE`] and a multiple of 32.
///
/// After this call:
/// - CP registers hold the FIFO base, end, watermarks, and initial pointers
/// - PI write-pointer register is set
/// - FIFO read is enabled and CPU/GP are linked
///
/// # Safety
/// The buffer `buf_phys..buf_phys+size` must be exclusively owned by the FIFO.
pub unsafe fn init(buf_virt: *mut u8, size: usize) {
    assert!(size >= FIFO_MIN_SIZE, "FIFO too small");
    assert!(size % 32 == 0, "FIFO size must be 32-byte aligned");
    assert!(buf_virt as usize % 32 == 0, "FIFO buffer must be 32-byte aligned");

    // Convert virtual (cached) address to physical: strip top byte.
    let phys_base = (buf_virt as usize) & 0x1FFF_FFFF;
    let phys_end  = phys_base + size - 32; // inclusive end = last valid 32-byte slot

    let hi_mark = size - FIFO_HI_WATERMARK;
    let lo_mark = FIFO_LO_WATERMARK;

    // ── Disable FIFO read and interrupts before reconfiguring ────────────
    // CP[1] bit 0: read enable. CP[1] bit 2,3: int hi/lo enable. Bit 4: linked.
    let ctrl = core::ptr::read_volatile(cp(1));
    core::ptr::write_volatile(cp(1), ctrl & !0x1F); // clear bits 0-4

    // Clear interrupt/overflow flags (CP[2])
    core::ptr::write_volatile(cp(2), 0x0003);

    // ── Program CP FIFO registers ────────────────────────────────────────
    // Base address
    core::ptr::write_volatile(cp(16), (phys_base & 0xFFFF) as u16);
    core::ptr::write_volatile(cp(17), (phys_base >> 16) as u16);
    // End address
    core::ptr::write_volatile(cp(18), (phys_end & 0xFFFF) as u16);
    core::ptr::write_volatile(cp(19), (phys_end >> 16) as u16);
    // High watermark
    core::ptr::write_volatile(cp(20), (hi_mark & 0xFFFF) as u16);
    core::ptr::write_volatile(cp(21), (hi_mark >> 16) as u16);
    // Low watermark
    core::ptr::write_volatile(cp(22), (lo_mark & 0xFFFF) as u16);
    core::ptr::write_volatile(cp(23), (lo_mark >> 16) as u16);
    // Read-write distance = 0 (empty)
    core::ptr::write_volatile(cp(24), 0);
    core::ptr::write_volatile(cp(25), 0);
    // Write pointer = base
    core::ptr::write_volatile(cp(26), (phys_base & 0xFFFF) as u16);
    core::ptr::write_volatile(cp(27), (phys_base >> 16) as u16);
    // Read pointer = base
    core::ptr::write_volatile(cp(28), (phys_base & 0xFFFF) as u16);
    core::ptr::write_volatile(cp(29), (phys_base >> 16) as u16);

    // ── Program PI FIFO registers ────────────────────────────────────────
    core::ptr::write_volatile(pi(3), phys_base as u32);
    core::ptr::write_volatile(pi(4), phys_end  as u32);
    core::ptr::write_volatile(pi(5), phys_base as u32); // initial write ptr

    // ── PowerPC sync before re-enabling ──────────────────────────────────
    core::arch::asm!("sync", options(nostack, nomem));

    // ── Enable: read enable (bit 0) + hi watermark int (bit 2) + link (bit 4)
    core::ptr::write_volatile(cp(1), 0x11); // read enable | linked (bit 0 | bit 4)
    // Reset overflow/underflow interrupt status
    core::ptr::write_volatile(cp(2), 0x0003);
}

/// Wait until the GP has finished reading all commands (FIFO empty).
///
/// Polls the CP read-write distance until it reaches zero.
pub unsafe fn drain() {
    loop {
        let lo = core::ptr::read_volatile(cp(24)) as u32;
        let hi = core::ptr::read_volatile(cp(25)) as u32;
        let dist = (hi << 16) | lo;
        if dist == 0 { break; }
    }
}
