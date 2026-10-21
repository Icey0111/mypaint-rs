//! Plan A 外壳：winit 0.31 + egui-wgpu（不经 eframe）。
//!
//! 目标是把 winit 0.31 的 `PointerSource::TabletTool` 压力/倾斜/橡皮端
//! 接进 `PigmentApp`。当前是原型：指针/键盘/滚轮/窗口缩放可用，IME、
//! 剪贴板、无障碍、多视口还没接（记账）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use winit::application::ApplicationHandler;
use winit::cursor::Cursor as WinitCursor;
use winit::data_transfer::TypeHint;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ButtonSource, ElementState, MouseScrollDelta, PointerSource, WindowEvent};
use winit::event_loop::{ActiveEventLoop, AsyncRequestSerial, ControlFlow, DndAction, EventLoop};
use winit::icon::{Icon, RgbaIcon};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{
    ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, Theme, Window, WindowAttributes,
    WindowId,
};

use crate::app::PigmentApp;
use crate::subwindow::{self, Kind as SubWindowKind};

/// UI 缩放。
///
/// 参考机 MyPaint 在 96dpi 下是 1:1；但 ArrowVortex 取向的小字号在 150%
/// 桌面上不可读。这里取 1.25×：正文 16pt → 约 20 物理像素、图标 21pt
/// → 约 26 物理像素。侧栏宽度与菜单宽度单独收紧，不跟随字号一起膨胀，
/// 避免“字大了、面板也占半屏”。
///
/// 输入坐标在事件循环里除以同一个常量，所以命中测试与绘制坐标始终一致。
const UI_SCALE: f32 = 1.25;

/// 窗口底色（Adwaita Dark 的面板底），也用作首帧清屏色。
///
/// 与 egui 主题的 `panel_fill` 一致，这样"先铺一帧纯色再出 UI"不会闪白。
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.08,
    g: 0.08,
    b: 0.09,
    a: 1.0,
};

/// 一个原生 OS 子窗口（偏好/背景/关于/输入测试）。
///
/// 每个窗口有自己的 wgpu surface 与 egui Context —— 与主窗口共享 device/queue，
/// 但纹理、字体、布局记忆相互独立。这对应源码的
/// `windowing.Dialog` / `windowing.SubWindow`（`gui/windowing.py:29-91`）。
struct SubWindow {
    kind: SubWindowKind,
    window: Arc<dyn Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    raw: RawInput,
    modifiers: Modifiers,
    last_pointer: Option<Pos2>,
    last_cursor: Option<egui::CursorIcon>,
    /// IME 只在文本控件获得焦点时启用（与主窗口策略一致）。
    ime_enabled: bool,
    last_title: String,
}

struct State {
    clipboard: Option<arboard::Clipboard>,
    window: Arc<dyn Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 新子窗口复用同一个 instance/adapter（见 [`create_subwindow`]）。
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    format: wgpu::TextureFormat,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    app: PigmentApp,
    raw: RawInput,
    modifiers: Modifiers,
    start: Instant,
    /// 主键是否按下（用于窗口不产事件时继续产帧 + 补最后一次指针位置）。
    pointer_down: bool,
    last_pointer: Option<Pos2>,
    last_move_sent: Option<Pos2>,
    last_cursor: Option<egui::CursorIcon>,
    /// IME 只在文本框获得焦点时启用，否则单键快捷键会被输入法吃掉。
    ime_enabled: bool,
    last_title: String,
    subwindows: Vec<SubWindow>,
    /// 正在等待的 winit DnD 数据请求（`DragEntered` → `DataTransferReceived`）。
    dnd_fetch: Option<AsyncRequestSerial>,
}

pub(crate) struct Shell {
    state: Option<State>,
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.run_app(Shell { state: None })?;
    Ok(())
}

fn map_key(key: &WinitKey) -> Option<egui::Key> {
    use egui::Key as K;
    match key {
        WinitKey::Named(n) => Some(match n {
            NamedKey::Enter => K::Enter,
            NamedKey::Escape => K::Escape,
            NamedKey::Backspace => K::Backspace,
            NamedKey::Delete => K::Delete,
            NamedKey::Tab => K::Tab,
            NamedKey::ArrowLeft => K::ArrowLeft,
            NamedKey::ArrowRight => K::ArrowRight,
            NamedKey::ArrowUp => K::ArrowUp,
            NamedKey::ArrowDown => K::ArrowDown,
            NamedKey::Home => K::Home,
            NamedKey::End => K::End,
            NamedKey::PageUp => K::PageUp,
            NamedKey::PageDown => K::PageDown,
            NamedKey::Insert => K::Insert,
            NamedKey::F1 => K::F1,
            NamedKey::F2 => K::F2,
            NamedKey::F3 => K::F3,
            NamedKey::F4 => K::F4,
            NamedKey::F5 => K::F5,
            NamedKey::F6 => K::F6,
            NamedKey::F7 => K::F7,
            NamedKey::F8 => K::F8,
            NamedKey::F9 => K::F9,
            NamedKey::F10 => K::F10,
            NamedKey::F11 => K::F11,
            NamedKey::F12 => K::F12,
            _ => return None,
        }),
        WinitKey::Character(s) => {
            let c = s.chars().next()?.to_ascii_lowercase();
            Some(match c {
                'a' => K::A,
                'b' => K::B,
                'c' => K::C,
                'd' => K::D,
                'e' => K::E,
                'f' => K::F,
                'g' => K::G,
                'h' => K::H,
                'i' => K::I,
                'j' => K::J,
                'k' => K::K,
                'l' => K::L,
                'm' => K::M,
                'n' => K::N,
                'o' => K::O,
                'p' => K::P,
                'q' => K::Q,
                'r' => K::R,
                's' => K::S,
                't' => K::T,
                'u' => K::U,
                'v' => K::V,
                'w' => K::W,
                'x' => K::X,
                'y' => K::Y,
                'z' => K::Z,
                '0' => K::Num0,
                '1' => K::Num1,
                '2' => K::Num2,
                '3' => K::Num3,
                '4' => K::Num4,
                '5' => K::Num5,
                '6' => K::Num6,
                '7' => K::Num7,
                '8' => K::Num8,
                '9' => K::Num9,
                ' ' => K::Space,
                '+' => K::Plus,
                '-' => K::Minus,
                ':' => K::Colon,
                ',' => K::Comma,
                '\\' => K::Backslash,
                '/' => K::Slash,
                '|' => K::Pipe,
                '?' => K::Questionmark,
                '!' => K::Exclamationmark,
                '[' => K::OpenBracket,
                ']' => K::CloseBracket,
                '{' => K::OpenCurlyBracket,
                '}' => K::CloseCurlyBracket,
                '`' => K::Backtick,
                '.' => K::Period,
                '=' => K::Equals,
                ';' => K::Semicolon,
                '\'' => K::Quote,
                _ => return None,
            })
        }
        _ => None,
    }
}

fn mouse_button(button: &ButtonSource) -> Option<PointerButton> {
    match button {
        ButtonSource::Mouse(m) => match m {
            winit::event::MouseButton::Left => Some(PointerButton::Primary),
            winit::event::MouseButton::Right => Some(PointerButton::Secondary),
            winit::event::MouseButton::Middle => Some(PointerButton::Middle),
            _ => None,
        },
        ButtonSource::TabletTool { button, .. } => match button {
            winit::event::TabletToolButton::Contact => Some(PointerButton::Primary),
            winit::event::TabletToolButton::Barrel => Some(PointerButton::Secondary),
            _ => None,
        },
        _ => None,
    }
}

/// egui 的 `ColorImage` → 系统剪贴板的 RGBA8。
fn color_image_to_clipboard(image: &egui::ColorImage) -> Option<arboard::ImageData<'static>> {
    if image.size[0] == 0 || image.size[1] == 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for c in &image.pixels {
        // `Color32` 内部是预乘的；剪贴板要非预乘 RGBA（否则半透明颜色发暗）。
        let [r, g, b, a] = c.to_srgba_unmultiplied();
        bytes.extend_from_slice(&[r, g, b, a]);
    }
    Some(arboard::ImageData {
        width: image.size[0],
        height: image.size[1],
        bytes: std::borrow::Cow::Owned(bytes),
    })
}

/// 系统剪贴板的 RGBA8 → egui 的 `ColorImage`。
fn clipboard_to_color_image(width: usize, height: usize, bytes: &[u8]) -> Option<egui::ColorImage> {
    if width == 0 || height == 0 || bytes.len() != width * height * 4 {
        return None;
    }
    let pixels = bytes
        .chunks_exact(4)
        .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    Some(egui::ColorImage::new([width, height], pixels))
}

