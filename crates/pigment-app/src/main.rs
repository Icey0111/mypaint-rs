//! pigment —— MyPaint 精神续作（Rust + egui + wgpu 重构）。
//!
//! M2 里程碑：无限画布 + GPU tile 渲染 + 平移缩放 + 基础绘制。

mod app;
mod convert;
mod gpu;
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
