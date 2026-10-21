//! 色轮 / 色方块 / 色立方的**纯数学**（B4-4b）——
//! `gui/colors/adjbases.py` 的 `HueSaturationWheelMixin`、色相畸变表，
//! 以及 `hsvwheel.py` / `hsvsquare.py` / `hsvcube.py` 里不碰 GTK 的部分。
//!
//! ## 源码里这块怎么分层
//!
//! | 源码 | 职责 | 这里 |
//! |---|---|---|
//! | `adjbases.py:1105-1114` | `HUE_SLICES` / `SAT_SLICES` / `SAT_GAMMA` | [`HUE_SLICES`] / [`SAT_SLICES`] / [`SAT_GAMMA`] |
//! | `adjbases.py:1116-1136` | `get_radius()` / `get_center()` | [`wheel_radius`] / [`wheel_center`] / [`WheelGeometry`] |
//! | `adjbases.py:1148-1167` | 位置 → (半径比, 色相角) | [`WheelGeometry::polar_at`] / [`saturation_from_radius`] / [`hue_from_angle`] |
//! | `adjbases.py:1301-1315` | (半径比, 色相角) → 像素位置 | [`WheelGeometry::position_for`] |
//! | `adjbases.py:1287-1299` | 极坐标 → 颜色（两个抽象钩子） | [`hsv_wheel_color_at_polar`] / [`hsv_ring_color_at_polar`] |
//! | `adjbases.py:80-95` | `_HUE_DISTORTION_TABLES`（rgb / ryb / rygb） | [`WheelType`] / [`RYB_HUE_RANGES`] / [`RYGB_HUE_RANGES`] |
//! | `adjbases.py:289-345` | `distort_hue()` / `undistort_hue()` | [`distort_hue`] / [`undistort_hue`]（+ [`super::ColorManager`] 的方法） |
//! | `adjbases.py:1233-1240` + `util.py:27-40` | 中心去饱和的径向渐变色标 | [`distance_fade_stops`] |
//! | `adjbases.py:1248-1283` | 盘边的纯色小刻痕（用 `get_pos_for_color`） | [`hsv_wheel_position_for_color`] |
//! | `hsvwheel.py:49-57` | HSV 轮：半径 → 饱和度、角度 → 色相 | [`hsv_wheel_polar_for_color`] / [`hsv_wheel_color_at_polar`] |
//! | `hsvwheel.py:35-47` | 轮上的滚轮改明度（±`SCROLL_DELTA`） | [`hsv_wheel_scroll`] |
//! | `hsvsquare.py:148-192` | 外环：位置 → 色相（半径被忽略） | [`hsv_ring_color_at_position`] / [`hsv_ring_color_at_polar`] |
//! | `hsvsquare.py:382-412` | 内方块：x → 面1、y → 面2（y 反向） | [`slice_color_at_position`] / [`slice_position_for_color`] |
//! | `hsvsquare.py:324-329` / `hsvcube.py:155-160` | `__get_faces()`（深度面是色相时换到 x） | [`slice_faces`] / [`HsvFace`] |
//! | `hsvcube.py:100-105` | `tumble()`：面顺序左旋一格 | [`tumble_faces`] |
//! | `hsvcube.py:118-142` | 深度滑条：面0 的量 ↔ 颜色 | [`depth_slider_amount`] / [`depth_slider_color`] / [`depth_slider_scroll`] |
//! | `uimisc.py:21-26` | 控件最小尺寸 | [`PRIMARY_ADJUSTERS_MIN_WIDTH`] / [`SLIDER_MIN_WIDTH`] 等 |
//!
//! ## `HueSaturationWheelMixin` 是怎么"可复用"的
//!
//! 源码把"极坐标几何 + 背景绘制"放进 mixin，把"极坐标 ↔ 颜色"留给子类
//! （`adjbases.py:1287-1299` 的两个 `raise NotImplementedError`）。本模块照这个
//! 分法拆成两层：
//!
//! - **几何层**：[`WheelGeometry`] 只看控件尺寸与边框，不知道颜色空间 ——
//!   `wheel_radius` / `wheel_center` / `polar_at` / `position_for` 全部与
//!   `lib/color.py` 无关。`SAT_GAMMA` 作为参数传入（源码里是类常量，
//!   `adjbases.py:1114`；HCY 轮没有覆盖它，见 `hcywheel.py` 全文）。
//! - **颜色层**：每个色轮给它自己的两个钩子 —— HSV 轮是
//!   [`hsv_wheel_color_at_polar`]（`hsvwheel.py:53-57`），HSV 方块的外环是
//!   [`hsv_ring_color_at_polar`]（`hsvsquare.py:189-192`）。B4-4c 的 HCY 轮
//!   只要再写这两个函数，几何层原样复用。
//!
//! 色相畸变（RYB/RYGB 轮）只作用在**角度**上：取色时
//! `undistort_hue(theta)`（`adjbases.py:1164-1166`），画标记时
//! `distort_hue(ntheta)`（`:1303-1305`）。畸变表由 [`super::ColorManager`]
//! 持有（源码 `_hue_distorts`，`:118` / `:289-345`）。
//!
//! ## 与源码不一致的两处（都是"没有 GTK"导致的，不是自创数学）
//!
//! 1. **除零**：源码在控件没有分配尺寸时会 `ZeroDivisionError`
//!    （`adjbases.py:1157` 的 `r /= radius`、`hsvsquare.py:389` 的
//!    `(x - b) / eff_wd`）。控件都有最小尺寸（`uimisc.py:21-26`，
//!    `adjbases.py:1334-1338`），真实路径走不到。Rust 侧把这些入口改成
//!    `Option`，返回 `None` 表示"这次分配太小，别算也别画" —— 见
//!    [`WheelGeometry::is_usable`] / [`slice_color_at_position`]。
//! 2. **`int()` 的方向**：源码的 `int(...)`（`adjbases.py:1313-1314`、
//!    `:1125`）是**向零截断**，不是向下取整；`//`（`:1134-1135`）才是向下。
//!    这里逐处照抄（`.trunc()` 对 `.floor()`），负坐标下的差别是可见的。
//!
//! ## 门槛
//!
//! `crates/pigment-engine/tests/wheel_vectors.rs` 拿
//! `tests/fixtures/wheel_vectors.txt` 逐位比对（`assert_eq!(to_bits())`）。
//! 测试数：4381 条向量。
//! fixture 由 `tests/tools/gen-wheel-vectors.py` 用 `ast` 从**真源码**抽出
//! 函数体后 `exec` 生成：抽的是 `HueSaturationWheelMixin.get_radius/get_center/
//! get_color_at_position/get_pos_for_color`、`ColorManager.distort_hue/
//! undistort_hue`、`hsvwheel.py` / `hsvsquare.py` / `hsvcube.py` 的钩子与
//! `get_color_at_position` / `get_position_for_color`、`HSVCubeSlider` 的两个
//! 映射、`gui/colors/util.py` 的 `add_distance_fade_stops`／`clamp`
//! （后者是**真 import**：`util.py` 只依赖 `lib.pycompat`）。
//!
//! ## 没移植的（记账，见交付说明）
//!
//! - `adjbases.py:1169-1285` 的**绘制**：本模块只给绘制用的采样数据
//!   （切片色相、渐变色标），Cairo 的扇区/径向渐变在
//!   `crates/pigment-app/src/shell/wheel_pages.rs` 用 egui mesh 表达。
//! - `adjbases.py:1138-1146` `get_background_validity()`：缓存重绘令牌；
//!   egui 立即模式每帧重画，没有对应物。
//! - `get_prefs()`（`adjbases.py:414-419`）：偏好系统属 B6；本模块只留
//!   [`PREFS_KEY_WHEEL_TYPE`] 键名。
//! - HCY 轮的颜色层（`hcywheel.py` 的 `color_at_normalized_polar_pos` 等）：
//!   B4-4c，几何层已可复用。

use std::f64::consts::PI;

use super::model::{Color, HsvColor, UIColor};
use super::tools::{clamp, scroll_delta, ScrollDirection, SCROLL_DELTA};

// ---------------------------------------------------------------------------
// 常量（逐条对应源码行）
// ---------------------------------------------------------------------------

/// 色轮画多少个扇区 —— `adjbases.py:1108` `HUE_SLICES = 64`。
///
/// 只影响绘制（`:1204` 的 `for ih in xrange(steps + 1)`），不影响取色：
/// 取色是连续的 `atan2`。
pub const HUE_SLICES: usize = 64;