/// egui 的光标图标 → winit 光标图标（None 由调用方隐藏系统光标）。
fn map_cursor(icon: egui::CursorIcon) -> winit::cursor::CursorIcon {
    use egui::CursorIcon as E;
    use winit::cursor::CursorIcon as W;
    match icon {
        E::Default | E::None => W::Default,
        E::ContextMenu => W::ContextMenu,
        E::Help => W::Help,
        E::PointingHand => W::Pointer,
        E::Progress => W::Progress,
        E::Wait => W::Wait,
        E::Cell => W::Cell,
        E::Crosshair => W::Crosshair,
        E::Text => W::Text,
        E::VerticalText => W::VerticalText,
        E::Alias => W::Alias,
        E::Copy => W::Copy,
        E::Move => W::Move,
        E::NoDrop => W::NoDrop,
        E::NotAllowed => W::NotAllowed,
        E::Grab => W::Grab,
        E::Grabbing => W::Grabbing,
        E::AllScroll => W::AllScroll,
        E::ResizeHorizontal => W::EwResize,
        E::ResizeVertical => W::NsResize,
        E::ResizeNeSw => W::NeswResize,
        E::ResizeNwSe => W::NwseResize,
        E::ResizeEast => W::EResize,
        E::ResizeSouthEast => W::SeResize,
        E::ResizeSouth => W::SResize,
        E::ResizeSouthWest => W::SwResize,
        E::ResizeWest => W::WResize,
        E::ResizeNorthWest => W::NwResize,
        E::ResizeNorth => W::NResize,
        E::ResizeNorthEast => W::NeResize,
        E::ResizeColumn => W::ColResize,
        E::ResizeRow => W::RowResize,
        E::ZoomIn => W::ZoomIn,
        E::ZoomOut => W::ZoomOut,
    }
}

/// winit 设备 id → 稳定的字符串键。
///
/// Windows 上优先用 `DeviceIdExtWindows::persistent_identifier()`（HID 接口路径，
/// 跨会话稳定）；其它平台/失败时回退到 `DeviceId` 原始整数。
fn device_key(id: Option<winit::event::DeviceId>) -> Option<String> {
    let id = id?;
    #[cfg(windows)]
    {
        use winit::platform::windows::DeviceIdExtWindows as _;
        if let Some(p) = id.persistent_identifier() {
            if !p.is_empty() {
                return Some(p);
            }
        }
    }
    Some(format!("#{}", id.into_raw()))
}

/// 剪贴板快捷键（Ctrl+C/X/V）的字符；其他键返回 None。
fn clipboard_shortcut(key: &WinitKey) -> Option<char> {
    match key {
        WinitKey::Character(s) => s.chars().next().map(|c| c.to_ascii_lowercase()),
        _ => None,
    }
}

/// 把系统剪贴板的 CRLF 统一成 egui 使用的 LF。
fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// winit 的 preedit 范围是 UTF-8 字节索引 → egui 的字符索引。
fn preedit_range(text: &str, start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    let start_chars = text.get(..start)?.chars().count();
    let middle_chars = text.get(start..end)?.chars().count();
    Some(start_chars..start_chars + middle_chars)
}

/// 源码窗口/任务栏图标：`desktop/org.mypaint.MyPaint.ico`。
///
/// egui/winit 默认没有应用图标；用同一份上游 ICO 解码成 RGBA 后交给 winit。
/// 主窗口和子窗口共用。
fn app_icon() -> Option<Icon> {
    let img = image::load_from_memory(include_bytes!("../assets/org.mypaint.MyPaint.ico"))
        .ok()?
        .to_rgba8();
    let (w, h) = img.dimensions();
    RgbaIcon::new(img.into_raw(), w, h).ok().map(Icon::from)
}

