//! HCY 色轮与色域遮罩的**纯数学**（B4-4c）——
//! `gui/colors/hcywheel.py` 里不碰 GTK / cairo 的全部，加上它依赖的
//! `lib/alg.py` 多边形算法子集。
//!
//! ## 源码里这块怎么分层
//!
//! `hcywheel.py` 一共 1360 行，按职责是四层：
//!
//! | 层 | 源码 | 依赖 | 这里 |
//! |---|---|---|---|
//! | 几何/数学 | `HCYHueChromaWheelMixin`（`:320-336`）、`MaskableWheelMixin` 的 void 计算（`:180-244`）、mood 预设（`:868-988`） | `lib/color.py` / `lib/alg.py` | **本模块** |
//! | 绘制 | `MaskableWheelMixin.draw_mask`（`:260-312`）、`draw_mask_control_points`（`:738-804`）、`HCYMaskPreview`（`:825-857`）；盘面本体在 `adjbases.py:1169-1285` | cairo | `crates/pigment-app/src/shell/hcy_wheel_page.rs` |
//! | 事件 | `__button_press_cb` / `__motion_cb` / `__scroll_cb`（`:356-367`、`:587-629`） | GDK | 同上（复用 `wheel_pages.rs` 的 [`super::ColorChanger`]） |
//! | 遮罩编辑对话框 | `HCYMaskEditorWheel`（`:370-822`）、两个 `Gtk.Dialog`（`:860-1265`） | GTK 对话框 + 拖拽状态机 | **未移植，记账**（见模块末） |
//!
//! 盘面几何（圆心/半径/极坐标/`SAT_GAMMA`/色相畸变）在 B4-4b 的
//! [`super::wheels`] 里，**与色彩空间无关**，这里直接复用：HCY 轮只补两个
//! 颜色钩子（半径 = HCY 彩度、角度 = 色相），与 HSV 轮完全同构。
//!
//! ## 「mood 预设」与「多边形」到底是什么
//!
//! 任务里说的"中心 mood and accent 多边形"在源码里**不是**轮子的一部分：
//! HCY 轮就是一个圆盘（`adjbases.py:1169-1285`），"多边形"是**色域遮罩**
//! （gamut mask）：每个遮罩形状是若干个 `HCYColor` 点，位置由
//! `get_pos_for_color()`（`adjbases.py:1301-1315`）算出，形状本身取这些点的
//! **凸包**（`lib/alg.py:25-69` 的 `convex_hull`，见 [`convex_hull`]）。
//!
//! "Mood and Accent" 是五个**预设遮罩**之一（`HCYMaskTemplateDialog.__templates`，
//! `:868-988`），见 [`mask_templates`]。源码里也没有"HCY 三角/六边形"坐标
//! ↔ HCY 的换算：六边形只是 `__coffin()`（`:878-888`）这个"棺材形"预设的
//! 顶点，坐标↔HCY 走的就是圆盘的极坐标。
//!
//! ## 门槛
//!
//! `crates/pigment-engine/tests/wheel_vectors.rs` 拿
//! `tests/fixtures/wheel_vectors.txt` 里的 `hcy_*` 行逐位比对
//! （`assert_eq!(to_bits())`）；fixture 由
//! `tests/tools/gen-wheel-vectors.py` 用 `ast` 从 `hcywheel.py` / `sliders.py`
//! 抽出方法体后 `exec` 生成，`lib/alg.py` 与 `lib/color.py` 是**真 import**
//! （两者都只依赖标准库 / `math`）。本块 1708 条 `hcy_*` 向量。
//!
//! ## 与源码不一致的地方（都是"没有 GTK/cairo"导致的）
//!
//! 1. **除零**：源码在控件没有分配尺寸时 `ZeroDivisionError`
//!    （`adjbases.py:1157`）。本模块的几何入口沿用 B4-4b 的
//!    [`WheelGeometry::is_usable`] 兜底，返回 `None`。
//! 2. **`sqrt(负面积)`**：`_get_void_size()`（`hcywheel.py:240-244`）是
//!    `sqrt(poly_area(void))`，面积来自凸包（`:204`）。凸包输出恒为正向
//!    遍历，面积非负；万一为负，Python 抛 `ValueError`，这里得 `NaN` ——
//!    NaN 在 `size >= min_size` 比较中为假，于是这个形状不画，不会 panic。
//! 3. **`_get_mask_fg/bg`**（`:246-258`）：读 GTK 主题颜色，属控件层。
//!
//! ## 没移植的（记账）
//!
//! - **遮罩编辑交互**：`HCYMaskEditorWheel` 的拖顶点 / 加删形状 / 整体旋转
//!   （`__drag_active_shape` `:530-543`、`__drag_active_ctrlpoint` `:545-557`、
//!   `__rotate_mask` `:559-585`、`__add_void` `:704-736`、`__cleanup_mask`
//!   `:631-683`、`__update_active_objects` `:452-528`）与
//!   `HCYMaskPropertiesDialog`（`:1055-1265`）。本块只做**遮罩的应用**
//!   （画灰 + 取色裁剪）与预设选择。上面那些计算用到的常量
//!   （[`EDITOR_MIN_LUMA`] 等）照抄保留，供后续块直接取值。
//! - **遮罩的持久化与文件 I/O**：`PREFS_MASK_KEY` / `PREFS_ACTIVE_KEY`
//!   （`:43-44`）只保留键名，偏好在 B6；`set_mask_from_palette`
//!   （`:129-159`）的 GIMP 调色板加载与 `_flatten_mask`/`_unflatten_mask`
//!   （`:113-127`）的 prefs 序列化未移植。
//! - **`get_prefs()`**（`adjbases.py:414-419`）：偏好系统属 B6。

use super::model::{Color, HcyColor, UIColor};
use super::tools::{clamp, ScrollDirection, SCROLL_DELTA};
use super::wheels::{distort_hue, undistort_hue, HueRange, WheelGeometry, SAT_GAMMA};

// ---------------------------------------------------------------------------
// 常量（逐条对应源码行）
// ---------------------------------------------------------------------------

/// 遮罩形状的"最小有用尺寸"（占半径的比例）—— `hcywheel.py:67`
/// `min_shape_size = 0.15`。
///
/// 两处消费：绘制时丢掉太小的 void（`:289-295`），清理时丢掉太小的形状
/// （`:645-654`，未移植）。0.15 的意思是"半径的 15%"：100px 的轮子上，
/// 尺寸（[`void_size`]，面积的平方根）小于 15 的形状不画。
pub const MIN_SHAPE_SIZE: f64 = 0.15;

/// 遮罩前景/背景合成的不透明度 —— `hcywheel.py:311` 的
/// `cr.paint_with_alpha(0.666)`。
///
/// 遮罩的应用方式是：把整块盘面用**背景色**盖住，再把各 void 的内部挖空
/// （`:296-307` 的 `OPERATOR_SOURCE` + 全透明填充），最后整体以 0.666 的
/// 不透明度贴回 —— 于是"不可达区域"看上去像蒙了一层灰。
pub const MASK_DRAW_ALPHA: f64 = 0.666;

/// 遮罩 void 的描边宽度 —— `hcywheel.py:303` `cr.set_line_width(2.0)`。
pub const MASK_EDGE_LINE_WIDTH: f64 = 2.0;

/// 一个遮罩形状至少要有几个颜色才会被当成 void —— `hcywheel.py:186`
/// 的 `if len(shape) >= 3`。
///
/// 少于 3 个点凑不出面积，源码直接跳过（`get_mask_voids()`）。
pub const MASK_MIN_VOID_POINTS: usize = 3;

/// 遮罩编辑器里允许的最暗值 —— `hcywheel.py:391` `__MIN_LUMA = 0.25`。
///
/// 编辑器把轮子的明度夹在 0.25..0.75：太黑/太白时 HCY 彩度会退化
/// （`HCY_to_RGB` 在 `y=0`/`y=1` 处收敛），形状就没法辨认了。
pub const EDITOR_MIN_LUMA: f64 = 0.25;

/// 遮罩编辑器里允许的最亮值 —— `hcywheel.py:390` `__MAX_LUMA = 0.75`。
pub const EDITOR_MAX_LUMA: f64 = 0.75;

/// 编辑器的控制点半径 —— `hcywheel.py:394` `__ctrlpoint_radius = 2.5`。
pub const EDITOR_CTRLPOINT_RADIUS: f64 = 2.5;

