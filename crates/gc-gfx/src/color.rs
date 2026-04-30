//! Common YCbCr color constants for the GameCube XFB.
//!
//! Colors are stored as pixel-pairs (two pixels sharing chroma).
//! The conversion from RGB to YCbCr uses BT.601:
//!
//!   Y  =  16 + 65.481·R + 128.553·G +  24.966·B
//!   Cb = 128 - 37.797·R -  74.203·G + 112.0  ·B
//!   Cr = 128 + 112.0  ·R -  93.786·G -  18.214·B

use crate::YcbcrPair;

/// Black     (R=0,   G=0,   B=0)
pub const BLACK:   YcbcrPair = YcbcrPair::new( 16, 128,  16, 128);
/// White     (R=255, G=255, B=255)
pub const WHITE:   YcbcrPair = YcbcrPair::new(235, 128, 235, 128);
/// Red       (R=255, G=0,   B=0)
pub const RED:     YcbcrPair = YcbcrPair::new( 81,  90,  81,  240);
/// Green     (R=0,   G=255, B=0)
pub const GREEN:   YcbcrPair = YcbcrPair::new(145,  54, 145,  34);
/// Blue      (R=0,   G=0,   B=255)
pub const BLUE:    YcbcrPair = YcbcrPair::new( 41, 240,  41, 110);
/// Yellow    (R=255, G=255, B=0)
pub const YELLOW:  YcbcrPair = YcbcrPair::new(210,  16, 210, 146);
/// Cyan      (R=0,   G=255, B=255)
pub const CYAN:    YcbcrPair = YcbcrPair::new(170, 166, 170,  16);
/// Magenta   (R=255, G=0,   B=255)
pub const MAGENTA: YcbcrPair = YcbcrPair::new(106, 202, 106, 222);
/// Dark grey
pub const DARK_GREY: YcbcrPair = YcbcrPair::new(70, 128, 70, 128);
/// Light grey
pub const LIGHT_GREY: YcbcrPair = YcbcrPair::new(180, 128, 180, 128);
