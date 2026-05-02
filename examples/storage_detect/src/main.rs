//! # storage_detect — DevKit DOL
//!
//! Scans all storage devices and displays what's found:
//!
//! - **Slot A** (EXI Ch0): SD Gecko SD card or GC Memory Card
//! - **Slot B** (EXI Ch1): SD Gecko SD card or GC Memory Card
//! - **SP2** (EXI Ch2): SD2SP2 SD card adapter
//! - **DVD drive**: Any disc (real or via ODE — CubeODE / GCLoader / Flippy)
//!
//! Then reads sector 0 from each writable device and shows a brief hex
//! summary, so you can verify data is accessible.
//!
//! ## Hardware notes
//!
//! - SD Gecko: adapter in memory card slot, exposes SD/SDHC as block device
//! - SD2SP2: adapter for Serial Port 2 (bottom of console)
//! - CubeODE / GCLoader / Flippy: ODEs appear as a normal DVD drive — fully
//!   transparent to this code
//! - MemCard PRO GC: detected as a memory card; raw reads show FAT structure
//!
//! ## Build & run
//!
//! ```sh
//! cargo +nightly build \
//!   -Z build-std=core,compiler_builtins \
//!   -Z build-std-features=compiler-builtins-mem \
//!   --target targets/powerpc-gekko-eabi.json \
//!   -p storage_detect --release
//!
//! cargo run -p elf2dol -- \
//!   target/powerpc-gekko-eabi/release/storage_detect storage_detect.dol
//!
//! dolphin-emu -e storage_detect.dol
//! ```

#![no_std]
#![no_main]

use core::fmt::Write;
use gc_gfx::{Console, Xfb, YcbcrPair, color};
use gc_hal::{vi, storage::{self, StorageKind, StorageInfo}};

// ─── Framebuffer ─────────────────────────────────────────────────────────────

const FB_WIDTH:  u32 = 640;
const FB_HEIGHT: u32 = 480;
const FB_WORDS:  usize = (FB_WIDTH * FB_HEIGHT / 2) as usize;

#[repr(C, align(32))]
struct Fb([u32; FB_WORDS]);
static mut FRAMEBUFFER: Fb = Fb([0u32; FB_WORDS]);

// ─── Sector buffer ────────────────────────────────────────────────────────────

#[repr(C, align(32))]
struct SectorBuf([u8; 512]);
static mut SECTOR: SectorBuf = SectorBuf([0u8; 512]);

