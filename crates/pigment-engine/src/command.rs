//! 撤销/重做命令模型 —— 照搬 MyPaint `lib/command.py`。
//!
//! 结构完全对齐源码：
//!
//! | MyPaint | 本文件 |
//! |---|---|
//! | `lib/command.py:33` `CommandStack` | [`CommandStack`] |
//! | `:61` `do()`（含 `is False` 取消语义） | [`CommandStack::do_command`]（`do` 是 Rust 关键字） |
//! | `:86` `undo()` / `:100` `redo()` | [`CommandStack::undo`] / [`CommandStack::redo`] |
//! | `:124` `reduce_undo_history()` | [`CommandStack::reduce_undo_history`] |
//! | `:129`/`:135` `get_last_command`/`update_last_command` | 同名方法 |
//! | `:152` `Command`（`redo`/`undo`/`update`/`display_name`/`automatic_undo`） | [`Command`] trait |
//! | `:250` `Brushwork` | [`Brushwork`] |
//!
//! # 与源码的三处结构性差异（都是"暂无图层栈"造成的）
//!
//! 1. **命令自己持 doc 引用 vs. 由栈传入**：MyPaint 的命令在构造时拿一个
//!    `weakref.proxy(doc)`，`redo()`/`undo()` 无参；Rust 里借用检查不允许
//!    自引用，所以改成把 [`DocContext`] 传进 `redo`/`undo`（语义相同：
//!    命令作用于同一个文档）。
//! 2. **`layer_path` / `layer` 参数取消**：目前文档只有一层表面，命令直接
//!    作用于它。P1 引入图层栈后按 `lib/command.py:341` 的 `_target_layer`
//!    恢复"路径寻址 + 显式目标"两种模式。
//! 3. **`add_stroke_shape` 未移植**：它把笔画外接框记进图层的 strokemap
//!    （服务 trim 与 `.ora` 导出里的 `mypaint_strokemap`），我们还没有
//!    strokemap 层。属已知缺口，随 `.ora` 读写一并补。

use std::collections::VecDeque;

use hokusai::{Brush, BrushState};
use pigment_surface::{PigmentSurface, SurfaceSnapshot};

use crate::stroke::{Stroke, StrokeEvent};

/// MyPaint 的 `DEFAULT_UNDO_STACK_SIZE`（`lib/document.py:59`）。
pub const DEFAULT_UNDO_STACK_SIZE: usize = 40;

/// 命令的执行上下文：命令在什么上干活。
///
/// 对应 MyPaint 命令里的 `self.doc`（文档模型）。
pub struct DocContext<'a> {
    /// 目标表面。P1 之前文档只有这一层。
    pub surface: &'a mut PigmentSurface,
    /// `Document.unsaved_painting_time`：`Brushwork` 的 redo/undo 会改写它
    /// （`lib/command.py:366`、`:374`）。
    pub unsaved_painting_time: &'a mut f64,
}

/// `CommandStack.update_last_command(**kwargs)` 的更新载荷。
///
/// MyPaint 用关键字参数把不同类型的更新塞给同一个命令；Rust 用枚举表达
/// 同一件事。目前唯一的调用方是笔刷编辑器的 Live Update
/// （`gui/brusheditor.py:1137` → `Document.redo_last_stroke_with_different_brush`）。
/// 图层类更新（`name` / `visible` / `locked` / `opacity` / `mode`）随 P1 的
/// 图层栈引入。
pub enum CommandUpdate<'a> {
    /// `Brushwork.update(brushinfo)`：用另一支笔重绘上一笔。
    Brush { brush: &'a Brush, name: &'a str },
}

/// 一次可撤销的改动（`lib/command.py:152` 的 `Command`）。
pub trait Command {
    /// 执行或重做这次改动；返回 `false` 表示**取消**——调用方须回滚且不入栈
    /// （`lib/command.py:70-81`，注意源码用的是 `completed is False`，
    /// 不是真值判断）。返回 `true` 但实际没有内容变化的改动（空笔画）由调用
    /// 方自行丢弃，不入栈。
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool;

