//! 颜色工具模型 —— `gui/colors/adjbases.py` + `sliders.py` + `combined.py`
//! 里**与 GTK 无关**的部分（B4-2）。
//!
//! ## 源码里这块怎么分层
//!
//! | 源码 | 职责 | 这里 |
//! |---|---|---|
//! | `adjbases.py:62-359` `ColorManager` | 持当前色 + 历史，并通知已注册的 adjuster | [`ColorManager`]（B4-1）+ 本模块的 [`ColorTool`] |
//! | `adjbases.py:360-429` `ColorAdjuster` | 绑定管理器；`get/set_managed_color()` 只是**转调管理器**；两个通知钩子 | [`ColorAdjuster`] trait（钩子）+ [`ColorTool`]（绑定与读写） |
//! | `adjbases.py:432-830` `ColorAdjusterWidget` | 指针事件状态机：按下 / 移动 / 抬起、拖动阈值、按钮 2/3 的 HCY 弯曲 | [`ColorChanger`] |
//! | `adjbases.py:933-1083` `SliderColorAdjuster` | 滑条坐标 ↔ 条值、滚轮增量、条值 ↔ 颜色 | [`point_to_amount`] / [`scroll_delta`] / [`ComponentSlider`] |
//! | `sliders.py:163-305` 九个分量滑条 | 条值写哪个分量、从颜色读哪个分量 | [`SliderKind`] / [`ComponentSlider`] |
//! | `combined.py:20-66` `CombinedAdjusterPage` | "一个页面把它拿到的管理器分发给所有子控件" + 页面元数据 | [`ColorTool`] 的注册表（元数据属 B4-3） |
//!
//! ## 没有「允许的颜色类型」白名单
//!
//! `adjbases.py` 全文没有 `_clamp_to_allowed`、没有「允许的颜色类型」集合 ——
//! 用 `grep` 搜不到这类符号。共享颜色的**类型**不是被夹取出来的，而是
//! "谁最后写回谁说了算"：`set_managed_color()` 把 adjuster 造出来的那个
//! `UIColor` 子类交给管理器，管理器 `copy()` 后存下（`adjbases.py:204-226`），
//! 于是中央色的类型跟着最后一次调整器的色彩空间变
//! （`lib/color.py:13-18` 的注释）。本移植照抄这个语义，不额外发明白名单。
//!
//! ## 没移植的（记账，见交付说明）
//!
//! - `adjbases.py:93-101` `CachedBgWidgetMixin` / `:432-830` 里的 GTK 装配
//!   （`connect("draw")`、`Gtk.EventBox`、Gdk 光标、DnD、`queue_draw`）：
//!   属控件层，B4-3。
//! - `adjbases.py:1011-1053` `get_bar_amount_for_color` 的**标记线绘制**与
//!   `:933-1009` 的渐变背景：纯绘制，B4-3。
//! - `adjbases.py:640-656` / `sliders.py:166-305` 的 `get_background_validity()`：
//!   缓存重绘令牌（"哪几个通道变了才需要重画背景"），是控件缓存的簿记；
//!   egui 立即模式没有这份缓存，B4-3 视需要再接。
//! - `adjbases.py:1086-1338` `HueSaturationWheelMixin`（极坐标 ↔ 颜色、
//!   `SAT_GAMMA`、`get_radius/get_center`）：色轮几何，B4-3 / B4-4。
//! - `adjbases.py:289-345` 色相畸变表 `distort_hue()` / `undistort_hue()`：
//!   只服务色轮控件，B4-3 / B4-4（B4-1 的交付说明里也记过）。
//! - `gui/colortools.py` 全文（`_PageToolAdapter`、`_SimpleAdjusterAdapter`
//!   与各 Tool 类）与 `combined.py:29-66` 的页面元数据：纯 GTK 装配 / 可
//!   翻译字符串，B4-3。
//! - `sliders.py:25-160` `ComponentSlidersAdjusterPage`：Gtk.Grid 的行布局
//!   （R/G/B/H/C/Y 六行、H 行 `margin_top=12`）与页签元数据，B4-3。
//! - `adjbases.py:461` / `:739-788` 之外的拖放（`IS_DRAG_SOURCE` 的
//!   `drag_begin_with_coordinates()`、`application/x-color` DnD）：需要 GTK
//!   的 DnD 事件模型。本模块只把"越过拖动阈值"投影成
//!   [`DragUpdate::BeginDrag`]，DnD 本身留给 B4-3。
//! - `adjbases.py:679-695` 的双击详情对话框：需要 GTK 对话框；本模块只把状态
//!   复位并把"请开对话框"投影成 [`PressOutcome::DetailsDialog`]，落色仍走
//!   调用方的 `set_color_at_position()`（B4-3）。
//! - `adjbases.py:412-419` `get_prefs()`：偏好系统属 B6。
//!
//! ## 门槛
//!
//! `tests/color_tools_vectors.rs` 拿 `tests/fixtures/color_tools_vectors.txt`
//! 逐位比对。fixture 由 `tests/tools/gen-color-tools-vectors.py` 用 `ast` 从
//! **真源码**抽出这些函数的函数体、包成普通函数后 `exec` 算出来
//! （`adjbases.py` / `sliders.py` / `gui/colors/util.py`）：不是照抄的公式。
//! 依赖 GTK 控件状态的部分（按下 / 抬起的状态转移）用源码行号手工推导的
//! 期望值写在 `tests/color_tools.rs` 里。

use super::manager::ColorManager;
use super::model::{Color, HcyColor, HsvColor, RgbColor, UIColor};

// ---------------------------------------------------------------------------
// 数值常量（逐条对应源码行）
// ---------------------------------------------------------------------------

