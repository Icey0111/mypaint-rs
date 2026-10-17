//! 交互模式框架 —— `gui/mode.py` 的逐块移植。
//!
//! 为什么在引擎层：MyPaint 的 mode 只依赖 `Document` + 笔刷状态 + 指针事件，
//! 不依赖 GTK。放到 `pigment-engine` 里就能被单元测试直接驱动（推/弹栈、
//! 弹簧加载的修饰键、brushwork 的原子提交），GUI 只负责把 egui 事件投影成
//! [`PointerEvent`] 再喂给栈顶模式。
//!
//! 对应关系（源码 → 本模块）：
//! - `Behavior`（`mode.py:63`）→ [`Behavior`] 位集合；
//! - `InteractionMode`（`:151`）→ [`InteractionMode`] trait；
//! - `BrushworkModeMixin`（`:659`）→ [`BrushworkState`]（组合替继承）；
//! - `SingleClickMode` / `DragMode` / `OneshotDragMode`（`:919/962/1277`）→
//!   [`SingleClickMode`]、[`DragMode`]、[`OneshotDragMode`]；
//! - `ModeStack`（`:1327`）→ [`ModeStack`]；
//! - `FreehandMode`（`gui/freehand.py`）→ [`freehand::FreehandMode`]；
//! - `LineModeBase`（`gui/linemode.py`）→ [`line::LineMode`]；
//! - `FloodFillMode`（`gui/fill.py`）→ [`fill::FillMode`]；
//! - `ColorPickMode`（`gui/colorpicker.py`）→ [`pick::ColorPickMode`]；
//! - `BrushResizeMode`（`gui/brushmanip.py`）→ [`brushresize::BrushResizeMode`]。
//!
//! 记账差异：
//! - GTK 的 `pointer_grab`/`keyboard_grab`/`grab-broken` 没有对应物：egui 的
//!   `drag` 语义本身就把指针"抓"在起手控件上，`grab` 失败的分支不会发生，
//!   所以 `_bailout()` / `_tdw_grab_broken_cb` 不移植；
//! - `current_position()` 读的是 `doc.tdw` 的指针位置，本实现由调用方传进来；
//! - `GLib.idle_add` 的排队处理改成显式的 [`ModeStack::process_pending`]，
//!   由 GUI 每帧调用一次（等价于 idle 优先级回调）。

pub mod brushresize;
pub mod fill;
pub mod frameedit;
pub mod freehand;
pub mod ink;
pub mod layermove;
pub mod line;
pub mod pick;
pub mod symmetryedit;

use crate::hokusai::BrushState;
use crate::command::Brushwork;
use crate::StrokeEvent;
use crate::document::Document;
use crate::hokusai::Brush;

/// 模式对指针/滚轮的"大致行为分类"（`mode.py:63-84`）。
///
/// 用来做上下文相关的模式切换（`ModeStack::context_push`）和设备白名单。
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Behavior(u32);

impl Behavior {
    pub const NONE: Self = Self(0x00);
    /// 自由手绘
    pub const PAINT_FREEHAND: Self = Self(0x01);
    /// 非自由手绘（直线、透视）
    pub const PAINT_LINE: Self = Self(0x02);
    /// 移动/调整屏幕上的对象
    pub const EDIT_OBJECTS: Self = Self(0x04);
    /// 移动视口
    pub const CHANGE_VIEW: Self = Self(0x08);
    /// 与笔刷无关的绘画（填充）
    pub const PAINT_NOBRUSH: Self = Self(0x10);

    /// 依赖笔刷的绘画（freehand | line）
    pub const PAINT_BRUSH: Self = Self(Self::PAINT_FREEHAND.0 | Self::PAINT_LINE.0);
    /// 受约束的绘画（line | nobrush）
    pub const PAINT_CONSTRAINED: Self = Self(Self::PAINT_LINE.0 | Self::PAINT_NOBRUSH.0);
    /// 非绘画类
    pub const NON_PAINTING: Self = Self(Self::EDIT_OBJECTS.0 | Self::CHANGE_VIEW.0);
    /// 全部绘画类
    pub const ALL_PAINTING: Self =
        Self(Self::PAINT_FREEHAND.0 | Self::PAINT_CONSTRAINED.0);
    /// 全部
    pub const ALL: Self = Self(Self::NON_PAINTING.0 | Self::ALL_PAINTING.0);

    /// 是否包含 `other` 的任意一位。
    #[inline]
    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// 是否包含 `other` 的全部位。
    #[inline]
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// 位模式（调试/日志用）。
    #[inline]
    pub fn bits(self) -> u32 {
        self.0
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Behavior {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// 修饰键状态（`Gdk.ModifierType` 里我们关心的三位）。
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
    };

    /// 是否按住了任意修饰键。
    #[inline]
    pub fn any(self) -> bool {
        self.shift || self.ctrl || self.alt
    }

    /// 交集（`DragMode.key_release_cb` 的 `modifiers & initial_modifiers`）。
    #[inline]
    pub fn intersects(self, other: Self) -> bool {
        (self.shift && other.shift) || (self.ctrl && other.ctrl) || (self.alt && other.alt)
    }
}

/// 键盘事件里我们关心的键（`mode.py` 只对 space 有特殊处理）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Space,
    Escape,
    Enter,
    Shift,
    Control,
    Alt,
    Other,
}