/// 显式装一个 CJK 字体（egui 自带字体不含汉字）。系统字体回退是异步的，
/// 裸外壳不能依赖它第一帧就绪。
fn install_cjk_font(ctx: &egui::Context) {
    // GTK/Pango 在中文 Windows 上的实际 UI 字体是 Segoe UI（拉丁）
    // + Microsoft YaHei UI（汉字）的 fallback 链。egui 默认的
    // Ubuntu-Light + NotoSansSC-VF 在 24px 以下明显发虚，变量字体
    // 还会让笔画粗细不一致；这里直接按 GTK 的字体链装。
    let latin = std::fs::read("C:/Windows/Fonts/segoeui.ttf").ok();
    let cjk = std::fs::read("C:/Windows/Fonts/msyh.ttc").ok();
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = latin {
        fonts.font_data.insert(
            "ui-latin".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
    }
    if let Some(bytes) = cjk {
        // msyh.ttc: face 0 = Microsoft YaHei Regular,
        //           face 1 = Microsoft YaHei UI Regular（GTK 用的是 UI 字形）
        let mut data = egui::FontData::from_owned(bytes);
        data.index = 1;
        fonts
            .font_data
            .insert("ui-cjk".to_owned(), std::sync::Arc::new(data));
    }
    if fonts.font_data.contains_key("ui-latin") || fonts.font_data.contains_key("ui-cjk") {
        let defaults = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        let mut proportional = Vec::new();
        if fonts.font_data.contains_key("ui-latin") {
            proportional.push("ui-latin".to_owned());
        }
        if fonts.font_data.contains_key("ui-cjk") {
            proportional.push("ui-cjk".to_owned());
        }
        // 保留 egui 默认字体作为符号/emoji 的最后回退。
        proportional.extend(defaults);
        fonts
            .families
            .insert(egui::FontFamily::Proportional, proportional);
        if fonts.font_data.contains_key("ui-cjk") {
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("ui-cjk".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}
#[cfg(windows)]
fn cursor_position() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut p) } != 0 {
        Some((p.x, p.y))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn cursor_position() -> Option<(i32, i32)> {
    None
}

/// 鼠标所在显示器的 `(x, y, w, h)`；取不到时退回主显示器。
fn target_monitor(event_loop: &(dyn ActiveEventLoop + 'static)) -> Option<(i64, i64, i64, i64)> {
    let cursor = cursor_position();
    let pick = |m: &winit::monitor::MonitorHandle| {
        let p = m
            .position()
            .unwrap_or(winit::dpi::PhysicalPosition::new(0, 0));
        let s = m
            .current_video_mode()
            .map(|v| v.size())
            .unwrap_or(winit::dpi::PhysicalSize::new(0, 0));
        (p.x as i64, p.y as i64, s.width as i64, s.height as i64)
    };
    let hit = cursor.and_then(|(x, y)| {
        event_loop
            .available_monitors()
            .find(|m| {
                let p = m
                    .position()
                    .unwrap_or(winit::dpi::PhysicalPosition::new(0, 0));
                let s = m
                    .current_video_mode()
                    .map(|v| v.size())
                    .unwrap_or(winit::dpi::PhysicalSize::new(0, 0));
                x >= p.x && y >= p.y && x < p.x + s.width as i32 && y < p.y + s.height as i32
            })
            .map(|m| pick(&m))
    });
    hit.or_else(|| event_loop.primary_monitor().map(|m| pick(&m)))
}

impl Shell {
    fn make_state(&mut self, event_loop: &(dyn ActiveEventLoop + 'static)) {
        // 上次窗口尺寸/位置（`workspace.layout.position`）。
        let prefs = pigment_engine::prefs::Prefs::load(pigment_engine::prefs::settings_path());
        let saved_max = crate::app::saved_window_maximized(&prefs);
        let saved_full = crate::app::saved_window_fullscreen(&prefs)
            || std::env::var("PIGMENT_STARTUP_FULLSCREEN").as_deref() == Ok("1");
        // 源码 `windowing.py:658-699` 的 `_final_rectangle`：`position`
        // 的 w/h 可以是负数（显示器尺寸减…），x/y 可以是负数（右/下锚定）。
        let target = target_monitor(event_loop).unwrap_or((0, 0, 2560, 1440));
        let (screen_w, screen_h) = event_loop
            .primary_monitor()
            .and_then(|m| m.current_video_mode().map(|v| v.size()))
            .map(|s| (s.width as i64, s.height as i64))
            .unwrap_or((2560, 1440));
        let (final_x, final_y, final_w, final_h) = match crate::app::saved_window_placement(&prefs)
        {
            Some(p) => crate::app::final_window_rectangle(p, screen_w, screen_h, target, 100),
            None => (None, None, None, None),
        };
        let (default_w, default_h) = (final_w.unwrap_or(1280), final_h.unwrap_or(800));
        let mut attrs = WindowAttributes::default()
            .with_title("pigment (winit 0.31 shell)")
            .with_window_icon(app_icon())
            // 参考机 MyPaint 的 Windows 原生标题栏是浅色（GTK 窗口不申请
            // immersive dark title bar），与客户端暗色主题无关。
            .with_theme(Some(Theme::Light))
            // 首帧之前不要露脸：`PigmentApp::new_with_renderer`（196 支笔刷
            // + 恢复自动备份文档 + 字体扫描）要一两秒才出第一帧，这期间可见
            // 而空白的窗口会被 Windows 用窗口类背景刷成一片纯白。GTK 版没有
            // 这个问题（它 realize 之前就把 UI 建好了）。这里先隐藏，等
            // make_state 里铺完底色（present_background）再 set_visible。
            .with_visible(false)
            .with_surface_size(winit::dpi::PhysicalSize::new(
                default_w.max(1) as u32,
                default_h.max(1) as u32,
            ));
        if let (Some(x), Some(y)) = (final_x, final_y) {
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x as i32, y as i32));
        }
        // 没有 position 时交给窗口管理器（源码 `workspace.py:262-264`）。
        let window: Arc<dyn Window> = match event_loop.create_window(attrs) {
            Ok(w) => Arc::from(w),
            Err(e) => {
                eprintln!("create_window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        if saved_max {
            window.set_maximized(true);
        }
        if saved_full {
            window.set_fullscreen(Some(winit::monitor::Fullscreen::Borderless(None)));
        }
        let size = window.surface_size();

        let Some((instance, surface, adapter, device, queue)) = init_gpu(&window) else {
            event_loop.exit();
            return;
        };
        let caps = surface.get_capabilities(&adapter);
        // egui-wgpu 的 shader 输出 gamma-space，必须配非 sRGB 目标。
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported");
        config.format = format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        // Vulkan/DX12 的 alpha 模式集合不同：优先 Opaque，不支持时取后端默认。
        config.alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::Opaque)
        {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes[0]
        };
        surface.configure(&device, &config);
        // 先垫一帧底色再显示窗口：裸 winit 外壳在
        // `PigmentApp::new_with_renderer`（笔刷库 / 自动备份恢复 / 字体扫描）
        // 完成之前一帧都画不出来，可见而空白的窗口会被 Windows 用窗口类背景
        // 刷成纯白。窗口创建后 winit 还会应用上次保存的尺寸（一次排队的
        // WM_SIZE），所以显示之前先按当前客户区对齐 surface。
        let size = window.surface_size();
        if size.width != config.width || size.height != config.height {
            config.width = size.width.max(1);
            config.height = size.height.max(1);
            surface.configure(&device, &config);
        }
        present_background(&device, &queue, &surface);
        window.set_visible(true);

        let ctx = egui::Context::default();
        // 主窗口与原生子窗口共用同一套 Adwaita Dark 色板（布局仍是紧凑的 ArrowVortex 结构）。
        crate::shell::theme::install(&ctx);
        // 系统字体回退（CJK 等）；eframe 默认做了这件事，裸外壳要自己加。
        ctx.add_font_provider(std::sync::Arc::new(
            egui_system_font_provider::SystemFontProvider::new(),
        ));
        install_cjk_font(&ctx);
        let renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        let mut app = PigmentApp::new_with_renderer(&device, format);
        // `PIGMENT_NO_GPU_DABS=1`：强制纯 CPU 光栅（对照实验用）。
        if adapter.get_info().device_type != wgpu::DeviceType::Cpu
            && std::env::var_os("PIGMENT_NO_GPU_DABS").is_none()
        {
            app.set_gpu_dab_engine(pigment_render::GpuDabEngine::new(&device, &queue));
        }
        // Plan A：画布走独立 wgpu pass，不再挂 egui PaintCallback。
        app.direct_canvas = true;
        let mut raw = RawInput::default();
        let scale = UI_SCALE;
        // 裸 winit 外壳必须把 native DPI 告给 egui；否则 egui 按 1×
        // 栅格化字体，再由 renderer 拉伸，文字会糊。
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        raw.screen_rect = Some(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(config.width as f32 / scale, config.height as f32 / scale),
        ));
        self.state = Some(State {
            clipboard: arboard::Clipboard::new().ok(),
            window,
            surface,
            config,
            device,
            queue,
            instance,
            adapter,
            format,
            ctx,
            renderer,
            app,
            raw,
            modifiers: Modifiers::default(),
            start: Instant::now(),
            pointer_down: false,
            last_pointer: None,
            last_move_sent: None,
            last_cursor: None,
            ime_enabled: false,
            last_title: String::new(),
            subwindows: Vec::new(),
            dnd_fetch: None,
        });
        if let Some(st) = self.state.as_mut() {
            // 初始化期间窗口可能已经被撑到保存的尺寸（`with_surface_size` 之后
            // 那次 WM_SIZE 还排在事件队列里，轮不到处理）。显示之前按真实客户区
            // 重配一次 surface 并重铺底色，避免露出没画过的边。
            let size = st.window.surface_size();
            if size.width != st.config.width || size.height != st.config.height {
                st.config.width = size.width.max(1);
                st.config.height = size.height.max(1);
                st.surface.configure(&st.device, &st.config);
                st.raw.screen_rect = Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(
                        st.config.width as f32 / UI_SCALE,
                        st.config.height as f32 / UI_SCALE,
                    ),
                ));
                present_background(&st.device, &st.queue, &st.surface);
            }
            // 底色已铺好，现在显示才不会闪白。
            st.window.set_visible(true);
            st.window.request_redraw();
        }
    }
}

/// 往 surface 里铺一帧纯底色（不经过 egui）。
///
/// 用途只有一个：窗口可见之前先让它有内容。裸 winit 外壳在
/// `PigmentApp::new_with_renderer` 完成之前一帧都画不出来，可见的空窗口
/// 会被 Windows 刷成纯白——这就是"启动先看到一个白板"的根因。
/// 渲染后端选择：默认优先 Vulkan，其次 D3D12；可用
/// `PIGMENT_WGPU_BACKEND=vulkan|dx12|gl|all|auto` 覆盖（`--vulkan` 亦同）。
///
/// 显式指定后端时**不**回退，方便确认 Vulkan 初始化是否真的成功；
/// `auto`/未设置时才在首选后端无可用适配器时回退到全部后端。
fn backend_preference() -> (wgpu::Backends, bool, bool) {
    let raw = std::env::var("PIGMENT_WGPU_BACKEND").unwrap_or_default();
    match raw.trim().to_ascii_lowercase().as_str() {
        "vulkan" | "vk" => (wgpu::Backends::VULKAN, true, false),
        "dx12" | "d3d12" => (wgpu::Backends::DX12, false, false),
        "gl" | "gles" | "opengl" => (wgpu::Backends::GL, false, false),
        "all" => (wgpu::Backends::all(), true, false),
        "" | "auto" | "default" => (
            wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            true,
            true,
        ),
        other => {
            eprintln!("PIGMENT_WGPU_BACKEND={other:?} 未知，按 auto 处理");
            (
                wgpu::Backends::VULKAN | wgpu::Backends::DX12,
                true,
                true,
            )
        }
    }
}

/// 适配器排序：独显优先，同类型内 Vulkan 优先（用户要求的 Vulkan 兼容路线）。
fn adapter_rank(adapter: &wgpu::Adapter) -> (u8, u8) {
    let info = adapter.get_info();
    let ty = match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Cpu => 3,
        wgpu::DeviceType::Other => 4,
    };
    let backend = match info.backend {
        wgpu::Backend::Vulkan => 0,
        wgpu::Backend::Dx12 => 1,
        wgpu::Backend::Gl => 2,
        wgpu::Backend::Metal => 3,
        _ => 4,
    };
    (ty, backend)
}

