//! IOS IPC — Wii I/O co-processor communication.
//!
//! The Wii has a second CPU: an ARM926EJ-S called the Starlet, running
//! Nintendo's proprietary IOS firmware. IOS owns Bluetooth, USB, the NAND,
//! the SD slot (on some IOS versions), and several other peripherals. The
//! main PowerPC (Broadway) communicates with IOS through a hardware mailbox
//! located at `0xCD000000`.
//!
//! ## IPC hardware registers
//!
//! ```text
//! 0xCD000000  IPC_PPCMSG   PPC writes physical address of IpcMsg here
//! 0xCD000004  IPC_PPCCTRL  Control and status bits (see below)
//! 0xCD000008  IPC_ARMMSG   IOS writes physical address of completed msg here
//! ```
//!
//! ### IPC_PPCCTRL bits
//!
//! | Bit | Name | Direction | Meaning                              |
//! |-----|------|-----------|--------------------------------------|
//! |  0  | X1   | PPC→IOS   | Set to send a new request to IOS     |
//! |  1  | X2   | PPC→IOS   | Set to acknowledge IOS's Y1 signal   |
//! |  2  | Y1   | IOS→PPC   | IOS sets when a reply is ready       |
//! |  3  | Y2   | IOS→PPC   | IOS sets to ack PPC's X1; PPC clears |
//!
//! ## IPC message format
//!
//! ```text
//! Offset  Size  Field
//!    0      4   cmd     — IPC command (see IpcCmd)
//!    4      4   result  — return value (filled by IOS, ≤0 is error)
//!    8      4   fd      — file descriptor (for fd-using commands)
//!   12     20   args    — command-specific arguments (5 × u32)
//! ```
//!
//! The message must be 32-byte aligned (one PowerPC cache line) and must
//! be flushed to RAM before IOS reads it, and cache-invalidated after IOS
//! writes the result.
//!
//! ## Error codes
//!
//! IOS returns negative POSIX-style error codes. Common ones:
//!
//! | Code | Constant       | Meaning                  |
//! |------|----------------|--------------------------|
//! |  0   | `IOS_OK`       | Success                  |
//! | -1   | `IOS_EINVAL`   | Invalid argument         |
//! | -2   | `IOS_ACCESS`   | Permission denied        |
//! | -4   | `IOS_ENOENT`   | Device / path not found  |
//! | -6   | `IOS_ENOMEM`   | Out of memory            |
//! | -29  | `IOS_TIMEOUT`  | Operation timed out      |

#![allow(dead_code)]

use core::ptr;

// ─── Hardware registers ───────────────────────────────────────────────────────

const IPC_PPCMSG:  usize = 0xCD00_0000;
const IPC_PPCCTRL: usize = 0xCD00_0004;
const IPC_ARMMSG:  usize = 0xCD00_0008;

const IPC_X1: u32 = 1 << 0; // PPC→IOS: new request pending
const IPC_X2: u32 = 1 << 1; // PPC→IOS: ack IOS reply
const IPC_Y1: u32 = 1 << 2; // IOS→PPC: reply ready
const IPC_Y2: u32 = 1 << 3; // IOS→PPC: ack PPC request

// ─── IOS error codes ──────────────────────────────────────────────────────────

pub const IOS_OK:      i32 =  0;
pub const IOS_EINVAL:  i32 = -1;
pub const IOS_ACCESS:  i32 = -2;
pub const IOS_ENOENT:  i32 = -4;
pub const IOS_ENOMEM:  i32 = -6;
pub const IOS_TIMEOUT: i32 = -29;

// ─── IPC message ─────────────────────────────────────────────────────────────

/// IPC command numbers.
#[repr(u32)]
enum IpcCmd {
    Open    = 1,
    Close   = 2,
    Read    = 3,
    Write   = 4,
    Seek    = 5,
    Ioctl   = 6,
    Ioctlv  = 7,
}

/// One IPC message slot. Must be 32-byte aligned (one cache line).
///
/// The PPC fills in `cmd`, `fd`, and `args`, then hands it to IOS.
/// IOS writes the result into `result` and signals back.
#[repr(C, align(32))]
struct IpcMsg {
    cmd:    u32,
    result: i32,
    fd:     i32,
    args:   [u32; 5],
}