    /// 撤销 `redo()` 的效果。
    fn undo(&mut self, ctx: &mut DocContext<'_>);

    /// 展示名（撤销/重做菜单项的文案，`lib/document.py:192` 的
    /// `"Undo %s"` 会填它）。
    fn display_name(&self) -> String;

    /// 是否属于"自动撤销"类命令（`Document.undo` 会连跳这些，
    /// `lib/document.py:1160-1164`）。
    fn automatic_undo(&self) -> bool {
        false
    }

    /// `Command.update(**kwargs)`（`lib/command.py:216`）。默认不支持。
    fn update(&mut self, _ctx: &mut DocContext<'_>, _update: &CommandUpdate<'_>) -> bool {
        false
    }
}

/// 一次 `undo()` / `redo()` 的结果。
///
/// MyPaint 的栈方法直接返回命令对象，`Document` 再读它的 `automatic_undo`
/// 决定要不要连跳（`lib/document.py:1160-1164`）；Rust 里命令的所有权已经
/// 搬到另一个栈上，所以把调用方需要的两件事抽成一个值返回。
pub struct CommandOutcome {
    /// 命令的展示名。
    pub display_name: String,
    /// 该命令是否属于"自动撤销"类（连跳依据）。
    pub automatic_undo: bool,
    /// `redo()` 专用：本次是否真的执行成功（`false` = 被取消，栈未动）。
    pub applied: bool,
}

/// 撤销/重做栈（`lib/command.py:33`）。
#[derive(Default)]
pub struct CommandStack {
    max_stack_size: usize,
    undo_stack: VecDeque<Box<dyn Command>>,
    redo_stack: VecDeque<Box<dyn Command>>,
}

impl CommandStack {
    /// 新建栈；`max_stack_size` 必须是正数（源码断言的等价物）。
    pub fn new(max_stack_size: usize) -> Self {
        assert!(max_stack_size > 0, "max_stack_size 必须为正");
        Self {
            max_stack_size,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
        }
    }

    /// 清空两个栈（`lib/command.py:50`；`Document.load` 会调用它）。
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// 执行新命令并压栈（`lib/command.py:61` 的 `do()`；`do` 是 Rust 保留字，故改名）。
    ///
    /// 与源码一致：**先无条件丢弃 redo 栈**（`:73-75` 注释：跟随 Krita 的
    /// 行为），再执行；命令返回"取消"时回滚且不入栈。
    ///
    /// 返回 `true` 表示命令已入栈。
    pub fn do_command(&mut self, mut command: Box<dyn Command>, ctx: &mut DocContext<'_>) -> bool {
        self.redo_stack.clear();
        let completed = command.redo(ctx);
        // 源码刻意写成 `if completed is False`（只有显式 False 才算取消），
        // 这里 `bool` 没有真值歧义，语义等价。
        if !completed {
            command.undo(ctx);
            false
        } else {
            self.undo_stack.push_back(command);
            self.reduce_undo_history();
            true
        }
    }

    /// 撤销最近一次命令（`lib/command.py:86`）。
    pub fn undo(&mut self, ctx: &mut DocContext<'_>) -> Option<CommandOutcome> {
        let mut command = self.undo_stack.pop_back()?;
        command.undo(ctx);
        let outcome = CommandOutcome {
            display_name: command.display_name(),
            automatic_undo: command.automatic_undo(),
            applied: true,
        };
        self.redo_stack.push_back(command);
        Some(outcome)
    }

