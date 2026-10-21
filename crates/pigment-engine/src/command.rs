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
//! 3. **`add_stroke_shape` 已补（C3-1）**：`stop_recording()` 用前后快照做
//!    tile 位图 diff，把形状存进 `PigmentSurface::strokes`；撤销/重做随快照
//!    一起摘挂（`lib/layer/data.py:1577`）。

use std::collections::VecDeque;

use hokusai::{Brush, BrushState, TiledSurface};
use pigment_surface::combine::CombineMode;
use pigment_surface::layer::LayerMode;
use pigment_surface::{Layer, LayerStack, PigmentSurface, StrokeShape, SurfaceSnapshot};

use crate::stroke::{Stroke, StrokeEvent};

/// MyPaint 的 `DEFAULT_UNDO_STACK_SIZE`（`lib/document.py:59`）。
pub const DEFAULT_UNDO_STACK_SIZE: usize = 40;

/// 命令的执行上下文：命令在什么上干活。
///
/// 对应 MyPaint 命令里的 `self.doc`（文档模型）。B2-3 起携带整个图层栈；
/// 命令按**图层 id**（[`Layer`] 的稳定 id，等价 MyPaint 的图层对象引用）
/// 寻址，不受栈重排影响。
pub struct DocContext<'a> {
    /// 图层栈。
    pub layers: &'a mut LayerStack,
    /// 命名可见性视图（`lib/layervis.py` 的 `LayerViewManager`）。
    pub views: &'a mut crate::layer_views::LayerViews,
    /// `Document.unsaved_painting_time`：`Brushwork` 的 redo/undo 会改写它
    /// （`lib/command.py:366`、`:374`）。
    pub unsaved_painting_time: &'a mut f64,
    /// `Document._frame`：画框命令直接改它（`UpdateFrame`）。
    pub frame: &'a mut (i32, i32, i32, i32),
}

impl<'a> DocContext<'a> {
    /// 当前层可画的表面（作画入口）；组/锁/隐藏分支 → `None`
    /// （`data.py:1364 get_paintable`）。
    pub fn current_surface_mut(&mut self) -> Option<&mut PigmentSurface> {
        self.layers.paintable_surface_mut()
    }

    /// 按 id 解析目标层表面（等价 `command.py` 的 `_target_layer()`）。
    ///
    /// id 不是绘画层（组）或不在栈里时返回 `None`——笔刷据此跳过（源码
    /// `Brushwork._check_recording_started` 对非可画层 `_stroke_target_layer = None`）。
    fn surface_for(&mut self, id: u64) -> Option<&mut PigmentSurface> {
        if self.layers.layer_by_id(id).is_none() {
            // 正常运行到不了这里：撤销按 LIFO，RemoveLayer 的 undo 会先于
            // 在其上作画的 Brushwork 的 undo 恢复图层。
            debug_assert!(false, "layer {id} 不在栈里（撤销顺序被破坏？）");
        }
        self.layers.surface_by_id(id)
    }
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
    /// `SetLayerVisibility.update(visible)`（滑条/勾选框拖动）。
    LayerVisibility(bool),
    /// `SetLayerLocked.update(locked)`。
    LayerLocked(bool),
    /// `SetLayerOpacity.update(opacity)`（不透明度滑条拖动）。
    LayerOpacity(f32),
    /// `RenameLayer.update(name)`（重命名连续提交时合并）。
    Rename(&'a str),
}

/// 图层属性的种类（源码 `isinstance(cmd, command.SetLayerVisibility)` 一类
/// 判据的等价物；见 [`CommandStack::top_is_layer_update`]）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayerProp {
    /// `SetLayerVisibility`
    Visibility,
    /// `SetLayerLocked`
    Locked,
    /// `SetLayerOpacity`
    Opacity,
    /// `RenameLayer`
    Name,
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

    /// 合并判据（`lib/document.py:1312/1374/1386/1404`）：
    /// 「本命令是否是**作用于某图层的某属性**的更新命令」——是则返回
    /// `(属性种类, 图层 id)`，栈顶据此判 `isinstance(cmd, cmd_class) and
    /// cmd.layer is layer` 并调 [`Command::update`] 合并，而不是入栈
    /// （滑条连续拖动不刷爆撤销栈）。默认 `None`（非图层属性命令）。
    fn layer_prop(&self) -> Option<(LayerProp, u64)> {
        None
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

    /// 栈顶命令是否是作用于 `layer_id` 的 `prop` 属性更新命令
    /// （`get_last_command()` + `isinstance(cmd, cmd_class) and cmd.layer is
    /// layer` 判据的合一）。
    ///
    /// **两类都必须匹配**：只比图层 id 会把「刚拖完不透明度，再点眼睛」误
    /// 判成可合并，`update` 因种类不符而拒绝 → 该次改动被静默丢弃。
    pub fn top_is_layer_update(&self, prop: LayerProp, layer_id: u64) -> bool {
        self.undo_stack
            .back()
            .and_then(|c| c.layer_prop())
            .is_some_and(|(p, id)| p == prop && id == layer_id)
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
    /// 目标图层 id（`lib/command.py:253` 的 `layer=None` → 构造时的当前层）。
    /// 作画方在起笔前设置；未设置时在第一次 undo/redo/update 时按当时
    /// 的当前层捕获（引擎内测试路径）。
    pub target_layer: Option<u64>,
    /// 记录期标志。
    recording_started: bool,
    recording_finished: bool,
    /// 事件流（`_stroke_seq`）。
    stroke_seq: Option<Stroke>,
    /// 切换用的两个状态。
    sshot_before: Option<SurfaceSnapshot>,
    sshot_after: Option<SurfaceSnapshot>,
    /// 这一笔的 strokemap 形状（撤销/重做时随快照一起摘/挂）。
    stroke_shape: Option<pigment_surface::StrokeShape>,
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

    /// 设置本段作画开始前的累计时长 —— `lib/command.py:415` 的
    /// `self._time_before = model.unsaved_painting_time`。
    ///
    /// `BrushworkState::begin` 在起笔时从 `ctx.doc` 取值；直接构造
    /// `Brushwork` 的测试路径可用它把起点接上。
    pub fn set_time_before(&mut self, time_before: f64) {
        assert!(!self.recording_started, "录制开始后不能再改起始时长");
        self.time_before = time_before.max(0.0);
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
        // `time_before` 已由 `set_time_before`（或 `BrushworkState::begin`）
        // 设好；这里不能清零，否则累计时长每笔都会从头开始。
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
        if self
            .start_recording(surface, brush, brush_name, state)
            .is_err()
        {
            return false;
        }
        if let Some(seq) = self.stroke_seq.as_mut() {
            seq.record_event(ev);
        }
        // 每个指针事件套一次原子窗口 —— 照 `lib/layer/data.py:1412-1425` 的
        // `PaintingLayer.stroke_to`（`begin_atomic` / `stroke_to` / `end_atomic`）。
        //
        // 这不只是脏区上报：对称矩阵的重算就挂在 `begin_atomic` 上
        // （`mypaint-tiled-surface.c:94-99`），所以「改了对称参数 → 下一个
        // 事件立刻生效」靠的正是这一对调用。
        surface.begin_atomic();
        let painted = ev.apply(brush, state, surface);
        surface.end_atomic();
        // 引擎的 split 请求（`mypaint_brush_stroke_to` 的返回值）
        // 交给 `ModeStack::stroke_to` 在下个事件前提交本段。
        self.split_due = state.split_due;
        state.split_due = false;
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
        // 画后快照必须包含所有已排队的 dab（GPU 后端可能还没同步）。
        surface.flush_dabs();
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
        // `add_stroke_shape`（`lib/layer/data.py:1577`）：形状从前后快照 diff 得出
        if let (Some(before), Some(after)) = (&self.sshot_before, &self.sshot_after) {
            let brush = seq.brush_settings().to_string();
            if let Some(mut shape) = pigment_surface::StrokeShape::from_snapshots(before, after) {
                shape.brush = brush;
                surface.push_stroke(shape.clone());
                self.stroke_shape = Some(shape);
            }
        }
        // 画作在记录期就已落到画布上，所以 redo() 不该再画一遍
        // （`lib/command.py:548` 的 `_sshot_after_applied = True`）
        self.sshot_after_applied = true;
        true
    }
}

const UNDEFINED_BRUSH: &str = "Undefined (command not started yet)";

impl Brushwork {
    /// 解析（并在缺省时捕获）目标层 id。
    fn resolve_target(&mut self, ctx: &mut DocContext<'_>) -> u64 {
        match self.target_layer {
            Some(id) => id,
            None => {
                let id = ctx.layers.current_id();
                self.target_layer = Some(id);
                id
            }
        }
    }
}

impl Command for Brushwork {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        assert!(self.recording_finished, "先调 stop_recording()");
        let id = self.resolve_target(ctx);
        let after = match &self.sshot_after {
            Some(a) => a,
            None => return false,
        };
        if !self.sshot_after_applied {
            if let Some(surface) = ctx.surface_for(id) {
                surface.load_snapshot(after);
                if let Some(shape) = &self.stroke_shape {
                    surface.push_stroke(shape.clone());
                }
            }
            self.sshot_after_applied = true;
        }
        *ctx.unsaved_painting_time = self.time_after;
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        assert!(self.recording_finished, "先调 stop_recording()");
        let id = self.resolve_target(ctx);
        if let Some(before) = &self.sshot_before {
            if let Some(surface) = ctx.surface_for(id) {
                surface.load_snapshot(before);
                // 撤销这一笔时把它的 strokemap 也摘掉（源码快照包含 strokes）
                if self.stroke_shape.is_some() {
                    surface.pop_stroke();
                }
            }
        }
        *ctx.unsaved_painting_time = self.time_before;
        self.sshot_after_applied = false;
    }

    fn display_name(&self) -> String {
        Brushwork::display_name(self)
    }

    /// 用另一支笔重绘上一笔（`lib/command.py:377`）。
    fn update(&mut self, ctx: &mut DocContext<'_>, update: &CommandUpdate<'_>) -> bool {
        let CommandUpdate::Brush { brush, name } = update else {
            return false;
        };
        if !self.recording_finished {
            return false;
        }
        assert!(
            self.sshot_after_applied,
            "command.Brushwork 必须先被应用才能 update"
        );
        let id = self.resolve_target(ctx);
        let Some(before) = self.sshot_before.clone() else {
            return false;
        };
        let Some(seq) = &self.stroke_seq else {
            return false;
        };
        let Ok(stroke) = seq.copy_using_different_brush(brush, name) else {
            return false;
        };
        let Some(surface) = ctx.surface_for(id) else {
            return false;
        };
        // 回到画前状态，再用新笔重放整条事件流
        surface.load_snapshot(&before);
        if self.stroke_shape.is_some() {
            surface.pop_stroke();
        }
        if !stroke.render(surface) {
            return false;
        }
        self.sshot_after = Some(surface.save_snapshot());
        if let (Some(before), Some(after)) = (&self.sshot_before, &self.sshot_after) {
            if let Some(mut shape) = pigment_surface::StrokeShape::from_snapshots(before, after) {
                shape.brush = stroke.brush_settings().to_string();
                surface.push_stroke(shape.clone());
                self.stroke_shape = Some(shape);
            }
        }
        self.stroke_seq = Some(stroke);
        true
    }
}

/// `FloodFill`（`lib/command.py:561-645`）：在当前层上做一次洪水填充。
///
/// 与源码同样的骨架：
/// - `redo()` 先抓一份目标层快照（`self.snapshot = layers.current.save_snapshot()`，
///   `command.py:619-620`），再把填充跑进目标层；
/// - `undo()` 用快照恢复内容，并把快照**清空**（`command.py:643-645`）——
///   下一次 `redo()` 会重新抓一份，这样"撤销 → 改别的东西 → 重做"不会
///   拿着一张过期的快照。
///
/// 记账差异：
/// - 未移植 `status_cb`（进度/取消对话框）：填充是同步的；
/// - 取样源图层（`src_path`）已接 GUI 与引擎：命令持稳定图层 id，`redo()`
///   时解析成路径，插层/删层/撤销重做后都不会指错；`sample_merged` 仍是
///   整份图层栈的合成。
pub struct FloodFill {
    /// 目标层 id（源码持图层对象引用，我们用稳定 id）。
    target_layer: u64,
    /// 取样源图层稳定 id（`command.py:596-599` 的 `src_layer = layers.current`）。
    ///
    /// 普通填充时等于目标层；"新建层再填"时源仍是填充前的当前层（可能是组）。
    /// 用 id 而不是路径：`redo()` 里新层会插在当前层位置，原层路径随之后移，
    /// 预先算好的路径会指到新层上。
    src_layer: u64,
    /// 填充参数（`command.py:581` 的 `self.fill_args`）。
    fill_args: crate::mode::fill::FillArgs,
    /// `sample merged`（`command.py:594-595`）：源改成整份图层栈的合成。
    sample_merged: bool,
    /// 是否写到新层（`command.py:601-616` / undo `:635-641`）。
    make_new_layer: bool,
    /// 填充前的快照（`command.py:588` 的 `self.snapshot`；undo 后清空）。
    snapshot: Option<SurfaceSnapshot>,
    /// 新建层的插入路径（undo 时按它删层；redo 时重建）。
    new_layer_path: Option<Vec<usize>>,
    /// 新建层的 id（插入后才知道；redo 里当目标）。
    new_layer_id: Option<u64>,
    /// 新建层之前的当前层（undo 恢复到这里）。
    prev_current: u64,
}

impl FloodFill {
    /// 新建一条填充命令（`FloodFill.__init__`，`command.py:566-589`）。
    pub fn new(
        target_layer: u64,
        src_layer: u64,
        fill_args: crate::mode::fill::FillArgs,
        sample_merged: bool,
        make_new_layer: bool,
    ) -> Self {
        Self {
            target_layer,
            src_layer,
            fill_args,
            sample_merged,
            make_new_layer,
            snapshot: None,
            new_layer_path: None,
            new_layer_id: None,
            prev_current: 0,
        }
    }

