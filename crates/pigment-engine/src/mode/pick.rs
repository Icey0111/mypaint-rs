//! 取色模式 —— `gui/colorpicker.py` 的移植。
//!
//! # 源码结构（本文件按同样顺序排列）
//!
//! | 参考源 | 内容 | 本文件 |
//! |---|---|---|
//! | `colorpicker.py:25` | `ColorPickMode(OneshotDragMode)` | [`ColorPickMode`] |
//! | `:135` | `get_new_color`（无彩色时保留画笔色相） | 同名方法 |
//! | `:143` | `_pick_color`（比较 → 算新色 → 直接落地/排队） | [`ColorPickMode::pick_color_at`] |
//! | `:154` | `_queue_color_change` / `_change_color`（60fps 节流） | [`ColorPickMode::flush_queued`] |
//! | `:172` | `_C_MIN` / `_Y_MIN` / `_Y_MAX` | [`C_MIN`] / [`Y_MIN`] / [`Y_MAX`] |
//! | `:177` | `ColorPickModeHCYBase` | [`ColorPickMode::get_new_hcy_color`] |
//! | `:191`/`:213`/`:233` | `ColorPickModeH` / `C` / `Y` | [`PickVariant`] |
//! | `:253` | `ColorPickPreviewOverlay` | 未移植（记账） |
//! | `tileddrawwidget.py:977` | `tdw.pick_color`（取样合并 + 3×3 平均） | [`crate::Document::pick_color`] |
//! | `lib/color.py:223` | `new_from_pixbuf_average` | 同上（8bpc sRGB 直接平均） |
//! | `lib/color.py:345` | `RGBColor.__eq__` 的比较规则 | [`rgb_eq`]（转调 [`crate::color`]） |
//! | `lib/color.py:804`/`:844` | `RGB_to_HCY` / `HCY_to_RGB` | [`crate::color::rgb_to_hcy`] / [`crate::color::hcy_to_rgb`] |
//! | `lib/color.py:521`/`:541` | `HCYColor(color=…)` / `get_hsv()` | [`crate::color::HcyColor::from_color`] / [`crate::color::UIColor::get_hsv`] |
//!
//! # 交互（照源码）
//!
//! - **一键式拖拽**：`OneshotDragMode`，`pointer_behavior = EDIT_OBJECTS`、
//!   `scroll_behavior = NONE`（`colorpicker.py:55-56`）；
//! - 单选动作（工具条/菜单）进来时**不立即取色**，得先在画布上按一下
//!   （`colorpicker.py:34-39`、`:100-104`）；按住拖动则持续取色（`:116-119`）；
//! - `Flip*` 动作（键盘 `r` / `Ctrl+r` / `Shift+r` / `Alt+r`）进来时带
//!   `ignore_modifiers=True, temporary_activation=False`
//!   （`document.py:2169-2173`）：`enter()` 就在**最后一次指针事件的位置**
//!   取一次色，并把"开始拖拽"推迟到下一次 motion（`colorpicker.py:80-92`）；
//! - 颜色出口是 `brush_color_manager.set_color()`（`brushcolor.py:33-37`）。
//!   模式住在引擎层，够不到应用的颜色状态，所以走
//!   [`ModeContext::picked_color`]：模式写、GUI 每帧读走并落地。
//!
//! # 记账（本块未移植 / 用最保守做法的地方）
//!
//! - **预览方块叠加层**（`ColorPickPreviewOverlay`，`colorpicker.py:253-364`）：
//!   要画布叠加层 + 圆角框，属另一块，未移植；
//! - **光标**（`inactive_cursor` 取 `app.cursor_color_picker*`，`:60-62`、
//!   `:196-198`）：光标族未移植，画布上仍是笔刷圆圈（`shell/workspace.rs`）；
//! - **拖拽结束后的自动出栈**：`OneshotDragMode.drag_stop_cb`（`mode.py:1302-1317`）
//!   按修饰键规则弹栈；模式对象够不到模式栈，[`ColorPickMode::pop_after_drag`]
//!   只把这条规则算出来，GUI 目前不调用（与 B3-1 起 `switch_mode` 用 `reset`
//!   的既有记账一致）。源码 `keyup_timeout = 0`（`colorpicker.py:53`）是
//!   "松开 `r` 就出栈"，依赖按键**释放**事件，本移植也没接；
//! - **60fps 节流**（`_queue_color_change` 的 `GLib.timeout_add(16.66)`）：
//!   我们的 idle 等价物是 `process_pending`（每帧一次），落地的是一个
//!   `Option`，没有节流必要；
//! - **空格起手拖拽**（`DragMode.key_press_cb`，`mode.py:1247-1252`）：本移植
//!   里空格是画布平移（`shell/workspace.rs`），事件到不了模式层；
//! - **`supports_button_switching = False`**（`colorpicker.py:58`）：那是
//!   设备表/按键切换那一套（`gui/devices.py`），本移植没有对应物；
//! - **`get_options_widget()` 返回 None**（`:132-133`）：本来就没有选项面板。

use super::{Behavior, InteractionMode, Key, ModeContext, OneshotDragMode, PointerEvent};

/// HCY 色度下限（`colorpicker.py:172` 的 `_C_MIN`）：避免除零/无色相。
pub const C_MIN: f32 = 0.0001;
/// HCY 亮度下限（`colorpicker.py:173` 的 `_Y_MIN`）。
pub const Y_MIN: f32 = 0.0001;
/// HCY 亮度上限（`colorpicker.py:174` 的 `_Y_MAX`）。
pub const Y_MAX: f32 = 0.9999;