/// 没有管理器的 adjuster 的兜底色 —— `adjbases.py:371`
/// `_DEFAULT_COLOR = RGBColor(0.0, 0.19, 0.33)`。
///
/// 源码只在 `get_managed_color()` 发现 `color_manager is None` 时返回它
/// （`adjbases.py:399-403`）。本移植里 adjuster 不持管理器（见模块文档），
/// 这个兜底值留给 B4-3 作为"还没有工具时的初始色"。
pub const DEFAULT_MANAGED_COLOR: RgbColor = RgbColor {
    r: 0.0,
    g: 0.19,
    b: 0.33,
};

/// 滚轮每格改变多少条值 —— `adjbases.py:449` `SCROLL_DELTA = 0.015`。
pub const SCROLL_DELTA: f64 = 0.015;

/// 滑条控件边框宽度 —— `adjbases.py:455` `BORDER_WIDTH = 2`。
///
/// 滑条的坐标 ↔ 条值换算要把这段边框扣掉（`adjbases.py:1055-1067`）。
pub const BORDER_WIDTH: f64 = 2.0;

/// 拖动阈值基数的上限 —— `adjbases.py:531`
/// `thres = min(4, int(settings.get_property("gtk-dnd-drag-threshold")))`。
pub const DRAG_THRESHOLD_CAP: i32 = 4;

/// 拖动阈值的倍数 —— `adjbases.py:532` `self._drag_threshold = 2 * thres`。
///
/// 源码注释（`:528-530`）："Use twice the standard threshold because tablets
/// always send more tiny motions than mice."
pub const DRAG_THRESHOLD_FACTOR: i32 = 2;

/// `gui/colors/util.py:18-24` 的 `clamp()`。
///
/// 逐行照抄（包括"区间反了也不报错"和"原样返回"这两点，见
/// `f64::clamp` 的差异）：
///
/// ```python
/// def clamp(v, bottom, top):
///     if v > top:
///         return top
///     if v < bottom:
///         return bottom
///     return v
/// ```
///
/// 色轮/色方块（`color::wheels`，B4-4b）也用它，所以是 crate 可见。
pub(crate) fn clamp(v: f64, bottom: f64, top: f64) -> f64 {
    if v > top {
        return top;
    }
    if v < bottom {
        return bottom;
    }
    v
}

// ---------------------------------------------------------------------------
// ColorTool：共享颜色 + adjuster 注册表
// ---------------------------------------------------------------------------

/// 可以被共享颜色变化通知到的对象 —— `adjbases.py:360-429` 的 `ColorAdjuster`。
///
/// 源码的 `ColorAdjuster` 基类只有四个东西：管理器绑定（`:373-395`）、
/// `get/set_managed_color()`（`:397-412`，**转调管理器**，自己不持色）、
/// 偏好读取（`:414-419`，属 B6），以及这两个通知钩子（`:421-429`）。
/// 前三者在 Rust 侧由 [`ColorTool`] 承担，这里只留钩子 —— 它就是
/// `queue_draw()` 之类的挂载点（`adjbases.py:827-829`）。
///
/// 注意源码用的是 `weakref.WeakSet`（`adjbases.py:112`）：管理器不持有
/// adjuster 的生命期。本移植的 [`ColorTool`] 用 `Box` **拥有** adjuster，
/// 这是"没有 GTK 控件树"时最保守的简化：注册即持有，移除即 drop。
pub trait ColorAdjuster {
    /// 共享颜色变了 —— `adjbases.py:423-425`（`ColorAdjuster.color_updated()`）。
    fn color_updated(&mut self) {}

    /// 颜色历史变了 —— `adjbases.py:427-429`。
    fn color_history_updated(&mut self) {}
}

/// 一个颜色工具 = 共享颜色 + 一组 adjuster —— `ColorManager` 与
/// `ColorAdjuster.set_color_manager()` 的合并模型。
///
/// 分工（逐条对上 `adjbases.py`）：
///
/// - 持色：`_color` / `_hist` 在 [`ColorManager`] 里（B4-1），本结构只转发；
/// - 注册：`add_adjuster()` / `remove_adjuster()` / `get_adjusters()`
///   （`adjbases.py:188-200`，后者在本移植里是 [`ColorTool::adjuster_count`]），
///   以及 `set_color_manager()`（`:375-385`）与
///   `_PageToolAdapter.set_color_manager()`（`gui/colortools.py:68-71`）做的
///   "把管理器分发给所有子 adjuster" —— 在 Rust 侧就是把它们注册进来；
/// - 传播：`set_color()` 里 `for adj in self._adjusters: adj.color_updated()`
///   （`adjbases.py:216-219`）；历史同理（`:264-266`）。
///
/// `combined.py:20-66` 的 `CombinedAdjusterPage` 在模型侧的职责正是
/// "把管理器分发给组件（`set_color_manager`）"，也就是这里的注册表；
/// 页面的图标 / 标题 / 描述 / `get_page_widget()` 是 GTK 装配，见模块文档。
pub struct ColorTool {
    /// 共享的中央色与历史（源码 `ColorManager`，B4-1）。
    manager: ColorManager,
    /// 已注册的 adjuster（源码 `ColorManager._adjusters`，顺序不作为语义）。
    adjusters: Vec<Box<dyn ColorAdjuster>>,
}

impl Default for ColorTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorTool {
    /// 新工具：当前色/历史按源码的启动路径初始化（[`ColorManager::new`]）。
    pub fn new() -> Self {
        Self {
            manager: ColorManager::new(),
            adjusters: Vec::new(),
        }
    }

    // ---- 共享颜色（`adjbases.py:202-226`）----

    /// `ColorManager.get_color()`（`adjbases.py:222-224`）：取一份拷贝。
    pub fn get_color(&self) -> Color {
        self.manager.get_color()
    }