/// 编辑器的控制点抓取半径 —— `hcywheel.py:395` `__ctrlpoint_grab_radius = 10`。
pub const EDITOR_CTRLPOINT_GRAB_RADIUS: f64 = 10.0;

/// 一个遮罩最多几个形状 —— `hcywheel.py:396` `__max_num_shapes = 6`。
pub const EDITOR_MAX_NUM_SHAPES: usize = 6;

/// 偏好键：遮罩形状表 —— `hcywheel.py:43` `PREFS_MASK_KEY`。
///
/// 值是"列表的列表"，每个 `UIColor` 以 `to_hex_str()` 扁平化存储
/// （`_flatten_mask`，`:113-119`）。键名照抄；偏好在 B6。
pub const PREFS_MASK_KEY: &str = "colors.hcywheel.mask.gamuts";

/// 偏好键：遮罩是否生效 —— `hcywheel.py:44` `PREFS_ACTIVE_KEY`。
pub const PREFS_ACTIVE_KEY: &str = "colors.hcywheel.mask.active";

/// HCY 轮的悬停提示 —— `hcywheel.py:344-347` 的 `STATIC_TOOLTIP_TEXT`。
pub const HCY_WHEEL_TOOLTIP: &str = "HCY hue and chroma.";

/// 遮罩开关的动作标题 —— `hcywheel.py:79-90` 的 `Gtk.ToggleAction` 文字。
pub const MASK_ACTIVE_LABEL: &str = "Gamut Mask Active";

/// 遮罩开关的悬停提示 —— `hcywheel.py:85-88`。
pub const MASK_ACTIVE_TOOLTIP: &str = "Limit your palette for specific moods using a gamut mask.";

/// 编辑器轮子（`HCYMaskEditorWheel`）的悬停提示 —— `hcywheel.py:400-405`。
///
/// 编辑器本身未移植（记账），文案照抄备用。
pub const MASK_EDITOR_TOOLTIP: &str = "Gamut mask editor. Click in the middle to create \
     or manipulate shapes, or rotate the mask using the edges of the disc.";

// ---------------------------------------------------------------------------
// `lib/alg.py` 的多边形子集（真源码，见模块文档）
// ---------------------------------------------------------------------------

/// 一个平面点：`(x, y)` 像素坐标（源码就是裸元组）。
pub type Point = (f64, f64);

/// 闭合配对迭代 —— `lib/alg.py:576-596` 的 `pairwise()`：
///
/// ```python
/// for item in seq:
///     if n == 0: first_item = item
///     else: yield prev_item, item
///     ...
/// if n > 1: yield item, first_item
/// ```
///
/// 产出 `n` 对（首尾相接），但**只有 `n > 1` 才产出**：0/1 个点的多边形
/// 一对都给不出来。这一点是有语义的 —— 见 [`point_in_convex_poly`]。
pub fn pairwise(poly: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    let n = poly.len();
    (0..n)
        .filter(move |_| n > 1)
        .map(move |i| (poly[i], poly[(i + 1) % n]))
}

/// 三点转向的行列式 —— `lib/alg.py:72-81`。
///
/// ```python
/// sum1 = q[0] * r[1] + p[0] * q[1] + r[0] * p[1]
/// sum2 = q[0] * p[1] + r[0] * q[1] + p[0] * r[1]
/// return sum1 - sum2
/// ```
///
/// 负 = 顺时针、零 = 共线。加法顺序照抄（浮点下顺序有意义）。
fn det(p: Point, q: Point, r: Point) -> f64 {
    let sum1 = q.0 * r.1 + p.0 * q.1 + r.0 * p.1;
    let sum2 = q.0 * p.1 + r.0 * q.1 + p.0 * r.1;
    sum1 - sum2
}

/// 凸包，按顺时针返回 —— `lib/alg.py:25-69` 的 `convex_hull()`（Graham 扫描）。
///
/// 逐步照抄：
/// 1. 去重（`dict.fromkeys`，保持首次出现顺序，`:38`）；
/// 2. 取 y 最小（并列时 x 最小）的点当极点 `p0`（`:42-46`）；
/// 3. 其余点按 `p0cos` 排序 —— 键是
///    `((p0x - x) / dist, (x, y))`（`:51-57`），比较时先比余弦再比坐标点；
/// 4. 栈式扫描，出现左转就弹出栈顶前一个（`:63-68` 的 `det(...) <= 0`）。
///
/// `p0cos` 的余弦取的是 `p0 - p` 而不是 `p - p0`：源码注释说 `p0` 的 y 最低，
/// 于是 `p0 -> p` 与 x 轴的夹角落在 `(0, pi)`，用余弦就够排序了（`:49-50`）。
///
/// 空输入在 Python 里会在 `points[0]` 处 `IndexError`，这里返回空表；
/// 调用方 [`colors_to_mask_void`] 已经用"至少 3 个颜色"挡在前面。
#[must_use]
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    // 去重，保序（`dict.fromkeys`）
    let mut uniq: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if !uniq.contains(p) {
            uniq.push(*p);
        }
    }
    if uniq.is_empty() {
        return Vec::new();
    }
    // y 最低、并列取 x 最低（`:42-46`）
    let mut p0 = uniq[0];
    for p in &uniq[1..] {
        if p.1 < p0.1 || (p.1 == p0.1 && p.0 < p0.0) {
            p0 = *p;
        }
    }
    uniq.retain(|p| *p != p0);

    // 按 (余弦, 点) 升序（`:51-57`）
    let mut keyed: Vec<((f64, Point), Point)> = uniq
        .iter()
        .map(|&p| {
            let dist = ((p0.0 - p.0).powi(2) + (p0.1 - p.1).powi(2)).sqrt();
            (((p0.0 - p.0) / dist, p), p)
        })
        .collect();
    keyed.sort_by(|a, b| {
        a.0 .0
            .partial_cmp(&b.0 .0)
            .expect("去重后 dist > 0，余弦不会是 NaN")
            .then_with(|| {
                a.0 .1
                    .0
                    .partial_cmp(&b.0 .1 .0)
                    .expect("坐标不会是 NaN")
                    .then(a.0 .1 .1.partial_cmp(&b.0 .1 .1).expect("坐标不会是 NaN"))
            })
    });
    let mut ordered: Vec<Point> = Vec::with_capacity(keyed.len() + 1);
    ordered.push(p0);
    ordered.extend(keyed.into_iter().map(|(_, p)| p));

    // 栈式扫描（`:63-68`）
    let mut hull: Vec<Point> = ordered.iter().take(2).copied().collect();
    for &p in ordered.iter().skip(2) {
        hull.push(p);
        while hull.len() > 2 && {
            let n = hull.len();
            det(hull[n - 3], hull[n - 2], hull[n - 1]) <= 0.0
        } {
            hull.remove(hull.len() - 2);
        }
    }
    hull
}

/// 多边形面积（有向，按 `pairwise` 顺序累加）—— `lib/alg.py:84-95`：
///
/// ```python
/// for pa, pb in pairwise(poly):
///     area += pa[0] * pb[1] - pb[0] * pa[1]
/// area /= 2.0
/// ```
pub fn poly_area(poly: &[Point]) -> f64 {
    let mut area = 0.0;
    for (pa, pb) in pairwise(poly) {
        area += pa.0 * pb.1 - pb.0 * pa.1;
    }
    area / 2.0
}

/// 点在凸多边形内（含边界）—— `lib/alg.py:127-159` 的
/// `point_in_convex_poly()`：
///
/// ```python
/// for p0, p1 in pairwise(poly):
///     det = ((y - y0) * (x1 - x0)) - ((x - x0) * (y1 - y0))
///     if det < 0:
///         if seen_left: return False
///         seen_right = True
///     elif det > 0:
///         if seen_right: return False
///         seen_left = True
/// return True
/// ```
///
/// 判据是"所有边都在同一侧"。**两个行为必须照抄**：
/// - 边界上（`det == 0`）算**在内**（`:157-159`）；
/// - 多边形点数 < 2 时 `pairwise` 一对都不产 → 直接 `True`
///   （任何点都算"在里面"）—— 见 [`pairwise`]。
#[must_use]
pub fn point_in_convex_poly(point: Point, poly: &[Point]) -> bool {
    let (x, y) = point;
    let mut seen_left = false;
    let mut seen_right = false;
    for (p0, p1) in pairwise(poly) {
        let (x0, y0) = p0;
        let (x1, y1) = p1;
        let d = ((y - y0) * (x1 - x0)) - ((x - x0) * (y1 - y0));
        if d < 0.0 {
            if seen_left {
                return false;
            }
            seen_right = true;
        } else if d > 0.0 {
            if seen_right {
                return false;
            }
            seen_left = true;
        }
    }
    true
}

