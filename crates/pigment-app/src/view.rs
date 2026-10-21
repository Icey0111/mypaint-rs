//! 视图变换 —— 已下沉到 GUI 无关的 [`pigment_render::view`]。
//!
//! 画布渲染器需要同一套缩放/旋转/镜像数学，但渲染层不能依赖 egui。
//! 这里保留原路径，GUI 与命令层继续通过 `crate::view::*` 使用。

pub use pigment_render::view::*;
