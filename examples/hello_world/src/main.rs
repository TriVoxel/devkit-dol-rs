//! # Hello World — DevKit DOL
//!
//! Boots on GameCube (or Dolphin), initialises NTSC 480i video, and prints
//! "Hello, GameCube!" to the screen using the framebuffer text console.
//!
//! ## Boot sequence (handled by dkdol-rt)
//!
//! 1. `_start` (assembly): initialise BATs, GPRs, FPU, cache, BSS → call `main`
//! 2. `main` (this file): init VI, allocate XFB, draw text, loop
//!
//! ## How to build and run
//!
//! ```sh
//! # From the workspace root:
//! cargo +nightly build \
//!     -Z build-std=core,compiler_builtins \
//!     -Z build-std-features=compiler-builtins-mem \
//!     --target targets/powerpc-gekko-eabi.json \
//!     -p hello_world --release
//!
//! cargo run -p elf2dol -- \
//!     target/powerpc-gekko-eabi/release/hello_world \
//!     hello_world.dol
//!
//! dolphin-emu -e hello_world.dol
//! ```

#![no_std]
#![no_main]

use dkdol_gfx::{Xfb, Console, YcbcrPair, color};
use dkdol_hal::vi;
use core::fmt::Write;

// ─────────────────────────────────────────────────────────────────────────────
// Static framebuffer
//
// We allocate the XFB as a static array so we don't need a heap.
// 640 × 480 × 2 bytes = 614,400 bytes ≈ 600 KB.
//
// Alignment: 32 bytes (cache line size and VI requirement).
// Address: the linker places this in .bss (zeroed by dkdol-rt before main).
// ─────────────────────────────────────────────────────────────────────────────

const FB_WIDTH:  u32 = 640;
const FB_HEIGHT: u32 = 480;
const FB_WORDS:  usize = (FB_WIDTH * FB_HEIGHT / 2) as usize; // u32 per pixel-pair

/// The external framebuffer. 32-byte aligned for the VI DMA and cache ops.
#[repr(C, align(32))]
struct AlignedFb([u32; FB_WORDS]);

/// Static XFB storage in BSS. Zeroed before main() by dkdol-rt.
static mut FRAMEBUFFER: AlignedFb = AlignedFb([0u32; FB_WORDS]);

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Application entry point, called by the dkdol-rt boot assembly after all
/// hardware initialisation and BSS zeroing.
#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe { run() }
}

unsafe fn run() -> ! {
    // ── 1. Initialise Video Interface for NTSC 480i ───────────────────────
    vi::init_ntsc_480i();

    // ── 2. Wrap the static buffer as an Xfb ──────────────────────────────
    let fb_ptr = FRAMEBUFFER.0.as_mut_ptr();
    let mut xfb = Xfb::from_raw(fb_ptr, FB_WIDTH, FB_HEIGHT);

    // ── 3. Clear the screen to a deep blue background ────────────────────
    // Navy blue in YCbCr: Y≈41, Cb≈240, Cr≈110
    let navy = YcbcrPair::new(41, 200, 41, 110);
    xfb.clear(navy);

    // ── 4. Create a text console and print ───────────────────────────────
    let mut con = Console::new(&mut xfb);
    con.set_bg(navy);
    con.set_fg(YcbcrPair::WHITE);

    // Banner
    con.print_str("\n");
    con.print_str("  ====================================\n");
    con.print_str("   DevKit DOL  --  Hello, World!\n");
    con.print_str("  ====================================\n");
    con.print_str("\n");

    // System info
    con.set_fg(color::YELLOW);
    con.print_str("  Video: NTSC 480i (640x480)\n");
    con.print_str("  CPU:   PowerPC 750CXe (Gekko)\n");
    con.print_str("  RAM:   24 MB MEM1\n");
    con.print_str("\n");

    // Milestone status
    con.set_fg(color::CYAN);
    con.print_str("  Milestone 0: Scaffold + hello_world\n");
    con.set_fg(color::LIGHT_GREY);
    con.print_str("  [x] Boot assembly (BATs, FPU, cache, BSS)\n");
    con.print_str("  [x] VI NTSC 480i init\n");
    con.print_str("  [x] XFB text console\n");
    con.print_str("  [x] elf2dol converter\n");
    con.print_str("\n");

    con.set_fg(color::WHITE);
    con.print_str("  Hello, GameCube!\n");

    // ── 5. Flush console to RAM, point VI at the framebuffer ─────────────
    con.flush();

    let stride_bytes = FB_WIDTH * 2; // bytes per scanline
    vi::set_framebuffer(fb_ptr, stride_bytes);
    vi::flush();

    // ── 6. Spin forever ───────────────────────────────────────────────────
    // On real hardware, you'd poll a vsync interrupt and update the display.
    // For this hello world we just loop and let the VI keep displaying.
    loop {
        // Keep the CPU from executing out of the interrupt handler
        dkdol_rt::cache::sync();
    }
}