    /// `ColorManager.set_color()`（`adjbases.py:204-220`）。
    ///
    /// 顺序照抄：先按相等短路（`:213-214`，**相等就不通知**），再存拷贝，
    /// 再逐个 `adj.color_updated()`（`:216-219`），最后是管理器自己的
    /// `color_updated()` 事件（`:220`；egui 立即模式里没人订阅，省掉）。
    ///
    /// 源码紧接着还有 `self._palette.match_color(color)`（`:219`）——
    /// 调色板属 B4-4。
    ///
    /// 返回是否真的变了（应用层据此决定要不要重推笔刷）。
    pub fn set_color(&mut self, color: Color) -> bool {
        let changed = self.manager.set_color(color);
        if changed {
            for adj in self.adjusters.iter_mut() {
                adj.color_updated();
            }
        }
        changed
    }

    /// `get_previous_color()`（`adjbases.py:279-281`）：历史里最近用过的颜色。
    pub fn get_previous_color(&self) -> Color {
        self.manager.get_previous_color()
    }

    // ---- 颜色历史（`adjbases.py:235-281`）----

    /// 历史快照（最旧 → 最新）—— `get_history()`（`adjbases.py:275-277`）。
    pub fn get_history(&self) -> &[Color] {
        self.manager.get_history()
    }

    /// `push_history()`（`adjbases.py:240-266`）：去重 → 追加 → 裁剪，然后
    /// 逐个 `adj.color_history_updated()`（`:264-266`）。
    pub fn push_history(&mut self, color: Color) {
        self.manager.push_history(color);
        for adj in self.adjusters.iter_mut() {
            adj.color_history_updated();
        }
    }

    // ---- adjuster 注册表（`adjbases.py:188-200`）----

    /// `add_adjuster()`（`adjbases.py:190-192`）：注册一个 adjuster。
    ///
    /// 返回它的句柄（下标），供 [`Self::remove_adjuster`] 用 —— 源码用对象本身
    /// 当句柄（`WeakSet` 的成员），Rust 用下标代替。
    pub fn add_adjuster(&mut self, adjuster: Box<dyn ColorAdjuster>) -> usize {
        self.adjusters.push(adjuster);
        self.adjusters.len() - 1
    }

    /// `remove_adjuster()`（`adjbases.py:194-196`）。
    ///
    /// 源码对不存在的成员会抛 `KeyError`；这里下标越界同样 panic。
    pub fn remove_adjuster(&mut self, handle: usize) -> Box<dyn ColorAdjuster> {
        self.adjusters.remove(handle)
    }

    /// 注册的 adjuster 数量（源码 `get_adjusters()` 的可数版本，`:198-200`）。
    pub fn adjuster_count(&self) -> usize {
        self.adjusters.len()
    }

    /// 只读地拿管理器 —— 对应 `ColorAdjuster.get_color_manager()`
    /// （`adjbases.py:387-393`），供 GUI 直接读历史/当前色。
    pub fn get_color_manager(&self) -> &ColorManager {
        &self.manager
    }
}

/// 指针状态机的"落色目标" —— [`ColorChanger`] 只需要两件事：
/// 读当前色、写新色（`ColorAdjuster` 的 `get/set_managed_color()`，
/// `adjbases.py:397-410`）。
///
/// 源码里这两个方法是 `ColorAdjuster` 基类的，`ColorTool` 与控件都继承它；
/// Rust 侧控件层（B4-4b 的色轮页）直接用 `ColorManager` 落色，所以把这两个
/// 方法抽成 trait：
///
/// - [`ColorTool`]（B4-2 的用法，带 adjuster 通知）；
/// - [`ColorManager`]（B4-4b 的控件层直接写管理器，由应用层随后 `derive_active`
///   把颜色推给笔刷）。
///
/// 方法名带 `sink_` 前缀，避免与两个类型各自的固有 `get_color`/`set_color`
/// 混淆（`ColorTool::set_color` 还负责通知 adjuster，语义更重）。
pub trait ColorSink {
    /// `get_managed_color()`（`adjbases.py:399-403`）—— 取一份拷贝。
    fn sink_color(&self) -> Color;

    /// `set_managed_color(color)`（`adjbases.py:405-410`）—— 返回是否真的变了。
    fn sink_set_color(&mut self, color: Color) -> bool;
}

impl ColorSink for ColorTool {
    fn sink_color(&self) -> Color {
        self.get_color()
    }

    fn sink_set_color(&mut self, color: Color) -> bool {
        self.set_color(color)
    }
}

impl ColorSink for ColorManager {
    fn sink_color(&self) -> Color {
        self.get_color()
    }

    fn sink_set_color(&mut self, color: Color) -> bool {
        self.set_color(color)
    }
}

// ---------------------------------------------------------------------------
// 滑条：坐标 ↔ 条值（`adjbases.py:1011-1083`）
// ---------------------------------------------------------------------------

/// 滚轮方向 —— `Gdk.ScrollDirection`（`adjbases.py:1073-1077`）。
///
/// GTK3 有五个值；源码只对 `DOWN`/`LEFT` 取反，其余（含 `SMOOTH`）当"正向"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollDirection {
    /// `Gdk.ScrollDirection.UP`
    Up,
    /// `Gdk.ScrollDirection.DOWN`
    Down,
    /// `Gdk.ScrollDirection.LEFT`
    Left,
    /// `Gdk.ScrollDirection.RIGHT`
    Right,
    /// `Gdk.ScrollDirection.SMOOTH`
    Smooth,
}

