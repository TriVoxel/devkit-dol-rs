//! Raw VI register access.
//!
//! The VI registers are 16-bit, volatile, memory-mapped at `0xCC002000`.
//! The uncached BAT (DBAT1) covers `0xC0000000–0xCFFFFFFF`, so hardware
//! register addresses in `0xCC0xxxxx` are already cache-inhibited by the
//! BAT configuration and do not need `dcbf`.

/// Physical base address of the Video Interface register block.
const VI_BASE: usize = crate::mmio::addr(0x002000);

/// Number of 16-bit VI registers.
const VI_REG_COUNT: usize = 64;

/// The VI register block, accessed as a newtype around the hardware address.
///
/// Construct via `VI_REGS` (a raw pointer to the register block) and
/// dereference only in `unsafe` code.
#[repr(C)]
pub struct ViRegs {
    regs: [u16; VI_REG_COUNT],
}

/// Pointer to the VI register block.
///
/// In Rust, `volatile` access is modelled by using `core::ptr::write_volatile` /
/// `read_volatile`. The `ViRegs::write` / `read` methods wrap these.
/// Sync wrapper for a raw hardware pointer. Safe on bare-metal single-core.
pub struct SyncPtr<T>(*mut T);
unsafe impl<T> Sync for SyncPtr<T> {}
impl<T> SyncPtr<T> { #[inline] pub fn get(&self) -> *mut T { self.0 } }
pub static VI_REGS: SyncPtr<ViRegs> = SyncPtr(VI_BASE as *mut ViRegs);

impl ViRegs {
    /// Read VI register `n` with a volatile load.
    ///
    /// # Safety
    ///
    /// Caller must hold exclusive access to the VI hardware.
    #[inline(always)]
    pub unsafe fn read(&self, n: usize) -> u16 {
        debug_assert!(n < VI_REG_COUNT);
        core::ptr::read_volatile(&self.regs[n])
    }

    /// Write VI register `n` with a volatile store.
    ///
    /// # Safety
    ///
    /// Caller must hold exclusive access to the VI hardware.
    #[inline(always)]
    pub unsafe fn write(&self, n: usize, val: u16) {
        debug_assert!(n < VI_REG_COUNT);
        core::ptr::write_volatile(core::ptr::addr_of!(self.regs[n]) as *mut u16, val);
    }
}