/// 中心去饱和渐变的"灰阶分段数" —— `adjbases.py:1111` `SAT_SLICES = 5`。
///
/// 交给 [`distance_fade_stops`]：源码 `:1235-1237` 原样把它作为 `nstops`
/// 传进去，函数内部再 `+2`（`util.py:36`）。
pub const SAT_SLICES: usize = 5;

/// 灰阶/饱和度的 gamma —— `adjbases.py:1114` `SAT_GAMMA = 1.50`。
///
/// 两个方向都要用，别只抄一半：
/// - 取色（位置 → 颜色）：`r **= SAT_GAMMA`（`:1158`），把半径压得更"靠边"，
///   于是中心那片难点的低饱和区在视觉上被放大；
/// - 画标记（颜色 → 位置）：`nr **= 1.0 / SAT_GAMMA`（`:1306`），是上一步的逆。
pub const SAT_GAMMA: f64 = 1.50;

/// 主调整器（色轮/方块/立方切片）的最小宽度 —— `uimisc.py:21`
/// `PRIMARY_ADJUSTERS_MIN_WIDTH = 100`。
pub const PRIMARY_ADJUSTERS_MIN_WIDTH: f64 = 100.0;

/// 主调整器的最小高度 —— `uimisc.py:22`
/// `PRIMARY_ADJUSTERS_MIN_HEIGHT = 100`。
pub const PRIMARY_ADJUSTERS_MIN_HEIGHT: f64 = 100.0;

/// 滑条的最小宽度 —— `uimisc.py:25` `SLIDER_MIN_WIDTH = 18`。
pub const SLIDER_MIN_WIDTH: f64 = 18.0;

/// 滑条的最小长度 —— `uimisc.py:26` `SLIDER_MIN_LENGTH = 50`。
pub const SLIDER_MIN_LENGTH: f64 = 50.0;

/// Gtk.Table 里每个单元格四周的 padding —— `hsvwheel.py:76-77` /
/// `hsvcube.py:69-81` 的 `attach(..., 3, 3)` 最后两个参数。
///
/// 相邻两格各留 3px，所以格与格之间实际是 6px。
pub const TABLE_PADDING: f64 = 3.0;

/// 控件的外框线宽 —— `adjbases.py:457` `OUTLINE_WIDTH = 3`。
///
/// 色轮/方块/立方共用的 Tango 风格描边，与滑条/底栏指示器同一组类常量。
pub const OUTLINE_WIDTH: f64 = 3.0;

/// 控件外框颜色 —— `adjbases.py:458` `OUTLINE_RGBA = (0, 0, 0, 0.4)`。
pub const OUTLINE_RGBA: [f64; 4] = [0.0, 0.0, 0.0, 0.4];

/// 内高光宽度 —— `adjbases.py:459` `EDGE_HIGHLIGHT_WIDTH = 1.0`。
pub const EDGE_HIGHLIGHT_WIDTH: f64 = 1.0;

/// 内高光颜色 —— `adjbases.py:460` `EDGE_HIGHLIGHT_RGBA = (1, 1, 1, 0.25)`。
pub const EDGE_HIGHLIGHT_RGBA: [f64; 4] = [1.0, 1.0, 1.0, 0.25];

/// HSV 方块外环的"额外采样数" —— `hsvsquare.py:142` `samples = 4`。
///
/// 这个属性在 `HueSaturationWheelAdjuster` 上没有消费者（mixin 的绘制按
/// `HUE_SLICES` 走）；它来自 `ColorAdjusterWidget` 的通用采样约定
/// （`adjbases.py:947` `samples = 0`），外环只是照抄了一个值。这里保留常量
/// 以对齐源码，绘制不用它。
pub const RING_SAMPLES: usize = 4;

/// HSV 立方深度滑条的"额外采样数" —— `hsvcube.py:122` `samples = 4`。
///
/// 有真实消费者：`SliderColorAdjuster.render_background_cb()`
/// （`adjbases.py:983-984`）用它决定渐变色标数
/// （`samples + 2` 段 → `samples + 3` 个色标，见
/// [`super::ComponentSlider`] 的绘制路径）。
pub const CUBE_SLIDER_SAMPLES: usize = 4;

/// 偏好键：色轮类型 —— `adjbases.py:47` `PREFS_KEY_WHEEL_TYPE`。
///
/// 键名照抄；偏好系统属 B6（见模块文档）。
pub const PREFS_KEY_WHEEL_TYPE: &str = "colors.wheels.type";

/// 默认色轮类型 —— `adjbases.py:95` `_DEFAULT_WHEEL_TYPE = "rgb"`。
pub const DEFAULT_WHEEL_TYPE: &str = "rgb";

// ---------------------------------------------------------------------------
// 色相畸变表（`adjbases.py:80-95` + `:289-345`）
// ---------------------------------------------------------------------------

/// 一段色相重映射：`(RGB 轮上的输入区间, 畸变轮上的输出区间)`。
///
/// 源码里就是 `((in0, in1), (out0, out1))` 的二元组
/// （`adjbases.py:83-93`）。
pub type HueRange = ((f64, f64), (f64, f64));

/// RYB 轮（红-黄-蓝）的三段 —— `adjbases.py:83-87`。
///
/// 注意它**只覆盖到 2/3**：`2/3` 之后原样返回（蓝 → 红那段被压缩掉，
/// 于是红黄蓝在轮上等距）。
pub static RYB_HUE_RANGES: [HueRange; 3] = [
    ((0.0, 1.0 / 6.0), (0.0, 1.0 / 3.0)),       // 红 → 黄
    ((1.0 / 6.0, 1.0 / 3.0), (1.0 / 3.0, 0.5)), // 黄 → 绿
    ((1.0 / 3.0, 2.0 / 3.0), (0.5, 2.0 / 3.0)), // 绿 → 蓝
];

/// RYGB 轮（红-黄-绿-蓝）的四段 —— `adjbases.py:88-93`。
///
/// 每段映射到 1/4，四个原色在轮上等距。
pub static RYGB_HUE_RANGES: [HueRange; 4] = [
    ((0.0, 1.0 / 6.0), (0.0, 0.25)),       // 红 → 黄
    ((1.0 / 6.0, 1.0 / 3.0), (0.25, 0.5)), // 黄 → 绿
    ((1.0 / 3.0, 2.0 / 3.0), (0.5, 0.75)), // 绿 → 蓝
    ((2.0 / 3.0, 1.0), (0.75, 1.0)),       // 蓝 → 红
];

/// 色轮类型 —— `adjbases.py:80-95` 的 `_HUE_DISTORTION_TABLES` 的三个键。
///
/// `Rgb` 的 "表" 是 `None`（`:82`），因此 [`distort_hue`] 原样返回。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelType {
    /// `"rgb"`（默认）：不做重映射。
    Rgb,
    /// `"ryb"`：红-黄-蓝三段。
    Ryb,
    /// `"rygb"`：红-黄-绿-蓝四段。
    Rygb,
}

impl WheelType {
    /// 默认值 —— `adjbases.py:95` `_DEFAULT_WHEEL_TYPE = "rgb"`。
    pub const DEFAULT: WheelType = WheelType::Rgb;

    /// 三个类型，顺序同源码字典的键顺序（`adjbases.py:82-93`）。
    pub const ALL: [WheelType; 3] = [WheelType::Rgb, WheelType::Ryb, WheelType::Rygb];

    /// 源码里的类型名（`set_wheel_type()` 的参数，`adjbases.py:291-310`）。
    pub fn name(self) -> &'static str {
        match self {
            WheelType::Rgb => "rgb",
            WheelType::Ryb => "ryb",
            WheelType::Rygb => "rygb",
        }
    }

    /// 由类型名反查；未知名字给 `None`
    /// （`set_wheel_type()` 里未知名字退回默认值，`adjbases.py:302-303`）。
    pub fn from_name(name: &str) -> Option<WheelType> {
        WheelType::ALL.into_iter().find(|t| t.name() == name)
    }

    /// 这个类型的重映射表 —— 对应 `_HUE_DISTORTION_TABLES[name]`
    /// （`adjbases.py:80-94`）。`Rgb` 是 `None`。
    pub fn ranges(self) -> Option<&'static [HueRange]> {
        match self {
            WheelType::Rgb => None,
            WheelType::Ryb => Some(&RYB_HUE_RANGES),
            WheelType::Rygb => Some(&RYGB_HUE_RANGES),
        }
    }
}

