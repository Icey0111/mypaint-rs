#![recursion_limit = "256"]

//! pigment —— MyPaint 精神续作（Rust + egui + wgpu 重构）。
//!
//! M2：无限画布 + GPU tile 渲染 + 平移缩放 + 绘制。
//! M3：.myb 笔刷库（196 支官方包内嵌）+ 笔刷列表 + 笔刷编辑器。
//! S1：窗口骨架（顶栏 / 工作区 / 底栏三段，照 `gui/mypaint.glade`）。

mod app;
mod blend_modes;
mod brushes;
mod color;
mod convert;
mod device;
mod display_filter;
mod dump;
mod editor;
mod gpu;
mod i18n;
mod native_dialog;
mod scratch;
mod shell;
mod shell31;
mod subwindow;
mod thumb;
mod view;

fn install_panic_hook() {
    // `gui/gtkexcepthook.py`：未捕获异常写崩溃日志并弹窗，而不是静默退出。
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{info}");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = pigment_engine::prefs::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("crash-{stamp}.txt"));
        let _ = std::fs::write(&path, &msg);
        eprintln!("MyPaint crashed: {msg}\nReport: {}", path.display());
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
            let text: Vec<u16> = format!(
                "MyPaint crashed:\n{msg}\n\nA crash report was written to:\n{}",
                path.display()
            )
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
            let title: Vec<u16> = "MyPaint".encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    }));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_panic_hook();
    // 参考机 MyPaint 2.0.1（GTK3/Windows）是 **system DPI aware**，且 GTK3
    // 不跟随系统缩放：在 150% 桌面上它仍按 96dpi 的 1:1 像素布局（窗口
    // 2460×1308 逻辑像素 = 2460×1308 物理像素，笔刷格 52px、图标 24px）。
    // Rust 默认进程是 DPI-unaware，Windows 会把整个窗口按 1.5× 拉伸，
    // 既导致窗口/输入坐标错位，也让界面尺寸与参考机不一致。
    // 这里显式声明 system DPI aware，与 MyPaint 完全一致；egui 侧再强制
    // pixels_per_point = 1.0（见 shell31），从而逐像素复刻 96dpi 布局。
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware;
        SetProcessDPIAware();
    }
    // `gui/main.py:229-268` 的命令行契约（子集）：
    //   [FILE] [-f/--fullscreen] [-V/--version] [-c/--config DIR]
    // 其它平台相关项（-l/-t/-R）由运行环境取代，不做。
    let args: Vec<String> = std::env::args().collect();
    let mut eframe_fallback = false;
    let mut fullscreen = false;
    let mut file: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--eframe" => eframe_fallback = true,
            // Plan A 的 wgpu 后端选择。默认 auto：独显优先、同类型 Vulkan 优先。
            "--vulkan" => std::env::set_var("PIGMENT_WGPU_BACKEND", "vulkan"),
            "--dx12" => std::env::set_var("PIGMENT_WGPU_BACKEND", "dx12"),
            "-f" | "--fullscreen" => fullscreen = true,
            "-V" | "--version" => {
                println!("MyPaint 2.0.1 (pigment {})", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-c" | "--config" => {
                if let Some(dir) = args.get(i + 1) {
                    std::env::set_var("PIGMENT_CONFIG_DIR", dir);
                    i += 1;
                }
            }
            "--" => {
                if let Some(f) = args.get(i + 1) {
                    file = Some(f.clone());
                }
                break;
            }
            _ if !a.starts_with('-') => file = Some(a.clone()),
            _ => {}
        }
        i += 1;
    }
    std::env::set_var(
        "PIGMENT_STARTUP_FULLSCREEN",
        if fullscreen { "1" } else { "0" },
    );
    if let Some(f) = file {
        std::env::set_var("PIGMENT_STARTUP_FILE", f);
    }

    // 默认走 Plan A 外壳（winit 0.31 + egui-wgpu），它支持数位板压感/倾斜/
    // 橡皮端与 IME；`--eframe` 保留旧的 eframe 路径做回退/对照。
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
