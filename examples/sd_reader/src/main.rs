//! # sd_reader — DevKit DOL
//!
//! Demonstrates reading from an SD card inserted in a SD Gecko adapter in
//! memory card slot A. Reads the first sector (MBR / boot sector) and
//! displays a hex dump on screen.
//!
//! ## Hardware required
//!
//! - GameCube with SD Gecko adapter in memory card slot A
//! - FAT32 or any formatted SD card
//!
//! ## What it shows
//!
//! - SD card detection via EXI probe
//! - SD card initialization (CMD0 → CMD8 → ACMD41 → CMD58 → CMD16)
//! - Sector 0 read (512 bytes)
//! - MBR signature check (0x55AA at bytes 510–511)
//! - Hex dump of first 128 bytes on screen
//!
//! ## Build & run
//!
//! ```sh
//! cargo +nightly build \
//!   -Z build-std=core,compiler_builtins \
//!   -Z build-std-features=compiler-builtins-mem \
//!   --target targets/powerpc-gekko-eabi.json \
//!   -p sd_reader --release
//!
//! cargo run -p elf2dol -- \
//!   target/powerpc-gekko-eabi/release/sd_reader sd_reader.dol
//!
//! dolphin-emu -e sd_reader.dol
//! ```

#![no_std]
#![no_main]

use core::fmt::Write;
use dkdol_gfx::{Console, Xfb, YcbcrPair, color};
use dkdol_hal::{vi, sd::{self, Slot, SdError}};

// ─── Framebuffer ─────────────────────────────────────────────────────────────

const FB_WIDTH:  u32 = 640;
const FB_HEIGHT: u32 = 480;
const FB_WORDS:  usize = (FB_WIDTH * FB_HEIGHT / 2) as usize;

#[repr(C, align(32))]
struct Fb([u32; FB_WORDS]);
static mut FRAMEBUFFER: Fb = Fb([0u32; FB_WORDS]);

// ─── Sector buffer ────────────────────────────────────────────────────────────

// 512-byte sector buffer, 32-byte aligned (required for DMA)
#[repr(C, align(32))]
struct SectorBuf([u8; 512]);
static mut SECTOR: SectorBuf = SectorBuf([0u8; 512]);

// ─── Entry ───────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe { run() }
}

unsafe fn run() -> ! {
    vi::init_ntsc_480i();
    let fb_ptr = FRAMEBUFFER.0.as_mut_ptr();
    vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
    vi::flush();

    let bg = YcbcrPair::new(8, 128, 8, 128);
    let mut xfb = Xfb::from_raw(fb_ptr, FB_WIDTH, FB_HEIGHT);
    xfb.clear(bg);
    let mut con = Console::new(&mut xfb);
    con.set_bg(bg);

    // ── Header ────────────────────────────────────────────────────────────
    con.set_fg(color::CYAN);
    con.print_str("\n  DevKit DOL -- SD Card Reader\n");
    con.set_fg(color::DARK_GREY);
    con.print_str("  ────────────────────────────────────────────\n\n");

    // ── Detect and init SD card ───────────────────────────────────────────
    let mut card = sd::SdCard::new(Slot::A);

    con.set_fg(color::YELLOW);
    con.print_str("  Initialising SD card in Slot A...\n");
    con.flush();
    vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
    vi::flush();

    match card.init() {
        Ok(()) => {
            con.set_fg(color::GREEN);
            let _ = write!(con, "  Card OK! Capacity: {} sectors ({} MB)\n",
                card.sectors(),
                card.sectors() / 2048);
        }
        Err(SdError::NoCard) => {
            con.set_fg(color::RED);
            con.print_str("  ERROR: No SD card detected in Slot A.\n");
            con.print_str("  Insert an SD Gecko with an SD card and reboot.\n");
            con.flush();
            vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
            vi::flush();
            loop { dkdol_rt::timer::delay_ms(1000); }
        }
        Err(e) => {
            con.set_fg(color::RED);
            let _ = write!(con, "  ERROR: Init failed: {:?}\n", e);
            con.flush();
            vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
            vi::flush();
            loop { dkdol_rt::timer::delay_ms(1000); }
        }
    }

    // ── Read sector 0 (MBR / boot sector) ────────────────────────────────
    con.set_fg(color::WHITE);
    con.print_str("  Reading sector 0 (MBR)...\n");
    con.flush();
    vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
    vi::flush();

    match card.read_sector(0, &mut SECTOR.0) {
        Ok(()) => {
            // Check MBR signature
            let sig = u16::from_be_bytes([SECTOR.0[510], SECTOR.0[511]]);
            if sig == 0xAA55 {
                con.set_fg(color::GREEN);
                con.print_str("  MBR signature: 0x55AA ✓\n\n");
            } else {
                con.set_fg(color::YELLOW);
                let _ = write!(con, "  Signature: 0x{:04X} (not a standard MBR)\n\n", sig);
            }

            // Hex dump: first 128 bytes, 16 per row
            con.set_fg(color::LIGHT_GREY);
            con.print_str("  Offset  00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F\n");
            con.set_fg(color::DARK_GREY);
            con.print_str("  ──────  ── ── ── ── ── ── ── ──  ── ── ── ── ── ── ── ──\n");

            for row in 0..8usize {
                let offset = row * 16;
                con.set_fg(color::DARK_GREY);
                let _ = write!(con, "  {:04X}:  ", offset);
                con.set_fg(color::WHITE);
                for col in 0..16usize {
                    if col == 8 { con.print_str(" "); }
                    let byte = SECTOR.0[offset + col];
                    let color = if byte == 0 {
                        color::DARK_GREY
                    } else if byte == 0xFF {
                        color::LIGHT_GREY
                    } else {
                        color::WHITE
                    };
                    con.set_fg(color);
                    let _ = write!(con, "{:02X} ", byte);
                }
                con.print_str("\n");
            }
        }
        Err(e) => {
            con.set_fg(color::RED);
            let _ = write!(con, "  Read error: {:?}\n", e);
        }
    }

    con.flush();
    vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
    vi::flush();

    loop { dkdol_rt::timer::delay_ms(1000); }
}
