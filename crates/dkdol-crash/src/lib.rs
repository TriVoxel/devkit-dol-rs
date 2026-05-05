//! # dkdol-crash — GameCube/Wii crash handler
//!
//! When any fatal exception fires and no user handler is registered, this
//! crate intercepts the default halt and renders a full crash screen:
//!
//! - Exception type and cause
//! - All 32 general-purpose registers (r0–r31)
//! - Special registers: SRR0 (faulting PC), SRR1 (MSR), CR, LR, CTR, XER
//! - Fault-specific registers: DAR (bad data address), DSISR
//! - PPC call stack trace (raw addresses via back-chain walk)
//! - D-pad scrolling to navigate when the output is taller than one screen
//!
//! ## Usage
//!
//! Add `dkdol-crash` to your `[dependencies]` and call [`init`] once during
//! startup, after `dkdol_rt::exception::init()`:
//!
//! ```rust,no_run
//! unsafe {
//!     dkdol_rt::exception::init();
//!     dkdol_crash::init();  // registers crash handler for all fatal exceptions
//! }
//! ```
//!
//! ## Stack trace
//!
//! The PowerPC ABI stores the previous stack frame pointer at `*(SP + 0)` and
//! the saved link register at `*(SP + 4)`. By following the back-chain we get
//! a sequence of return addresses, displayed as hex. Match them against your
//! `.map` file or load the `.elf` in Dolphin's debugger for symbol names.
//!
//! ## Controls (during crash)
//!
//! | Button | Action          |
//! |--------|-----------------|
//! | D-Up   | Scroll up       |
//! | D-Down | Scroll down     |
//! | Start  | Soft reset (if reset handler is installed) |

#![no_std]
#![feature(asm_experimental_arch)]

use dkdol_gfx::{Console, Xfb, YcbcrPair, color};
use dkdol_hal::{vi, si::{self, Port, Buttons, PadResult}};
use dkdol_rt::exception::{Exception, ExcCtx};
use core::fmt::Write;

// ─── Framebuffer for crash display ───────────────────────────────────────────

const FB_W: u32 = 640;
const FB_H: u32 = 480;
const FB_WORDS: usize = (FB_W * FB_H / 2) as usize;

/// Dedicated crash framebuffer — statically allocated so the crash handler
/// works even if the application's framebuffer was corrupted.
#[repr(C, align(32))]
struct CrashFb([u32; FB_WORDS]);
static mut CRASH_FB: CrashFb = CrashFb([0u32; FB_WORDS]);

// ─── Crash screen line buffer ─────────────────────────────────────────────────

/// Maximum lines we can capture in the crash output.
const MAX_LINES: usize = 256;
/// Maximum characters per line.
const LINE_LEN: usize = 80;

struct LineBuffer {
    lines:  [[u8; LINE_LEN]; MAX_LINES],
    colors: [YcbcrPair; MAX_LINES],
    count:  usize,
}

impl LineBuffer {
    const fn new() -> Self {
        LineBuffer {
            lines:  [[b' '; LINE_LEN]; MAX_LINES],
            colors: [YcbcrPair::WHITE; MAX_LINES],
            count:  0,
        }
    }

    fn push(&mut self, color: YcbcrPair, s: &str) {
        if self.count >= MAX_LINES { return; }
        let dst = &mut self.lines[self.count];
        let bytes = s.as_bytes();
        let len = bytes.len().min(LINE_LEN);
        dst[..len].copy_from_slice(&bytes[..len]);
        if len < LINE_LEN { dst[len] = 0; }
        self.colors[self.count] = color;
        self.count += 1;
    }

    fn push_fmt(&mut self, color: YcbcrPair, args: core::fmt::Arguments<'_>) {
        if self.count >= MAX_LINES { return; }
        let mut w = LineWriter { buf: &mut self.lines[self.count], pos: 0 };
        let _ = core::fmt::write(&mut w, args);
        self.colors[self.count] = color;
        self.count += 1;
    }
}

struct LineWriter<'a> {
    buf: &'a mut [u8; LINE_LEN],
    pos: usize,
}
impl core::fmt::Write for LineWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            if self.pos >= LINE_LEN { break; }
            self.buf[self.pos] = b;
            self.pos += 1;
        }
        Ok(())
    }
}

static mut LINE_BUF: LineBuffer = LineBuffer::new();

// ─── Public API ───────────────────────────────────────────────────────────────

/// Register the crash handler for all fatal exception types.
///
/// Call this once during startup after [`dkdol_rt::exception::init`].
/// When a fatal exception fires with no user handler, instead of halting,
/// this displays a crash screen.
///
/// # Safety
/// Must be called after `dkdol_rt::exception::init()`.
pub unsafe fn init() {
    use dkdol_rt::exception::{Exception::*, register};
    // Register for all exceptions that would otherwise halt
    register(SystemReset,       crash_handler);
    register(MachineCheck,      crash_handler);
    register(Dsi,               crash_handler);
    register(Isi,               crash_handler);
    register(Alignment,         crash_handler);
    register(Program,           crash_handler);
    register(FpUnavailable,     crash_handler);
}

