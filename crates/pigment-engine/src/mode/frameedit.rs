//! 编辑画框 —— `gui/framewindow.py` 的 `FrameEditMode`（核心拖拽/创建）。
//!
//! 已移植：边/角命中（`EDGE_SENSITIVITY=10` 显示像素）、九种拖拽效果
//! （`DRAG_EFFECTS`）、`_MIN_FRAME_SIZE=1`、左右/上下边的补偿钳位、
//! 空白处点击创建画框、`UpdateFrame` 撤销命令。画框按钮（删除）与
//! 选项面板/光标形状未移植。

use crate::command::UpdateFrame;
use super::{Behavior, InteractionMode, Key, ModeContext, PointerEvent};

const LEFT: u8 = 1;
const TOP: u8 = 2;
const RIGHT: u8 = 4;
const BOTTOM: u8 = 8;
const INSIDE: u8 = 16;

const EDGE_SENSITIVITY: f32 = 10.0;
const MIN_FRAME_SIZE: i32 = 1;
/// 新建画框的默认边长（源码取视口最短对角线的 max(100, ·)，引擎层没有
/// 视口，取 256 作为稳定近似）。
const NEW_FRAME_SIZE: i32 = 256;

/// 画框编辑模式。
pub struct FrameEditMode {
    zone: u8,
    orig: (i32, i32, i32, i32),
    start: (f32, f32),
    dragging: bool,
    create_at: Option<(f32, f32)>,
}

impl Default for FrameEditMode {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameEditMode {
    pub fn new() -> Self {
        Self {
            zone: 0,
            orig: (0, 0, 0, 0),
            start: (0.0, 0.0),
            dragging: false,
            create_at: None,
        }
    }

    fn zone_at(&self, ctx: &ModeContext<'_>, ev: &PointerEvent) -> u8 {
        let (fx, fy, fw, fh) = ctx.doc.get_frame();
        if !ctx.doc.get_frame_enabled() {
            return 0; // CREATE：任意位置
        }
        let max_d = EDGE_SENSITIVITY / ev.viewzoom.max(1e-6);
        let (x, y) = (ev.x, ev.y);
        let (x2, y2) = ((fx + fw) as f32, (fy + fh) as f32);
        if x < fx as f32 - max_d
            || x > x2 + max_d
            || y < fy as f32 - max_d
            || y > y2 + max_d
        {
            return 0; // OUTSIDE
        }
        let mut z = 0;
        if (x2 - x).abs() <= max_d {
            z |= RIGHT;
        } else if (x - fx as f32).abs() <= max_d {
            z |= LEFT;
        }
        if (y2 - y).abs() <= max_d {
            z |= BOTTOM;
        } else if (y - fy as f32).abs() <= max_d {
            z |= TOP;
        }
        if z == 0 {
            z = INSIDE;
        }
        z
    }

    /// `DRAG_EFFECTS`：返回 `(mdx, mdy, mdw, mdh)`。
    fn drag_effect(zone: u8) -> Option<(i32, i32, i32, i32)> {
        Some(match zone {
            z if z == LEFT | TOP => (1, 1, -1, -1),
            z if z == LEFT => (1, 0, -1, 0),
            z if z == LEFT | BOTTOM => (1, 0, -1, 1),
            z if z == TOP => (0, 1, 0, -1),
            z if z == INSIDE => (1, 1, 0, 0),
            z if z == BOTTOM => (0, 0, 0, 1),
            z if z == RIGHT | TOP => (0, 1, 1, -1),
            z if z == RIGHT => (0, 0, 1, 0),
            z if z == RIGHT | BOTTOM => (0, 0, 1, 1),
            _ => return None,
        })
    }

    fn apply(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) {
        let Some((mdx, mdy, mdw, mdh)) = Self::drag_effect(self.zone) else {
            return;
        };
        let (mx, my) = (ev.x - self.start.0, ev.y - self.start.1);
        let (fdx, fdy) = (mx.round() as i32, my.round() as i32);
        let (x0, y0, w0, h0) = self.orig;
        let (mut x, mut y, mut w, mut h) = self.orig;
        if mdx != 0 {
            x += mdx * fdx;
            if mdw == -1 {
                x = x.min(x0 + w0 - MIN_FRAME_SIZE);
            }
        }
        if mdy != 0 {
            y += mdy * fdy;
            if mdh == -1 {
                y = y.min(y0 + h0 - MIN_FRAME_SIZE);
            }
        }
        if mdw != 0 {
            w += mdw * fdx;
            w = w.max(MIN_FRAME_SIZE);
        }
        if mdh != 0 {
            h += mdh * fdy;
            h = h.max(MIN_FRAME_SIZE);
        }
        ctx.doc.set_frame(x, y, w, h);
    }

