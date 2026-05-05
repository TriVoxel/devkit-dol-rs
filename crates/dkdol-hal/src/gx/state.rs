//! GX pipeline state: vertex format, matrices, viewport, TEV, blend, Z.
//!
//! All state is written immediately to the FIFO (no lazy accumulation).
//! This keeps the implementation simple at the cost of a tiny bit of
//! bandwidth for redundant state writes — acceptable for our use case.

use super::wgpipe as wp;
use super::types::*;

// ─────────────────────────────────────────────────────────────────────────────
// Vertex descriptor (VCD) — which attributes are present and how
// ─────────────────────────────────────────────────────────────────────────────

/// Clear all vertex attributes (set everything to NONE / DIRECT = 0).
pub unsafe fn clear_vtx_desc() {
    // VCD Lo: CP reg 0x50, VCD Hi: CP reg 0x60
    wp::load_cp_reg(0x50, 0);
    wp::load_cp_reg(0x60, 0);
}

/// Set position attribute type (VCD Lo bits 10:9).
pub unsafe fn set_vtx_desc_pos(t: AttrType) {
    // Read-modify-write isn't possible without shadow state.
    // We write the full VCD after calling set_vtx_attr_fmt_direct() which
    // calls set_vtx_desc_pos_color() together. Use the combo below.
    let _ = t; // used in set_vtx_desc_pos_clr0
}

/// Set up VCD Lo for: position DIRECT + color0 DIRECT (common 2D/3D case).
pub unsafe fn set_vtx_desc_pos_clr0() {
    // CP VCD Lo: pos bits 10:9 = 01 (DIRECT) | clr0 bits 14:13 = 01 (DIRECT)
    wp::load_cp_reg(0x50, 0x2200);
    wp::load_cp_reg(0x60, 0);
    // XF VTXSPECS (0x1008): INVTXSPEC — tells XF how many of each attribute.
    // bits 1:0 = numcolors (1), bits 3:2 = numnormals (0), bits 7:4 = numtextures (0)
    // pos+clr0, no normals, no tex → 0x01
    wp::load_xf_reg(0x1008, 0x01);
}

/// Set up VCD Lo for: position DIRECT only (no color attribute).
pub unsafe fn set_vtx_desc_pos_only() {
    wp::load_cp_reg(0x50, 0x0200);
    wp::load_cp_reg(0x60, 0);
    // XF VTXSPECS: numcolors=0, numnormals=0, numtextures=0 → 0x00
    wp::load_xf_reg(0x1008, 0x00);
}

// ─────────────────────────────────────────────────────────────────────────────
// Vertex Attribute Table (VAT) — component types and sizes
// ─────────────────────────────────────────────────────────────────────────────
//
// Each VTXFMT has three CP register groups:
//   VAT group 0: CP 0x70+fmt  (pos, nrm, clr0, clr1)
//   VAT group 1: CP 0x80+fmt  (tex0..tex3 components)
//   VAT group 2: CP 0x90+fmt  (tex4..tex7 components)
//
// VAT group 0 bit layout (from YAGCD):
//   bits  0:0  PosElements  (0=XY, 1=XYZ)
//   bits  4:1  PosFormat    (0=U8, 1=S8, 2=U16, 3=S16, 4=F32)
//   bits  8:5  PosFrac      (fixed-point scale exponent, 0..31)
//   bits  9:9  NrmElements
//   bits 13:10 NrmFormat
//   bits 14:14 NrmMidx3
//   bit  13:14 Clr0Elements (0=RGB, 1=RGBA)
//   bits 16:14 Clr0Format   (0=RGB565, 1=RGB8, 2=RGBX8, 3=RGBA4, 4=RGBA6, 5=RGBA8)
//   bits 22:20 Clr1Elements+Format (same pattern)
//   bits 25:23 Clr1Format
//   bits 29:26 (tex0 overflow)
//   bit  30:   NrmMidx3
//   bit  31:   ByteDequant (apply position fraction)