// HCY 的 luma 权重只有一份：`lib/color.py:799-801` 的 `_HCY_*_LUMA`，
// B4-1 起住在 [`crate::color`]（f64）。这里只转出去给本模块用。
pub use crate::color::{HCY_BLUE_LUMA, HCY_GREEN_LUMA, HCY_RED_LUMA};

// ---- 色彩空间换算：**唯一实现在 [`crate::color`]** --------------------------
//
// B3-4 时这七个函数在本文件里各有一份 f32 实现；B4-1 把 `lib/color.py`
// 完整移植到 `crate::color`（f64，逐位对照源码，由
// `tests/color_vectors.rs` 的 2.3 万条向量门禁），于是这里改成薄包装：
//
// - 上面各函数的 doc 注释（源行号、源码行为说明）已随之移到 `crate::color`；
// - 本层保留 f32 签名，是因为 [`PickColor`] / [`ModeContext::color`] 这条
//   通道是模式层的 `[f32; 3]`；换算本身在 f64 里做，避免二次误差累积。

/// `[f32; 3]` → `[f64; 3]`。
fn f64v(v: [f32; 3]) -> [f64; 3] {
    [v[0] as f64, v[1] as f64, v[2] as f64]
}

/// `[f64; 3]` → `[f32; 3]`。
fn f32v(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

/// `colorsys.rgb_to_hsv`（`UIColor.get_hsv`，`lib/color.py:65-74`）。
fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    f32v(crate::color::rgb_to_hsv(f64v(rgb)))
}

/// `colorsys.hsv_to_rgb`（`HSVColor.get_rgb`，`lib/color.py:398-399`）。
fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    f32v(crate::color::hsv_to_rgb(f64v(hsv)))
}

/// `HCY_to_RGB`（`lib/color.py:844-916`）。
fn hcy_to_rgb(hcy: [f32; 3]) -> [f32; 3] {
    f32v(crate::color::hcy_to_rgb(f64v(hcy)))
}

/// `RGB_to_HCY`（`lib/color.py:804-841`）—— 只有测试用到：
/// 取色模式自己不直接转 HCY（它经 `crate::color::HcyColor::from_color` 走
/// 源码那条"先取 HSV 再转"的构造路径），所以这里标了 `#[cfg(test)]`。
#[cfg(test)]
fn rgb_to_hcy(rgb: [f32; 3]) -> [f32; 3] {
    f32v(crate::color::rgb_to_hcy(f64v(rgb)))
}

/// `HCYColor(color=…)` 在来源不是 `HCYColor` 时的那条分支
/// （`lib/color.py:521-531`，含"纯黑但饱和度不为 0"的特例）。
fn hcy_from_hsv(hsv: [f32; 3]) -> [f32; 3] {
    use crate::color::{Color, HcyColor, HsvColor};
    let c = HcyColor::from_color(&Color::Hsv(HsvColor::from_hsv(f64v(hsv))));
    [c.h as f32, c.c as f32, c.y as f32]
}

/// `HCYColor.get_hsv()`（`lib/color.py:541-547`，色相直接用存下的 `h`）。
fn hsv_from_hcy(hcy: [f32; 3]) -> [f32; 3] {
    use crate::color::{HcyColor, UIColor};
    f32v(HcyColor::from_hcy(f64v(hcy)).get_hsv())
}

/// 两个颜色是否"相等" —— 照 `RGBColor.__eq__`（`lib/color.py:345-353`）：
/// 两边都折成 RGB，各分量 `round(3)` 后逐通道比。
///
/// 源码里 `pick_color` 永远是一个 `RGBColor`（`tdw.pick_color` 的返回值），
/// 所以 `pick_color != self._color` 走的正是这一支。
fn rgb_eq(a: [f32; 3], b: [f32; 3]) -> bool {
    use crate::color::{Color, RgbColor, UIColor};
    let ca = Color::Rgb(RgbColor::from_rgb(f64v(a)));
    let cb = Color::Rgb(RgbColor::from_rgb(f64v(b)));
    ca.eq_src(&cb)
}

/// 上一次算出来的颜色（源码 `self._color`）。
///
/// 源码里 `self._color` 是个 `UIColor`，具体类随变体走：完整取色时可能是
/// `RGBColor`（直接取自屏幕）或 `HSVColor`（无彩色时改写了色相），HCY 变体
/// 则一直是 `HCYColor`。保留这个"类型"是有意义的 —— `get_hsv()` 在
/// `HCYColor` 上有黑点特例（`lib/color.py:541-547`），换成 HSV 往返会丢信息。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickColor {
    /// `RGBColor`
    Rgb([f32; 3]),
    /// `HSVColor`
    Hsv([f32; 3]),
    /// `HCYColor`
    Hcy([f32; 3]),
}

impl PickColor {
    /// `get_rgb()`（`lib/color.py:299-300` / `:398-399` / `:549-550`）。
    pub fn rgb(self) -> [f32; 3] {
        match self {
            PickColor::Rgb(rgb) => rgb,
            PickColor::Hsv(hsv) => hsv_to_rgb(hsv),
            PickColor::Hcy(hcy) => hcy_to_rgb(hcy),
        }
    }

    /// `get_hsv()`（`lib/color.py:65-74` / `:395-396` / `:541-547`）。
    ///
    /// 这就是 `brush.set_color_hsv(color.get_hsv())` 吃的东西
    /// （`brushcolor.py:37`），也是本移植写进
    /// [`ModeContext::picked_color`] 的通道格式。
    pub fn hsv(self) -> [f32; 3] {
        match self {
            PickColor::Rgb(rgb) => rgb_to_hsv(rgb),
            PickColor::Hsv(hsv) => hsv,
            PickColor::Hcy(hcy) => hsv_from_hcy(hcy),
        }
    }
}