/// 建立 instance/surface，并按优先级尝试适配器与设备。
///
/// 画布、图层和笔刷引擎都不依赖这里；这个函数只把 wgpu 后端选好，
/// 之后 [`PigmentApp`] 的渲染器在任意后端上使用同一套管线。
fn init_gpu(
    window: &Arc<dyn Window>,
) -> Option<(
    wgpu::Instance,
    wgpu::Surface<'static>,
    wgpu::Adapter,
    wgpu::Device,
    wgpu::Queue,
)> {
    let (backends, prefer_vulkan, allow_fallback) = backend_preference();
    let mut tiers = vec![backends];
    if allow_fallback && backends != wgpu::Backends::all() {
        tiers.push(wgpu::Backends::all());
    }
    let tier_count = tiers.len();

    for (tier_idx, tier) in tiers.into_iter().enumerate() {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = tier;
        let instance = wgpu::Instance::new(desc);
        let surface = match instance.create_surface(window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("create_surface({tier:?}) failed: {e}");
                continue;
            }
        };
        let mut adapters = pollster::block_on(instance.enumerate_adapters(tier));
        adapters.retain(|a| a.is_surface_supported(&surface));
        adapters.sort_by_key(adapter_rank);
        if adapters.is_empty() {
            // 某些后端不实现枚举；退回 request_adapter。
            if let Ok(a) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..Default::default()
            })) {
                adapters.push(a);
            }
        }
        for adapter in adapters {
            let info = adapter.get_info();
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("pigment-shell"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })) {
                Ok((device, queue)) => {
                    eprintln!(
                        "wgpu backend={:?} type={:?} adapter={} driver={} {}",
                        info.backend,
                        info.device_type,
                        info.name,
                        info.driver,
                        if prefer_vulkan && info.backend == wgpu::Backend::Vulkan {
                            "(Vulkan path)"
                        } else {
                            ""
                        }
                    );
                    return Some((instance, surface, adapter, device, queue));
                }
                Err(e) => eprintln!(
                    "request_device failed on {:?} / {}: {e}",
                    info.backend, info.name
                ),
            }
        }
        eprintln!("后端 {tier:?} 没有可用适配器/设备");
        if tier_idx + 1 < tier_count {
            eprintln!("回退到全部后端重试");
        }
    }
    eprintln!(
        "没有可用的 wgpu 后端；可用 PIGMENT_WGPU_BACKEND=vulkan|dx12|gl|all 指定"
    );
    None
}

fn present_background(device: &wgpu::Device, queue: &wgpu::Queue, surface: &wgpu::Surface<'_>) {
    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        _ => return,
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pigment-first-frame"),
    });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pigment-first-frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    queue.submit(std::iter::once(encoder.finish()));
    queue.present(frame);
}

/// 新建一个原生子窗口（共享主窗口的 wgpu device/queue）。
fn create_subwindow(
    st: &mut State,
    kind: SubWindowKind,
    event_loop: &(dyn ActiveEventLoop + 'static),
) {
    let (w, h) = kind.default_size();
    let mut attrs = WindowAttributes::default()
        .with_title(kind.title())
        .with_window_icon(app_icon())
        // 与主窗口一致：原生标题栏保持浅色（参考 MyPaint 的 GTK 窗口）。
        .with_theme(Some(Theme::Light))
        // GTK 的默认尺寸是 96dpi 逻辑点；参考机 scale=1.0，所以直接用物理尺寸。
        .with_surface_size(winit::dpi::PhysicalSize::new(w, h));
    // `windowing.SubWindow.__init__` 的 `set_transient_for(app.drawWindow)`
    // （`windowing.py:74-77`）：Windows 上等价物是 owned window，保证子窗口
    // 始终位于主窗口之上、随主窗口最小化/销毁。
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
        if let Ok(handle) = st.window.window_handle() {
            if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                let owner = win32.hwnd.get() as *mut core::ffi::c_void;
                let platform = winit::platform::windows::WindowAttributesWindows::default()
                    .with_owner_window(owner);
                attrs = attrs.with_platform_attributes(Box::new(platform));
            }
        }
    }
    if let Some((x, y, sw, sh)) = subwindow::geometry_from_prefs(&st.app.prefs, kind) {
        attrs = attrs
            .with_surface_size(winit::dpi::PhysicalSize::new(sw, sh))
            .with_position(winit::dpi::PhysicalPosition::new(x, y));
    } else {
        // GTK 的 transient 对话框默认由窗口管理器居中（参考机偏好窗口
        // 在 2560×1440 上位于 (826,377)，即屏幕中心附近）。
        let mode = event_loop
            .primary_monitor()
            .and_then(|m| m.current_video_mode())
            .map(|v| v.size())
            .unwrap_or(winit::dpi::PhysicalSize::new(2560, 1440));
        let x = ((mode.width as i32 - w as i32) / 2).max(0);
        let y = ((mode.height as i32 - h as i32) / 2).max(0);
        attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
    }
    let window: Arc<dyn Window> = match event_loop.create_window(attrs) {
        Ok(w) => Arc::from(w),
        Err(e) => {
            eprintln!("create subwindow {:?} failed: {e}", kind);
            // 清掉 app 的打开标志，避免 `sync_subwindows` 每帧重试失败的窗口。
            st.app.native_subwindows[kind.index()] = false;
            kind.set_closed(&mut st.app);
            return;
        }
    };
    // GTK 的 transient 子窗口显示时会压在父窗口上方；winit 的 owner 关系
    // 在 Windows 上只保证“属于”，不保证主窗口被外部置顶时子窗口也在上面
    // （截图脚本会把主窗口设成 HWND_TOPMOST）。这里按 owner 的 z-order
    // 继承：父窗口是 topmost 时子窗口也 topmost，否则把子窗口放到父窗口上方。
    #[cfg(windows)]
    inherit_owner_z_order(&st.window, &window);
    window.focus_window();
    let size = window.surface_size();
    let surface = match st.instance.create_surface(window.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("create subwindow surface {:?} failed: {e}", kind);
            st.app.native_subwindows[kind.index()] = false;
            kind.set_closed(&mut st.app);
            return;
        }
    };
    let caps = surface.get_capabilities(&st.adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .unwrap_or(st.format);
    let mut config = surface
        .get_default_config(&st.adapter, size.width.max(1), size.height.max(1))
        .expect("subwindow surface unsupported");
    config.format = format;
    config.present_mode = wgpu::PresentMode::AutoVsync;
    config.alpha_mode = if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::Opaque)
    {
        wgpu::CompositeAlphaMode::Opaque
    } else {
        caps.alpha_modes[0]
    };
    surface.configure(&st.device, &config);
    let ctx = egui::Context::default();
    crate::shell::theme::install(&ctx);
    ctx.add_font_provider(std::sync::Arc::new(
        egui_system_font_provider::SystemFontProvider::new(),
    ));
    install_cjk_font(&ctx);
    let renderer =
        egui_wgpu::Renderer::new(&st.device, format, egui_wgpu::RendererOptions::default());
    let mut raw = RawInput::default();
    let scale = UI_SCALE;
    raw.viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(scale);
    raw.screen_rect = Some(subwindow::screen_rect(egui::vec2(
        config.width as f32 / scale,
        config.height as f32 / scale,
    )));
    st.subwindows.push(SubWindow {
        kind,
        window: window.clone(),
        surface,
        config,
        ctx,
        renderer,
        raw,
        modifiers: st.modifiers,
        last_pointer: None,
        last_cursor: None,
        ime_enabled: false,
        last_title: kind.title().to_string(),
    });
    st.app.native_subwindows[kind.index()] = true;
    window.request_redraw();
}

