//! 编辑对称轴 —— `gui/symmetry.py` 的 `SymmetryEditMode`（核心拖拽）。
//!
//! 已移植：关闭时点击建立中心并开启、拖中心、拖轴旋转。
//! 未移植：画布上的“关闭对称”按钮、轴端点手柄、选项面板、光标族。
//! 对称状态本身没有撤销命令（源码 `set_symmetry_state` 也不入栈）。

use super::{Behavior, InteractionMode, Key, ModeContext, PointerEvent};

/// 拖动对象。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Drag {
    Center,
    Axis,
}

/// 对称轴编辑模式。
pub struct SymmetryEditMode {
    drag: Option<Drag>,
    start: (f32, f32),
}

impl Default for SymmetryEditMode {
    fn default() -> Self {
        Self::new()
    }
}

impl SymmetryEditMode {
    pub fn new() -> Self {
        Self {
            drag: None,
            start: (0.0, 0.0),
        }
    }

    /// 点到轴所在直线的距离（模型坐标）。
    fn axis_distance(ctx: &ModeContext<'_>, x: f32, y: f32) -> f32 {
        let (cx, cy) = ctx.doc.symmetry_center();
        let a = ctx.doc.symmetry_angle().to_radians();
        let (s, c) = a.sin_cos();
        (-(s) * (x - cx as f32) + c * (y - cy as f32)).abs()
    }
}

impl InteractionMode for SymmetryEditMode {
    fn name(&self) -> &'static str {
        "Edit Symmetry Axis"
    }

    fn usage(&self) -> &str {
        "Adjust the painting symmetry axis"
    }

    fn action_name(&self) -> Option<&'static str> {
        Some("SymmetryEditMode")
    }

    fn pointer_behavior(&self) -> Behavior {
        Behavior::EDIT_OBJECTS
    }

    fn enter(&mut self, _ctx: &mut ModeContext<'_>) {
        self.drag = None;
    }

    fn leave(&mut self, _ctx: &mut ModeContext<'_>) {
        self.drag = None;
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 {
            return false;
        }
        if !ctx.doc.symmetry_active() {
            // 关闭状态：点击建立中心并开启
            ctx.doc.set_symmetry_state(
                Some(true),
                Some((ev.x.round() as i32, ev.y.round() as i32)),
                None,
                None,
                None,
            );
            ctx.doc.set_symmetry_unset(false);
            return true;
        }
        let hit = 16.0 / ev.viewzoom.max(1e-6);
        let (cx, cy) = ctx.doc.symmetry_center();
        let center = ((ev.x - cx as f32).powi(2) + (ev.y - cy as f32).powi(2)).sqrt();
        if center <= hit {
            self.drag = Some(Drag::Center);
            self.start = (ev.x, ev.y);
            return true;
        }
        if Self::axis_distance(ctx, ev.x, ev.y) <= hit {
            self.drag = Some(Drag::Axis);
            self.start = (ev.x, ev.y);
            return true;
        }
        false
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        match self.drag {
            Some(Drag::Center) => {
                ctx.doc.set_symmetry_state(
                    None,
                    Some((ev.x.round() as i32, ev.y.round() as i32)),
                    None,
                    None,
                    None,
                );
                true
            }
            Some(Drag::Axis) => {
                let (cx, cy) = ctx.doc.symmetry_center();
                let angle = (ev.y - cy as f32).atan2(ev.x - cx as f32).to_degrees();
                ctx.doc
                    .set_symmetry_state(None, None, None, None, Some(angle));
                true
            }
            None => false,
        }
    }

    fn button_release(&mut self, _ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 {
            return false;
        }
        let was = self.drag.is_some();
        self.drag = None;
        was
    }

    fn key_press(&mut self, _ctx: &mut ModeContext<'_>, key: Key) -> bool {
        if key == Key::Escape && self.drag.is_some() {
            self.drag = None;
            return true;
        }
        false
    }

    fn process_pending(&mut self, _ctx: &mut ModeContext<'_>) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    fn ctx<'a>(
        doc: &'a mut Document,
        brush: &'a crate::hokusai::Brush,
        state: &'a mut crate::hokusai::BrushState,
        picked: &'a mut Option<[f32; 3]>,
    ) -> ModeContext<'a> {
        ModeContext {
            doc,
            brush,
            brush_name: "test",
            brush_state: state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            color: [0.0, 0.0, 0.0],
            picked_color: Some(picked),
        }
    }

    #[test]
    fn click_creates_center_then_drag_and_rotate() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut picked = None;
        let mut mode = SymmetryEditMode::new();
        {
            let mut c = ctx(&mut doc, &brush, &mut state, &mut picked);
            assert!(mode.button_press(&mut c, &PointerEvent::button(100.0, 100.0, 0.0, 1)));
            mode.button_release(&mut c, &PointerEvent::button(100.0, 100.0, 16.0, 1));
        }
        assert!(doc.symmetry_active());
        assert_eq!(doc.symmetry_center(), (100, 100));

        // 拖中心
        {
            let mut c = ctx(&mut doc, &brush, &mut state, &mut picked);
            assert!(mode.button_press(&mut c, &PointerEvent::button(100.0, 100.0, 32.0, 1)));
            mode.motion(&mut c, &PointerEvent::motion(120.0, 90.0, 48.0));
            mode.button_release(&mut c, &PointerEvent::button(120.0, 90.0, 64.0, 1));
        }
        assert_eq!(doc.symmetry_center(), (120, 90));

        // 轴上的点 (170,90) 拖到 (120,140) → 90°
        {
            let mut c = ctx(&mut doc, &brush, &mut state, &mut picked);
            assert!(mode.button_press(&mut c, &PointerEvent::button(170.0, 90.0, 80.0, 1)));
            mode.motion(&mut c, &PointerEvent::motion(120.0, 140.0, 96.0));
            mode.button_release(&mut c, &PointerEvent::button(120.0, 140.0, 112.0, 1));
        }
        assert!((doc.symmetry_angle() - 90.0).abs() < 1e-4, "angle={}", doc.symmetry_angle());
    }
}