/// 取色的四个变体（`ColorPickMode` + `H` / `C` / `Y` 三个子类）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickVariant {
    /// `ColorPickMode`：整份颜色
    Full,
    /// `ColorPickModeH`：只改色相
    Hue,
    /// `ColorPickModeC`：只改彩度
    Chroma,
    /// `ColorPickModeY`：只改亮度
    Luma,
}

impl PickVariant {
    /// 关联的动作名（`ACTION_NAME`，`colorpicker.py:49`/`:194`/`:215`/`:235`）。
    pub fn action_name(self) -> &'static str {
        match self {
            PickVariant::Full => "ColorPickMode",
            PickVariant::Hue => "ColorPickModeH",
            PickVariant::Chroma => "ColorPickModeC",
            PickVariant::Luma => "ColorPickModeY",
        }
    }

    /// 由动作名反查（GUI 把动作映射到模式类时用）。
    pub fn from_action_name(id: &str) -> Option<Self> {
        match id {
            "ColorPickMode" => Some(PickVariant::Full),
            "ColorPickModeH" => Some(PickVariant::Hue),
            "ColorPickModeC" => Some(PickVariant::Chroma),
            "ColorPickModeY" => Some(PickVariant::Luma),
            _ => None,
        }
    }

    /// `get_name()`（`colorpicker.py:64-66`/`:200-202`/`:221-223`/`:241-243`）。
    pub fn display_name(self) -> &'static str {
        match self {
            PickVariant::Full => "Pick Color",
            PickVariant::Hue => "Pick Hue",
            PickVariant::Chroma => "Pick Chroma",
            PickVariant::Luma => "Pick Luma",
        }
    }

    /// `get_usage()`（同上几处）。
    pub fn usage(self) -> &'static str {
        match self {
            PickVariant::Full => "Set the color used for painting",
            PickVariant::Hue => "Set the color Hue used for painting",
            PickVariant::Chroma => "Set the color Chroma used for painting",
            PickVariant::Luma => "Set the color Luma used for painting",
        }
    }
}

/// 取色模式（`ColorPickMode`，`colorpicker.py:25`）。
///
/// 三个 HCY 变体共用同一套交互，只有 [`ColorPickMode::get_new_color`] 的
/// 算法不同 —— 源码也是这个结构（`ColorPickModeHCYBase` 只覆写
/// `get_new_color`，H/C/Y 再各自覆写 `get_new_hcy_color`）。
pub struct ColorPickMode {
    /// 一键式拖拽的基础设施（`OneshotDragMode`：拖拽状态 + 出栈开关）
    pub oneshot: OneshotDragMode,
    /// 变体
    pub variant: PickVariant,
    /// 上一次算出的颜色（源码 `self._color`）
    last_color: PickColor,
    /// 排队等着落地的新颜色（源码 `self._queued_data`）
    queued: Option<[f32; 3]>,
    /// 是否由键盘（`Flip*` 动作）起手（`self._started_from_key_press`）
    started_from_key_press: bool,
    /// 键盘起手时 `enter()` 立刻取色用的位置
    /// （源码 `doc.get_last_event_info(tdw)` 的 `(x, y)`）
    enter_pick: Option<(f32, f32)>,
    /// 键盘起手后，真正的拖拽要等下一次 motion
    /// （`self._start_drag_on_next_motion_event`）
    start_drag_on_next_motion: bool,
    /// 最近一次指针事件的视图缩放（屏幕 3×3 取样间距 = 1/zoom）。
    last_viewzoom: f32,
}

impl Default for ColorPickMode {
    fn default() -> Self {
        Self::new(PickVariant::Full)
    }
}

impl ColorPickMode {
    /// 单选动作（工具条 / Edit→Current Mode 菜单）建出来的取色模式。
    ///
    /// `ignore_modifiers=False, temporary_activation=True`
    /// （`document.py:2175` 的另一条分支 → 类默认值）：**不立即取色**，
    /// 要在画布上按一下（`colorpicker.py:34-39`）。
    pub fn new(variant: PickVariant) -> Self {
        Self {
            oneshot: OneshotDragMode::default(),
            variant,
            // `enter()` 里会被当前颜色覆盖
            last_color: PickColor::Hsv([0.0, 0.0, 0.0]),
            queued: None,
            started_from_key_press: false,
            enter_pick: None,
            start_drag_on_next_motion: false,
            last_viewzoom: 1.0,
        }
    }

    /// `Flip*` 动作（键盘 `r` / `Ctrl+r` / `Shift+r` / `Alt+r`）建出来的。
    ///
    /// `ignore_modifiers=True, temporary_activation=False`
    /// （`document.py:2169-2173`）：`enter()` 时在 `last_event` 处立即取一次色，
    /// 然后等下一次 motion 才开始拖拽（`colorpicker.py:80-92`）。
    /// `last_event` 为 `None` 时（指针不在画布上）跳过立即取色 —— 源码同样
    /// 在 `x`/`y` 为 `None` 时不取（`:88-90`）。
    pub fn flip(variant: PickVariant, last_event: Option<(f32, f32)>) -> Self {
        Self {
            oneshot: OneshotDragMode {
                // `temporary_activation=False`：无修饰键也不在拖拽后出栈
                temporary_activation: false,
                drag_mode: super::DragMode {
                    ignore_modifiers: true,
                    ..super::DragMode::default()
                },
                ..OneshotDragMode::default()
            },
            variant,
            last_color: PickColor::Hsv([0.0, 0.0, 0.0]),
            queued: None,
            started_from_key_press: true,
            enter_pick: last_event,
            start_drag_on_next_motion: false,
            last_viewzoom: 1.0,
        }
    }

    /// 上一次算出的颜色（调试/测试用）。
    #[inline]
    pub fn last_color(&self) -> PickColor {
        self.last_color
    }

