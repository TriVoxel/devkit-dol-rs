//! USB HID — keyboard and mouse via IOS USB host controller.
//!
//! The Wii has two USB 2.0 host controllers managed by IOS:
//!
//! | Path          | Description                             |
//! |---------------|-----------------------------------------|
//! | `/dev/usb/oh0`| OHCI 0 — internal (BT chip, front USB) |
//! | `/dev/usb/oh1`| OHCI 1 — external USB ports (rear)     |
//!
//! This module uses `/dev/usb/oh1` for keyboards and mice plugged into
//! the Wii's rear USB ports. The interface is standard IOS USB HID, which
//! presents attached HID devices as a list of handles that can be read for
//! raw HID reports.
//!
//! ## HID report parsing
//!
//! Raw HID reports from keyboards and mice follow the standard USB HID
//! Boot Protocol, which has a fixed, well-known format independent of
//! device-specific report descriptors:
//!
//! ### Keyboard boot report (8 bytes)
//! ```text
//! Byte 0: Modifier keys
//! Byte 1: Reserved (OEM)
//! Bytes 2–7: Up to 6 simultaneous keycodes (HID Usage Page 0x07)
//! ```
//!
//! ### Mouse boot report (3–5 bytes)
//! ```text
//! Byte 0: Button bitmask (bit 0=left, 1=right, 2=middle)
//! Byte 1: X delta (i8)
//! Byte 2: Y delta (i8)
//! Byte 3: Vertical scroll (i8, optional)
//! Byte 4: Horizontal scroll (i8, optional)
//! ```
//!
//! ## Verification note
//!
//! The IOCTL command numbers below match the IOS USB HID interface as
//! documented on WiiBrew and in libogc's USB sources. The device class
//! constants (keyboard = 0x01, mouse = 0x02) are standard USB HID values.

use super::{ios_close, ios_ioctl, ios_open, dcbi, dcbf, IOS_OK};
use crate::si::{KbdState, MouseState};

// ─── USB HID IOCTL commands ───────────────────────────────────────────────────

/// Enumerate attached HID devices. Fills the device table.
const USB_HID_IOCTL_GET_DEVICES:  u32 = 0x00;
/// Open a specific HID device for reading.
const USB_HID_IOCTL_OPEN:         u32 = 0x01;
/// Read one HID report from an open device.
const USB_HID_IOCTL_READ:         u32 = 0x02;
/// Set the HID boot protocol (vs. report protocol) on a device.
const USB_HID_IOCTL_SET_PROTOCOL: u32 = 0x0A;

// ─── USB HID device descriptor ───────────────────────────────────────────────

/// HID device entry returned by GET_DEVICES.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UsbHidDevice {
    /// Vendor ID.
    vid:        u16,
    /// Product ID.
    pid:        u16,
    /// HID interface subclass: 0x01 = Boot Interface.
    subclass:   u8,
    /// HID protocol: 0x01 = Keyboard, 0x02 = Mouse.
    protocol:   u8,
    /// IOS internal device handle.
    handle:     u32,
}

const USB_PROTOCOL_KEYBOARD: u8 = 0x01;
const USB_PROTOCOL_MOUSE:    u8 = 0x02;

const MAX_HID_DEVICES: usize = 8;

// ─── State ───────────────────────────────────────────────────────────────────

static mut OH1_FD:      i32 = -1;
static mut USB_READY:   bool = false;
static mut KBD_HANDLE:  u32 = u32::MAX;
static mut MOUSE_HANDLE: u32 = u32::MAX;

#[repr(C, align(32))]
struct DeviceTable([UsbHidDevice; MAX_HID_DEVICES]);

static mut DEVICE_TABLE: DeviceTable = DeviceTable([UsbHidDevice {
    vid: 0, pid: 0, subclass: 0, protocol: 0, handle: u32::MAX,
}; MAX_HID_DEVICES]);

/// Last received keyboard report.
#[repr(C, align(32))]
struct KbdReport([u8; 8]);
static mut KBD_REPORT: KbdReport = KbdReport([0u8; 8]);

