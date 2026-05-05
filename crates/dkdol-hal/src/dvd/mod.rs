//! DVD Interface (DI) — disc drive control and sector reading.
//!
//! ## Register map (32-bit, base crate::mmio::addr(0x006000))
//!
//! | Index | Name      | Description                                      |
//! |-------|-----------|--------------------------------------------------|
//! | 0     | DISR      | Status register: interrupt flags (TC, BRK, CVR)  |
//! | 1     | DICVR     | Cover register: cover state + interrupt mask     |
//! | 2     | DICMD0    | Command word 0 (command opcode)                  |
//! | 3     | DICMD1    | Command word 1 (disc offset >> 2)                |
//! | 4     | DICMD2    | Command word 2 (transfer length)                 |
//! | 5     | DIMAR     | DMA MEM1 address (physical)                      |
//! | 6     | DILENGTH  | DMA transfer length (bytes)                      |
//! | 7     | DICR      | Control: DMA bit, START bit, interrupt mode bit  |
//! | 8     | DIIMMBUF  | Immediate data buffer (for non-DMA commands)     |
//! | 9     | DICFG     | Configuration register                           |
//!
//! ## Key commands
//!
//! | Opcode (CMD0) | Name          | Description                          |
//! |---------------|---------------|--------------------------------------|
//! | 0xA8000000    | READ          | Read sectors from disc via DMA        |
//! | 0xA8000040    | READ_DISKID   | Read disc ID (32 bytes)              |
//! | 0xAB000000    | SEEK          | Seek to disc offset                  |
//! | 0xE3000000    | STOP_MOTOR    | Spin down the disc motor             |
//! | 0x12000000    | INQUIRY       | Read drive firmware info             |
//!
//! ## Read protocol
//!
//! 1. Write CMD0 = 0xA8000000, CMD1 = offset>>2, CMD2 = length
//! 2. Write DMA address (DIMAR) and length (DILENGTH)
//! 3. Write DICR = DMA | START (0x03)
//! 4. Poll DISR bit 0 (TC) until set, or wait for DI interrupt
//! 5. Clear TC bit in DISR (write 1 to clear)

#![allow(dead_code)]

const DI_BASE: usize = crate::mmio::addr(0x006000);

// Register indices (u32)
const REG_DISR:     usize = 0;
const REG_DICVR:    usize = 1;
const REG_DICMD0:   usize = 2;
const REG_DICMD1:   usize = 3;
const REG_DICMD2:   usize = 4;
const REG_DIMAR:    usize = 5;
const REG_DILENGTH: usize = 6;
const REG_DICR:     usize = 7;
const REG_DIIMMBUF: usize = 8;
const REG_DICFG:    usize = 9;

// DISR bits
const DISR_TCINT:   u32 = 1 << 4; // Transfer complete interrupt
const DISR_BRKINT:  u32 = 1 << 6; // Break interrupt
const DISR_BRKINTMSK: u32 = 1 << 5;
const DISR_TCINTMSK:  u32 = 1 << 3;
const DISR_DEINT:   u32 = 1 << 2; // Device error interrupt
const DISR_DEINTMSK:u32 = 1 << 1;

// DICR bits
const DICR_DMA:   u32 = 1 << 1;
const DICR_START: u32 = 1 << 0;
const DICR_MODE:  u32 = 1 << 2; // 0=DMA, 1=immediate

// Cover register
const CVR_STATE:  u32 = 1 << 0; // 0=closed, 1=open

// Commands
const CMD_READ:     u32 = 0xA800_0000;
const CMD_SEEK:     u32 = 0xAB00_0000;
const CMD_INQUIRY:  u32 = 0x1200_0000;
const CMD_STOP_MTR: u32 = 0xE300_0000;
const CMD_SPIN_UP:  u32 = 0xE300_0100; // with motor mode = SPINMOTOR_UP

/// Minimum read size and alignment: 32 bytes.
pub const MIN_READ_ALIGN: usize = 32;
/// Maximum DMA read in a single command.
pub const MAX_READ_BYTES: usize = 0x00A0_0000; // 10 MB