    /// 还没落地的颜色（调试/测试用）。
    #[inline]
    pub fn queued(&self) -> Option<[f32; 3]> {
        self.queued
    }

    /// 算出新颜色（`get_new_color`，`colorpicker.py:135-141` 与 `:179-185`）。
    ///
    /// `pick_rgb` 是屏幕上取到的 RGB（`tdw.pick_color` 的返回值），
    /// `brush` 是上一次的颜色（源码的 `brush_color` 参数）。
    fn get_new_color(&self, pick_rgb: [f32; 3], brush: &PickColor) -> PickColor {
        match self.variant {
            PickVariant::Full => {
                // "正常取色，但无彩色时保留色相"：s 或 v 为 0 的取样色没有
                // 可用的色相，直接采用会把用户的色相抹成 0（灰色/黑色）。
                let pick_hsv = rgb_to_hsv(pick_rgb);
                if pick_hsv[1] == 0.0 || pick_hsv[2] == 0.0 {
                    PickColor::Hsv([brush.hsv()[0], pick_hsv[1], pick_hsv[2]])
                } else {
                    PickColor::Rgb(pick_rgb)
                }
            }
            _ => {
                // `ColorPickModeHCYBase.get_new_color`：在 HCY 里改一个分量，
                // 再把 c/y 夹进合法范围（避免除零与纯白/纯黑）。
                //
                // 记账细节：源码的 `self._color` 一旦变成 `HCYColor` 就**直接
                // 复用**它的 h/c/y（`HCYColor(color=…)` 的 isinstance 分支，
                // `lib/color.py:521-526`）；本移植的颜色通道是 HSV，所以每帧
                // 都要从 HSV 重建一次画笔色的 HCY。对 y 落在 [Y_MIN, Y_MAX]
                // 的正常颜色，这两个往返是精确的（`RGB_to_HCY`/`HCY_to_RGB`
                // 互逆，见单测），只有纯黑/纯白才落进特例分支。
                let pick_hcy = hcy_from_hsv(rgb_to_hsv(pick_rgb));
                let brush_hcy = hcy_from_hsv(brush.hsv());
                let mut new = self.get_new_hcy_color(pick_hcy, brush_hcy);
                new[1] = new[1].max(C_MIN);
                new[2] = new[2].clamp(Y_MIN, Y_MAX);
                PickColor::Hcy(new)
            }
        }
    }

    /// `get_new_hcy_color` 的分派（`colorpicker.py:207-250`）。
    fn get_new_hcy_color(&self, pick: [f32; 3], mut brush: [f32; 3]) -> [f32; 3] {
        match self.variant {
            PickVariant::Hue => {
                // 取样色本身得有彩度与亮度，否则"取色相"没有意义
                if pick[1] >= C_MIN && pick[2] >= Y_MIN {
                    brush[0] = pick[0];
                }
            }
            PickVariant::Chroma => brush[1] = pick[1],
            PickVariant::Luma => brush[2] = pick[2],
            // 完整取色不走 HCY 分支
            PickVariant::Full => {}
        }
        brush
    }

    /// 取一次色（`_pick_color`，`colorpicker.py:143-152`）。
    ///
    /// `direct=True` 时立刻写进出口，否则排进队列等 [`Self::flush_queued`]
    /// （源码是 `cm.set_color()` / `_queue_color_change()` 两条路）。
    fn pick_color_at(
        &mut self,
        ctx: &mut ModeContext<'_>,
        x: f32,
        y: f32,
        direct: bool,
        viewzoom: f32,
    ) {
        // 源码 `tdw.pick_color` 在 device 坐标系里取 3×3 像素；换算到模型
        // 坐标就是间距 `1 / zoom`（旋转下的方格方向近似，见 document.rs 记账）。
        self.last_viewzoom = if viewzoom.is_finite() && viewzoom > 0.0 {
            viewzoom
        } else {
            1.0
        };
        let step = 1.0 / self.last_viewzoom;
        let Some(pick_rgb) = ctx.doc.pick_color_scaled(x, y, step) else {
            return;
        };
        // `if pick_color != self._color`：颜色没变就什么都不做，免得每帧
        // 都往历史/笔刷里推一遍同样的颜色
        if rgb_eq(pick_rgb, self.last_color.rgb()) {
            return;
        }
        let new_color = self.get_new_color(pick_rgb, &self.last_color);
        self.last_color = new_color;
        let hsv = new_color.hsv();
        if direct {
            ctx.set_picked_color(hsv);
        } else {
            self.queued = Some(hsv);
        }
    }

    /// 把排队的颜色落地（`_change_color`，`colorpicker.py:162-167`）。
    pub fn flush_queued(&mut self, ctx: &mut ModeContext<'_>) {
        if let Some(hsv) = self.queued.take() {
            ctx.set_picked_color(hsv);
        }
    }

    /// 拖拽结束时是否该出栈（`OneshotDragMode.drag_stop_cb` 的 `pop` 规则，
    /// `mode.py:1302-1317`）。
    ///
    /// 规则本身在 [`OneshotDragMode::should_pop`] 里；本移植把结果算出来留给
    /// GUI 决定（模式对象够不到模式栈，见模块记账）。
    pub fn pop_after_drag(&self, modifiers: super::Modifiers) -> bool {
        self.oneshot.should_pop(modifiers)
    }
}

