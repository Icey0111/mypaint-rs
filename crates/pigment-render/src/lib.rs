//! 画布渲染层 —— 只依赖 wgpu 与 CPU 侧图层数据，不依赖 egui/eframe/winit。
//!
//! 这里是「画布/笔刷引擎/图层独立于 GUI」中的画布部分：
//! - [`view::View`]：世界坐标 ↔ 屏幕坐标、缩放/旋转/镜像，纯数学；
//! - [`display_filter::DisplayFilter`]：显示用像素过滤；
//! - [`canvas::prepare_canvas`]：可见 tile 枚举、CPU 合成结果上传计划；
//! - [`gpu::TileRenderer`]：tile 显存缓存 + wgpu 管线。多块 tile 放进 atlas
//!   页，按页合并成一次 instanced draw（低缩放下可见格上千，逐格 draw call
//!   是上一版的主要 CPU/驱动开销）。
//!
//! GUI 只负责把窗口矩形、指针事件和偏好传进来；渲染器自己可以脱离窗口
//! 在任意 wgpu target 上工作（也便于无头测试）。

pub mod canvas;
pub mod display_filter;
pub mod gpu;
pub mod gpu_dabs;
pub mod gpu_sample;
pub mod view;

pub use canvas::{mip_level_for_zoom, prepare_canvas, CanvasFrame, MAX_VISIBLE_TILES_PER_AXIS};
pub use display_filter::DisplayFilter;
pub use gpu_dabs::{GpuDabEngine, SurfaceGpuSink};
pub use gpu::{
    capacity_for, plan_evictions, plan_lru_evictions, plan_uploads, TileKey, TileRenderer,
    TileUpload,
};
pub use view::View;