/// 把子窗口放到 owner 之上；owner 是 topmost 时子窗口也 topmost。
///
/// 截图/审计脚本常把主窗口设为 `HWND_TOPMOST`，此时普通 owned window
/// 仍会被压在下面。GTK 的 transient 对话框不会出现这种层级倒挂，所以
/// 这里按 owner 的扩展样式补齐。
#[cfg(windows)]
fn inherit_owner_z_order(owner: &Arc<dyn Window>, child: &Arc<dyn Window>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, WS_EX_TOPMOST,
    };
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    fn hwnd(window: &Arc<dyn Window>) -> Option<*mut core::ffi::c_void> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut core::ffi::c_void),
            _ => None,
        }
    }

    let (Some(owner), Some(child)) = (hwnd(owner), hwnd(child)) else {
        return;
    };
    unsafe {
        let ex_style = GetWindowLongPtrW(owner, GWL_EXSTYLE) as u32;
        let insert_after = if ex_style & WS_EX_TOPMOST != 0 {
            HWND_TOPMOST
        } else {
            owner
        };
        SetWindowPos(
            child,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 把 app 的“打开”状态与原生子窗口列表同步。
fn sync_subwindows(st: &mut State, event_loop: &(dyn ActiveEventLoop + 'static)) {
    let mut changed = false;
    // 关闭：app 里已经关了，或动作切掉了。
    let mut i = 0;
    while i < st.subwindows.len() {
        let kind = st.subwindows[i].kind;
        if !kind.is_open(&st.app) {
            st.app.native_subwindows[kind.index()] = false;
            st.subwindows.remove(i);
            changed = true;
        } else {
            i += 1;
        }
    }
    // 打开：app 里开着但还没有原生窗口。
    for kind in SubWindowKind::ALL {
        if kind.is_open(&st.app) && !st.subwindows.iter().any(|w| w.kind == kind) {
            create_subwindow(st, kind, event_loop);
            changed = true;
        }
    }
    // 原生窗口出现/消失的那一帧，主窗口可能还留着上一帧的嵌入式回退
    // （`preferences_window` 等）。winit 不会自动重绘主窗口，必须显式
    // 请求一次，否则回退窗口的像素会一直留在主画布上。
    if changed {
        st.window.request_redraw();
    }
}

/// 子窗口内容：直接调用 app 的嵌入式绘制方法（不套内层 Window）。
fn draw_subwindow(_app: &mut PigmentApp, _kind: SubWindowKind, _ui: &mut egui::Ui) {
    // 子窗口的界面代码已整体删除；外壳暂时保留窗口创建逻辑，
    // 但没有任何动作会再把它们打开。
}

/// 处理一个子窗口事件。返回 true 表示窗口已关闭/应移除。
fn subwindow_event(
    st: &mut State,
    _event_loop: &(dyn ActiveEventLoop + 'static),
    idx: usize,
    event: WindowEvent,
) -> bool {
    let kind = st.subwindows[idx].kind;
    let scale = UI_SCALE;
    let pos =
        |p: winit::dpi::PhysicalPosition<f64>| Pos2::new(p.x as f32 / scale, p.y as f32 / scale);
    let mut close = false;
    match event {
        WindowEvent::CloseRequested => {
            kind.set_closed(&mut st.app);
            close = true;
        }
        WindowEvent::Moved(p) => {
            let size = st.subwindows[idx].window.surface_size();
            subwindow::store_geometry(
                &mut st.app.prefs,
                kind,
                p.x,
                p.y,
                size.width.max(1),
                size.height.max(1),
            );
        }
        WindowEvent::SurfaceResized(size) => {
            {
                let sw = &mut st.subwindows[idx];
                sw.config.width = size.width.max(1);
                sw.config.height = size.height.max(1);
                sw.surface.configure(&st.device, &sw.config);
                sw.raw.screen_rect = Some(subwindow::screen_rect(egui::vec2(
                    sw.config.width as f32 / scale,
                    sw.config.height as f32 / scale,
                )));
            }
            let p = st.subwindows[idx].window.surface_position();
            subwindow::store_geometry(
                &mut st.app.prefs,
                kind,
                p.x,
                p.y,
                size.width.max(1),
                size.height.max(1),
            );
            st.subwindows[idx].window.request_redraw();
        }
        WindowEvent::PointerMoved { position, .. } => {
            let p = pos(position);
            let sw = &mut st.subwindows[idx];
            sw.last_pointer = Some(p);
            sw.raw.events.push(Event::PointerMoved(p));
            sw.window.request_redraw();
        }
        WindowEvent::PointerButton {
            state,
            position,
            button,
            ..
        } => {
            if let Some(b) = mouse_button(&button) {
                let p = pos(position);
                let pressed = state == ElementState::Pressed;
                let sw = &mut st.subwindows[idx];
                sw.last_pointer = Some(p);
                sw.raw.events.push(Event::PointerButton {
                    pos: p,
                    button: b,
                    pressed,
                    modifiers: sw.modifiers,
                });
                sw.window.request_redraw();
            }
        }
        WindowEvent::KeyboardInput { event, .. } => {
            let pressed = event.state == ElementState::Pressed;
            // 子窗口里的 Gtk.Entry 也要有 Ctrl+C/X/V；只有文本控件有焦点时
            // 才把按键翻译成 egui 的复制/剪切/粘贴事件（照主窗口策略）。
            if pressed
                && st.subwindows[idx].modifiers.ctrl
                && st.subwindows[idx].ctx.text_edit_focused()
            {
                let shortcut = clipboard_shortcut(&event.logical_key);
                let paste = if shortcut == Some('v') {
                    st.clipboard
                        .as_mut()
                        .and_then(|c| c.get_text().ok())
                        .map(|t| normalize_paste(&t))
                } else {
                    None
                };
                let sw = &mut st.subwindows[idx];
                match shortcut {
                    Some('c') => sw.raw.events.push(Event::Copy),
                    Some('x') => sw.raw.events.push(Event::Cut),
                    Some('v') => {
                        if let Some(text) = paste {
                            if !text.is_empty() {
                                sw.raw.events.push(Event::Paste(text));
                            }
                        }
                    }
                    _ => {}
                }
                sw.window.request_redraw();
                return false;
            }
            if let Some(key) = map_key(&event.logical_key) {
                let sw = &mut st.subwindows[idx];
                sw.raw.events.push(Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: event.repeat,
                    modifiers: sw.modifiers,
                });
            }
            if pressed {
                if let Some(text) = &event.text {
                    if !text.chars().next().is_some_and(|c| c.is_control()) {
                        st.subwindows[idx]
                            .raw
                            .events
                            .push(Event::Text(text.to_string()));
                    }
                }
            }
            st.subwindows[idx].window.request_redraw();
        }
        WindowEvent::Focused(false) => {
            let sw = &mut st.subwindows[idx];
            if sw.ime_enabled {
                let _ = sw.window.request_ime_update(ImeRequest::Disable);
                sw.ime_enabled = false;
            }
        }
        WindowEvent::Ime(ime) => {
            // 与主窗口相同的 winit → egui 映射（`:1051-1077`）。
            match ime {
                winit::event::Ime::Preedit(text, range) => {
                    let active_range_chars =
                        range.and_then(|(start, end)| preedit_range(&text, start, end));
                    st.subwindows[idx]
                        .raw
                        .events
                        .push(Event::Ime(egui::ImeEvent::Preedit {
                            text,
                            active_range_chars,
                        }));
                }
                winit::event::Ime::Commit(text) => {
                    st.subwindows[idx]
                        .raw
                        .events
                        .push(Event::Ime(egui::ImeEvent::Commit(text)));
                }
                winit::event::Ime::Enabled
                | winit::event::Ime::Disabled
                | winit::event::Ime::DeleteSurrounding { .. } => {}
                _ => {}
            }
            st.subwindows[idx].window.request_redraw();
        }
        WindowEvent::ModifiersChanged(m) => {
            let s = m.state();
            let mods = Modifiers {
                alt: s.alt_key(),
                ctrl: s.control_key(),
                shift: s.shift_key(),
                mac_cmd: false,
                command: s.control_key(),
            };
            let sw = &mut st.subwindows[idx];
            sw.modifiers = mods;
            sw.raw.events.push(Event::ModifiersChanged(mods));
            sw.window.request_redraw();
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let d = match delta {
                MouseScrollDelta::LineDelta(x, y) => Vec2::new(x * 50.0, y * 50.0),
                MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32),
                _ => Vec2::ZERO,
            };
            let sw = &mut st.subwindows[idx];
            sw.raw.events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: d,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: sw.modifiers,
            });
            sw.window.request_redraw();
        }
        WindowEvent::RedrawRequested => {
            if render_subwindow(st, idx) {
                return true;
            }
        }
        _ => {}
    }
    if close {
        st.app.native_subwindows[kind.index()] = false;
    }
    close
}

