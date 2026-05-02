//! # sine_wave — DevKit DOL RS
//!
//! Generates a 440 Hz stereo sine wave and plays it continuously through
//! the GameCube's Audio Interface DMA at 32 kHz. Displays audio status on
//! screen while the tone plays.
//!
//! ## Audio pipeline
//!
//! ```text
//! CPU generates PCM samples → MEM1 buffer (32-byte aligned)
//!     → AI DMA register (dspReg[24-27])
//!     → AI DAC → analogue output
//! ```
//!
//! ## Double-buffering strategy
//!
//! Two buffers (A and B) are pre-filled with one period's worth of samples.
//! The AI DMA interrupt fires when each buffer finishes playing. In the
//! callback we immediately start the other buffer (already filled). This
//! gives ~21 ms of buffer at 32 kHz with 682-sample buffers.
//!
//! ## Build & run
//!
//! ```sh
//! cargo +nightly build \
//!   -Z build-std=core,compiler_builtins \
//!   -Z build-std-features=compiler-builtins-mem \
//!   --target targets/powerpc-gekko-eabi.json \
//!   -p sine_wave --release
//!
//! cargo run -p elf2dol -- \
//!   target/powerpc-gekko-eabi/release/sine_wave sine_wave.dol
//!
//! dolphin-emu -e sine_wave.dol
//! ```

#![no_std]
#![no_main]

use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

use gc_gfx::{Console, Xfb, YcbcrPair, color};
use gc_hal::{vi, ai};
use gc_hal::ai::SampleRate;

// ─── Audio constants ──────────────────────────────────────────────────────────

/// DSP/DMA sample rate: 32,000 Hz (stereo i16)
const SAMPLE_RATE: u32 = 32_000;

/// Frequency of the tone in Hz
const TONE_HZ: u32 = 440;

/// Samples per stereo frame (L+R = 2 i16 values per frame)
/// Using 704 stereo frames = 1408 i16 values = 2816 bytes (multiple of 32).
/// 704 / 32 kHz ≈ 22 ms of audio per buffer.
const FRAMES_PER_BUF: usize = 704;
const SAMPLES_PER_BUF: usize = FRAMES_PER_BUF * 2; // L+R interleaved

// ─── Audio buffers (must be 32-byte aligned) ─────────────────────────────────

#[repr(C, align(32))]
struct AudioBuf([i16; SAMPLES_PER_BUF]);

static mut BUF_A: AudioBuf = AudioBuf([0i16; SAMPLES_PER_BUF]);
static mut BUF_B: AudioBuf = AudioBuf([0i16; SAMPLES_PER_BUF]);

/// 0 = BUF_A is currently playing, 1 = BUF_B is currently playing
static PLAYING_BUF: AtomicU32 = AtomicU32::new(0);
/// Count of DMA completions (for display)
static DMA_COUNT: AtomicU32 = AtomicU32::new(0);
/// Sine phase accumulator (Q16.16 fixed-point, 0..SAMPLE_RATE)
static PHASE: AtomicU32 = AtomicU32::new(0);

// ─── Framebuffer ─────────────────────────────────────────────────────────────

const FB_WIDTH:  u32 = 640;
const FB_HEIGHT: u32 = 480;
const FB_WORDS:  usize = (FB_WIDTH * FB_HEIGHT / 2) as usize;

#[repr(C, align(32))]
struct Fb([u32; FB_WORDS]);
static mut FRAMEBUFFER: Fb = Fb([0u32; FB_WORDS]);

// ─── Entry ───────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe { run() }
}

unsafe fn run() -> ! {
    // ── Video ─────────────────────────────────────────────────────────────
    vi::init_ntsc_480i();
    let fb_ptr = FRAMEBUFFER.0.as_mut_ptr();
    vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
    vi::flush();

    // ── Audio init ────────────────────────────────────────────────────────
    ai::init();
    ai::set_dsp_sample_rate(SampleRate::Hz32000);
    ai::set_volume(255, 255);

    // ── Pre-fill both buffers ─────────────────────────────────────────────
    fill_sine(&mut BUF_A.0, 0);
    fill_sine(&mut BUF_B.0, FRAMES_PER_BUF as u32);
    PHASE.store(FRAMES_PER_BUF as u32 * 2, Ordering::Relaxed);

    // ── Register DMA callback and start playback ──────────────────────────
    ai::register_dma_callback(dma_callback);
    // Start with buffer A
    PLAYING_BUF.store(0, Ordering::Release);
    ai::start_dma(BUF_A.0.as_ptr(), SAMPLES_PER_BUF * 2);

    // ── Main loop: update display ─────────────────────────────────────────
    let mut frame: u32 = 0;
    loop {
        let xfb = Xfb::from_raw(fb_ptr, FB_WIDTH, FB_HEIGHT);
        let bg = YcbcrPair::new(10, 128, 10, 128);
        let mut xfb = xfb;
        xfb.clear(bg);

        let mut con = Console::new(&mut xfb);
        con.set_bg(bg);

        con.set_fg(color::CYAN);
        con.print_str("\n  DevKit DOL RS -- Sine Wave Demo\n");
        con.set_fg(color::DARK_GREY);
        con.print_str("  ─────────────────────────────────────\n\n");

        con.set_fg(color::WHITE);
        con.print_str("  Audio: AI DMA @ 32 kHz, stereo i16\n");
        con.set_fg(color::YELLOW);
        let _ = write!(con, "  Tone:  {} Hz (A4 / concert pitch)\n", TONE_HZ);
        let _ = write!(con, "  Buffer: {} stereo frames = {} bytes\n",
                       FRAMES_PER_BUF, SAMPLES_PER_BUF * 2);

        con.set_fg(color::GREEN);
        let count = DMA_COUNT.load(Ordering::Relaxed);
        let _ = write!(con, "\n  DMA completions: {}\n", count);
        let _ = write!(con, "  Playing: Buffer {}\n",
                       if PLAYING_BUF.load(Ordering::Relaxed) == 0 { 'A' } else { 'B' });

        con.set_fg(color::LIGHT_GREY);
        let _ = write!(con, "\n  Uptime frames: {}\n", frame);

        con.flush();
        vi::set_framebuffer(fb_ptr, FB_WIDTH * 2);
        vi::flush();

        gc_rt::timer::delay_ms(16);
        frame = frame.wrapping_add(1);
    }
}

