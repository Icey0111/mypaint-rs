//! 数位板输入探针（临时验证工具，不属于应用）。
//!
//! 用 winit 0.31 的 `PointerSource::TabletTool` 打印压力/倾斜/橡皮端，
//! 用来确认当前机器 + 驱动 + winit 后端能拿到笔数据。

use winit::application::ApplicationHandler;
use winit::event::{ElementState, PointerSource, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes};

struct Probe {
    window: Option<Box<dyn Window>>,
    last_print: f64,
}

impl Probe {
    fn new() -> Self {
        Self {
            window: None,
            last_print: -1.0,
        }
    }
}

impl ApplicationHandler for Probe {
    fn can_create_surfaces(&mut self, event_loop: &(dyn ActiveEventLoop + 'static)) {
        if self.window.is_none() {
            let attrs = WindowAttributes::default().with_title("tablet probe — draw with pen");
            match event_loop.create_window(attrs) {
                Ok(w) => {
                    println!("window created; draw with the pen, close to quit");
                    self.window = Some(w);
                }
                Err(e) => {
                    eprintln!("create_window failed: {e}");
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &(dyn ActiveEventLoop + 'static),
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::PointerMoved {
                position,
                source: PointerSource::TabletTool { kind, data },
                ..
            } => {
                let force = data.force.map(|f| f.normalized(None)).unwrap_or(-1.0);
                let tilt = data
                    .tilt
                    .map(|t| (f64::from(t.x), f64::from(t.y)))
                    .unwrap_or((0.0, 0.0));
                println!(
                    "tablet move  kind={kind:?} pos=({:.1},{:.1}) pressure={force:.3} tilt=({:.1},{:.1})",
                    position.x, position.y, tilt.0, tilt.1
                );
            }
            WindowEvent::PointerButton {
                state,
                position,
                button: winit::event::ButtonSource::TabletTool { kind, data, .. },
                ..
            } => {
                let force = data.force.map(|f| f.normalized(None)).unwrap_or(-1.0);
                println!(
                    "tablet button {state:?} kind={kind:?} pos=({:.1},{:.1}) pressure={force:.3}",
                    position.x, position.y
                );
                if state == ElementState::Pressed {
                    self.last_print = 0.0;
                }
            }
            WindowEvent::PointerMoved { position, .. } => {
                // 鼠标/触摸：低频打印，避免刷屏
                let t = self.last_print;
                if t < 0.0 {
                    println!("mouse move pos=({:.1},{:.1})", position.x, position.y);
                    self.last_print = 0.0;
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &(dyn ActiveEventLoop + 'static)) {
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.run_app(Probe::new())?;
    Ok(())
}