    /// 重做最近一次撤销（`lib/command.py:100`）。
    ///
    /// 命令被取消时栈**不动**，返回 `applied = false`。
    pub fn redo(&mut self, ctx: &mut DocContext<'_>) -> Option<CommandOutcome> {
        let completed = {
            let command = self.redo_stack.back_mut()?;
            command.redo(ctx)
        };
        if !completed {
            // 被取消：就地回滚，栈不动（`lib/command.py:116-117`）
            let command = self.redo_stack.back_mut()?;
            command.undo(ctx);
            return Some(CommandOutcome {
                display_name: command.display_name(),
                automatic_undo: command.automatic_undo(),
                applied: false,
            });
        }
        let command = self.redo_stack.pop_back()?;
        let outcome = CommandOutcome {
            display_name: command.display_name(),
            automatic_undo: command.automatic_undo(),
            applied: true,
        };
        self.undo_stack.push_back(command);
        Some(outcome)
    }

    /// 把撤销栈修剪到上限（`lib/command.py:124`）。
    pub fn reduce_undo_history(&mut self) {
        while self.undo_stack.len() > self.max_stack_size {
            self.undo_stack.pop_front();
        }
    }

    /// 最近一次执行的命令（`lib/command.py:129`）。
    pub fn get_last_command(&self) -> Option<&dyn Command> {
        self.undo_stack.back().map(|c| c.as_ref())
    }

    /// 更新最近一次执行的命令（`lib/command.py:135`）。
    ///
    /// 返回更新后的展示名；没有可更新的命令、或命令不支持这种更新时返回
    /// `None`。
    pub fn update_last_command(
        &mut self,
        ctx: &mut DocContext<'_>,
        update: &CommandUpdate<'_>,
    ) -> Option<String> {
        let command = self.undo_stack.back_mut()?;
        let old_name = command.display_name();
        if !command.update(ctx, update) {
            return None;
        }
        let new_name = command.display_name();
        let _ = old_name != new_name; // 源码据此触发 stack_updated 事件（我们按帧轮询）
        Some(new_name)
    }

    /// 撤销栈深度。
    #[inline]
    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    /// 重做栈深度。
    #[inline]
    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// 栈上限。
    #[inline]
    pub fn max_stack_size(&self) -> usize {
        self.max_stack_size
    }

    /// 下一个重做命令的展示名（UI 文案用，`gui/document.py:582`）。
    pub fn next_redo_name(&self) -> Option<String> {
        self.redo_stack.back().map(|c| c.display_name())
    }
}

/// 一段作画（`lib/command.py:250` 的 `Brushwork`）。
///
/// 一个 Brushwork 的生命周期：
///
/// 1. **记录期**：起笔时 [`Brushwork::start_recording`] 取一份"画前快照"
///    （`_sshot_before`）并开一条 [`Stroke`]；之后每个指针事件走
///    [`Brushwork::stroke_to`]，一边画一边记；
/// 2. **收尾**：[`Brushwork::stop_recording`] 结束录制，取"画后快照"
///    （`_sshot_after`）。没画到东西（`stroke_seq.empty`）就整个丢弃；
/// 3. **撤销/重做**：就是在这两份快照之间来回切（[`Brushwork::undo`] /
///    [`Brushwork::redo`]）。
#[derive(Default)]
pub struct Brushwork {
    /// 展示名覆盖（`description` 参数；auto-split 分段时继承上一段的描述）。
    pub description: Option<String>,
    /// 记录期标志。
    recording_started: bool,
    recording_finished: bool,
    /// 事件流（`_stroke_seq`）。
    stroke_seq: Option<Stroke>,
    /// 切换用的两个状态。
    sshot_before: Option<SurfaceSnapshot>,
    sshot_after: Option<SurfaceSnapshot>,
    time_before: f64,
    time_after: f64,
    sshot_after_applied: bool,
    /// 是否该把这段作画切成一段新的 Brushwork（`lib/command.py:306`）。
    ///
    /// MyPaint 取图层 `stroke_to()` 的返回值（图层内部按 FIFO 桶/`split` 决定）；
    /// 我们还没有需要切段的图层逻辑，恒为 `false`。
    pub split_due: bool,
}

impl Brushwork {
    pub fn new() -> Self {
        Self::default()
    }

    /// 带展示名新建（`description` 参数）。
    pub fn with_description(description: impl Into<String>) -> Self {
        Self {
            description: Some(description.into()),
            ..Self::default()
        }
    }