    fn commit_drag(&mut self, ctx: &mut ModeContext<'_>) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        let before = self.orig;
        let after = ctx.doc.get_frame();
        if before != after {
            ctx.doc.do_command(Box::new(UpdateFrame::new(before, after)));
        }
    }
}

impl InteractionMode for FrameEditMode {
    fn name(&self) -> &'static str {
        "Edit Frame"
    }

    fn usage(&self) -> &str {
        "Adjust the document frame"
    }

    fn action_name(&self) -> Option<&'static str> {
        Some("FrameEditMode")
    }

    fn pointer_behavior(&self) -> Behavior {
        Behavior::EDIT_OBJECTS
    }

    fn enter(&mut self, ctx: &mut ModeContext<'_>) {
        // 源码：已有尺寸但未启用时，进入即显示画框
        let (_, _, w, h) = ctx.doc.get_frame();
        if !ctx.doc.get_frame_enabled() && w > 0 && h > 0 {
            ctx.doc.set_frame_enabled(true);
        }
    }

    fn leave(&mut self, ctx: &mut ModeContext<'_>) {
        self.commit_drag(ctx);
    }

    fn checkpoint(&mut self, ctx: &mut ModeContext<'_>) {
        self.commit_drag(ctx);
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 {
            return false;
        }
        if !ctx.doc.get_frame_enabled() {
            self.create_at = Some((ev.x, ev.y));
            return true;
        }
        let zone = self.zone_at(ctx, ev);
        if zone == 0 {
            return false; // 画框外：交给下层/平移
        }
        self.zone = zone;
        self.orig = ctx.doc.get_frame();
        self.start = (ev.x, ev.y);
        self.dragging = true;
        true
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.dragging {
            return false;
        }
        self.apply(ctx, ev);
        true
    }

    fn button_release(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 {
            return false;
        }
        if let Some((cx, cy)) = self.create_at.take() {
            let x = cx.round() as i32 - NEW_FRAME_SIZE / 2;
            let y = cy.round() as i32 - NEW_FRAME_SIZE / 2;
            let before = ctx.doc.get_frame();
            ctx.doc.set_frame(x, y, NEW_FRAME_SIZE, NEW_FRAME_SIZE);
            ctx.doc.set_frame_enabled(true);
            ctx.doc
                .do_command(Box::new(UpdateFrame::new(before, (x, y, NEW_FRAME_SIZE, NEW_FRAME_SIZE))));
            return true;
        }
        if self.dragging {
            self.apply(ctx, ev);
            self.commit_drag(ctx);
            return true;
        }
        false
    }

    fn key_press(&mut self, ctx: &mut ModeContext<'_>, key: Key) -> bool {
        if key == Key::Escape && self.dragging {
            ctx.doc.set_frame(self.orig.0, self.orig.1, self.orig.2, self.orig.3);
            self.dragging = false;
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
    fn drag_right_edge_resizes_and_undo_restores() {
        let mut doc = Document::new();
        doc.set_frame(0, 0, 100, 100);
        doc.set_frame_enabled(true);
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut picked = None;
        let mut mode = FrameEditMode::new();
        {
            let mut c = ctx(&mut doc, &brush, &mut state, &mut picked);
            mode.enter(&mut c);
            assert!(mode.button_press(&mut c, &PointerEvent::button(100.0, 50.0, 0.0, 1)));
            mode.motion(&mut c, &PointerEvent::motion(150.0, 50.0, 16.0));
            mode.button_release(&mut c, &PointerEvent::button(150.0, 50.0, 32.0, 1));
        }
        assert_eq!(doc.get_frame(), (0, 0, 150, 100));
        assert_eq!(doc.command_stack.undo_len(), 1);
        doc.undo();
        assert_eq!(doc.get_frame(), (0, 0, 100, 100));
    }

    #[test]
    fn click_creates_frame() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut picked = None;
        let mut mode = FrameEditMode::new();
        let mut c = ctx(&mut doc, &brush, &mut state, &mut picked);
        assert!(mode.button_press(&mut c, &PointerEvent::button(300.0, 200.0, 0.0, 1)));
        assert!(mode.button_release(&mut c, &PointerEvent::button(300.0, 200.0, 16.0, 1)));
        assert!(doc.get_frame_enabled());
        assert_eq!(doc.get_frame(), (300 - 128, 200 - 128, 256, 256));
    }
}
