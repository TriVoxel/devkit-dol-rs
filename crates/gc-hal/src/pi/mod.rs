//! Processor Interface (PI) — interrupt controller.
//!
//! The PI aggregates all hardware interrupt sources into the CPU's external
//! interrupt line. It has two 32-bit registers:
//!
//! - `PI_INTSR` (crate::mmio::addr(0x003000)): Interrupt source register. Each bit represents
//!   one hardware source. **Write 1 to clear a pending interrupt.**
//! - `PI_INTMR` (0xCC003004): Interrupt mask register. **Write 1 to enable**
//!   a source (so it can drive the CPU interrupt line).
//!
//! ## Interrupt Bit Positions (0 = MSB = bit 31)
//!
//! | Bit | Name        | Source                        |
//! |-----|-------------|-------------------------------|
//! |  0  | MEM0        | MI error 0                    |
//! |  1  | MEM1        | MI error 1                    |
//! |  2  | MEM2        | MI error 2                    |
//! |  3  | MEM3        | MI error 3                    |
//! |  4  | MEMADDRESS  | MI address error              |
//! |  5  | DSP_AI      | DSP audio interface           |
//! |  6  | DSP_ARAM    | DSP ARAM DMA                  |
//! |  7  | DSP_DSP     | DSP coprocessor               |
//! |  8  | AI          | Audio Interface (streaming)   |
//! |  9  | EXI0_EXI    | EXI channel 0                 |
//! | 10  | EXI0_TC     | EXI channel 0 TC              |
//! | 11  | EXI0_EXT    | EXI channel 0 ext             |
//! | 12  | EXI1_EXI    | EXI channel 1                 |
//! | 13  | EXI1_TC     | EXI channel 1 TC              |
//! | 14  | EXI1_EXT    | EXI channel 1 ext             |
//! | 15  | EXI2_EXI    | EXI channel 2                 |
//! | 16  | EXI2_TC     | EXI channel 2 TC              |
//! | 17  | PI_CP       | GX command processor          |
//! | 18  | PI_PETOKEN  | Pixel engine token            |
//! | 19  | PI_PEFINISH | Pixel engine finish           |
//! | 20  | PI_SI       | Serial Interface              |
//! | 21  | PI_DI       | DVD Interface                 |
//! | 22  | PI_RSW      | Reset switch                  |
//! | 23  | PI_ERROR    | PI error                      |
//! | 24  | PI_VI       | Video Interface (vsync)       |
//! | 25  | PI_DEBUG    | Debug interface               |
//! | 26  | PI_HSP      | High-speed port               |

pub const PI_BASE: usize = crate::mmio::addr(0x003000);

#[inline(always)]
fn pi_intsr() -> *mut u32 { PI_BASE as *mut u32 }
#[inline(always)]
fn pi_intmr() -> *mut u32 { (PI_BASE + 4) as *mut u32 }

// ─── Interrupt source bitmasks (bit N = 0x80000000 >> N) ─────────────────────

pub const IM_MEM0:        u32 = 0x8000_0000;
pub const IM_MEM1:        u32 = 0x4000_0000;
pub const IM_MEM2:        u32 = 0x2000_0000;
pub const IM_MEM3:        u32 = 0x1000_0000;
pub const IM_MEMADDRESS:  u32 = 0x0800_0000;
pub const IM_DSP_AI:      u32 = 0x0400_0000;
pub const IM_DSP_ARAM:    u32 = 0x0200_0000;
pub const IM_DSP_DSP:     u32 = 0x0100_0000;
pub const IM_AI:          u32 = 0x0080_0000;
pub const IM_EXI0_EXI:   u32 = 0x0040_0000;
pub const IM_EXI0_TC:    u32 = 0x0020_0000;
pub const IM_EXI0_EXT:   u32 = 0x0010_0000;
pub const IM_EXI1_EXI:   u32 = 0x0008_0000;
pub const IM_EXI1_TC:    u32 = 0x0004_0000;
pub const IM_EXI1_EXT:   u32 = 0x0002_0000;
pub const IM_EXI2_EXI:   u32 = 0x0001_0000;
pub const IM_EXI2_TC:    u32 = 0x0000_8000;
pub const IM_PI_CP:       u32 = 0x0000_4000;
pub const IM_PI_PETOKEN:  u32 = 0x0000_2000;
pub const IM_PI_PEFINISH: u32 = 0x0000_1000;
pub const IM_PI_SI:       u32 = 0x0000_0800;
pub const IM_PI_DI:       u32 = 0x0000_0400;
pub const IM_PI_RSW:      u32 = 0x0000_0200;
pub const IM_PI_ERROR:    u32 = 0x0000_0100;
pub const IM_PI_VI:       u32 = 0x0000_0080;
pub const IM_PI_DEBUG:    u32 = 0x0000_0040;
pub const IM_PI_HSP:      u32 = 0x0000_0020;