/// 绘制并 present 一个子窗口；返回 true 表示它应该被移除。
fn render_subwindow(st: &mut State, idx: usize) -> bool {
    let kind = st.subwindows[idx].kind;
    let now = st.start.elapsed().as_secs_f64();
    let scale = UI_SCALE;
    let (mut output, close) = {
        let app = &mut st.app;
        let sw = &mut st.subwindows[idx];
        sw.raw.time = Some(now);
        let ctx = sw.ctx.clone();
        let raw = sw.raw.take();
        let output = ctx.run_ui(raw, |ui| {
            draw_subwindow(app, kind, ui);
        });
        (output, !kind.is_open(app))
    };
    if close {
        // 窗口马上销毁：未应用的纹理增量必须先清掉，否则 epaint 会 panic。
        output.textures_delta.clear();
        st.app.native_subwindows[kind.index()] = false;
        return true;
    }
    let sw = &mut st.subwindows[idx];
    // 标题：BrushIconEditor 在编辑态切换到 "(editing)"
    // （`brushiconeditor.py:35-53`）。
    let title = kind.title_for(&st.app);
    if title != sw.last_title {
        sw.window.set_title(&title);
        sw.last_title = title;
    }
    // 子窗口的 IME：文本框获得焦点时启用，候选框跟随光标
    // （与主窗口 `render_frame` 相同）。
    let ime_wanted = output.platform_output.ime.is_some();
    if ime_wanted != sw.ime_enabled {
        if ime_wanted {
            let rect = output
                .platform_output
                .ime
                .as_ref()
                .map(|i| i.cursor_rect)
                .unwrap_or(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1.0, 1.0),
                ));
            let data = ImeRequestData::default().with_cursor_area(
                LogicalPosition::new(rect.min.x as f64, rect.min.y as f64).into(),
                LogicalSize::new(rect.width().max(1.0) as f64, rect.height().max(1.0) as f64)
                    .into(),
            );
            if let Some(req) =
                ImeEnableRequest::new(ImeCapabilities::new().with_cursor_area(), data)
            {
                let _ = sw.window.request_ime_update(ImeRequest::Enable(req));
                sw.ime_enabled = true;
            }
        } else {
            let _ = sw.window.request_ime_update(ImeRequest::Disable);
            sw.ime_enabled = false;
        }
    } else if sw.ime_enabled {
        if let Some(ime) = output.platform_output.ime.as_ref() {
            let p = ime.cursor_rect.min;
            let s = ime.cursor_rect.size();
            let data = ImeRequestData::default().with_cursor_area(
                LogicalPosition::new(p.x as f64, p.y as f64).into(),
                LogicalSize::new(s.x.max(1.0) as f64, s.y.max(1.0) as f64).into(),
            );
            let _ = sw.window.request_ime_update(ImeRequest::Update(data));
        }
    }
    // 子窗口文本框的复制命令同样写系统剪贴板。
    for cmd in &output.platform_output.commands {
        if let egui::OutputCommand::CopyText(text) = cmd {
            if let Some(cb) = st.clipboard.as_mut() {
                let _ = cb.set_text(text.clone());
            }
        }
    }
    // 光标
    let cursor = output.platform_output.cursor_icon;
    if cursor == egui::CursorIcon::None {
        sw.window.set_cursor_visible(false);
        // 进入自绘光标区域时把“已选光标”复位成默认箭头；否则从滑条/分隔条
        // 直接移到画布再移出时，winit 可能恢复上一次的 resize 光标。
        if sw.last_cursor != Some(cursor) {
            sw.window
                .set_cursor(WinitCursor::Icon(map_cursor(egui::CursorIcon::Default)));
            sw.last_cursor = Some(cursor);
        }
    } else {
        sw.window.set_cursor_visible(true);
        if sw.last_cursor != Some(cursor) {
            sw.window.set_cursor(WinitCursor::Icon(map_cursor(cursor)));
            sw.last_cursor = Some(cursor);
        }
    }
    let jobs = sw.ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [sw.config.width, sw.config.height],
        pixels_per_point: scale,
    };
    let mut textures = output.textures_delta;
    for (id, deltas) in &textures.set {
        for delta in deltas {
            sw.renderer
                .update_texture(&st.device, &st.queue, *id, delta);
        }
    }
    let frame = match sw.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) => f,
        wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        other => {
            eprintln!("subwindow surface: {other:?}");
            sw.surface.configure(&st.device, &sw.config);
            textures.clear();
            return false;
        }
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = st
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pigment-subwindow"),
        });
    let cmd = sw
        .renderer
        .update_buffers(&st.device, &st.queue, &mut encoder, &jobs, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui-subwindow"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut pass = pass.forget_lifetime();
        sw.renderer.render(&mut pass, &jobs, &screen);
    }
    st.queue
        .submit(cmd.into_iter().chain(std::iter::once(encoder.finish())));
    st.queue.present(frame);
    for id in &textures.free {
        sw.renderer.free_texture(id);
    }
    textures.clear();
    let delay = output
        .viewport_output
        .values()
        .map(|v| v.repaint_delay)
        .min()
        .unwrap_or(Duration::from_millis(500));
    if delay.is_zero() {
        sw.window.request_redraw();
    } else {
        let window = sw.window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay.min(Duration::from_secs(1)));
            window.request_redraw();
        });
    }
    false
}