    /// 按源码的方式挑源，并把填充跑进 `target`。
    fn run_fill(&self, ctx: &mut DocContext<'_>, target_id: u64) {
        use crate::mode::fill::{flood_fill, flood_fill_surface, LayerSource, MergedSource};

        // 源与目标相同（普通填充）时走两阶段就地填充，避免同一表面的双重借用。
        // 指定了别的取样源时不能走这条捷径，否则源会被忽略。
        let same_source =
            !self.sample_merged && self.src_layer == target_id && self.target_layer == target_id;
        if same_source {
            if let Some(surface) = ctx.layers.surface_by_id(target_id) {
                flood_fill_surface(surface, &self.fill_args);
            }
            return;
        }
        // 其余情形都需要"源"与"目标"同时存在：克隆一份图层栈当只读源
        // （tile 像素按 Arc 共享，写目标时 COW 不会污染源快照）。
        //
        // 源路径在**插层之后**解析（`command.py:596-599`：源码持有图层对象
        // 引用，路径怎么变都指得对）。找不到源层（被别处删掉）时退回当前层。
        let src_path = ctx
            .layers
            .path_of_id(self.src_layer)
            .or_else(|| ctx.layers.path_of_id(self.target_layer))
            .unwrap_or_else(|| ctx.layers.current_path().to_vec());
        let stack = ctx.layers.clone();
        let Some(target) = ctx.layers.surface_by_id(target_id) else {
            return;
        };
        if self.sample_merged {
            let src = MergedSource::new(&stack);
            flood_fill(&src, &self.fill_args, target);
        } else {
            let src = LayerSource::new(&stack, &src_path);
            flood_fill(&src, &self.fill_args, target);
        }
    }
}

impl Command for FloodFill {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        // `command.py:591-623`
        if self.make_new_layer {
            // `command.py:601-616`：新建一层插在当前层位置（即当前层之上），
            // 切到新层，再用它当目标。
            if self.new_layer_path.is_none() {
                self.prev_current = ctx.layers.current_id();
                let insert = ctx.layers.current_path().to_vec();
                let layer = pigment_surface::layer::Layer::new();
                let id = layer.id;
                let Some(p) = ctx.layers.insert_at(&insert, layer) else {
                    return false;
                };
                self.new_layer_id = Some(id);
                self.new_layer_path = Some(p.clone());
                ctx.layers.set_current_path(&p);
            }
            let target_id = self.new_layer_id.expect("上面刚插入");
            // `command.py:615-616`：锁 alpha / dst-in 等模式在空层上没有意义
            if self.fill_args.skip_empty_dst() {
                return true;
            }
            self.run_fill(ctx, target_id);
            return true;
        }

        let Some(surface) = ctx.layers.surface_by_id(self.target_layer) else {
            // 目标不是绘画层（组）：无从填充 → 按"取消"处理，不入栈
            return false;
        };
        // 只抓一次快照（`assert self.snapshot is None`，command.py:619）
        if self.snapshot.is_none() {
            self.snapshot = Some(surface.save_snapshot());
        }
        self.run_fill(ctx, self.target_layer);
        // 源码 `redo()` 没有返回值，等价于"不是取消"→ 一定入栈
        // （`command.py:70` 的 `completed is False` 判据）。空填充也入栈，
        // 与源码一致：撤销栈里可能出现一条内容无变化的 Flood Fill。
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        // `command.py:633-645`
        if self.make_new_layer {
            if let Some(path) = self.new_layer_path.take() {
                ctx.layers.remove_at(&path);
            }
            self.new_layer_id = None;
            // 记账：源码 undo 用 `set_current_path(旧的新层路径)`（删层后会被
            // 修剪到父路径）；这里恢复成填充前的当前层，语义更接近用户预期，
            // 也与 `AddLayer.undo` 一致。
            ctx.layers.set_current_by_id(self.prev_current);
            return;
        }
        if let Some(snapshot) = self.snapshot.take() {
            if let Some(surface) = ctx.layers.surface_by_id(self.target_layer) {
                surface.load_snapshot(&snapshot);
            }
        }
    }

    fn display_name(&self) -> String {
        // `display_name = _("Flood Fill")`（`command.py:564`）
        crate::mode::fill::DISPLAY_NAME.to_string()
    }
}

// ---- 图层命令族（`lib/command.py:971-1560`）----
//
// 均按图层 id 寻址（等价 MyPaint 持图层对象引用）。属性类命令在 redo 时
// 捕获旧值、undo 恢复（照源码 `_old_xxx = layer.x; layer.x = new` 的次序）。

/// `AddLayer`（`lib/command.py:971`）：在 `insert_path` 处插入一层并把当前层切过去。
///
/// `insert_path` 是 **deepinsert 路径**：前几段指向存在的组，最后一段是插入索引
/// （可越界，按 `list.insert` 钳位）。
pub struct AddLayer {
    layer: Option<Layer>,
    insert_path: Vec<usize>,
    inserted_path: Vec<usize>,
    prev_current: u64,
    is_import: bool,
}

impl AddLayer {
    pub fn new(insert_path: Vec<usize>, layer: Layer, is_import: bool) -> Self {
        Self {
            layer: Some(layer),
            insert_path,
            inserted_path: Vec::new(),
            prev_current: 0,
            is_import,
        }
    }
}

impl Command for AddLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        self.prev_current = ctx.layers.current_id();
        let layer = self.layer.take().expect("double redo()?");
        let Some(p) = ctx.layers.insert_at(&self.insert_path, layer) else {
            return false; // 父路径不是组/不存在（源码 raise IndexError）
        };
        self.inserted_path = p.clone();
        ctx.layers.set_current_path(&p);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        // 对齐源码的 deepremove(self._layer)：LIFO 保证插入的层还在原地
        if let Some(layer) = ctx.layers.remove_at(&self.inserted_path) {
            self.layer = Some(layer);
        }
        ctx.layers.set_current_by_id(self.prev_current);
    }

    fn display_name(&self) -> String {
        // command.py:1003-1010：导入用 "Import Layers"，否则 "Add {DEFAULT_NAME}"
        if self.is_import {
            "Import Layers".to_string()
        } else if self.layer.as_ref().map(|l| l.is_group()).unwrap_or(false) {
            "Add Group".to_string() // group.py:63 LayerStack.DEFAULT_NAME = "Group"
        } else if self.layer.as_ref().map(|l| l.is_vector()).unwrap_or(false) {
            "Add Vector Layer".to_string() // data.py:1173 VectorLayer.DEFAULT_NAME
        } else {
            "Add Layer".to_string() // data.py PaintingLayer.DEFAULT_NAME = "Layer"
        }
    }
}

