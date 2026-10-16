//! pigment-engine: 引擎层（笔刷引擎 / 光谱混色 / tile 表面）。
//!
//! 以 hokusai（libmypaint 的纯 Rust 移植，MIT OR Apache-2.0）为基座，
//! 本 crate 只承载自家增量：无限画布 TileMap、笔刷库（.myb）、
//! 笔画事件流、tooth 纹理沉积等。

pub mod brushlib;

pub use hokusai;
pub use pigment_surface as surface;

pub use brushlib::{BrushEntry, BrushLibrary, Origin};

pub const ENGINE_INFO: &str = "pigment-engine 0.1.0 (hokusai 0.3 + pigment-surface)";