/// 滑条像素坐标 → 条值 —— `SliderColorAdjuster.point_to_amount()`
/// （`adjbases.py:1055-1067`）。
///
/// 参数对应源码的 `alloc.width`（或 `alloc.height`）、`BORDER_WIDTH`、
/// `vertical`：
///
/// ```python
/// len = alloc.width - 2 * self.BORDER_WIDTH
/// p = clamp(p - self.BORDER_WIDTH, 0, len)
/// amt = float(p) / len
/// if self.vertical:
///     amt = 1 - amt
/// ```
///
/// `length` 是分配到的**总长度**（含两侧边框），内部扣掉 `2 * border_width`。
/// 竖直滑条反向（`1 - amt`）：GTK 的 y 轴向下，条值向上增长。
///
/// # Panics
///
/// `length - 2 * border_width <= 0` 时 panic —— 源码在同一处除以
/// `len`（`:1064`），会抛 `ZeroDivisionError`；滑条控件的
/// `set_size_request()`（`adjbases.py:961-968`）保证真实调用不会走到这里。
#[must_use]
pub fn point_to_amount(pos: f64, length: f64, border_width: f64, vertical: bool) -> f64 {
    let len = length - 2.0 * border_width;
    assert!(
        len > 0.0,
        "滑条长度 {length} 扣掉两侧边框 {border_width} 后不是正数（源码此处会 ZeroDivisionError）"
    );
    let p = clamp(pos - border_width, 0.0, len);
    let amt = p / len;
    if vertical {
        1.0 - amt
    } else {
        amt
    }
}

/// 滚轮一格带来的条值增量 —— `SliderColorAdjuster.__scroll_cb()`
/// （`adjbases.py:1069-1077`）。
///
/// ```python
/// d = self.SCROLL_DELTA
/// if not self.vertical:
///     d *= -1
/// if event.direction in (Gdk.ScrollDirection.DOWN, Gdk.ScrollDirection.LEFT):
///     d *= -1
/// ```
///
/// 横向滑条整体取反（向左滚 = 条值变小，符合"横条从左往右变亮/变大"）；
/// 竖直条不取反。
#[must_use]
pub fn scroll_delta(vertical: bool, direction: ScrollDirection) -> f64 {
    let mut d = SCROLL_DELTA;
    if !vertical {
        d *= -1.0;
    }
    if matches!(direction, ScrollDirection::Down | ScrollDirection::Left) {
        d *= -1.0;
    }
    d
}

// ---------------------------------------------------------------------------
// 分量滑条（`sliders.py:163-305`）
// ---------------------------------------------------------------------------

/// 九个分量滑条 —— `sliders.py` 里每个滑条一个类。
///
/// 源码**只有**这九个：RGB 三原色、HSV 的 h/s/v、HCY 的 h/c/y。
/// `sliders.py` 里**没有** YCbCr 滑条（`YCbCrColor` 只在 `lib/color.py` 里
/// 作为一个色彩空间存在），所以这里也不凭空造。
///
/// `ComponentSlidersAdjusterPage` 实际摆出来的只有 R/G/B/H/C/Y 六条
/// （`sliders.py:42-73`），但九个类都完整可用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderKind {
    /// `RGBRedSlider`（`sliders.py:163-178`）：条值 → `RGBColor.r`。
    RgbR,
    /// `RGBGreenSlider`（`sliders.py:180-195`）。
    RgbG,
    /// `RGBBlueSlider`（`sliders.py:197-212`）。
    RgbB,
    /// `HSVHueSlider`（`sliders.py:214-225`）：条值 → `HSVColor.h`。
    HsvH,
    /// `HSVSaturationSlider`（`sliders.py:227-240`）。
    HsvS,
    /// `HSVValueSlider`（`sliders.py:242-252`）。
    HsvV,
    /// `HCYHueSlider`（`sliders.py:254-266`）：条值 → `HCYColor.h`。
    HcyH,
    /// `HCYChromaSlider`（`sliders.py:268-279`）。
    HcyC,
    /// `HCYLumaSlider`（`sliders.py:281-305`）。
    HcyY,
}

impl SliderKind {
    /// 全部九个（顺序同 `sliders.py` 里的类定义顺序）。
    pub const ALL: [SliderKind; 9] = [
        SliderKind::RgbR,
        SliderKind::RgbG,
        SliderKind::RgbB,
        SliderKind::HsvH,
        SliderKind::HsvS,
        SliderKind::HsvV,
        SliderKind::HcyH,
        SliderKind::HcyC,
        SliderKind::HcyY,
    ];

    /// 源码类名（`sliders.py` 的 `class` 名），fixture 与日志用它。
    pub fn class_name(self) -> &'static str {
        match self {
            SliderKind::RgbR => "RGBRedSlider",
            SliderKind::RgbG => "RGBGreenSlider",
            SliderKind::RgbB => "RGBBlueSlider",
            SliderKind::HsvH => "HSVHueSlider",
            SliderKind::HsvS => "HSVSaturationSlider",
            SliderKind::HsvV => "HSVValueSlider",
            SliderKind::HcyH => "HCYHueSlider",
            SliderKind::HcyC => "HCYChromaSlider",
            SliderKind::HcyY => "HCYLumaSlider",
        }
    }

    /// 由源码类名反查。
    pub fn from_class_name(name: &str) -> Option<SliderKind> {
        SliderKind::ALL.into_iter().find(|k| k.class_name() == name)
    }

    /// 这个滑条写回的颜色类型（源码 `get_color_for_bar_amount()` 里
    /// `RGBColor(color=...)` / `HSVColor(color=...)` / `HCYColor(color=...)`）。
    pub fn color_class_name(self) -> &'static str {
        match self {
            SliderKind::RgbR | SliderKind::RgbG | SliderKind::RgbB => "RGBColor",
            SliderKind::HsvH | SliderKind::HsvS | SliderKind::HsvV => "HSVColor",
            SliderKind::HcyH | SliderKind::HcyC | SliderKind::HcyY => "HCYColor",
        }
    }
}

