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
use winit::event::{
    ButtonSource, ElementState, MouseScrollDelta, PointerSource, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::app::PigmentApp;

struct State {
    window: Arc<dyn Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
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
                'a' => K::A, 'b' => K::B, 'c' => K::C, 'd' => K::D,
                'e' => K::E, 'f' => K::F, 'g' => K::G, 'h' => K::H,
                'i' => K::I, 'j' => K::J, 'k' => K::K, 'l' => K::L,
                'm' => K::M, 'n' => K::N, 'o' => K::O, 'p' => K::P,
                'q' => K::Q, 'r' => K::R, 's' => K::S, 't' => K::T,
                'u' => K::U, 'v' => K::V, 'w' => K::W, 'x' => K::X,
                'y' => K::Y, 'z' => K::Z,
                '0' => K::Num0, '1' => K::Num1, '2' => K::Num2,
                '3' => K::Num3, '4' => K::Num4, '5' => K::Num5,
                '6' => K::Num6, '7' => K::Num7, '8' => K::Num8,
                '9' => K::Num9,
                ' ' => K::Space,
                '+' => K::Plus, '-' => K::Minus,
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

/// winit 的 preedit 范围是 UTF-8 字节索引 → egui 的字符索引。
fn preedit_range(text: &str, start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    let start_chars = text.get(..start)?.chars().count();
    let middle_chars = text.get(start..end)?.chars().count();
    Some(start_chars..start_chars + middle_chars)
}

/// 显式装一个 CJK 字体（egui 自带字体不含汉字）。系统字体回退是异步的，
/// 裸外壳不能依赖它第一帧就绪。
fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        "C:/Windows/Fonts/NotoSansSC-VF.ttf",
        "C:/Windows/Fonts/Deng.ttf",
        "C:/Windows/Fonts/simhei.ttf",
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}
impl Shell {
    fn make_state(&mut self, event_loop: &(dyn ActiveEventLoop + 'static)) {
        let attrs = WindowAttributes::default()
            .with_title("pigment (winit 0.31 shell)")
            .with_surface_size(winit::dpi::PhysicalSize::new(1280, 800));
        let window: Arc<dyn Window> = match event_loop.create_window(attrs) {
            Ok(w) => Arc::from(w),
            Err(e) => {
                eprintln!("create_window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let size = window.surface_size();
        // winit 0.31 的新接口是 `request_ime_update`；这里沿用旧入口，
        // 它内部会转成同一个 ImeRequest::Enable。
        #[allow(deprecated)]
        window.set_ime_allowed(true);
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_without_display_handle(),
        );
        let surface = match instance.create_surface(window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("create_surface failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let adapter = match pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            },
        )) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("request_adapter failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let (device, queue) = match pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("pigment-shell"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            },
        )) {
            Ok(dq) => dq,
            Err(e) => {
                eprintln!("request_device failed: {e}");
                event_loop.exit();
                return;
            }
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
        config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        surface.configure(&device, &config);

        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        // 系统字体回退（CJK 等）；eframe 默认做了这件事，裸外壳要自己加。
        ctx.add_font_provider(std::sync::Arc::new(
            egui_system_font_provider::SystemFontProvider::new(),
        ));
        install_cjk_font(&ctx);
        let renderer = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions::default(),
        );
        let app = PigmentApp::new_with_renderer(&device, format);
        let mut raw = RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(
                config.width as f32 / window.scale_factor() as f32,
                config.height as f32 / window.scale_factor() as f32,
            ),
        ));
        self.state = Some(State {
            window,
            surface,
            config,
            device,
            queue,
            ctx,
            renderer,
            app,
            raw,
            modifiers: Modifiers::default(),
            start: Instant::now(),
            pointer_down: false,
            last_pointer: None,
            last_move_sent: None,
        });
        if let Some(st) = self.state.as_ref() {
            st.window.request_redraw();
        }
    }
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
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(st) = self.state.as_mut() else {
            return;
        };
        let scale = st.window.scale_factor() as f32;
        let pos = |p: winit::dpi::PhysicalPosition<f64>| {
            Pos2::new(p.x as f32 / scale, p.y as f32 / scale)
        };
        match event {
            WindowEvent::CloseRequested => {
                eframe::App::on_exit(&mut st.app);
                event_loop.exit();
            }
            WindowEvent::SurfaceResized(size) => {
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
                position, source, ..
            } => {
                let p = pos(position);
                st.last_pointer = Some(p);
                st.last_move_sent = Some(p);
                st.raw.events.push(Event::PointerMoved(p));
                if let PointerSource::TabletTool { kind, data } = source {
                    let pressure =
                        data.force.map(|f| f.normalized(None) as f32).unwrap_or(1.0);
                    let tilt = data
                        .tilt
                        .map(|t| {
                            (f32::from(t.x) / 90.0, f32::from(t.y) / 90.0)
                        })
                        .unwrap_or((0.0, 0.0));
                    st.app.tablet.set(
                        pressure,
                        tilt,
                        format!("{kind:?}").contains("Eraser"),
                    );
                }
                st.window.request_redraw();
            }
            WindowEvent::PointerButton {
                state,
                position,
                button,
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
                        let pressure =
                            data.force.map(|f| f.normalized(None) as f32).unwrap_or(1.0);
                        let tilt = data
                            .tilt
                            .map(|t| {
                                (f32::from(t.x) / 90.0, f32::from(t.y) / 90.0)
                            })
                            .unwrap_or((0.0, 0.0));
                        st.app.tablet.set(
                            pressure,
                            tilt,
                            format!("{kind:?}").contains("Eraser"),
                        );
                    }
                    if !pressed {
                        st.app.tablet.clear();
                    }
                }
                st.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
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
                    MouseScrollDelta::PixelDelta(p) => {
                        Vec2::new(p.x as f32, p.y as f32)
                    }
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
                render_frame(st);
            }
            WindowEvent::Ime(ime) => {
                // 照 egui-winit 0.37 的映射（`:770-828`）：winit 的 preedit 范围是
                // 字节索引，egui 要字符索引。
                match ime {
                    winit::event::Ime::Preedit(text, range) => {
                        let active_range_chars = range.and_then(|(start, end)| {
                            preedit_range(&text, start, end)
                        });
                        st.raw
                            .events
                            .push(Event::Ime(egui::ImeEvent::Preedit {
                                text,
                                active_range_chars,
                            }));
                    }
                    winit::event::Ime::Commit(text) => {
                        st.raw
                            .events
                            .push(Event::Ime(egui::ImeEvent::Commit(text)));
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
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

fn render_frame(st: &mut State) {
    st.raw.time = Some(st.start.elapsed().as_secs_f64());
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
    let tablet_eraser = st.app.tablet.is_eraser();
    st.app.apply_tablet_eraser(tablet_eraser);
    let output = st.ctx.run_ui(st.raw.take(), |ui| {
        st.app.ui_root(ui);
    });
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

    let jobs = st
        .ctx
        .tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [st.config.width, st.config.height],
        pixels_per_point: st.window.scale_factor() as f32,
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
            return;
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
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.08,
                        g: 0.08,
                        b: 0.09,
                        a: 1.0,
                    }),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preedit_range_converts_utf8_bytes_to_chars() {
        // "你a"：你 3 字节，a 1 字节
        assert_eq!(preedit_range("你a", 0, 3), Some(0..1));
        assert_eq!(preedit_range("你a", 3, 4), Some(1..2));
        // 非法边界回退 None（winit 偶尔会给越界范围）
        assert_eq!(preedit_range("你a", 2, 4), None);
    }
}