/// Last received mouse report.
#[repr(C, align(32))]
struct MouseReport([u8; 8]);
static mut MOUSE_REPORT: MouseReport = MouseReport([0u8; 8]);

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialise USB HID and discover attached keyboard/mouse.
///
/// Opens `/dev/usb/oh1`, enumerates HID devices, and caches handles for
/// the first keyboard and first mouse found. Call once at startup.
///
/// Returns `true` if at least one HID device was found.
pub unsafe fn usb_hid_init() -> bool {
    if USB_READY { return true; }

    let fd = ios_open(b"/dev/usb/oh1\0", 0);
    if fd < 0 { return false; }
    OH1_FD = fd;

    // Enumerate devices
    dcbf(&DEVICE_TABLE as *const _ as usize, core::mem::size_of::<DeviceTable>());
    let r = ios_ioctl(fd, USB_HID_IOCTL_GET_DEVICES, &[],
        core::slice::from_raw_parts_mut(
            DEVICE_TABLE.0.as_mut_ptr() as *mut u8,
            core::mem::size_of::<DeviceTable>()));
    if r < 0 { ios_close(fd); OH1_FD = -1; return false; }

    // Find keyboard and mouse by HID protocol field
    for dev in &DEVICE_TABLE.0 {
        if dev.handle == u32::MAX { continue; }
        if dev.protocol == USB_PROTOCOL_KEYBOARD && KBD_HANDLE == u32::MAX {
            // Switch to boot protocol for predictable report format
            let mut proto = [0u8; 4];
            proto[0..4].copy_from_slice(&dev.handle.to_be_bytes());
            ios_ioctl(fd, USB_HID_IOCTL_SET_PROTOCOL,
                &proto, &mut []);
            KBD_HANDLE = dev.handle;
        }
        if dev.protocol == USB_PROTOCOL_MOUSE && MOUSE_HANDLE == u32::MAX {
            let mut proto = [0u8; 4];
            proto[0..4].copy_from_slice(&dev.handle.to_be_bytes());
            ios_ioctl(fd, USB_HID_IOCTL_SET_PROTOCOL,
                &proto, &mut []);
            MOUSE_HANDLE = dev.handle;
        }
    }

    USB_READY = true;
    KBD_HANDLE != u32::MAX || MOUSE_HANDLE != u32::MAX
}

/// Re-enumerate USB devices to detect newly plugged-in hardware.
///
/// Call from your `poll()` loop at a low frequency (e.g. every 2 seconds).
pub unsafe fn usb_hid_rescan() {
    if OH1_FD < 0 { return; }
    KBD_HANDLE   = u32::MAX;
    MOUSE_HANDLE = u32::MAX;
    USB_READY    = false;
    usb_hid_init();
}

/// Read the current keyboard state.
///
/// Issues a USB HID report read (8 bytes, boot protocol). Returns `None`
/// if no keyboard is connected or the read fails.
pub unsafe fn usb_hid_read_kbd() -> Option<KbdState> {
    if OH1_FD < 0 || KBD_HANDLE == u32::MAX { return None; }

    // Build ioctl args: [handle (u32), report_type (u8 = 1 = input), report_id (u8 = 0)]
    let mut args = [0u8; 8];
    args[0..4].copy_from_slice(&KBD_HANDLE.to_be_bytes());
    args[4] = 1; // HID_REPORT_TYPE_INPUT
    args[5] = 0; // report ID 0 = no report ID (boot protocol)

    dcbf(&KBD_REPORT as *const _ as usize, 8);
    let r = ios_ioctl(OH1_FD, USB_HID_IOCTL_READ, &args,
        core::slice::from_raw_parts_mut(KBD_REPORT.0.as_mut_ptr(), 8));

    if r < 0 { return None; }
    dcbi(&KBD_REPORT as *const _ as usize, 8);

    // Boot protocol keyboard report: [modifiers, reserved, key0..key5]
    let rep = &KBD_REPORT.0;
    let mut keys = [0u8; 6];
    keys.copy_from_slice(&rep[2..8]);

    Some(KbdState {
        modifiers: rep[0],
        keys,
        connected: 1,
    })
}

/// Read the current mouse state.
///
/// Issues a USB HID report read (up to 5 bytes, boot protocol). Returns
/// `None` if no mouse is connected or the read fails.
pub unsafe fn usb_hid_read_mouse() -> Option<MouseState> {
    if OH1_FD < 0 || MOUSE_HANDLE == u32::MAX { return None; }

    let mut args = [0u8; 8];
    args[0..4].copy_from_slice(&MOUSE_HANDLE.to_be_bytes());
    args[4] = 1; // HID_REPORT_TYPE_INPUT
    args[5] = 0;

    dcbf(&MOUSE_REPORT as *const _ as usize, 8);
    let r = ios_ioctl(OH1_FD, USB_HID_IOCTL_READ, &args,
        core::slice::from_raw_parts_mut(MOUSE_REPORT.0.as_mut_ptr(), 8));

    if r < 0 { return None; }
    dcbi(&MOUSE_REPORT as *const _ as usize, 8);

    // Boot protocol mouse report: [buttons, dx_i8, dy_i8, scroll_y_i8?, scroll_x_i8?]
    let rep = &MOUSE_REPORT.0;
    Some(MouseState {
        buttons:  rep[0],
        dx:       rep[1] as i8 as i16,
        dy:       rep[2] as i8 as i16,
        scroll_y: if r >= 4 { rep[3] as i8 } else { 0 },
        scroll_x: if r >= 5 { rep[4] as i8 } else { 0 },
        connected: 1,
    })
}

/// Returns `true` if a USB keyboard is present.
pub unsafe fn usb_kbd_connected() -> bool { USB_READY && KBD_HANDLE != u32::MAX }

/// Returns `true` if a USB mouse is present.
pub unsafe fn usb_mouse_connected() -> bool { USB_READY && MOUSE_HANDLE != u32::MAX }