/// 一个分量滑条 —— `SliderColorAdjuster` 的**数学部分**
/// （`adjbases.py:1011-1022` + `sliders.py:163-305`）。
///
/// 滑条自身不持状态，也不持管理器：源码的
/// `get_color_for_bar_amount(amt)` 读的是 `self.get_managed_color()`，
/// 这里把那份颜色作为参数传进来。B4-3 的控件每帧照这样算：
///
/// ```text
/// let col = tool.get_color();
/// let amt = slider.get_bar_amount_for_color(&col);   // 画标记线
/// let new = slider.get_color_for_bar_amount(&col, amt); // 拖动后落色
/// tool.set_color(new);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ComponentSlider {
    kind: SliderKind,
}

impl ComponentSlider {
    /// 按种类构造。
    pub fn new(kind: SliderKind) -> Self {
        Self { kind }
    }

    /// 这个滑条的种类。
    pub fn kind(&self) -> SliderKind {
        self.kind
    }

    /// `get_color_for_bar_amount(amt)` —— 源码九个滑条各自的实现
    /// （`sliders.py:171-174` / `:188-191` / `:205-208` / `:218-221` /
    /// `:233-236` / `:245-248` / `:258-261` / `:271-274` / `:294-297`）。
    ///
    /// 三步照抄：把被编辑的颜色转成该滑条所在空间的子类
    /// （`RGBColor(color=...)` 等，**不是**经 RGB 往返，见
    /// [`RgbColor::from_color`] 等），把目标分量**直接赋成** `amt`，返回。
    ///
    /// 两个容易看漏的点：
    /// - `amt` **不夹取**。滑条自身的 `point_to_amount()` 已经把它夹在
    ///   `0..1`，滚轮路径也夹到 `0..1`（`adjbases.py:1080`），此外源码没有
    ///   任何一层把分量夹回合法域。
    /// - HCY 的 `c` 合法域是 `0..=2`，而条值只能是 `0..1`，所以彩度滑条
    ///   只能拖到一半（源码就是这样，见 [`SliderKind::HcyC`]）。
    #[must_use]
    pub fn get_color_for_bar_amount(&self, managed: &Color, amt: f64) -> Color {
        match self.kind {
            // sliders.py:171-174
            SliderKind::RgbR => {
                let mut col = RgbColor::from_color(managed);
                col.r = amt;
                Color::Rgb(col)
            }
            // sliders.py:188-191
            SliderKind::RgbG => {
                let mut col = RgbColor::from_color(managed);
                col.g = amt;
                Color::Rgb(col)
            }
            // sliders.py:205-208
            SliderKind::RgbB => {
                let mut col = RgbColor::from_color(managed);
                col.b = amt;
                Color::Rgb(col)
            }
            // sliders.py:218-221
            SliderKind::HsvH => {
                let mut col = HsvColor::from_color(managed);
                col.h = amt;
                Color::Hsv(col)
            }
            // sliders.py:233-236
            SliderKind::HsvS => {
                let mut col = HsvColor::from_color(managed);
                col.s = amt;
                Color::Hsv(col)
            }
            // sliders.py:245-248
            SliderKind::HsvV => {
                let mut col = HsvColor::from_color(managed);
                col.v = amt;
                Color::Hsv(col)
            }
            // sliders.py:258-261
            SliderKind::HcyH => {
                let mut col = HcyColor::from_color(managed);
                col.h = amt;
                Color::Hcy(col)
            }
            // sliders.py:271-274
            SliderKind::HcyC => {
                let mut col = HcyColor::from_color(managed);
                col.c = amt;
                Color::Hcy(col)
            }
            // sliders.py:294-297
            SliderKind::HcyY => {
                let mut col = HcyColor::from_color(managed);
                col.y = amt;
                Color::Hcy(col)
            }
        }
    }

    /// `get_bar_amount_for_color(col)` —— 九个滑条各自的实现
    /// （`sliders.py:176-177` / `:193-194` / `:210-211` / `:223-224` /
    /// `:238-239` / `:250-251` / `:263-265` / `:276-278` / `:299-301`）。
    ///
    /// RGB 读 `col.r/g/b`（= `get_rgb()` 的分量），HSV 读 `col.h/s/v`
    /// （= `get_hsv()` 的分量，注意 `HCYColor.get_hsv()` 有纯黑特例），
    /// HCY 三个都先 `HCYColor(color=col)` 再读 `h/c/y`。
    #[must_use]
    pub fn get_bar_amount_for_color(&self, color: &Color) -> f64 {
        match self.kind {
            // sliders.py:176-177
            SliderKind::RgbR => color.get_rgb()[0],
            // sliders.py:193-194
            SliderKind::RgbG => color.get_rgb()[1],
            // sliders.py:210-211
            SliderKind::RgbB => color.get_rgb()[2],
            // sliders.py:223-224
            SliderKind::HsvH => color.get_hsv()[0],
            // sliders.py:238-239
            SliderKind::HsvS => color.get_hsv()[1],
            // sliders.py:250-251
            SliderKind::HsvV => color.get_hsv()[2],
            // sliders.py:263-265
            SliderKind::HcyH => HcyColor::from_color(color).h,
            // sliders.py:276-278
            SliderKind::HcyC => HcyColor::from_color(color).c,
            // sliders.py:299-301
            SliderKind::HcyY => HcyColor::from_color(color).y,
        }
    }

    /// `get_color_at_position(x, y)`（`adjbases.py:1019-1022`）：
    /// 先 [`point_to_amount`]，再 `get_color_for_bar_amount()`。
    #[must_use]
    pub fn color_at_position(
        &self,
        managed: &Color,
        pos: f64,
        length: f64,
        border_width: f64,
        vertical: bool,
    ) -> Color {
        let amt = point_to_amount(pos, length, border_width, vertical);
        self.get_color_for_bar_amount(managed, amt)
    }