/// 线段上离给定点最近的那一点（按垂直投影，**不含端点**）——
/// `lib/alg.py:162-186` 的 `nearest_point_in_segment()`，实现是
/// `_nearest_point(..., perpendicular=True, inclusive=False, line_type=SEGMENT)`
/// （`:262-323`）：
///
/// ```python
/// denominator = (x2 - x1) ** 2 + (y2 - y1) ** 2
/// if denominator == 0: return None            # 两端重合
/// u = ((x3 - x1) * (x2 - x1) + (y3 - y1) * (y2 - y1)) / denominator
/// outside = not (0 < u < 1)                   # 严格开区间
/// if outside and perpendicular:
///     if line_type == SEGMENT: return None    # 投影落在段外
/// return x1 + u * (x2 - x1), y1 + u * (y2 - y1)
/// ```
///
/// 注意 `0 < u < 1` 两端都是**开**区间：正好落在端点上的投影返回 `None`，
/// 这就是为什么 `MaskableWheelMixin.get_color_at_position()` 要额外把每个
/// `p1` 及其距离塞进候选表（`hcywheel.py:231-233`）。
#[must_use]
pub fn nearest_point_in_segment(seg_start: Point, seg_end: Point, point: Point) -> Option<Point> {
    let (x1, y1) = seg_start;
    let (x2, y2) = seg_end;
    let (x3, y3) = point;
    let denominator = (x2 - x1).powi(2) + (y2 - y1).powi(2);
    if denominator == 0.0 {
        return None;
    }
    let u = ((x3 - x1) * (x2 - x1) + (y3 - y1) * (y2 - y1)) / denominator;
    if !(0.0 < u && u < 1.0) {
        return None;
    }
    Some((x1 + u * (x2 - x1), y1 + u * (y2 - y1)))
}

/// 遮罩 void 的尺寸度量 —— `hcywheel.py:240-244` 的 `_get_void_size()`：
///
/// ```python
/// area = geom.poly_area(void)
/// return math.sqrt(area)
/// ```
///
/// 是个"面积的平方根"，与半径同量纲，所以可以直接跟
/// `radius * MIN_SHAPE_SIZE` 比（`:289`）。
///
/// 负面积在 Python 里会 `ValueError`；这里得 `NaN`（见模块文档），
/// 于是比较为假、形状不画。
#[must_use]
pub fn void_size(void: &[Point]) -> f64 {
    poly_area(void).sqrt()
}

// ---------------------------------------------------------------------------
// HCY 轮的钩子（`hcywheel.py:320-336`）
// ---------------------------------------------------------------------------

/// HCY 轮：颜色 → `(彩度, 色相)` —— `HCYHueChromaWheelMixin.
/// get_normalized_polar_pos_for_color()`（`hcywheel.py:328-330`）：
///
/// ```python
/// col = HCYColor(color=col)
/// return col.c, col.h
/// ```
///
/// 与 HSV 轮（`hsvwheel.py:49-51` 返回 `col.s, col.h`）逐行同构：**半径就是
/// HCY 彩度**。彩度可以到 2（`lib/color.py:809`），而半径比被
/// [`WheelGeometry::position_for`] 夹到 1 —— 所以高彩度色的标记贴着圆边。
#[must_use]
pub fn hcy_wheel_polar_for_color(color: &Color) -> (f64, f64) {
    let col = HcyColor::from_color(color);
    (col.c, col.h)
}

/// HCY 轮：`(彩度, 色相)` → 颜色 —— `HCYHueChromaWheelMixin.
/// color_at_normalized_polar_pos()`（`hcywheel.py:332-336`）：
///
/// ```python
/// col = HCYColor(color=self.get_managed_color())
/// col.h = theta
/// col.c = r
/// return col
/// ```
///
/// 明度（`y`）原样保留 —— 所以圆盘上**同一半径的一圈都是等明度的**
/// （页面的说明文字就是 "The circular slices are equiluminant."，
/// `hcywheel.py:1321-1323`）。
#[must_use]
pub fn hcy_wheel_color_at_polar(managed: &Color, r: f64, theta: f64) -> HcyColor {
    let mut col = HcyColor::from_color(managed);
    col.h = theta;
    col.c = r;
    col
}

/// 轮上取色：位置 → 颜色 —— `HueSaturationWheelMixin.get_color_at_position()`
/// （`adjbases.py:1148-1167`）套用 HCY 的两个钩子。
///
/// 顺序照抄：极坐标（含 `SAT_GAMMA`）→ 逆畸变（`:1164-1166`）→
/// `color_at_normalized_polar_pos`。
pub fn hcy_wheel_color_at_position(
    geom: &WheelGeometry,
    x: f64,
    y: f64,
    managed: &Color,
    ranges: Option<&[HueRange]>,
) -> Option<HcyColor> {
    let (r, theta) = geom.polar_at(x, y, SAT_GAMMA)?;
    let theta = undistort_hue(theta, ranges);
    Some(hcy_wheel_color_at_polar(managed, r, theta))
}

/// 标记位置：颜色 → 像素 —— `get_pos_for_color()`（`adjbases.py:1301-1315`）
/// 套用 HCY 的钩子。
///
/// 源码先 `distort_hue(ntheta)`（`:1303-1305`）再进几何（`:1306-1314`），
/// 与本函数一致。
#[must_use]
pub fn hcy_wheel_position_for_color(
    geom: &WheelGeometry,
    color: &Color,
    ranges: Option<&[HueRange]>,
) -> (f64, f64) {
    let (nr, ntheta) = hcy_wheel_polar_for_color(color);
    let ntheta = distort_hue(ntheta, ranges);
    geom.position_for(nr, ntheta, SAT_GAMMA)
}

/// 盘面上某个色相的边缘色 —— `adjbases.py:1208-1209` 的
/// `color_at_normalized_polar_pos(1.0, h)`，再 `get_rgb()`。
///
/// `r = 1.0` 是圆边，也就是**满彩度**（`c = 1`）；明度跟着当前色走，所以
/// 整块盘面的明度随 HCY 明度一起变。与 HSV 轮的 `hsv_wheel_slice_rgb`
/// （`wheels.rs`）逐行同构，只是钩子换成 HCY。
#[must_use]
pub fn hcy_wheel_slice_rgb(managed: &Color, theta: f64) -> [f64; 3] {
    hcy_wheel_color_at_polar(managed, 1.0, theta).get_rgb()
}

/// 盘心那层去饱和渐变的颜色 —— `adjbases.py:1173` 的
/// `ref_grey = color_at_normalized_polar_pos(0, 0)`。
///
/// HCY 下 `c = 0` → `HCY_to_RGB` 直接返回 `(y, y, y)`（`lib/color.py:864-865`），
/// 所以盘心是**当前 HCY 明度的灰**，不是 HSV 的 `(v, v, v)`。源码
/// `:1141-1146` 的 `get_background_validity()` 还断言过这一点
/// （`assert k == min(rgb)`）。
#[must_use]
pub fn hcy_wheel_center_rgb(managed: &Color) -> [f64; 3] {
    hcy_wheel_color_at_polar(managed, 0.0, 0.0).get_rgb()
}

