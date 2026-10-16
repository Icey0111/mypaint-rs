//! 文档模型 —— 照搬 MyPaint `lib/document.py` 中与撤销/重做有关的部分。
//!
//! 完整移植 `lib/document.py`（2200 行，含图层栈、缓存目录、自动保存、
//! 帧/画框、分辨率、图层视图…）不在本轮范围；这里先建立**命令栈的宿主**，
//! 让后续的 `.ora`/`.png` 读写、图层栈、自动保存都挂到同一个对象上。
//!
//! | MyPaint | 本文件 |
//! |---|---|
//! | `lib/document.py:247` `Document` | [`Document`] |
//! | `:59` `DEFAULT_UNDO_STACK_SIZE = 40` | [`DEFAULT_UNDO_STACK_SIZE`] |
//! | `:309` `self.command_stack = CommandStack(max_undo_stack_size)` | [`Document::new`] |
//! | `:1158` `undo()` / `:1166` `redo()`（连跳 `automatic_undo`） | [`Document::undo`] / [`Document::redo`] |
//! | `:1174` `do(cmd)` | [`Document::do_command`] |
//! | `:1179`/`:1184` `update_last_command`/`get_last_command` | 同名方法 |
//! | `:1191` `get_bbox()` | [`Document::data_bbox`] |
//! | `:923` `clear()` | [`Document::clear`] |
//! | `:1046` `unsaved_painting_time` | [`Document::unsaved_painting_time`] |
//!
//! 尚未移植（属后续里程碑）：`layer_stack`（P1 图层）、`sync_pending_changes`
//! （我们的作画是同步落盘到表面的，没有待刷队列）、帧/画框、分辨率、
//! 自动保存与缓存目录、`.ora`/`.png` 读写（下一轮的文档 I/O）。

use hokusai::tile::TILE_SIZE;
use pigment_surface::PigmentSurface;

use crate::command::{
    Brushwork, Command, CommandStack, CommandUpdate, DocContext, DEFAULT_UNDO_STACK_SIZE,
};

/// 文档模型：一个表面 + 一条撤销栈。
pub struct Document {
    surface: PigmentSurface,
    /// 撤销/重做栈（`lib/document.py:309`）。
    pub command_stack: CommandStack,
    /// 累计的"未保存作画时长"（秒），`.ora` 的 JSON 设置会写它
    /// （`lib/document.py:1046`；`Brushwork` 的 redo/undo 会改写）。
    pub unsaved_painting_time: f64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// 用 MyPaint 的默认栈深新建（偏好 `command.max_undo_stack_size`
    /// 覆盖它，见 `gui/application.py:338-343`）。
    pub fn new() -> Self {
        Self::with_max_undo_stack_size(DEFAULT_UNDO_STACK_SIZE)
    }

    /// 指定栈深新建。
    pub fn with_max_undo_stack_size(max_undo_stack_size: usize) -> Self {
        Self {
            surface: PigmentSurface::new(),
            command_stack: CommandStack::new(max_undo_stack_size),
            unsaved_painting_time: 0.0,
        }
    }

    #[inline]
    pub fn surface(&self) -> &PigmentSurface {
        &self.surface
    }

    #[inline]
    pub fn surface_mut(&mut self) -> &mut PigmentSurface {
        &mut self.surface
    }

    /// 命令上下文（`DocContext` 的两个字段都来自本对象）。
    ///
    /// 直接解构 `self` 而不是调 `self.context()`：栈与表面要同时可变借用，
    /// 只能靠字段级拆分（Rust 里没有 `weakref` 那种绕法）。
    fn split(&mut self) -> (&mut CommandStack, DocContext<'_>) {
        let Document {
            command_stack,
            surface,
            unsaved_painting_time,
        } = self;
        (
            command_stack,
            DocContext {
                surface,
                unsaved_painting_time,
            },
        )
    }