    /// 滚轮一格 —— `SliderColorAdjuster.__scroll_cb()`（`adjbases.py:1069-1083`）。
    ///
    /// ```python
    /// d = self.SCROLL_DELTA
    /// if not self.vertical: d *= -1
    /// if event.direction in (DOWN, LEFT): d *= -1
    /// col = self.get_managed_color()
    /// amt = self.get_bar_amount_for_color(col)
    /// amt = clamp(amt + d, 0.0, 1.0)
    /// col = self.get_color_for_bar_amount(amt)
    /// self.set_managed_color(col)
    /// ```
    ///
    /// 返回要落进管理器的颜色；调用方交给 [`ColorTool::set_color`]，由管理器的
    /// 相等判断决定要不要真的换色（`adjbases.py:213-214`）。
    #[must_use]
    pub fn scroll(&self, managed: &Color, vertical: bool, direction: ScrollDirection) -> Color {
        let amt = self.get_bar_amount_for_color(managed);
        let amt = clamp(amt + scroll_delta(vertical, direction), 0.0, 1.0);
        self.get_color_for_bar_amount(managed, amt)
    }
}

impl ColorAdjuster for ComponentSlider {
    // 滑条没有需要缓存的东西（背景重绘令牌属 B4-3，见模块文档）。
}

// ---------------------------------------------------------------------------
// 拖动状态机（`adjbases.py:432-830` 的 `ColorAdjusterWidget` 数据侧）
// ---------------------------------------------------------------------------

/// 修饰键的两位 —— `Gdk.ModifierType.SHIFT_MASK` / `CONTROL_MASK`
/// （`adjbases.py:756-759`、`:740-743`）。
///
/// 用结构体而不是枚举：真实事件可能两个都按着，源码是 `if shift ... elif
/// control ...`（shift 优先）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// 按住 Shift。
    pub shift: bool,
    /// 按住 Ctrl。
    pub control: bool,
}

impl Modifiers {
    /// 什么都没按。
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        control: false,
    };
    /// 只按 Shift。
    pub const SHIFT: Modifiers = Modifiers {
        shift: true,
        control: false,
    };
    /// 只按 Ctrl。
    pub const CONTROL: Modifiers = Modifiers {
        shift: false,
        control: true,
    };
}

/// 按钮 2/3 拖动时弯曲哪个分量 —— `adjbases.py:755-763` 的 `bend`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BendAxis {
    /// Shift：彩度（`adjbases.py:756-758`）。
    Chroma,
    /// Ctrl：色相（`:759-760`）。
    Hue,
    /// 其余：亮度（`:761-763`）。
    Luma,
}

/// 选弯曲的轴 —— `adjbases.py:755-763`（顺序照抄：shift → control → 其余）。
#[must_use]
pub fn bend_axis(modifiers: Modifiers) -> BendAxis {
    if modifiers.shift {
        BendAxis::Chroma
    } else if modifiers.control {
        BendAxis::Hue
    } else {
        BendAxis::Luma
    }
}

/// 按钮 2/3 拖动时的 HCY 弯曲 —— `ColorAdjusterWidget.__motion_notify_cb()`
/// 里按钮 > 1 的那一段（`adjbases.py:739-788`）。
///
/// 逐行对照：
///
/// ```python
/// col = HCYColor(color=self.__drag_start_color)   # :747
/// w, h = allocation; size = max(w, h)             # :748-750
/// dx, dy = sx - ex, sy - ey                       # :753
/// if shift: bend = "chroma"; dy = -dy             # :756-758
/// elif control: bend = "hue"                      # :759-760
/// else: bend = "luma"; dy = -dy                   # :761-763
/// if RTL: dx = -dx                                # :766-767
/// dd = dx if abs(dx) > abs(dy) else dy            # :771
/// ...见下面三个分支...
/// self.set_managed_color(col)                     # :788
/// ```
///
/// 几个源码里就有的怪癖，照抄：
/// - 取"位移较大的那个轴"，相等时取 `dy`（`abs(dx) > abs(dy)` 是严格大于）；
/// - chroma 分支把 `c` 夹到 `0..1`，而 HCY 的 `c` 合法域是 `0..2`；
/// - 拖动结果**总是** `HCYColor`（`:747` 构造的就是它），所以中央色的类型
///   跟着变成 HCY；
/// - 色相分支 `while h < 0: h += 1.0` 之后还要 `% 1.0`（`:781-783`）。
///
/// `area` 是控件分配到的 `(width, height)`。
///
/// # Panics
///
/// `max(width, height) <= 0` 时 panic —— 源码在 `:776` / `:780` / `:787`
/// 除以 `size`，会抛 `ZeroDivisionError`；控件都有最小尺寸
/// （`adjbases.py:1334-1338` 等），真实调用走不到。
#[must_use]
pub fn bend_hcy(
    start_color: Color,
    start_pos: (f64, f64),
    pos: (f64, f64),
    area: (f64, f64),
    modifiers: Modifiers,
    rtl: bool,
) -> HcyColor {
    let mut col = HcyColor::from_color(&start_color);
    let size = area.0.max(area.1);
    assert!(
        size > 0.0,
        "控件尺寸 {area:?} 的较长边不是正数（源码此处会 ZeroDivisionError）"
    );
    let (sx, sy) = start_pos;
    let (ex, ey) = pos;
    let mut dx = sx - ex;
    let mut dy = sy - ey;

    let axis = bend_axis(modifiers);
    match axis {
        // :756-758
        BendAxis::Chroma => dy = -dy,
        // :759-760
        BendAxis::Hue => {}
        // :761-763
        BendAxis::Luma => dy = -dy,
    }

    // :766-767 —— 文本从右往左时的镜像
    if rtl {
        dx = -dx;
    }

    // :769-771 —— 用绝对值大的那个位移
    let dd = if dx.abs() > dy.abs() { dx } else { dy };

    match axis {
        // :773-776
        BendAxis::Chroma => {
            let c0 = clamp(col.c, 0.0, 1.0);
            let p = (c0 * size) - dd;
            col.c = clamp(p / size, 0.0, 1.0);
        }
        // :777-783
        BendAxis::Hue => {
            let h0 = clamp(col.h, 0.0, 1.0);
            let p = (h0 * size) - dd;
            let mut h = p / size;
            while h < 0.0 {
                h += 1.0;
            }
            col.h = h.rem_euclid(1.0);
        }
        // :784-787
        BendAxis::Luma => {
            let y0 = clamp(col.y, 0.0, 1.0);
            let p = (y0 * size) - dd;
            col.y = clamp(p / size, 0.0, 1.0);
        }
    }

    col
}