/// 色相畸变：RGB 轮角度 → 当前轮类型角度 —— `ColorManager.distort_hue()`
/// （`adjbases.py:317-330`）。
///
/// 逐行照抄：
///
/// ```python
/// if self._hue_distorts is None: return h
/// h %= 1.0
/// for rgb_wheel_range, distorted_wheel_range in self._hue_distorts:
///     in0, in1 = rgb_wheel_range
///     out0, out1 = distorted_wheel_range
///     if h > in0 and h <= in1:          # 左开右闭！
///         h -= in0
///         h *= (out1 - out0) / (in1 - in0)
///         h += out0
///         break
/// return h
/// ```
///
/// 三个容易看漏的点，都照抄：
/// - 区间是 **`h > in0` 且 `h <= in1`**，所以 `h=0` 不进第一段（RYB/RYGB 下
///   红色恰好不动，符合直觉）；区间之间不会重复命中（前一段含上端，后一段
///   不含下端）。
/// - 只命中**一段**就 `break`，没有表尾的兜底段 —— 落在表外的 `h` 原样返回
///   （所以 `ryb` 在 `h > 2/3` 时是恒等）。
/// - `h %= 1.0` 是 Python 的向下取模（负角度绕回正值），用
///   [`f64::rem_euclid`]。
pub fn distort_hue(h: f64, ranges: Option<&[HueRange]>) -> f64 {
    let Some(ranges) = ranges else {
        return h;
    };
    let mut h = h.rem_euclid(1.0);
    for &((in0, in1), (out0, out1)) in ranges {
        if h > in0 && h <= in1 {
            h -= in0;
            h *= (out1 - out0) / (in1 - in0);
            h += out0;
            break;
        }
    }
    h
}

/// 色相逆畸变：当前轮类型角度 → RGB 轮角度 —— `ColorManager.undistort_hue()`
/// （`adjbases.py:332-345`）。
///
/// 与 [`distort_hue`] **只差输入/输出区间对调**（`:337-339`）；源码把这段
/// 复制了一遍，本模块同样分成两个函数，方便逐条对照与分别门禁。
pub fn undistort_hue(h: f64, ranges: Option<&[HueRange]>) -> f64 {
    let Some(ranges) = ranges else {
        return h;
    };
    let mut h = h.rem_euclid(1.0);
    for &((out0, out1), (in0, in1)) in ranges {
        if h > in0 && h <= in1 {
            h -= in0;
            h *= (out1 - out0) / (in1 - in0);
            h += out0;
            break;
        }
    }
    h
}

// ---------------------------------------------------------------------------
// 极坐标几何（`adjbases.py:1116-1136` + `:1148-1167` + `:1301-1315`）
// ---------------------------------------------------------------------------

/// 色轮的半径 —— `HueSaturationWheelMixin.get_radius()`
/// （`adjbases.py:1116-1125`）：
///
/// ```python
/// return int((min(wd, ht) / 2.0)) - int(border) + 0.5
/// ```
///
/// `+0.5` 是为了让圆心落在像素中心（源码 docstring 原话 "suitable for a
/// pixel-edge-aligned centre"）；两个 `int()` 都是向零截断（见模块文档）。
///
/// 尺寸太小时结果为负 —— 源码靠最小尺寸（`adjbases.py:1334-1338` 的
/// 100×100）保证不会，egui 侧用 [`WheelGeometry::is_usable`] 兜底。
pub fn wheel_radius(width: f64, height: f64, border: f64) -> f64 {
    (width.min(height) / 2.0).trunc() - border.trunc() + 0.5
}

/// 色轮的圆心 —— `HueSaturationWheelMixin.get_center()`
/// （`adjbases.py:1127-1136`）：
///
/// ```python
/// cx = int(wd // 2)
/// cy = int(ht // 2)
/// ```
///
/// 这里是 `//`（**向下取整**），与 [`wheel_radius`] 的 `int()`（向零截断）不是
/// 同一个运算 —— 正尺寸下结果相同，负尺寸下不同。照抄。
pub fn wheel_center(width: f64, height: f64) -> (f64, f64) {
    ((width / 2.0).floor(), (height / 2.0).floor())
}

/// 归一化极坐标角度：屏幕位移 → 色相 —— `adjbases.py:1160-1163`：
///
/// ```python
/// theta = 1.25 - (math.atan2(x - cx, y - cy) / (2 * math.pi))
/// while theta <= 0:
///     theta += 1.0
/// theta %= 1.0
/// ```
///
/// **角度方向是这个移植里最容易被"顺手写对"的地方**，照抄要点：
/// - `atan2` 的两个参数是 `(dx, dy)`（先 x 后 y），不是常见的 `(dy, dx)`
///   —— 于是 0 角在**屏幕下方**（+y）；
/// - 再被 `1.25 -` 反转/平移，最终：右 (+x) = 0.0、下 (+y) = 0.25、
///   左 (-x) = 0.5、上 (-y) = 0.75，即**色相沿顺时针增长**；
/// - `while` 与 `%` 都留着：当前输入下 `theta` 恒在 0.75..=1.75，`while` 不会
///   执行，但源码写了，B4-4c 的 HCY 轮与未来改动都吃这两步。
pub fn hue_from_angle(dx: f64, dy: f64) -> f64 {
    let mut theta = 1.25 - (dx.atan2(dy) / (2.0 * PI));
    while theta <= 0.0 {
        theta += 1.0;
    }
    theta.rem_euclid(1.0)
}

/// 位置半径 → 归一化饱和度 —— `adjbases.py:1153-1158`：
///
/// ```python
/// r = math.sqrt((x - cx) ** 2 + (y - cy) ** 2)
/// if r > radius: r = radius
/// r /= radius
/// r **= self.SAT_GAMMA
/// ```
///
/// 先夹到半径（圆外点击 = 最饱和，而不是"取不到色"），再归一化，再上
/// gamma。`sat_gamma` 是参数（源码是类常量 `self.SAT_GAMMA`）。
///
/// 半径不为正时返回 `None` —— 源码在 `r /= radius` 处会
/// `ZeroDivisionError`（模块文档"与源码不一致的两处"）。
pub fn saturation_from_radius(r: f64, radius: f64, sat_gamma: f64) -> Option<f64> {
    if radius <= 0.0 {
        return None;
    }
    let r = if r > radius { radius } else { r };
    Some((r / radius).powf(sat_gamma))
}

/// 一帧的色轮几何：圆心 + 半径。
///
/// 源码没有这个结构体（两个方法各自 `get_allocation()` 现算），但同一帧里
/// 绘制、命中、标记都要用同一组数，egui 立即模式每帧只算一次更稳。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelGeometry {
    /// 圆心 x（`adjbases.py:1134` 的 `cx`）。
    pub center_x: f64,
    /// 圆心 y（`:1135` 的 `cy`）。
    pub center_y: f64,
    /// 半径（`adjbases.py:1125`）。
    pub radius: f64,
}

impl WheelGeometry {
    /// 由控件尺寸与边框宽度算几何 —— `get_radius()` + `get_center()`
    /// （`adjbases.py:1116-1136`）。
    pub fn for_size(width: f64, height: f64, border: f64) -> Self {
        let (center_x, center_y) = wheel_center(width, height);
        Self {
            center_x,
            center_y,
            radius: wheel_radius(width, height, border),
        }
    }

    /// 半径是否为正 —— "这次分配够画/够点吗"。
    ///
    /// 源码没有这个判断：控件最小尺寸（`uimisc.py:21-22` 的 100×100，
    /// 经 `adjbases.py:1334-1338` 的 `set_size_request` 生效）保证
    /// `min(w,h) >= 100`，于是 `radius >= 48.5`。egui 面板可以被压到任意小，
    /// 这里把这些路径显式挡掉（不 panic、不画 NaN）。
    pub fn is_usable(&self) -> bool {
        self.radius > 0.0
    }

    /// 像素位置 → `(归一化半径, 色相角)` —— `get_color_at_position()`
    /// 的前半段（`adjbases.py:1150-1163`）。
    ///
    /// 返回的 `r` 已经过 `SAT_GAMMA`（`:1158`），`theta` **还没有**做色相
    /// 逆畸变 —— 那一步要看管理器（`:1164-1166`），留给页面层，因为几何层
    /// 不该知道轮类型。
    ///
    /// 半径不为正时给 `None`（见 [`Self::is_usable`]）。
    pub fn polar_at(&self, x: f64, y: f64, sat_gamma: f64) -> Option<(f64, f64)> {
        if !self.is_usable() {
            return None;
        }
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        let r = (dx * dx + dy * dy).sqrt();
        let r = saturation_from_radius(r, self.radius, sat_gamma)?;
        Some((r, hue_from_angle(dx, dy)))
    }

