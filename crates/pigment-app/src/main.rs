//! pigment —— MyPaint 精神续作（Rust + egui + wgpu 重构）。
//!
//! M2：无限画布 + GPU tile 渲染 + 平移缩放 + 绘制。
//! M3：.myb 笔刷库（196 支官方包内嵌）+ 笔刷列表 + 笔刷编辑器。
//! S1：窗口骨架（顶栏 / 工作区 / 底栏三段，照 `gui/mypaint.glade`）。

mod app;
mod brushes;
mod color;
mod convert;
mod dump;
mod editor;
mod gpu;
mod i18n;
mod scratch;
mod shell;
mod shell31;
mod thumb;
mod view;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 默认走 Plan A 外壳（winit 0.31 + egui-wgpu），它支持数位板压感/倾斜/
    // 橡皮端与 IME；`--eframe` 保留旧的 eframe 路径做回退/对照。
    let args: Vec<String> = std::env::args().collect();
    let eframe_fallback = args.iter().any(|a| a == "--eframe");
    if !eframe_fallback {
        shell31::run()?;
        return Ok(());
    }
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
    )?;
    Ok(())
}
