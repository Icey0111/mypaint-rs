//! egui 适配层 —— 画布渲染本身在 GUI 无关的 [`pigment_render`]。
//!
//! Plan A 外壳（`shell31`）不走这里的回调：它在 egui 之前直接开自己的
//! wgpu render pass。本模块只为 `--eframe` 回退保留一个 `PaintCallback`
//! 适配器，让渲染器仍然可以挂进 eframe/egui-wgpu 的帧里。

use eframe::egui;
use eframe::wgpu;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

pub use pigment_render::{CanvasFrame, TileKey, TileRenderer};

/// eframe 回退路径的适配器：prepare 上传/建批次，paint 只发已准备好的 draw。
pub struct CanvasPaint {
    pub frame: CanvasFrame,
}

impl CallbackTrait for CanvasPaint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let mut r = self.frame.renderer.lock().unwrap();
        r.prepare(
            device,
            queue,
            &self.frame.uploads,
            &self.frame.evict,
            &self.frame.visible,
            &self.frame.view_matrix,
            self.frame.nearest_filter,
            &self.frame.draw,
        );
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &CallbackResources,
    ) {
        let r = self.frame.renderer.lock().unwrap();
        r.draw_prepared(render_pass, self.frame.checks);
    }
}