/// Configure VTXFMT0 for position=XYZ/F32, color0=RGBA8.
pub unsafe fn set_vtx_fmt_pos_xyz_f32_clr_rgba8(fmt: VtxFmt) {
    let f = fmt as usize;
    // Group 0:
    //   PosElements = 1 (XYZ)      → bit 0  = 1
    //   PosFormat   = 4 (F32)      → bits 4:1 = 4 → 4<<1 = 0x08
    //   PosFrac     = 0            → bits 8:5 = 0
    //   Clr0Elems   = 1 (RGBA)     → bit 13   → 1<<13 = 0x2000
    //   Clr0Format  = 5 (RGBA8)    → bits 16:14 → 5<<14 = 0x14000
    //   NrmMidx3    = 1 (bit 30, libogc2 init default)
    let vat0: u32 = 0x4000_0000  // NrmMidx3 bit (libogc2 default)
                  | (5u32 << 14) // Clr0Format = RGBA8  (bits 16:14)
                  | (1u32 << 13) // Clr0Elements = RGBA (bit 13)
                  | (4u32 <<  1) // PosFormat = F32
                  | (1u32 <<  0) // PosElements = XYZ
                  ;
    // Group 1: bit 31 set (libogc2 default)
    let vat1: u32 = 0x8000_0000;
    // Group 2: 0
    wp::load_cp_reg(0x70 + f as u8, vat0);
    wp::load_cp_reg(0x80 + f as u8, vat1);
    wp::load_cp_reg(0x90 + f as u8, 0);
}