/// 按下按钮之后发生什么 —— `__button_press_cb()`（`adjbases.py:660-702`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressOutcome {
    /// 只记下起点，没改色：按钮 2/3 的弯曲起手（`:697-699`）、
    /// 或按钮 1 但位置取不到色（`:672-676` 里 `color is None` 那支）。
    Armed,
    /// 按钮 1 单击：位置色已设为当前色（`:671-672`）。
    ColorSet,
    /// 按钮 1 单击 + `IS_DRAG_SOURCE`：颜色已设，且可能起 DnD
    /// （`:673-676`；源码这里 `return True`）。
    DragPossible,
    /// 双击 + `HAS_DETAILS_DIALOG`：状态已清，请 GUI 开详情对话框
    /// （`:679-695`；确认后由调用方 `set_color_at_position()` 落色）。
    DetailsDialog,
}

/// 拖动过程中的更新 —— `__motion_notify_cb()`（`adjbases.py:704-791`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragUpdate {
    /// 没有按钮按着 / 没有弯曲起点 / 还没越过拖动阈值（`:701` 那条路）。
    None,
    /// 按钮 1 且不是 drag source：调用方按位置取色并设色（`:733-737`）。
    ///
    /// 位置 → 颜色的映射是控件自己的 `get_color_at_position()`
    /// （`:619-626`），滑条用 [`ComponentSlider::color_at_position`]，
    /// 色轮/方块属 B4-3，所以这里只给信号。
    SamplePosition,
    /// 按钮 1 且是 drag source：越过阈值，调用方起 DnD（`:708-732`）。
    BeginDrag,
    /// 按钮 > 1 的 HCY 弯曲结果，颜色**已经**落进工具（`:739-788`）。
    Bended(Color),
}

/// 拖动状态机 —— `ColorAdjusterWidget` 的指针事件部分
/// （`adjbases.py:509-808`），不含 GTK 装配与绘制。
///
/// 一个控件（滑条 / 色轮 / 方块）持一个本结构，把 egui 的
/// pointer-down / move / up 投影成 [`Self::start_drag`] /
/// [`Self::update`] / [`Self::stop_drag`]。
///
/// 三个类常量照抄成字段（源码里是类属性，允许子类覆盖）：
/// `IS_DRAG_SOURCE`（`:450`）、`ALLOW_HCY_TWEAKING`（`:461`）、
/// `HAS_DETAILS_DIALOG`（`:454`）。
pub struct ColorChanger {
    /// `__button_down`（`adjbases.py:512`）：当前按着的按钮号。
    button_down: Option<u8>,
    /// `__drag_start_pos`（`:513`）：按下时的位置。
    drag_start_pos: Option<(f64, f64)>,
    /// `__drag_start_color`（`:514`）：按下时取到的颜色 / 弯曲起点色。
    drag_start_color: Option<Color>,
    /// `_drag_threshold`（`:532`）：多少像素算拖动而不是点击。
    drag_threshold: f64,
    /// `IS_DRAG_SOURCE`（`:450`）：按钮 1 按下 + 拖动是否起 DnD。
    pub is_drag_source: bool,
    /// `ALLOW_HCY_TWEAKING`（`:461`）：按钮 2/3 是否做 HCY 弯曲。
    pub allow_hcy_tweaking: bool,
    /// `HAS_DETAILS_DIALOG`（`:454`）：双击是否开详情对话框。
    pub has_details_dialog: bool,
}

impl ColorChanger {
    /// 用给定的拖动阈值构造；`is_drag_source` / `has_details_dialog` 默认
    /// `false`、`allow_hcy_tweaking` 默认 `true`，同源码类常量的默认值
    /// （`adjbases.py:450-461`）。
    pub fn new(drag_threshold: f64) -> Self {
        Self {
            button_down: None,
            drag_start_pos: None,
            drag_start_color: None,
            drag_threshold,
            is_drag_source: false,
            allow_hcy_tweaking: true,
            has_details_dialog: false,
        }
    }

    /// 由 GTK 的 `gtk-dnd-drag-threshold` 算拖动阈值 ——
    /// `adjbases.py:528-532`。
    ///
    /// ```python
    /// settings = Gtk.Settings.get_default()
    /// thres = min(4, int(settings.get_property("gtk-dnd-drag-threshold")))
    /// self._drag_threshold = 2 * thres
    /// ```
    ///
    /// GTK 拿不到设置时给 0；`min()` 的上限是 4，所以阈值落在 `0..=8`。
    #[must_use]
    pub fn drag_threshold_from_gtk(gtk_dnd_drag_threshold: i32) -> f64 {
        let thres = gtk_dnd_drag_threshold.min(DRAG_THRESHOLD_CAP);
        f64::from(DRAG_THRESHOLD_FACTOR * thres)
    }

    /// 当前拖动阈值。
    pub fn drag_threshold(&self) -> f64 {
        self.drag_threshold
    }

    /// `__button_down`（测试与 GUI 用）。
    pub fn button_down(&self) -> Option<u8> {
        self.button_down
    }

    /// `__drag_start_pos`。
    pub fn drag_start_pos(&self) -> Option<(f64, f64)> {
        self.drag_start_pos
    }