    /// 记录期是否已开始。
    #[inline]
    pub fn is_recording(&self) -> bool {
        self.recording_started && !self.recording_finished
    }

    /// 记录期是否已结束。
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.recording_finished
    }

    /// 事件流（记录期结束后才有意义）。
    #[inline]
    pub fn stroke(&self) -> Option<&Stroke> {
        self.stroke_seq.as_ref()
    }

    /// 展示名（`lib/command.py:321`）。
    pub fn display_name(&self) -> String {
        if let Some(desc) = &self.description {
            return desc.clone();
        }
        let (time, brush_name) = match &self.stroke_seq {
            None => (0.0, UNDEFINED_BRUSH.to_string()),
            Some(seq) => (seq.total_painting_time(), seq.brush_name().to_string()),
        };
        // 对应源码的 `_("{seconds:.01f}s of painting with {brush_name}")`
        format!("{time:.1}s of painting with {brush_name}")
    }

    /// 起笔：取画前快照、开一条事件流（`lib/command.py:391` 的
    /// `_check_recording_started`）。
    ///
    /// 返回 `Err` 时录制未开始：调用方应继续作画但**不要**把这段作画
    /// 提交到命令栈（等价于源码里"记录起不来 → `tiles_changed=False`"）。
    pub fn start_recording(
        &mut self,
        surface: &PigmentSurface,
        brush: &Brush,
        brush_name: &str,
        state: &BrushState,
    ) -> Result<(), String> {
        assert!(!self.recording_finished, "收录已结束，不可重新开始");
        if self.recording_started {
            return Ok(());
        }
        self.sshot_before = Some(surface.save_snapshot());
        self.time_before = 0.0;
        let mut seq = Stroke::new();
        seq.start_recording(brush, brush_name, state)?;
        self.stroke_seq = Some(seq);
        self.recording_started = true;
        Ok(())
    }

    /// 画一笔并记录一个事件（`lib/command.py:421` 的 `stroke_to`）。
    ///
    /// 返回引擎是否真的画了 dab。未开始录制时自动开始（源码在同一处调
    /// `_check_recording_started`），因此调用方只需在起笔那一刻调用即可。
    pub fn stroke_to(
        &mut self,
        surface: &mut PigmentSurface,
        brush: &Brush,
        brush_name: &str,
        state: &mut BrushState,
        ev: StrokeEvent,
    ) -> bool {
        if self.start_recording(surface, brush, brush_name, state).is_err() {
            return false;
        }
        if let Some(seq) = self.stroke_seq.as_mut() {
            seq.record_event(ev);
        }
        let painted = ev.apply(brush, state, surface);
        // 见字段文档：暂无切段逻辑
        self.split_due = false;
        painted
    }

    /// 收笔：结束录制并取画后快照（`lib/command.py:500`）。
    ///
    /// `revert = true` 时把表面回滚到画前状态（`brushwork_rollback` 的路径），
    /// 此时应丢弃本命令。
    ///
    /// 返回 `true` 表示**这一笔确实改动了画面**——只有这种情况才该把命令
    /// 交给命令栈（`lib/command.py:549` 的 `tiles_changed`）。
    pub fn stop_recording(
        &mut self,
        surface: &mut PigmentSurface,
        state: &BrushState,
        revert: bool,
    ) -> bool {
        if !self.recording_started {
            // 记录期没开始（笔刷设置无法序列化等）→ 无内容可提交
            return false;
        }
        if self.recording_finished {
            return false;
        }
        self.recording_finished = true;
        let Some(seq) = self.stroke_seq.as_mut() else {
            return false;
        };
        seq.stop_recording(state);
        if revert {
            if let Some(before) = &self.sshot_before {
                surface.load_snapshot(before);
            }
            return false;
        }
        if seq.is_empty() {
            return false; // 没画到东西 → 不入栈
        }
        let time_after = self.time_before + seq.total_painting_time();
        self.time_after = time_after;
        self.sshot_after = Some(surface.save_snapshot());
        // 画作在记录期就已落到画布上，所以 redo() 不该再画一遍
        // （`lib/command.py:548` 的 `_sshot_after_applied = True`）
        self.sshot_after_applied = true;
        true
    }
}

