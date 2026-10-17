//! 移动当前图层 —— `gui/layermanip.py` 的 `LayerMoveMode`。
//!
//! 源码用 `MoveLayer` 命令在拖拽中增量搬 tile；本实现每次 motion 从拖拽
//! 前快照重建表面再整体平移，落笔结束时提交一条 `MoveLayer` 命令
//! （前后快照 + strokemap），撤销/重做都走快照。
//!
//! 记账：
//! - 源码按 tile 分块 idle 处理以保持响应；这里同步平移（小图足够快，
//!   大图可能有一帧卡顿）；
//! - 源码的“不可移动”光标（隐藏/锁定层）未画，只拒绝操作。

use pigment_surface::{StrokeShape, SurfaceSnapshot};

use crate::command::MoveLayer;
use super::{Behavior, InteractionMode, Key, ModeContext, PointerEvent};

/// 交互式移动当前图层。
pub struct LayerMoveMode {
    dragging: bool,
    start: (f32, f32),
    layer_id: Option<u64>,
    before: Option<SurfaceSnapshot>,
    before_strokes: Vec<StrokeShape>,
    last: (i32, i32),
    moved: bool,
}

impl Default for LayerMoveMode {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerMoveMode {
    pub fn new() -> Self {
        Self {
            dragging: false,
            start: (0.0, 0.0),
            layer_id: None,
            before: None,
            before_strokes: Vec::new(),
            last: (0, 0),
            moved: false,
        }
    }

    /// 把表面恢复到拖拽前，再按当前整数偏移平移。
    fn apply(&mut self, ctx: &mut ModeContext<'_>, dx: i32, dy: i32) {
        let Some(id) = self.layer_id else { return };
        let Some(before) = &self.before else { return };
        let Some(surface) = ctx.doc.layers_mut().surface_by_id(id) else {
            return;
        };
        surface.load_snapshot(before);
        surface.replace_strokes(self.before_strokes.clone());
        if dx != 0 || dy != 0 {
            surface.translate(dx, dy);
        }
        self.last = (dx, dy);
        self.moved = dx != 0 || dy != 0;
    }

    fn finish(&mut self, ctx: &mut ModeContext<'_>) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        let (dx, dy) = self.last;
        if !self.moved || (dx == 0 && dy == 0) {
            self.before = None;
            return;
        }
        let Some(id) = self.layer_id.take() else { return };
        let Some(before) = self.before.take() else { return };
        let Some(surface) = ctx.doc.layers_mut().surface_by_id(id) else {
            return;
        };
        let after = surface.save_snapshot();
        let after_strokes = surface.strokes().to_vec();
        ctx.doc.do_command(Box::new(MoveLayer::new(
            id,
            before,
            after,
            self.before_strokes.clone(),
            after_strokes,
        )));
        self.moved = false;
    }

    fn cancel(&mut self, ctx: &mut ModeContext<'_>) {
        if let (Some(id), Some(before)) = (self.layer_id.take(), self.before.take()) {
            if let Some(surface) = ctx.doc.layers_mut().surface_by_id(id) {
                surface.load_snapshot(&before);
                surface.replace_strokes(self.before_strokes.clone());
            }
        }
        self.dragging = false;
        self.moved = false;
        self.last = (0, 0);
    }
}

impl InteractionMode for LayerMoveMode {
    fn name(&self) -> &'static str {
        "Move Layer"
    }

    fn usage(&self) -> &str {
        "Move the current layer"
    }

    fn action_name(&self) -> Option<&'static str> {
        Some("LayerMoveMode")
    }

    fn pointer_behavior(&self) -> Behavior {
        Behavior::CHANGE_VIEW
    }

    fn scroll_behavior(&self) -> Behavior {
        Behavior::CHANGE_VIEW
    }

    fn enter(&mut self, _ctx: &mut ModeContext<'_>) {
        self.dragging = false;
        self.before = None;
        self.layer_id = None;
        self.moved = false;
    }

    fn leave(&mut self, ctx: &mut ModeContext<'_>) {
        self.finish(ctx);
    }

    fn checkpoint(&mut self, ctx: &mut ModeContext<'_>) {
        self.finish(ctx);
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 || !ctx.current_is_paintable() {
            return false;
        }
        let Some(surface) = ctx.doc.layers_mut().paintable_surface_mut() else {
            return false;
        };
        self.before = Some(surface.save_snapshot());
        self.before_strokes = surface.strokes().to_vec();
        self.layer_id = Some(ctx.doc.layers().current_id());
        self.start = (ev.x, ev.y);
        self.last = (0, 0);
        self.moved = false;
        self.dragging = true;
        true
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.dragging {
            return false;
        }
        let dx = (ev.x - self.start.0).round() as i32;
        let dy = (ev.y - self.start.1).round() as i32;
        if (dx, dy) != self.last {
            self.apply(ctx, dx, dy);
        }
        true
    }

    fn button_release(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 || !self.dragging {
            return false;
        }
        let dx = (ev.x - self.start.0).round() as i32;
        let dy = (ev.y - self.start.1).round() as i32;
        if (dx, dy) != self.last {
            self.apply(ctx, dx, dy);
        }
        self.finish(ctx);
        true
    }

    fn key_press(&mut self, ctx: &mut ModeContext<'_>, key: Key) -> bool {
        if key == Key::Escape && self.dragging {
            self.cancel(ctx);
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
    fn drag_moves_layer_and_undo_restores() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut picked = None;
        {
            let c = ctx(&mut doc, &brush, &mut state, &mut picked);
            let red = vec![255u8, 0, 0, 255].repeat(8 * 8);
            c.doc
                .surface_mut()
                .load_from_rgba8(&red, 0, 0, 8, 8);
        }
        let mut mode = LayerMoveMode::new();
        {
            let mut c = ctx(&mut doc, &brush, &mut state, &mut picked);
            assert!(mode.button_press(&mut c, &PointerEvent::button(4.0, 4.0, 0.0, 1)));
            mode.motion(&mut c, &PointerEvent::motion(68.0, 4.0, 16.0));
            mode.button_release(&mut c, &PointerEvent::button(68.0, 4.0, 32.0, 1));
        }
        assert_eq!(doc.command_stack.undo_len(), 1);
        let shifted = doc
            .surface()
            .tile(1, 0)
            .expect("红块应移到右侧 tile");
        assert_eq!(shifted[0][0][3], 1 << 15);
        doc.undo();
        let back = doc.surface().tile(0, 0).expect("撤销后回到原 tile");
        assert_eq!(back[0][0][3], 1 << 15);
    }
}