impl InteractionMode for ColorPickMode {
    /// `get_name()`。
    fn name(&self) -> &'static str {
        self.variant.display_name()
    }

    /// `get_usage()`。
    fn usage(&self) -> &str {
        self.variant.usage()
    }

    /// `ACTION_NAME`（`colorpicker.py:49` 等）。
    fn action_name(&self) -> Option<&'static str> {
        Some(self.variant.action_name())
    }

    /// `pointer_behavior = Behavior.EDIT_OBJECTS`（`colorpicker.py:55`）。
    fn pointer_behavior(&self) -> Behavior {
        Behavior::EDIT_OBJECTS
    }

    /// `scroll_behavior = Behavior.NONE`（`colorpicker.py:56`；
    /// 源码注释说它 grab 了指针，所以不叠 CHANGE_VIEW）。
    fn scroll_behavior(&self) -> Behavior {
        Behavior::NONE
    }

    /// `OneshotDragMode`（`mode.py:1298-1300`）：只叠在**非**一键式模式之上，
    /// 用完就回到原来那个模式。
    fn stackable_on(&self, mode: &dyn InteractionMode) -> bool {
        !mode.is_oneshot()
    }

    /// 见 [`InteractionMode::is_oneshot`]。
    fn is_oneshot(&self) -> bool {
        true
    }

    /// `ColorPickMode.enter`（`colorpicker.py:80-92`）：拿当前画笔颜色当基准，
    /// 键盘起手时立刻取一次色并把拖拽推迟到下一次 motion。
    ///
    /// 取色不作画：[`InteractionMode::is_painting`] /
    /// [`InteractionMode::is_live_updateable`] 都保持默认 `false`
    /// （取色不产生 brushwork，也不该被笔刷编辑器的实时更新波及）。
    fn enter(&mut self, ctx: &mut ModeContext<'_>) {
        // `self._color = cm.get_color()`
        self.last_color = PickColor::Hsv(ctx.color);
        self.queued = None;
        if self.started_from_key_press {
            // 键盘起手：用"最后一次指针事件的位置"立刻取一次色
            if let Some((x, y)) = self.enter_pick {
                self.pick_color_at(ctx, x, y, true, self.last_viewzoom);
            }
            // 拖拽要等下一次 motion 才开始
            self.start_drag_on_next_motion = true;
        }
        // 记账：`DragMode.enter`（`mode.py:1155-1190`）还要记录进入时的修饰键
        // （弹簧加载用），我们的 `enter()` 拿不到修饰键，改为在
        // `button_press` 里调 `self.oneshot.drag_mode.enter(ev.modifiers)`
        // （与 line.rs / fill.rs 同一做法）。
    }

    /// `ColorPickMode.leave`（`colorpicker.py:94-98`）：还有排队的颜色就落地，
    /// 然后去掉叠加层（未移植）、`DragMode.leave` 停掉拖拽。
    fn leave(&mut self, ctx: &mut ModeContext<'_>) {
        self.flush_queued(ctx);
        self.oneshot.drag_mode.drag.stop();
        self.start_drag_on_next_motion = false;
    }

    /// `button_press_cb`（`colorpicker.py:100-104`）：**先直接取色**，
    /// 再把事件交给 `DragMode` 去开拖拽。
    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        // 源码这里不筛按键号（`button_press_cb` 对任何键都先取色）
        self.pick_color_at(ctx, ev.x, ev.y, true, ev.viewzoom);
        self.start_drag_on_next_motion = false;
        let drag = &mut self.oneshot.drag_mode;
        if drag.drag.in_drag {
            // `DragMode.button_press_cb`（`mode.py:1210-1215`）：已在拖拽中，
            // 只补记起手按键（让单击能结束键盘起的拖拽）
            if drag.drag.start_button.is_none() {
                drag.drag.start_button = Some(ev.button);
            }
        } else {
            // `DragMode.enter` + `_start_drag`：记录修饰键（弹簧加载）并起手
            drag.enter(ev.modifiers);
            drag.drag.start(ev.x, ev.y);
            drag.drag.start_button = Some(ev.button);
        }
        true
    }

    /// `motion_notify_cb`（`colorpicker.py:106-110` + `DragMode`，`mode.py:1229-1240`）。
    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if self.start_drag_on_next_motion {
            // 键盘起手：第一次 motion 才开始拖拽
            self.oneshot.drag_mode.drag.start(ev.x, ev.y);
            self.oneshot.drag_mode.drag.start_button = Some(ev.button);
            self.start_drag_on_next_motion = false;
        }
        if !self.oneshot.drag_mode.drag.in_drag {
            return false;
        }
        // `DragMode.motion_notify_cb` → `drag_update_cb`（`colorpicker.py:116-119`）：
        // 拖拽中持续取色；这一路是**排队**的（源码 60fps 节流），
        // 落地由 `process_pending` 完成。
        self.pick_color_at(ctx, ev.x, ev.y, false, ev.viewzoom);
        true
    }

    /// `button_release_cb`（`mode.py:1223-1227`）+ `drag_stop_cb`
    /// （`colorpicker.py:112-114`：去叠加层、按一键式规则出栈）。
    fn button_release(&mut self, _ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        let drag = &self.oneshot.drag_mode;
        if !drag.drag.in_drag || ev.button != drag.drag.start_button.unwrap_or(1) {
            return false;
        }
        self.oneshot.drag_mode.drag.stop();
        true
    }

    /// `DragMode.key_press_cb`（`mode.py:1242-1246`）：拖拽中一律吃掉按键。
    ///
    /// 空格起手拖拽的分支在本移植里由画布平移接手（见模块记账）。
    fn key_press(&mut self, _ctx: &mut ModeContext<'_>, _key: Key) -> bool {
        self.oneshot.drag_mode.drag.in_drag
    }

    /// 键盘 `r` 起手的取色：松开按键即出栈（`colorpicker.py:53` 的
    /// `keyup_timeout = 0`，`document.py:2200` 的 `_exit_mode_early_keyup_cb`）。
    fn pop_on_key_release(&self) -> bool {
        self.started_from_key_press
    }

    /// 工具条/菜单起手的取色：一次拖拽后按 Oneshot 规则出栈
    /// （`mode.py:1302-1317`）。
    fn oneshot_pop_after_drag(&self, modifiers: super::Modifiers) -> bool {
        self.pop_after_drag(modifiers)
    }

    /// idle 的等价物：把排队的颜色落地（`_change_color`）。
    fn process_pending(&mut self, ctx: &mut ModeContext<'_>) -> bool {
        self.flush_queued(ctx);
        // 落地后没有剩余排队
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::hokusai::{Brush, BrushState};
    use crate::mode::Modifiers;
    use pigment_surface::layer::Layer;

    /// 8bpc sRGB 的纯色块（与最终像素一一对应，断言可直接写字面值）。
    const RED: [u8; 4] = [204, 51, 51, 255];
    const BLUE: [u8; 4] = [51, 51, 204, 255];
    /// 测试用的"画笔颜色"（画布上不会出现这个色，便于区分颜色来源）。
    const BRUSH_COLOR: [f32; 3] = [0.42, 0.6, 0.5];

    /// 用 8bpc 像素直接铺一块（绕开笔刷，颜色可精确断言）。
    fn fill_block(doc: &mut Document, x: i32, y: i32, w: u32, h: u32, px: [u8; 4]) {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&px);
        }
        doc.surface_mut().load_from_rgba8(&buf, x, y, w, h);
    }

    /// 读通道当前的值（从上下文这一侧读，避免与 `Some(&mut picked)` 抢借用）。
    fn channel(ctx: &ModeContext<'_>) -> Option<[f32; 3]> {
        ctx.picked_color.as_deref().copied().flatten()
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn rgb8(px: [u8; 4]) -> [f32; 3] {
        [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ]
    }

    /// 一段"按下 →（每站移动 + 一帧 process_pending）→ 松开 → leave"，
    /// 返回通道里最后落地的颜色。
    fn run_pick(
        doc: &mut Document,
        variant: PickVariant,
        from: (f32, f32),
        motions: &[(f32, f32)],
    ) -> Option<[f32; 3]> {
        let brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let mut picked: Option<[f32; 3]> = None;
        let mut mode = ColorPickMode::new(variant);
        {
            let mut ctx = ModeContext {
                doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: BRUSH_COLOR,
                picked_color: Some(&mut picked),
            };
            InteractionMode::enter(&mut mode, &mut ctx);
            InteractionMode::button_press(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(from.0, from.1, 0.0, 1),
            );
            let mut last = from;
            for (i, (x, y)) in motions.iter().enumerate() {
                InteractionMode::motion(
                    &mut mode,
                    &mut ctx,
                    &PointerEvent::motion(*x, *y, (i as f64 + 1.0) * 16.0),
                );
                InteractionMode::process_pending(&mut mode, &mut ctx);
                last = (*x, *y);
            }
            InteractionMode::button_release(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(last.0, last.1, 64.0, 1),
            );
            InteractionMode::leave(&mut mode, &mut ctx);
        }
        picked
    }

    /// 单击：通道里拿到该像素的 HSV；**不**产生命令，也**不**改画面。
    #[test]
    fn click_picks_the_color_without_painting() {
        let mut doc = Document::new();
        fill_block(&mut doc, 0, 0, 8, 8, RED);
        let undo_before = doc.command_stack.undo_len();
        let tiles_before = doc.surface().tile_count();

        let got = run_pick(&mut doc, PickVariant::Full, (4.0, 4.0), &[]);
        let hsv = got.expect("单击应把颜色写进通道");
        let want = rgb_to_hsv(rgb8(RED));
        assert!(near(hsv[0], want[0]), "色相 {} vs {}", hsv[0], want[0]);
        assert!(near(hsv[1], want[1]), "饱和 {} vs {}", hsv[1], want[1]);
        assert!(near(hsv[2], want[2]), "明度 {} vs {}", hsv[2], want[2]);

        assert_eq!(
            doc.command_stack.undo_len(),
            undo_before,
            "取色不该产生任何命令"
        );
        assert_eq!(doc.surface().tile_count(), tiles_before, "取色不该改画面");
    }

    /// 拖动：每一站都排队，由 `process_pending`（idle 的等价物）落地；
    /// 整段拖拽同样不产生命令、不改画面。
    #[test]
    fn dragging_updates_the_color_through_the_queue() {
        let mut doc = Document::new();
        fill_block(&mut doc, 0, 0, 8, 8, RED);
        fill_block(&mut doc, 16, 0, 8, 8, BLUE);
        let undo_before = doc.command_stack.undo_len();
        let tiles_before = doc.surface().tile_count();

        let brush: Brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let mut picked: Option<[f32; 3]> = None;
        let mut mode = ColorPickMode::new(PickVariant::Full);
        {
            let mut ctx = ModeContext {
                doc: &mut doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: BRUSH_COLOR,
                picked_color: Some(&mut picked),
            };
            InteractionMode::enter(&mut mode, &mut ctx);
            InteractionMode::button_press(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(4.0, 4.0, 0.0, 1),
            );
            // 按下是 direct：立刻拿到红
            let red = channel(&ctx).expect("按下应立即取色");
            assert!(near(red[1], 0.75), "红块饱和 0.75，实得 {}", red[1]);

            // 拖到蓝块：motion 只排队（源码 60fps 节流），通道还是旧色
            InteractionMode::motion(&mut mode, &mut ctx, &PointerEvent::motion(20.0, 4.0, 16.0));
            assert!(mode.queued().is_some(), "拖拽中的取色应排队");
            assert!(
                near(channel(&ctx).expect("通道有值")[1], 0.75),
                "排队期间通道仍是旧色"
            );

            // idle 的等价物：本帧落地
            InteractionMode::process_pending(&mut mode, &mut ctx);
            let blue = channel(&ctx).expect("落地后通道有值");
            assert!(near(blue[0], 4.0 / 6.0), "蓝块色相 2/3，实得 {}", blue[0]);
            assert!(near(blue[1], 0.75), "蓝块饱和 0.75，实得 {}", blue[1]);
            assert!(mode.queued().is_none(), "落地后队列应清空");

            InteractionMode::button_release(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(20.0, 4.0, 32.0, 1),
            );
            InteractionMode::leave(&mut mode, &mut ctx);
        }

        assert_eq!(
            doc.command_stack.undo_len(),
            undo_before,
            "拖拽取色不该产生命令"
        );
        assert_eq!(doc.surface().tile_count(), tiles_before, "取色不该改画面");
    }

    /// 无彩色取样（白/灰/黑）时保留画笔色相（`colorpicker.py:135-141`）。
    #[test]
    fn achromatic_pick_keeps_the_brush_hue() {
        // 空白文档 → 合成结果就是背景白（无彩色）
        let mut doc = Document::new();
        let got = run_pick(&mut doc, PickVariant::Full, (4.0, 4.0), &[]);
        let hsv = got.expect("通道应有颜色");
        assert!(
            near(hsv[0], BRUSH_COLOR[0]),
            "色相应保留画笔的 {}，实得 {}",
            BRUSH_COLOR[0],
            hsv[0]
        );
        assert!(near(hsv[1], 0.0), "饱和取自取样色（白 → 0），实得 {}", hsv[1]);
        assert!(near(hsv[2], 1.0), "明度取自取样色（白 → 1），实得 {}", hsv[2]);
    }

    /// 三个 HCY 变体各只改自己那一个分量（`colorpicker.py:179-250`）。
    fn check_variant(variant: PickVariant, take_h: bool, take_c: bool, take_y: bool) {
        let mut doc = Document::new();
        // 蓝块：与画笔色（绿系，h=0.42）在 H/C/Y 上都不同
        fill_block(&mut doc, 0, 0, 8, 8, BLUE);
        let pick_rgb = doc.pick_color(4.0, 4.0).expect("取样点应有色");
        let pick_hcy = hcy_from_hsv(rgb_to_hsv(pick_rgb));
        let brush_hcy = hcy_from_hsv(BRUSH_COLOR);

        let got = run_pick(&mut doc, variant, (4.0, 4.0), &[]).expect("通道应有颜色");
        // 通道是 HSV（`brush.set_color_hsv()` 吃的东西），折回 HCY 才能比分量
        let got_hcy = hcy_from_hsv(got);

        let case = |label: &str, got: f32, from_pick: f32, from_brush: f32, want_pick: bool| {
            let want = if want_pick { from_pick } else { from_brush };
            assert!(
                (got - want).abs() < 5e-3,
                "{variant:?} 的 {label} 应为 {want}，实得 {got}"
            );
        };
        case("h", got_hcy[0], pick_hcy[0], brush_hcy[0], take_h);
        case("c", got_hcy[1], pick_hcy[1], brush_hcy[1], take_c);
        case("y", got_hcy[2], pick_hcy[2], brush_hcy[2], take_y);
    }

    #[test]
    fn hue_variant_only_changes_the_hue() {
        check_variant(PickVariant::Hue, true, false, false);
    }

    #[test]
    fn chroma_variant_only_changes_the_chroma() {
        check_variant(PickVariant::Chroma, false, true, false);
    }

    #[test]
    fn luma_variant_only_changes_the_luma() {
        check_variant(PickVariant::Luma, false, false, true);
    }

    /// 单选动作进来**不**立即取色（`colorpicker.py:34-39`）：要先按一下画布。
    #[test]
    fn radio_mode_does_not_pick_on_enter() {
        let mut doc = Document::new();
        fill_block(&mut doc, 0, 0, 8, 8, RED);
        let brush: Brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let mut picked: Option<[f32; 3]> = None;
        {
            let mut ctx = ModeContext {
                doc: &mut doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: BRUSH_COLOR,
                picked_color: Some(&mut picked),
            };
            let mut mode = ColorPickMode::new(PickVariant::Full);
            InteractionMode::enter(&mut mode, &mut ctx);
            assert!(channel(&ctx).is_none(), "单选动作进来时不该取色");
            // `enter()` 记下了当前画笔颜色（源码 `self._color = cm.get_color()`）
            assert_eq!(mode.last_color(), PickColor::Hsv(BRUSH_COLOR));
        }
    }

    /// `Flip*`（键盘）进来：`enter()` 就在最后一次指针位置取色，
    /// 拖拽推迟到下一次 motion（`colorpicker.py:80-92`）。
    #[test]
    fn flip_mode_picks_immediately_on_enter() {
        let mut doc = Document::new();
        fill_block(&mut doc, 0, 0, 8, 8, RED);
        let brush: Brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let mut picked: Option<[f32; 3]> = None;
        {
            let mut ctx = ModeContext {
                doc: &mut doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: BRUSH_COLOR,
                picked_color: Some(&mut picked),
            };
            let mut mode = ColorPickMode::flip(PickVariant::Full, Some((4.0, 4.0)));
            InteractionMode::enter(&mut mode, &mut ctx);
            let hsv = channel(&ctx).expect("键盘起手应在 enter 时立即取色");
            assert!(near(hsv[1], 0.75), "应取到红色，实得 {hsv:?}");
            assert!(
                mode.start_drag_on_next_motion,
                "拖拽要等下一次 motion 才开始"
            );
        }
        // 指针不在画布上（源码 `x`/`y` 为 None）→ 不立即取色
        let mut no_pos: Option<[f32; 3]> = None;
        let mut state = BrushState::default();
        {
            let mut ctx = ModeContext {
                doc: &mut doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: BRUSH_COLOR,
                picked_color: Some(&mut no_pos),
            };
            let mut no_event = ColorPickMode::flip(PickVariant::Full, None);
            InteractionMode::enter(&mut no_event, &mut ctx);
            assert!(channel(&ctx).is_none(), "没有指针位置时不该取色");
        }
    }

    /// 取色读的是**合成结果**（sample merged）：当前层是组、被锁都不影响，
    /// 只有"层被隐藏/不参与合成"才会看出区别
    /// （`colorpicker.py:143-145`、`tileddrawwidget.py:991`）。
    #[test]
    fn picking_is_sample_merged_over_groups_and_locks() {
        let mut doc = Document::new();
        fill_block(&mut doc, 0, 0, 8, 8, RED);
        // 在绘画层之上插一个组并选中它（当前层 = 组，路径 [0]；绘画层变 [1]）
        let mut group = Layer::new_group();
        group.name = Some("G".into());
        doc.layers_mut().insert(0, group);
        doc.select_layer(&[0]);
        assert!(
            doc.layers().paintable_at(&[0]).is_none(),
            "当前层应是不可画的组"
        );

        let got = run_pick(&mut doc, PickVariant::Full, (4.0, 4.0), &[]);
        let hsv = got.expect("组上也能取到下层合成的颜色");
        assert!(near(hsv[1], 0.75), "应取到红色，实得 {hsv:?}");

        // 锁定绘画层：照样能取（合成不看锁）
        doc.set_layer_locked(&[1], true);
        let got = run_pick(&mut doc, PickVariant::Full, (4.0, 4.0), &[]);
        assert!(
            near(got.expect("锁定层也能取色")[1], 0.75),
            "锁定不该影响取色"
        );

        // 隐藏绘画层：它不再参与合成 → 取到背景白
        doc.set_layer_visible(&[1], false);
        let got = run_pick(&mut doc, PickVariant::Full, (4.0, 4.0), &[]);
        let hsv = got.expect("通道总有值");
        assert!(
            near(hsv[1], 0.0) && near(hsv[2], 1.0),
            "隐藏后应取到背景白，实得 {hsv:?}"
        );
    }

    /// 出栈规则（`OneshotDragMode.drag_stop_cb`，`mode.py:1302-1317`）。
    #[test]
    fn pop_after_drag_rules_match_oneshot_semantics() {
        // 单选动作进来：`temporary_activation=True` → 无修饰键时拖完就出栈
        let radio = ColorPickMode::new(PickVariant::Full);
        assert!(radio.pop_after_drag(Modifiers::NONE));
        // `Flip*` 进来：`temporary_activation=False` 且 `unmodified_persist`
        // → 拖完留在模式里（源码靠"松开 r"出栈，见模块记账）
        let flip = ColorPickMode::flip(PickVariant::Full, None);
        assert!(!flip.pop_after_drag(Modifiers::NONE));
    }

    #[test]
    fn pick_mode_stacks_over_freehand_and_pops_back() {
        use crate::document::Document;
        use crate::mode::freehand::FreehandMode;
        use crate::mode::{ModeStack, Modifiers};
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
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
        let mut stack = ModeStack::new(|| Box::new(FreehandMode::new()));
        stack.context_push(Box::new(ColorPickMode::new(PickVariant::Full)), &mut ctx);
        assert_eq!(stack.len(), 2, "取色应叠在自由手绘之上，保留原工具");
        assert!(stack.top_pops_after_drag(Modifiers::NONE), "工具条起手：拖完回原工具");
        stack.pop(&mut ctx);
        assert_eq!(stack.top().name(), "Freehand Drawing");

        // 键盘 Flip：不在拖拽后弹，而是松开按键时弹
        stack.context_push(
            Box::new(ColorPickMode::flip(PickVariant::Full, None)),
            &mut ctx,
        );
        assert!(stack.top_pops_on_key_release());
        assert!(!stack.top_pops_after_drag(Modifiers::NONE));
        stack.pop(&mut ctx);
        assert_eq!(stack.top().name(), "Freehand Drawing");
    }

    /// HCY 换算与源码一致：`lib/color.py:851-859` 的 doctest 是"往返无误差"，
    /// 外加几个已知值（纯红的 luma = 0.3、无彩色的 chroma = 0）。
    #[test]
    fn hcy_conversions_match_source() {
        let n = 8;
        for r in 0..=n {
            for g in 0..=n {
                for b in 0..=n {
                    let rgb = [
                        r as f32 / n as f32,
                        g as f32 / n as f32,
                        b as f32 / n as f32,
                    ];
                    let back = hcy_to_rgb(rgb_to_hcy(rgb));
                    for i in 0..3 {
                        assert!(
                            (back[i] - rgb[i]).abs() < 1e-5,
                            "RGB→HCY→RGB 往返失败：{rgb:?} → {back:?}（分量 {i}）"
                        );
                    }
                }
            }
        }
        let red = rgb_to_hcy([1.0, 0.0, 0.0]);
        assert!(red[0].abs() < 1e-6, "纯红色相 0，实得 {}", red[0]);
        assert!(
            (red[2] - HCY_RED_LUMA as f32).abs() < 1e-6,
            "纯红 luma = 0.3，实得 {}",
            red[2]
        );
        assert!(red[1] > 0.99 && red[1] < 1.01, "纯红彩度 ≈ 1，实得 {}", red[1]);
        // 无彩色：彩度 0（源码靠 `r == g == b` 分支避免除零）
        assert_eq!(rgb_to_hcy([1.0, 1.0, 1.0])[1], 0.0);
        assert_eq!(rgb_to_hcy([0.0, 0.0, 0.0])[1], 0.0);
    }
}