/// `LoadLayer`（`lib/command.py:734`）：把一张现成的表面作为**新图层**插入，
/// 用于 Paste Layer / Import Layers（源码 `to_new_layer=True`）。
///
/// 插入/撤销直接复用 [`AddLayer`]，因此撤销栈里仍是一条命令。
pub struct LoadLayer {
    add: AddLayer,
    label: &'static str,
}

impl LoadLayer {
    pub fn new(insert_path: Vec<usize>, layer: Layer, label: &'static str) -> Self {
        Self {
            add: AddLayer::new(insert_path, layer, false),
            label,
        }
    }
}

impl Command for LoadLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        self.add.redo(ctx)
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        self.add.undo(ctx);
    }

    fn display_name(&self) -> String {
        self.label.to_string()
    }
}

/// `RemoveLayer`（`lib/command.py:1025`）：移除构造时的当前层；
/// **根栈**被清空时补一层新的（`CREATE_PAINTING_LAYER_IF_EMPTY`；组可以空着）。
pub struct RemoveLayer {
    target_id: u64,
    path: Vec<usize>,
    removed: Option<Layer>,
    /// 根栈被清空时补进来的替代层 id（`CREATE_PAINTING_LAYER_IF_EMPTY`）。
    replacement_id: Option<u64>,
}

impl RemoveLayer {
    pub fn new(ctx: &mut DocContext<'_>) -> Self {
        Self {
            target_id: ctx.layers.current_id(),
            path: ctx.layers.current_path().to_vec(),
            removed: None,
            replacement_id: None,
        }
    }
}

impl Command for RemoveLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        assert!(self.removed.is_none(), "double redo()?");
        let Some(removed) = ctx.layers.remove_at(&self.path) else {
            return false;
        };
        self.removed = Some(removed);
        if ctx.layers.is_empty() {
            let repl = Layer::new();
            self.replacement_id = Some(repl.id);
            ctx.layers.insert(0, repl);
        }
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(rid) = self.replacement_id.take() {
            ctx.layers.remove_by_id(rid);
        }
        let removed = self.removed.take().expect("undo 先于 redo？");
        ctx.layers.insert_at(&self.path, removed);
        ctx.layers.set_current_by_id(self.target_id);
    }

    fn display_name(&self) -> String {
        "Remove Layer".to_string()
    }
}

/// `SelectLayer`（`lib/command.py:1072`）：切换当前层，**自动撤销**类。
pub struct SelectLayer {
    prev_id: u64,
    new_id: u64,
}

impl SelectLayer {
    pub fn new(prev_id: u64, new_id: u64) -> Self {
        Self { prev_id, new_id }
    }
}

impl Command for SelectLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        ctx.layers.set_current_by_id(self.new_id);
        true
    }
    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        ctx.layers.set_current_by_id(self.prev_id);
    }
    fn display_name(&self) -> String {
        "Select Layer".to_string()
    }
    fn automatic_undo(&self) -> bool {
        true
    }
}

/// `DuplicateLayer`（`lib/command.py:1202`）：原位复制当前层（**深拷贝子树**）。
///
/// `deepinsert(self._path, layer_copy)` —— 副本落在原层的位置上，
/// 原层被推到其下；当前层切到副本。像素是 COW 共享（MyPaint 用
/// deepcopy 急拷贝；可观察行为一致，首笔作画才真正分离）。
pub struct DuplicateLayer {
    /// 原层路径（`_path`）：插入/移除都用它。
    path: Vec<usize>,
    /// 被复制的原层 id —— undo 后当前层要回到它（源码靠「路径不变 → 路径上
    /// 的层又变回原层」实现，我们按 id 寻址，须显式设回）。
    orig_id: u64,
    /// 本次 redo 造出的副本 id（undo 用它移除）。
    copy_id: u64,
}

impl DuplicateLayer {
    pub fn new(ctx: &mut DocContext<'_>) -> Self {
        Self {
            path: ctx.layers.current_path().to_vec(),
            orig_id: ctx.layers.current_id(),
            copy_id: 0,
        }
    }
}

impl Command for DuplicateLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        // 源码每次 redo 都重新 `deepcopy(layers.current)`：撤销后又在原层上
        // 画了东西，再 redo 得到的副本**包含**那些新笔迹（缓存旧副本会丢）。
        let Some(mut copy) = ctx.layers.layer_at(&self.path).cloned() else {
            return false;
        };
        copy.id = pigment_surface::layer::allocate_layer_id();
        self.copy_id = copy.id;
        let Some(p) = ctx.layers.insert_at(&self.path, copy) else {
            return false;
        };
        // `set_current_path(self._path)`：当前层 = 副本
        ctx.layers.set_current_path(&p);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        ctx.layers.remove_by_id(self.copy_id);
        // 源码路径不变 → 当前层落回原层
        ctx.layers.set_current_by_id(self.orig_id);
    }

    fn display_name(&self) -> String {
        "Duplicate Layer".to_string()
    }
}

/// `BubbleLayerUp` / `BubbleLayerDown`（`lib/command.py:1226/1240`）。
///
/// "Up" = 朝栈顶（同层内 index 0 方向）。完整照搬 `tree.py:1380 _bubble_layer`：
/// 同栈内换位、到端点**出组**、相邻兄弟是组时**入组**。
/// 撤销 = 反方向再冒一次（源码如此，两端语义对称）。
pub struct BubbleLayer {
    id: u64,
    up: bool,
}

impl BubbleLayer {
    pub fn up(id: u64) -> Self {
        Self { id, up: true }
    }
    pub fn down(id: u64) -> Self {
        Self { id, up: false }
    }

    /// `_bubble_layer`；`up = true` 朝栈顶。返回是否真的动了。
    fn swap(&self, ctx: &mut DocContext<'_>, up: bool) -> bool {
        let Some(path) = ctx.layers.path_of_id(self.id) else {
            return false;
        };
        let moved = ctx.layers.bubble_layer(&path, up);
        if moved {
            ctx.layers.set_current_by_id(self.id);
        }
        moved
    }
}

impl Command for BubbleLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        self.swap(ctx, self.up)
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        // 换位是自逆的：反方向再换一次
        self.swap(ctx, !self.up);
    }

    fn display_name(&self) -> String {
        if self.up {
            "Move Layer Up".to_string()
        } else {
            "Move Layer Down".to_string()
        }
    }
}

/// `AddLayerView`（`lib/layervis.py:555-583`）：把当前可见层存成一个新视图。
pub struct AddLayerView {
    name: Option<String>,
    prev_active: Option<usize>,
    applied: bool,
}

impl AddLayerView {
    pub fn new(name: Option<String>) -> Self {
        Self {
            name,
            prev_active: None,
            applied: false,
        }
    }
}

impl Command for AddLayerView {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        if !self.applied {
            self.prev_active = ctx.views.active_index();
        }
        ctx.views.add_new(self.name.as_deref(), ctx.layers);
        self.applied = true;
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        ctx.views.remove_active(self.prev_active, ctx.layers);
        self.applied = false;
    }

    fn display_name(&self) -> String {
        "Add Layer View".to_string()
    }
}

/// `RemoveActiveLayerView`（`lib/layervis.py:586-608`）。
pub struct RemoveActiveLayerView {
    removed: Option<(usize, crate::layer_views::LayerView)>,
}

impl RemoveActiveLayerView {
    pub fn new() -> Self {
        Self { removed: None }
    }
}

impl Command for RemoveActiveLayerView {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(removed) = ctx.views.remove_active(None, ctx.layers) else {
            return false;
        };
        self.removed = Some(removed);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some((index, view)) = self.removed.take() {
            ctx.views.insert_view(index, view, ctx.layers);
        }
    }

    fn display_name(&self) -> String {
        "Remove Layer View".to_string()
    }
}

/// `RenameActiveLayerView`（`lib/layervis.py:611-637`）。
pub struct RenameActiveLayerView {
    new_name: String,
    old_name: Option<String>,
}

impl RenameActiveLayerView {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            new_name: name.into(),
            old_name: None,
        }
    }
}

impl Command for RenameActiveLayerView {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some((old, _)) = ctx.views.rename_active(&self.new_name) else {
            return false;
        };
        self.old_name = Some(old);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(old) = self.old_name.take() {
            ctx.views.rename_active(&old);
        }
    }

    fn display_name(&self) -> String {
        "Rename Layer View".to_string()
    }
}

/// `ActivateLayerView`（`lib/layervis.py:640-669`）。
pub struct ActivateLayerView {
    name: Option<String>,
    prev: Option<String>,
    applied: bool,
}

impl ActivateLayerView {
    pub fn new(name: Option<String>) -> Self {
        Self {
            name,
            prev: None,
            applied: false,
        }
    }
}

impl Command for ActivateLayerView {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        if !self.applied {
            self.prev = ctx.views.current_name().map(str::to_string);
        }
        let ok = ctx.views.activate_by_name(self.name.as_deref(), ctx.layers);
        self.applied = ok;
        ok
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        ctx.views.activate_by_name(self.prev.as_deref(), ctx.layers);
        self.applied = false;
    }

    fn display_name(&self) -> String {
        match &self.name {
            Some(name) => format!("Activate Layer View “{name}”"),
            None => "Deactivate Layer View".to_string(),
        }
    }
}

/// `SetActiveLayerViewLocked`（`lib/layervis.py:672-700`）。
pub struct SetActiveLayerViewLocked {
    locked: bool,
    old: Option<bool>,
}

impl SetActiveLayerViewLocked {
    pub fn new(locked: bool) -> Self {
        Self { locked, old: None }
    }
}

impl Command for SetActiveLayerViewLocked {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        self.old = Some(ctx.views.current_locked());
        ctx.views.set_locked(self.locked)
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(old) = self.old.take() {
            ctx.views.set_locked(old);
        }
    }

    fn display_name(&self) -> String {
        if self.locked {
            "Lock Layer View".to_string()
        } else {
            "Unlock Layer View".to_string()
        }
    }
}

/// `RenameLayer`（`lib/command.py:1384`）。
pub struct RenameLayer {
    id: u64,
    new_name: Option<String>,
    old_name: Option<Option<String>>,
}

impl RenameLayer {
    pub fn new(id: u64, new_name: Option<String>) -> Self {
        Self {
            id,
            new_name,
            old_name: None,
        }
    }
}

