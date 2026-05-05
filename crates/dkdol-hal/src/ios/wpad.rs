//! WPAD — Wiimote input via IOS Bluetooth manager.
//!
//! IOS owns the Wii's BCM2045 Bluetooth chip. It handles pairing, HID
//! report parsing, and extension detection entirely on the ARM co-processor.
//! The PPC queries the current state of each channel (0–3, mapping to our
//! ports P1–P4) via IOCTLs on `/dev/btm`.
//!
//! ## Channel mapping
//!
//! | Channel | VFS port  | Notes                            |
//! |---------|-----------|----------------------------------|
//! | 0       | `P1`      | First synced Wiimote             |
//! | 1       | `P2`      | Second synced Wiimote            |
//! | 2       | `P3`      |                                  |
//! | 3       | `P4`      |                                  |
//!
//! ## WPAD state memory layout
//!
//! IOS writes a `WpadIosState` (64 bytes, 32-byte aligned) per channel into
//! a shared buffer. After each `wpad_poll()`, the PPC invalidates cache and
//! reads the fresh state.
//!
//! ## Extension mapping to GC pad
//!
//! IOS performs all extension mapping. The `pad_buttons` field of
//! `WpadIosState` always contains a synthesised GC-compatible button
//! bitmask matching the `dkdol_hal::si::Buttons` layout, regardless of
//! whether a Nunchuck, Classic Controller, or no extension is attached.
//!
//! | Extension      | Mapping                                      |
//! |----------------|----------------------------------------------|
//! | None           | WiiMote buttons (sideways hold)              |
//! | Nunchuck       | Stick → main stick; C → L; Z → Z; A/B → A/B|
//! | Classic Ctrl   | Full GC layout; ZL → Z; both sticks; triggers|
//!
//! ## Verification note
//!
//! The IOCTL command numbers (`WPAD_IOCTL_*`) and `WpadIosState` field
//! offsets listed here match the libogc WPAD implementation and IOS
//! reverse-engineering documentation. They should be verified against
//! IOS binary analysis or libogc source before relying on them in
//! production code.

use super::{ios_close, ios_ioctl, ios_open, dcbi, IOS_OK};
use crate::si::{WiimoteState, WiiExtension};

// ─── IOS WPAD IOCTL commands ──────────────────────────────────────────────────

/// Initialize the WPAD subsystem and register the state buffer.
const WPAD_IOCTL_INIT:      u32 = 0x01;
/// Activate listening on a channel (0–3). Arg: channel index.
const WPAD_IOCTL_LISTEN:    u32 = 0x02;
/// Read current state for all channels into the registered buffer.
const WPAD_IOCTL_GET_STATUS: u32 = 0x09;
/// Set the reporting mode (determines which data fields IOS populates).
const WPAD_IOCTL_SET_REPORT: u32 = 0x06;

/// Reporting mode: buttons + accelerometer + IR.
const WPAD_REPORT_BUTTONS_ACCEL_IR: u8 = 0x37;

// ─── IOS WPAD state structure ─────────────────────────────────────────────────

/// IOS WPAD channel state as written by the ARM co-processor.
///
/// One of these per Wiimote channel, 64 bytes, 32-byte aligned.
/// After IOS writes it, the PPC cache-invalidates the region and reads.
#[repr(C, align(32))]
#[derive(Clone, Copy, Default)]
struct WpadIosState {
    /// IOS error/connection status. 0 = connected, non-zero = disconnected.
    err:          i32,
    /// Synthesised GC-compatible button bitmask (dkdol_hal::si::Buttons layout).
    pad_buttons:  u16,
    /// Extra digital buttons (dkdol_hal::si::ExtButtons layout).
    ext_buttons:  u8,
    /// Extension attachment type (dkdol_hal::si::WiiExtension constants).
    extension:    u8,
    /// Raw WiiMote digital buttons (dkdol_hal::si::WiiButtons layout).
    wii_buttons:  u16,
    _pad0:        u16,
    /// Synthesised main stick X, center = 128.
    stick_x:      u8,
    /// Synthesised main stick Y, center = 128.
    stick_y:      u8,
    /// Synthesised C-stick X (Classic Controller right stick), center = 128.
    cstick_x:     u8,
    /// Synthesised C-stick Y, center = 128.
    cstick_y:     u8,
    /// Synthesised left trigger (0–255).
    trigger_l:    u8,
    /// Synthesised right trigger (0–255).
    trigger_r:    u8,
    _pad1:        u16,
    /// IR pointer X (0–1023). `0xFFFF` = not pointing at sensor bar.
    ir_x:         u16,
    /// IR pointer Y (0–767). `0xFFFF` = not visible.
    ir_y:         u16,
    /// Accelerometer X (0–255, center = 128).
    accel_x:      u8,
    /// Accelerometer Y (0–255, center = 128).
    accel_y:      u8,
    /// Accelerometer Z (0–255, center = 128).
    accel_z:      u8,
    _pad2:        u8,
    _reserved:    [u8; 36],
}