    /// `__drag_start_color`。
    pub fn drag_start_color(&self) -> Option<Color> {
        self.drag_start_color
    }

    /// 按钮按下 —— `__button_press_cb()`（`adjbases.py:660-702`）。
    ///
    /// 状态照抄：先无条件记下 `button_down` 与 `drag_start_pos`（`:662-667`）；
    /// 然后分三条支路。`color_at_pos` 是控件自己的
    /// `get_color_at_position(x, y)` 的结果（`Option` 对应源码的 `None`）。
    ///
    /// `double_click` 对应 `event.type == Gdk.EventType._2BUTTON_PRESS`
    /// （`:671` / `:681`）。
    ///
    /// 源码里 `self.__initial_bg_validity = repr(...)`（`:664`）与
    /// `set_managed_color(color)` 的 `None` 忽略语义（`:405-410`）都在这里
    /// 体现：`None` 不动颜色。事件继续传播的 `return False`（`:702`）不是
    /// 模型语义，省掉。
    pub fn start_drag(
        &mut self,
        sink: &mut impl ColorSink,
        button: u8,
        double_click: bool,
        pos: (f64, f64),
        color_at_pos: Option<Color>,
    ) -> PressOutcome {
        self.button_down = Some(button);
        self.drag_start_pos = Some(pos);

        // :671-677 —— 按钮 1 单击：设色；drag source 还记下拖动起点
        if button == 1 && !double_click {
            let set = match color_at_pos {
                Some(color) => {
                    sink.sink_set_color(color);
                    true
                }
                None => false,
            };
            if self.is_drag_source {
                self.drag_start_color = color_at_pos;
                if color_at_pos.is_some() {
                    return PressOutcome::DragPossible;
                }
                return PressOutcome::Armed;
            }
            return if set {
                PressOutcome::ColorSet
            } else {
                PressOutcome::Armed
            };
        }

        // :679-695 —— 双击：清状态，请求 GUI 开详情对话框
        if button == 1 && double_click && self.has_details_dialog {
            self.button_down = None;
            self.drag_start_color = None;
            return PressOutcome::DetailsDialog;
        }

        // :697-699 —— 按钮 2/3：只记下弯曲起点色，不动当前色
        if button != 1 && self.allow_hcy_tweaking {
            self.drag_start_color = color_at_pos;
        }

        PressOutcome::Armed
    }

    /// 指针移动 —— `__motion_notify_cb()`（`adjbases.py:704-791`）。
    ///
    /// 三条支路照抄：
    /// 1. 按钮 1 + drag source：超过阈值就起 DnD（`:708-732`）；还没超过就
    ///    什么都不做；
    /// 2. 按钮 1 + 非 drag source：连续按位置取色（`:733-737`）→
    ///    [`DragUpdate::SamplePosition`]；
    /// 3. 按钮 > 1：HCY 弯曲（`:739-788`），直接写进工具 →
    ///    [`DragUpdate::Bended`]。
    ///
    /// `area` 是控件的 `(width, height)`（弯曲要用 `max(w, h)`）。
    pub fn update(
        &mut self,
        sink: &mut impl ColorSink,
        pos: (f64, f64),
        area: (f64, f64),
        modifiers: Modifiers,
        rtl: bool,
    ) -> DragUpdate {
        // :707-708
        if self.button_down == Some(1) {
            let Some(start_pos) = self.drag_start_pos else {
                return DragUpdate::None;
            };
            if self.is_drag_source {
                // :709-710 —— 拖动起点色已被清掉 ⇒ DnD 已经开始
                if self.drag_start_color.is_none() {
                    return DragUpdate::None;
                }
                // :711-714
                let dist = (pos.0 - start_pos.0).hypot(pos.1 - start_pos.1);
                // :715-732
                if dist > self.drag_threshold {
                    self.drag_start_color = None;
                    return DragUpdate::BeginDrag;
                }
                return DragUpdate::None;
            }
            // :733-737 —— 非 drag source：连续设色
            return DragUpdate::SamplePosition;
        }

        // :740-744 —— 按钮 > 1 的 HCY 弯曲
        if self.allow_hcy_tweaking && self.button_down.is_some_and(|b| b > 1) {
            let Some(start_color) = self.drag_start_color else {
                return DragUpdate::None;
            };
            let Some(start_pos) = self.drag_start_pos else {
                return DragUpdate::None;
            };
            let col = bend_hcy(start_color, start_pos, pos, area, modifiers, rtl);
            let color = Color::Hcy(col);
            sink.sink_set_color(color);
            return DragUpdate::Bended(color);
        }

        // :790-791
        DragUpdate::None
    }

    /// 按钮抬起 —— `__button_release_cb()`（`adjbases.py:793-808`）。
    ///
    /// 只有按钮号与 `button_down` 相同时才处理（`:795`），此时四个状态字段
    /// 全部清空（`:796-801`）。若抬起点离按下点小于拖动阈值（`:806`），
    /// 这就是一次"点击"，返回 `Some(按下点)` —— 源码把它交给
    /// `clicked(event, (xa, ya))` 事件（`:807`、`:810-811`），用的是**按下**
    /// 的位置而不是抬起位置。
    ///
    /// 源码里"非立即模式下重画背景"（`:802-804`）属控件层。
    pub fn stop_drag(&mut self, button: u8, pos: (f64, f64)) -> Option<(f64, f64)> {
        // :795
        if self.button_down != Some(button) {
            return None;
        }
        // :796-801
        let start_pos = self.drag_start_pos;
        self.button_down = None;
        self.drag_start_pos = None;
        self.drag_start_color = None;
        // :806-807
        let start_pos = start_pos?;
        let dist = (pos.0 - start_pos.0).hypot(pos.1 - start_pos.1);
        if dist < self.drag_threshold {
            Some(start_pos)
        } else {
            None
        }
    }
}