    /// `(归一化半径, 色相角)` → 像素位置 —— `get_pos_for_color()`
    /// （`adjbases.py:1301-1315`）的后半段：
    ///
    /// ```python
    /// nr **= 1.0 / self.SAT_GAMMA
    /// r = radius * clamp(nr, 0, 1)
    /// t = clamp(ntheta, 0, 1) * 2 * math.pi
    /// x = int(cx + r * math.cos(t)) + 0.5
    /// y = int(cy + r * math.sin(t)) + 0.5
    /// ```
    ///
    /// 这里**没有**做色相畸变（源码在 `:1303-1305` 先畸变再进这个函数），
    /// 调用方负责；`int()` 是向零截断（模块文档）。
    ///
    /// 半径不为正时不会失败（没有除法）：`r` 会退化成 0 或负值，函数照算 ——
    /// 与 Python 同处一致（那里也不报错）。
    pub fn position_for(&self, nr: f64, ntheta: f64, sat_gamma: f64) -> (f64, f64) {
        let nr = nr.powf(1.0 / sat_gamma);
        let r = self.radius * clamp(nr, 0.0, 1.0);
        let t = clamp(ntheta, 0.0, 1.0) * 2.0 * PI;
        (
            (self.center_x + r * t.cos()).trunc() + 0.5,
            (self.center_y + r * t.sin()).trunc() + 0.5,
        )
    }
}

// ---------------------------------------------------------------------------
// 颜色层的两个钩子（`adjbases.py:1287-1299`）
// ---------------------------------------------------------------------------

/// HSV 轮：颜色 → `(饱和度, 色相)` —— `HSVHueSaturationWheel.
/// get_normalized_polar_pos_for_color()`（`hsvwheel.py:49-51`）。
///
/// 源码只有一行 `return col.s, col.h`（先把任意颜色转成 `HSVColor`）。
pub fn hsv_wheel_polar_for_color(color: &Color) -> (f64, f64) {
    let col = HsvColor::from_color(color);
    (col.s, col.h)
}

/// HSV 轮：`(饱和度, 色相)` → 颜色 —— `HSVHueSaturationWheel.
/// color_at_normalized_polar_pos()`（`hsvwheel.py:53-57`）。
///
/// 两条赋值：`col.h = theta`、`col.s = r`；`v` 原样保留。所以**半径就是
/// 饱和度**，而中心的灰是"明度为 v、饱和度为 0"的灰。
pub fn hsv_wheel_color_at_polar(managed: &Color, r: f64, theta: f64) -> HsvColor {
    let mut col = HsvColor::from_color(managed);
    col.h = theta;
    col.s = r;
    col
}

/// HSV 方块的外环：`(半径, 色相)` → 颜色 —— `_HSVSquareOuterRing.
/// color_at_normalized_polar_pos()`（`hsvsquare.py:189-192`）。
///
/// **只写色相**：半径（饱和度）被丢弃 —— 外环是纯色相环，点哪里都只改色相
/// （饱和度/明度由内方块管）。这就是为什么 `hsvsquare.py:185-187` 的
/// `get_normalized_polar_pos_for_color()` 仍返回 `(s, h)`：那个 `s` 只用于
/// 画标记（`get_pos_for_color()`，`:148-162`）。
pub fn hsv_ring_color_at_polar(managed: &Color, theta: f64) -> HsvColor {
    let mut col = HsvColor::from_color(managed);
    col.h = theta;
    col
}

/// 外环的"颜色 → `(饱和度, 色相)`" —— `hsvsquare.py:185-187`。
///
/// 与 [`hsv_wheel_polar_for_color`] 同一份代码（源码也是各写一遍）。
pub fn hsv_ring_polar_for_color(color: &Color) -> (f64, f64) {
    let col = HsvColor::from_color(color);
    (col.s, col.h)
}

/// 轮上取色：位置 → 颜色 —— `HueSaturationWheelMixin.get_color_at_position()`
/// （`adjbases.py:1148-1167`）套用 HSV 轮的两个钩子。
///
/// 顺序照抄：极坐标 → 逆畸变（`:1164-1166`）→ `color_at_normalized_polar_pos`。
pub fn hsv_wheel_color_at_position(
    geom: &WheelGeometry,
    x: f64,
    y: f64,
    managed: &Color,
    ranges: Option<&[HueRange]>,
) -> Option<HsvColor> {
    let (r, theta) = geom.polar_at(x, y, SAT_GAMMA)?;
    let theta = undistort_hue(theta, ranges);
    Some(hsv_wheel_color_at_polar(managed, r, theta))
}

/// 外环取色：位置 → 颜色 —— `_HSVSquareOuterRing.get_color_at_position()`
/// （`hsvsquare.py:164-183`）。
///
/// 与 mixin 的版本**逐行相同**（源码整段复制），差别只在最后调的是外环自己的
/// `color_at_normalized_polar_pos`（半径被忽略）。
pub fn hsv_ring_color_at_position(
    geom: &WheelGeometry,
    x: f64,
    y: f64,
    managed: &Color,
    ranges: Option<&[HueRange]>,
) -> Option<HsvColor> {
    let (_r, theta) = geom.polar_at(x, y, SAT_GAMMA)?;
    let theta = undistort_hue(theta, ranges);
    Some(hsv_ring_color_at_polar(managed, theta))
}

/// 标记位置：颜色 → 像素 —— `get_pos_for_color()`
/// （`adjbases.py:1301-1315`）套用 HSV 轮的钩子。
///
/// 注意源码先 `distort_hue(ntheta)`（`:1303-1305`）再交给 `:1306-1314`。
pub fn hsv_wheel_position_for_color(
    geom: &WheelGeometry,
    color: &Color,
    ranges: Option<&[HueRange]>,
) -> (f64, f64) {
    let (nr, ntheta) = hsv_wheel_polar_for_color(color);
    let ntheta = distort_hue(ntheta, ranges);
    geom.position_for(nr, ntheta, SAT_GAMMA)
}

/// 外环标记位置：颜色 → 像素 —— 外环自己的 `get_pos_for_color()`
/// （`hsvsquare.py:148-162`）。
pub fn hsv_ring_position_for_color(
    geom: &WheelGeometry,
    color: &Color,
    ranges: Option<&[HueRange]>,
) -> (f64, f64) {
    let (nr, ntheta) = hsv_ring_polar_for_color(color);
    let ntheta = distort_hue(ntheta, ranges);
    geom.position_for(nr, ntheta, SAT_GAMMA)
}

// ---------------------------------------------------------------------------
// HSV 方块 / 立方切片（`hsvsquare.py:313-416` + `hsvcube.py:145-247`）
// ---------------------------------------------------------------------------

/// 立方的一个面 —— `hsvcube.py:52` 的 `self._faces = ["h", "s", "v"]`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HsvFace {
    /// `"h"`：色相。
    H,
    /// `"s"`：饱和度。
    S,
    /// `"v"`：明度。
    V,
}

impl HsvFace {
    /// 源码里的单字母名（`hsvcube.py:44-49` 的 tooltip 表就是按它索引）。
    pub fn name(self) -> &'static str {
        match self {
            HsvFace::H => "h",
            HsvFace::S => "s",
            HsvFace::V => "v",
        }
    }

    /// 由单字母名反查。
    pub fn from_name(name: &str) -> Option<HsvFace> {
        match name {
            "h" => Some(HsvFace::H),
            "s" => Some(HsvFace::S),
            "v" => Some(HsvFace::V),
            _ => None,
        }
    }

    /// `getattr(col, face)` —— 源码用 `getattr`/`setattr` 按面名读写。
    pub fn get(self, color: &HsvColor) -> f64 {
        match self {
            HsvFace::H => color.h,
            HsvFace::S => color.s,
            HsvFace::V => color.v,
        }
    }

    /// `setattr(col, face, value)`。
    pub fn set(self, color: &mut HsvColor, value: f64) {
        match self {
            HsvFace::H => color.h = value,
            HsvFace::S => color.s = value,
            HsvFace::V => color.v = value,
        }
    }
}