/// 指针事件（`Gdk.EventButton` / `EventMotion` 的引擎侧投影）。
///
/// 坐标是**模型坐标**（`display_to_model` 之后），时间单位毫秒。
/// 压力/倾角/桶旋转用 `Option`：`None` 表示设备没提供该轴，交给
/// [`freehand::PressureAndTiltInterpolator`] 插值。
#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    pub time_ms: f64,
    pub x: f32,
    pub y: f32,
    pub pressure: Option<f32>,
    pub xtilt: Option<f32>,
    pub ytilt: Option<f32>,
    pub viewzoom: f32,
    pub viewrotation: f32,
    pub barrel_rotation: Option<f32>,
    /// 鼠标按键号（1 = 左键；0 = 纯移动事件）。
    pub button: u32,
    pub modifiers: Modifiers,
}

impl PointerEvent {
    /// 纯移动事件（`MotionNotify`）。
    pub fn motion(x: f32, y: f32, time_ms: f64) -> Self {
        Self {
            time_ms,
            x,
            y,
            pressure: None,
            xtilt: None,
            ytilt: None,
            viewzoom: 1.0,
            viewrotation: 0.0,
            barrel_rotation: None,
            button: 0,
            modifiers: Modifiers::NONE,
        }
    }

    /// 按键事件（`ButtonPress`/`ButtonRelease`）。
    pub fn button(x: f32, y: f32, time_ms: f64, button: u32) -> Self {
        Self {
            button,
            ..Self::motion(x, y, time_ms)
        }
    }
}

/// 模式处理输入时能用到的一切（等价 Python 里模式通过 `self.doc` 拿到的东西）。
///
/// 字段都是借用：`doc` 可变（作画、提交命令），笔刷只读，笔刷状态可变。
pub struct ModeContext<'a> {
    pub doc: &'a mut Document,
    pub brush: &'a Brush,
    pub brush_name: &'a str,
    pub brush_state: &'a mut BrushState,
    /// 鼠标/无压感设备的假压力（`app.fakepressure`，`freehand.py:439`）。
    pub fakepressure: f32,
    /// 假桶旋转（`app.fakerotation`）。
    pub fakerotation: f32,
    /// 画的时候隐藏光标（`ui.hide_cursor_while_painting`）。
    pub hide_cursor_while_painting: bool,
    /// 状态栏消息出口（模式用它报告「已填充」之类）。
    pub status: Option<&'a mut Vec<String>>,
    /// 当前画笔颜色 `[h, s, v]`（HSV，与应用的 `color` 同一通道）。
    ///
    /// 对应 `doc.app.brush_color_manager.get_color()`：取色模式在 `enter()` 里
    /// 读它当"画笔颜色"（`gui/colorpicker.py:83`），取到无彩色时保留它的色相
    /// （`:135-141`），HCY 变体要在它之上只改一个分量（`:177-250`）。
    pub color: [f32; 3],
    /// 取色出口 —— `brush_color_manager.set_color()`（`brushcolor.py:33-37`）
    /// 的等价物。
    ///
    /// 模式在引擎层，够不到应用的颜色状态，所以由 GUI 传一个槽位进来：取色
    /// 模式把新颜色（HSV `[h, s, v]`，即 `UIColor.get_hsv()` 的结果，也正是
    /// `brush.set_color_hsv()` 吃的东西）写进它，GUI 随后读走并落地到自己的
    /// 颜色状态。其它模式不碰这个字段，因此不需要任何改动；为 `None` 时取色
    /// 模式只做计算、不落地（单元测试可以直接省略）。
    pub picked_color: Option<&'a mut Option<[f32; 3]>>,
}

impl ModeContext<'_> {
    /// 当前可画面（组/锁/隐藏 → `None`）。
    #[inline]
    pub fn paintable(&mut self) -> Option<&mut pigment_surface::PigmentSurface> {
        self.doc.paintable_surface_mut()
    }

    /// 当前层是否可画（`data.py:1364 get_paintable`）。
    #[inline]
    pub fn current_is_paintable(&self) -> bool {
        let path = self.doc.layers().current_path().to_vec();
        self.doc.layers().paintable_at(&path).is_some()
    }

    /// 当前层 id（brushwork 的目标层）。
    #[inline]
    pub fn current_layer_id(&self) -> u64 {
        self.doc.layers().current_id()
    }

    /// 把取到的颜色写进出口（`BrushColorManager.set_color` 的落地端）。
    ///
    /// 没有出口（`picked_color == None`）时什么也不做。
    #[inline]
    pub fn set_picked_color(&mut self, hsv: [f32; 3]) {
        if let Some(sink) = self.picked_color.as_deref_mut() {
            *sink = Some(hsv);
        }
    }

    /// 到一个引擎事件（`StrokeEvent`）。
    pub fn stroke_event(
        &self,
        dtime: f64,
        x: f32,
        y: f32,
        pressure: f32,
        xtilt: f32,
        ytilt: f32,
        viewzoom: f32,
        viewrotation: f32,
        barrel_rotation: f32,
    ) -> StrokeEvent {
        StrokeEvent::new(
            dtime,
            x,
            y,
            pressure,
            xtilt,
            ytilt,
            viewzoom,
            viewrotation,
            barrel_rotation,
        )
    }
}

