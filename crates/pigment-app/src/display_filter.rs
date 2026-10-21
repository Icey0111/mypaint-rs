//! 显示过滤器 —— 已下沉到 GUI 无关的 [`pigment_render::display_filter`]。
//!
//! 过滤发生在 CPU 合成 tile 之后、GPU 上传之前，属于画布渲染层。

pub use pigment_render::display_filter::*;