/// 轮上滚轮改明度 —— `HCYHueChromaWheel.__scroll_cb()`（`hcywheel.py:356-367`）：
///
/// ```python
/// d = self.SCROLL_DELTA
/// directions = (Gdk.ScrollDirection.DOWN, Gdk.ScrollDirection.LEFT)
/// if event.direction in directions:
///     d *= -1
/// col = HCYColor(color=self.get_managed_color())
/// y = clamp(col.y + d, 0.0, 1.0)
/// if col.y != y:
///     col.y = y
///     self.set_managed_color(col)
/// return True
/// ```
///
/// 与 HSV 轮（`hsvwheel.py:35-47`）同构，只有"改哪个分量"不同：这里改 HCY
/// 的 `y`。返回 `None` 表示 `y` 没变（源码那个 `if` 不成立，不会落色）。
#[must_use]
pub fn hcy_wheel_scroll(managed: &Color, direction: ScrollDirection) -> Option<Color> {
    let mut d = SCROLL_DELTA;
    if matches!(direction, ScrollDirection::Down | ScrollDirection::Left) {
        d *= -1.0;
    }
    let mut col = HcyColor::from_color(managed);
    let y = clamp(col.y + d, 0.0, 1.0);
    if col.y != y {
        col.y = y;
        Some(Color::Hcy(col))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 遮罩：颜色 → void → 取色裁剪（`hcywheel.py:180-244`）
// ---------------------------------------------------------------------------

/// 遮罩里的一个形状：一组 `UIColor`。源码就是 `list of lists of UIColor`
/// （`MaskableWheelMixin.__mask`，`hcywheel.py:70/76`）。
pub type MaskShape = Vec<Color>;

/// 颜色集合 → 遮罩 void（凸包）—— `MaskableWheelMixin.colors_to_mask_void()`
/// （`hcywheel.py:191-204`）：
///
/// ```python
/// points = []
/// if len(colors) < 3:
///     return points
/// for col in colors:
///     points.append(self.get_pos_for_color(col))
/// return geom.convex_hull(points)
/// ```
///
/// 也就是说 void 就是**形状里各颜色的屏幕位置的凸包** —— 形状本身不必是凸的，
/// 画出来/命中用的永远是凸包。位置由 [`hcy_wheel_position_for_color`] 给出
/// （含色相畸变与 `int()+0.5` 量化）。
#[must_use]
pub fn colors_to_mask_void(
    colors: &[Color],
    geom: &WheelGeometry,
    ranges: Option<&[HueRange]>,
) -> Vec<Point> {
    if colors.len() < MASK_MIN_VOID_POINTS {
        return Vec::new();
    }
    let points: Vec<Point> = colors
        .iter()
        .map(|c| hcy_wheel_position_for_color(geom, c, ranges))
        .collect();
    convex_hull(&points)
}

/// 整块遮罩 → 各个 void —— `MaskableWheelMixin.get_mask_voids()`
/// （`hcywheel.py:180-189`）：
///
/// ```python
/// voids = []
/// if not self.__mask:
///     return voids
/// for shape in self.__mask:
///     if len(shape) >= 3:
///         void = self.colors_to_mask_void(shape)
///         voids.append(void)
/// return voids
/// ```
///
/// 注意过滤条件是**颜色个数** ≥ 3，不是凸包后的点数：凸包可能退化成一两个
/// 点（颜色位置重合时），这种 void 仍然会出现在结果里（之后由尺寸过滤掉）。
#[must_use]
pub fn mask_voids(
    mask: &[MaskShape],
    geom: &WheelGeometry,
    ranges: Option<&[HueRange]>,
) -> Vec<Vec<Point>> {
    let mut voids = Vec::new();
    for shape in mask {
        if shape.len() >= MASK_MIN_VOID_POINTS {
            voids.push(colors_to_mask_void(shape, geom, ranges));
        }
    }
    voids
}

/// 真正会被画出来的 void —— `MaskableWheelMixin.draw_mask()` 的过滤
/// （`hcywheel.py:288-295`）：
///
/// ```python
/// voids = []
/// min_size = radius * self.min_shape_size
/// for void in self.get_mask_voids():
///     if len(void) < 3:
///         continue
///     size = self._get_void_size(void)
///     if size >= min_size:
///         voids.append(void)
/// ```
///
/// 两层过滤：点数 < 3（凸包退化）与尺寸 < `radius * MIN_SHAPE_SIZE`。
/// 顺序与源码一致（先点数、后尺寸），但两者都是纯判断，顺序不影响结果。
#[must_use]
pub fn visible_mask_voids(
    mask: &[MaskShape],
    geom: &WheelGeometry,
    ranges: Option<&[HueRange]>,
    radius: f64,
) -> Vec<Vec<Point>> {
    let min_size = radius * MIN_SHAPE_SIZE;
    mask_voids(mask, geom, ranges)
        .into_iter()
        .filter(|void| {
            if void.len() < 3 {
                return false;
            }
            void_size(void) >= min_size
        })
        .collect()
}

/// 取色时用遮罩裁剪 —— `MaskableWheelMixin.get_color_at_position()`
/// （`hcywheel.py:206-238`）：
///
/// ```python
/// sup = HueSaturationWheelMixin
/// if ignore_mask or not self.mask_toggle.get_active():
///     return sup.get_color_at_position(self, x, y)
/// voids = self.get_mask_voids()
/// if not voids:
///     return sup.get_color_at_position(self, x, y)
/// isects = []
/// for vi, void in enumerate(voids):
///     if geom.point_in_convex_poly((x, y), void):
///         return sup.get_color_at_position(self, x, y)
///     for p1, p2 in geom.pairwise(void):
///         isect = geom.nearest_point_in_segment(p1, p2, (x, y))
///         if isect is not None:
///             d = sqrt((isect[0]-x)**2 + (isect[1]-y)**2)
///             isects.append((d, isect))
///         d = sqrt((p1[0]-x)**2 + (p1[1]-y)**2)
///         isects.append((d, p1))
/// if isects:
///     isects.sort()
///     x, y = isects[0][1]
/// return sup.get_color_at_position(self, x, y)
/// ```
///
/// 语义：**点在任一 void 内 → 原样取色；否则映射到"最近一个 void 的最近
/// 边/顶点"上取色**。这就是"把不可达区域裁掉"的实质（画灰只是它的视觉提示）。
///
/// 三个必须照抄的细节：
/// 1. 判定"在内"用的是**未经尺寸过滤**的 `get_mask_voids()`（`:217`），
///    而画面上看到的是 [`visible_mask_voids`] —— 太小的 void 虽然不画，却
///    仍然参与命中（源码就是这样）。
/// 2. 候选表里除了垂足，每个 `pairwise` 的 `p1` 也进候选（`:231-233`，
///    因为 [`nearest_point_in_segment`] 不含端点）。
/// 3. `isects.sort()` 是**元组排序**：先比距离，距离相同再比点坐标
///    （`:236`）—— 所以最近点不唯一时取坐标字典序最小的那个。
pub fn masked_color_at_position(
    mask: &[MaskShape],
    geom: &WheelGeometry,
    x: f64,
    y: f64,
    managed: &Color,
    ranges: Option<&[HueRange]>,
) -> Option<HcyColor> {
    let voids = mask_voids(mask, geom, ranges);
    let (mut sx, mut sy) = (x, y);
    if !voids.is_empty() {
        let mut isects: Vec<(f64, Point)> = Vec::new();
        for void in &voids {
            if point_in_convex_poly((x, y), void) {
                // 在里面：原样
                return hcy_wheel_color_at_position(geom, x, y, managed, ranges);
            }
            for (p1, p2) in pairwise(void) {
                if let Some(isect) = nearest_point_in_segment(p1, p2, (x, y)) {
                    let d = ((isect.0 - x).powi(2) + (isect.1 - y).powi(2)).sqrt();
                    isects.push((d, isect));
                }
                let d = ((p1.0 - x).powi(2) + (p1.1 - y).powi(2)).sqrt();
                isects.push((d, p1));
            }
        }
        if !isects.is_empty() {
            isects.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .expect("距离不会是 NaN")
                    .then(a.1 .0.partial_cmp(&b.1 .0).expect("坐标不会是 NaN"))
                    .then(a.1 .1.partial_cmp(&b.1 .1).expect("坐标不会是 NaN"))
            });
            sx = isects[0].1 .0;
            sy = isects[0].1 .1;
        }
    }
    hcy_wheel_color_at_position(geom, sx, sy, managed, ranges)
}

// ---------------------------------------------------------------------------
// mood 预设（`hcywheel.py:868-988` 的 `HCYMaskTemplateDialog.__templates`）
// ---------------------------------------------------------------------------

/// 一个遮罩模板：名字、说明、若干形状（每个形状是一串 `(h, c, y)`）。
///
/// 三段信息全部来自 `__templates` 里的三元组（`hcywheel.py:902-987`）。
/// `h` 是**未畸变**的色相；套到具体轮子上还要过一次 `undistort_hue`
/// （见 [`template_mask`]，对应 `:1013`）。
#[derive(Clone, Debug, PartialEq)]
pub struct MaskTemplate {
    /// 模板名（`C_("HCY Gamut Mask template name", ...)`）。
    pub name: &'static str,
    /// 模板说明（`C_("HCY Gamut Mask template description", ...)`）。
    pub description: &'static str,
    /// 形状表：形状 → 点 → `[h, c, y]`。
    pub shapes: Vec<Vec<[f64; 3]>>,
}