// Storage scan results
static mut DEVICES: [StorageInfo; 8] = [StorageInfo {
    kind: StorageKind::DvdDisc,
    dev_type: gc_hal::exi::DeviceType::None,
    sector_size: 0,
    sector_count: 0,
    read_only: false,
}; 8];

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

    let bg = YcbcrPair::new(6, 128, 6, 128);
    let mut xfb = Xfb::from_raw(fb_ptr, FB_WIDTH, FB_HEIGHT);
    xfb.clear(bg);
    let mut con = Console::new(&mut xfb);
    con.set_bg(bg);

    // ── Header ────────────────────────────────────────────────────────────
    con.set_fg(color::CYAN);
    con.print_str("\n  DevKit DOL -- Storage Device Scanner\n");
    con.set_fg(color::DARK_GREY);
    con.print_str("  ──────────────────────────────────────────────\n\n");
    con.set_fg(color::YELLOW);
    con.print_str("  Scanning: Slot A, Slot B, SP2, DVD...\n\n");
    con.flush();
    vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
    vi::flush();

    // ── Scan ──────────────────────────────────────────────────────────────
    let found = storage::scan(&mut DEVICES);

    if found == 0 {
        con.set_fg(color::RED);
        con.print_str("  No storage devices detected.\n");
        con.print_str("  Insert an SD Gecko, memory card, or disc and reboot.\n");
    } else {
        let _ = write!(con, "  Found {} device(s):\n\n", found);

        for i in 0..found {
            let dev = &DEVICES[i];
            let mb = dev.sector_count * dev.sector_size as u64 / (1024 * 1024);

            // Icon + name
            let icon = match dev.kind {
                StorageKind::SdCardSlotA |
                StorageKind::SdCardSlotB |
                StorageKind::SdCardSp2   => "SD",
                StorageKind::MemCardSlotA|
                StorageKind::MemCardSlotB=> "MC",
                StorageKind::DvdDisc     => "DV",
            };

            let name_color = match dev.kind {
                StorageKind::SdCardSlotA |
                StorageKind::SdCardSlotB |
                StorageKind::SdCardSp2   => color::GREEN,
                StorageKind::MemCardSlotA|
                StorageKind::MemCardSlotB=> color::YELLOW,
                StorageKind::DvdDisc     => color::CYAN,
            };

            con.set_fg(color::DARK_GREY);
            let _ = write!(con, "  [");
            con.set_fg(name_color);
            con.print_str(icon);
            con.set_fg(color::DARK_GREY);
            let _ = write!(con, "]  ");

            con.set_fg(name_color);
            con.print_str(dev.kind.name());

            con.set_fg(color::LIGHT_GREY);
            if mb > 0 {
                let _ = write!(con, "  —  {} MB  ({} sectors × {} B)",
                    mb, dev.sector_count, dev.sector_size);
            } else {
                con.print_str("  —  size unknown");
            }
            if dev.read_only {
                con.set_fg(color::DARK_GREY);
                con.print_str("  [read-only]");
            }
            con.print_str("\n");

            // Try to read sector 0 and show first 8 bytes as hex
            if !dev.read_only && dev.sector_size <= 512 {
                match try_read_first(dev) {
                    Some(first8) => {
                        con.set_fg(color::DARK_GREY);
                        con.print_str("         Sector 0: ");
                        con.set_fg(color::WHITE);
                        for b in &first8 {
                            let _ = write!(con, "{:02X} ", b);
                        }
                        con.print_str("...\n");
                    }
                    None => {
                        con.set_fg(color::RED);
                        con.print_str("         Sector 0: read error\n");
                    }
                }
            }
        }
    }

    // ── DVD note ──────────────────────────────────────────────────────────
    con.set_fg(color::DARK_GREY);
    con.print_str("\n  Note: CubeODE, GCLoader, and Flippy Drives\n");
    con.print_str("  are transparent — they appear as a normal DVD.\n");

    con.flush();
    vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
    vi::flush();

    loop { gc_rt::timer::delay_ms(1000); }
}

/// Try to read sector 0 from a device, return first 8 bytes or None.
unsafe fn try_read_first(dev: &StorageInfo) -> Option<[u8; 8]> {
    use gc_hal::storage::{SdCard, SdSlot, MemCard, CardSlot};

    match dev.kind {
        StorageKind::SdCardSlotA => {
            let mut card = SdCard::new(SdSlot::A);
            if card.init().is_ok() {
                if card.read_sector(0, &mut SECTOR.0).is_ok() {
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&SECTOR.0[..8]);
                    return Some(out);
                }
            }
            None
        }
        StorageKind::SdCardSlotB => {
            let mut card = SdCard::new(SdSlot::B);
            if card.init().is_ok() {
                if card.read_sector(0, &mut SECTOR.0).is_ok() {
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&SECTOR.0[..8]);
                    return Some(out);
                }
            }
            None
        }
        StorageKind::SdCardSp2 => {
            let mut card = SdCard::new(SdSlot::Sp2);
            if card.init().is_ok() {
                if card.read_sector(0, &mut SECTOR.0).is_ok() {
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&SECTOR.0[..8]);
                    return Some(out);
                }
            }
            None
        }
        StorageKind::MemCardSlotA => {
            if let Ok(card) = MemCard::probe(CardSlot::A) {
                if card.read_segment(0, &mut SECTOR.0).is_ok() {
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&SECTOR.0[..8]);
                    return Some(out);
                }
            }
            None
        }
        StorageKind::MemCardSlotB => {
            if let Ok(card) = MemCard::probe(CardSlot::B) {
                if card.read_segment(0, &mut SECTOR.0).is_ok() {
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&SECTOR.0[..8]);
                    return Some(out);
                }
            }
            None
        }
        _ => None,
    }
}