const _: () = assert!(core::mem::size_of::<WpadIosState>() == 64);

// ─── Shared state buffer ──────────────────────────────────────────────────────

/// State buffer written by IOS for all 4 channels.
/// Must be in memory IOS can access — MEM1 is fine.
#[repr(C, align(32))]
struct WpadBuf {
    channels: [WpadIosState; 4],
}

static mut WPAD_BUF: WpadBuf = WpadBuf {
    channels: [WpadIosState {
        err: -1, pad_buttons: 0, ext_buttons: 0, extension: WiiExtension::None,
        wii_buttons: 0, _pad0: 0, stick_x: 128, stick_y: 128,
        cstick_x: 128, cstick_y: 128, trigger_l: 0, trigger_r: 0,
        _pad1: 0, ir_x: 0xFFFF, ir_y: 0xFFFF,
        accel_x: 128, accel_y: 128, accel_z: 128, _pad2: 0,
        _reserved: [0u8; 36],
    }; 4],
};

static mut BTM_FD: i32 = -1;
static mut WPAD_READY: bool = false;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialise the WPAD subsystem.
///
/// Opens `/dev/btm`, registers the shared state buffer, and enables all
/// four channels. Call once at startup. Safe to call multiple times (no-op
/// after first call).
///
/// Returns `true` on success.
pub unsafe fn wpad_init() -> bool {
    if WPAD_READY { return true; }

    let fd = ios_open(b"/dev/btm\0", 0);
    if fd < 0 { return false; }
    BTM_FD = fd;

    // Register the state buffer address so IOS knows where to write.
    let buf_phys = super::to_phys(&WPAD_BUF as *const _ as usize);
    let mut init_args = [buf_phys, core::mem::size_of::<WpadBuf>() as u32];
    super::dcbf(init_args.as_ptr() as usize, 8);
    let r = ios_ioctl(fd, WPAD_IOCTL_INIT,
        core::slice::from_raw_parts(init_args.as_ptr() as *const u8, 8),
        &mut []);
    if r != IOS_OK { ios_close(fd); BTM_FD = -1; return false; }

    // Enable all 4 channels and set reporting mode
    for ch in 0u32..4 {
        let ch_bytes = ch.to_be_bytes();
        ios_ioctl(fd, WPAD_IOCTL_LISTEN,
            core::slice::from_raw_parts(ch_bytes.as_ptr(), 4), &mut []);
        let mode = [WPAD_REPORT_BUTTONS_ACCEL_IR, ch as u8];
        ios_ioctl(fd, WPAD_IOCTL_SET_REPORT,
            core::slice::from_raw_parts(mode.as_ptr(), 2), &mut []);
    }

    WPAD_READY = true;
    true
}

/// Request IOS to refresh all channels' state into the shared buffer.
///
/// Call once per frame before any `wpad_read()` calls. If WPAD was not
/// initialised (or initialisation failed) this is a no-op.
pub unsafe fn wpad_poll() {
    if !WPAD_READY || BTM_FD < 0 { return; }

    // Ask IOS to flush current state into WPAD_BUF
    ios_ioctl(BTM_FD, WPAD_IOCTL_GET_STATUS, &[], &mut []);

    // Invalidate our cache so we read IOS's freshly written data
    dcbi(&WPAD_BUF as *const _ as usize, core::mem::size_of::<WpadBuf>());
}

/// Read the current WiiMote state for `channel` (0–3).
///
/// Returns `None` if WPAD is not initialised or the channel is disconnected.
/// Must call [`wpad_poll`] each frame before calling this.
pub unsafe fn wpad_read(channel: u8) -> Option<WiimoteState> {
    if !WPAD_READY || channel >= 4 { return None; }
    let ch = &WPAD_BUF.channels[channel as usize];
    if ch.err != 0 { return None; }

    use crate::si::{PadState, WiimoteState};
    Some(WiimoteState {
        pad: PadState {
            buttons:   ch.pad_buttons,
            stick_x:   ch.stick_x,
            stick_y:   ch.stick_y,
            cstick_x:  ch.cstick_x,
            cstick_y:  ch.cstick_y,
            trigger_l: ch.trigger_l,
            trigger_r: ch.trigger_r,
        },
        ext_buttons: ch.ext_buttons,
        extension:   ch.extension,
        wii_buttons: ch.wii_buttons,
        ir_x:        ch.ir_x,
        ir_y:        ch.ir_y,
        accel_x:     ch.accel_x,
        accel_y:     ch.accel_y,
        accel_z:     ch.accel_z,
        connected:   1,
    })
}

/// Returns `true` if a Wiimote is synced and connected on `channel`.
pub unsafe fn wpad_connected(channel: u8) -> bool {
    if !WPAD_READY || channel >= 4 { return false; }
    WPAD_BUF.channels[channel as usize].err == 0
}