const UNDEFINED_BRUSH: &str = "Undefined (command not started yet)";

impl Command for Brushwork {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        assert!(self.recording_finished, "先调 stop_recording()");
        let (Some(before), Some(after)) = (&self.sshot_before, &self.sshot_after) else {
            return false;
        };
        let _ = before;
        if !self.sshot_after_applied {
            ctx.surface.load_snapshot(after);
            self.sshot_after_applied = true;
        }
        *ctx.unsaved_painting_time = self.time_after;
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        assert!(self.recording_finished, "先调 stop_recording()");
        if let Some(before) = &self.sshot_before {
            ctx.surface.load_snapshot(before);
        }
        *ctx.unsaved_painting_time = self.time_before;
        self.sshot_after_applied = false;
    }

    fn display_name(&self) -> String {
        Brushwork::display_name(self)
    }

    /// 用另一支笔重绘上一笔（`lib/command.py:377`）。
    fn update(&mut self, ctx: &mut DocContext<'_>, update: &CommandUpdate<'_>) -> bool {
        let CommandUpdate::Brush { brush, name } = update;
        if !self.recording_finished {
            return false;
        }
        assert!(
            self.sshot_after_applied,
            "command.Brushwork 必须先被应用才能 update"
        );
        let Some(before) = &self.sshot_before else {
            return false;
        };
        let Some(seq) = &self.stroke_seq else {
            return false;
        };
        let Ok(stroke) = seq.copy_using_different_brush(brush, name) else {
            return false;
        };
        // 回到画前状态，再用新笔重放整条事件流
        ctx.surface.load_snapshot(before);
        if !stroke.render(ctx.surface) {
            return false;
        }
        self.stroke_seq = Some(stroke);
        self.sshot_after = Some(ctx.surface.save_snapshot());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushlib::default_brush;
    use hokusai::BrushSetting;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn test_brush(radius: f32) -> Brush {
        let mut b = default_brush();
        b.get_mut(BrushSetting::Radius).base_value = radius;
        b
    }

    fn ctx<'a>(
        surface: &'a mut PigmentSurface,
        time: &'a mut f64,
    ) -> DocContext<'a> {
        DocContext {
            surface,
            unsaved_painting_time: time,
        }
    }

    /// 记录调用顺序的假命令，用于验证栈的语义（不碰表面）。
    struct FakeCommand {
        log: Rc<RefCell<Vec<String>>>,
        name: String,
        cancel: bool,
        automatic: bool,
    }

    impl FakeCommand {
        fn new(log: &Rc<RefCell<Vec<String>>>, name: &str) -> Box<Self> {
            Box::new(Self {
                log: Rc::clone(log),
                name: name.to_string(),
                cancel: false,
                automatic: false,
            })
        }
    }

    impl Command for FakeCommand {
        fn redo(&mut self, _ctx: &mut DocContext<'_>) -> bool {
            self.log.borrow_mut().push(format!("redo:{}", self.name));
            !self.cancel
        }
        fn undo(&mut self, _ctx: &mut DocContext<'_>) {
            self.log.borrow_mut().push(format!("undo:{}", self.name));
        }
        fn display_name(&self) -> String {
            self.name.clone()
        }
        fn automatic_undo(&self) -> bool {
            self.automatic
        }
    }

    #[test]
    fn do_pushes_and_calls_redo() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut stack = CommandStack::new(4);
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        assert!(stack.do_command(FakeCommand::new(&log, "A"), &mut ctx(&mut s, &mut t)));
        assert_eq!(&*log.borrow(), &["redo:A".to_string()]);
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 0);
        assert_eq!(stack.get_last_command().unwrap().display_name(), "A");
    }

    /// 取消语义：`redo()` 返回 false 时要**就地回滚**且不入栈，
    /// 但 redo 栈依然被丢弃（`lib/command.py:73-81`）。
    #[test]
    fn cancelled_command_is_rolled_back_and_not_pushed() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut stack = CommandStack::new(4);
        let mut s = PigmentSurface::new();
        let mut t = 0.0;

        stack.do_command(FakeCommand::new(&log, "A"), &mut ctx(&mut s, &mut t));
        stack.do_command(FakeCommand::new(&log, "B"), &mut ctx(&mut s, &mut t));
        stack.undo(&mut ctx(&mut s, &mut t));
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 1);

        let mut cancelling = FakeCommand::new(&log, "C");
        cancelling.cancel = true;
        assert!(!stack.do_command(cancelling, &mut ctx(&mut s, &mut t)));

        assert_eq!(
            &*log.borrow(),
            &[
                "redo:A".to_string(),
                "redo:B".to_string(),
                "undo:B".to_string(),
                "redo:C".to_string(),
                "undo:C".to_string()
            ]
        );
        assert_eq!(stack.undo_len(), 1, "取消的命令不入栈");
        assert_eq!(stack.redo_len(), 0, "redo 栈要先被丢弃（A 之后被完好的 B 挡着）");
    }

    #[test]
    fn undo_and_redo_move_commands_between_stacks() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut stack = CommandStack::new(4);
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        stack.do_command(FakeCommand::new(&log, "A"), &mut ctx(&mut s, &mut t));
        stack.do_command(FakeCommand::new(&log, "B"), &mut ctx(&mut s, &mut t));

        assert_eq!(stack.undo(&mut ctx(&mut s, &mut t)).map(|o| o.display_name).as_deref(), Some("B"));
        assert_eq!(stack.next_redo_name().as_deref(), Some("B"));
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo(&mut ctx(&mut s, &mut t)).map(|o| o.display_name).as_deref(), Some("B"));
        assert_eq!(stack.undo_len(), 2);
        assert_eq!(stack.redo_len(), 0);
        assert_eq!(
            &*log.borrow(),
            &[
                "redo:A".to_string(),
                "redo:B".to_string(),
                "undo:B".to_string(),
                "redo:B".to_string()
            ]
        );
    }

    #[test]
    fn undo_on_empty_stack_is_a_noop() {
        let mut stack = CommandStack::new(4);
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        assert!(stack.undo(&mut ctx(&mut s, &mut t)).is_none());
        assert!(stack.redo(&mut ctx(&mut s, &mut t)).is_none());
    }

    /// 上限修剪：最旧的命令被丢掉（`lib/command.py:124`）。
    #[test]
    fn reduce_undo_history_trims_oldest() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut stack = CommandStack::new(2);
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        for name in ["A", "B", "C"] {
            stack.do_command(FakeCommand::new(&log, name), &mut ctx(&mut s, &mut t));
        }
        assert_eq!(stack.undo_len(), 2);
        assert_eq!(stack.get_last_command().unwrap().display_name(), "C");
        assert_eq!(stack.undo(&mut ctx(&mut s, &mut t)).map(|o| o.display_name).as_deref(), Some("C"));
        assert_eq!(stack.undo(&mut ctx(&mut s, &mut t)).map(|o| o.display_name).as_deref(), Some("B"));
        assert!(stack.undo(&mut ctx(&mut s, &mut t)).is_none(), "A 已被丢弃");
    }

    #[test]
    fn stack_requires_positive_size() {
        assert!(std::panic::catch_unwind(|| CommandStack::new(0)).is_err());
    }

    /// 用真实笔刷画一笔，验证 Brushwork 的完整生命周期。
    fn paint_a_stroke(surface: &mut PigmentSurface) -> Brushwork {
        let brush = test_brush(2.0);
        let mut state = BrushState::default();
        let mut bw = Brushwork::new();
        for i in 0..40 {
            let ev = StrokeEvent::new(
                0.016,
                i as f32 * 2.0,
                20.0,
                1.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
            );
            bw.stroke_to(surface, &brush, "Test Brush", &mut state, ev);
        }
        assert!(bw.stop_recording(surface, &state, false));
        bw
    }

    #[test]
    fn brushwork_undo_redo_restores_pixels() {
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        let bw = paint_a_stroke(&mut s);
        assert!(s.tile_count() > 0, "应先画出内容");
        let painted: Vec<_> = s
            .tiles()
            .map(|(c, tile)| (c, tile.pixels().to_vec()))
            .collect();

        let mut stack = CommandStack::new(8);
        assert!(stack.do_command(Box::new(bw), &mut ctx(&mut s, &mut t)));
        assert!(t > 0.0, "redo 应把作画时长写进 unsaved_painting_time");

        // 撤销 → 画布清空
        assert!(stack.undo(&mut ctx(&mut s, &mut t)).is_some());
        assert_eq!(s.tile_count(), 0, "撤销后不该有像素");
        assert_eq!(t, 0.0);

        // 重做 → 像素逐个还原
        assert!(stack.redo(&mut ctx(&mut s, &mut t)).is_some());
        assert_eq!(s.tile_count(), painted.len());
        for (coord, px) in &painted {
            assert_eq!(
                s.tile(coord.0, coord.1).map(|t| t.to_vec()),
                Some(px.clone()),
                "重做应逐像素还原 {coord:?}"
            );
        }
    }

    #[test]
    fn brushwork_display_name_uses_stroke_metadata() {
        let mut s = PigmentSurface::new();
        let bw = paint_a_stroke(&mut s);
        let name = bw.display_name();
        assert!(
            name.ends_with("of painting with Test Brush"),
            "展示名格式应对齐源码：{name}"
        );
        assert!(!name.starts_with("Undefined"));

        let fresh = Brushwork::new();
        // 对齐源码：未开始的命令展示名是 "0.0s of painting with Undefined (…)"
        assert_eq!(
            fresh.display_name(),
            "0.0s of painting with Undefined (command not started yet)"
        );
        let described = Brushwork::with_description("Some seconds of painting");
        assert_eq!(described.display_name(), "Some seconds of painting");
    }

    /// 空笔画（没画到东西）不该入栈。
    #[test]
    fn empty_brushwork_is_discarded() {
        let brush = test_brush(2.0);
        let state = BrushState::default();
        let mut s = PigmentSurface::new();
        let mut bw = Brushwork::new();
        bw.start_recording(&s, &brush, "Test", &state).unwrap();
        assert!(!bw.stop_recording(&mut s, &state, false), "空笔画返回 false");
        assert!(bw.stroke().unwrap().is_empty());
    }

    /// `revert = true` 要把画布退回起笔前（`brushwork_rollback`）。
    #[test]
    fn revert_rolls_back_the_stroke() {
        let brush = test_brush(2.0);
        let mut state = BrushState::default();
        let mut s = PigmentSurface::new();
        let mut bw = Brushwork::new();
        for i in 0..40 {
            let ev = StrokeEvent::new(0.016, i as f32 * 2.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            bw.stroke_to(&mut s, &brush, "Test", &mut state, ev);
        }
        assert!(s.tile_count() > 0);
        assert!(!bw.stop_recording(&mut s, &state, true));
        assert_eq!(s.tile_count(), 0, "revert 后画布应回到起笔前");
    }

    #[test]
    fn multiple_brushworks_undo_in_order() {
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        for _ in 0..3 {
            let bw = paint_a_stroke(&mut s);
            stack.do_command(Box::new(bw), &mut ctx(&mut s, &mut t));
        }
        assert_eq!(stack.undo_len(), 3);
        for _ in 0..3 {
            stack.undo(&mut ctx(&mut s, &mut t));
        }
        assert_eq!(s.tile_count(), 0, "三次撤销后应回到空画布");
    }

    /// 撤销之后再画一笔：redo 栈被清空（新命令顶掉旧的分支）。
    #[test]
    fn new_command_after_undo_clears_redo_stack() {
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let bw = paint_a_stroke(&mut s);
        stack.do_command(Box::new(bw), &mut ctx(&mut s, &mut t));
        stack.undo(&mut ctx(&mut s, &mut t));
        assert_eq!(stack.redo_len(), 1);

        let bw2 = paint_a_stroke(&mut s);
        stack.do_command(Box::new(bw2), &mut ctx(&mut s, &mut t));
        assert_eq!(stack.redo_len(), 0, "新命令必须清掉 redo 分支");
    }

    /// `Brushwork.update`：换一支笔重绘上一笔（笔刷编辑器的 Live Update）。
    #[test]
    fn update_rerenders_last_stroke_with_another_brush() {
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let bw = paint_a_stroke(&mut s);
        stack.do_command(Box::new(bw), &mut ctx(&mut s, &mut t));
        let before: Vec<(i32, i32)> = s.tiles().map(|(c, _)| c).collect();

        // 换成半径更大的笔刷重绘
        let fatter = test_brush(4.0);
        let name = stack.update_last_command(
            &mut ctx(&mut s, &mut t),
            &CommandUpdate::Brush {
                brush: &fatter,
                name: "Fatter",
            },
        );
        assert!(name.is_some());
        let after: Vec<(i32, i32)> = s.tiles().map(|(c, _)| c).collect();
        assert_ne!(before, after, "更大的笔刷应铺到更多 tile");
        let display = stack.get_last_command().unwrap().display_name();
        assert!(
            display.ends_with("with Fatter"),
            "换笔后展示名应跟着变：{display}"
        );

        // 更新后的状态仍可撤销回起笔前
        stack.undo(&mut ctx(&mut s, &mut t));
        assert_eq!(s.tile_count(), 0);
    }

    /// 不能更新已经被撤销掉的命令：此时命令在 redo 栈上，`undo()` 清掉了
    /// `_sshot_after_applied`，源码在此断言失败（`lib/command.py:381-383`）。
    ///
    /// 对应应用层的保护：笔刷编辑器的 Live Update 只作用于 `get_last_command()`
    /// （撤销栈顶），撤销后栈顶换人，所以走不到这里。本测试钉住这条不变量。
    #[test]
    fn update_after_undo_is_rejected_by_assertion() {
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let bw = paint_a_stroke(&mut s);
        stack.do_command(Box::new(bw), &mut ctx(&mut s, &mut t));
        // 撤销 → 命令移到 redo 栈，且标记为"未应用"
        stack.undo(&mut ctx(&mut s, &mut t));
        let fatter = test_brush(4.0);
        let cmd = stack.redo_stack.back_mut().unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cmd.update(
                    &mut ctx(&mut s, &mut t),
                    &CommandUpdate::Brush {
                        brush: &fatter,
                        name: "X",
                    },
                )
            }))
            .is_err(),
            "对已撤销的命令调用 update 必须触发断言（与源码一致）"
        );
    }

    /// 未应用的命令不允许 update（源码里的断言）。
    #[test]
    fn update_before_apply_is_rejected() {
        let mut s = PigmentSurface::new();
        let mut t = 0.0;
        let bw = paint_a_stroke(&mut s);
        let mut bw = bw;
        // 把 _sshot_after_applied 清掉：模拟"命令尚未应用"
        bw.sshot_after_applied = false;
        let fatter = test_brush(4.0);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bw.update(
                &mut ctx(&mut s, &mut t),
                &CommandUpdate::Brush {
                    brush: &fatter,
                    name: "X",
                },
            )
        }))
        .is_err());
    }

    #[test]
    fn automatic_undo_flag_defaults_to_false() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let cmd = FakeCommand::new(&log, "A");
        assert!(!cmd.automatic_undo());
        let mut auto = FakeCommand::new(&log, "B");
        auto.automatic = true;
        assert!(auto.automatic_undo());
    }
}