impl Command for RenameLayer {
    fn layer_prop(&self) -> Option<(LayerProp, u64)> {
        Some((LayerProp::Name, self.id))
    }
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.id) else {
            return false;
        };
        if self.old_name.is_none() {
            self.old_name = Some(layer.name.clone());
        }
        layer.name = self.new_name.clone();
        true
    }
    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            layer.name = self.old_name.clone().flatten();
        }
    }
    /// `RenameLayer.update(name)`（`lib/command.py:1410-1412`）：只改新名，
    /// **不动 `_old_name`**（撤销仍回到最初的名字）。
    fn update(&mut self, ctx: &mut DocContext<'_>, update: &CommandUpdate<'_>) -> bool {
        let CommandUpdate::Rename(name) = update else {
            return false;
        };
        let name = if name.is_empty() {
            None
        } else {
            Some((*name).to_string())
        };
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            layer.name = name.clone();
        }
        self.new_name = name;
        true
    }
    fn display_name(&self) -> String {
        "Rename Layer".to_string()
    }
}

/// `SetLayerVisibility`（`lib/command.py:1415`）。
pub struct SetLayerVisibility {
    id: u64,
    new_visible: bool,
    old: Option<bool>,
}

impl SetLayerVisibility {
    pub fn new(id: u64, visible: bool) -> Self {
        Self {
            id,
            new_visible: visible,
            old: None,
        }
    }
}

impl Command for SetLayerVisibility {
    fn layer_prop(&self) -> Option<(LayerProp, u64)> {
        Some((LayerProp::Visibility, self.id))
    }
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.id) else {
            return false;
        };
        if self.old.is_none() {
            self.old = Some(layer.visible);
        }
        layer.visible = self.new_visible;
        // 可见性影响合成 → 必须抬高合成版本号，否则 GPU 侧不会重传
        // （`LayerStack::composite_revision` 是显存水位线）。
        ctx.layers.touch();
        true
    }
    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            layer.visible = self.old.unwrap_or(true);
        }
        ctx.layers.touch();
    }
    fn update(&mut self, ctx: &mut DocContext<'_>, update: &CommandUpdate<'_>) -> bool {
        let CommandUpdate::LayerVisibility(visible) = update else {
            return false;
        };
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            layer.visible = *visible;
        }
        self.new_visible = *visible;
        ctx.layers.touch();
        true
    }
    fn display_name(&self) -> String {
        if self.new_visible {
            "Make Layer Visible".to_string()
        } else {
            "Make Layer Invisible".to_string()
        }
    }
}

/// `SetLayerLocked`（`lib/command.py:1450`）。
pub struct SetLayerLocked {
    id: u64,
    new_locked: bool,
    old: Option<bool>,
}

impl SetLayerLocked {
    pub fn new(id: u64, locked: bool) -> Self {
        Self {
            id,
            new_locked: locked,
            old: None,
        }
    }
}

impl Command for SetLayerLocked {
    fn layer_prop(&self) -> Option<(LayerProp, u64)> {
        Some((LayerProp::Locked, self.id))
    }
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.id) else {
            return false;
        };
        if self.old.is_none() {
            self.old = Some(layer.locked);
        }
        layer.locked = self.new_locked;
        true
    }
    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            layer.locked = self.old.unwrap_or(false);
        }
    }
    fn update(&mut self, ctx: &mut DocContext<'_>, update: &CommandUpdate<'_>) -> bool {
        let CommandUpdate::LayerLocked(locked) = update else {
            return false;
        };
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            layer.locked = *locked;
        }
        self.new_locked = *locked;
        true
    }
    fn display_name(&self) -> String {
        if self.new_locked {
            "Lock Layer".to_string()
        } else {
            "Unlock Layer".to_string()
        }
    }
}

/// `SetLayerOpacity`（`lib/command.py:1490`）。
pub struct SetLayerOpacity {
    id: u64,
    new_opacity: f32,
    old: Option<f32>,
}

impl SetLayerOpacity {
    pub fn new(id: u64, opacity: f32) -> Self {
        Self {
            id,
            new_opacity: opacity,
            old: None,
        }
    }
}

impl Command for SetLayerOpacity {
    fn layer_prop(&self) -> Option<(LayerProp, u64)> {
        Some((LayerProp::Opacity, self.id))
    }
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.id) else {
            return false;
        };
        if self.old.is_none() {
            self.old = Some(layer.opacity);
        }
        // 走设置器：直通组的 opacity 不可改（core.py:281-293）
        layer.set_opacity(self.new_opacity);
        ctx.layers.touch();
        true
    }
    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            let old = self.old.unwrap_or(1.0);
            layer.set_opacity(old);
        }
        ctx.layers.touch();
    }
    fn update(&mut self, ctx: &mut DocContext<'_>, update: &CommandUpdate<'_>) -> bool {
        let CommandUpdate::LayerOpacity(opacity) = update else {
            return false;
        };
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            layer.set_opacity(*opacity);
        }
        self.new_opacity = *opacity;
        ctx.layers.touch();
        true
    }
    fn display_name(&self) -> String {
        format!("Set Layer Opacity: {:.1}%", self.new_opacity * 100.0)
    }
}

/// `SetLayerMode`（`lib/command.py:1528`）。注意 undo 同时恢复不透明度
/// （源码就是这么写的——为 NormalizeLayerMode 的联动留的口子）。
pub struct SetLayerMode {
    id: u64,
    new_mode: LayerMode,
    old: Option<(LayerMode, f32)>,
}

impl SetLayerMode {
    pub fn new(id: u64, mode: LayerMode) -> Self {
        Self {
            id,
            new_mode: mode,
            old: None,
        }
    }
}

impl Command for SetLayerMode {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.id) else {
            return false;
        };
        if self.old.is_none() {
            self.old = Some((layer.mode, layer.opacity));
        }
        // 照 `core.py:473-494` 的设置器语义（直通强制 opacity=1、非法回落默认）
        layer.set_mode(self.new_mode);
        ctx.layers.touch();
        true
    }
    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.id) {
            if let Some((mode, opacity)) = self.old {
                // 顺序照 `command.py:1557-1560`：先 mode 后 opacity
                // （pass-through 下 opacity 会被拒，必须先把 mode 换回来）
                layer.set_mode(mode);
                layer.set_opacity(opacity);
            }
        }
        ctx.layers.touch();
    }
    fn display_name(&self) -> String {
        format!("Set Layer Mode: {}", self.new_mode.label())
    }
}

/// `MergeLayerDown`（`lib/command.py:889`）：当前层并入**下一个兄弟**。
///
/// 结构照 `tree.py:2003 get_merge_down_target`：目标 = `path[:-1] + (path[-1]+1,)`，
/// 换言之只在**同一父层内**往下并；两层都须可归一化
/// （`core.py:644 get_mode_normalizable`：零 alpha 有作用/会减背板 alpha 的
/// 模式不行）且未锁。
///
/// 像素语义：逐 tile 把两层各自**自己的 op**（`Layer::compose_tile_into`——
/// 包含组隔离/直通与层自身模式、有效不透明度）按「先下后上」叠到新层；
/// 新层模式照 `:2092-2095`（上层是 SpectralWGM → SpectralWGM，否则 Normal）。
/// 记账：MyPaint 的 `layer_new_normalized` 还会渲染并**减去背板**
/// （对 Multiply 这类依赖背板的模式才严格等价），本实现取「自身 op 叠合」的
/// 近似——视觉结果在两层之间一致，与更下方背板的交互可能不同。
pub struct MergeLayerDown {
    path: Vec<usize>,
    target_path: Vec<usize>,
    upper_id: u64,
    merged: Option<Layer>,
    upper_layer: Option<Layer>,
    lower_layer: Option<Layer>,
}

/// `core.py:644 get_mode_normalizable`：零 alpha 有作用或会减背板 alpha 的模式
/// 不能归一化（`modes.py:197/205` 的两个集合）。
pub fn normalizable(layer: &Layer) -> bool {
    match layer.mode.combine() {
        Some(mode) => !mode.zero_alpha_has_effect() && !mode.can_decrease_alpha(),
        // 直通组（mode=-1）不在两个集合里 → 可归一化（`tree.py:1946-1947`）
        None => true,
    }
}

impl MergeLayerDown {
    pub fn new(ctx: &mut DocContext<'_>) -> Option<Self> {
        let path = ctx.layers.current_path().to_vec();
        let (index, parent) = path.split_last()?;
        let mut target_path = parent.to_vec();
        target_path.push(index + 1);
        let upper = ctx.layers.layer_at(&path)?;
        let lower = ctx.layers.layer_at(&target_path)?;
        // 两端都须未锁（含祖先锁）且可归一化
        if ctx.layers.branch_locked(&path) || ctx.layers.branch_locked(&target_path) {
            return None;
        }
        if !normalizable(upper) || !normalizable(lower) {
            return None;
        }
        Some(Self {
            path,
            target_path,
            upper_id: upper.id,
            merged: None,
            upper_layer: None,
            lower_layer: None,
        })
    }
}

/// 交互式移动图层（`lib/command.py:1093` 的 `MoveLayer`）。
///
/// 源码在拖拽期间只改表面，提交时把命令入栈；这里用前后快照实现
/// 撤销/重做，并同时保存 strokemap（平移会改形状坐标）。
pub struct MoveLayer {
    layer_id: u64,
    before: SurfaceSnapshot,
    after: SurfaceSnapshot,
    before_strokes: Vec<StrokeShape>,
    after_strokes: Vec<StrokeShape>,
    /// 撤销栈显示的动词。移动是 `Move Layer`，裁剪是源码独立的
    /// `TrimLayer` 命令（`command.py:648`，显示名 `Trim Layer`）。
    label: &'static str,
}

impl MoveLayer {
    pub fn new(
        layer_id: u64,
        before: SurfaceSnapshot,
        after: SurfaceSnapshot,
        before_strokes: Vec<StrokeShape>,
        after_strokes: Vec<StrokeShape>,
    ) -> Self {
        Self::with_label(
            layer_id,
            before,
            after,
            before_strokes,
            after_strokes,
            "Move Layer",
        )
    }