/// 模板里所有点共用的明度 —— `hcywheel.py:869` `Y = 0.5`。
///
/// 所有预设都在"中等明度"这一层，所以遮罩只限制色相与彩度。
const TEMPLATE_Y: f64 = 0.5;

/// 模板主色相的基数 —— `hcywheel.py:870` `H = 1 - 0.05`。
///
/// 取 0.95 而不是 0，是为了让"主色"落在**红**上（色相 0.95 略偏品红，
/// 视觉上仍是红），同时给"环绕"留出余量 —— 后面大量 `(H ± d) % 1`
/// 就会绕回 0 附近。
fn template_h() -> f64 {
    1.0 - 0.05
}

/// 可复用的主三角 —— `hcywheel.py:872-876` 的 `atmos_triad`：
///
/// ```python
/// atmos_triad = [
///     (H, 0.95, Y),
///     ((H + 0.275) % 1, 0.55, Y),
///     ((1 + H - 0.275) % 1, 0.55, Y),
/// ]
/// ```
///
/// 一个强主色 + 两个弱一些的补色（角度 ±0.275，彩度 0.55）。
fn atmos_triad() -> Vec<[f64; 3]> {
    let y = TEMPLATE_Y;
    let h = template_h();
    vec![
        [h, 0.95, y],
        [(h + 0.275) % 1.0, 0.55, y],
        [(1.0 + h - 0.275) % 1.0, 0.55, y],
    ]
}

/// "棺材形"六边形 —— `hcywheel.py:878-888` 的 `__coffin(h)`：
///
/// ```python
/// shape.append(((h + 0.25) % 1, 0.03, Y))
/// shape.append(((h + 1 - 0.25) % 1, 0.03, Y))
/// shape.append(((h + 0.01) % 1, 0.95, Y))
/// shape.append(((h + 1 - 0.01) % 1, 0.95, Y))
/// shape.append(((h + 0.04) % 1, 0.70, Y))
/// shape.append(((h + 1 - 0.04) % 1, 0.70, Y))
/// ```
///
/// 顶点顺序是"绕一圈"：先两个近中心（彩度 0.03）的窄端，再两个高彩度
/// （0.95）的宽端，最后两个 0.70 的肩 —— 连起来才是六边形。
/// 注释说"脚端在盘心"（`:879`），即彩度 0.03 那端朝里。
fn coffin(h: f64) -> Vec<[f64; 3]> {
    let y = TEMPLATE_Y;
    vec![
        [(h + 0.25) % 1.0, 0.03, y],
        [(h + 1.0 - 0.25) % 1.0, 0.03, y],
        [(h + 0.01) % 1.0, 0.95, y],
        [(h + 1.0 - 0.01) % 1.0, 0.95, y],
        [(h + 0.04) % 1.0, 0.70, y],
        [(h + 1.0 - 0.04) % 1.0, 0.70, y],
    ]
}

/// 小五边形"色块" —— `hcywheel.py:890-899` 的 `__complement_blob(h)`：
///
/// ```python
/// shape.append(((h + 0.015) % 1, 0.94, Y))
/// shape.append(((h + 0.985) % 1, 0.94, Y))
/// shape.append(((h + 0.035) % 1, 0.71, Y))
/// shape.append(((h + 0.965) % 1, 0.71, Y))
/// shape.append(((h) % 1, 0.54, Y))
/// ```
///
/// 用来给"mood and accent"补一小块互补色的点缀（`:890-892` 的注释）。
/// 注意第三、四个顶点用的是 `h + 0.985` / `h + 0.965`（不是 `h - 0.015`）：
/// 对 `h = 1.45` 这种"绕了一圈"的输入，`+0.985` 与 `-0.015` 在浮点上
/// 结果不同，值以源码写的为准。
fn complement_blob(h: f64) -> Vec<[f64; 3]> {
    let y = TEMPLATE_Y;
    vec![
        [(h + 0.015) % 1.0, 0.94, y],
        [(h + 0.985) % 1.0, 0.94, y],
        [(h + 0.035) % 1.0, 0.71, y],
        [(h + 0.965) % 1.0, 0.71, y],
        [h % 1.0, 0.54, y],
    ]
}

/// 五个预设遮罩 —— `hcywheel.py:868-988` 的 `__templates`（顺序即对话框里的
/// 顺序）。
///
/// 形状表逐条对应：
///
/// | 名字 | 源码 | 形状 |
/// |---|---|---|
/// | Atmospheric Triad | `:902-915` | `[atmos_triad]` |
/// | Shifted Triad | `:916-934` | 一个三角，角度 ±0.35、彩度 0.4 |
/// | Complementary | `:935-958` | 一个六边形：两侧高彩度 + 两个近中性低彩度 |
/// | Mood and Accent | `:959-973` | `[atmos_triad, complement_blob(H+0.5)]` |
/// | Split Complementary | `:974-987` | 三个 coffin：`H+0.5`、`1+H-0.1`、`H+0.1` |
///
/// 浮点是**照源码表达式现算**的（不写死十进制常量），因为像
/// `(H + 0.275) % 1` = `0.2250000000000001` 这种值抄错一位就过不了零容差门禁。
#[must_use]
pub fn mask_templates() -> Vec<MaskTemplate> {
    let y = TEMPLATE_Y;
    let h = template_h();
    vec![
        MaskTemplate {
            name: "Atmospheric Triad",
            description: "Moody and subjective, defined by one dominant primary \
                          and two primaries which are less intense.",
            shapes: vec![atmos_triad()],
        },
        MaskTemplate {
            name: "Shifted Triad",
            description: "Weighted more strongly towards the dominant color.",
            shapes: vec![vec![
                [h, 0.95, y],
                [(h + 0.35) % 1.0, 0.4, y],
                [(1.0 + h - 0.35) % 1.0, 0.4, y],
            ]],
        },
        MaskTemplate {
            name: "Complementary",
            description: "Contrasting opposites, balanced by having central neutrals \
                          between them on the color wheel.",
            shapes: vec![vec![
                [(h + 0.005) % 1.0, 0.9, y],
                [(h + 0.995) % 1.0, 0.9, y],
                [(h + 0.250) % 1.0, 0.1, y],
                [(h + 0.750) % 1.0, 0.1, y],
                [(h + 0.505) % 1.0, 0.9, y],
                [(h + 0.495) % 1.0, 0.9, y],
            ]],
        },
        MaskTemplate {
            name: "Mood and Accent",
            description: "One main range of colors, with a complementary accent for \
                          variation and highlights.",
            shapes: vec![atmos_triad(), complement_blob(h + 0.5)],
        },
        MaskTemplate {
            name: "Split Complementary",
            description: "Two analogous colors and a complement to them, \
                          with no secondary colors between them.",
            shapes: vec![
                coffin(h + 0.5),
                coffin(1.0 + h - 0.1),
                coffin(h + 0.1),
            ],
        },
    ]
}

/// 整体旋转遮罩 —— `HCYMaskEditorWheel` 拖轮缘的 `__rotate_drag`
/// （`hcywheel.py:760-790`）：所有形状的色相加同一个 `delta`。
pub fn rotate_mask(mask: &mut [MaskShape], delta: f64) {
    for shape in mask.iter_mut() {
        for color in shape.iter_mut() {
            let mut hcy = HcyColor::from_color(color);
            hcy.h = (hcy.h + delta).rem_euclid(1.0);
            *color = Color::Hcy(hcy);
        }
    }
}

/// 模板 → 真正的遮罩（`HCYColor` 列表）—— `HCYMaskTemplateDialog.__init__`
/// （`hcywheel.py:1008-1015`）：
///
/// ```python
/// for name, desc, mask_shapes_float in self.__templates:
///     mask = []
///     for mask_shape_float in mask_shapes_float:
///         shape = []
///         for h, c, y in mask_shape_float:
///             h = mgr.undistort_hue(h)
///             shape.append(HCYColor(h, c, y))
///         mask.append(shape)
/// ```
///
/// 只有色相过 `undistort_hue`（RYB/RYGB 轮上才真的改值），彩度/明度原样。
#[must_use]
pub fn template_mask(template: &MaskTemplate, ranges: Option<&[HueRange]>) -> Vec<MaskShape> {
    template
        .shapes
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|&[h, c, y]| Color::Hcy(HcyColor::new(undistort_hue(h, ranges), c, y)))
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 滑条与编辑器的小算术（`sliders.py:281-305`、`hcywheel.py:811-822`）
// ---------------------------------------------------------------------------