/// 一个交互模式（`mode.py:151` 的 `InteractionMode`）。
///
/// 约定：事件处理返回 `true` 表示"事件被吃掉了"（GTK 的返回值语义），
/// 未处理的事件会继续往上传递（本移植里只用作状态报告）。
pub trait InteractionMode {
    /// 短名（状态栏；`get_name()`）。
    fn name(&self) -> &'static str;
    /// 用法说明（`get_usage()`）。
    fn usage(&self) -> &str {
        ""
    }
    /// 指针行为分类（`pointer_behavior`）。
    fn pointer_behavior(&self) -> Behavior {
        Behavior::NONE
    }
    /// 滚轮行为分类（`scroll_behavior`）。
    fn scroll_behavior(&self) -> Behavior {
        Behavior::NONE
    }
    /// 关联的动作名（`resources.xml` 里的 id；`get_action()` 的来源）。
    ///
    /// GUI 用它取图标/tooltip，并据此把动作映射到模式类。
    fn action_name(&self) -> Option<&'static str> {
        None
    }

    /// 是否有一段作画正在录制（guis mode 的实时更新要跳过它）。
    fn is_painting(&self) -> bool {
        false
    }

    /// 是否支持笔刷编辑器的实时更新（`IS_LIVE_UPDATEABLE`）。
    fn is_live_updateable(&self) -> bool {
        false
    }

    /// 收下应用推来的自由手绘配置（偏好热更新的通道）。
    ///
    /// 源码的自由手绘模式在**每个事件**里现读 `tdw.app.preferences`
    /// （`gui/freehand.py:461-478`、`:280`），所以没有这个"推送"步骤；
    /// Rust 侧把配置缓存在 [`freehand::FreehandMode`] 里，必须由
    /// [`ModeStack::set_freehand_config`] 在启动/偏好变化时重推。
    /// 其余模式没有这类配置，默认什么都不做。
    fn set_freehand_config(&mut self, _config: freehand::FreehandConfig) {}

    /// 选项面板是否沿用**栈里下面那个模式**的控件
    /// （`BrushResizeMode.get_options_widget()`，`gui/brushmanip.py:296-297`）。
    fn delegates_options(&self) -> bool {
        false
    }

    /// 取走模式请求的 `radius_logarithmic`（`BrushResizeMode.drag_stop_cb`
    /// 的 `base_radius_adj.set_value(...)`，`gui/brushmanip.py:287-290`）。
    ///
    /// 模式在引擎层拿不到应用的 `radius_override`，所以把结果留在自己身上，
    /// 由 GUI 在事件后取走并落地。
    fn take_brush_radius(&mut self) -> Option<f32> {
        None
    }

    /// 笔刷尺寸覆盖层状态（`gui/brushmanip.py` 的 `BrushSizeOverlay`）。
    fn brush_size_overlay(&self) -> Option<crate::mode::brushresize::BrushSizeOverlayState> {
        None
    }

    /// 是否是"一键式拖拽模式"（`OneshotDragMode`，`mode.py:1277`）。
    ///
    /// 源码用 `isinstance(mode, OneshotDragMode)` 判断（`mode.py:1298-1300`），
    /// Rust 的 `&dyn InteractionMode` 没有运行时类型判断，所以改成这个默认
    /// `false` 的钩子：只有一键式模式覆写成 `true`，`stackable_on` 的语义
    /// 因此与源码一致（一键式模式用完回到原来那个模式）。
    fn is_oneshot(&self) -> bool {
        false
    }

    /// 能否叠在 `mode` 之上（`mode.py:279`）。
    ///
    /// 默认：笔刷类工具不能叠在笔刷类工具之上（这样用户换笔刷时
    /// 仍然停留在"最后一个对笔刷敏感的工具"上，见 issue #530）。
    fn stackable_on(&self, mode: &dyn InteractionMode) -> bool {
        if self.pointer_behavior().intersects(Behavior::PAINT_BRUSH) {
            return false;
        }
        mode.pointer_behavior().intersects(Behavior::PAINT_BRUSH)
    }

    fn enter(&mut self, _ctx: &mut ModeContext<'_>) {}
    fn leave(&mut self, _ctx: &mut ModeContext<'_>) {}
    /// 有待提交的改动时被调用（`sync_pending_changes` → `checkpoint()`）。
    fn checkpoint(&mut self, _ctx: &mut ModeContext<'_>) {}

    fn button_press(&mut self, _ctx: &mut ModeContext<'_>, _ev: &PointerEvent) -> bool {
        false
    }
    fn button_release(&mut self, _ctx: &mut ModeContext<'_>, _ev: &PointerEvent) -> bool {
        false
    }
    fn motion(&mut self, _ctx: &mut ModeContext<'_>, _ev: &PointerEvent) -> bool {
        false
    }
    fn scroll(&mut self, _ctx: &mut ModeContext<'_>, _dy: f32) -> bool {
        false
    }
    fn key_press(&mut self, _ctx: &mut ModeContext<'_>, _key: Key) -> bool {
        false
    }
    fn key_release(&mut self, _ctx: &mut ModeContext<'_>, _key: Key) -> bool {
        false
    }

    /// 松开起手按键时是否应弹栈（`OneshotDragMode` 的键盘起手，
    /// `mode.py:1256-1274`；取色模式 `r` 松开即回原工具）。
    fn pop_on_key_release(&self) -> bool {
        false
    }

    /// 一次拖拽结束后是否应弹栈（`OneshotDragMode.drag_stop_cb` 的修饰键规则，
    /// `mode.py:1302-1317`）。
    fn oneshot_pop_after_drag(&self, _modifiers: Modifiers) -> bool {
        false
    }

    /// Inking ADJUST 阶段的控制点（覆盖层绘制用；其它模式返回 `None`）。
    fn ink_nodes(&self) -> Option<&[crate::mode::ink::InkNode]> {
        None
    }

    /// 是否处于 Inking ADJUST 阶段（覆盖层只在此时显示节点）。
    fn ink_editing(&self) -> bool {
        false
    }

    /// 预亮（鼠标悬停）的 Inking 节点。
    fn ink_target_node(&self) -> Option<usize> {
        None
    }

    /// 当前在 OptionsUI 里编辑的 Inking 节点。
    fn ink_current_node(&self) -> Option<usize> {
        None
    }

    /// OptionsUI 的编辑操作（滑条/插点/删点）。
    fn ink_apply(&mut self, _ctx: &mut ModeContext<'_>, _edit: crate::mode::ink::InkNodeEdit) -> bool {
        false
    }

    /// 画布 OK 按钮：接受当前 Inking 曲线。
    fn ink_accept(&mut self, _ctx: &mut ModeContext<'_>) -> bool {
        false
    }

    /// 画布垃圾桶按钮：放弃当前 Inking 曲线。
    fn ink_reject(&mut self, _ctx: &mut ModeContext<'_>) -> bool {
        false
    }

    /// 是否正在拖节点（此时源码隐藏画布按钮）。
    fn ink_dragging(&self) -> bool {
        false
    }

    /// 填充模式的可调项（`gui/fill.py:433` 的 `FloodFillOptionsWidget`）。
    fn fill_options(&self) -> Option<crate::mode::fill::FillOptions> {
        None
    }

    /// 更新填充选项。
    fn set_fill_options(&mut self, _options: crate::mode::fill::FillOptions) {}

    /// `SimplifyNodes` 动作（`document.py:2318`）；返回删掉的节点数。
    fn simplify_nodes(&mut self, _ctx: &mut ModeContext<'_>) -> usize {
        0
    }

    /// `CullNodes` 动作（`document.py:2324`）。
    fn cull_nodes(&mut self, _ctx: &mut ModeContext<'_>) -> usize {
        0
    }

    /// 拖拽开始（`drag_start_cb`）。
    fn drag_start(&mut self, _ctx: &mut ModeContext<'_>) {}
    /// 拖拽更新（`drag_update_cb`，参数是**本帧位移**）。
    fn drag_update(&mut self, _ctx: &mut ModeContext<'_>, _dx: f32, _dy: f32) {}
    /// 拖拽结束（`drag_stop_cb`）。
    fn drag_stop(&mut self, _ctx: &mut ModeContext<'_>) {}

    /// 把排队中的输入处理掉（`GLib.idle_add` 的等价物）；返回是否还有剩余。
    fn process_pending(&mut self, _ctx: &mut ModeContext<'_>) -> bool {
        false
    }
}

/// `BrushworkModeMixin`（`mode.py:659`）的组合实现。
///
/// 负责把"一段连续作画"原子地提交到撤销栈：`begin` 开新段（自动切段时
/// 提交上一段）、`stroke_to` 喂事件、`commit` 结束并视情况入栈、
/// `rollback` 回滚（用于需要"取消这次拖拽"的模式）。
pub struct BrushworkState {
    /// 新建模式后的第一段总是 abrupt 起笔（`mode.py:726`）。
    first_begin: bool,
    /// 在录的那一段。
    active: Option<Brushwork>,
    /// 最后一次喂给笔刷的位置（`cmd.__last_pos`），abrupt 收尾要用。
    last_pos: Option<(f32, f32, f32, f32, f32, f32, f32)>,
}

impl Default for BrushworkState {
    fn default() -> Self {
        Self {
            first_begin: true,
            active: None,
            last_pos: None,
        }
    }
}

impl BrushworkState {
    /// 是否有在录的作画。
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// 最后一次喂给笔刷的位置（`cmd.__last_pos`）：
    /// `(x, y, xtilt, ytilt, viewzoom, viewrotation, barrel_rotation)`。
    #[inline]
    pub fn last_pos(&self) -> Option<(f32, f32, f32, f32, f32, f32, f32)> {
        self.last_pos
    }

    /// 开一段新作画（`brushwork_begin`）；已有段先提交。
    pub fn begin(&mut self, ctx: &mut ModeContext<'_>, abrupt: bool, description: Option<String>) {
        if self.active.is_some() {
            self.commit(ctx, abrupt);
        }
        let mut cmd = match description {
            Some(d) => Brushwork::with_description(d),
            None => Brushwork::new(),
        };
        cmd.target_layer = Some(ctx.current_layer_id());
        let _ = abrupt || self.first_begin;
        self.first_begin = false;
        self.last_pos = None;
        self.active = Some(cmd);
    }

    /// 提交（`brushwork_commit`）：只有真改动了画面才入栈。
    pub fn commit(&mut self, ctx: &mut ModeContext<'_>, abrupt: bool) {
        let Some(mut cmd) = self.active.take() else {
            return;
        };
        if abrupt {
            if let Some((x, y, xtilt, ytilt, viewzoom, viewrotation, _br)) = self.last_pos {
                // 照 `mode.py:748-765`：用零压力补一个收尾事件
                let ev = StrokeEvent::new(
                    0.0,
                    x,
                    y,
                    0.0,
                    xtilt,
                    ytilt,
                    viewzoom,
                    viewrotation,
                    0.0,
                );
                if let Some(surface) = ctx.doc.paintable_surface_mut() {
                    cmd.stroke_to(surface, ctx.brush, ctx.brush_name, ctx.brush_state, ev);
                }
            }
        }
        let changed = match ctx.doc.paintable_surface_mut() {
            Some(surface) => cmd.stop_recording(surface, ctx.brush_state, false),
            None => false,
        };
        if changed {
            ctx.doc.do_brushwork(cmd);
        }
    }

    /// 回滚（`brushwork_rollback`）：恢复到这段作画开始前的状态。
    pub fn rollback(&mut self, ctx: &mut ModeContext<'_>) {
        let Some(mut cmd) = self.active.take() else {
            return;
        };
        if let Some(surface) = ctx.doc.paintable_surface_mut() {
            cmd.stop_recording(surface, ctx.brush_state, true);
        }
    }

    /// 喂一个事件（`stroke_to`；`auto_split=True` 的路径）。
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_to(
        &mut self,
        ctx: &mut ModeContext<'_>,
        dtime: f64,
        x: f32,
        y: f32,
        pressure: f32,
        xtilt: f32,
        ytilt: f32,
        viewzoom: f32,
        viewrotation: f32,
        barrel_rotation: f32,
        auto_split: bool,
    ) {
        // 到点了就切段（`mode.py:841-845`；`auto_split=False` 时不切，
        // 直线/椭圆模式靠它在一段里画出整条几何图形）
        if auto_split && self.active.as_ref().map(|c| c.split_due).unwrap_or(false) {
            let desc = self.active.as_ref().and_then(|c| c.description.clone());
            self.commit(ctx, false);
            self.active = None;
            if desc.is_some() {
                self.begin(ctx, false, desc);
            }
        }
        if self.active.is_none() {
            self.begin(ctx, false, None);
        }
        // 不可画（组/锁/隐藏）：整段作废，事件也丢掉（`command.py:403-408`）
        let ev = StrokeEvent::new(
            dtime,
            x,
            y,
            pressure,
            xtilt,
            ytilt,
            viewzoom,
            viewrotation,
            barrel_rotation,
        );
        let Some(surface) = ctx.doc.paintable_surface_mut() else {
            self.active = None;
            return;
        };
        if let Some(cmd) = self.active.as_mut() {
            cmd.stroke_to(surface, ctx.brush, ctx.brush_name, ctx.brush_state, ev);
        }
        self.last_pos = Some((x, y, xtilt, ytilt, viewzoom, viewrotation, barrel_rotation));
    }
}

/// 拖拽状态（`DragMode` 里与 GTK 无关的那部分）。
///
/// 弹簧加载语义：进入模式时记住当时按住的修饰键，之后这些键全部松开
/// 就自动出栈（`mode.py:1263-1271`）。
#[derive(Default)]
pub struct DragState {
    pub in_drag: bool,
    pub start_x: Option<f32>,
    pub start_y: Option<f32>,
    pub last_x: Option<f32>,
    pub last_y: Option<f32>,
    pub start_button: Option<u32>,
    /// 进入时按住的修饰键（`None` = 还没记录过）。
    pub initial_modifiers: Option<Modifiers>,
    /// 用空格起手的拖拽（记录是用哪个键起的）。
    pub start_key: Option<Key>,
}

impl DragState {
    /// 开始拖拽（`_start_drag` 去掉 grab 的版本）。
    pub fn start(&mut self, x: f32, y: f32) {
        if self.in_drag {
            return;
        }
        self.start_x = Some(x);
        self.start_y = Some(y);
        self.last_x = Some(x);
        self.last_y = Some(y);
        self.in_drag = true;
    }

    /// 结束拖拽（`_stop_drag`），返回是否真的结束了一段。
    pub fn stop(&mut self) -> bool {
        if !self.in_drag {
            return false;
        }
        self.in_drag = false;
        self.start_x = None;
        self.start_y = None;
        self.last_x = None;
        self.last_y = None;
        self.start_button = None;
        self.start_key = None;
        true
    }
}

/// 单击模式基类（`mode.py:919`）。
#[derive(Default)]
pub struct SingleClickMode {
    pub drag: DragState,
}

/// 拖拽模式基类（`mode.py:962`）。
pub struct DragMode {
    pub drag: DragState,
    /// 是否在进入时忽略修饰键（键盘快捷键起手时为真）。
    pub ignore_modifiers: bool,
    /// 修饰键松开就出栈。
    pub spring_loaded: bool,
}

impl Default for DragMode {
    fn default() -> Self {
        Self {
            drag: DragState::default(),
            ignore_modifiers: false,
            spring_loaded: true,
        }
    }
}

impl DragMode {
    /// 进入时记录修饰键（`DragMode.enter`，`mode.py:1155-1190`）。
    ///
    /// 返回 `true` 表示"重新进入时发现修饰键已经松开，应该再弹一次"
    /// （源码用 idle 回调做这件事，见 `__pop_modestack_idle_cb`）。
    pub fn enter(&mut self, modifiers: Modifiers) -> bool {
        if !self.spring_loaded {
            return false;
        }
        if self.ignore_modifiers {
            self.drag.initial_modifiers = Some(Modifiers::NONE);
            return false;
        }
        match self.drag.initial_modifiers {
            None => {
                self.drag.initial_modifiers = Some(modifiers);
                false
            }
            Some(old) => old.any() && !modifiers.intersects(old),
        }
    }
}

/// 一键式拖拽模式（`mode.py:1277`）：拖完就出栈，除非按住修饰键。
pub struct OneshotDragMode {
    pub drag_mode: DragMode,
    /// 没有修饰键时保持激活（`unmodified_persist`）。
    pub unmodified_persist: bool,
    /// 临时激活（`temporary_activation`）。
    pub temporary_activation: bool,
}

impl Default for OneshotDragMode {
    fn default() -> Self {
        Self {
            drag_mode: DragMode::default(),
            unmodified_persist: true,
            temporary_activation: true,
        }
    }
}

impl OneshotDragMode {
    /// 拖拽结束时是否应该出栈（`mode.py:1302-1316`）。
    pub fn should_pop(&self, modifiers: Modifiers) -> bool {
        if !self.drag_mode.spring_loaded {
            return true;
        }
        match self.drag_mode.drag.initial_modifiers {
            Some(init) if init.any() => init.intersects(modifiers) != true,
            _ => self.temporary_activation || !self.unmodified_persist,
        }
    }
}

/// 模式栈（`mode.py:1327`）。栈顶就是当前活动模式，且**永不为空**。
pub struct ModeStack {
    stack: Vec<Box<dyn InteractionMode>>,
    /// 栈被清空时的替补（`default_mode_class`）。
    default_mode: fn() -> Box<dyn InteractionMode>,
    /// 正在跑 `checkpoint`（防递归，`mode.py:1348`）。
    syncing: bool,
}

impl ModeStack {
    /// 建一个只含默认模式的栈。
    pub fn new(default_mode: fn() -> Box<dyn InteractionMode>) -> Self {
        Self {
            stack: vec![default_mode()],
            default_mode,
            syncing: false,
        }
    }

    /// 栈深。
    #[inline]
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// 恒为 `false`（栈永不为空，照源码 `__nonzero__`）。
    #[inline]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// 栈顶（只读）。
    #[inline]
    pub fn top(&self) -> &dyn InteractionMode {
        self.stack.last().expect("模式栈永不为空").as_ref()
    }

    /// 栈顶（可变）。
    ///
    /// 注意：这里**不会**触发 `enter()`；需要补齐 `_check()` 语义时用
    /// [`ModeStack::ensure_current`]。
    #[inline]
    pub fn top_mut(&mut self) -> &mut dyn InteractionMode {
        self.stack.last_mut().expect("模式栈永不为空").as_mut()
    }

    /// 栈里是否有某个模式（按 `name()` 比较；调试/测试用）。
    pub fn names(&self) -> Vec<&'static str> {
        self.stack.iter().map(|m| m.name()).collect()
    }

    /// 选项面板应该显示哪个模式的选项。
    ///
    /// 默认就是栈顶；像 `BrushResizeMode` 这种把自己声明为
    /// [`InteractionMode::delegates_options`] 的模式会跳过，沿用下面那个
    /// 模式的控件（`gui/brushmanip.py:296-297` 的 `get_options_widget()`）。
    pub fn options_mode(&self) -> &dyn InteractionMode {
        self.stack
            .iter()
            .rev()
            .find(|m| !m.delegates_options())
            .map(|m| m.as_ref())
            .unwrap_or_else(|| self.top())
    }

    /// 栈顶是否要求"松开按键即出栈"（仅键盘起手的 Oneshot 模式）。
    pub fn top_pops_on_key_release(&self) -> bool {
        self.top().pop_on_key_release()
    }

    /// 栈顶是否应在一次拖拽后按修饰键规则出栈。
    pub fn top_pops_after_drag(&self, modifiers: Modifiers) -> bool {
        self.top().oneshot_pop_after_drag(modifiers)
    }

    /// 把自由手绘配置推给栈里**所有**模式（偏好热更新）。
    ///
    /// 源码每个事件都现读 `app.preferences`（`gui/freehand.py:461-478`），
    /// 改偏好立刻生效；本移植的配置缓存在模式对象上，所以偏好一变就必须
    /// 重推一次。默认模式与"被压上来又弹回去"的模式都在栈里，一次遍历覆盖。
    pub fn set_freehand_config(&mut self, config: freehand::FreehandConfig) {
        for mode in &mut self.stack {
            mode.set_freehand_config(config);
        }
    }

    /// 压入一个模式：先 `leave()` 旧栈顶，再 `enter()` 新模式（`push`）。
    pub fn push(&mut self, mut mode: Box<dyn InteractionMode>, ctx: &mut ModeContext<'_>) {
        if let Some(old) = self.stack.last_mut() {
            old.leave(ctx);
        }
        mode.enter(ctx);
        self.stack.push(mode);
    }

    /// 上下文感知压栈（`context_push`）：先把不兼容的栈顶逐个弹出。
    pub fn context_push(&mut self, mode: Box<dyn InteractionMode>, ctx: &mut ModeContext<'_>) {
        while let Some(top) = self.stack.last() {
            if mode.stackable_on(top.as_ref()) {
                break;
            }
            let mut popped = self.stack.pop().expect("刚看过非空");
            popped.leave(ctx);
            if let Some(new_top) = self.stack.last_mut() {
                new_top.enter(ctx);
            }
        }
        self.push(mode, ctx);
    }

    /// 弹出栈顶（`pop`）；弹出后若栈空则补默认模式并 `enter()`。
    pub fn pop(&mut self, ctx: &mut ModeContext<'_>) -> Option<Box<dyn InteractionMode>> {
        let mut popped = self.stack.pop();
        if let Some(mode) = popped.as_mut() {
            mode.leave(ctx);
        }
        if self.stack.is_empty() {
            let mut fresh = (self.default_mode)();
            fresh.enter(ctx);
            self.stack.push(fresh);
        } else if let Some(top) = self.stack.last_mut() {
            top.enter(ctx);
        }
        popped
    }

    /// 清空重来（`reset`），可带一个替补模式。
    pub fn reset(
        &mut self,
        replacement: Option<Box<dyn InteractionMode>>,
        ctx: &mut ModeContext<'_>,
    ) {
        while let Some(mut mode) = self.stack.pop() {
            mode.leave(ctx);
        }
        let mut fresh = replacement.unwrap_or_else(|| (self.default_mode)());
        fresh.enter(ctx);
        self.stack.push(fresh);
    }

    /// 一直弹到栈顶满足 `flags` 为止（`pop_to_behaviour`）。
    pub fn pop_to_behaviour(&mut self, flags: Behavior, ctx: &mut ModeContext<'_>) {
        while !self.top().pointer_behavior().intersects(flags) {
            if self.stack.len() == 1 {
                self.pop(ctx);
                return;
            }
            self.pop(ctx);
        }
    }

    /// 把待提交的改动落账（`sync_pending_changes` → `top.checkpoint()`）。
    pub fn checkpoint(&mut self, ctx: &mut ModeContext<'_>) {
        if self.syncing {
            return;
        }
        self.syncing = true;
        self.top_mut().checkpoint(ctx);
        self.syncing = false;
    }

    /// 让栈顶处理排队输入（idle 回调的等价物）。
    pub fn process_pending(&mut self, ctx: &mut ModeContext<'_>) {
        let more = self.top_mut().process_pending(ctx);
        let _ = more;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个只记录生命周期的最小模式。
    struct Probe {
        name: &'static str,
        log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        behavior: Behavior,
    }

    impl InteractionMode for Probe {
        fn name(&self) -> &'static str {
            self.name
        }
        fn pointer_behavior(&self) -> Behavior {
            self.behavior
        }
        fn enter(&mut self, _ctx: &mut ModeContext<'_>) {
            self.log.borrow_mut().push(format!("enter {}", self.name));
        }
        fn leave(&mut self, _ctx: &mut ModeContext<'_>) {
            self.log.borrow_mut().push(format!("leave {}", self.name));
        }
    }

    #[test]
    fn behavior_bits_match_source() {
        assert_eq!(Behavior::ALL.bits(), 0x1f);
        assert_eq!(Behavior::PAINT_BRUSH.bits(), 0x03);
        assert_eq!(Behavior::PAINT_CONSTRAINED.bits(), 0x12);
        assert!(Behavior::ALL_PAINTING.intersects(Behavior::PAINT_LINE));
        assert!(!Behavior::NON_PAINTING.intersects(Behavior::PAINT_FREEHAND));
    }

    /// 模式栈的生命周期顺序与"栈永不为空"（`mode.py:1327-1508`）。
    ///
    /// 不能只靠"能编译"验收栈：`push`/`pop`/`context_push` 都会调
    /// `leave`/`enter`，顺序错了模式就会带着脏状态继续收事件。
    #[test]
    fn mode_stack_lifecycle_and_never_empty() {
        use crate::document::Document;
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut doc = Document::new();
        let mut picked = None;

        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            color: [0.0, 0.0, 0.0],
            picked_color: Some(&mut picked),
        };
        let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut stack = ModeStack::new(|| {
            Box::new(Probe {
                name: "default",
                log: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
                behavior: Behavior::PAINT_FREEHAND,
            })
        });
        assert_eq!(stack.len(), 1, "新建的栈里就有默认模式");
        assert!(!stack.is_empty(), "栈永远不为空（`__nonzero__`）");
        assert_eq!(stack.names(), vec!["default"]);

        // push 一个非笔刷类：旧栈顶 leave → 新 enter
        stack.push(Box::new(Probe { name: "pan", log: std::rc::Rc::clone(&log), behavior: Behavior::CHANGE_VIEW }), &mut ctx);
        assert_eq!(stack.names(), vec!["default", "pan"]);
        // 默认模式记的是自己的日志（`leave default` 不在共享日志里）
        assert_eq!(log.borrow().as_slice(), &["enter pan".to_string()]);

        // context_push 视图类模式：`stackable_on` 只允许叠在笔刷类之上，
        // 所以先把同样不兼容的 `pan` 弹掉
        stack.context_push(Box::new(Probe { name: "zoom", log: std::rc::Rc::clone(&log), behavior: Behavior::CHANGE_VIEW }), &mut ctx);
        assert_eq!(stack.names(), vec!["default", "zoom"]);
        // context_push 一个笔刷类：不兼容的栈顶被逐个弹掉
        stack.context_push(Box::new(Probe { name: "brush2", log: std::rc::Rc::clone(&log), behavior: Behavior::PAINT_FREEHAND }), &mut ctx);
        // 笔刷类不能叠在笔刷类之上（issue #530），所以默认模式也被弹掉`r`n        assert_eq!(stack.names(), vec!["brush2"]);

        // pop 到空 → 自动补默认模式并 enter
        stack.pop(&mut ctx);
        assert_eq!(stack.names(), vec!["default"]);
        let tail = log.borrow().last().cloned().unwrap_or_default();
        assert_eq!(tail, "leave brush2", "弹空后默认模式会重新 enter（记在它自己的日志里）");

        // reset：清空重来 + 替补模式
        stack.reset(Some(Box::new(Probe { name: "reset-mode", log: std::rc::Rc::clone(&log), behavior: Behavior::PAINT_LINE })), &mut ctx);
        assert_eq!(stack.names(), vec!["reset-mode"]);

        // pop_to_behaviour：一直弹到满足标志为止
        stack.push(Box::new(Probe { name: "pan2", log: std::rc::Rc::clone(&log), behavior: Behavior::CHANGE_VIEW }), &mut ctx);
        stack.pop_to_behaviour(Behavior::ALL_PAINTING, &mut ctx);
        assert_eq!(stack.names(), vec!["reset-mode"], "视图类模式被弹掉");
    }

    #[test]
    fn modifiers_intersect_is_bitwise() {
        let m = Modifiers {
            shift: true,
            ctrl: false,
            alt: true,
        };
        assert!(m.any());
        assert!(m.intersects(Modifiers {
            shift: true,
            ..Modifiers::NONE
        }));
        assert!(!m.intersects(Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }));
    }

    #[test]
    fn oneshot_pop_rules_match_source() {
        // 无修饰键 + temporary_activation → 出栈
        let m = OneshotDragMode::default();
        assert!(m.should_pop(Modifiers::NONE));
        // 无修饰键 + 不临时激活 + unmodified_persist → 留下
        let keep = OneshotDragMode {
            unmodified_persist: true,
            temporary_activation: false,
            ..OneshotDragMode::default()
        };
        assert!(!keep.should_pop(Modifiers::NONE));
        // 带着 shift 进入：shift 还按着 → 留下；松开了 → 出栈
        let mut spring = OneshotDragMode::default();
        spring.drag_mode.drag.initial_modifiers = Some(Modifiers {
            shift: true,
            ..Modifiers::NONE
        });
        assert!(!spring.should_pop(Modifiers {
            shift: true,
            ..Modifiers::NONE
        }));
        assert!(spring.should_pop(Modifiers::NONE));
    }

    #[test]
    fn drag_mode_spring_loaded_exit() {
        let mut d = DragMode::default();
        assert!(!d.enter(Modifiers {
            shift: true,
            ..Modifiers::NONE
        }));
        // 再次进入（上面压了一个模式）时 shift 已经松开 → 要求再弹一次
        assert!(d.enter(Modifiers::NONE));
        // 忽略修饰键的模式永远不弹
        let mut ig = DragMode {
            ignore_modifiers: true,
            ..DragMode::default()
        };
        assert!(!ig.enter(Modifiers {
            shift: true,
            ..Modifiers::NONE
        }));
        assert_eq!(ig.drag.initial_modifiers, Some(Modifiers::NONE));
    }
}