// ─── DMA callback (called from AI interrupt) ─────────────────────────────────

fn dma_callback() {
    unsafe {
        DMA_COUNT.fetch_add(1, Ordering::Relaxed);

        // Determine which buffer just finished and which is next
        let just_played = PLAYING_BUF.load(Ordering::Acquire);
        let next_buf = 1 - just_played;

        // Fill the buffer that just finished with fresh samples
        let phase = PHASE.load(Ordering::Relaxed);
        if just_played == 0 {
            fill_sine(&mut BUF_A.0, phase);
        } else {
            fill_sine(&mut BUF_B.0, phase);
        }
        PHASE.store(phase.wrapping_add(FRAMES_PER_BUF as u32), Ordering::Relaxed);

        // Start DMA on the next (already-filled) buffer
        PLAYING_BUF.store(next_buf, Ordering::Release);
        if next_buf == 0 {
            ai::start_dma(BUF_A.0.as_ptr(), SAMPLES_PER_BUF * 2);
        } else {
            ai::start_dma(BUF_B.0.as_ptr(), SAMPLES_PER_BUF * 2);
        }
    }
}

// ─── Sine wave generation ─────────────────────────────────────────────────────

/// Fill `buf` with TONE_HZ Hz stereo sine samples starting at frame `start_frame`.
///
/// Uses a fixed-point phase accumulator to maintain phase continuity
/// across buffer boundaries.
fn fill_sine(buf: &mut [i16], start_frame: u32) {
    // Phase increment per frame (Q16.16): (TONE_HZ << 16) / SAMPLE_RATE
    // = (440 * 65536) / 32000 = 902
    const PHASE_INC: u32 = (TONE_HZ << 16) / SAMPLE_RATE;

    let mut phase = (start_frame as u64 * PHASE_INC as u64) as u32;

    let mut i = 0;
    while i < buf.len() {
        // Normalised phase 0..65536 → angle 0..2π
        let amp = sinf_q16(phase); // returns value in -32768..32767
        buf[i] = amp;     // left channel
        buf[i + 1] = amp; // right channel (same for mono tone)
        phase = phase.wrapping_add(PHASE_INC);
        i += 2;
    }
}

/// Compute sin(x) where x is Q16.16 phase (0..65535 = 0..2π).
///
/// Returns a value in [-32767, 32767] suitable for direct use as i16 PCM.
/// Uses a 5-term Taylor series on the reduced quadrant.
fn sinf_q16(phase: u32) -> i16 {
    // Map phase to quadrant
    let p = phase & 0xFFFF; // 0..65535

    // Map to [0, 16384) (first half-period), track sign
    let (half, negative) = if p < 0x8000 { (p, false) } else { (p - 0x8000, true) };

    // Map to [0, 8192) (first quarter), track reflection
    let (quarter, reflect) = if half < 0x4000 { (half, false) } else { (0x8000 - half, true) };

    // quarter is now in [0, 16384) representing [0, π/2]
    // Convert to f32 angle
    let x = (quarter as f32) * (core::f32::consts::FRAC_PI_2 / 16384.0);

    // sin(x) Taylor: x - x³/6 + x⁵/120 - x⁷/5040
    let x2 = x * x;
    let s = x * (1.0 - x2 * (1.0 / 6.0 - x2 * (1.0 / 120.0 - x2 / 5040.0)));
    let s = if reflect { s } else { s }; // no change for reflect (sin is symmetric around π/2)
    let _ = reflect; // reflection already handled by quarter remapping

    let amp = (s * 32767.0) as i16;
    if negative { amp.wrapping_neg() } else { amp }
}