impl ApplicationHandler for Shell {
    fn can_create_surfaces(&mut self, event_loop: &(dyn ActiveEventLoop + 'static)) {
        if self.state.is_none() {
            self.make_state(event_loop);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &(dyn ActiveEventLoop + 'static),
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(st) = self.state.as_mut() else {
            return;
        };
        // 子窗口事件独立路由（每个窗口有自己的 egui Context）。
        if window_id != st.window.id() {
            if let Some(idx) = st
                .subwindows
                .iter()
                .position(|w| w.window.id() == window_id)
            {
                if subwindow_event(st, event_loop, idx, event) {
                    st.subwindows.remove(idx);
                }
            }
            return;
        }
        let scale = UI_SCALE;
        let pos = |p: winit::dpi::PhysicalPosition<f64>| {
            Pos2::new(p.x as f32 / scale, p.y as f32 / scale)
        };
        match event {
            WindowEvent::DragEntered { id, .. } => {
                // 只接受 URI 列表（资源管理器拖文件）；颜色拖放需要
                // application/x-color 自定义类型，winit 暂不暴露。
                if let Ok(dt) = event_loop.data_transfer(id) {
                    // 文件（UriList）优先；没有文件时接受文本颜色
                    // （winit 没有 GTK 的 application/x-color）。
                    let hint = if dt.has_type(&TypeHint::UriList) {
                        Some(TypeHint::UriList)
                    } else if dt.has_type(&TypeHint::Plaintext) {
                        Some(TypeHint::Plaintext)
                    } else {
                        None
                    };
                    if let Some(hint) = hint {
                        let _ = event_loop.set_valid_dnd_actions(id, &[DndAction::Copy]);
                        st.dnd_fetch = event_loop.fetch_data_transfer(id, &hint).ok();
                    } else {
                        let _ = event_loop.set_valid_dnd_actions(id, &[]);
                    }
                }
            }
            WindowEvent::DataTransferReceived { value, serial, .. } => {
                if st.dnd_fetch == Some(serial) {
                    st.dnd_fetch = None;
                    match value.type_().hint() {
                        Some(TypeHint::Plaintext) => {
                            if let Ok(text) = value.try_as_string() {
                                if let Some(rgb) = crate::app::parse_color_text(&text) {
                                    st.app.set_color(pigment_engine::color::Color::Rgb(
                                        pigment_engine::color::RgbColor::from_rgb([
                                            rgb[0] as f64,
                                            rgb[1] as f64,
                                            rgb[2] as f64,
                                        ]),
                                    ));
                                    st.window.request_redraw();
                                }
                            }
                        }
                        Some(TypeHint::UriList) => {
                            if let Ok(paths) = value.try_as_file_paths() {
                                if let Some(path) = paths.into_iter().next() {
                                    st.app.dropped_file = Some(path);
                                    st.window.request_redraw();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::DragLeft { .. } | WindowEvent::DragDropped { .. } => {
                st.dnd_fetch = None;
            }
            WindowEvent::CloseRequested => {
                // 走应用层的退出确认（`quit_cb`）；确认/无未保存后由
                // render_frame 返回 quit 标志再退出事件循环。
                let now = st.start.elapsed().as_secs_f64();
                st.app.request_quit(now);
                st.window.request_redraw();
            }
            WindowEvent::Moved(pos) => {
                // 最大化/全屏时不记录几何（源码 `workspace.py:604-638`
                // 在 window-state-event 里跳过）。
                if !st.window.is_maximized() && st.window.fullscreen().is_none() {
                    st.app.window_pos = Some((pos.x, pos.y));
                }
            }
            WindowEvent::SurfaceResized(size) => {
                if !st.window.is_maximized() && st.window.fullscreen().is_none() {
                    st.app.window_size = Some((size.width.max(1), size.height.max(1)));
                }
                st.config.width = size.width.max(1);
                st.config.height = size.height.max(1);
                st.surface.configure(&st.device, &st.config);
                st.raw.screen_rect = Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(
                        st.config.width as f32 / scale,
                        st.config.height as f32 / scale,
                    ),
                ));
                st.window.request_redraw();
            }
            WindowEvent::PointerMoved {
                position,
                source,
                device_id,
                ..
            } => {
                let p = pos(position);
                st.last_pointer = Some(p);
                st.last_move_sent = Some(p);
                st.raw.events.push(Event::PointerMoved(p));
                if let PointerSource::TabletTool { kind, data } = source {
                    st.app.tablet.set_device(device_key(device_id));
                    let pressure = data.force.map(|f| f.normalized(None) as f32).unwrap_or(1.0);
                    let tilt = data
                        .tilt
                        .map(|t| (f32::from(t.x) / 90.0, f32::from(t.y) / 90.0))
                        .unwrap_or((0.0, 0.0));
                    st.app
                        .tablet
                        .set(pressure, tilt, format!("{kind:?}").contains("Eraser"));
                }
                st.window.request_redraw();
            }
            WindowEvent::PointerButton {
                state,
                position,
                button,
                device_id,
                ..
            } => {
                if let Some(b) = mouse_button(&button) {
                    let p = pos(position);
                    st.last_pointer = Some(p);
                    let pressed = state == ElementState::Pressed;
                    if b == PointerButton::Primary {
                        st.pointer_down = pressed;
                    }
                    st.raw.events.push(Event::PointerButton {
                        pos: p,
                        button: b,
                        pressed,
                        modifiers: st.modifiers,
                    });
                    if let ButtonSource::TabletTool { kind, data, .. } = button {
                        st.app.tablet.set_device(device_key(device_id));
                        let pressure = data.force.map(|f| f.normalized(None) as f32).unwrap_or(1.0);
                        let tilt = data
                            .tilt
                            .map(|t| (f32::from(t.x) / 90.0, f32::from(t.y) / 90.0))
                            .unwrap_or((0.0, 0.0));
                        st.app
                            .tablet
                            .set(pressure, tilt, format!("{kind:?}").contains("Eraser"));
                    }
                    if !pressed {
                        st.app.tablet.clear();
                    }
                }
                st.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // 剪贴板快捷键（照 egui-winit `:1128-1152`）：只有文本框
                // 有焦点时才在这里转成 Copy/Cut/Paste 事件。画布上的
                // Ctrl+C/Ctrl+V 必须继续走键盘表，命中 CopyLayer/PasteLayer
                // （resources.xml:456/465）。
                if pressed && st.modifiers.ctrl && st.ctx.text_edit_focused() {
                    let shortcut = clipboard_shortcut(&event.logical_key);
                    match shortcut {
                        Some('c') => {
                            st.raw.events.push(Event::Copy);
                            st.window.request_redraw();
                            return;
                        }
                        Some('x') => {
                            st.raw.events.push(Event::Cut);
                            st.window.request_redraw();
                            return;
                        }
                        Some('v') => {
                            let text = st.clipboard.as_mut().and_then(|c| c.get_text().ok());
                            if let Some(text) = text {
                                let text = normalize_paste(&text);
                                if !text.is_empty() {
                                    st.raw.events.push(Event::Paste(text));
                                }
                            } else if let Some(image) =
                                st.clipboard.as_mut().and_then(|c| c.get_image().ok())
                            {
                                if let Some(ci) = clipboard_to_color_image(
                                    image.width,
                                    image.height,
                                    &image.bytes,
                                ) {
                                    st.raw
                                        .events
                                        .push(Event::PasteImage(std::sync::Arc::new(ci)));
                                }
                            }
                            st.window.request_redraw();
                            return;
                        }
                        _ => {}
                    }
                }
                if let Some(key) = map_key(&event.logical_key) {
                    let repeat = event.repeat;
                    st.raw.events.push(Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat,
                        modifiers: st.modifiers,
                    });
                }
                if pressed {
                    if let Some(text) = &event.text {
                        if !text.chars().next().is_some_and(|c| c.is_control()) {
                            st.raw.events.push(Event::Text(text.to_string()));
                        }
                    }
                }
                st.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                st.modifiers = Modifiers {
                    alt: s.alt_key(),
                    ctrl: s.control_key(),
                    shift: s.shift_key(),
                    mac_cmd: false,
                    command: s.control_key(),
                };
                let m = st.modifiers;
                st.raw.events.push(Event::ModifiersChanged(m));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(x * 50.0, y * 50.0),
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32),
                    _ => Vec2::ZERO,
                };
                st.raw.events.push(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: d,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: st.modifiers,
                });
                st.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if render_frame(st) {
                    eframe::App::on_exit(&mut st.app);
                    event_loop.exit();
                }
            }
            WindowEvent::Ime(ime) => {
                // 照 egui-winit 0.37 的映射（`:770-828`）：winit 的 preedit 范围是
                // 字节索引，egui 要字符索引。
                match ime {
                    winit::event::Ime::Preedit(text, range) => {
                        let active_range_chars =
                            range.and_then(|(start, end)| preedit_range(&text, start, end));
                        st.raw.events.push(Event::Ime(egui::ImeEvent::Preedit {
                            text,
                            active_range_chars,
                        }));
                    }
                    winit::event::Ime::Commit(text) => {
                        st.raw.events.push(Event::Ime(egui::ImeEvent::Commit(text)));
                    }
                    winit::event::Ime::Enabled
                    | winit::event::Ime::Disabled
                    | winit::event::Ime::DeleteSurrounding { .. } => {}
                    _ => {}
                }
                st.window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &(dyn ActiveEventLoop + 'static)) {
        if let Some(st) = self.state.as_mut() {
            sync_subwindows(st, event_loop);
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

fn render_frame(st: &mut State) -> bool {
    st.raw.time = Some(st.start.elapsed().as_secs_f64());
    // 从 OS 轮询窗口状态：标题栏最大化/还原不会经过 egui 的 ViewportCommand。
    // 全屏优先（全屏时 is_maximized 可能为 true，但保存语义是二者独立）。
    let full = st.window.fullscreen().is_some();
    st.app.window_fullscreen = full;
    st.app.window_maximized = !full && st.window.is_maximized();
    // 主键按住时，Windows 可能把整段移动合并到一帧；补一个当前指针位置，
    // 并在本帧结束后继续要帧，让 egui 的 drag 状态有机会推进。
    if st.pointer_down {
        if let Some(p) = st.last_pointer {
            if st.last_move_sent != Some(p) {
                st.raw.events.push(Event::PointerMoved(p));
                st.last_move_sent = Some(p);
            }
        }
    }
    // 笔尾橡皮端：按下时临时切到橡皮，抬起后恢复用户原来的 E 开关。
    st.app.note_input_device();
    let tablet_eraser = st.app.tablet.is_eraser();
    st.app.apply_tablet_eraser(tablet_eraser);
    let output = st.ctx.run_ui(st.raw.take(), |ui| {
        st.app.ui_root(ui);
    });
    // 光标：egui 每帧给出图标；None 表示隐藏系统光标（画布自绘圆环）。
    let cursor = output.platform_output.cursor_icon;
    if cursor == egui::CursorIcon::None {
        st.window.set_cursor_visible(false);
        // 同子窗口：隐藏前把“已选光标”复位，避免从滑条/分隔条移到画布后
        // 再移出时恢复成上一次的左右双箭头。
        if st.last_cursor != Some(cursor) {
            st.window
                .set_cursor(WinitCursor::Icon(map_cursor(egui::CursorIcon::Default)));
            st.last_cursor = Some(cursor);
        }
    } else {
        st.window.set_cursor_visible(true);
        if st.last_cursor != Some(cursor) {
            st.window.set_cursor(WinitCursor::Icon(map_cursor(cursor)));
            st.last_cursor = Some(cursor);
        }
    }
    // 标题跟随当前文件/兼容模式（`drawwindow.py:274-289`）。
    let title = st.app.window_title();
    if title != st.last_title {
        st.window.set_title(&title);
        st.last_title = title;
    }
    // egui 的视口命令（菜单 Quit / Fullscreen / Maximized 等）。
    for viewport in output.viewport_output.values() {
        for cmd in &viewport.commands {
            match cmd {
                egui::ViewportCommand::Close => {
                    let now = st.start.elapsed().as_secs_f64();
                    st.app.request_quit(now);
                    st.window.request_redraw();
                }
                egui::ViewportCommand::Fullscreen(on) => {
                    st.window.set_fullscreen(if *on {
                        Some(winit::monitor::Fullscreen::Borderless(None))
                    } else {
                        None
                    });
                    st.app.window_fullscreen = *on;
                }
                egui::ViewportCommand::Maximized(on) => {
                    st.window.set_maximized(*on);
                    st.app.window_maximized = *on;
                }
                egui::ViewportCommand::Title(title) => st.window.set_title(title),
                egui::ViewportCommand::InnerSize(size) => {
                    let _ = st.window.request_surface_size(winit::dpi::Size::Physical(
                        winit::dpi::PhysicalSize::new(
                            size.x.max(1.0) as u32,
                            size.y.max(1.0) as u32,
                        ),
                    ));
                }
                _ => {}
            }
        }
    }
    // IME 只在文本框获得焦点时启用。全局启用会把单键快捷键（k/t/…）
    // 送进输入法，画到候选窗里。
    let ime_wanted = output.platform_output.ime.is_some();
    if ime_wanted != st.ime_enabled {
        if ime_wanted {
            let rect = output.platform_output.ime.map(|i| i.cursor_rect).unwrap_or(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0)),
            );
            let data = ImeRequestData::default().with_cursor_area(
                LogicalPosition::new(rect.min.x as f64, rect.min.y as f64).into(),
                LogicalSize::new(rect.width().max(1.0) as f64, rect.height().max(1.0) as f64)
                    .into(),
            );
            if let Some(req) =
                ImeEnableRequest::new(ImeCapabilities::new().with_cursor_area(), data)
            {
                let _ = st.window.request_ime_update(ImeRequest::Enable(req));
                st.ime_enabled = true;
            }
        } else {
            let _ = st.window.request_ime_update(ImeRequest::Disable);
            st.ime_enabled = false;
        }
    }
    // 候选框跟随文本框光标（egui 的 IMEOutput.cursor_rect）。
    if st.ime_enabled {
        if let Some(ime) = &output.platform_output.ime {
            let p = ime.cursor_rect.min;
            let s = ime.cursor_rect.size();
            let data = ImeRequestData::default().with_cursor_area(
                LogicalPosition::new(p.x as f64, p.y as f64).into(),
                LogicalSize::new(s.x.max(1.0) as f64, s.y.max(1.0) as f64).into(),
            );
            let _ = st.window.request_ime_update(ImeRequest::Update(data));
        }
    }
    // 把 egui 的复制命令写进系统剪贴板（Cut/Copy 的事件路径）。
    for cmd in &output.platform_output.commands {
        match cmd {
            egui::OutputCommand::CopyText(text) => {
                if let Some(cb) = st.clipboard.as_mut() {
                    let _ = cb.set_text(text.clone());
                }
            }
            egui::OutputCommand::CopyImage(image) => {
                if let (Some(cb), Some(data)) =
                    (st.clipboard.as_mut(), color_image_to_clipboard(image))
                {
                    let _ = cb.set_image(data);
                }
            }
            _ => {}
        }
    }
    // 面板的自动备份需要 1Hz 帧；egui 的 repaint_delay 已经包含它。
    let delay = output
        .viewport_output
        .values()
        .map(|v| v.repaint_delay)
        .min()
        .unwrap_or(Duration::from_secs(1));
    if delay.is_zero() {
        st.window.request_redraw();
    } else {
        let window = st.window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay.min(Duration::from_secs(1)));
            window.request_redraw();
        });
    }

    let jobs = st.ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [st.config.width, st.config.height],
        pixels_per_point: UI_SCALE,
    };
    let mut textures = output.textures_delta;
    for (id, deltas) in &textures.set {
        for delta in deltas {
            st.renderer
                .update_texture(&st.device, &st.queue, *id, delta);
        }
    }
    let frame = match st.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) => f,
        wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        other => {
            eprintln!("surface: {other:?}");
            st.surface.configure(&st.device, &st.config);
            textures.clear();
            return false;
        }
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = st
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pigment-shell"),
        });
    // 画布是独立的 wgpu pass：先清目标并按 atlas 批次绘制 tile，
    // egui 随后以 LoadOp::Load 叠上菜单/面板/画布覆盖层。
    {
        let device = st.device.clone();
        let queue = st.queue.clone();
        st.app.render_canvas(
            &device,
            &queue,
            &mut encoder,
            &view,
            [st.config.width, st.config.height],
        );
    }
    let cmd = st
        .renderer
        .update_buffers(&st.device, &st.queue, &mut encoder, &jobs, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut pass = pass.forget_lifetime();
        st.renderer.render(&mut pass, &jobs, &screen);
    }
    st.queue
        .submit(cmd.into_iter().chain(std::iter::once(encoder.finish())));
    st.queue.present(frame);
    // 按住期间必须继续产帧：egui 的 drag_started/dragged 分帧推进，
    // 只靠 PointerMoved 事件在 Windows 上会被合并成一帧，表现为只落一个 dab。
    if st.pointer_down {
        st.window.request_redraw();
    }
    for id in &textures.free {
        st.renderer.free_texture(id);
    }
    textures.clear();
    st.app.quit_requested
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_icons_map_to_winit_names() {
        use winit::cursor::CursorIcon as W;
        assert_eq!(map_cursor(egui::CursorIcon::PointingHand), W::Pointer);
        assert_eq!(map_cursor(egui::CursorIcon::Crosshair), W::Crosshair);
        assert_eq!(map_cursor(egui::CursorIcon::Grab), W::Grab);
        assert_eq!(map_cursor(egui::CursorIcon::ResizeHorizontal), W::EwResize);
        assert_eq!(map_cursor(egui::CursorIcon::ResizeNeSw), W::NeswResize);
        assert_eq!(map_cursor(egui::CursorIcon::None), W::Default);
    }

