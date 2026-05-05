//! GX draw calls: begin, submit vertices, end.
//!
//! Usage pattern:
//!
//! ```rust,no_run
//! unsafe {
//!     gx::draw::begin(Primitive::Triangles, VtxFmt::Fmt0, 3);
//!     gx::draw::pos3f(0.0, 1.0, 0.0); gx::draw::color4u8(255,  0,  0, 255);
//!     gx::draw::pos3f(-1.0,-1.0, 0.0); gx::draw::color4u8(  0,255,  0, 255);
//!     gx::draw::pos3f( 1.0,-1.0, 0.0); gx::draw::color4u8(  0,  0,255, 255);
//!     // no explicit "end" — vertex count controls termination
//! }
//! ```

use super::wgpipe as wp;
use super::types::{Primitive, VtxFmt};

/// Begin a primitive draw call.
///
/// `prim`: primitive type (triangles, quads, etc.)
/// `fmt`: vertex format index (must match the format configured via `set_vtx_fmt_*`)
/// `vtx_count`: exact number of vertices that will follow
pub unsafe fn begin(prim: Primitive, fmt: VtxFmt, vtx_count: u16) {
    let opcode = (prim as u8) | (fmt as u8 & 7);
    wp::write8(opcode);
    wp::write16(vtx_count);
}

/// Submit a 3-component float position.
///
/// Must be called once per vertex when the vertex format has `PosElements = XYZ/F32`.
#[inline(always)]
pub unsafe fn pos3f(x: f32, y: f32, z: f32) {
    wp::writef32(x);
    wp::writef32(y);
    wp::writef32(z);
}

/// Submit a 2-component float position (XY only).
#[inline(always)]
pub unsafe fn pos2f(x: f32, y: f32) {
    wp::writef32(x);
    wp::writef32(y);
}

/// Submit an RGBA8 colour attribute (4 bytes packed).
///
/// Must be called once per vertex when the vertex format has `Clr0 = RGBA8`.
#[inline(always)]
pub unsafe fn color4u8(r: u8, g: u8, b: u8, a: u8) {
    let packed: u32 = ((r as u32) << 24)
                    | ((g as u32) << 16)
                    | ((b as u32) <<  8)
                    | (a as u32);
    wp::write32(packed);
}

/// Submit an RGB8 colour (no alpha; packed as RGBX8 with X = 0xFF).
#[inline(always)]
pub unsafe fn color3u8(r: u8, g: u8, b: u8) {
    color4u8(r, g, b, 0xFF);
}

/// Submit a single f32 texture coordinate (S only).
#[inline(always)]
pub unsafe fn tex1f(s: f32) {
    wp::writef32(s);
}

/// Submit 2 f32 texture coordinates (ST).
#[inline(always)]
pub unsafe fn tex2f(s: f32, t: f32) {
    wp::writef32(s);
    wp::writef32(t);
}
