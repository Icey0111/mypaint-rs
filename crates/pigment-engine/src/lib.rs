//! pigment-engine: 引擎层（笔刷引擎 / 光谱混色 / tile 表面 / 文档模型）。
//!
//! 以 hokusai（libmypaint 的纯 Rust 移植，MIT OR Apache-2.0）为基座，
//! 本 crate 只承载自家增量：笔刷库（.myb）、笔画事件流（lib/stroke.py）、
//! 撤销/重做命令模型与文档（lib/command.py、lib/document.py）、
//! 文档 I/O（.ora / .png，lib/document.py 的 save/load）、tooth 纹理沉积等。

pub mod brushlib;
pub mod color;
pub mod command;
pub mod document;
pub mod fileio;
pub mod mode;
pub mod stroke;
pub mod zip;

pub use hokusai;
pub use pigment_surface as surface;

pub use brushlib::{BrushEntry, BrushLibrary, Origin};
pub use command::{
    Brushwork, Command, CommandOutcome, CommandStack, CommandUpdate, DocContext,
    DEFAULT_UNDO_STACK_SIZE,
};
pub use document::{Document, LoadOutcome, SaveOutcome};
pub use fileio::{
    compatibility, format_for_path, Compatibility, FileError, Format, OPENRASTER_MEDIA_TYPE,
    OPENRASTER_VERSION,
};
pub use stroke::{Stroke, StrokeEvent, EVENT_FIELDS, STROKE_DATA_VERSION};

pub const ENGINE_INFO: &str = "pigment-engine 0.1.0 (hokusai 0.3 + pigment-surface)";
