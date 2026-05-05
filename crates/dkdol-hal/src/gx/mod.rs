//! Graphics Processor (GX) — FIFO command buffer interface.
//!
//! # Architecture
//!
//! The GX is a fixed-function GPU fed by a write-gather FIFO at
//! `0xCC008000`. The CPU accumulates writes into a 32-byte hardware buffer
//! (the write-gather pipe, WGP) that bursts automatically to a circular FIFO
//! buffer in MEM1. The GP reads and executes those commands independently.
//!
//! Pipeline stages:
//! ```text
//!  CPU writes → WGP → FIFO buffer → CP (Command Processor)
//!             → XF (Transform Engine: matrices, viewport)
//!             → TX (Texture fetch, TMEM)
//!             → TEV (16-stage fixed combiner)
//!             → PE (Pixel Engine: Z, blend, fog)
//!             → EFB (Embedded Framebuffer, 640×528 max)
//!             → EFB→XFB copy → VI → display
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use dkdol_hal::gx;
//! use dkdol_hal::gx::{types::*, state, draw};
//!
//! // 1. Allocate a 256 KB FIFO buffer (must be 32-byte aligned):
//! static mut FIFO_BUF: [u8; 256*1024] = [0; 256*1024];
//!
//! unsafe {
//!     // 2. Initialise GX
//!     gx::init(FIFO_BUF.as_mut_ptr(), FIFO_BUF.len());
//!
//!     // 3. Set up pipeline for flat-shaded 3D:
//!     state::set_vtx_desc_pos_clr0();
//!     state::set_vtx_fmt_pos_xyz_f32_clr_rgba8(VtxFmt::Fmt0);
//!     state::set_viewport(0.0, 0.0, 640.0, 480.0, 0.0, 1.0);
//!     state::set_z_mode(true, Compare::LEqual, true);
//!     state::set_blend_mode(BlendMode::None, BlendFactor::SrcAlpha, BlendFactor::InvSrcAlpha);
//!     state::set_tev_passthrough_vtx_color();
//!     state::set_tev_order_vtx_only();
//!     state::set_num_tev_stages(1);
//!     state::set_num_color_chans(1);
//!     state::set_num_tex_gens(0);
//!
//!     let proj = state::Proj::perspective(
//!         60_f32.to_radians(), 640.0/480.0, 0.1, 1000.0);
//!     state::load_projection_mtx(&proj);
//!     state::load_pos_mtx_imm(&state::IDENTITY, 0);
//!     state::set_current_mtx(0);
//!
//!     // 4. Per-frame: clear, draw, copy to XFB
//!     state::set_copy_clear(0, 0, 0, 255, gx::types::GX_MAX_Z24);
//!     // ... draw calls ...
//!     state::copy_disp(xfb_physical_addr >> 5, true);
//! }
//! ```

#![allow(dead_code)]

pub mod draw;
pub mod fifo;
pub mod state;
pub mod types;
pub mod wgpipe;

pub use types::*;

/// Minimum FIFO buffer size.
pub const FIFO_MIN_SIZE: usize = fifo::FIFO_MIN_SIZE;

/// Initialise the GX subsystem.
///
/// `fifo_buf` must be a 32-byte-aligned MEM1 buffer of at least 64 KB.
/// After this call you can start writing GX state and draw commands.
///
/// # Safety
/// - Must be called once, before any other `gx::*` functions.
/// - `fifo_buf` must remain valid for the lifetime of GX usage.
/// - The buffer must not overlap any other data.
pub unsafe fn init(fifo_buf: *mut u8, size: usize) {
    // 1. Set up the write-gather pipe (WPAR + HID2[WPE])
    wgpipe::init();

    // 2. Wait for any existing WGP traffic to drain
    wgpipe::flush();

    // 3. Configure FIFO circular buffer in CP and PI
    fifo::init(fifo_buf, size);

    // 4. Invalidate vertex cache
    wgpipe::inv_vtx_cache();

    // 5. Flush texture state (BP 0x0F = indirect texture mask = 0xFF)
    wgpipe::load_bp_reg(0x0F00_00FF);

    // 6. Init VAT rev bits (from libogc2 __GX_InitRevBits):
    //    VAT1 for each format gets bit 31 set; CP 0x20 gets 0.
    for i in 0u8..8 {
        wgpipe::load_cp_reg(0x80 + i, 0x8000_0000);
    }
    // XF 0x1000 = vtx specs (0x3F = 63 default)
    wgpipe::load_xf_reg(0x1000, 0x3F);
    // XF 0x1012 = output vtx specs  
    wgpipe::load_xf_reg(0x1012, 0x01);
    // BP 0x58 = GEN_MODE flush (from __GX_InitRevBits)
    wgpipe::load_bp_reg(0x5800_000F);

    // 7. Default PE state
    // Pixel engine control: RGB8_Z24 (PE_CNTRL = 0x43)
    wgpipe::load_bp_reg(0x4300_0000); // RGB8_Z24, no alpha compare override

    // Z mode: enable, LEQUAL, update (PE_ZMODE = 0x40)
    state::set_z_mode(true, types::Compare::LEqual, true);

    // Blend: none (PE_CMODE0 = 0x41)
    state::set_blend_mode(types::BlendMode::None,
                          types::BlendFactor::SrcAlpha,
                          types::BlendFactor::InvSrcAlpha);

    // Dither on (PE_CMODE0 bit 10, set separately)
    // Alpha update on, colour update on (default in PE_CMODE0)
    // These are already 0 (off) in the blend write — that's fine for now.

    // 8. Set sensible defaults for scissor (full 640×528 EFB)
    state::set_scissor(0, 0, 640, 528);

    // 9. Copy-clear defaults (black, max Z)
    state::set_copy_clear(0, 0, 0, 0xFF, types::GX_MAX_Z24);

    // 10. TEV defaults: 1 stage, passthrough vertex colour, no texture
    state::set_num_tev_stages(1);
    state::set_num_color_chans(0);
    state::set_num_tex_gens(0);
    state::set_tev_order_vtx_only();
    state::set_tev_passthrough_vtx_color();
}

/// Wait for all pending GX commands to complete (FIFO drain).
pub unsafe fn flush() {
    // Send a NOP draw call to flush any cached state
    wgpipe::flush();
    fifo::drain();
}