/// Static pool of IPC message slots (one per concurrent call).
/// We are single-threaded, so one slot is enough.
static mut MSG: IpcMsg = IpcMsg { cmd: 0, result: 0, fd: 0, args: [0u32; 5] };

// ─── Cache operations (PowerPC) ───────────────────────────────────────────────

/// Flush (write back) dirty cache lines covering `[addr, addr+len)` to RAM.
///
/// Must be called before IOS reads a buffer the PPC has written.
pub unsafe fn dcbf(addr: usize, len: usize) {
    let start = addr & !31usize;
    let end   = (addr + len + 31) & !31usize;
    let mut p = start;
    while p < end {
        core::arch::asm!("dcbf 0, {r}", r = in(reg) p);
        p += 32;
    }
    core::arch::asm!("sync");
}

/// Invalidate cache lines covering `[addr, addr+len)`.
///
/// Must be called after IOS has written to a buffer, so the PPC sees
/// the new data rather than its cached (stale) copy.
pub unsafe fn dcbi(addr: usize, len: usize) {
    let start = addr & !31usize;
    let end   = (addr + len + 31) & !31usize;
    let mut p = start;
    while p < end {
        core::arch::asm!("dcbi 0, {r}", r = in(reg) p);
        p += 32;
    }
    core::arch::asm!("sync");
}

/// Convert a virtual address to the physical address IOS uses.
///
/// On Wii:
/// - MEM1 virtual `0x8000_0000` → physical `0x0000_0000`
/// - MEM2 virtual `0x9000_0000` → physical `0x1000_0000`
///
/// Both regions map with the same formula: `phys = virt & 0x1FFF_FFFF`.
#[inline(always)]
fn to_phys(virt: usize) -> u32 {
    (virt & 0x1FFF_FFFF) as u32
}

// ─── IPC transact ─────────────────────────────────────────────────────────────

