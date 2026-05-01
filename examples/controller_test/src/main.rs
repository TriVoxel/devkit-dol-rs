//! # controller_test — DevKit DOL RS
//!
//! Reads all four controller ports every frame and displays live pad state
//! on screen: button names, stick values, and trigger depths.
//!
//! ## Build & run
//!
//! ```sh
//! cargo +nightly build \
//!   -Z build-std=core,compiler_builtins \
//!   -Z build-std-features=compiler-builtins-mem \
//!   --target targets/powerpc-gekko-eabi.json \
//!   -p controller_test --release
//!
//! cargo run -p elf2dol -- \
//!   target/powerpc-gekko-eabi/release/controller_test \
//!   controller_test.dol
//!
//! dolphin-emu -e controller_test.dol
//! ```

#![no_std]
#![no_main]

use core::fmt::Write;
use gc_gfx::{Console, Xfb, YcbcrPair, color};
use gc_hal::{vi, si::{self, Port, Buttons, PadResult}};

// ─── Framebuffer ─────────────────────────────────────────────────────────────

const FB_WIDTH:  u32 = 640;
const FB_HEIGHT: u32 = 480;
const FB_WORDS:  usize = (FB_WIDTH * FB_HEIGHT / 2) as usize;

#[repr(C, align(32))]
struct AlignedFb([u32; FB_WORDS]);
static mut FRAMEBUFFER: AlignedFb = AlignedFb([0u32; FB_WORDS]);

// ─── Entry ───────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe { run() }
}

unsafe fn run() -> ! {
    vi::init_ntsc_480i();

    let fb_ptr = FRAMEBUFFER.0.as_mut_ptr();
    let mut xfb = Xfb::from_raw(fb_ptr, FB_WIDTH, FB_HEIGHT);
    let stride  = FB_WIDTH * 2;

    let bg   = YcbcrPair::new(16, 128, 16, 128);   // black
    let grey = color::DARK_GREY;
    let cyan = color::CYAN;
    let yel  = color::YELLOW;
    let grn  = color::GREEN;
    let red  = color::RED;
    let wht  = color::WHITE;
    let mag  = color::MAGENTA;

    vi::set_framebuffer(fb_ptr, stride);
    vi::flush();

    let mut frame: u32 = 0;

    loop {
        xfb.clear(bg);
        let mut con = Console::new(&mut xfb);
        con.set_bg(bg);

        // ── Header ───────────────────────────────────────────────────
        con.set_fg(cyan);
        con.print_str("\n  DevKit DOL RS -- Controller Test\n");
        con.set_fg(grey);
        con.print_str("  ──────────────────────────────────────────────────\n\n");

        // ── Poll all four ports ───────────────────────────────────────
        for port_idx in 0u8..4 {
            let port = match port_idx {
                0 => Port::P1, 1 => Port::P2, 2 => Port::P3, _ => Port::P4,
            };

            con.set_fg(yel);
            let _ = write!(con, "  PORT {}:  ", port_idx + 1);

            match si::read_pad(port) {
                PadResult::NoController => {
                    con.set_fg(grey);
                    con.print_str("(no controller)\n\n");
                }
                PadResult::Error => {
                    con.set_fg(red);
                    con.print_str("ERROR\n\n");
                }
                PadResult::Ok(pad) => {
                    // ── Buttons line ─────────────────────────────────
                    con.set_fg(wht);
                    let b = pad.buttons;
                    let _ = write!(con, "BTN [");
                    btn_char(&mut con, b, Buttons::A,     'A', grn);
                    btn_char(&mut con, b, Buttons::B,     'B', red);
                    btn_char(&mut con, b, Buttons::X,     'X', wht);
                    btn_char(&mut con, b, Buttons::Y,     'Y', wht);
                    btn_char(&mut con, b, Buttons::Start, 'S', cyan);
                    btn_char(&mut con, b, Buttons::Z,     'Z', mag);
                    btn_char(&mut con, b, Buttons::L,     'L', yel);
                    btn_char(&mut con, b, Buttons::R,     'R', yel);
                    btn_char(&mut con, b, Buttons::DUp,   '^', wht);
                    btn_char(&mut con, b, Buttons::DDown, 'v', wht);
                    btn_char(&mut con, b, Buttons::DLeft, '<', wht);
                    btn_char(&mut con, b, Buttons::DRight,'>', wht);
                    con.set_fg(wht);
                    con.print_str("]\n");

                    // ── Analog axes ──────────────────────────────────
                    con.set_fg(grey);
                    let sx = pad.stick_x_centered();
                    let sy = pad.stick_y_centered();
                    let cx = pad.cstick_x_centered();
                    let cy = pad.cstick_y_centered();
                    let _ = write!(
                        con,
                        "         Stick({:+4},{:+4})  CStick({:+4},{:+4})  L={:3}  R={:3}\n\n",
                        sx, sy, cx, cy, pad.trigger_l, pad.trigger_r
                    );

                    // ── Trigger bars ─────────────────────────────────
                    trigger_bar(&mut con, "  L[", pad.trigger_l, "] ");
                    trigger_bar(&mut con, " R[",  pad.trigger_r, "]\n");
                }
            }
        }

        // ── Footer ───────────────────────────────────────────────────
        con.set_fg(grey);
        let _ = write!(con, "\n  Frame: {}\n", frame);

        // ── Flush to screen ──────────────────────────────────────────
        con.flush();
        vi::set_framebuffer(fb_ptr, stride);
        vi::flush();

        // Simple vsync-ish delay: ~1/60 s at 40.5 MHz TBR
        gc_rt::timer::delay_ms(16);

        frame = frame.wrapping_add(1);
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn btn_char(con: &mut Console, buttons: u16, mask: u16, ch: char, active_color: YcbcrPair) {
    if buttons & mask != 0 {
        con.set_fg(active_color);
        let mut buf = [0u8; 4];
        con.print_str(ch.encode_utf8(&mut buf));
    } else {
        con.set_fg(color::DARK_GREY);
        con.print_str(".");
    }
}

fn trigger_bar(con: &mut Console, prefix: &str, val: u8, suffix: &str) {
    con.set_fg(color::LIGHT_GREY);
    con.print_str(prefix);
    let filled = (val as usize * 16) / 255;
    for i in 0..16 {
        if i < filled {
            con.set_fg(color::GREEN);
            con.print_str("#");
        } else {
            con.set_fg(color::DARK_GREY);
            con.print_str("-");
        }
    }
    con.set_fg(color::LIGHT_GREY);
    con.print_str(suffix);
}
