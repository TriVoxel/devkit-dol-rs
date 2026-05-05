//! GX pipeline type definitions.

// ── Primitive types ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Primitive {
    Quads        = 0x80,
    Triangles    = 0x90,
    TriangleStrip= 0x98,
    TriangleFan  = 0xA0,
    Lines        = 0xA8,
    LineStrip    = 0xB0,
    Points       = 0xB8,
}

// ── Vertex format index ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VtxFmt {
    Fmt0 = 0,
    Fmt1 = 1,
    Fmt2 = 2,
    Fmt3 = 3,
    Fmt4 = 4,
    Fmt5 = 5,
    Fmt6 = 6,
    Fmt7 = 7,
}

// ── Vertex attribute type ─────────────────────────────────────────────────────

/// Input method for a vertex attribute.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AttrType {
    None    = 0,
    Direct  = 1,
    Index8  = 2,
    Index16 = 3,
}

// ── Component type/size ───────────────────────────────────────────────────────

/// Number of components for position/normal.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PosComp {
    Xy  = 0,
    Xyz = 1,
}

/// Component scalar type.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompType {
    U8  = 0,
    S8  = 1,
    U16 = 2,
    S16 = 3,
    F32 = 4,
}

/// Color component format.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ColorFmt {
    Rgb565  = 0,
    Rgb8    = 1,
    Rgbx8   = 2,
    Rgba4   = 3,
    Rgba6   = 4,
    Rgba8   = 5,
}

// ── Cull mode ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CullMode {
    None  = 0,
    Front = 1,
    Back  = 2,
    All   = 3,
}

// ── Z compare function ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Compare {
    Never   = 0,
    Less    = 1,
    Equal   = 2,
    LEqual  = 3,
    Greater = 4,
    NEqual  = 5,
    GEqual  = 6,
    Always  = 7,
}

// ── Blend mode ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlendMode {
    None     = 0,
    Blend    = 1,
    Logic    = 2,
    Subtract = 3,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlendFactor {
    Zero        = 0,
    One         = 1,
    SrcColor    = 2,
    InvSrcColor = 3,
    SrcAlpha    = 4,
    InvSrcAlpha = 5,
    DstAlpha    = 6,
    InvDstAlpha = 7,
}

// ── Pixel format ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PixelFmt {
    Rgb8Z24  = 0,
    Rgba6Z24 = 1,
    Rgb565Z16= 2,
    Z24      = 3,
    Y8       = 4,
}

// ── Projection ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ProjType {
    Perspective  = 0,
    Orthographic = 1,
}

// ── TEV stage ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TevStage {
    Stage0  =  0,
    Stage1  =  1,
    Stage2  =  2,
    Stage3  =  3,
    Stage4  =  4,
    Stage5  =  5,
    Stage6  =  6,
    Stage7  =  7,
    Stage8  =  8,
    Stage9  =  9,
    Stage10 = 10,
    Stage11 = 11,
    Stage12 = 12,
    Stage13 = 13,
    Stage14 = 14,
    Stage15 = 15,
}

/// Common TEV colour-input register values (GX_CC_*).
#[allow(non_camel_case_types, dead_code)]
pub mod cc {
    pub const CPREV: u8  = 0;   // TEV register prev (color)
    pub const APREV: u8  = 1;   // TEV register prev (alpha as colour)
    pub const C0:    u8  = 2;
    pub const A0:    u8  = 3;
    pub const C1:    u8  = 4;
    pub const A1:    u8  = 5;
    pub const C2:    u8  = 6;
    pub const A2:    u8  = 7;
    pub const TEXC:  u8  = 8;
    pub const TEXA:  u8  = 9;
    pub const RASC:  u8  = 10;
    pub const RASA:  u8  = 11;
    pub const ONE:   u8  = 12;
    pub const HALF:  u8  = 13;
    pub const KONST: u8  = 14;
    pub const ZERO:  u8  = 15;
}

/// TEV alpha-input register values (GX_CA_*).
#[allow(non_camel_case_types, dead_code)]
pub mod ca {
    pub const APREV: u8 = 0;
    pub const A0:    u8 = 1;
    pub const A1:    u8 = 2;
    pub const A2:    u8 = 3;
    pub const TEXA:  u8 = 4;
    pub const RASA:  u8 = 5;
    pub const KONST: u8 = 6;
    pub const ZERO:  u8 = 7;
}

// ── Copy/EFB ─────────────────────────────────────────────────────────────────

pub const GX_MAX_Z24: u32 = 0x00FF_FFFF;