/// HCY 明度滑条的采样段数 —— `HCYLumaSlider.samples`（`sliders.py:287-292`）：
///
/// ```python
/// alloc = self.get_allocation()
/// len = self.vertical and alloc.height or alloc.width
/// len -= self.BORDER_WIDTH * 2
/// return min(int(len // 3), 64)
/// ```
///
/// 三个必须照抄的点：
/// 1. `and/or` 是 Python 的短路**取值**表达式，不是三元判断：竖条但高度是
///    `0.0`（假值）时会**回落到宽度**。所以参数是 `(width, height, vertical)`，
///    而不是一个"长度"。
/// 2. `//` 是**向下取整**（与 `int()` 的向零截断不同），外面再包一个 `int()`
///    只是转成整数。
/// 3. 长度越长采样越密，但**最多 64 段** —— 明度渐变不需要更细。
///
/// `width < 2 * border`（或回落到宽度后仍不够）时结果是负数，源码不设防；
/// 这里同样返回负数，调用方自己夹取。
#[must_use]
pub fn hcy_luma_slider_samples(width: f64, height: f64, vertical: bool, border: f64) -> i64 {
    // `self.vertical and alloc.height or alloc.width`
    let len = if vertical && height != 0.0 {
        height
    } else {
        width
    };
    let len = len - border * 2.0;
    let n = (len / 3.0).floor() as i64;
    n.min(64)
}

/// 编辑器轮子的明度夹取 —— `HCYMaskEditorWheel.get_managed_color()`
/// （`hcywheel.py:811-816`）与 `set_managed_color()`（`:818-822`）：
///
/// ```python
/// col.y = clamp(col.y, self.__MIN_LUMA, self.__MAX_LUMA)
/// ```
///
/// 两端都夹：编辑器不允许把轮子推到全黑/全白（那样彩度会退化，
/// 形状边界看不出来）。`clamp` 来自 `gui/colors/util.py:18-24`。
#[must_use]
pub fn editor_luma_y(y: f64) -> f64 {
    clamp(y, EDITOR_MIN_LUMA, EDITOR_MAX_LUMA)
}