/// Configure VTXFMT0 for position=XYZ/F32 only (no color attribute).
pub unsafe fn set_vtx_fmt_pos_xyz_f32(fmt: VtxFmt) {
    let f = fmt as usize;
    let vat0: u32 = 0x4000_0000
                  | (4u32 <<  1) // PosFormat = F32
                  | (1u32 <<  0) // PosElements = XYZ
                  ;
    wp::load_cp_reg(0x70 + f as u8, vat0);
    wp::load_cp_reg(0x80 + f as u8, 0x8000_0000);
    wp::load_cp_reg(0x90 + f as u8, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrices
// ─────────────────────────────────────────────────────────────────────────────

/// A 3×4 position/normal matrix (row-major: 3 rows of 4 floats).
pub type Mtx34 = [[f32; 4]; 3];

/// Identity 3×4 matrix.
pub const IDENTITY: Mtx34 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// Load a 3×4 position matrix into XF slot `pnidx` (0..9 → GX_PNMTX0..9).
///
/// `pnidx` * 3 + 0x0000 = XF address for that slot.
pub unsafe fn load_pos_mtx_imm(mt: &Mtx34, pnidx: u32) {
    // 12 floats = 12 XF registers starting at (pnidx << 2)
    wp::load_xf_regs((pnidx << 2) as u16, 12);
    // Write 3 rows × 4 floats = 12 f32 values
    for row in mt {
        for &v in row {
            wp::writef32(v);
        }
    }
}

/// Set which position-normal matrix slot to use (XF reg 0x1018).
pub unsafe fn set_current_mtx(pnidx: u32) {
    wp::load_xf_reg(0x1018, pnidx & 0x3F);
}

/// Load a projection matrix.
///
/// GX_LOAD_XF_REGS(0x1020, 7): 6 floats + 1 u32 type flag.
pub unsafe fn load_projection_mtx(proj: &Proj) {
    wp::load_xf_regs(0x1020, 7);
    wp::writef32(proj.p[0]);
    wp::writef32(proj.p[1]);
    wp::writef32(proj.p[2]);
    wp::writef32(proj.p[3]);
    wp::writef32(proj.p[4]);
    wp::writef32(proj.p[5]);
    wp::write32(proj.kind as u32);
}

/// Projection matrix: 6 parameters + type.
pub struct Proj {
    pub p: [f32; 6],
    pub kind: ProjType,
}

impl Proj {
    /// Perspective projection.
    ///
    /// `fov_y`: field of view in radians, `aspect`: width/height,
    /// `near`, `far`: clip planes.
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Self {
        // Standard GX perspective: same as guPerspective in libogc2.
        let f = 1.0 / libm_tanf(fov_y * 0.5);
        Self {
            p: [
                f / aspect,                      // p0
                0.0,                             // p1 (skew = 0)
                f,                               // p2
                0.0,                             // p3 (skew = 0)
                (far) / (near - far),            // p4
                (far * near) / (near - far),     // p5
            ],
            kind: ProjType::Perspective,
        }
    }

    /// Orthographic projection mapping [left,right] × [bottom,top] × [near,far]
    /// to the GX clip cube.
    pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Self {
        let rl = 1.0 / (right - left);
        let tb = 1.0 / (top - bottom);
        let fn_ = 1.0 / (far - near);
        Self {
            p: [
                2.0 * rl,               // p0
                -(right + left) * rl,   // p1
                2.0 * tb,               // p2
                -(top + bottom) * tb,   // p3
                -fn_,                   // p4
                -far * fn_,             // p5
            ],
            kind: ProjType::Orthographic,
        }
    }
}

// Pure-Rust tanf — avoids extern "C" linkage requirement on bare metal.
fn libm_tanf(x: f32) -> f32 {
    // tan(x) = sin(x)/cos(x), valid for x ∈ (-π/2, π/2)
    // For perspective FOV/2 this is always in range.
    let r2 = x * x;
    let s = x * (1.0 - r2 * (1.0/6.0 - r2 * (1.0/120.0 - r2 / 5040.0)));
    let c = 1.0 - r2 * (0.5 - r2 * (1.0/24.0 - r2 / 720.0));
    s / c
}

// ─────────────────────────────────────────────────────────────────────────────
// Viewport
// ─────────────────────────────────────────────────────────────────────────────

/// Set the rendering viewport.
///
/// `x_orig`, `y_orig`: top-left in EFB pixels.
/// `wd`, `ht`: dimensions in pixels.
/// `near_z`, `far_z`: normalised depth range (typically 0.0 and 1.0).
pub unsafe fn set_viewport(x_orig: f32, y_orig: f32, wd: f32, ht: f32,
                            near_z: f32, far_z: f32) {
    const XFACTOR: f32 = 0.5;
    const YFACTOR: f32 = 342.0 + (0.5 / 12.0);
    const ZFACTOR: f32 = 16_777_215.0;

    let x0 = wd * XFACTOR;
    let y0 = (-ht) * XFACTOR;
    let x1 = (x_orig + (wd * XFACTOR)) + YFACTOR;
    let y1 = (y_orig + (ht * XFACTOR)) + YFACTOR;
    let n  = ZFACTOR * near_z;
    let f  = ZFACTOR * far_z;
    let z  = f - n;

    wp::load_xf_regs(0x101A, 6);
    wp::writef32(x0);
    wp::writef32(y0);
    wp::writef32(z);
    wp::writef32(x1);
    wp::writef32(y1);
    wp::writef32(f);
}

/// Set scissor box (in EFB pixel coordinates).
pub unsafe fn set_scissor(x: u32, y: u32, w: u32, h: u32) {
    // BP 0x20: scissor TL corner.  BP 0x21: scissor BR corner.
    // Format: bits 21:10 = Y, bits 9:0 = X.
    wp::load_bp_reg(0x20_00_00_00 | ((y << 10) | x));
    wp::load_bp_reg(0x21_00_00_00 | (((y + h - 1) << 10) | (x + w - 1)));
}

// ─────────────────────────────────────────────────────────────────────────────
// Rasteriser state
// ─────────────────────────────────────────────────────────────────────────────

/// Set back-face culling mode.
///
/// GEN_MODE register (BP 0x00) bits 14:13 = cull mode.
/// We track the full genMode word manually since we write BP directly.
pub unsafe fn set_cull_mode(mode: CullMode) {
    // GEN_MODE (BP addr 0x00). We only touch bits 14:13. Other bits = 0 for now.
    // This must match the full GEN_MODE write done during init.
    // Since we set genMode to 0 in init, just write the cull bits.
    let gen_mode: u32 = (mode as u32) << 13;
    wp::load_bp_reg(0x0000_0000 | gen_mode); // BP addr 0x00 = GEN_MODE
}

/// Enable/disable clipping (XF reg 0x101A bit 0; confusingly same addr as viewport).
/// Enable or disable GX clipping.
///
/// GX clipping is always active; this is currently a no-op. Do not call.
#[allow(dead_code)]
pub unsafe fn set_clip_mode(_enable: bool) {
    // XF 0x1008 is VTXSPECS, not clip control. GX has no software
    // clip-disable register — clipping is always on.
}

// ─────────────────────────────────────────────────────────────────────────────
// Pixel engine state
// ─────────────────────────────────────────────────────────────────────────────

/// Set Z-buffer mode.
///
/// `enable`: enable Z test, `func`: comparison function, `update`: write Z.
/// BP register 0x40 = PE_ZMODE.
pub unsafe fn set_z_mode(enable: bool, func: Compare, update: bool) {
    let val: u32 = (if enable { 1 } else { 0 })
                 | ((func as u32) << 1)
                 | (if update { 1 << 4 } else { 0 });
    wp::load_bp_reg(0x4000_0000 | val);
}

/// Set blend mode.
///
/// BP register 0x41 = PE_CMODE0.
pub unsafe fn set_blend_mode(mode: BlendMode, src: BlendFactor, dst: BlendFactor) {
    let mut val: u32 = 0;
    if matches!(mode, BlendMode::Blend | BlendMode::Subtract) { val |= 0x1; }
    if matches!(mode, BlendMode::Subtract) { val |= 0x800; }
    if matches!(mode, BlendMode::Logic)    { val |= 0x2;   }
    val |= (dst as u32) << 5;
    val |= (src as u32) << 8;
    wp::load_bp_reg(0x4100_0000 | val);
}

/// Set pixel format (BP addr 0x43 = PE_CNTRL).
///
/// `fmt`: pixel format for the EFB. For most GC apps use `PixelFmt::Rgb8Z24`.
pub unsafe fn set_pixel_fmt(fmt: PixelFmt) {
    // PE_CNTRL bits 2:0 = pixel format. Z-compression disabled (bit 3 = 0).
    wp::load_bp_reg(0x4300_0000 | (fmt as u32));
}

// ─────────────────────────────────────────────────────────────────────────────
// Texture Environment (TEV) — stage 0 only, pass-through colour
// ─────────────────────────────────────────────────────────────────────────────

/// Set the number of active TEV stages (1..16).
///
/// Writes GEN_MODE bits 3:0 = num_tev_stages - 1.
pub unsafe fn set_num_tev_stages(n: u8) {
    // GEN_MODE (BP addr 0x00). bits 3:0 = n-1.
    wp::load_bp_reg(0x0000_0000 | ((n - 1) as u32 & 0xF));
}

/// Configure TEV stage 0 to pass through vertex colour (no texture).
///
/// Sets colour combiner: D = vertex colour (RASA/RASC).
/// Writes BP regs 0xC0 and 0xC1 (TEV colour and alpha env for stage 0).
pub unsafe fn set_tev_passthrough_vtx_color() {
    // TEV colour env for stage 0 (BP addr 0xC0):
    //   A=ZERO, B=ZERO, C=ZERO, D=RASC (vertex colour), op=ADD, clamp=1, out=PREV
    //   RASC = 10 = 0xA
    //   d = cc::RASC << 0, others = cc::ZERO = 15 << {12,8,4}
    let color_env: u32 = (cc::ZERO  as u32) << 12  // A
                       | (cc::ZERO  as u32) <<  8  // B
                       | (cc::ZERO  as u32) <<  4  // C
                       | (cc::RASC  as u32) <<  0  // D = vertex colour
                       | (1u32 << 19)               // clamp
                       | (0u32 << 22)               // out = GX_TEVPREV
                       ;
    wp::load_bp_reg(0xC000_0000 | color_env);

    // TEV alpha env for stage 0 (BP addr 0xC1):
    //   A=ZERO, B=ZERO, C=ZERO, D=RASA, op=ADD, clamp=1, out=PREV
    let alpha_env: u32 = (ca::ZERO  as u32) << 13  // A
                       | (ca::ZERO  as u32) << 10  // B
                       | (ca::ZERO  as u32) <<  7  // C
                       | (ca::RASA  as u32) <<  4  // D = vertex alpha
                       | (1u32 << 19)               // clamp
                       | (0u32 << 22)               // out = GX_TEVPREV
                       ;
    wp::load_bp_reg(0xC100_0000 | alpha_env);
}

/// Set TEV stage 0 "rasterized colour" source (BP addr 0xE0 = TEV_ORDER).
///
/// For vertex-colour only (no texture), raster = GX_COLOR0A0 = 0, texmap = NULL.
pub unsafe fn set_tev_order_vtx_only() {
    // TEV_ORDER[0] (BP 0xE0):
    //   bits 3:0   = TEXCOORD_NULL (0xF)
    //   bits 10:6  = TEXMAP_NULL   (0x7)
    //   bits 13:11 = color channel: COLOR0A0 = 0
    let order: u32 = 0xF             // texcoord null
                   | (0x7 << 6)     // texmap null
                   | (0x0 << 11)    // color0a0
                   ;
    wp::load_bp_reg(0xE000_0000 | order);
}

/// Set the number of colour channels output by rasteriser (XF reg 0x100E = SETNUMCHAN).
pub unsafe fn set_num_color_chans(n: u8) {
    wp::load_xf_reg(0x100E, n as u32);
}


/// Configure colour channel 0 for vertex-colour passthrough (no lighting).
///
/// Sets the XF channel control so the rasteriser outputs the raw vertex colour:
/// - Material source = vertex (matSrc=1, bit 1)
/// - Lighting disabled (enable=0, bit 0)
/// - Ambient source = register (ambSrc=0, bit 2)
///
/// Writes XF 0x100F (CHAN0_COLOR) and XF 0x1011 (CHAN0_ALPHA).
/// Call after [`set_num_color_chans`].
pub unsafe fn set_chan_ctrl_vtx_color() {
    // XF CHAN_COLOR control register format:
    //   bit 0:    lighting enable (0=off)
    //   bit 1:    material source (0=register, 1=vertex)
    //   bit 2:    ambient source  (0=register, 1=vertex)
    //   bits 6:3: light mask
    //   bit 7:    diffuse function
    //   bits 9:8: attenuation function
    //
    // For vertex-colour passthrough: enable=0, matSrc=vertex(1) → 0b010 = 0x02
    const CTRL: u32 = 1 << 1; // matSrc = vertex

    wp::load_xf_reg(0x100F, CTRL); // CHAN0_COLOR
    wp::load_xf_reg(0x1011, CTRL); // CHAN0_ALPHA
}

/// Set the number of texture coordinate generators (XF reg 0x103F).
pub unsafe fn set_num_tex_gens(n: u32) {
    wp::load_xf_reg(0x103F, n);
}

// ─────────────────────────────────────────────────────────────────────────────
// EFB → XFB copy
// ─────────────────────────────────────────────────────────────────────────────

/// Set the clear colour (used when `copy_disp` is called with `clear = true`).
///
/// BP regs 0x4F (AR), 0x50 (GB), 0x51 (Z).
pub unsafe fn set_copy_clear(r: u8, g: u8, b: u8, a: u8, z: u32) {
    wp::load_bp_reg(0x4F00_0000 | ((a as u32) << 8) | (r as u32));
    wp::load_bp_reg(0x5000_0000 | ((g as u32) << 8) | (b as u32));
    wp::load_bp_reg(0x5100_0000 | (z & 0x00FF_FFFF));
}

/// Set EFB copy source region (the portion of the EFB to copy).
pub unsafe fn set_disp_copy_src(x: u16, y: u16, w: u16, h: u16) {
    // BP 0x49: TL corner. bits 21:10 = Y, bits 9:0 = X.
    wp::load_bp_reg(0x4900_0000 | ((y as u32) << 10) | (x as u32));
    // BP 0x4A: size. bits 21:10 = H-1, bits 9:0 = W-1.
    wp::load_bp_reg(0x4A00_0000 | (((h - 1) as u32) << 10) | ((w - 1) as u32));
}

/// Set EFB copy destination stride (XFB stride in bytes / 32).
///
/// For a 640-pixel-wide 16bpp XFB: stride = 640 * 2 / 32 = 40... wait,
/// actually the stride field is in "half-words" = bytes/2... Let's use
/// libogc2's formula: `wd * 2 / 32 = wd / 16`.
/// Actually libogc2 just stores `wd` directly in `dispCopyDst`.
/// The register packs stride and height differently. Use `wd` only.
pub unsafe fn set_disp_copy_dst(w: u16, _h: u16) {
    // BP 0x4B: dest stride. bits 9:0 = width in 16-pixel units = w >> 4.
    // Actually libogc2 stores (w/16) as the stride here.
    // The actual BP addr for this is 0x4B (dispCopyDst).
    wp::load_bp_reg(0x4B00_0000 | ((w as u32 >> 4) & 0x3FF));
}

/// Set Y-scale for EFB→XFB copy (for interlaced modes).
///
/// For 480i with efbHeight=240 and xfbHeight=480: yscale = 480.0/240.0 = 2.0.
/// For progressive / 1:1: yscale = 1.0.
pub unsafe fn set_disp_copy_y_scale(yscale: f32) {
    // BP 0x4E: Y scale factor, encoded as fixed-point.
    // libogc2: val = (u32)(256.0f * yscale)
    let val = (256.0 * yscale) as u32;
    wp::load_bp_reg(0x4E00_0000 | (val & 0x1FF));
}

/// Copy the EFB to an XFB buffer in MEM1 and optionally clear the EFB.
///
/// `dest` must be the **physical** address of the XFB divided by 32.
/// Pass `clear = true` to apply the copy-clear colour after copying.
pub unsafe fn copy_disp(dest_phys: u32, clear: bool) {
    // Temporarily set Z mode to always-pass and disable colour update
    // during the copy if clearing (matches libogc2 behaviour).
    if clear {
        // Full Z clear: peZMode = 0x4F = enable + ALWAYS + update
        wp::load_bp_reg(0x400F_000F);
        // Disable colour blending for copy
        wp::load_bp_reg(0x4100_0000);
    }

    // BP 0x4B: dest address (physical >> 5), top 8 bits = BP addr = 0x4B
    wp::load_bp_reg(0x4B00_0000 | ((dest_phys >> 5) & 0x00FF_FFFF));

    // BP 0x52: copy execute + clear flag
    //   bit 11: clear = 1
    //   bit 14: execute = 1 (always set to trigger copy)
    //   bits 1:0: clamp (top+bottom = 3)
    let execute_flags: u32 = (1u32 << 14)  // execute copy
                           | (1u32 << 0)   // clamp top
                           | (1u32 << 1)   // clamp bottom
                           | (if clear { 1u32 << 11 } else { 0 });
    wp::load_bp_reg(0x5200_0000 | execute_flags);

    if clear {
        // Restore Z mode (enable + LEQUAL + update)
        wp::load_bp_reg(0x4000_0007); // enable + LEQUAL (3) + update
        // Restore blend
        wp::load_bp_reg(0x4100_0000);
    }
}
