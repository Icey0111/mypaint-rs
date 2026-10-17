//! 拖拽改笔刷尺寸 —— `gui/brushmanip.py` 的 `BrushResizeMode` 与
//! `BrushSizeOverlay` 的引擎侧状态。
//!
//! 源码（`gui/brushmanip.py:159-291`）是一个 `OneshotDragMode`：
//! - 起手时记录起点、当前笔刷的**视觉半径**（`lib/brush.py:623-625` 的
//!   `exp(radius) + 2*exp(radius)*by_random`）、缩放、随机偏移因子；
//! - 手柄从起点右侧一个视觉半径处开始，跟随指针位移的 **0.5 倍**
//!   （按住 Shift 切精度模式后是 0.125 倍，`:276-279`）；
//! - 每次移动用 `log((hypot(handle-start)/scale) * radius_factor)` 反算
//!   `radius_logarithmic`，到上限（`brushsettings.json` 的 6.0）就夹住；
//! - 松手时把结果写回 `brush_adjustment["radius_logarithmic"]`，模式出栈。
//!
//! 本模块只做**数学与状态**：半径的落地出口由应用层提供
//! （[`super::InteractionMode::take_brush_radius`]），圆圈覆盖层由
//! [`BrushSizeOverlayState`] 描述、应用层画。
//!
//! 记账差异：
//! - 源码从按钮映射（`input.button_mapping`）进入本模式；本移植的
//!   `ButtonMappingEditor` 还没接（见路线图），暂以应用本地键
//!   `Ctrl+Shift+R` 触发，接上按钮映射后撤掉；
//! - 源码 `get_options_widget()` 返回**上一个模式**的选项控件；本移植用
//!   `ModeStack::options_mode()` 表达同一件事；
//! - 源码在 `drag_start_cb` 里从 `tdw.scale` 取缩放、从 `event` 取显示坐标；
//!   本移植的指针事件是模型坐标，所以显示位移由 `viewzoom`/`viewrotation`
//!   反推（距离计算与旋转无关，见 `radius_px` 的注释）。

use crate::hokusai::BrushSetting;

use super::{Behavior, InteractionMode, ModeContext, PointerEvent};

/// `radius_logarithmic` 的上界 —— `brushsettings.json:222`（`GtkAdjustment`
/// 的 `upper`，源码 `self._base_radius_adj.get_upper()`，`:238`）。
pub const MAX_RADIUS_LOG: f64 = 6.0;

/// 普通模式的位移倍率（`brushmanip.py:277`）。
const MOVE_FACTOR: f32 = 0.5;
/// 精度模式的位移倍率（`brushmanip.py:277`）。
const MOVE_FACTOR_PRECISION: f32 = 0.125;

/// 画布上画的圆圈覆盖层状态（`gui/brushmanip.py:22-156` 的
/// `BrushSizeOverlay`）。
///
/// 坐标分两套：
/// - `center` 是模型坐标（画圈的位置）；
/// - `radius_px` 与 `handle` 是**显示像素**（源码的 `_x/_y` / `_radius` /
///   `_hx/_hy` 都在 TDW 的显示坐标系里）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushSizeOverlayState {
    /// 圆圈中心（模型坐标）。
    pub center: (f32, f32),
    /// 当前半径（显示像素）。
    pub radius_px: f32,
    /// 手柄相对圆心的显示位移（像素）。
    pub handle: (f32, f32),
    /// 最大半径（显示像素）——源码在半径触顶时用 `_max_px_radius` 重画。
    pub max_radius_px: f32,
}

/// 笔刷尺寸拖拽模式。
pub struct BrushResizeMode {
    /// 拖拽是否已开始。
    active: bool,
    /// 圆圈中心（模型坐标）。
    center: (f32, f32),
    /// 上一次指针位置（模型坐标）。
    last: (f32, f32),
    /// 手柄相对圆心的显示位移（像素）。
    handle: (f32, f32),
    /// 当前半径（显示像素）。
    radius_px: f32,
    /// 最大半径（显示像素）。
    max_radius_px: f32,
    /// `1 / (1 + 2 * offset_by_random)`（`brushmanip.py:231`）。
    radius_factor: f32,
    /// 起手时的 Shift 状态（精度模式 = 它与当前 Shift 的异或）。
    initial_shift: bool,
    /// 松手后要写回的 `radius_logarithmic`。
    new_radius: Option<f32>,
    /// 待应用取走的半径请求。
    pending_radius: Option<f32>,
}