#[inline(always)]
fn di(idx: usize) -> *mut u32 { (DI_BASE + idx * 4) as *mut u32 }

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DvdError {
    /// No disc in drive.
    NoDisk,
    /// Drive cover is open.
    CoverOpen,
    /// Transfer timed out.
    Timeout,
    /// Drive returned an error.
    DriveError,
    /// Buffer alignment or size error.
    AlignmentError,
}

pub type Result<T> = core::result::Result<T, DvdError>;

// ─── State ────────────────────────────────────────────────────────────────────

/// Callback type for async DVD operations.
pub type DvdCallback = fn(result: Result<()>);

static mut DI_CALLBACK: Option<DvdCallback> = None;

// ─── Disc ID (32 bytes at disc offset 0) ─────────────────────────────────────

/// GameCube disc identification header (first 32 bytes of disc).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DiscId {
    /// 4-character game ID (e.g. b"GALE")
    pub game_code:  [u8; 4],
    /// 2-character company code (e.g. b"01")
    pub maker_code: [u8; 2],
    /// Disc number (for multi-disc games)
    pub disc_num:   u8,
    /// Game version
    pub game_ver:   u8,
    /// 1 = audio streaming enabled
    pub audio_streaming: u8,
    /// Streaming buffer size
    pub stream_buf_size: u8,
    /// Padding
    pub _pad: [u8; 14],
    /// Magic number: 0xC2339F3D
    pub magic: u32,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialise the DVD interface.
///
/// Clears all interrupt flags and enables the TC interrupt.
///
/// # Safety
/// Must be called once during startup, after the exception system is ready.
pub unsafe fn init() {
    // Clear all pending interrupts (write 1 to clear)
    let sr = core::ptr::read_volatile(di(REG_DISR));
    core::ptr::write_volatile(di(REG_DISR), sr | DISR_TCINT | DISR_BRKINT | DISR_DEINT);

    // Enable TC interrupt
    let sr2 = core::ptr::read_volatile(di(REG_DISR));
    core::ptr::write_volatile(di(REG_DISR), sr2 | DISR_TCINTMSK);
}

/// Register an async completion callback.
pub unsafe fn register_callback(cb: DvdCallback) {
    DI_CALLBACK = Some(cb);
}

/// Return true if the drive cover is open.
pub unsafe fn cover_open() -> bool {
    core::ptr::read_volatile(di(REG_DICVR)) & CVR_STATE != 0
}

/// Wait for the drive to complete the last command by polling TC bit.
///
/// Times out after approximately 10 seconds.
pub unsafe fn wait_ready() -> Result<()> {
    let deadline = dkdol_rt::timer::tbr64() + 40_500_000u64 * 10; // 10 s
    loop {
        let sr = core::ptr::read_volatile(di(REG_DISR));
        if sr & DISR_TCINT != 0 {
            // Clear TC
            core::ptr::write_volatile(di(REG_DISR),
                (sr & !(DISR_DEINT | DISR_BRKINT)) | DISR_TCINT);
            return Ok(());
        }
        if sr & DISR_DEINT != 0 {
            core::ptr::write_volatile(di(REG_DISR),
                (sr & !(DISR_TCINT | DISR_BRKINT)) | DISR_DEINT);
            return Err(DvdError::DriveError);
        }
        if dkdol_rt::timer::tbr64() > deadline {
            return Err(DvdError::Timeout);
        }
    }
}

