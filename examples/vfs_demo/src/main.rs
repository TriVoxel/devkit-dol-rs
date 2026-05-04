//! # vfs_demo — DevKit DOL
//!
//! Demonstrates the gc-vfs unified device tree:
//!   - Device enumeration via `list_devices()`
//!   - Lazy filesystem mounting on first `open()` to a device path
//!   - Reading a file from an SD card  (/dev/sd/sp/…)
//!   - Reading controller state as a file (/dev/hid/p1/std)
//!
//! No filesystem knowledge is needed in application code — the path prefix
//! determines which backend is used automatically.

#![no_std]
#![no_main]

use core::fmt::Write;
use gc_gfx::{Console, Xfb, YcbcrPair, color};
use gc_hal::vi;
use gc_vfs::{self as vfs, ControllerState, O_RDONLY};

const W: u32 = 640;
const H: u32 = 480;

#[repr(C, align(32))]
struct Fb([u32; (W * H / 2) as usize]);
static mut FB: Fb = Fb([0u32; (W * H / 2) as usize]);

#[no_mangle]
pub extern "C" fn main() -> ! { unsafe { run() } }

unsafe fn run() -> ! {
    vi::init_ntsc_480i();
    let ptr = FB.0.as_mut_ptr();
    vi::set_framebuffer(ptr, W * 2);
    vi::flush();

    // ── Init: probe hardware, populate /dev/ ─────────────────────────────
    // No filesystem is mounted yet — just lists what hardware is present.
    vfs::init();

    let bg = YcbcrPair::new(16, 128, 16, 128);
    let mut frame = 0u32;

    loop {
        let mut xfb = Xfb::from_raw(ptr, W, H);
        xfb.clear(bg);
        let mut con = Console::new(&mut xfb);
        con.set_bg(bg);

        // ── Header ───────────────────────────────────────────────────────
        con.set_fg(color::CYAN);
        con.print_str("\n  DevKit DOL — VFS Demo\n");
        con.set_fg(color::DARK_GREY);
        con.print_str("  ─────────────────────────────────────\n\n");

        // ── Device tree ───────────────────────────────────────────────────
        con.set_fg(color::YELLOW);
        con.print_str("  Devices:\n");
        vfs::list_devices(|path, present, mounted| {
            if path.starts_with("/dev/hid") { return; }
            let c = match (present, mounted) {
                (false, _)   => color::DARK_GREY,
                (true, false)=> color::LIGHT_GREY,
                (true, true) => color::GREEN,
            };
            con.set_fg(c);
            let label = if !present { "(absent)" }
                        else if mounted { "(mounted)" }
                        else { "(ready)" };
            let _ = write!(con, "    {:<16} {}\n", path, label);
        });

        // ── File read from SD2SP2 ─────────────────────────────────────────
        con.set_fg(color::YELLOW);
        con.print_str("\n  /dev/sd/sp/hello.txt:\n");
        match vfs::open("/dev/sd/sp/hello.txt", O_RDONLY) {
            Ok(fd) => {
                let mut buf = [0u8; 48];
                let n = vfs::read(fd, &mut buf).unwrap_or(0);
                vfs::close(fd);
                con.set_fg(color::GREEN);
                let s = core::str::from_utf8(&buf[..n]).unwrap_or("<binary>");
                let _ = write!(con, "    ok: \"{}\"\n", &s[..s.len().min(40)]);
            }
            Err(vfs::Error::NoDevice)  => { con.set_fg(color::DARK_GREY); con.print_str("    no card\n"); }
            Err(vfs::Error::FsNotAvailable) => { con.set_fg(color::RED); con.print_str("    (add all-fs feature)\n"); }
            Err(e) => { con.set_fg(color::RED); let _ = write!(con, "    {:?}\n", e); }
        }

        // ── Controller ────────────────────────────────────────────────────
        con.set_fg(color::YELLOW);
        con.print_str("\n  /dev/hid/p1/std:\n");
        if let Ok(fd) = vfs::open("/dev/hid/p1/std", O_RDONLY) {
            let mut state = ControllerState::default();
            let bytes = core::slice::from_raw_parts_mut(
                &mut state as *mut _ as *mut u8,
                core::mem::size_of::<ControllerState>());
            let _ = vfs::read(fd, bytes);
            vfs::close(fd);
            if state.connected != 0 {
                con.set_fg(color::GREEN);
                let _ = write!(con,
                    "    BTN={:#06x}  ({},{})  L={}  R={}\n",
                    state.buttons, state.stick_x, state.stick_y,
                    state.trigger_l, state.trigger_r);
            } else {
                con.set_fg(color::DARK_GREY);
                con.print_str("    (no controller)\n");
            }
        }

        // ── Hotplug ───────────────────────────────────────────────────────
        vfs::poll();

        con.set_fg(color::DARK_GREY);
        let _ = write!(con, "\n  frame {}\n", frame);
        con.flush();
        vi::set_framebuffer(ptr, W * 2);
        vi::flush();
        gc_rt::timer::delay_ms(16);
        frame = frame.wrapping_add(1);
    }
}