impl Default for BrushResizeMode {
    fn default() -> Self {
        Self::new()
    }
}

impl BrushResizeMode {
    pub fn new() -> Self {
        Self {
            active: false,
            center: (0.0, 0.0),
            last: (0.0, 0.0),
            handle: (0.0, 0.0),
            radius_px: 0.0,
            max_radius_px: 0.0,
            radius_factor: 1.0,
            initial_shift: false,
            new_radius: None,
            pending_radius: None,
        }
    }

    /// 视觉半径 —— `lib/brush.py:623-625`：
    /// `exp(base_radius) + 2 * exp(base_radius) * base_random_offset`。
    fn visual_radius(base_radius: f64, by_random: f64) -> f64 {
        let base_r = base_radius.exp();
        base_r + 2.0 * base_r * by_random
    }

    /// 显式复位一次拖拽（`DragMode._reset_drag_state` 的对应物）。
    fn reset_drag(&mut self) {
        self.active = false;
        self.handle = (0.0, 0.0);
        self.radius_px = 0.0;
        self.new_radius = None;
    }

    /// 把一次**模型坐标**位移换算成显示像素位移。
    ///
    /// 源码的 `drag_update_cb` 拿到的是 TDW 显示坐标的 `dx/dy`；
    /// 本移植拿到模型坐标，必须补上缩放与旋转（`viewrotation` 与
    /// `View::world_to_screen` 同为弧度、同一方向）。镜像只翻 X 的符号，
    /// 对半径（`hypot`）没有影响，所以这里不处理。
    fn model_delta_to_display(&self, ev: &PointerEvent, dx: f32, dy: f32) -> (f32, f32) {
        let (s, c) = ev.viewrotation.sin_cos();
        let zoom = ev.viewzoom.max(1e-6);
        ((c * dx - s * dy) * zoom, (s * dx + c * dy) * zoom)
    }
}