// ---------------------------------------------------------------------------
// 测试（期望值都能从上面的源码行号手推；逐位门禁在
// `tests/wheel_vectors.rs` 的 `hcy_*` 部分）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::wheels::{HUE_SLICES, SAT_SLICES};
    use crate::color::{Color, SCROLL_DELTA};

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn rotate_mask_turns_every_point_and_wraps() {
        let mut mask: Vec<MaskShape> = vec![vec![
            Color::Hcy(HcyColor::new(0.9, 0.5, 0.5)),
            Color::Hcy(HcyColor::new(0.1, 0.4, 0.6)),
        ]];
        rotate_mask(&mut mask, 0.25);
        let a = HcyColor::from_color(&mask[0][0]);
        let b = HcyColor::from_color(&mask[0][1]);
        assert!(close(a.h, 0.15, 1e-9), "0.9+0.25 应回绕到 0.15，实得 {}", a.h);
        assert!(close(b.h, 0.35, 1e-9));
        assert!(close(a.c, 0.5, 1e-9) && close(a.y, 0.5, 1e-9));
    }

    /// 常量 = 源码类属性/模块常量（`hcywheel.py:43-44`、`:67`、`:303-311`、
    /// `:390-396`、`:344-347`、`:79-90`）。
    #[test]
    fn constants_match_source_class_attributes() {
        assert_eq!(MIN_SHAPE_SIZE, 0.15, "hcywheel.py:67");
        assert_eq!(MASK_DRAW_ALPHA, 0.666, "hcywheel.py:311");
        assert_eq!(MASK_EDGE_LINE_WIDTH, 2.0, "hcywheel.py:303");
        assert_eq!(MASK_MIN_VOID_POINTS, 3, "hcywheel.py:186");
        assert_eq!(EDITOR_MIN_LUMA, 0.25, "hcywheel.py:391");
        assert_eq!(EDITOR_MAX_LUMA, 0.75, "hcywheel.py:390");
        assert_eq!(EDITOR_CTRLPOINT_RADIUS, 2.5, "hcywheel.py:394");
        assert_eq!(EDITOR_CTRLPOINT_GRAB_RADIUS, 10.0, "hcywheel.py:395");
        assert_eq!(EDITOR_MAX_NUM_SHAPES, 6, "hcywheel.py:396");
        assert_eq!(PREFS_MASK_KEY, "colors.hcywheel.mask.gamuts", ":43");
        assert_eq!(PREFS_ACTIVE_KEY, "colors.hcywheel.mask.active", ":44");
        assert_eq!(HCY_WHEEL_TOOLTIP, "HCY hue and chroma.", ":344-347");
        assert_eq!(MASK_ACTIVE_LABEL, "Gamut Mask Active", ":79-90");
        // 明度滑条的采样上限是 64（`sliders.py:292`）
        assert_eq!(hcy_luma_slider_samples(18.0, 1.0e6, true, 2.0), 64);
        // 两个色轮的扇区/渐变常量沿用 mixin（HCY 轮没有覆盖它们）
        assert_eq!(HUE_SLICES, 64);
        assert_eq!(SAT_SLICES, 5);
    }

    /// `pairwise` 的边界：0/1 个点一对都不产 —— `lib/alg.py:576-596`。
    #[test]
    fn pairwise_matches_source_edge_cases() {
        assert_eq!(pairwise(&[]).count(), 0);
        assert_eq!(pairwise(&[(1.0, 2.0)]).count(), 0, "单点无配对（n > 1 才产）");
        assert_eq!(
            pairwise(&[(0.0, 0.0), (1.0, 0.0)]).collect::<Vec<_>>(),
            vec![
                ((0.0, 0.0), (1.0, 0.0)),
                ((1.0, 0.0), (0.0, 0.0)),
            ],
            "两点也是一对半（首尾相接）"
        );
        // 正方形：4 对，最后一对回到起点
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let pairs: Vec<_> = pairwise(&sq).collect();
        assert_eq!(pairs.len(), 4);
        assert_eq!(pairs[3], ((0.0, 1.0), (0.0, 0.0)));
    }

    /// 凸包 —— `lib/alg.py:25-69`（含 docstring 里那个例子）。
    #[test]
    fn convex_hull_matches_source_examples() {
        // docstring 的例子（`:28-29`）
        let hull = convex_hull(&[
            (1.0, 1.0),
            (1.0, -1.0),
            (0.0, 0.0),
            (-1.0, -1.0),
            (-1.0, 1.0),
        ]);
        assert_eq!(
            hull,
            vec![(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)],
            "y 最低点起、顺时针"
        );
        // 内部点被吃掉；重复点被去重
        let hull = convex_hull(&[
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (2.0, 2.0),
            (2.0, 2.0),
        ]);
        assert_eq!(hull.len(), 4);
        // 退化：一个点/两个点原样返回
        assert_eq!(convex_hull(&[(3.0, 4.0), (3.0, 4.0)]), vec![(3.0, 4.0)]);
        assert_eq!(convex_hull(&[]), Vec::new());
        // 面积是正的（`_get_void_size` 要开方，见模块文档第 2 条）
        assert!(poly_area(&hull_of_unit_square()) > 0.0);
    }

    fn hull_of_unit_square() -> Vec<Point> {
        convex_hull(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
    }

    /// 面积与"尺寸" —— `lib/alg.py:84-95` + `hcywheel.py:240-244`。
    #[test]
    fn poly_area_and_void_size_match_source() {
        // docstring：边长 2 的正方形面积 4（`:87-88`）
        let sq = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        assert_eq!(poly_area(&sq), 4.0);
        // 尺寸 = sqrt(面积) → 2
        assert_eq!(void_size(&sq), 2.0);
        // 退化成一/两点：面积 0、尺寸 0（源码里会 `< min_size` 被丢掉）
        assert_eq!(poly_area(&[(1.0, 1.0)]), 0.0);
        assert_eq!(void_size(&[(1.0, 1.0)]), 0.0);
    }

    /// 点在凸多边形内（含边界）—— `lib/alg.py:127-159`。
    #[test]
    fn point_in_convex_poly_matches_source_examples() {
        let square = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        assert!(point_in_convex_poly((0.0, 0.0), &square));
        assert!(point_in_convex_poly((0.0, 1.0), &square), "边界算在内");
        assert!(!point_in_convex_poly((0.0, 1.1), &square));
        assert!(!point_in_convex_poly((2.0, 0.0), &square));
        // 顶点顺序反着也给同样的答案（docstring `:139-140`）
        let mut rev = square;
        rev.reverse();
        assert!(point_in_convex_poly((0.0, 0.0), &rev));
        // 点数 < 2：pairwise 不产任何边 → 恒 True（源码的怪癖）
        assert!(point_in_convex_poly((99.0, 99.0), &[(0.0, 0.0)]));
        assert!(point_in_convex_poly((99.0, 99.0), &[]));
    }

    /// 线段垂足（不含端点）—— `lib/alg.py:162-186` 的 docstring 例子。
    #[test]
    fn nearest_point_in_segment_matches_source_examples() {
        assert_eq!(
            nearest_point_in_segment((0.0, 0.0), (4.0, 0.0), (2.0, 2.0)),
            Some((2.0, 0.0))
        );
        assert_eq!(
            nearest_point_in_segment((1.0, 1.0), (3.0, 3.0), (2.0, 1.0)),
            Some((1.5, 1.5))
        );
        // 投影落在段外 → None
        assert_eq!(
            nearest_point_in_segment((0.0, 0.0), (3.0, 0.0), (0.0, 1.0)),
            None
        );
        assert_eq!(
            nearest_point_in_segment((1.0, 1.0), (3.0, 3.0), (12.0, -1.0)),
            None
        );
        // 两端重合 → None（除零保护）
        assert_eq!(
            nearest_point_in_segment((1.0, 1.0), (1.0, 1.0), (2.0, 2.0)),
            None
        );
    }

    /// 两个 HCY 钩子：半径 = 彩度、角度 = 色相，`y` 不动
    /// （`hcywheel.py:328-336`）。
    #[test]
    fn hcy_wheel_hooks_set_chroma_and_hue() {
        let managed = Color::Hcy(HcyColor::new(0.4, 0.3, 0.6));
        assert_eq!(hcy_wheel_polar_for_color(&managed), (0.3, 0.4));
        let col = hcy_wheel_color_at_polar(&managed, 0.8, 0.13);
        assert_eq!((col.h, col.c, col.y), (0.13, 0.8, 0.6));
        // 取色：正下方（色相 0.25）圆上 = 彩度 1
        let geom = WheelGeometry::for_size(100.0, 100.0, 2.0);
        let col = hcy_wheel_color_at_position(&geom, 50.0, 50.0 + 48.5, &managed, None).unwrap();
        assert_eq!((col.h, col.c, col.y), (0.25, 1.0, 0.6));
        // 盘心：彩度 0，色相由 `hue_from_angle(0,0)` = 0.25 决定
        let center = hcy_wheel_color_at_position(&geom, 50.0, 50.0, &managed, None).unwrap();
        assert_eq!((center.c, center.h), (0.0, 0.25));
        // 标记位置：色相 0.25、彩度 1 → 正下方圆边
        let (x, y) =
            hcy_wheel_position_for_color(&geom, &Color::Hcy(HcyColor::new(0.25, 1.0, 0.6)), None);
        assert_eq!((x, y), (50.5, (50.0f64 + 48.5).trunc() + 0.5));
        // 彩度 > 1 的标记夹到圆边（`get_pos_for_color` 的 clamp，:1311）：
        // 色相 0 = 正右方，x 落在 radius 处而不是 2 * radius
        let (x, _y) =
            hcy_wheel_position_for_color(&geom, &Color::Hcy(HcyColor::new(0.0, 2.0, 0.6)), None);
        assert_eq!(x, (50.0f64 + 48.5).trunc() + 0.5);
    }

    /// 盘面用的边缘色与盘心色 —— `adjbases.py:1173` + `:1208-1209`。
    #[test]
    fn hcy_disc_colors_are_equiluminant() {
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        // 盘心 = 明度的灰
        assert_eq!(hcy_wheel_center_rgb(&managed), [0.5, 0.5, 0.5]);
        // 边缘色：HCY(h, 1, 0.5) 的 RGB 和 = 明度权重和 = y
        for h in [0.0, 0.1, 0.25, 0.5, 0.75, 0.95] {
            let rgb = hcy_wheel_slice_rgb(&managed, h);
            let luma = 0.3 * rgb[0] + 0.59 * rgb[1] + 0.11 * rgb[2];
            assert!(
                close(luma, 0.5, 1e-12),
                "色相 {h} 的 rim 明度应恒为 0.5，实得 {luma}"
            );
        }
    }

    /// 滚轮只改 HCY 明度 —— `hcywheel.py:356-367`。
    #[test]
    fn hcy_wheel_scroll_changes_luma_only() {
        let managed = Color::Hcy(HcyColor::new(0.3, 0.5, 0.5));
        let Some(Color::Hcy(col)) = hcy_wheel_scroll(&managed, ScrollDirection::Up) else {
            panic!("向上滚应改明度");
        };
        assert_eq!((col.h, col.c), (0.3, 0.5));
        assert_eq!(col.y, 0.5 + SCROLL_DELTA);
        // DOWN / LEFT 取反
        let Some(Color::Hcy(col)) = hcy_wheel_scroll(&managed, ScrollDirection::Down) else {
            panic!()
        };
        assert_eq!(col.y, 0.5 - SCROLL_DELTA);
        // 已经在顶/底 → 不改（返回 None，源码的 if 不成立）
        let full = Color::Hcy(HcyColor::new(0.3, 0.5, 1.0));
        assert_eq!(hcy_wheel_scroll(&full, ScrollDirection::Up), None);
        let empty = Color::Hcy(HcyColor::new(0.3, 0.5, 0.0));
        assert_eq!(hcy_wheel_scroll(&empty, ScrollDirection::Down), None);
    }

    /// 明度滑条的采样段数 —— `sliders.py:287-292`。
    #[test]
    fn luma_slider_samples_follow_source() {
        // 竖条 100 高、边框 2 → 96 / 3 = 32
        assert_eq!(hcy_luma_slider_samples(18.0, 100.0, true, 2.0), 32);
        // 299 高 → 295 / 3 = 98.33 → floor 98 → min(98, 64) = 64
        assert_eq!(hcy_luma_slider_samples(18.0, 299.0, true, 2.0), 64);
        // 正好 194 高 → 190 / 3 = 63.33 → 63（还没到上限）
        assert_eq!(hcy_luma_slider_samples(18.0, 194.0, true, 2.0), 63);
        // 扣完边框是负数 → 负数（源码不设防，调用方夹取）
        assert_eq!(hcy_luma_slider_samples(18.0, 6.0, true, 2.0), 0);
        assert_eq!(hcy_luma_slider_samples(18.0, 3.0, true, 2.0), -1);
        // `vertical and height or width` 的假值回落：竖条但高度 0 → 用宽度
        assert_eq!(hcy_luma_slider_samples(18.0, 0.0, true, 2.0), 4);
        // 横条（vertical 假）永远用宽度
        assert_eq!(hcy_luma_slider_samples(300.0, 200.0, false, 2.0), 64);
    }

    /// 编辑器明度夹取 —— `hcywheel.py:811-822`。
    #[test]
    fn editor_luma_clamps_to_quarter_and_three_quarters() {
        assert_eq!(editor_luma_y(0.0), 0.25);
        assert_eq!(editor_luma_y(0.5), 0.5);
        assert_eq!(editor_luma_y(1.0), 0.75);
        assert_eq!(editor_luma_y(0.75), 0.75);
    }

    /// 五个预设：名字、形状数、点数 —— `hcywheel.py:902-987`。
    ///
    /// 具体浮点值的零容差断言在 `tests/wheel_vectors.rs`（`hcy_template_*`），
    /// 这里只钉结构与几个"人眼可查"的值。
    #[test]
    fn mask_templates_match_source_structure() {
        let t = mask_templates();
        let names: Vec<&str> = t.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "Atmospheric Triad",
                "Shifted Triad",
                "Complementary",
                "Mood and Accent",
                "Split Complementary",
            ]
        );
        let shapes: Vec<usize> = t.iter().map(|t| t.shapes.len()).collect();
        assert_eq!(shapes, [1, 1, 1, 2, 3]);
        let points: Vec<Vec<usize>> = t
            .iter()
            .map(|t| t.shapes.iter().map(|s| s.len()).collect())
            .collect();
        assert_eq!(
            points,
            vec![vec![3], vec![3], vec![6], vec![3, 5], vec![6, 6, 6]]
        );
        // 所有点都在 y = 0.5 这一层（`Y = 0.5`，:869）
        for tpl in &t {
            for shape in &tpl.shapes {
                for p in shape {
                    assert_eq!(p[2], 0.5, "{} 的明度应恒为 Y", tpl.name);
                }
            }
        }
        // 主色相 H = 0.95（:870）
        assert_eq!(t[1].shapes[0][0][0], 0.95);
        // atmos_triad 的两个补色：(H+0.275)%1 与 (1+H-0.275)%1
        assert_eq!(t[0].shapes[0][1][0], (0.95f64 + 0.275) % 1.0);
        assert_eq!(t[0].shapes[0][2][0], (1.0f64 + 0.95 - 0.275) % 1.0);
        // "Mood and Accent" 的第二个形状是 H+0.5 处的五边形色块
        assert_eq!(t[3].shapes[1].len(), 5);
    }

    /// 模板 → 遮罩：色相过逆畸变，彩度/明度原样 —— `hcywheel.py:1008-1015`。
    #[test]
    fn template_mask_applies_undistort_only_to_hue() {
        let t = mask_templates();
        let rgb = template_mask(&t[0], None);
        assert_eq!(rgb.len(), 1);
        assert_eq!(rgb[0].len(), 3);
        let Color::Hcy(col) = rgb[0][1] else {
            panic!("模板颜色应是 HCYColor")
        };
        assert_eq!(col.h, (0.95f64 + 0.275) % 1.0);
        assert_eq!(col.c, 0.55);
        assert_eq!(col.y, 0.5);

        // rygb 轮：色相被逆畸变改掉，彩度/明度不动
        let rygb = crate::color::WheelType::Rygb.ranges();
        let hcy = template_mask(&t[0], rygb);
        let Color::Hcy(col) = hcy[0][0] else {
            panic!()
        };
        assert_eq!(col.h, undistort_hue(0.95, rygb));
        assert_ne!(col.h, 0.95, "rygb 轮上 0.95 应被重映射");
        assert_eq!((col.c, col.y), (0.95, 0.5));
    }

    /// 遮罩 void = 各颜色位置的凸包；点数 < 3 的形状不参与
    /// （`hcywheel.py:180-204`）。
    #[test]
    fn mask_voids_are_convex_hulls_of_positions() {
        let geom = WheelGeometry::for_size(200.0, 200.0, 2.0);
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        let t = mask_templates();
        let mask = template_mask(&t[0], None);
        let voids = mask_voids(&mask, &geom, None);
        assert_eq!(voids.len(), 1);
        // 三个点，都不同 → 凸包应该也是三个点
        assert_eq!(voids[0].len(), 3);
        // 凸包会重排点，所以按集合比：每个 void 点都等于某个颜色的位置
        let direct: Vec<Point> = mask[0]
            .iter()
            .map(|c| hcy_wheel_position_for_color(&geom, c, None))
            .collect();
        for void_pt in &voids[0] {
            assert!(direct.contains(void_pt), "{void_pt:?} 不在颜色位置表里");
        }
        // 只有两个颜色 → 空表
        let two = vec![mask[0][0], mask[0][1]];
        assert!(colors_to_mask_void(&two, &geom, None).is_empty());
        // 整块遮罩里点数不足的形状被跳过
        let mixed = vec![two, mask[0].clone()];
        assert_eq!(mask_voids(&mixed, &geom, None).len(), 1);
        let _ = managed;
    }

    /// 遮罩的尺寸过滤 —— `hcywheel.py:288-295`。
    #[test]
    fn visible_mask_voids_drop_tiny_shapes() {
        let geom = WheelGeometry::for_size(200.0, 200.0, 2.0);
        // 半径 98.5 → 最小尺寸 14.775
        let min_size = geom.radius * MIN_SHAPE_SIZE;
        assert!(close(min_size, 14.775, 1e-12));
        let t = mask_templates();
        // Atmospheric Triad 的三个点离得挺开 → 保留
        let big = template_mask(&t[0], None);
        assert_eq!(visible_mask_voids(&big, &geom, None, geom.radius).len(), 1);
        // 三个颜色全挤在盘心附近 → 尺寸太小 → 丢掉
        let tiny: Vec<MaskShape> = vec![vec![
            Color::Hcy(HcyColor::new(0.0, 0.0, 0.5)),
            Color::Hcy(HcyColor::new(0.01, 0.0, 0.5)),
            Color::Hcy(HcyColor::new(0.02, 0.001, 0.5)),
        ]];
        assert_eq!(mask_voids(&tiny, &geom, None).len(), 1, "void 本身存在");
        assert_eq!(
            visible_mask_voids(&tiny, &geom, None, geom.radius).len(),
            0,
            "尺寸不够 → 不画"
        );
    }

    /// 取色裁剪：void 内原样、void 外映射到最近边/顶点 ——
    /// `hcywheel.py:206-238`。
    ///
    /// 用一个**手推的**三角形：200×200、边框 2 的轮子半径 98.5、圆心 (100,100)；
    /// `get_pos_for_color()` 最后是 `int(...) + 0.5`（`adjbases.py:1313-1314`），
    /// 所以圆心附近的坐标都带半像素。取三个彩度 1 的颜色，色相 0 / 0.25 / 0.5
    /// → 半径比 1、角度 0 / π/2 / π，位置分别是
    /// (198.5, 100.5)、(100.5, 198.5)、(1.5, 100.5)，凸包就是这三个点组成的
    /// 朝下三角形。
    #[test]
    fn masked_sampling_remaps_outside_points_to_nearest_edge() {
        let geom = WheelGeometry::for_size(200.0, 200.0, 2.0);
        assert_eq!(
            (geom.center_x, geom.center_y, geom.radius),
            (100.0, 100.0, 98.5)
        );
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.6));
        let y = 0.6;
        let mask: Vec<MaskShape> = vec![vec![
            Color::Hcy(HcyColor::new(0.0, 1.0, y)),
            Color::Hcy(HcyColor::new(0.25, 1.0, y)),
            Color::Hcy(HcyColor::new(0.5, 1.0, y)),
        ]];
        let void = &mask_voids(&mask, &geom, None)[0];
        assert_eq!(void.len(), 3);
        assert!(void.contains(&(198.5, 100.5)), "{void:?}");
        assert!(void.contains(&(100.5, 198.5)), "{void:?}");
        assert!(void.contains(&(1.5, 100.5)), "{void:?}");

        // 内部点 (100,150)：原样取色（与该位置直接算的逐位一致）
        let inside = masked_color_at_position(&mask, &geom, 100.0, 150.0, &managed, None).unwrap();
        assert_eq!(
            inside,
            hcy_wheel_color_at_position(&geom, 100.0, 150.0, &managed, None).unwrap()
        );

        // 外部点 (100,50)：三组候选里最近的是上边（y = 100.5）上的垂足
        // (100, 100.5)，距离 50.5；另两条边的垂足都在 100 以外。
        // 所以结果必须等于"在 (100,100.5) 处直接取色"。
        let out = masked_color_at_position(&mask, &geom, 100.0, 50.0, &managed, None).unwrap();
        assert_eq!(
            out,
            hcy_wheel_color_at_position(&geom, 100.0, 100.5, &managed, None).unwrap()
        );

        // 顶点也是候选（`nearest_point_in_segment` 不含端点，源码因此在每个
        // pair 里额外塞 p1，`:231-233`）：点 (0,100) 在三角形左外侧，三条边的
        // 垂足都不在段内，最近的候选是顶点 (1.5,100.5)。
        let out = masked_color_at_position(&mask, &geom, 0.0, 100.0, &managed, None).unwrap();
        let expect = hcy_wheel_color_at_position(&geom, 1.5, 100.5, &managed, None).unwrap();
        assert_eq!(out, expect);
    }

    /// 空遮罩 = 不裁剪（源码 `if not voids: return 原样`，`:218-219`）。
    #[test]
    fn empty_mask_disables_clipping() {
        let geom = WheelGeometry::for_size(100.0, 100.0, 2.0);
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        let plain = hcy_wheel_color_at_position(&geom, 10.0, 10.0, &managed, None).unwrap();
        let masked = masked_color_at_position(&[], &geom, 10.0, 10.0, &managed, None).unwrap();
        assert_eq!(plain, masked);
    }
}