    fn with_label(
        layer_id: u64,
        before: SurfaceSnapshot,
        after: SurfaceSnapshot,
        before_strokes: Vec<StrokeShape>,
        after_strokes: Vec<StrokeShape>,
        label: &'static str,
    ) -> Self {
        Self {
            layer_id,
            before,
            after,
            before_strokes,
            after_strokes,
            label,
        }
    }

    /// 同 [`MoveLayer::new`] 的前后快照形态，但用于 `TrimLayer` 命令的显示名。
    pub fn trim(
        layer_id: u64,
        before: SurfaceSnapshot,
        after: SurfaceSnapshot,
        before_strokes: Vec<StrokeShape>,
        after_strokes: Vec<StrokeShape>,
    ) -> Self {
        Self::with_label(
            layer_id,
            before,
            after,
            before_strokes,
            after_strokes,
            "Trim Layer",
        )
    }
}

impl Command for MoveLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(surface) = ctx.layers.surface_by_id(self.layer_id) else {
            return false;
        };
        surface.load_snapshot(&self.after);
        surface.replace_strokes(self.after_strokes.clone());
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        let Some(surface) = ctx.layers.surface_by_id(self.layer_id) else {
            return;
        };
        surface.load_snapshot(&self.before);
        surface.replace_strokes(self.before_strokes.clone());
    }

    fn display_name(&self) -> String {
        self.label.to_string()
    }
}

/// 外部编辑的类型（决定临时文件怎么读回）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalEditKind {
    /// 位图：PNG 像素放回 `(x, y)`（对应 `PaintingLayer.load_from_external_edit_tempfile`）。
    Raster { x: i32, y: i32 },
    /// 矢量：SVG 文本重新栅格化（对应 `FileBackedLayer.load_from_external_edit_tempfile`）。
    Vector,
}

/// `ExternalLayerEdit`（`lib/command.py:1615-1639`）：外部应用改完的图层。
///
/// 第一次 redo 读临时文件并保存 after 快照；undo/redo 之后只换图层快照，
/// 与源码 `_before/_after` 的缓存语义一致。图层用 id 寻址（等价 weakref）。
pub struct ExternalLayerEdit {
    layer_id: u64,
    tmpfile: std::path::PathBuf,
    kind: ExternalEditKind,
    before: Option<Layer>,
    after: Option<Layer>,
}

impl ExternalLayerEdit {
    pub fn new(
        layer_id: u64,
        tmpfile: impl Into<std::path::PathBuf>,
        kind: ExternalEditKind,
    ) -> Self {
        Self {
            layer_id,
            tmpfile: tmpfile.into(),
            kind,
            before: None,
            after: None,
        }
    }

    fn load_into(&self, layer: &mut Layer) -> Result<(), String> {
        match self.kind {
            ExternalEditKind::Raster { x, y } => {
                let bytes = std::fs::read(&self.tmpfile)
                    .map_err(|e| format!("读临时文件 {} 失败：{e}", self.tmpfile.display()))?;
                let (px, w, h) =
                    pigment_surface::png::decode_rgba8_png(&bytes).map_err(|e| e.to_string())?;
                let surface = layer
                    .surface_mut()
                    .ok_or_else(|| "外部编辑目标不是表面层".to_string())?;
                surface.clear();
                surface.replace_from_rgba8(&px, x, y, w, h);
                Ok(())
            }
            ExternalEditKind::Vector => {
                let svg = std::fs::read_to_string(&self.tmpfile)
                    .map_err(|e| format!("读临时文件 {} 失败：{e}", self.tmpfile.display()))?;
                layer.set_vector_svg(svg)
            }
        }
    }
}

impl Command for ExternalLayerEdit {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.layer_id) else {
            return false;
        };
        if self.before.is_none() {
            self.before = Some(layer.clone());
        }
        if let Some(after) = &self.after {
            *layer = after.clone();
            return true;
        }
        let mut updated = layer.clone();
        match self.load_into(&mut updated) {
            Ok(()) => {
                *layer = updated;
                self.after = Some(layer.clone());
                true
            }
            Err(e) => {
                eprintln!("ExternalLayerEdit: {e}");
                false
            }
        }
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        let Some(before) = &self.before else {
            return;
        };
        if let Some(layer) = ctx.layers.layer_by_id_mut(self.layer_id) {
            *layer = before.clone();
        }
    }

    fn display_name(&self) -> String {
        "Edit Layer Externally".to_string() // command.py:1618
    }
}

/// `ClearLayer`（`lib/command.py:714`）：清空当前层内容，保留层对象/属性。
///
/// 绘画层清 surface + strokemap；组清空子层（`group.py:198-204`）。用整层
/// COW 克隆做前后快照，所以组的子树也能一次撤销恢复。
pub struct ClearLayer {
    layer_id: u64,
    before: Layer,
    after: Layer,
}

impl ClearLayer {
    pub fn new(layer: &Layer) -> Self {
        let before = layer.clone();
        let mut after = before.clone();
        after.clear_content();
        Self {
            layer_id: layer.id,
            before,
            after,
        }
    }
}

impl Command for ClearLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.layer_id) else {
            return false;
        };
        *layer = self.after.clone();
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        let Some(layer) = ctx.layers.layer_by_id_mut(self.layer_id) else {
            return;
        };
        *layer = self.before.clone();
    }

    fn display_name(&self) -> String {
        "Clear Layer".to_string()
    }
}

/// `NormalizeLayerMode`（`lib/command.py:933-968`）：把当前层/组替换成外观
/// 等价、全不透明、Normal（光谱源除外）的普通绘画层。
pub struct NormalizeLayerMode {
    path: Vec<usize>,
    before: Layer,
    after: Layer,
    old_current: Vec<usize>,
}

impl NormalizeLayerMode {
    pub fn new(path: Vec<usize>, before: Layer, after: Layer, old_current: Vec<usize>) -> Self {
        Self {
            path,
            before,
            after,
            old_current,
        }
    }
}

impl Command for NormalizeLayerMode {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(slot) = ctx.layers.layer_at_mut(&self.path) else {
            return false;
        };
        *slot = self.after.clone();
        ctx.layers.set_current_path(&self.path);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(slot) = ctx.layers.layer_at_mut(&self.path) {
            *slot = self.before.clone();
        }
        ctx.layers.set_current_path(&self.old_current);
    }

    fn display_name(&self) -> String {
        "Normalize Layer Mode".to_string()
    }
}

/// `MergeVisibleLayers`（`lib/command.py:809-886`）：删除所有可见层，并在
/// 最高可见根层的位置插入 `layer_new_merge_visible()` 的结果。
///
/// 用整棵 `LayerStack` 的 COW 快照做 undo/redo；图层对象与 tile Arc 共享。
pub struct MergeVisibleLayers {
    before: LayerStack,
    after: LayerStack,
}

impl MergeVisibleLayers {
    pub fn new(before: LayerStack, after: LayerStack) -> Self {
        Self { before, after }
    }
}

impl Command for MergeVisibleLayers {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        *ctx.layers = self.after.clone();
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        *ctx.layers = self.before.clone();
    }

    fn display_name(&self) -> String {
        "Merge Visible Layers".to_string()
    }
}

/// `UniqLayer`（`lib/command.py:668-689`）：删除不改变背板的 tile/像素。
///
/// 源码 `display_name` 对 tiles/pixels 两种模式都是 "Uniquify Layer Pixels"；
/// 前后整层 COW 快照用于撤销。
pub struct UniqLayer {
    path: Vec<usize>,
    before: Layer,
    after: Layer,
}

impl UniqLayer {
    pub fn new(path: Vec<usize>, before: Layer, after: Layer) -> Self {
        Self {
            path,
            before,
            after,
        }
    }
}

impl Command for UniqLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(slot) = ctx.layers.layer_at_mut(&self.path) else {
            return false;
        };
        *slot = self.after.clone();
        ctx.layers.set_current_path(&self.path);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(slot) = ctx.layers.layer_at_mut(&self.path) {
            *slot = self.before.clone();
        }
        ctx.layers.set_current_path(&self.path);
    }

    fn display_name(&self) -> String {
        "Uniquify Layer Pixels".to_string()
    }
}

/// `RefactorGroup`（`lib/command.py:691-712`）：把组内公共 tile/像素抽到
/// `Common` 层。前后整组 COW 快照用于撤销。
pub struct RefactorGroup {
    path: Vec<usize>,
    before: Layer,
    after: Layer,
}

impl RefactorGroup {
    pub fn new(path: Vec<usize>, before: Layer, after: Layer) -> Self {
        Self {
            path,
            before,
            after,
        }
    }
}

impl Command for RefactorGroup {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let Some(slot) = ctx.layers.layer_at_mut(&self.path) else {
            return false;
        };
        *slot = self.after.clone();
        ctx.layers.set_current_path(&self.path);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        if let Some(slot) = ctx.layers.layer_at_mut(&self.path) {
            *slot = self.before.clone();
        }
        ctx.layers.set_current_path(&self.path);
    }

    fn display_name(&self) -> String {
        "Refactor Group".to_string()
    }
}

/// 画框矩形更新（`lib/command.py` 的 `UpdateFrame`，由 `Document.update_frame`
/// 在 `user_initiated=True` 时入栈）。拖拽期间只改模型，松手时提交一条。
pub struct UpdateFrame {
    before: (i32, i32, i32, i32),
    after: (i32, i32, i32, i32),
}

impl UpdateFrame {
    pub fn new(before: (i32, i32, i32, i32), after: (i32, i32, i32, i32)) -> Self {
        Self { before, after }
    }
}

impl Command for UpdateFrame {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        *ctx.frame = self.after;
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        *ctx.frame = self.before;
    }

    fn display_name(&self) -> String {
        "Update Frame".to_string()
    }
}