impl InteractionMode for BrushResizeMode {
    fn name(&self) -> &'static str {
        "Resize Brush"
    }

    fn usage(&self) -> &str {
        "Change brush size by dragging on the canvas"
    }

    // 源码的 `ACTION_NAME = "BrushResizeMode"` 是一个**只用于注册**的伪动作
    // （`brushmanip.py:165`），`resources.xml` 里没有对应 GtkAction，所以
    // `get_action()` 返回 None、图标沿用上一个模式（`get_icon_name`）。
    fn action_name(&self) -> Option<&'static str> {
        Some("BrushResizeMode")
    }

    fn pointer_behavior(&self) -> Behavior {
        Behavior::EDIT_OBJECTS
    }

    fn is_oneshot(&self) -> bool {
        true
    }

    fn delegates_options(&self) -> bool {
        // `get_options_widget()` 直接返回上一个模式的控件（`:296-297`）。
        true
    }

    fn stackable_on(&self, mode: &dyn InteractionMode) -> bool {
        // `OneshotDragMode.stackable_on`：只要下面不是另一个 Oneshot 就能叠
        // （`mode.py:1298-1300`）。
        !mode.is_oneshot()
    }

    fn enter(&mut self, _ctx: &mut ModeContext<'_>) {
        self.reset_drag();
        self.pending_radius = None;
    }

    fn leave(&mut self, _ctx: &mut ModeContext<'_>) {
        self.reset_drag();
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 {
            return false;
        }
        let base_radius = ctx.brush.get(BrushSetting::Radius).base_value as f64;
        let by_random = ctx.brush.get(BrushSetting::RadiusByRandom).base_value as f64;
        let zoom = ev.viewzoom.max(1e-6);
        let visual_radius = Self::visual_radius(base_radius, by_random);

        self.active = true;
        self.center = (ev.x, ev.y);
        self.last = (ev.x, ev.y);
        self.radius_px = (visual_radius * zoom as f64) as f32;
        // 源码：`self._handle_x = x + radius_px; self._handle_y = y`（`:232-233`）。
        self.handle = (self.radius_px, 0.0);
        // `1 / (1 + by_random * 2)`（`:231`）。
        self.radius_factor = (1.0 / (1.0 + by_random * 2.0)) as f32;
        // `_max_px_radius = brush_visual_radius(max_radius, by_random) * scale`
        // （`:240-241`）。
        self.max_radius_px = (Self::visual_radius(MAX_RADIUS_LOG, by_random) * zoom as f64) as f32;
        self.initial_shift = ev.modifiers.shift;
        self.new_radius = None;
        true
    }

    fn motion(&mut self, _ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.active || ev.button != 0 {
            return false;
        }
        let zoom = ev.viewzoom.max(1e-6);
        let (dx, dy) = (ev.x - self.last.0, ev.y - self.last.1);
        self.last = (ev.x, ev.y);
        let (ddx, ddy) = self.model_delta_to_display(ev, dx, dy);

        // `_update_precision_state`（`:256-261`）：按下时的 Shift 与当前
        // Shift 的异或。本移植没有把普通 Shift 键事件送进模式，所以按
        // **每次移动时的修饰键状态**现算，等价于源码的按下/松开回调。
        let precision = self.initial_shift ^ ev.modifiers.shift;
        let move_factor = if precision {
            MOVE_FACTOR_PRECISION
        } else {
            MOVE_FACTOR
        };
        self.handle.0 += ddx * move_factor;
        self.handle.1 += ddy * move_factor;

        let mut radius_px = self.handle.0.hypot(self.handle.1);
        // `new_radius = log((radius_px / scale) * radius_factor)`
        // （`:283`；`log` 是自然对数）。
        let mut new_radius = ((radius_px / zoom) * self.radius_factor).ln();
        if !new_radius.is_finite() {
            // 半径为 0（手柄还在起点）时源码会得到 -inf；GtkAdjustment 会夹到
            // lower（-2.0），这里直接按同一语义夹住。
            new_radius = crate::color::RADIUS_LOG_MIN as f32;
        }
        if new_radius as f64 >= MAX_RADIUS_LOG {
            new_radius = MAX_RADIUS_LOG as f32;
            radius_px = self.max_radius_px;
        }
        self.radius_px = radius_px;
        self.new_radius = Some(new_radius);
        true
    }

    fn button_release(&mut self, _ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.active || ev.button != 1 {
            return false;
        }
        // `drag_stop_cb`（`:287-290`）：只有拿到新半径才写回。
        if let Some(r) = self.new_radius.take() {
            self.pending_radius = Some(r);
        }
        self.active = false;
        true
    }

    fn take_brush_radius(&mut self) -> Option<f32> {
        self.pending_radius.take()
    }

    fn brush_size_overlay(&self) -> Option<BrushSizeOverlayState> {
        if !self.active {
            return None;
        }
        Some(BrushSizeOverlayState {
            center: self.center,
            radius_px: self.radius_px,
            handle: self.handle,
            max_radius_px: self.max_radius_px,
        })
    }

    fn oneshot_pop_after_drag(&self, _modifiers: super::Modifiers) -> bool {
        // 源码 `SPRING_LOADED = False`，所以 `drag_stop_cb` 里 `pop = True`
        // （`mode.py:1305-1317`）。
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::hokusai::{Brush, BrushState};

    fn brush_with(radius: f32, by_random: f32) -> Brush {
        let mut b = crate::brushlib::default_brush();
        b.get_mut(BrushSetting::Radius).base_value = radius;
        b.get_mut(BrushSetting::RadiusByRandom).base_value = by_random;
        b
    }

    fn ctx<'a>(
        doc: &'a mut Document,
        brush: &'a Brush,
        state: &'a mut BrushState,
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
            picked_color: None,
        }
    }

    fn press(x: f32, y: f32, shift: bool) -> PointerEvent {
        let mut ev = PointerEvent::button(x, y, 0.0, 1);
        ev.modifiers.shift = shift;
        ev
    }

    #[test]
    fn drag_half_rate_and_log_radius() {
        let mut doc = Document::new();
        let brush = brush_with(0.0, 0.0); // 视觉半径 = exp(0) = 1
        let mut state = BrushState::default();
        let mut mode = BrushResizeMode::new();

        {
            let mut c = ctx(&mut doc, &brush, &mut state);
            assert!(mode.button_press(&mut c, &press(100.0, 100.0, false)));
            let overlay = mode.brush_size_overlay().unwrap();
            assert_eq!(overlay.center, (100.0, 100.0));
            assert!((overlay.radius_px - 1.0).abs() < 1e-6);
            assert!((overlay.handle.0 - 1.0).abs() < 1e-6);

            // 指针右移 10 显示像素：手柄只走 5（0.5 倍），半径 = 1 + 5 = 6
            let mut ev = PointerEvent::motion(110.0, 100.0, 16.0);
            assert!(mode.motion(&mut c, &ev));
            let overlay = mode.brush_size_overlay().unwrap();
            assert!(
                (overlay.radius_px - 6.0).abs() < 1e-4,
                "r={}",
                overlay.radius_px
            );
            assert!((overlay.handle.0 - 6.0).abs() < 1e-4);

            // 松手 → 半径请求 = ln(6)
            ev = PointerEvent::button(110.0, 100.0, 32.0, 1);
            assert!(mode.button_release(&mut c, &ev));
            let r = mode.take_brush_radius().expect("松手后应有半径请求");
            assert!((r - 6.0f32.ln()).abs() < 1e-4, "r={r}");
            assert!(mode.take_brush_radius().is_none(), "请求只能被取走一次");
        }
    }

    #[test]
    fn shift_precision_uses_eighth_rate() {
        let mut doc = Document::new();
        let brush = brush_with(0.0, 0.0);
        let mut state = BrushState::default();
        let mut mode = BrushResizeMode::new();
        let mut c = ctx(&mut doc, &brush, &mut state);

        // 按下时没按 Shift，移动时按住 Shift → 精度模式（0.125 倍）
        assert!(mode.button_press(&mut c, &press(0.0, 0.0, false)));
        let mut ev = PointerEvent::motion(80.0, 0.0, 16.0);
        ev.modifiers.shift = true;
        assert!(mode.motion(&mut c, &ev));
        let overlay = mode.brush_size_overlay().unwrap();
        // 1 + 80*0.125 = 11
        assert!(
            (overlay.radius_px - 11.0).abs() < 1e-4,
            "r={}",
            overlay.radius_px
        );
        // 松开 Shift 回到普通速率
        let mut ev2 = PointerEvent::motion(88.0, 0.0, 32.0);
        ev2.modifiers.shift = false;
        assert!(mode.motion(&mut c, &ev2));
        let overlay = mode.brush_size_overlay().unwrap();
        assert!(
            (overlay.radius_px - 15.0).abs() < 1e-4,
            "r={}",
            overlay.radius_px
        );
    }

    #[test]
    fn radius_is_clamped_to_upper_bound() {
        let mut doc = Document::new();
        let brush = brush_with(0.0, 0.0);
        let mut state = BrushState::default();
        let mut mode = BrushResizeMode::new();
        let mut c = ctx(&mut doc, &brush, &mut state);

        assert!(mode.button_press(&mut c, &press(0.0, 0.0, false)));
        // 拖得远远超过上限：半径夹到 6.0，覆盖层半径用 max_px_radius
        let ev = PointerEvent::motion(10_000.0, 0.0, 16.0);
        assert!(mode.motion(&mut c, &ev));
        let overlay = mode.brush_size_overlay().unwrap();
        assert!((overlay.radius_px - overlay.max_radius_px).abs() < 1e-3);
        assert!((overlay.radius_px - MAX_RADIUS_LOG.exp() as f32).abs() < 1e-3);

        let ev = PointerEvent::button(10_000.0, 0.0, 32.0, 1);
        assert!(mode.button_release(&mut c, &ev));
        let r = mode.take_brush_radius().unwrap();
        assert!((r - MAX_RADIUS_LOG as f32).abs() < 1e-6);
    }

    #[test]
    fn by_random_scales_visual_radius_and_factor() {
        let mut doc = Document::new();
        // by_random = 1 → 视觉半径 = exp(0) + 2*exp(0)*1 = 3
        let brush = brush_with(0.0, 1.0);
        let mut state = BrushState::default();
        let mut mode = BrushResizeMode::new();
        let mut c = ctx(&mut doc, &brush, &mut state);

        assert!(mode.button_press(&mut c, &press(0.0, 0.0, false)));
        let overlay = mode.brush_size_overlay().unwrap();
        assert!((overlay.radius_px - 3.0).abs() < 1e-6);
        // 指针右移 2 显示像素：手柄 3 + 1 = 4；半径 = ln(4 * 1/3)
        let ev = PointerEvent::motion(2.0, 0.0, 16.0);
        assert!(mode.motion(&mut c, &ev));
        assert!((mode.radius_px - 4.0).abs() < 1e-4, "r={}", mode.radius_px);
    }

    #[test]
    fn escape_equivalent_leave_drops_request() {
        let mut doc = Document::new();
        let brush = brush_with(0.0, 0.0);
        let mut state = BrushState::default();
        let mut mode = BrushResizeMode::new();
        let mut c = ctx(&mut doc, &brush, &mut state);

        assert!(mode.button_press(&mut c, &press(0.0, 0.0, false)));
        let ev = PointerEvent::motion(40.0, 0.0, 16.0);
        assert!(mode.motion(&mut c, &ev));
        // 模式被弹掉（Escape / 切换工具）：不落地任何半径
        mode.leave(&mut c);
        assert!(mode.take_brush_radius().is_none());
        assert!(mode.brush_size_overlay().is_none());
    }

    #[test]
    fn stacks_on_freehand_and_delegates_options_then_pops() {
        use super::super::ModeStack;
        use crate::mode::freehand::FreehandMode;

        let mut doc = Document::new();
        let brush = brush_with(0.0, 0.0);
        let mut state = BrushState::default();
        let mut c = ctx(&mut doc, &brush, &mut state);
        let mut stack = ModeStack::new(|| Box::new(FreehandMode::new()));

        // 源码从按钮映射 push（`document.py:765-772`），不是替换栈。
        stack.push(Box::new(BrushResizeMode::new()), &mut c);
        assert_eq!(stack.names(), vec!["Freehand Drawing", "Resize Brush"]);
        // 选项/图标沿用下面那个模式（`get_options_widget`/`get_icon_name`）。
        assert_eq!(stack.options_mode().name(), "Freehand Drawing");

        // Oneshot 拖完弹回原模式（`OneshotDragMode.drag_stop_cb`）。
        stack.pop(&mut c);
        assert_eq!(stack.names(), vec!["Freehand Drawing"]);
        assert_eq!(stack.options_mode().name(), "Freehand Drawing");
    }

    #[test]
    fn release_without_motion_writes_nothing() {
        let mut doc = Document::new();
        let brush = brush_with(0.0, 0.0);
        let mut state = BrushState::default();
        let mut mode = BrushResizeMode::new();
        let mut c = ctx(&mut doc, &brush, &mut state);

        assert!(mode.button_press(&mut c, &press(5.0, 5.0, false)));
        let ev = PointerEvent::button(5.0, 5.0, 16.0, 1);
        assert!(mode.button_release(&mut c, &ev));
        assert!(
            mode.take_brush_radius().is_none(),
            "源码没拿到新半径就不写回"
        );
        assert!(mode.brush_size_overlay().is_none());
    }
}
