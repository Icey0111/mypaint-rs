//! pigment —— MyPaint 精神续作（Rust + egui + wgpu 重构）。
//!
//! M2：无限画布 + GPU tile 渲染 + 平移缩放 + 绘制。
//! M3：.myb 笔刷库（196 支官方包内嵌）+ 笔刷列表 + 笔刷编辑器。

mod app;
mod brushes;
mod color;
mod convert;
mod dump;
mod editor;
mod gpu;
mod thumb;
mod view;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title(format!("pigment — {}", pigment_engine::ENGINE_INFO)),
        ..Default::default()
    };
    eframe::run_native(
        "pigment",
        options,
        Box::new(|cc| Ok(Box::new(app::PigmentApp::new(cc)))),
    )
}