// ─── Exception handler entry point ───────────────────────────────────────────

fn crash_handler(exc: Exception, ctx: &mut ExcCtx) {
    // Safety: we're in an exception context, single-threaded bare metal.
    unsafe { render_crash(exc, ctx); }
}

// ─── Crash rendering ─────────────────────────────────────────────────────────

unsafe fn render_crash(exc: Exception, ctx: &ExcCtx) -> ! {
    // Build the full output into a line buffer first
    let buf = &mut LINE_BUF;
    buf.count = 0; // reset

    // ── Header ────────────────────────────────────────────────────────────
    buf.push(color::RED, "╔══════════════════════════════════════════════════════════╗");
    buf.push_fmt(color::RED,  format_args!("  FATAL EXCEPTION — {:?}", exc));
    buf.push(color::RED, "╚══════════════════════════════════════════════════════════╝");
    buf.push(YcbcrPair::new(16,128,16,128), "");

    // ── Fault address and cause ───────────────────────────────────────────
    buf.push_fmt(color::YELLOW, format_args!("  PC (SRR0): 0x{:08X}    MSR (SRR1): 0x{:08X}", ctx.srr0, ctx.srr1));
    match exc {
        Exception::Dsi | Exception::Alignment => {
            buf.push_fmt(color::YELLOW,
                format_args!("  DAR:       0x{:08X}    DSISR:      0x{:08X}", ctx.dar, ctx.dsisr));
            buf.push_fmt(color::LIGHT_GREY,
                format_args!("  Cause: {}", dsi_cause(ctx.dsisr)));
        }
        Exception::MachineCheck => {
            buf.push_fmt(color::YELLOW,
                format_args!("  DAR:       0x{:08X}    DSISR:      0x{:08X}", ctx.dar, ctx.dsisr));
        }
        _ => {}
    }

    buf.push(color::DARK_GREY, "  ───────────────────────────────────────────────────────────");

    // ── General-purpose registers (8 per row) ─────────────────────────────
    buf.push(color::CYAN, "  General-Purpose Registers:");
    for row in 0..4 {
        let base = row * 8;
        let mut line = [0u8; LINE_LEN];
        let mut lw = LineWriter { buf: &mut line, pos: 0 };
        let _ = write!(lw, "  ");
        for col in 0..8 {
            let r = base + col;
            let _ = write!(lw, "r{:<2}={:08X} ", r, ctx.gprs[r]);
        }
        buf.push_fmt(color::WHITE, format_args!("{}", core::str::from_utf8(&line[..lw.pos]).unwrap_or("")));
    }

    buf.push(color::DARK_GREY, "  ───────────────────────────────────────────────────────────");

    // ── Special registers ─────────────────────────────────────────────────
    buf.push(color::CYAN, "  Special Registers:");
    buf.push_fmt(color::WHITE, format_args!(
        "   LR ={:08X}  CTR={:08X}  CR ={:08X}  XER={:08X}",
        ctx.lr, ctx.ctr, ctx.cr, ctx.xer));

    buf.push(color::DARK_GREY, "  ───────────────────────────────────────────────────────────");

    // ── Stack trace ───────────────────────────────────────────────────────
    buf.push(color::CYAN, "  Stack Trace (return addresses, oldest last):");
    buf.push(color::DARK_GREY, "  (match against .map file or load .elf in Dolphin debugger)");

    let sp_start = ctx.gprs[1]; // r1 = stack pointer at time of exception
    walk_stack(buf, sp_start, ctx.srr0);

    buf.push(color::DARK_GREY, "  ───────────────────────────────────────────────────────────");
    buf.push(color::DARK_GREY, "  D-Up/D-Down: scroll  |  Reset to reboot");

    // ── Initialize video and display ──────────────────────────────────────
    vi::init_ntsc_480i();
    let fb_ptr = CRASH_FB.0.as_mut_ptr();

    display_loop(fb_ptr, buf);
}

/// Interactable display loop — polls D-pad, redraws on scroll.
unsafe fn display_loop(fb_ptr: *mut u32, buf: &LineBuffer) -> ! {
    const ROWS_PER_SCREEN: i32 = 55; // 480px / 8px font - 5 for header space
    let total = buf.count as i32;
    let mut scroll: i32 = 0;

    loop {
        render_screen(fb_ptr, buf, scroll);
        vi::set_framebuffer(fb_ptr, FB_W * 2);
        vi::flush();

        dkdol_rt::timer::delay_ms(80); // ~12 fps for the crash display

        // Poll controller port 1
        if let PadResult::Ok(pad) = si::read_pad(Port::P1) {
            if pad.pressed(Buttons::DDown) {
                scroll = (scroll + 3).min(total - ROWS_PER_SCREEN).max(0);
            }
            if pad.pressed(Buttons::DUp) {
                scroll = (scroll - 3).max(0);
            }
        }
    }
}