impl Command for MergeLayerDown {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        let merged = match self.merged.take() {
            Some(m) => m,
            None => {
                // 首次执行：现场计算合并结果
                let upper = ctx.layers.layer_at(&self.path).expect("上层丢失").clone();
                let lower = ctx
                    .layers
                    .layer_at(&self.target_path)
                    .expect("下层丢失")
                    .clone();
                let merged_mode = if upper.mode.combine() == Some(CombineMode::SpectralWGM) {
                    LayerMode::Standard(CombineMode::SpectralWGM)
                } else {
                    LayerMode::Standard(CombineMode::Normal)
                };
                let mut merged = Layer::new();
                merged.set_mode(merged_mode);
                // 合并名字：照 tree.py:2097-2106（取有意义的名字，", " 连接）
                let names: Vec<String> = [lower.name.clone(), upper.name.clone()]
                    .into_iter()
                    .flatten()
                    .collect();
                if !names.is_empty() {
                    merged.name = Some(names.join(", "));
                }
                // 逐 tile 叠合：先下层后上层，各用自身 op（模式/不透明度都在内）
                let mut coords: Vec<(i32, i32)> = lower.tile_coords();
                for c in upper.tile_coords() {
                    if !coords.contains(&c) {
                        coords.push(c);
                    }
                }
                let mut buf = vec![0u16; 64 * 64 * 4];
                let mut acc = vec![0u16; 64 * 64 * 4];
                for (tx, ty) in coords {
                    acc.fill(0);
                    let mut has_alpha = true;
                    let mut any = false;
                    for layer in [&lower, &upper] {
                        if !layer.compose_tile_into(tx, ty, &mut buf) {
                            continue;
                        }
                        pigment_surface::combine::tile_combine(
                            CombineMode::Normal,
                            &buf,
                            &mut acc,
                            has_alpha,
                            1.0,
                        );
                        has_alpha = true;
                        any = true;
                    }
                    if any {
                        if let Some(surface) = merged.surface_mut() {
                            surface.paste_tile(tx, ty, &acc);
                        }
                    }
                }
                merged
            }
        };
        // 弹出两层、在同一位置插入合并层（照源码次序：先 lower 再 upper）
        self.lower_layer = ctx.layers.remove_at(&self.target_path);
        self.upper_layer = ctx.layers.remove_at(&self.path);
        let Some(p) = ctx.layers.insert_at(&self.path, merged) else {
            return false;
        };
        let merged_id = ctx.layers.layer_at(&p).map(|l| l.id).unwrap_or(0);
        ctx.layers.set_current_by_id(merged_id);
        true
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        // 取出合并层，塞回两层（同一位置：先 lower 再 upper）
        let merged = ctx.layers.remove_at(&self.path);
        self.merged = merged;
        let lower = self.lower_layer.take().expect("undo 先于 redo？");
        let upper = self.upper_layer.take().expect("undo 先于 redo？");
        ctx.layers.insert_at(&self.target_path, lower);
        ctx.layers.insert_at(&self.path, upper);
        ctx.layers.set_current_by_id(self.upper_id);
    }

    fn display_name(&self) -> String {
        "Merge Down".to_string()
    }
}

/// `RestackLayer`（`lib/command.py:1254`）：把一层挪到另一个位置（可跨组）。
///
/// 目标路径是 **deepinsert 语义**：前几段指向组，最后一段是插入索引。
/// 用「占位层」保索引稳定（照 `command.py:1310-1337`）：先把源位置换成一个
/// 空占位层，插入移动层，再删占位层——否则目标在同父层下方时索引会漂。
///
/// 记账：源码「目标父是数据层 → 现场新建父组」的分支未移植（面板拖动只允许
/// 拖进组或拖到行间）；`ValueError` 的三种校验改为 `new` 返回 `None`。
pub struct RestackLayer {
    before: LayerStack,
    after: Option<LayerStack>,
    moved_id: u64,
    orig_targ: Vec<usize>,
}

/// 新父组的唯一名字（`tree.py:1197-1213` 的 `get_unique_name` +
/// `lib/naming.py:36-81` 的 `make_unique_name`）。
fn unique_group_name(layers: &LayerStack) -> String {
    let existing: std::collections::HashSet<&str> = layers
        .walk()
        .into_iter()
        .filter_map(|(_, l)| l.name.as_deref())
        .collect();
    if !existing.contains("Group") {
        return "Group".to_string();
    }
    for n in 1.. {
        let name = format!("Group {n}");
        if !existing.contains(name.as_str()) {
            return name;
        }
    }
    unreachable!()
}

impl RestackLayer {
    pub fn new(
        ctx: &mut DocContext<'_>,
        src_path: Vec<usize>,
        targ_path: Vec<usize>,
    ) -> Option<Self> {
        if targ_path.is_empty() || targ_path.starts_with(&src_path) {
            return None; // 不能挪进自己/自己内部，也不能挪到根
        }
        let moved = ctx.layers.layer_at(&src_path)?;
        let parent = &targ_path[..targ_path.len() - 1];
        if !parent.is_empty() && ctx.layers.layer_at(parent).is_none() {
            return None;
        }
        Some(Self {
            before: ctx.layers.clone(),
            after: None,
            moved_id: moved.id,
            orig_targ: targ_path,
        })
    }

    /// 执行一次移动；目标父不是组时现场建组（`command.py:1310-1337`）。
    fn apply(&self, ctx: &mut DocContext<'_>) -> bool {
        let src = self.src_path_for_apply();
        let targ = self.targ_path_for_apply();
        let Some(moved) = ctx.layers.remove_at(&src) else {
            return false;
        };
        // 占位层保索引（照 command.py:1310-1314）
        let placeholder = Layer::new();
        let ph_id = placeholder.id;
        if ctx.layers.insert_at(&src, placeholder).is_none() {
            return false;
        }
        let parent_path = &targ[..targ.len() - 1];
        let parent_is_group = ctx
            .layers
            .layer_at(parent_path)
            .map(|l| l.is_group())
            .unwrap_or(false);
        let ok = if parent_is_group {
            ctx.layers.insert_at(&targ, moved).is_some()
        } else {
            // 目标父是数据层：把目标层和移动层装进新组，组放在目标层原位置。
            let Some(target_layer) = ctx.layers.remove_at(parent_path) else {
                ctx.layers.remove_by_id(ph_id);
                return false;
            };
            let mut group = Layer::new_group();
            group.name = Some(unique_group_name(ctx.layers));
            let group_path = parent_path.to_vec();
            if ctx.layers.insert_at(&group_path, group).is_none() {
                let _ = ctx.layers.insert_at(parent_path, target_layer);
                ctx.layers.remove_by_id(ph_id);
                return false;
            }
            // 组内顺序：移动层在上，目标层在下。
            let mut target_path = group_path.clone();
            target_path.push(0);
            if ctx.layers.insert_at(&target_path, target_layer).is_none() {
                ctx.layers.remove_by_id(ph_id);
                return false;
            }
            if ctx.layers.insert_at(&target_path, moved).is_none() {
                ctx.layers.remove_by_id(ph_id);
                return false;
            }
            true
        };
        ctx.layers.remove_by_id(ph_id);
        if ok {
            if let Some(p) = ctx.layers.path_of_id(self.moved_id) {
                ctx.layers.set_current_path(&p);
            }
        }
        ok
    }

    fn src_path_for_apply(&self) -> Vec<usize> {
        self.before.path_of_id(self.moved_id).unwrap_or_default()
    }

    fn targ_path_for_apply(&self) -> Vec<usize> {
        // `new` 保存的原始目标路径；用 before 里的路径定位目标父。
        self.orig_targ.clone()
    }
}
impl Command for RestackLayer {
    fn redo(&mut self, ctx: &mut DocContext<'_>) -> bool {
        if let Some(after) = self.after.clone() {
            *ctx.layers = after;
            if let Some(p) = ctx.layers.path_of_id(self.moved_id) {
                ctx.layers.set_current_path(&p);
            }
            return true;
        }
        let ok = self.apply(ctx);
        if ok {
            self.after = Some(ctx.layers.clone());
        }
        ok
    }

    fn undo(&mut self, ctx: &mut DocContext<'_>) {
        *ctx.layers = self.before.clone();
        if let Some(p) = ctx.layers.path_of_id(self.moved_id) {
            ctx.layers.set_current_path(&p);
        }
    }

    fn display_name(&self) -> String {
        "Move Layer in Stack".to_string()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushlib::default_brush;
    use crate::Document;
    use hokusai::BrushSetting;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn test_brush(radius: f32) -> Brush {
        let mut b = default_brush();
        b.get_mut(BrushSetting::Radius).base_value = radius;
        b
    }

    fn ctx<'a>(stack: &'a mut LayerStack, time: &'a mut f64) -> DocContext<'a> {
        // 测试辅助：多数命令不碰视图，泄漏一个空管理器即可。
        let views: &'a mut crate::layer_views::LayerViews =
            Box::leak(Box::new(crate::layer_views::LayerViews::default()));
        let frame: &'a mut (i32, i32, i32, i32) = Box::leak(Box::new((0, 0, 0, 0)));
        DocContext {
            layers: stack,
            views,
            unsaved_painting_time: time,
            frame,
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
        let mut st = LayerStack::new();
        let mut t = 0.0;
        assert!(stack.do_command(FakeCommand::new(&log, "A"), &mut ctx(&mut st, &mut t)));
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
        let mut st = LayerStack::new();
        let mut t = 0.0;

        stack.do_command(FakeCommand::new(&log, "A"), &mut ctx(&mut st, &mut t));
        stack.do_command(FakeCommand::new(&log, "B"), &mut ctx(&mut st, &mut t));
        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 1);

        let mut cancelling = FakeCommand::new(&log, "C");
        cancelling.cancel = true;
        assert!(!stack.do_command(cancelling, &mut ctx(&mut st, &mut t)));

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
        assert_eq!(
            stack.redo_len(),
            0,
            "redo 栈要先被丢弃（A 之后被完好的 B 挡着）"
        );
    }

    #[test]
    fn undo_and_redo_move_commands_between_stacks() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut stack = CommandStack::new(4);
        let mut st = LayerStack::new();
        let mut t = 0.0;
        stack.do_command(FakeCommand::new(&log, "A"), &mut ctx(&mut st, &mut t));
        stack.do_command(FakeCommand::new(&log, "B"), &mut ctx(&mut st, &mut t));

        assert_eq!(
            stack
                .undo(&mut ctx(&mut st, &mut t))
                .map(|o| o.display_name)
                .as_deref(),
            Some("B")
        );
        assert_eq!(stack.next_redo_name().as_deref(), Some("B"));
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(
            stack
                .redo(&mut ctx(&mut st, &mut t))
                .map(|o| o.display_name)
                .as_deref(),
            Some("B")
        );
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
        let mut st = LayerStack::new();
        let mut t = 0.0;
        assert!(stack.undo(&mut ctx(&mut st, &mut t)).is_none());
        assert!(stack.redo(&mut ctx(&mut st, &mut t)).is_none());
    }