/// Send `MSG` to IOS and busy-wait for the reply.
///
/// The caller must have filled `MSG.cmd`, `MSG.fd`, and `MSG.args` before
/// calling this. On return, `MSG.result` contains IOS's return value.
///
/// # Safety
/// Must be called from a single-threaded context (bare metal).
unsafe fn transact() -> i32 {
    let msg_addr = &MSG as *const IpcMsg as usize;

    // Flush our written fields to RAM before IOS reads them
    dcbf(msg_addr, 32);

    // Deliver message address to IOS and trigger processing
    ptr::write_volatile(IPC_PPCMSG  as *mut u32, to_phys(msg_addr));
    ptr::write_volatile(IPC_PPCCTRL as *mut u32, IPC_X1 | IPC_Y2);

    // Wait for IOS to set Y1 (reply ready)
    let mut timeout = 0x1000_0000u32;
    loop {
        let ctrl = ptr::read_volatile(IPC_PPCCTRL as *mut u32);
        if ctrl & IPC_Y1 != 0 { break; }
        timeout -= 1;
        if timeout == 0 { return IOS_TIMEOUT; }
    }

    // Acknowledge reply and clear Y1
    ptr::write_volatile(IPC_PPCCTRL as *mut u32, IPC_Y1 | IPC_X2);

    // Invalidate cache so we read IOS's written result field
    dcbi(msg_addr, 32);

    MSG.result
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Open an IOS device by path.
///
/// Returns a non-negative file descriptor on success, or a negative IOS
/// error code on failure.
///
/// Common paths:
/// - `"/dev/btm"` — Bluetooth manager (Wiimote)
/// - `"/dev/usb/oh1"` — USB host controller (external USB ports)
/// - `"/dev/usb/hid"` — USB HID device interface
pub unsafe fn ios_open(path: &[u8], mode: u32) -> i32 {
    // Path must be NUL-terminated and in RAM IOS can access.
    // We store it in a stack buffer and flush before use.
    let mut buf = [0u8; 64];
    let len = path.len().min(63);
    buf[..len].copy_from_slice(&path[..len]);
    dcbf(buf.as_ptr() as usize, 64);

    MSG.cmd    = IpcCmd::Open as u32;
    MSG.result = 0;
    MSG.fd     = 0;
    MSG.args[0]= to_phys(buf.as_ptr() as usize);
    MSG.args[1]= mode;
    MSG.args[2]= 0; MSG.args[3]= 0; MSG.args[4]= 0;

    transact()
}

/// Close an IOS file descriptor.
pub unsafe fn ios_close(fd: i32) -> i32 {
    MSG.cmd    = IpcCmd::Close as u32;
    MSG.result = 0;
    MSG.fd     = fd;
    MSG.args   = [0u32; 5];
    transact()
}

/// Issue an ioctl to an IOS device.
///
/// `in_buf` is data sent TO IOS; `out_buf` is filled BY IOS.
/// Either may be empty (pass `&[]` / `&mut []`).
pub unsafe fn ios_ioctl(
    fd:      i32,
    cmd:     u32,
    in_buf:  &[u8],
    out_buf: &mut [u8],
) -> i32 {
    if !in_buf.is_empty() { dcbf(in_buf.as_ptr() as usize, in_buf.len()); }
    if !out_buf.is_empty() { dcbf(out_buf.as_ptr() as usize, out_buf.len()); }

    MSG.cmd    = IpcCmd::Ioctl as u32;
    MSG.result = 0;
    MSG.fd     = fd;
    MSG.args[0]= cmd;
    MSG.args[1]= if in_buf.is_empty()  { 0 } else { to_phys(in_buf.as_ptr()  as usize) };
    MSG.args[2]= in_buf.len() as u32;
    MSG.args[3]= if out_buf.is_empty() { 0 } else { to_phys(out_buf.as_ptr() as usize) };
    MSG.args[4]= out_buf.len() as u32;

    let r = transact();

    // Invalidate output buffer so PPC reads IOS's data
    if !out_buf.is_empty() { dcbi(out_buf.as_ptr() as usize, out_buf.len()); }
    r
}

/// Issue an ioctlv (scatter-gather ioctl) to an IOS device.
///
/// `in_vecs` are buffers sent TO IOS; `out_vecs` are filled BY IOS.
pub unsafe fn ios_ioctlv(
    fd:       i32,
    cmd:      u32,
    in_vecs:  &[(*const u8, u32)],
    out_vecs: &[(*mut u8, u32)],
) -> i32 {
    // Build the ioctlv vector table. Each entry is (u32 phys_addr, u32 len).
    // We need this in RAM IOS can read; use a small stack buffer.
    let total = in_vecs.len() + out_vecs.len();
    let mut vtable = [0u32; 32]; // max 16 in + 16 out vectors
    debug_assert!(total <= 16, "ios_ioctlv: too many vectors");

    for (i, &(ptr, len)) in in_vecs.iter().enumerate() {
        dcbf(ptr as usize, len as usize);
        vtable[i * 2    ] = to_phys(ptr as usize);
        vtable[i * 2 + 1] = len;
    }
    for (i, &(ptr, len)) in out_vecs.iter().enumerate() {
        dcbf(ptr as usize, len as usize);
        let j = (in_vecs.len() + i) * 2;
        vtable[j    ] = to_phys(ptr as usize);
        vtable[j + 1] = len;
    }
    dcbf(vtable.as_ptr() as usize, core::mem::size_of_val(&vtable));

    MSG.cmd    = IpcCmd::Ioctlv as u32;
    MSG.result = 0;
    MSG.fd     = fd;
    MSG.args[0]= cmd;
    MSG.args[1]= in_vecs.len() as u32;
    MSG.args[2]= to_phys(vtable.as_ptr() as usize);
    MSG.args[3]= out_vecs.len() as u32;
    MSG.args[4]= to_phys(vtable[in_vecs.len() * 2..].as_ptr() as usize);

    let r = transact();

    for &(ptr, len) in out_vecs { dcbi(ptr as usize, len as usize); }
    r
}

pub mod wpad;
pub mod usb_hid;
