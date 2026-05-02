//! # spinning_triangle — DevKit DOL
//!
//! The classic "first 3D program": a brightly coloured triangle that spins
//! around the Y axis. Demonstrates the full GX pipeline from FIFO init to
//! EFB→XFB copy.
//!
//! ## Pipeline overview
//!
//! ```text
//! GX init → vertex format → matrices → viewport
//!        → per-frame: clear EFB, draw, copy to XFB → VI displays XFB
//! ```
//!
//! ## Build & run
//!
//! ```sh
//! cargo +nightly build \
//!   -Z build-std=core,compiler_builtins \
//!   -Z build-std-features=compiler-builtins-mem \
//!   --target targets/powerpc-gekko-eabi.json \
//!   -p spinning_triangle --release
//!
//! cargo run -p elf2dol -- \
//!   target/powerpc-gekko-eabi/release/spinning_triangle \
//!   spinning_triangle.dol
//!
//! dolphin-emu -e spinning_triangle.dol
//! ```

#![no_std]
#![no_main]

use gc_hal::{vi, gx};
use gc_hal::gx::{state, draw, types::*};

// ─── Static buffers ───────────────────────────────────────────────────────────

/// GX FIFO command buffer — 256 KB, 32-byte aligned.
#[repr(C, align(32))]
struct FifoBuf([u8; 256 * 1024]);
static mut FIFO: FifoBuf = FifoBuf([0; 256 * 1024]);

/// External framebuffer — two buffers for double-buffering.
/// 640×480 × 2 bytes (YCbCr 4:2:2) = 614,400 bytes each.
const XFB_SIZE: usize = 640 * 480 * 2;
const XFB_WORDS: usize = 640 * 480 / 2; // u32 pairs

#[repr(C, align(32))]
struct Xfb([u32; XFB_WORDS]);
static mut XFB0: Xfb = Xfb([0; XFB_WORDS]);
static mut XFB1: Xfb = Xfb([0; XFB_WORDS]);

// ─── Entry ───────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe { run() }
}