    /// 上限修剪：最旧的命令被丢掉（`lib/command.py:124`）。
    #[test]
    fn reduce_undo_history_trims_oldest() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut stack = CommandStack::new(2);
        let mut st = LayerStack::new();
        let mut t = 0.0;
        for name in ["A", "B", "C"] {
            stack.do_command(FakeCommand::new(&log, name), &mut ctx(&mut st, &mut t));
        }
        assert_eq!(stack.undo_len(), 2);
        assert_eq!(stack.get_last_command().unwrap().display_name(), "C");
        assert_eq!(
            stack
                .undo(&mut ctx(&mut st, &mut t))
                .map(|o| o.display_name)
                .as_deref(),
            Some("C")
        );
        assert_eq!(
            stack
                .undo(&mut ctx(&mut st, &mut t))
                .map(|o| o.display_name)
                .as_deref(),
            Some("B")
        );
        assert!(
            stack.undo(&mut ctx(&mut st, &mut t)).is_none(),
            "A 已被丢弃"
        );
    }

    #[test]
    fn stack_requires_positive_size() {
        assert!(std::panic::catch_unwind(|| CommandStack::new(0)).is_err());
    }

    /// 用真实笔刷在栈的当前层画一笔，验证 Brushwork 的完整生命周期。
    fn paint_a_stroke(stack: &mut LayerStack) -> Brushwork {
        let brush = test_brush(2.0);
        let mut state = BrushState::default();
        let mut bw = Brushwork::new();
        bw.target_layer = Some(stack.current_id());
        let surface = &mut stack
            .current_mut()
            .surface_mut()
            .expect("Test: 当前层必须是绘画层");
        for i in 0..40 {
            let ev = StrokeEvent::new(0.016, i as f32 * 2.0, 20.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            bw.stroke_to(surface, &brush, "Test Brush", &mut state, ev);
        }
        assert!(bw.stop_recording(surface, &state, false));
        bw
    }

    #[test]
    fn brushwork_undo_redo_restores_pixels() {
        let mut st = LayerStack::new();
        let mut t = 0.0;
        let bw = paint_a_stroke(&mut st);
        assert!(
            st.current()
                .surface()
                .expect("Test: 当前层必须是绘画层")
                .tile_count()
                > 0,
            "应先画出内容"
        );
        let painted: Vec<_> = st
            .current()
            .surface()
            .unwrap()
            .tiles()
            .map(|(c, tile)| (c, tile.pixels().to_vec()))
            .collect();

        let mut stack = CommandStack::new(8);
        assert!(stack.do_command(Box::new(bw), &mut ctx(&mut st, &mut t)));
        assert!(t > 0.0, "redo 应把作画时长写进 unsaved_painting_time");

        // 撤销 → 画布清空
        assert!(stack.undo(&mut ctx(&mut st, &mut t)).is_some());
        assert_eq!(
            st.current()
                .surface()
                .expect("Test: 当前层必须是绘画层")
                .tile_count(),
            0,
            "撤销后不该有像素"
        );
        assert_eq!(t, 0.0);

        // 重做 → 像素逐个还原
        assert!(stack.redo(&mut ctx(&mut st, &mut t)).is_some());
        assert_eq!(
            st.current()
                .surface()
                .expect("Test: 当前层必须是绘画层")
                .tile_count(),
            painted.len()
        );
        for (coord, px) in &painted {
            assert_eq!(
                st.current()
                    .surface()
                    .expect("Test: 当前层必须是绘画层")
                    .tile(coord.0, coord.1)
                    .map(|t| t.to_vec()),
                Some(px.clone()),
                "重做应逐像素还原 {coord:?}"
            );
        }
    }

    #[test]
    fn brushwork_display_name_uses_stroke_metadata() {
        let mut st = LayerStack::new();
        let bw = paint_a_stroke(&mut st);
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
        let mut st = LayerStack::new();
        let mut bw = Brushwork::new();
        bw.start_recording(
            &st.current().surface().expect("Test: 当前层必须是绘画层"),
            &brush,
            "Test",
            &state,
        )
        .unwrap();
        assert!(
            !bw.stop_recording(
                &mut st
                    .current_mut()
                    .surface_mut()
                    .expect("Test: 当前层必须是绘画层"),
                &state,
                false
            ),
            "空笔画返回 false"
        );
        assert!(bw.stroke().unwrap().is_empty());
    }

    /// `revert = true` 要把画布退回起笔前（`brushwork_rollback`）。
    #[test]
    fn revert_rolls_back_the_stroke() {
        let brush = test_brush(2.0);
        let mut state = BrushState::default();
        let mut st = LayerStack::new();
        let mut bw = Brushwork::new();
        let surface = &mut st
            .current_mut()
            .surface_mut()
            .expect("Test: 当前层必须是绘画层");
        for i in 0..40 {
            let ev = StrokeEvent::new(0.016, i as f32 * 2.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            bw.stroke_to(surface, &brush, "Test", &mut state, ev);
        }
        assert!(surface.tile_count() > 0);
        assert!(!bw.stop_recording(surface, &state, true));
        assert_eq!(surface.tile_count(), 0, "revert 后画布应回到起笔前");
    }

    /// 未保存作画时长要跨命令累计，undo 回到 `_time_before`
    /// （`lib/command.py:415/544-545/366/374`）。
    #[test]
    fn painting_time_accumulates_and_undo_restores_it() {
        let brush = test_brush(2.0);
        let mut state = BrushState::default();
        let mut doc = Document::new();

        let paint_once = |doc: &mut Document, state: &mut BrushState| {
            let mut bw = Brushwork::new();
            bw.set_time_before(doc.unsaved_painting_time);
            let surface = doc.paintable_surface_mut().unwrap();
            for i in 0..40 {
                let ev = StrokeEvent::new(0.016, i as f32 * 2.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
                bw.stroke_to(surface, &brush, "Test", state, ev);
            }
            let changed = bw.stop_recording(doc.paintable_surface_mut().unwrap(), state, false);
            assert!(changed);
            doc.do_brushwork(bw);
        };

        paint_once(&mut doc, &mut state);
        let first = doc.unsaved_painting_time;
        assert!(first > 0.5, "第一笔应累计 ~0.64s，实得 {first}");
        paint_once(&mut doc, &mut state);
        assert!(
            doc.unsaved_painting_time > first + 0.5,
            "第二笔要接在第一笔之后：{} → {}",
            first,
            doc.unsaved_painting_time
        );
        let second = doc.unsaved_painting_time;
        doc.undo();
        assert_eq!(
            doc.unsaved_painting_time, first,
            "undo 要把时长退回本笔的 _time_before"
        );
        doc.redo();
        assert_eq!(doc.unsaved_painting_time, second);
    }

    // ---- 图层命令（B2-3）----

    fn solid_stack() -> LayerStack {
        // 两层：0=红（顶），1=空（底），模式 Normal
        let mut st = LayerStack::new();
        st.layer_mut(0)
            .set_mode(pigment_surface::layer::LayerMode::Standard(
                pigment_surface::combine::CombineMode::Normal,
            ));
        let mut red = Layer::new();
        red.set_mode(pigment_surface::layer::LayerMode::Standard(
            pigment_surface::combine::CombineMode::Normal,
        ));
        let mut px = Vec::new();
        for _ in 0..64 * 64 {
            px.extend_from_slice(&[255, 0, 0, 255]);
        }
        red.surface_mut()
            .unwrap()
            .load_from_rgba8(&px, 0, 0, 64, 64);
        st.insert(0, red);
        st
    }

    #[test]
    fn add_layer_undo_redo_restores_structure() {
        let mut st = solid_stack();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let top_id = st.layer(0).id;
        st.set_current_index(0);

        assert!(stack.do_command(
            Box::new(AddLayer::new(vec![0], Layer::new(), false)),
            &mut ctx(&mut st, &mut t)
        ));
        assert_eq!(st.len(), 3);
        assert_ne!(st.current_id(), top_id, "当前层应切到新层");

        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.len(), 2);
        assert_eq!(st.current_id(), top_id, "撤销应恢复当前层");

        stack.redo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.len(), 3);
    }

    #[test]
    fn remove_layer_undo_restores_pixels() {
        let mut st = solid_stack();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let red_id = st.layer(0).id;
        let red_tiles = st.layer(0).surface().unwrap().tile_count();
        assert!(red_tiles > 0);
        st.set_current_index(0);

        let cmd = RemoveLayer::new(&mut ctx(&mut st, &mut t));
        assert!(stack.do_command(Box::new(cmd), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.len(), 1, "红层被移除");
        assert!(!st.layer_by_id(red_id).is_some());

        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.len(), 2);
        let restored = st.layer_by_id(red_id).expect("撤销应恢复红层");
        assert_eq!(restored.surface().unwrap().tile_count(), red_tiles);
        assert_eq!(st.current_id(), red_id, "撤销后当前层回到被恢复的层");
    }

    #[test]
    fn remove_last_layer_replaces_with_fresh_one() {
        let mut st = LayerStack::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        // 只有一层：移除后应补一个新的空层
        st.set_current_index(0);
        let cmd = RemoveLayer::new(&mut ctx(&mut st, &mut t));
        assert!(stack.do_command(Box::new(cmd), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.len(), 1, "栈不应为空（CREATE_PAINTING_LAYER_IF_EMPTY）");
        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.len(), 1, "撤销应换回原层");
    }

    #[test]
    fn select_layer_is_automatic_undo() {
        let mut st = solid_stack();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let top = st.layer(0).id;
        let bottom = st.layer(1).id;

        let cmd = SelectLayer::new(top, bottom);
        assert!(stack.do_command(Box::new(cmd), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.current_id(), bottom);

        // Document.undo 会连跳 automatic_undo 命令——这里直接验证标志
        let outcome = stack.undo(&mut ctx(&mut st, &mut t)).unwrap();
        assert!(outcome.automatic_undo);
        assert_eq!(st.current_id(), top);
    }

    #[test]
    fn bubble_layer_up_moves_toward_top() {
        let mut st = solid_stack();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let bottom = st.layer(1).id;
        let top = st.layer(0).id;

        assert!(stack.do_command(Box::new(BubbleLayer::up(bottom)), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.layer(0).id, bottom, "上移后应到栈顶");
        assert_eq!(st.layer(1).id, top);
        assert_eq!(st.current_id(), bottom);

        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.layer(0).id, top);
        assert_eq!(st.layer(1).id, bottom);

        // 顶端层再上移 → 取消，不入栈
        let before = stack.undo_len();
        assert!(!stack.do_command(Box::new(BubbleLayer::up(top)), &mut ctx(&mut st, &mut t)));
        assert_eq!(stack.undo_len(), before, "取消的命令不入栈");
    }

    #[test]
    fn unique_group_names_follow_mypaint_numbering() {
        let mut st = LayerStack::new();
        assert_eq!(unique_group_name(&st), "Group");
        let mut g = Layer::new_group();
        g.name = Some("Group".to_string());
        st.insert(0, g);
        assert_eq!(unique_group_name(&st), "Group 1");
        let mut g2 = Layer::new_group();
        g2.name = Some("Group 1".to_string());
        st.insert(0, g2);
        assert_eq!(unique_group_name(&st), "Group 2");
    }

    /// 跨组冒泡：出组/入组后，`undo` 用反方向冒泡必须精确还原结构。
    #[test]
    fn restack_into_data_layer_creates_parent_group_and_undoes() {
        let a = Layer::new();
        let b = Layer::new();
        let c = Layer::new();
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);
        let mut st = LayerStack::from_children(vec![a, b, c]);
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        st.set_current_by_id(c_id);

        let cmd = RestackLayer::new(&mut ctx(&mut st, &mut t), vec![2], vec![1, 0])
            .expect("target parent is a data layer");
        assert!(stack.do_command(Box::new(cmd), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.len(), 2, "root should be [a, new group]");
        assert_eq!(st.path_of_id(a_id), Some(vec![0]));
        let group = st.layer(1);
        assert!(group.is_group());
        assert_eq!(st.path_of_id(c_id), Some(vec![1, 0]));
        assert_eq!(st.path_of_id(b_id), Some(vec![1, 1]));
        assert_eq!(st.current_id(), c_id);

        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.len(), 3);
        assert_eq!(st.path_of_id(a_id), Some(vec![0]));
        assert_eq!(st.path_of_id(b_id), Some(vec![1]));
        assert_eq!(st.path_of_id(c_id), Some(vec![2]));
    }
    #[test]
    fn bubble_layer_crosses_group_and_undo_restores() {
        let a = Layer::new();
        let mut g = Layer::new_group();
        let b = Layer::new();
        let c = Layer::new();
        g.stack_mut().unwrap().insert(0, b.clone());
        g.stack_mut().unwrap().insert(1, c.clone());
        let gid = g.id;
        let mut st = LayerStack::from_children(vec![a.clone(), g]);
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        st.set_current_by_id(b.id);

        // 组内顶层 B 上移 → 出组 [A, B, G[C]]
        assert!(stack.do_command(Box::new(BubbleLayer::up(b.id)), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.path_of_id(b.id), Some(vec![1]));
        assert_eq!(st.path_of_id(c.id), Some(vec![2, 0]));
        assert_eq!(st.current_id(), b.id);

        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.path_of_id(b.id), Some(vec![1, 0]));
        assert_eq!(st.path_of_id(c.id), Some(vec![1, 1]));
        assert_eq!(st.path_of_id(gid), Some(vec![1]));
        assert_eq!(st.current_id(), b.id);

        // 根层 A 下移 → 入组 [G[A, B, C]]
        assert!(stack.do_command(Box::new(BubbleLayer::down(a.id)), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.path_of_id(a.id), Some(vec![0, 0]));
        assert_eq!(st.path_of_id(b.id), Some(vec![0, 1]));

        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.path_of_id(a.id), Some(vec![0]));
        assert_eq!(st.path_of_id(b.id), Some(vec![1, 0]));
        assert_eq!(st.path_of_id(gid), Some(vec![1]));
        assert_eq!(st.current_id(), a.id);
    }

    #[test]
    fn set_layer_opacity_mode_roundtrip() {
        let mut st = solid_stack();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let id = st.layer(0).id;

        assert!(stack.do_command(
            Box::new(SetLayerOpacity::new(id, 0.5)),
            &mut ctx(&mut st, &mut t)
        ));
        assert_eq!(st.layer_by_id(id).unwrap().opacity, 0.5);

        assert!(stack.do_command(
            Box::new(SetLayerMode::new(
                id,
                LayerMode::Standard(pigment_surface::combine::CombineMode::Multiply)
            )),
            &mut ctx(&mut st, &mut t)
        ));
        assert_eq!(
            st.layer_by_id(id).unwrap().mode,
            LayerMode::Standard(pigment_surface::combine::CombineMode::Multiply)
        );

        stack.undo(&mut ctx(&mut st, &mut t)); // 撤销模式
        assert_eq!(
            st.layer_by_id(id).unwrap().mode,
            LayerMode::Standard(pigment_surface::combine::CombineMode::Normal)
        );
        stack.undo(&mut ctx(&mut st, &mut t)); // 撤销不透明度
        assert_eq!(st.layer_by_id(id).unwrap().opacity, 1.0);

        // 展示名
        assert_eq!(
            SetLayerOpacity::new(id, 0.345).display_name(),
            "Set Layer Opacity: 34.5%"
        );
        assert_eq!(
            SetLayerMode::new(
                id,
                LayerMode::Standard(pigment_surface::combine::CombineMode::SpectralWGM)
            )
            .display_name(),
            "Set Layer Mode: Pigment"
        );
    }

    #[test]
    fn merge_down_composites_and_undo_restores() {
        let mut st = solid_stack(); // 0=红(顶) 1=空(底)
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);

        st.set_current_index(0);
        let cmd = MergeLayerDown::new(&mut ctx(&mut st, &mut t)).expect("应可合并");
        assert!(stack.do_command(Box::new(cmd), &mut ctx(&mut st, &mut t)));
        assert_eq!(st.len(), 1, "两层并一层");
        let merged = st.layer(0);
        assert!(
            merged.surface().unwrap().tile_count() > 0,
            "合并层应携带红层的内容"
        );

        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(st.len(), 2);
        assert!(
            st.layer(0).surface().unwrap().tile_count() > 0,
            "上层应带着像素回来"
        );
        assert_eq!(st.current_id(), st.layer(0).id);
    }

    #[test]
    fn multiple_brushworks_undo_in_order() {
        let mut st = LayerStack::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        for _ in 0..3 {
            let bw = paint_a_stroke(&mut st);
            stack.do_command(Box::new(bw), &mut ctx(&mut st, &mut t));
        }
        assert_eq!(stack.undo_len(), 3);
        for _ in 0..3 {
            stack.undo(&mut ctx(&mut st, &mut t));
        }
        assert_eq!(
            st.current()
                .surface()
                .expect("Test: 当前层必须是绘画层")
                .tile_count(),
            0,
            "三次撤销后应回到空画布"
        );
    }

    /// 撤销之后再画一笔：redo 栈被清空（新命令顶掉旧的分支）。
    #[test]
    fn new_command_after_undo_clears_redo_stack() {
        let mut st = LayerStack::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let bw = paint_a_stroke(&mut st);
        stack.do_command(Box::new(bw), &mut ctx(&mut st, &mut t));
        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(stack.redo_len(), 1);

        let bw2 = paint_a_stroke(&mut st);
        stack.do_command(Box::new(bw2), &mut ctx(&mut st, &mut t));
        assert_eq!(stack.redo_len(), 0, "新命令必须清掉 redo 分支");
    }

    /// `Brushwork.update`：换一支笔重绘上一笔（笔刷编辑器的 Live Update）。
    #[test]
    fn update_rerenders_last_stroke_with_another_brush() {
        let mut st = LayerStack::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let bw = paint_a_stroke(&mut st);
        stack.do_command(Box::new(bw), &mut ctx(&mut st, &mut t));
        let before: Vec<(i32, i32)> = st
            .current()
            .surface()
            .expect("Test: 当前层必须是绘画层")
            .tiles()
            .map(|(c, _)| c)
            .collect();

        // 换成半径更大的笔刷重绘
        let fatter = test_brush(4.0);
        let name = stack.update_last_command(
            &mut ctx(&mut st, &mut t),
            &CommandUpdate::Brush {
                brush: &fatter,
                name: "Fatter",
            },
        );
        assert!(name.is_some());
        let after: Vec<(i32, i32)> = st
            .current()
            .surface()
            .expect("Test: 当前层必须是绘画层")
            .tiles()
            .map(|(c, _)| c)
            .collect();
        assert_ne!(before, after, "更大的笔刷应铺到更多 tile");
        let display = stack.get_last_command().unwrap().display_name();
        assert!(
            display.ends_with("with Fatter"),
            "换笔后展示名应跟着变：{display}"
        );

        // 更新后的状态仍可撤销回起笔前
        stack.undo(&mut ctx(&mut st, &mut t));
        assert_eq!(
            st.current()
                .surface()
                .expect("Test: 当前层必须是绘画层")
                .tile_count(),
            0
        );
    }

    /// 不能更新已经被撤销掉的命令：此时命令在 redo 栈上，`undo()` 清掉了
    /// `_sshot_after_applied`，源码在此断言失败（`lib/command.py:381-383`）。
    ///
    /// 对应应用层的保护：笔刷编辑器的 Live Update 只作用于 `get_last_command()`
    /// （撤销栈顶），撤销后栈顶换人，所以走不到这里。本测试钉住这条不变量。
    #[test]
    fn update_after_undo_is_rejected_by_assertion() {
        let mut st = LayerStack::new();
        let mut t = 0.0;
        let mut stack = CommandStack::new(8);
        let bw = paint_a_stroke(&mut st);
        stack.do_command(Box::new(bw), &mut ctx(&mut st, &mut t));
        // 撤销 → 命令移到 redo 栈，且标记为"未应用"
        stack.undo(&mut ctx(&mut st, &mut t));
        let fatter = test_brush(4.0);
        let cmd = stack.redo_stack.back_mut().unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cmd.update(
                    &mut ctx(&mut st, &mut t),
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
        let mut st = LayerStack::new();
        let mut t = 0.0;
        let bw = paint_a_stroke(&mut st);
        let mut bw = bw;
        // 把 _sshot_after_applied 清掉：模拟"命令尚未应用"
        bw.sshot_after_applied = false;
        let fatter = test_brush(4.0);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bw.update(
                &mut ctx(&mut st, &mut t),
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