    /// 撤销最近一次命令（`lib/document.py:1158`），返回被撤销命令的展示名。
    ///
    /// 与源码一致地连跳 `automatic_undo` 命令（那些命令的撤销是自动行为，
    /// 不该占用用户的一次撤销）。
    pub fn undo(&mut self) -> Option<String> {
        self.sync_pending_changes();
        let (stack, mut ctx) = self.split();
        loop {
            let outcome = stack.undo(&mut ctx)?;
            if !outcome.automatic_undo {
                return Some(outcome.display_name);
            }
        }
    }

    /// 重做最近一次撤销（`lib/document.py:1166`）。
    pub fn redo(&mut self) -> Option<String> {
        self.sync_pending_changes();
        let (stack, mut ctx) = self.split();
        loop {
            let outcome = stack.redo(&mut ctx)?;
            if !outcome.applied {
                // 命令被取消：栈没动，继续重做也没有意义
                return None;
            }
            if !outcome.automatic_undo {
                return Some(outcome.display_name);
            }
        }
    }

    /// 执行一条命令并入栈（`lib/document.py:1174`）。
    pub fn do_command(&mut self, command: Box<dyn Command>) -> bool {
        let (stack, mut ctx) = self.split();
        stack.do_command(command, &mut ctx)
    }

    /// 执行一段已收尾的作画（`lib/command.py:Brushwork` 的常规入口）。
    pub fn do_brushwork(&mut self, brushwork: Brushwork) -> bool {
        self.do_command(Box::new(brushwork))
    }

    /// 更新最近一次命令（`lib/document.py:1179`），供笔刷编辑器的
    /// Live Update 用。
    pub fn update_last_command(&mut self, update: &CommandUpdate<'_>) -> Option<String> {
        let (stack, mut ctx) = self.split();
        stack.update_last_command(&mut ctx, update)
    }

    /// 最近一次命令（`lib/document.py:1184`）。
    pub fn get_last_command(&self) -> Option<&dyn Command> {
        self.command_stack.get_last_command()
    }

    /// 内容外接框（`lib/document.py:1191` 的 `get_bbox()` 的 tile 对齐版）：
    /// 已分配 tile 的最小外接矩形，返回 `(x, y, w, h)`；空画布返回全 0。
    ///
    /// 与 MyPaint 一致地**按 tile 边界取整**（`lib/tiledsurface.py` 的
    /// `get_bbox()`），因为它会被 `.ora`/`.png` 导出直接当画幅用。
    pub fn data_bbox(&self) -> (i32, i32, i32, i32) {
        let mut it = self.surface.tiles().map(|(c, _)| c);
        let Some(first) = it.next() else {
            return (0, 0, 0, 0);
        };
        let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
        for (tx, ty) in it {
            x0 = x0.min(tx);
            y0 = y0.min(ty);
            x1 = x1.max(tx);
            y1 = y1.max(ty);
        }
        let ts = TILE_SIZE as i32;
        (x0 * ts, y0 * ts, (x1 - x0 + 1) * ts, (y1 - y0 + 1) * ts)
    }

    /// 清空文档（`lib/document.py:923`）：丢内容、清栈、作画时长归零。
    pub fn clear(&mut self) {
        self.surface = PigmentSurface::new();
        self.command_stack.clear();
        self.unsaved_painting_time = 0.0;
    }