    #[test]
    fn shell_maps_drawing_shortcut_keys() {
        for (c, key) in [
            ("[", egui::Key::OpenBracket),
            ("]", egui::Key::CloseBracket),
            ("e", egui::Key::E),
            ("p", egui::Key::P),
            ("-", egui::Key::Minus),
            ("=", egui::Key::Equals),
        ] {
            assert_eq!(
                map_key(&WinitKey::Character(c.into())),
                Some(key),
                "key {c:?}"
            );
        }
        assert_eq!(
            map_key(&WinitKey::Named(NamedKey::Escape)),
            Some(egui::Key::Escape)
        );
    }

    #[test]
    fn clipboard_image_roundtrips_rgba() {
        let img = egui::ColorImage::new(
            [2, 1],
            vec![
                egui::Color32::from_rgba_unmultiplied(10, 20, 30, 255),
                egui::Color32::from_rgba_unmultiplied(200, 100, 50, 255),
            ],
        );
        let data = color_image_to_clipboard(&img).unwrap();
        assert_eq!((data.width, data.height), (2, 1));
        let back = clipboard_to_color_image(data.width, data.height, &data.bytes).unwrap();
        assert_eq!(back.size, [2, 1]);
        assert_eq!(back.pixels[0], img.pixels[0]);
        assert_eq!(back.pixels[1], img.pixels[1]);
        // 尺寸不匹配时拒绝
        assert!(clipboard_to_color_image(2, 2, &data.bytes).is_none());
    }

    #[test]
    fn clipboard_image_preserves_unmultiplied_semi_transparency() {
        let img = egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::from_rgba_unmultiplied(200, 100, 50, 128)],
        );
        let data = color_image_to_clipboard(&img).unwrap();
        let [r, g, b, a] = [data.bytes[0], data.bytes[1], data.bytes[2], data.bytes[3]];
        assert!((r as i16 - 200).abs() <= 1, "r={r}");
        assert!((g as i16 - 100).abs() <= 1, "g={g}");
        assert!((b as i16 - 50).abs() <= 1, "b={b}");
        assert_eq!(a, 128, "alpha 不能被预乘吃掉");
    }

    #[test]
    fn clipboard_shortcuts_and_newlines_are_normalized() {
        assert_eq!(
            clipboard_shortcut(&WinitKey::Character("C".into())),
            Some('c')
        );
        assert_eq!(
            clipboard_shortcut(&WinitKey::Character("v".into())),
            Some('v')
        );
        assert_eq!(clipboard_shortcut(&WinitKey::Named(NamedKey::Enter)), None);
        assert_eq!(normalize_paste("a\r\nb"), "a\nb");
    }

    #[test]
    fn preedit_range_converts_utf8_bytes_to_chars() {
        // "你a"：你 3 字节，a 1 字节
        assert_eq!(preedit_range("你a", 0, 3), Some(0..1));
        assert_eq!(preedit_range("你a", 3, 4), Some(1..2));
        // 非法边界回退 None（winit 偶尔会给越界范围）
        assert_eq!(preedit_range("你a", 2, 4), None);
    }
}