/// 切片的两个可拖拽面 —— `__get_faces()`
/// （`hsvsquare.py:324-329` / `hsvcube.py:155-160`，两份相同的代码）：
///
/// ```python
/// f1 = self.__cube._faces[1]
/// f2 = self.__cube._faces[2]
/// if f2 == "h":
///     f1, f2 = f2, f1
/// return f1, f2
/// ```
///
/// **x 轴永远给色相**（当面2 是色相时交换）：色相横向、另一个面纵向。
/// 这样用户旋转立方时，切片的方向感保持稳定。
pub fn slice_faces(faces: [HsvFace; 3]) -> (HsvFace, HsvFace) {
    let (mut f1, mut f2) = (faces[1], faces[2]);
    if f2 == HsvFace::H {
        std::mem::swap(&mut f1, &mut f2);
    }
    (f1, f2)
}

/// 旋转立方：面顺序左移一格 —— `HSVCubePage.tumble()`
/// （`hsvcube.py:100-105`）：
///
/// ```python
/// f0 = self._faces.pop(0)
/// self._faces.append(f0)
/// ```
///
/// 初始 `[h, s, v]` → 一次 `[s, v, h]` → 两次 `[v, h, s]` → 三次回到原样。
pub fn tumble_faces(faces: &mut [HsvFace; 3]) {
    let f0 = faces[0];
    faces[0] = faces[1];
    faces[1] = faces[2];
    faces[2] = f0;
}

/// 切片取色：位置 → 颜色 —— `_HSVSquareInnerSquare.get_color_at_position()`
/// （`hsvsquare.py:382-396`）与 `HSVCubeSlice.get_color_at_position()`
/// （`hsvcube.py:213-227`）**共用的**同一段数学：
///
/// ```python
/// eff_wd = wd - 2 * b
/// eff_ht = ht - 2 * b
/// f1_amt = clamp((x - b) / eff_wd, 0, 1)
/// f2_amt = clamp((y - b) / eff_ht, 0, 1)
/// f2_amt = 1.0 - f2_amt                  # y 轴反向：顶部是 1
/// col = HSVColor(color=managed)
/// setattr(col, f1, f1_amt)
/// setattr(col, f2, f2_amt)
/// ```
///
/// `f1`/`f2` 由 [`slice_faces`] 从面表算。返回 `None` 表示有效尺寸不是正数
/// （源码此处会 `ZeroDivisionError`；控件有 100×100 最小尺寸，
/// `hsvcube.py:150-153`）。
pub fn slice_color_at_position(
    managed: &Color,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    border: f64,
    faces: [HsvFace; 3],
) -> Option<HsvColor> {
    let eff_wd = width - 2.0 * border;
    let eff_ht = height - 2.0 * border;
    if eff_wd <= 0.0 || eff_ht <= 0.0 {
        return None;
    }
    let (f1, f2) = slice_faces(faces);
    let f1_amt = clamp((x - border) / eff_wd, 0.0, 1.0);
    let f2_amt = clamp((y - border) / eff_ht, 0.0, 1.0);
    let f2_amt = 1.0 - f2_amt;
    let mut col = HsvColor::from_color(managed);
    f1.set(&mut col, f1_amt);
    f2.set(&mut col, f2_amt);
    Some(col)
}

/// 切片标记：颜色 → 像素 —— `get_position_for_color()`
/// （`hsvsquare.py:398-412` / `hsvcube.py:229-243`）：
///
/// ```python
/// f1_amt = getattr(col, f1)
/// f2_amt = 1.0 - getattr(col, f2)
/// x = b + f1_amt * eff_wd
/// y = b + f2_amt * eff_ht
/// ```
///
/// 不夹取（源码也不夹）：颜色分量超出 0..1 时标记会画到切片外，但外层有
/// 裁剪（`paint_foreground_cb` 的调用方是控件自身）。
pub fn slice_position_for_color(
    color: &Color,
    width: f64,
    height: f64,
    border: f64,
    faces: [HsvFace; 3],
) -> (f64, f64) {
    let col = HsvColor::from_color(color);
    let (f1, f2) = slice_faces(faces);
    let f1_amt = f1.get(&col);
    let f2_amt = 1.0 - f2.get(&col);
    let eff_wd = width - 2.0 * border;
    let eff_ht = height - 2.0 * border;
    (border + f1_amt * eff_wd, border + f2_amt * eff_ht)
}

// ---------------------------------------------------------------------------
// 立方深度滑条（`hsvcube.py:118-142`）
// ---------------------------------------------------------------------------

/// 深度滑条的条值 —— `HSVCubeSlider.get_bar_amount_for_color()`
/// （`hsvcube.py:139-142`）：`getattr(HSVColor(managed), faces[0])`。
///
/// 源码对任意 `UIColor` 直接 `getattr(col, f0)` —— 四个子类都有 `h/s/v`
/// 属性（`UIColor` 的只读属性，`lib/color.py:97-110`），所以等价于先转
/// `HSVColor` 再取。
pub fn depth_slider_amount(managed: &Color, face0: HsvFace) -> f64 {
    face0.get(&HsvColor::from_color(managed))
}

/// 深度滑条的条值 → 颜色 —— `HSVCubeSlider.get_color_for_bar_amount()`
/// （`hsvcube.py:133-137`）：
///
/// ```python
/// col = HSVColor(color=self.get_managed_color())
/// setattr(col, f0, amt)
/// ```
///
/// 条值不夹取（滑条自己的 `point_to_amount()`/滚轮那两处已经夹过，
/// `adjbases.py:1063`/`:1080`）。
pub fn depth_slider_color(managed: &Color, face0: HsvFace, amount: f64) -> Color {
    let mut col = HsvColor::from_color(managed);
    face0.set(&mut col, amount);
    Color::Hsv(col)
}

/// 深度滑条滚轮一格 —— `SliderColorAdjuster.__scroll_cb()`
/// （`adjbases.py:1069-1083`）套上 [`depth_slider_color`]：
///
/// ```python
/// amt = clamp(get_bar_amount_for_color(col) + d, 0.0, 1.0)
/// ```
pub fn depth_slider_scroll(
    managed: &Color,
    face0: HsvFace,
    vertical: bool,
    direction: ScrollDirection,
) -> Color {
    let amount = depth_slider_amount(managed, face0) + scroll_delta(vertical, direction);
    depth_slider_color(managed, face0, clamp(amount, 0.0, 1.0))
}

// ---------------------------------------------------------------------------
// HSV 轮的滚轮（`hsvwheel.py:35-47`）
// ---------------------------------------------------------------------------