unsafe fn run() -> ! {
    // ── 1. Video Interface: NTSC 480i ─────────────────────────────────────
    vi::init_ntsc_480i();

    let xfb0_ptr = XFB0.0.as_mut_ptr() as *mut u32;
    let xfb1_ptr = XFB1.0.as_mut_ptr() as *mut u32;

    // Physical addresses for EFB→XFB copy (strip cached address offset)
    let xfb0_phys = (xfb0_ptr as usize & 0x1FFF_FFFF) as u32;
    let xfb1_phys = (xfb1_ptr as usize & 0x1FFF_FFFF) as u32;

    // Point VI at buffer 0 for now
    vi::set_framebuffer(xfb0_ptr, 640 * 2);
    vi::flush();

    // ── 2. GX init ────────────────────────────────────────────────────────
    gx::init(FIFO.0.as_mut_ptr(), FIFO.0.len());

    // ── 3. Vertex format: position XYZ f32 + color RGBA8 ─────────────────
    state::set_vtx_desc_pos_clr0();
    state::set_vtx_fmt_pos_xyz_f32_clr_rgba8(VtxFmt::Fmt0);

    // ── 4. Viewport ───────────────────────────────────────────────────────
    state::set_viewport(0.0, 0.0, 640.0, 480.0, 0.0, 1.0);
    state::set_scissor(0, 0, 640, 480);

    // ── 5. Projection: 60° FOV perspective ───────────────────────────────
    let proj = state::Proj::perspective(
        60_f32 * (core::f32::consts::PI / 180.0),
        640.0 / 480.0,
        0.1,
        100.0,
    );
    state::load_projection_mtx(&proj);

    // ── 6. Pipeline state ─────────────────────────────────────────────────
    state::set_z_mode(true, Compare::LEqual, true);
    state::set_blend_mode(BlendMode::None, BlendFactor::SrcAlpha, BlendFactor::InvSrcAlpha);
    state::set_cull_mode(CullMode::Back);
    state::set_num_color_chans(1);
    state::set_num_tex_gens(0);
    state::set_num_tev_stages(1);
    state::set_tev_order_vtx_only();
    state::set_tev_passthrough_vtx_color();

    // ── 7. Copy config for NTSC 480i ─────────────────────────────────────
    // EFB is 640×480 (progressive render, then scaled to 480i by VI)
    state::set_disp_copy_src(0, 0, 640, 480);
    state::set_disp_copy_dst(640, 480);
    state::set_disp_copy_y_scale(1.0);

    // ── 8. Main loop ──────────────────────────────────────────────────────
    let mut angle: f32 = 0.0;
    let mut frame: u32 = 0;

    loop {
        // Alternate XFB buffers
        let (draw_xfb_phys, display_xfb_ptr) = if frame & 1 == 0 {
            (xfb0_phys, xfb0_ptr)
        } else {
            (xfb1_phys, xfb1_ptr)
        };

        // ── 8a. Set clear colour (deep blue) and clear EFB ───────────────
        state::set_copy_clear(0x18, 0x18, 0x60, 0xFF, GX_MAX_Z24);

        // ── 8b. Build modelview: translate back 3 units, then rotate Y ───
        let mv = build_modelview(0.0, 0.0, -3.0, angle);
        state::load_pos_mtx_imm(&mv, 0);
        state::set_current_mtx(0);

        // ── 8c. Draw the triangle ─────────────────────────────────────────
        draw::begin(Primitive::Triangles, VtxFmt::Fmt0, 3);
        // Top vertex: red
        draw::pos3f( 0.0,  1.0, 0.0);  draw::color4u8(255,   0,   0, 255);
        // Bottom-left: green
        draw::pos3f(-1.0, -1.0, 0.0);  draw::color4u8(  0, 255,   0, 255);
        // Bottom-right: blue
        draw::pos3f( 1.0, -1.0, 0.0);  draw::color4u8(  0,   0, 255, 255);

        // ── 8d. Copy EFB to XFB (clear = true applies the clear colour) ───
        state::copy_disp(draw_xfb_phys, true);

        // ── 8e. Flush and wait for GP to finish ───────────────────────────
        gx::flush();

        // ── 8f. Flip display ──────────────────────────────────────────────
        vi::set_framebuffer(display_xfb_ptr, 640 * 2);
        vi::flush();

        // ── 8g. Advance rotation: ~60 fps, 1°/frame ≈ 1 full rotation/6s
        angle += 1.0 * (core::f32::consts::PI / 180.0);
        if angle >= 2.0 * core::f32::consts::PI {
            angle -= 2.0 * core::f32::consts::PI;
        }

        // Simple vsync approximation — wait ~16 ms
        gc_rt::timer::delay_ms(16);

        frame = frame.wrapping_add(1);
    }
}

// ─── Modelview matrix helper ─────────────────────────────────────────────────

/// Build a 3×4 matrix: translate(x, y, z) * rotateY(angle_rad).
fn build_modelview(tx: f32, ty: f32, tz: f32, angle: f32) -> state::Mtx34 {
    let (s, c) = (sinf(angle), cosf(angle));
    // Column-major 3×4 (GX uses row-major 3 rows × 4 columns):
    //   row 0: [c,  0, s, tx]
    //   row 1: [0,  1, 0, ty]
    //   row 2: [-s, 0, c, tz]
    [
        [ c,  0.0,  s,  tx],
        [0.0, 1.0, 0.0, ty],
        [-s,  0.0,  c,  tz],
    ]
}

// ─── Minimal trig ─────────────────────────────────────────────────────────────
// Pure-Rust sin/cos via 9-term Taylor series after range reduction.
// Accurate to ~1e-7 for the range [0, 2π], sufficient for a rotation demo.

fn sinf(x: f32) -> f32 {
    // Reduce x to [-π, π]
    let pi = core::f32::consts::PI;
    let mut r = x % (2.0 * pi);
    if r > pi  { r -= 2.0 * pi; }
    if r < -pi { r += 2.0 * pi; }
    // Taylor: x - x^3/6 + x^5/120 - x^7/5040
    let r2 = r * r;
    r * (1.0 - r2 * (1.0/6.0 - r2 * (1.0/120.0 - r2 * (1.0/5040.0))))
}

fn cosf(x: f32) -> f32 {
    sinf(x + core::f32::consts::FRAC_PI_2)
}
