//! 数位板输入探针（Plan A 原型，临时工具，不属于应用）。
//!
//! 用 winit 0.31 的 `PointerSource::TabletTool` + egui-wgpu 画一个窗口，
//! 显示压力/倾斜/工具类型。验证“winit 0.31 + egui + wgpu”这条外壳可行，
//! 以及当前机器/驱动能拿到笔数据。

use std::sync::Arc;

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use winit::application::ApplicationHandler;
use winit::event::{ButtonSource, ElementState, MouseScrollDelta, PointerSource, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Default)]
struct Pen {
    kind: String,
    pressure: f32,
    tilt: (f32, f32),
    pos: (f32, f32),
}

struct State {
    window: Arc<dyn Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    pen: Pen,
    raw: RawInput,
    modifiers: Modifiers,
    start: std::time::Instant,
}

struct Probe {
    state: Option<State>,
}

impl Probe {
    fn new() -> Self {
        Self { state: None }
    }
}

fn tilt_to_unit(t: winit::event::TabletToolTilt) -> (f32, f32) {
    // winit 给的是角度（度）；MyPaint 用 -1..1 的归一化倾斜。
    (f32::from(t.x) / 90.0, f32::from(t.y) / 90.0)
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

impl ApplicationHandler for Probe {
    fn can_create_surfaces(&mut self, event_loop: &(dyn ActiveEventLoop + 'static)) {
        if self.state.is_some() {
            return;
        }
        let attrs =
            WindowAttributes::default().with_title("pigment tablet probe (winit 0.31 + egui-wgpu)");
        let window: Arc<dyn Window> = match event_loop.create_window(attrs) {
            Ok(w) => Arc::from(w),
            Err(e) => {
                eprintln!("create_window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let size = window.surface_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = match instance.create_surface(window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("create_surface failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("request_adapter failed: {e}");
                    event_loop.exit();
                    return;
                }
            };
        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("tablet-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })) {
                Ok(dq) => dq,
                Err(e) => {
                    eprintln!("request_device failed: {e}");
                    event_loop.exit();
                    return;
                }
            };
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported");
        config.format = format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        surface.configure(&device, &config);
        let renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
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
            egui_ctx: {
                let c = egui::Context::default();
                c.set_visuals(egui::Visuals::light());
                c
            },
            renderer,
            pen: Pen::default(),
            raw,
            modifiers: Modifiers::default(),
            start: std::time::Instant::now(),
        });
        if let Some(st) = self.state.as_ref() {
            st.window.request_redraw();
        }
        println!("probe window created: draw with the pen");
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
                st.raw.events.push(Event::PointerMoved(p));
                if let PointerSource::TabletTool { kind, data } = source {
                    st.pen.kind = format!("{kind:?}");
                    st.pen.pressure = data.force.map(|f| f.normalized(None) as f32).unwrap_or(0.0);
                    st.pen.tilt = data.tilt.map(tilt_to_unit).unwrap_or((0.0, 0.0));
                    st.pen.pos = (p.x, p.y);
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
                    let pressed = state == ElementState::Pressed;
                    st.raw.events.push(Event::PointerButton {
                        pos: p,
                        button: b,
                        pressed,
                        modifiers: st.modifiers,
                    });
                    if pressed {
                        if let ButtonSource::TabletTool { kind, data, .. } = button {
                            st.pen.kind = format!("{kind:?}");
                            st.pen.pressure =
                                data.force.map(|f| f.normalized(None) as f32).unwrap_or(0.0);
                            st.pen.tilt = data.tilt.map(tilt_to_unit).unwrap_or((0.0, 0.0));
                            st.pen.pos = (p.x, p.y);
                        }
                    }
                }
                st.window.request_redraw();
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
            WindowEvent::RedrawRequested => {
                render_frame(st);
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
    let pen = &st.pen;
    let output = st.egui_ctx.run_ui(st.raw.take(), |ui| {
        ui.heading("pigment tablet probe");
        ui.label("Draw with the pen. Values update from winit 0.31 tablet events.");
        ui.separator();
        ui.monospace(format!("kind:     {}", pen.kind));
        ui.monospace(format!("pressure: {:.3}", pen.pressure));
        ui.monospace(format!("tilt:     ({:.2}, {:.2})", pen.tilt.0, pen.tilt.1));
        ui.monospace(format!("pos:      ({:.0}, {:.0})", pen.pos.0, pen.pos.1));
        if pen.kind.contains("Eraser") {
            ui.colored_label(egui::Color32::YELLOW, "ERASER END");
        }
        ui.separator();
        ui.label(if pen.pressure > 0.0 {
            "tablet data received"
        } else {
            "waiting for pen… (mouse only shows no pressure)"
        });
        // 压力可视化：半径随压力变大，便于肉眼确认笔数据
        if pen.pressure > 0.0 {
            let r = 4.0 + pen.pressure * 36.0;
            ui.painter().circle_filled(
                Pos2::new(pen.pos.0, pen.pos.1),
                r,
                if pen.kind.contains("Eraser") {
                    egui::Color32::from_rgb(220, 60, 60)
                } else {
                    egui::Color32::from_rgb(40, 120, 220)
                },
            );
        }
    });
    let jobs = st
        .egui_ctx
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
            return;
        }
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = st
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tablet-probe"),
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
                        r: 0.10,
                        g: 0.10,
                        b: 0.12,
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
    for id in &textures.free {
        st.renderer.free_texture(id);
    }
    textures.clear();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.run_app(Probe::new())?;
    Ok(())
}