/// 色轮上的滚轮：只改明度 —— `HSVHueSaturationWheel.__scroll_cb()`
/// （`hsvwheel.py:35-47`）：
///
/// ```python
/// d = self.SCROLL_DELTA
/// if event.direction in (DOWN, LEFT): d *= -1
/// col = HSVColor(color=self.get_managed_color())
/// v = clamp(col.v + d, 0.0, 1.0)
/// if col.v != v:
///     col.v = v
///     self.set_managed_color(col)
/// ```
///
/// 与滑条的 `__scroll_cb()`（`adjbases.py:1069-1083`）不同：色轮**没有**
/// "横条取反"那一步（它本来就是两维控件）；`DOWN`/`LEFT` 仍然取反。
///
/// 返回 `None` 表示明度没变（源码那个 `if` 不成立，不会 `set_managed_color`）。
pub fn hsv_wheel_scroll(managed: &Color, direction: ScrollDirection) -> Option<Color> {
    let mut d = SCROLL_DELTA;
    if matches!(direction, ScrollDirection::Down | ScrollDirection::Left) {
        d *= -1.0;
    }
    let mut col = HsvColor::from_color(managed);
    let v = clamp(col.v + d, 0.0, 1.0);
    if col.v != v {
        col.v = v;
        Some(Color::Hsv(col))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 绘制用的采样（`adjbases.py:1204-1240` + `hsvsquare.py:231-238`）
// ---------------------------------------------------------------------------

/// 一圈扇区各自对应的色相（含末尾多一格）—— `adjbases.py:1204-1207`：
///
/// ```python
/// for ih in xrange(steps + 1):        # 多一格：最后那格不再画实心
///     h = ih / steps
///     if mgr:
///         h = mgr.undistort_hue(h)
/// ```
///
/// 返回 `steps + 1` 个**未畸变**的色相。绘制层第 `ih` 格用的就是
/// `hues[ih]`（`ih == steps` 那格只用于渐变收尾）。
pub fn hue_slice_hues(steps: usize, ranges: Option<&[HueRange]>) -> Vec<f64> {
    (0..=steps)
        .map(|ih| undistort_hue(ih as f64 / steps as f64, ranges))
        .collect()
}

/// HSV 轮某个色相上的边缘色 —— `hsvwheel.py:53-57` 的钩子在 `r = 1.0` 处，
/// 再 `get_rgb()`（`adjbases.py:1208-1209`）。
///
/// 半径 1.0 = 满饱和，所以颜色是 `HSVColor(h, 1.0, managed.v)` —— 明度跟着
/// 当前色走，轮子的亮度会随明度整体变化（源码如此）。
pub fn hsv_wheel_slice_rgb(managed: &Color, theta: f64) -> [f64; 3] {
    hsv_wheel_color_at_polar(managed, 1.0, theta).get_rgb()
}

/// HSV 方块外环某个色相上的颜色 —— `hsvsquare.py:231-238`：
///
/// ```python
/// edge_col = self.color_at_normalized_polar_pos(1.0, h)
/// edge_col.s = 1.0
/// edge_col.v = 1.0
/// rgb = edge_col.get_rgb()
/// ```
///
/// **外环强制 `s = v = 1`**（纯色相环），与 HSV 轮（明度跟当前色）不同。
pub fn hsv_ring_slice_rgb(managed: &Color, theta: f64) -> [f64; 3] {
    let mut col = hsv_ring_color_at_polar(managed, theta);
    col.s = 1.0;
    col.v = 1.0;
    col.get_rgb()
}

/// HSV 轮中心那层去饱和渐变的颜色 —— `adjbases.py:1173`：
/// `ref_grey = self.color_at_normalized_polar_pos(0, 0)`。
///
/// 对 HSV 轮，`r = 0` 让 `s = 0`，于是颜色是 `(v, v, v)` 的灰；源码
/// `:1141-1146` 的 `get_background_validity()` 还断言过这一点
/// （`assert k == min(rgb)`）。
pub fn hsv_wheel_center_rgb(managed: &Color) -> [f64; 3] {
    hsv_wheel_color_at_polar(managed, 0.0, 0.0).get_rgb()
}

/// 径向渐变的色标 —— `gui/colors/util.py:27-40` 的
/// `add_distance_fade_stops()`：
///
/// ```python
/// nstops = int(nstops) + 2
/// for s in xrange(nstops + 1):
///     a = alpha * (((nstops - s) / nstops) ** gamma)
///     stop = s / nstops
///     gr.add_color_stop_rgba(stop, red, green, blue, a)
/// ```
///
/// 返回 `nstops + 3` 个 `(位置, rgba)`，位置从 0 到 1 均匀。源码 `nstops`
/// 参数是 `SAT_SLICES`（5，`adjbases.py:1236`）→ 内部 7 → 8 个色标；
/// `gamma = 1.0 / SAT_GAMMA`（`:1236`）。
///
/// 注意 `+2` 在函数**内部**做（`util.py:36`）—— 这是源码的怪癖，照抄。
pub fn distance_fade_stops(
    rgb: [f64; 3],
    nstops: usize,
    gamma: f64,
    alpha: f64,
) -> Vec<(f64, [f64; 4])> {
    let nstops = nstops + 2;
    (0..=nstops)
        .map(|s| {
            let a = alpha * (((nstops - s) as f64 / nstops as f64).powf(gamma));
            let stop = s as f64 / nstops as f64;
            (stop, [rgb[0], rgb[1], rgb[2], a])
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 测试（期望值全部能从上方的源码行号手推）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::hsv_to_rgb;

    /// 常量 = 源码类常量（`adjbases.py:1105-1114`、`uimisc.py:21-26`）。
    #[test]
    fn constants_match_source_class_attributes() {
        assert_eq!(HUE_SLICES, 64, "adjbases.py:1108");
        assert_eq!(SAT_SLICES, 5, "adjbases.py:1111");
        assert_eq!(SAT_GAMMA, 1.50, "adjbases.py:1114");
        assert_eq!(PRIMARY_ADJUSTERS_MIN_WIDTH, 100.0, "uimisc.py:21");
        assert_eq!(PRIMARY_ADJUSTERS_MIN_HEIGHT, 100.0, "uimisc.py:22");
        assert_eq!(SLIDER_MIN_WIDTH, 18.0, "uimisc.py:25");
        assert_eq!(SLIDER_MIN_LENGTH, 50.0, "uimisc.py:26");
        assert_eq!(TABLE_PADDING, 3.0, "hsvwheel.py:76-77");
        assert_eq!(OUTLINE_WIDTH, 3.0, "adjbases.py:457");
        assert_eq!(OUTLINE_RGBA, [0.0, 0.0, 0.0, 0.4], "adjbases.py:458");
        assert_eq!(EDGE_HIGHLIGHT_WIDTH, 1.0, "adjbases.py:459");
        assert_eq!(
            EDGE_HIGHLIGHT_RGBA,
            [1.0, 1.0, 1.0, 0.25],
            "adjbases.py:460"
        );
        assert_eq!(RING_SAMPLES, 4, "hsvsquare.py:142");
        assert_eq!(CUBE_SLIDER_SAMPLES, 4, "hsvcube.py:122");
        assert_eq!(PREFS_KEY_WHEEL_TYPE, "colors.wheels.type", "adjbases.py:47");
        assert_eq!(DEFAULT_WHEEL_TYPE, "rgb", "adjbases.py:95");
    }

    /// 畸变表的内容与形状 —— `adjbases.py:80-94`。
    #[test]
    fn hue_distortion_tables_match_source() {
        assert_eq!(WheelType::DEFAULT, WheelType::Rgb);
        assert_eq!(WheelType::Rgb.ranges(), None, "rgb 的表是 None（:82）");
        let ryb = WheelType::Ryb.ranges().unwrap();
        assert_eq!(ryb.len(), 3);
        assert_eq!(ryb[0], ((0.0, 1.0 / 6.0), (0.0, 1.0 / 3.0)));
        assert_eq!(ryb[2], ((1.0 / 3.0, 2.0 / 3.0), (0.5, 2.0 / 3.0)));
        let rygb = WheelType::Rygb.ranges().unwrap();
        assert_eq!(rygb.len(), 4);
        assert_eq!(rygb[3], ((2.0 / 3.0, 1.0), (0.75, 1.0)));
        // 名字与反查
        assert_eq!(WheelType::Rygb.name(), "rygb");
        assert_eq!(WheelType::from_name("ryb"), Some(WheelType::Ryb));
        assert_eq!(WheelType::from_name("cmy"), None);
    }

    /// `distort_hue()` / `undistort_hue()` 的分段语义 —— `adjbases.py:317-345`。
    #[test]
    fn hue_distortion_maps_segments_and_is_invertible() {
        let rygb = WheelType::Rygb.ranges();
        // rgb 轮：恒等（表是 None）
        assert_eq!(distort_hue(0.3, None).to_bits(), 0.3f64.to_bits());
        // rygb：红→黄那段被拉长到 0..0.25
        assert_eq!(distort_hue(1.0 / 12.0, rygb).to_bits(), 0.125f64.to_bits());
        // 段边界：左开右闭（h=0 不进第一段，h=1/6 进第一段）
        assert_eq!(
            distort_hue(0.0, rygb),
            0.0,
            "h=0 不重映射（:325 的 h > in0）"
        );
        assert_eq!(distort_hue(1.0 / 6.0, rygb).to_bits(), 0.25f64.to_bits());
        // 负角度先绕回（:321 的 h %= 1.0）；-5/6 绕回后略小于 1/6，
        // 于是落进第一段、被拉成 ~0.25（浮点上是 0.24999999999999994）
        let wrapped = distort_hue(-5.0 / 6.0, rygb);
        assert!(
            (wrapped - 0.25).abs() < 1e-15,
            "-5/6 绕回后应映到 0.25 附近，实得 {wrapped}"
        );
        // 逐段往返（段内点在 undistort 后回到原值）
        for h in [0.02, 0.1, 1.0 / 6.0, 0.2, 0.4, 0.6, 0.8, 0.99] {
            let back = undistort_hue(distort_hue(h, rygb), rygb);
            assert!(
                (back - h).abs() < 1e-12,
                "h={h} 往返成了 {back}（adjbases.py:317-345）"
            );
        }
    }

    /// 半径与圆心 —— `adjbases.py:1125` / `:1134-1135`。
    #[test]
    fn wheel_radius_and_center_follow_source_formulas() {
        // int(min(100,100)/2) - int(2) + 0.5
        assert_eq!(wheel_radius(100.0, 100.0, 2.0), 48.5);
        // 非正方：min 起作用；99/2 = 49.5 → int 49
        assert_eq!(wheel_radius(200.0, 99.0, 2.0), 47.5);
        // 圆心用 //（向下取整）
        assert_eq!(wheel_center(100.0, 100.0), (50.0, 50.0));
        assert_eq!(wheel_center(101.0, 99.0), (50.0, 49.0));
        // 尺寸退化：半径变负，几何不可用
        let tiny = WheelGeometry::for_size(8.0, 8.0, 2.0);
        assert_eq!(tiny.radius, 2.5, "int(8/2) - 2 + 0.5");
        let tiny = WheelGeometry::for_size(3.0, 3.0, 2.0);
        assert!(!tiny.is_usable(), "半径 {0} 应为负", tiny.radius);
        assert_eq!(tiny.polar_at(1.0, 1.0, SAT_GAMMA), None);
    }

    /// 角度方向 —— `adjbases.py:1160-1163`：右 0、下 .25、左 .5、上 .75。
    #[test]
    fn hue_angle_direction_matches_source() {
        assert_eq!(hue_from_angle(1.0, 0.0), 0.0, "右 = 色相 0（红）");
        assert_eq!(hue_from_angle(0.0, 1.0), 0.25, "下 = 0.25");
        assert_eq!(hue_from_angle(-1.0, 0.0), 0.5, "左 = 0.5");
        assert_eq!(hue_from_angle(0.0, -1.0), 0.75, "上 = 0.75");
        // 色相沿顺时针增长
        assert!(hue_from_angle(0.7, 0.7) > 0.0);
        assert!(hue_from_angle(0.7, 0.7) < 0.25);
        assert!(hue_from_angle(-0.7, 0.7) > 0.25);
        assert!(hue_from_angle(-0.7, 0.7) < 0.5);
    }

    /// 半径映射 + `SAT_GAMMA` —— `adjbases.py:1153-1158` / `:1306`。
    #[test]
    fn radius_maps_to_saturation_with_gamma() {
        // 正中心 = 0；圆上 = 1；圆外夹到 1
        let r = WheelGeometry::for_size(100.0, 100.0, 2.0);
        assert_eq!(r.polar_at(50.0, 50.0, SAT_GAMMA).unwrap().0, 0.0);
        assert_eq!(r.polar_at(50.0 + 48.5, 50.0, SAT_GAMMA).unwrap().0, 1.0);
        assert_eq!(r.polar_at(50.0 + 999.0, 50.0, SAT_GAMMA).unwrap().0, 1.0);
        // 半径一半：0.5 ** 1.5
        let half = 0.5f64.powf(SAT_GAMMA);
        assert_eq!(r.polar_at(50.0 + 24.25, 50.0, SAT_GAMMA).unwrap().0, half);
        // 画标记是逆运算：nr ** (1/1.5)
        let (x, _y) = r.position_for(half, 0.0, SAT_GAMMA);
        assert_eq!(x, (50.0f64 + 48.5 * 0.5).trunc() + 0.5);
    }

    /// 位置 ↔ 极坐标往返（有 ±1px 的 int() 量化误差）—— `:1148-1167` + `:1301-1315`。
    #[test]
    fn position_and_polar_round_trip() {
        let g = WheelGeometry::for_size(200.0, 140.0, 2.0);
        for (x, y) in [
            (100.0, 70.0),
            (100.0, 118.0),
            (148.0, 70.0),
            (52.0, 70.0),
            (60.0, 40.0),
            (140.0, 100.0),
        ] {
            let (r, theta) = g.polar_at(x, y, SAT_GAMMA).unwrap();
            let (bx, by) = g.position_for(r, theta, SAT_GAMMA);
            assert!((bx - x).abs() <= 1.0, "x {x} → {bx}");
            assert!((by - y).abs() <= 1.0, "y {y} → {by}");
        }
    }

    /// HSV 轮的钩子：半径 = 饱和度、角度 = 色相，`v` 不变
    /// （`hsvwheel.py:49-57`）。
    #[test]
    fn hsv_wheel_hooks_set_saturation_and_hue() {
        let managed = Color::Hsv(HsvColor::new(0.4, 0.2, 0.7));
        let (r, h) = hsv_wheel_polar_for_color(&managed);
        assert_eq!((r, h), (0.2, 0.4));
        let col = hsv_wheel_color_at_polar(&managed, 0.8, 0.13);
        assert_eq!((col.s, col.h, col.v), (0.8, 0.13, 0.7));
        // 取色会带上逆畸变（默认 rgb 表是恒等）
        let g = WheelGeometry::for_size(100.0, 100.0, 2.0);
        let col = hsv_wheel_color_at_position(&g, 50.0, 50.0 + 48.5, &managed, None).unwrap();
        assert_eq!((col.s, col.h), (1.0, 0.25), "下 = 0.25、圆上 = 满饱和");
        // 标记位置：色相 0.25 → 正下方
        let (x, y) =
            hsv_wheel_position_for_color(&g, &Color::Hsv(HsvColor::new(0.25, 1.0, 0.7)), None);
        assert_eq!(x, 50.5);
        assert_eq!(y, (50.0f64 + 48.5).trunc() + 0.5);
    }

    /// 外环取色：中心与圆边的色相（`atan2(0,0)` 在 Rust/Python 都是 0）。
    ///
    /// `hue_from_angle(0, 0)` = `1.25 - 0/(2π)` = 1.25 → `% 1.0` = 0.25，
    /// 所以**正中心**的色相是 0.25（不是 0）—— 源码在 `:1160` 就是这么算的。
    #[test]
    fn hsv_ring_center_hue_is_quarter() {
        assert_eq!(hue_from_angle(0.0, 0.0), 0.25);
        let g = WheelGeometry::for_size(100.0, 100.0, 2.0);
        let managed = Color::Hsv(HsvColor::new(0.4, 0.3, 0.9));
        let center = hsv_ring_color_at_position(&g, 50.0, 50.0, &managed, None).unwrap();
        assert_eq!(center.h, 0.25);
        assert_eq!(center.s, 0.3, "外环不动饱和度");
        assert_eq!(center.v, 0.9);
    }

    /// 方块/切片坐标 ↔ HSV —— `hsvsquare.py:382-412`，y 轴反向。
    #[test]
    fn slice_coordinates_map_to_hsv_faces() {
        let faces = [HsvFace::H, HsvFace::S, HsvFace::V];
        let managed = Color::Hsv(HsvColor::new(0.4, 0.3, 0.9));
        // 左上角：s=0、v=1
        let c = slice_color_at_position(&managed, 2.0, 2.0, 100.0, 100.0, 2.0, faces).unwrap();
        assert_eq!((c.h, c.s, c.v), (0.4, 0.0, 1.0));
        // 右下角：s=1、v=0
        let c = slice_color_at_position(&managed, 98.0, 98.0, 100.0, 100.0, 2.0, faces).unwrap();
        assert_eq!((c.s, c.v), (1.0, 0.0));
        // 正中：s=0.5、v=0.5
        let c = slice_color_at_position(&managed, 50.0, 50.0, 100.0, 100.0, 2.0, faces).unwrap();
        assert_eq!((c.s, c.v), (0.5, 0.5));
        // 夹取：越界落到 0/1
        let c = slice_color_at_position(&managed, -50.0, 900.0, 100.0, 100.0, 2.0, faces).unwrap();
        assert_eq!((c.s, c.v), (0.0, 0.0));
        // 反向：颜色 → 位置
        let col = Color::Hsv(HsvColor::new(0.7, 0.75, 0.25));
        let (x, y) = slice_position_for_color(&col, 100.0, 100.0, 2.0, faces);
        assert_eq!(x, 2.0 + 0.75 * 96.0);
        assert_eq!(y, 2.0 + (1.0 - 0.25) * 96.0);
        // 有效尺寸退化 → None（源码会 ZeroDivisionError）
        assert_eq!(
            slice_color_at_position(&managed, 1.0, 1.0, 4.0, 10.0, 2.0, faces),
            None
        );
    }

    /// 面表与旋转 —— `hsvcube.py:100-105` + `:155-160`。
    #[test]
    fn faces_tumble_and_swap_hue_to_x() {
        // 初始 [h,s,v]：x=s、y=v
        let mut faces = [HsvFace::H, HsvFace::S, HsvFace::V];
        assert_eq!(slice_faces(faces), (HsvFace::S, HsvFace::V));
        // 一次：[s,v,h] → x 给色相（交换），y 给 v
        tumble_faces(&mut faces);
        assert_eq!(faces, [HsvFace::S, HsvFace::V, HsvFace::H]);
        assert_eq!(slice_faces(faces), (HsvFace::H, HsvFace::V));
        // 两次：[v,h,s] → x=h、y=s
        tumble_faces(&mut faces);
        assert_eq!(faces, [HsvFace::V, HsvFace::H, HsvFace::S]);
        assert_eq!(slice_faces(faces), (HsvFace::H, HsvFace::S));
        // 三次回到原点
        tumble_faces(&mut faces);
        assert_eq!(faces, [HsvFace::H, HsvFace::S, HsvFace::V]);
    }

    /// 深度滑条 —— `hsvcube.py:133-142` + `adjbases.py:1069-1083`。
    #[test]
    fn depth_slider_maps_amount_and_scrolls() {
        let managed = Color::Hsv(HsvColor::new(0.2, 0.4, 0.6));
        assert_eq!(depth_slider_amount(&managed, HsvFace::H), 0.2);
        assert_eq!(depth_slider_amount(&managed, HsvFace::V), 0.6);
        let col = depth_slider_color(&managed, HsvFace::S, 0.9);
        let Color::Hsv(hsv) = col else {
            panic!("深度滑条应回写 HSVColor");
        };
        assert_eq!((hsv.h, hsv.s, hsv.v), (0.2, 0.9, 0.6));
        // 竖直滑条向上滚 +SCROLL_DELTA，夹在 1
        let col = depth_slider_scroll(&managed, HsvFace::V, true, ScrollDirection::Up);
        let Color::Hsv(hsv) = col else { panic!() };
        assert_eq!(hsv.v, 0.6 + SCROLL_DELTA);
        let col = depth_slider_scroll(&managed, HsvFace::V, true, ScrollDirection::Down);
        let Color::Hsv(hsv) = col else { panic!() };
        assert_eq!(hsv.v, 0.6 - SCROLL_DELTA);
        let full = Color::Hsv(HsvColor::new(0.2, 0.4, 1.0));
        let Color::Hsv(hsv) = depth_slider_scroll(&full, HsvFace::V, true, ScrollDirection::Up)
        else {
            panic!()
        };
        assert_eq!(hsv.v, 1.0, "夹到 1");
    }

    /// 轮上滚轮只改明度 —— `hsvwheel.py:35-47`。
    #[test]
    fn wheel_scroll_changes_value_only() {
        let managed = Color::Hsv(HsvColor::new(0.3, 0.5, 0.5));
        let Some(Color::Hsv(col)) = hsv_wheel_scroll(&managed, ScrollDirection::Up) else {
            panic!("向上滚应改明度");
        };
        assert_eq!((col.h, col.s), (0.3, 0.5));
        assert_eq!(col.v, 0.5 + SCROLL_DELTA);
        // DOWN/LEFT 取反
        let Some(Color::Hsv(col)) = hsv_wheel_scroll(&managed, ScrollDirection::Down) else {
            panic!()
        };
        assert_eq!(col.v, 0.5 - SCROLL_DELTA);
        // 已经在 1：不变 → None（源码的 if 不成立）
        let full = Color::Hsv(HsvColor::new(0.3, 0.5, 1.0));
        assert_eq!(hsv_wheel_scroll(&full, ScrollDirection::Up), None);
    }

    /// 绘制采样：扇区色相、两种色轮的边缘色、中心灰 ——
    /// `adjbases.py:1204-1209` / `:1173`、`hsvsquare.py:231-238`。
    #[test]
    fn drawing_samples_match_source_hooks() {
        // 64 步 → 65 个色相，首尾是 0 和 1（rgb 表恒等）
        let hues = hue_slice_hues(HUE_SLICES, None);
        assert_eq!(hues.len(), HUE_SLICES + 1);
        assert_eq!(hues[0], 0.0);
        assert_eq!(hues[1], 1.0 / 64.0);
        assert_eq!(hues[HUE_SLICES], 1.0);
        // 畸变轮：色相被逆畸变回 RGB 轮的角度
        // （undistort 的区间对调后：0.25 → 1/6、0.5 → 1/3、0.75 → 2/3）
        let hues = hue_slice_hues(4, WheelType::Rygb.ranges());
        assert_eq!(hues, vec![0.0, 1.0 / 6.0, 1.0 / 3.0, 2.0 / 3.0, 0.0]);
        // HSV 轮边缘色 = HSV(h, 1, managed.v)
        let managed = Color::Hsv(HsvColor::new(0.4, 0.2, 0.6));
        assert_eq!(
            hsv_wheel_slice_rgb(&managed, 0.1),
            hsv_to_rgb([0.1, 1.0, 0.6])
        );
        // 外环边缘色 = HSV(h, 1, 1)
        let managed = Color::Hsv(HsvColor::new(0.4, 0.2, 0.6));
        assert_eq!(
            hsv_ring_slice_rgb(&managed, 0.1),
            hsv_to_rgb([0.1, 1.0, 1.0])
        );
        // 中心灰 = (v, v, v)
        assert_eq!(hsv_wheel_center_rgb(&managed), [0.6, 0.6, 0.6]);
    }

    /// 径向渐变色标 —— `util.py:27-40`（`nstops = 5` → 内部 7 → 8 个色标）。
    #[test]
    fn distance_fade_stops_follow_util_formula() {
        let stops = distance_fade_stops([0.5, 0.6, 0.7], SAT_SLICES, 1.0 / SAT_GAMMA, 1.0);
        assert_eq!(stops.len(), SAT_SLICES + 3, "nstops + 2 后再 +1");
        // 位置均匀 0..=1（s / nstops，nstops = 7）
        for (i, (stop, rgba)) in stops.iter().enumerate() {
            assert_eq!(*stop, i as f64 / 7.0);
            assert_eq!((rgba[0], rgba[1], rgba[2]), (0.5, 0.6, 0.7));
        }
        // 中心全不透明，边缘全透明
        assert_eq!(stops[0].1[3], 1.0);
        assert_eq!(stops[7].1[3], 0.0);
        // 中间：((7-3)/7) ** (1/1.5)
        assert_eq!(stops[3].1[3], (4.0f64 / 7.0).powf(1.0 / SAT_GAMMA));
    }

    /// 管理器侧的色轮类型与畸变（`adjbases.py:289-345`）：
    /// 方法只是 [`distort_hue`] / [`undistort_hue`] 加一个类型字段。
    #[test]
    fn manager_wheel_type_drives_distortion() {
        use crate::color::ColorManager;

        let mut manager = ColorManager::new();
        // 默认 rgb = 恒等（`adjbases.py:95` / `:134-136`）
        assert_eq!(manager.wheel_type(), WheelType::Rgb);
        assert_eq!(manager.wheel_type_name(), "rgb");
        assert_eq!(manager.distort_hue(0.3).to_bits(), 0.3f64.to_bits());
        assert_eq!(manager.undistort_hue(0.3).to_bits(), 0.3f64.to_bits());

        // 换到 rygb：`set_wheel_type()` 返回"是否真的变了"（`:304-305`）
        assert!(manager.set_wheel_type(WheelType::Rygb));
        assert!(!manager.set_wheel_type(WheelType::Rygb));
        assert_eq!(manager.wheel_type_name(), "rygb");
        assert_eq!(manager.hue_ranges(), WheelType::Rygb.ranges());
        // 红→黄那段：1/12 映到 0.125，逆映射回来
        assert_eq!(
            manager.distort_hue(1.0 / 12.0).to_bits(),
            0.125f64.to_bits()
        );
        assert_eq!(
            manager.undistort_hue(0.125).to_bits(),
            (1.0 / 12.0f64).to_bits()
        );

        // 未知名字退回默认（`adjbases.py:302-303`）
        assert!(manager.set_wheel_type_by_name("cmy"));
        assert_eq!(manager.wheel_type(), WheelType::Rgb);
        assert!(!manager.set_wheel_type_by_name("rgb"));
    }
}