/// Render `buf` starting at `scroll` line offset into the framebuffer.
unsafe fn render_screen(fb_ptr: *mut u32, buf: &LineBuffer, scroll: i32) {
    let bg = YcbcrPair::new(10, 128, 10, 40); // very dark red-tinted background
    let mut xfb = Xfb::from_raw(fb_ptr, FB_W, FB_H);
    xfb.clear(bg);

    let mut con = Console::new(&mut xfb);
    con.set_bg(bg);

    let start = scroll as usize;
    let end = (start + 60).min(buf.count); // 60 rows visible

    for i in start..end {
        let line = &buf.lines[i];
        let color = buf.colors[i];
        con.set_fg(color);

        // Print until NUL or end of buffer
        let mut len = LINE_LEN;
        for (j, &b) in line.iter().enumerate() {
            if b == 0 { len = j; break; }
        }
        let s = core::str::from_utf8(&line[..len]).unwrap_or("???");
        con.print_str(s);
        con.print_str("\n");
    }

    // Scroll indicator at bottom
    if buf.count > 60 {
        con.set_fg(color::DARK_GREY);
        let pct = if buf.count > 0 {
            (scroll as usize * 100) / buf.count.saturating_sub(60)
        } else { 0 };
        let _ = write!(con, "  [{:3}% — {} lines total — D-Up/D-Down to scroll]",
            pct.min(100), buf.count);
    }

    con.flush();
}

// ─── Stack walk ───────────────────────────────────────────────────────────────

const STACK_TOP:    u32 = 0x8180_0000;
const STACK_BOTTOM: u32 = 0x8000_0000;
const MAX_FRAMES:   usize = 32;

unsafe fn walk_stack(buf: &mut LineBuffer, sp: u32, pc: u32) {
    buf.push_fmt(color::WHITE,
        format_args!("   #00  0x{:08X}  ← exception PC (SRR0)", pc));

    let mut frame_sp = sp;
    let mut depth = 1usize;

    while depth <= MAX_FRAMES {
        // Validate SP is in a reasonable range
        if frame_sp == 0
            || frame_sp & 0x3 != 0      // must be 4-byte aligned
            || frame_sp < STACK_BOTTOM
            || frame_sp >= STACK_TOP
        {
            break;
        }

        // PPC ABI: back-chain at SP+0, saved LR at SP+4
        let prev_sp   = core::ptr::read_volatile(frame_sp as *const u32);
        let saved_lr  = core::ptr::read_volatile((frame_sp + 4) as *const u32);

        // Validate saved LR is in a reasonable address range
        if saved_lr >= 0x8000_0000 && saved_lr < 0x8180_0000 {
            buf.push_fmt(color::WHITE,
                format_args!("   #{:02}  0x{:08X}", depth, saved_lr));
        } else if saved_lr == 0 || saved_lr == 0xFFFF_FFFF {
            buf.push_fmt(color::DARK_GREY,
                format_args!("   #{:02}  0x{:08X}  ← end of stack", depth, saved_lr));
            break;
        } else {
            buf.push_fmt(color::DARK_GREY,
                format_args!("   #{:02}  0x{:08X}  ← outside MEM1", depth, saved_lr));
        }

        if prev_sp == 0 || prev_sp == frame_sp { break; }
        frame_sp = prev_sp;
        depth += 1;
    }

    if depth > MAX_FRAMES {
        buf.push(color::DARK_GREY, "   ... (truncated at 32 frames)");
    }
}

// ─── DSI cause decoder ────────────────────────────────────────────────────────

fn dsi_cause(dsisr: u32) -> &'static str {
    if dsisr & (1 << 31) != 0 { return "direct-store error"; }
    if dsisr & (1 << 30) != 0 { return "page fault (no translation)"; }
    if dsisr & (1 << 27) != 0 { return "protection violation (store to RO)"; }
    if dsisr & (1 << 26) != 0 { return "protection violation (load from WO)"; }
    if dsisr & (1 << 25) != 0 { return "lwarx/stwcx to cache-inhibited storage"; }
    if dsisr & (1 << 24) != 0 { return "alignment exception in l/lfscx"; }
    if dsisr & (1 << 23) != 0 { return "data address breakpoint"; }
    if dsisr & (1 << 22) != 0 { return "TLB miss on load"; }
    if dsisr & (1 << 21) != 0 { return "TLB miss on store"; }
    "unknown DSI cause"
}
