//! 显示用像素转换 —— 实现在 [`pigment_surface::color`]，本模块只做转发。
//!
//! 这段逻辑原先就在这里。文档 I/O（导出 PNG / ORA）也需要它，而引擎层不能
//! 依赖应用层，故按 MyPaint 的分层（`lib/pixops.cpp` / `lib/surface.py` 都在
//! `lib/`）下沉到了 `pigment-surface`。保留本模块是为了让应用层的既有调用点
//! （`gpu.rs` / `dump.rs` / 光标预览）不必改名。
//!
//! 关键结论仍在 `pigment_surface::color` 的模块文档里：**色彩转换用 EOTF 2.2，
//! 不是 IEC sRGB 曲线**；**画布底色是 MyPaint 的米白纸色**。
//!
//! 只转发本 crate 真正用到的那几项（binary crate 里多余的 `pub use` 会被
//! 判为 unused import）；测试要用别的常量时直接从 `pigment_surface::color` 取。

pub use pigment_surface::color::{
    eotf_encode, tile_to_rgba8, CANVAS_BG, TILE_RGBA8_BYTES,
};

/// [`CANVAS_BG`] 的 egui 形式。
///
/// 放在应用层是因为它依赖 egui（引擎层不该依赖 UI 库）。
pub fn canvas_bg_color() -> eframe::egui::Color32 {
    eframe::egui::Color32::from_rgb(CANVAS_BG[0], CANVAS_BG[1], CANVAS_BG[2])
}