/// Read data from disc into `buf`, blocking until complete.
///
/// `offset`: byte offset on disc (must be 32-byte aligned).
/// `buf`: destination buffer in MEM1, must be 32-byte aligned.
/// `len`: byte count, must be a multiple of 32 and ≤ `MAX_READ_BYTES`.
///
/// On a GC disc, user data starts at offset 0x20000 (after the header area).
///
/// # Safety
/// - `buf` must be 32-byte aligned.
/// - `len` must be a multiple of 32.
/// - Must not overlap the GX FIFO or other hardware-mapped regions.
pub unsafe fn read(buf: *mut u8, len: usize, offset: u64) -> Result<()> {
    if buf as usize % 32 != 0 { return Err(DvdError::AlignmentError); }
    if len % 32 != 0 { return Err(DvdError::AlignmentError); }
    if cover_open() { return Err(DvdError::CoverOpen); }

    let phys = (buf as usize) & 0x1FFF_FFFF;

    // CMD0 = READ opcode
    core::ptr::write_volatile(di(REG_DICMD0), CMD_READ);
    // CMD1 = offset >> 2 (disc addresses are in 4-byte units)
    core::ptr::write_volatile(di(REG_DICMD1), (offset >> 2) as u32);
    // CMD2 = length (bytes)
    core::ptr::write_volatile(di(REG_DICMD2), len as u32);
    // DMA address and length
    core::ptr::write_volatile(di(REG_DIMAR), phys as u32);
    core::ptr::write_volatile(di(REG_DILENGTH), len as u32);
    // Start DMA
    core::ptr::write_volatile(di(REG_DICR), DICR_DMA | DICR_START);

    wait_ready()
}

/// Read the disc identification header (first 32 bytes).
pub unsafe fn read_disc_id() -> Result<DiscId> {
    // DiscId must be read to a 32-byte aligned buffer
    #[repr(C, align(32))]
    struct IdBuf([u8; 32]);
    static mut ID_BUF: IdBuf = IdBuf([0u8; 32]);

    core::ptr::write_volatile(di(REG_DICMD0), 0xA800_0040); // READ_DISKID
    core::ptr::write_volatile(di(REG_DICMD1), 0);
    core::ptr::write_volatile(di(REG_DICMD2), 32);
    let phys = (ID_BUF.0.as_ptr() as usize) & 0x1FFF_FFFF;
    core::ptr::write_volatile(di(REG_DIMAR), phys as u32);
    core::ptr::write_volatile(di(REG_DILENGTH), 32);
    core::ptr::write_volatile(di(REG_DICR), DICR_DMA | DICR_START);

    wait_ready()?;

    Ok(*(ID_BUF.0.as_ptr() as *const DiscId))
}

/// Seek to a disc offset (non-blocking).
pub unsafe fn seek(offset: u64) -> Result<()> {
    core::ptr::write_volatile(di(REG_DICMD0), CMD_SEEK);
    core::ptr::write_volatile(di(REG_DICMD1), (offset >> 2) as u32);
    core::ptr::write_volatile(di(REG_DICMD2), 0);
    core::ptr::write_volatile(di(REG_DICR), DICR_START);
    wait_ready()
}

/// Spin up the disc motor.
pub unsafe fn spin_up() -> Result<()> {
    core::ptr::write_volatile(di(REG_DICMD0), CMD_SPIN_UP);
    core::ptr::write_volatile(di(REG_DICMD1), 0);
    core::ptr::write_volatile(di(REG_DICMD2), 0);
    core::ptr::write_volatile(di(REG_DICR), DICR_START);
    wait_ready()
}

/// Stop the disc motor.
pub unsafe fn stop_motor() -> Result<()> {
    core::ptr::write_volatile(di(REG_DICMD0), CMD_STOP_MTR);
    core::ptr::write_volatile(di(REG_DICMD1), 0);
    core::ptr::write_volatile(di(REG_DICMD2), 0);
    core::ptr::write_volatile(di(REG_DICR), DICR_START);
    wait_ready()
}

/// Called from the DI interrupt handler.
#[no_mangle]
pub unsafe extern "C" fn __dvd_tc_handler() {
    let sr = core::ptr::read_volatile(di(REG_DISR));
    core::ptr::write_volatile(di(REG_DISR),
        (sr & !(DISR_BRKINT | DISR_DEINT)) | DISR_TCINT);

    if let Some(cb) = DI_CALLBACK {
        cb(Ok(()));
    }
}

// ── DvdDisk wrapper struct ────────────────────────────────────────────────────

/// A handle to the DVD drive, used as a block device by dkdol-fs.
///
/// Obtain one after a successful `dvd::init()`:
/// ```rust,no_run
/// unsafe {
///     dkdol_hal::dvd::init();
///     let disk = dkdol_hal::dvd::DvdDisk;
/// }
/// ```
pub struct DvdDisk;
