//! Wii-native input dispatch.
//!
//! On Wii, the VFS checks the SI bus first (GC controllers still work in
//! the front ports). When SI reports `DeviceKind::None` for a given port,
//! it falls through to IOS-backed devices:
//!
//! ```text
//! Priority (highest → lowest) for each VFS path on Wii:
//!
//! /dev/hid/pN/std   → SI GC controller
//!                   → IOS Wiimote N (synthesised pad)
//!                   → disconnected
//!
//! /dev/hid/pN/kbd   → SI keyboard (PSO kbd or BlueRetro)
//!                   → IOS USB keyboard (shared across all ports)
//!                   → disconnected
//!
//! /dev/hid/pN/mouse → SI mouse or BlueRetro IR
//!                   → IOS Wiimote N IR (absolute coordinates)
//!                   → IOS USB mouse (shared)
//!                   → disconnected
//!
//! /dev/hid/pN/wii   → IOS Wiimote N (full raw state)
//!                   → disconnected
//! ```
//!
//! USB keyboard and mouse are "shared" devices — the first USB keyboard
//! found is reported on all ports' `/kbd` paths simultaneously, since
//! physically only one keyboard is usually present. Game code that needs
//! to distinguish input sources should open `/wii` directly.

use dkdol_hal::ios::{wpad, usb_hid};
use dkdol_hal::si::{WiimoteState, KbdState, MouseState};
use crate::{ControllerState, WiimoteState as VfsWiimoteState,
            KbdState as VfsKbdState, MouseState as VfsMouseState};

/// Initialise all Wii IOS backends.
///
/// Called from [`crate::init`] when the `wii` feature is enabled.
/// Safe to call multiple times (no-op after first call).
pub unsafe fn init() {
    wpad::wpad_init();
    usb_hid::usb_hid_init();
}

/// Poll all IOS backends for fresh data.
///
/// Called from [`crate::poll`] every frame. The SI bus is polled separately
/// in the main poll loop; this only refreshes IOS-side state.
pub unsafe fn poll() {
    wpad::wpad_poll();
}

/// Re-scan USB for newly connected devices.
///
/// Intended to be called infrequently (e.g. every few seconds) — USB
/// enumeration takes several milliseconds. The main poll loop calls this
/// on a rate-limited timer.
pub unsafe fn rescan_usb() {
    usb_hid::usb_hid_rescan();
}

// ─── Per-path dispatch ────────────────────────────────────────────────────────

/// Try to fill `state` from IOS Wiimote `channel`.
///
/// Returns `true` if a Wiimote is connected and the state was filled.
/// The returned `ControllerState` uses the synthesised GC pad layout
/// that IOS provides after extension mapping.
pub unsafe fn try_read_std(channel: u8) -> Option<ControllerState> {
    let wii = wpad::wpad_read(channel)?;
    Some(ControllerState {
        buttons:     wii.pad.buttons,
        stick_x:     wii.pad.stick_x,
        stick_y:     wii.pad.stick_y,
        cstick_x:    wii.pad.cstick_x,
        cstick_y:    wii.pad.cstick_y,
        trigger_l:   wii.pad.trigger_l,
        trigger_r:   wii.pad.trigger_r,
        connected:   1,
        ext_buttons: wii.ext_buttons,
        _pad:        [0u8; 6],
    })
}

/// Try to fill keyboard state from IOS USB HID.
///
/// USB keyboards are not port-specific — the same device appears on any
/// port that asks for it. Returns `None` if no USB keyboard is connected.
pub unsafe fn try_read_kbd(_channel: u8) -> Option<VfsKbdState> {
    let raw = usb_hid::usb_hid_read_kbd()?;
    Some(VfsKbdState {
        modifiers: raw.modifiers,
        _reserved: 0,
        keys:      raw.keys,
        connected: 1,
        _pad:      [0u8; 7],
    })
}

/// Try to fill mouse state from IOS.
///
/// Priority:
/// 1. Wiimote IR (absolute coordinates) for this channel.
/// 2. USB mouse (relative deltas, shared across ports).
pub unsafe fn try_read_mouse(channel: u8) -> Option<VfsMouseState> {
    // Wiimote IR takes priority over USB mouse for this channel
    if let Some(wii) = wpad::wpad_read(channel) {
        return Some(VfsMouseState {
            buttons:  (wii.wii_buttons & 0x0F) as u8, // A=bit3 etc.
            absolute: 1,
            dx:       if wii.ir_x == 0xFFFF { -1i16 } else { wii.ir_x as i16 },
            dy:       if wii.ir_y == 0xFFFF { -1i16 } else { wii.ir_y as i16 },
            scroll_y: 0,
            scroll_x: 0,
            connected: 1,
            _pad:     [0u8; 7],
        });
    }
    // Fall back to USB mouse (relative)
    let raw = usb_hid::usb_hid_read_mouse()?;
    Some(VfsMouseState {
        buttons:  raw.buttons,
        absolute: 0,
        dx:       raw.dx,
        dy:       raw.dy,
        scroll_y: raw.scroll_y,
        scroll_x: raw.scroll_x,
        connected: 1,
        _pad:     [0u8; 7],
    })
}

/// Try to fill full Wiimote state from IOS for `channel`.
pub unsafe fn try_read_wii(channel: u8) -> Option<VfsWiimoteState> {
    let raw = wpad::wpad_read(channel)?;
    Some(VfsWiimoteState {
        buttons:     raw.pad.buttons,
        stick_x:     raw.pad.stick_x,
        stick_y:     raw.pad.stick_y,
        cstick_x:    raw.pad.cstick_x,
        cstick_y:    raw.pad.cstick_y,
        trigger_l:   raw.pad.trigger_l,
        trigger_r:   raw.pad.trigger_r,
        ext_buttons: raw.ext_buttons,
        extension:   raw.extension,
        wii_buttons: raw.wii_buttons,
        ir_x:        raw.ir_x,
        ir_y:        raw.ir_y,
        accel_x:     raw.accel_x,
        accel_y:     raw.accel_y,
        accel_z:     raw.accel_z,
        connected:   1,
        _pad:        [0u8; 4],
    })
}

/// Returns true if IOS has a Wiimote on `channel` or any USB HID device.
pub unsafe fn wii_present(channel: u8) -> bool {
    wpad::wpad_connected(channel)
        || usb_hid::usb_kbd_connected()
        || usb_hid::usb_mouse_connected()
}
