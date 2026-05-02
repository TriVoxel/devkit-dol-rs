//! MMIO base address selection.
//!
//! GameCube and Wii share the same hardware register layout but the
//! virtual address prefix differs:
//!
//! | Platform  | MMIO base | Reason                              |
//! |-----------|-----------|-------------------------------------|
//! | GameCube  | `0xCC00_0000` | DBAT1 maps 0xC0… → physical 0x0C… |
//! | Wii       | `0xCD00_0000` | DBAT maps 0xCD… → physical 0x0D…  |
//!
//! Use [`BASE`] for all MMIO register calculations so a single
//! `--features wii` flag switches the entire HAL.

/// MMIO base address — `0xCC000000` on GC, `0xCD000000` on Wii.
#[cfg(not(feature = "wii"))]
pub const BASE: usize = 0xCC00_0000;

#[cfg(feature = "wii")]
pub const BASE: usize = 0xCD00_0000;

/// Convenience: compute a full MMIO address from a hardware offset.
///
/// ```rust
/// use gc_hal::mmio;
/// let vi_base = mmio::addr(0x002000); // 0xCC002000 on GC
/// ```
#[inline(always)]
pub const fn addr(offset: usize) -> usize { BASE + offset }