    /// `sync_pending_changes()`（`lib/document.py:1127`）的等价物。
    ///
    /// MyPaint 需要它把"待刷的笔画工作"提交给模型（Brushwork 的记录期是
    /// 异步收尾的）；我们的 [`Brushwork`] 在 `stop_recording` 时同步完成，
    /// 因此这里无事可做。保留这个方法名是为了让调用点与源码一一对应。
    #[inline]
    pub fn sync_pending_changes(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushlib::default_brush;
    use crate::stroke::StrokeEvent;
    use hokusai::{Brush, BrushSetting, BrushState};

    fn test_brush(radius: f32) -> Brush {
        let mut b = default_brush();
        b.get_mut(BrushSetting::Radius).base_value = radius;
        b
    }

    /// 在文档上画一笔并提交（走 app 相同的三段式流程）。
    fn paint_and_commit(doc: &mut Document, radius: f32) -> bool {
        let brush = test_brush(radius);
        let mut state = BrushState::default();
        let mut bw = Brushwork::new();
        for i in 0..40 {
            let ev = StrokeEvent::new(
                0.016,
                i as f32 * 2.0,
                radius * 10.0,
                1.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
            );
            bw.stroke_to(doc.surface_mut(), &brush, "Test Brush", &mut state, ev);
        }
        let changed = bw.stop_recording(doc.surface_mut(), &state, false);
        if changed {
            doc.do_brushwork(bw);
        }
        changed
    }

    #[test]
    fn default_stack_size_matches_mypaint() {
        assert_eq!(DEFAULT_UNDO_STACK_SIZE, 40);
        let doc = Document::new();
        assert_eq!(doc.command_stack.max_stack_size(), 40);
    }

    #[test]
    fn undo_and_redo_round_trip_pixels() {
        let mut doc = Document::new();
        assert!(paint_and_commit(&mut doc, 2.0));
        assert!(doc.surface().tile_count() > 0);
        assert!(doc.unsaved_painting_time > 0.0);

        doc.undo();
        assert_eq!(doc.surface().tile_count(), 0);
        assert_eq!(doc.unsaved_painting_time, 0.0);

        doc.redo();
        assert!(doc.surface().tile_count() > 0);
    }

    #[test]
    fn undo_returns_the_command_display_name() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        let name = doc.undo().unwrap();
        assert!(name.ends_with("of painting with Test Brush"), "{name}");
        assert!(doc.undo().is_none(), "栈空后返回 None");
    }

    #[test]
    fn empty_stroke_is_not_pushed_onto_the_stack() {
        let mut doc = Document::new();
        // 半径极小 + 只喂一个事件 → 引擎什么都没画
        let brush = test_brush(-6.0);
        let mut state = BrushState::default();
        let mut bw = Brushwork::new();
        let ev = StrokeEvent::new(0.001, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        bw.stroke_to(doc.surface_mut(), &brush, "Tiny", &mut state, ev);
        let changed = bw.stop_recording(doc.surface_mut(), &state, false);
        assert!(!changed, "零时长笔画应被判为空");
        assert_eq!(doc.command_stack.undo_len(), 0);
    }

    #[test]
    fn data_bbox_is_tile_aligned() {
        let mut doc = Document::new();
        assert_eq!(doc.data_bbox(), (0, 0, 0, 0), "空画布无内容");
        paint_and_commit(&mut doc, 2.0);
        let (x, y, w, h) = doc.data_bbox();
        let ts = TILE_SIZE as i32;
        assert_eq!(x % ts, 0);
        assert_eq!(y % ts, 0);
        assert_eq!(w % ts, 0);
        assert_eq!(h % ts, 0);
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn clear_drops_content_and_history() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        doc.clear();
        assert_eq!(doc.surface().tile_count(), 0);
        assert_eq!(doc.command_stack.undo_len(), 0);
        assert_eq!(doc.command_stack.redo_len(), 0);
        assert_eq!(doc.unsaved_painting_time, 0.0);
    }

    #[test]
    fn update_last_command_rerenders_with_another_brush() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        let tile_before = doc.surface().tile_count();
        let fatter = test_brush(4.0);
        let name = doc.update_last_command(&CommandUpdate::Brush {
            brush: &fatter,
            name: "Fatter",
        });
        assert!(name.is_some());
        assert!(doc.surface().tile_count() >= tile_before);
        assert!(doc.get_last_command().unwrap().display_name().ends_with("with Fatter"));
    }
}