pub const IM_DSP: u32 = IM_DSP_AI | IM_DSP_ARAM | IM_DSP_DSP;
pub const IM_EXI: u32 = IM_EXI0_EXI | IM_EXI0_TC | IM_EXI0_EXT
                       | IM_EXI1_EXI | IM_EXI1_TC | IM_EXI1_EXT
                       | IM_EXI2_EXI | IM_EXI2_TC;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialise the PI.
///
/// Enables only the reset-switch interrupt by default, which lets the user
/// reset the console. Call [`mask_irq`] / [`unmask_irq`] after this to
/// enable additional sources.
///
/// # Safety
/// Must be called once during startup.
pub unsafe fn init() {
    // Enable only PI_RSW initially (safe default)
    core::ptr::write_volatile(pi_intmr(), IM_PI_RSW);
}

/// Read the current interrupt source register (which sources are pending).
#[inline]
pub unsafe fn pending() -> u32 {
    core::ptr::read_volatile(pi_intsr())
}

/// Read the current interrupt mask register.
#[inline]
pub unsafe fn mask() -> u32 {
    core::ptr::read_volatile(pi_intmr())
}

/// Unmask (enable) one or more interrupt sources.
///
/// `bits` is a bitmask of `IM_*` constants OR'd together.
pub unsafe fn unmask_irq(bits: u32) {
    let cur = core::ptr::read_volatile(pi_intmr());
    core::ptr::write_volatile(pi_intmr(), cur | bits);
}

/// Mask (disable) one or more interrupt sources.
pub unsafe fn mask_irq(bits: u32) {
    let cur = core::ptr::read_volatile(pi_intmr());
    core::ptr::write_volatile(pi_intmr(), cur & !bits);
}

/// Acknowledge (clear) one or more pending interrupts.
///
/// Write 1 to each bit to clear it. Must be called from the External Interrupt
/// exception handler for each source that was serviced.
pub unsafe fn clear_irq(bits: u32) {
    core::ptr::write_volatile(pi_intsr(), bits);
}

/// Return true if the reset button is currently held down.
pub unsafe fn reset_button_down() -> bool {
    // PIINTSR bit 22 (IM_PI_RSW) is set when the reset switch is pressed.
    // Wait for it to clear to detect a clean press.
    pending() & IM_PI_RSW != 0
}

/// Enable external interrupts on the CPU (MSR[EE] = 1).
///
/// # Safety
/// `gc_rt::exception::init()` must have been called first.
#[inline(always)]
pub unsafe fn enable_irq() {
    gc_rt::irq::enable();
}

/// Disable external interrupts on the CPU (MSR[EE] = 0).
///
/// Returns the previous MSR state for use with [`restore_irq`].
#[inline(always)]
pub fn disable_irq() -> gc_rt::irq::IrqState {
    gc_rt::irq::disable()
}

/// Restore interrupts to a previously saved state.
#[inline(always)]
pub unsafe fn restore_irq(state: gc_rt::irq::IrqState) {
    gc_rt::irq::restore(state);
}
