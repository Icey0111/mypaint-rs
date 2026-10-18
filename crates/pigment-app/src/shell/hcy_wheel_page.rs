//! HCY 色轮页的控件层 —— `gui/colors/hcywheel.py`（B4-4c）。
//!
//! ## 源码里这一页是什么
//!
//! `HCYAdjusterPage`（`hcywheel.py:1268-1334`）是一个 `CombinedAdjusterPage`：
//!
//! ```python
//! y_adj = HCYLumaSlider();  y_adj.vertical = True
//! hc_adj = HCYHueChromaWheel()
//! table = Gtk.Table(rows=2, columns=2)
//! table.attach(y_adj,  0, 1, 0, 1, FILL,       yopts, 3, 3)   # 左上：明度滑条
//! table.attach(hc_adj, 1, 2, 0, 2, xopts,      yopts, 3, 3)   # 右列跨两行：色轮
//! frame = Gtk.AspectFrame(obey_child=True); frame.add(table)
//! ```
//!
//! 也就是说：**左上是竖直的 HCY 明度滑条，右下空着，右列整列是色轮**。轮子
//! 本体是 `adjbases.py:1169-1285` 的 `HueSaturationWheelMixin.render_background_cb()`，
//! 与 HSV 轮**同一段绘制代码**，差别只有三个颜色钩子（`hcywheel.py:328-336`）——
//! 所以这里复用 [`crate::shell::wheel_pages::paint_wheel_disc`] 的
//! `WheelStyle::Hcy` 分支，不另写一份。
//!
//! 轮子画的就是"极坐标圆盘"：半径 = HCY 彩度（`c`）、角度 = 色相（`h`）、
//! 中心一层"当前明度的灰"的径向去饱和。**没有三角/六边形布局** —— 任务里
//! 说的"多边形"是色域遮罩（gamut mask），见下。
//!
//! ## 色域遮罩：画什么、怎么裁剪
//!
//! 每个遮罩形状是一串 `HCYColor`（`MaskableWheelMixin.__mask`，
//! `hcywheel.py:70/76`）。它在这页上有两个作用：
//!
//! 1. **取色裁剪**（`hcywheel.py:206-238`）：遮罩生效时，点在 void 内按原样
//!    取色，点在 void 外则**映射到最近一个 void 的最近边/顶点**再取色 ——
//!    这就是"把不可达区域裁掉"的实质。数学在引擎
//!    [`pigment_engine::color::masked_color_at_position`]，这里只负责调用。
//! 2. **画灰提示**（`hcywheel.py:260-312`）：整块盘面盖一层 **主题背景色**
//!    （alpha 0.666），再把各 void 的内部挖空、边界描一圈前景色。
//!
//! 第 2 点的 Cairo 实现用的是 `push_group` + `OPERATOR_SOURCE` 填全透明来
//! "挖洞"（`:283-307`）。egui 没有混合运算，这里的等价做法是：盖灰之后
//! **按 void 内部每个点重新算一遍盘面颜色**（盘面颜色本来就是位置的纯函数，
//! 见 [`hcy_wheel_visible_rgb`]），于是洞里看到的就是没被灰盖住的盘面。
//! 描边仍然画在盖灰之后、挖洞之前 —— 与源码的合成顺序一致（Cairo 用
//! `OPERATOR_SOURCE` 只清掉描边靠内的半个像素）。
//!
//! ## 布局换算（GTK 的两轮协商 → 每帧一次纯函数）
//!
//! [`hcy_page_rects`] 把 `Gtk.Table` 的分配规则翻成算术：
//! - 两列：左列 = 滑条的宽度申请（`uimisc.py:25` 的 18），右列吃掉剩下的；
//! - 两行：滑条所在的行有 50px 的高度申请（`uimisc.py:26` 的
//!   `SLIDER_MIN_LENGTH`，`adjbases.py:961-968` 的 `set_size_request`），
//!   另一行没有子控件（natural = 0）；
//! - 色轮跨两行且 `vexpand`，`Gtk.Table` 因此把**两行都**标成可扩展，
//!   多出来的高度**平分**给两行 —— 于是滑条拿到 `50 + extra/2`，剩下的空着。
//!   （这是按 `gtk_table_size_allocate()` 的算法推的，不是实测：跨行的
//!   子控件会给每个被跨的行置 `expand`。源码没有写死行高比例。）
//!
//! ## 与源码的差异（逐条记账）
//!
//! 1. **遮罩编辑对话框**（`HCYMaskEditorWheel` + `HCYMaskPropertiesDialog`，
//!    `hcywheel.py:370-822` / `:1055-1265`）：源码从工具页签的"属性"菜单弹一个
//!    GTK 模态对话框。本页把它压成一行页内控件 + 轮上的编辑模式：
//!    拖顶点移动、Shift+点击加顶点、右键删顶点、清空、套模板都已实现；
//!    拖轮缘整体旋转、从 GIMP 调色板读写、形状数上限/明度夹取仍未做。
//! 2. **`Gtk.AspectFrame` 的正方约束**：egui 没有 AspectFrame，用
//!    [`crate::shell::wheel_pages::page_size`] 的 `square = true` 近似（与
//!    HSV 轮页同一处置）。
//! 3. **主题色**：`_get_mask_fg()` / `_get_mask_bg()`（`:246-258`）读 GTK 主题的
//!    前景/背景色，这里用 `ui.visuals()` 的 `text_color` / `panel_fill`。
//! 4. **`get_prefs()` 持久化**（`:43-44` 的两个偏好键
//!    `colors.hcywheel.mask.gamuts` / `...mask.active`）：偏好系统已由 B6-1
//!    提供，但本页的读写未接（记账），遮罩只在本次运行内有效（键名在引擎里保留）。

use eframe::egui;
use pigment_engine::color::{
    distance_fade_stops, hcy_luma_slider_samples, hcy_wheel_center_rgb,
    hcy_wheel_color_at_position, hcy_wheel_position_for_color, hcy_wheel_scroll, mask_templates,
    masked_color_at_position, template_mask, visible_mask_voids, Color, ColorChanger,
    ComponentSlider, HueRange, MaskShape, Point, SliderKind, UIColor,
    WheelGeometry, BORDER_WIDTH, HCY_WHEEL_TOOLTIP, MASK_ACTIVE_LABEL, MASK_ACTIVE_TOOLTIP,
    MASK_DRAW_ALPHA, MASK_EDGE_LINE_WIDTH, SAT_GAMMA, SAT_SLICES, SLIDER_MIN_LENGTH,
    SLIDER_MIN_WIDTH,
};

use crate::app::PigmentApp;
use crate::shell::color_panel::rgb32;
use crate::shell::wheel_pages::{
    drive_changer, page_size, paint_marker_circle, paint_vertical_slider, paint_wheel_disc,
    scroll_direction, PointerFrame, WheelStyle, DRAG_THRESHOLD, PAGE_PADDING,
};

// ---------------------------------------------------------------------------
// 常量（逐条对应源码行）
// ---------------------------------------------------------------------------

/// 明度滑条的悬停提示 —— `sliders.py:281-285` 的 `HCYLumaSlider.STATIC_TOOLTIP_TEXT`：
///
/// ```python
/// STATIC_TOOLTIP_TEXT = C_(
///     "color component slider: tooltip",
///     "HCY Luma (Y')",
/// )
/// ```
pub(crate) const LUMA_SLIDER_TOOLTIP: &str = "HCY Luma (Y')";

/// 遮罩模板下拉在未选过模板时的文字 —— `HCYMaskTemplateDialog` 的窗口标题
/// （`hcywheel.py:991-995`）："New Gamut Mask from Template"。
pub(crate) const MASK_TEMPLATE_LABEL: &str = "New Gamut Mask from Template";

/// "清空遮罩"按钮的文字 —— `HCYMaskPropertiesDialog` 的 `Gtk.STOCK_CLEAR`
/// 按钮提示（`hcywheel.py:1134-1136`：`C_("...", "Erase the mask.")`）。
pub(crate) const MASK_CLEAR_TOOLTIP: &str = "Erase the mask.";

/// 挖洞补画的三角网细分目标边长（像素）。
///
/// 源码用 Cairo 的 `OPERATOR_SOURCE` 直接把洞挖成透明，不需要这个数；egui
/// 侧要把洞里的盘面重新算一遍，细分越密越接近原样。4px 时一个 100px 见方的
/// 洞大约 600 个三角形，肉眼看不出接缝。
const VOID_PATCH_EDGE: f32 = 4.0;

/// 细分递归的深度上限（防止极端小的洞无限细分）。
const VOID_PATCH_MAX_DEPTH: u32 = 6;

// ---------------------------------------------------------------------------
// 布局（纯函数，可脱离窗口测试）
// ---------------------------------------------------------------------------

/// HCY 页的两块 —— `hcywheel.py:1281-1285` 的 `Gtk.Table(rows=2, columns=2)`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HcyPageRects {
    /// 左上：竖直的 HCY 明度滑条（`attach(y_adj, 0, 1, 0, 1, ...)`）。
    pub luma: egui::Rect,
    /// 右列跨两行：色轮（`attach(hc_adj, 1, 2, 0, 2, ...)`）。
    pub wheel: egui::Rect,
}

/// HCY 页的分配 —— 见模块文档"布局换算"。
///
/// 三个数值来源：
/// - [`PAGE_PADDING`] = 3：`attach(..., 3, 3)` 的上下左右 padding；
/// - [`SLIDER_MIN_WIDTH`] = 18：滑条的宽度申请（`uimisc.py:25`）；
/// - [`SLIDER_MIN_LENGTH`] = 50：滑条的高度申请（`uimisc.py:26`），也就是
///   滑条那一行的 natural 高度。
///
/// 分给两行的"额外高度" = `inner_h - 行间距 - natural0`；色轮跨行且 vexpand
/// 会把**两行**都标成可扩展（`gtk_table_attach` 对每一个被跨的行都置
/// `row->expand`），所以两行平分，滑条拿到 `natural0 + extra/2`。
pub(crate) fn hcy_page_rects(rect: egui::Rect) -> HcyPageRects {
    let pad = PAGE_PADDING;
    let inner_h = (rect.height() - 2.0 * pad).max(0.0);
    let inner_w = (rect.width() - 2.0 * pad).max(0.0);
    // 行间距 = 上一行的下 padding + 下一行的上 padding
    let row_gap = 2.0 * pad;
    let natural0 = SLIDER_MIN_LENGTH as f32;
    let extra = (inner_h - row_gap - natural0).max(0.0);
    let luma_h = natural0 + extra / 2.0;
    let left_w = SLIDER_MIN_WIDTH as f32;
    let luma = egui::Rect::from_min_size(
        rect.min + egui::vec2(pad, pad),
        egui::vec2(left_w, luma_h.min(inner_h)),
    );
    // 两列之间同样空 6px（左格右 padding 3 + 右格左 padding 3）
    let wheel_left = rect.left() + pad + left_w + row_gap;
    let wheel = egui::Rect::from_min_size(
        egui::pos2(wheel_left, rect.top() + pad),
        egui::vec2((rect.left() + pad + inner_w - wheel_left).max(0.0), inner_h),
    );
    HcyPageRects { luma, wheel }
}

// ---------------------------------------------------------------------------
// 盘面颜色的重算（挖洞补画用）
// ---------------------------------------------------------------------------

/// 径向去饱和渐变在半径比 `p` 处的 alpha —— `adjbases.py:1233-1240` +
/// `util.py:27-40`。
///
/// 源码是一个 `cairo.RadialGradient(0,0,0, 0,0,radius)`，色标由
/// [`distance_fade_stops`] 给出（`SAT_SLICES` 段、`gamma = 1/SAT_GAMMA`、
/// alpha 从 1 衰减到 0）。Cairo 在相邻色标之间**线性**插值 alpha，所以这里
/// 也照做 —— 不复制那段几何，只把 alpha 取出来用。
fn fade_alpha(p: f64) -> f64 {
    let stops = distance_fade_stops([0.0, 0.0, 0.0], SAT_SLICES, 1.0 / SAT_GAMMA, 1.0);
    if p <= 0.0 {
        return stops[0].1[3];
    }
    for pair in stops.windows(2) {
        let (p0, a0) = (pair[0].0, pair[0].1[3]);
        let (p1, a1) = (pair[1].0, pair[1].1[3]);
        if p <= p1 {
            if p1 <= p0 {
                return a1;
            }
            return a0 + (a1 - a0) * (p - p0) / (p1 - p0);
        }
    }
    stops.last().map(|s| s.1[3]).unwrap_or(0.0)
}

/// 盘面某点最终显示的颜色 —— `render_background_cb()` 的两层叠加：
///
/// 1. 扇区色：`color_at_normalized_polar_pos(1.0, h)` 画出的扇区，在 (x, y)
///    处就是 `color_at_normalized_polar_pos(r, theta)`（`adjbases.py:1208`）；
/// 2. 中心去饱和：`RadialGradient` 用盘心灰
///    `color_at_normalized_polar_pos(0, 0)`（`:1173`）按 `fade_alpha(r)` 盖在上面
///    （`:1233-1240`）。
///
/// 合成是 `grey * a + slice * (1 - a)`（Cairo 的 source-over，两层都不透明）。
/// 挖洞时用它把"本来会显示在那里的盘面"算出来。
fn hcy_wheel_visible_rgb(
    geom: &WheelGeometry,
    x: f64,
    y: f64,
    managed: &Color,
    ranges: Option<&[HueRange]>,
) -> [f64; 3] {
    let Some(slice) = hcy_wheel_color_at_position(geom, x, y, managed, ranges) else {
        return managed.get_rgb();
    };
    let rgb = slice.get_rgb();
    let grey = hcy_wheel_center_rgb(managed);
    let dx = x - geom.center_x;
    let dy = y - geom.center_y;
    let rr = (dx * dx + dy * dy).sqrt();
    let p = if geom.radius > 0.0 {
        (rr / geom.radius).min(1.0)
    } else {
        1.0
    };
    let a = fade_alpha(p);
    [
        grey[0] * a + rgb[0] * (1.0 - a),
        grey[1] * a + rgb[1] * (1.0 - a),
        grey[2] * a + rgb[2] * (1.0 - a),
    ]
}

/// 挖洞补画的三角网顶点色 —— 见 [`hcy_wheel_visible_rgb`]。
fn patch_vertex_color(
    geom: &WheelGeometry,
    rect: egui::Rect,
    pos: egui::Pos2,
    managed: &Color,
    ranges: Option<&[HueRange]>,
) -> egui::Color32 {
    rgb32(hcy_wheel_visible_rgb(
        geom,
        f64::from(pos.x - rect.left()),
        f64::from(pos.y - rect.top()),
        managed,
        ranges,
    ))
}

/// 把一个三角形按 [`VOID_PATCH_EDGE`] 递归细分后塞进 `mesh`。
#[allow(clippy::too_many_arguments)]
fn emit_patch_triangle(
    mesh: &mut egui::Mesh,
    geom: &WheelGeometry,
    rect: egui::Rect,
    managed: &Color,
    ranges: Option<&[HueRange]>,
    a: egui::Pos2,
    b: egui::Pos2,
    c: egui::Pos2,
    depth: u32,
) {
    let longest = (a - b).length().max((b - c).length()).max((c - a).length());
    if depth < VOID_PATCH_MAX_DEPTH && longest > VOID_PATCH_EDGE {
        let ab = a + (b - a) * 0.5;
        let bc = b + (c - b) * 0.5;
        let ca = c + (a - c) * 0.5;
        for tri in [
            (a, ab, ca),
            (ab, b, bc),
            (ca, bc, c),
            (ab, bc, ca),
        ] {
            emit_patch_triangle(mesh, geom, rect, managed, ranges, tri.0, tri.1, tri.2, depth + 1);
        }
        return;
    }
    let base = mesh.vertices.len() as u32;
    for pos in [a, b, c] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color: patch_vertex_color(geom, rect, pos, managed, ranges),
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2]);
}

/// 把 void 内部重新算成盘面颜色 —— `draw_mask()` 里 `OPERATOR_SOURCE` 挖洞的
/// 等价物（`hcywheel.py:305-307`）。
///
/// 做法：凸多边形从顶点均值（凸多边形的顶点均值一定在内部）扇形三角化，
/// 每个三角形按 [`VOID_PATCH_EDGE`] 递归细分，顶点色 =
/// [`hcy_wheel_visible_rgb`]。三角形永远落在多边形内，所以不会盖到描边的
/// 外半边。
fn paint_void_patch(
    painter: &egui::Painter,
    rect: egui::Rect,
    void: &[Point],
    managed: &Color,
    ranges: Option<&[HueRange]>,
) {
    if void.len() < 3 {
        return;
    }
    let geom = WheelGeometry::for_size(
        f64::from(rect.width()),
        f64::from(rect.height()),
        BORDER_WIDTH,
    );
    if !geom.is_usable() {
        return;
    }
    let to_screen = |p: &Point| rect.min + egui::vec2(p.0 as f32, p.1 as f32);
    let n = void.len() as f32;
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;
    for p in void {
        cx += p.0 as f32;
        cy += p.1 as f32;
    }
    let centroid = rect.min + egui::vec2(cx / n, cy / n);

    let mut mesh = egui::Mesh::default();
    for i in 0..void.len() {
        let a = to_screen(&void[i]);
        let b = to_screen(&void[(i + 1) % void.len()]);
        emit_patch_triangle(&mut mesh, &geom, rect, managed, ranges, centroid, a, b, 0);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// 画遮罩 —— `MaskableWheelMixin.draw_mask()`（`hcywheel.py:260-312`）。
///
/// 顺序与源码一致（合成语义见模块文档）：
/// 1. 裁剪到 `radius + BORDER_WIDTH`（`:277-278`）；
/// 2. 整块盘面填主题**背景色**（`:283-287`）；
/// 3. 每个 void 描边 [`MASK_EDGE_LINE_WIDTH`] 宽的前景色（`:296-304`）；
/// 4. 每个 void 内部挖空（`:305-307`）。
///
/// 最后整体以 [`MASK_DRAW_ALPHA`] 的 alpha 贴回（`:311`）—— 这里直接把
/// 两层颜色都按该 alpha 画，等价于"先合成到离屏组、再半透明贴回"。
pub(crate) fn paint_hcy_mask(
    painter: &egui::Painter,
    rect: egui::Rect,
    managed: &Color,
    ranges: Option<&[HueRange]>,
    voids: &[Vec<Point>],
    dim: egui::Color32,
    edge: egui::Color32,
) {
    let geom = WheelGeometry::for_size(
        f64::from(rect.width()),
        f64::from(rect.height()),
        BORDER_WIDTH,
    );
    if !geom.is_usable() || voids.is_empty() {
        return;
    }
    let center = egui::pos2(
        rect.left() + geom.center_x as f32,
        rect.top() + geom.center_y as f32,
    );
    let clip_radius = geom.radius as f32 + BORDER_WIDTH as f32;
    let clip = egui::Rect::from_center_size(
        center,
        egui::vec2(2.0 * clip_radius, 2.0 * clip_radius),
    )
    .intersect(painter.clip_rect());
    let painter = painter.with_clip_rect(clip);

    // ---- 盖灰（`:283-287`）----
    painter.circle_filled(center, clip_radius, dim);
    // ---- 描边（`:296-304`）----
    for void in voids {
        let pts: Vec<egui::Pos2> = void
            .iter()
            .map(|p| rect.min + egui::vec2(p.0 as f32, p.1 as f32))
            .collect();
        painter.add(egui::Shape::closed_line(
            pts,
            egui::Stroke::new(MASK_EDGE_LINE_WIDTH as f32, edge),
        ));
    }
    // ---- 挖洞（`:305-307`）----
    for void in voids {
        paint_void_patch(&painter, rect, void, managed, ranges);
    }
}

/// 轮上的标记 —— `HueSaturationWheelMixin.paint_foreground_cb()`
/// （`adjbases.py:1317-1326`）：`draw_marker_circle(..., size=2)`，位置由
/// HCY 的钩子给出（[`hcy_wheel_position_for_color`]）。
pub(crate) fn paint_hcy_marker(
    painter: &egui::Painter,
    rect: egui::Rect,
    managed: &Color,
    ranges: Option<&[HueRange]>,
) {
    let geom = WheelGeometry::for_size(
        f64::from(rect.width()),
        f64::from(rect.height()),
        BORDER_WIDTH,
    );
    if !geom.is_usable() {
        return;
    }
    let (x, y) = hcy_wheel_position_for_color(&geom, managed, ranges);
    let clip = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + geom.center_x as f32,
            rect.top() + geom.center_y as f32,
        ),
        egui::vec2(2.0 * geom.radius as f32, 2.0 * geom.radius as f32),
    )
    .intersect(painter.clip_rect());
    paint_marker_circle(
        &painter.with_clip_rect(clip),
        rect.min + egui::vec2(x as f32, y as f32),
        2.0,
    );
}

/// 半透明版本 —— `paint_with_alpha(0.666)`（`:311`）的等价物。
fn with_alpha(color: egui::Color32, alpha: f64) -> egui::Color32 {
    let a = (alpha.clamp(0.0, 1.0) * f64::from(color.a())).round() as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

// ---------------------------------------------------------------------------
// 页面状态（每个可拖控件一个 `ColorChanger`，对应源码每个控件一个
// `ColorAdjusterWidget`）
// ---------------------------------------------------------------------------

/// HCY 页的可变状态。
///
/// 源码里"当前遮罩"挂在 `MaskableWheelMixin` 实例上（`self.__mask`，
/// `hcywheel.py:70`），"是否生效"挂在 `mask_toggle`（`Gtk.ToggleAction`，
/// `:71`）；egui 立即模式没有控件对象，只能由应用持有。`ColorChanger` 同理
/// （两个控件各一份按下/拖动状态）。
pub(crate) struct HcyWheelState {
    /// HCY 色轮（`HCYHueChromaWheel`，`hcywheel.py:339-367`）。
    pub wheel: ColorChanger,
    /// 明度滑条（`HCYLumaSlider`，`sliders.py:281-305`）。
    pub luma: ColorChanger,
    /// 遮罩形状（`MaskableWheelMixin.__mask`）：形状 → 颜色。
    pub mask: Vec<MaskShape>,
    /// 遮罩是否生效（`mask_toggle.get_active()`）。
    pub mask_active: bool,
    /// 当前套用的模板下标（`None` = 没套过 / 已清空）；只用于下拉框显示。
    pub mask_template: Option<usize>,
    /// 遮罩编辑模式（`HCYMaskPropertiesDialog` 里的编辑轮，本项目做成页内开关）。
    pub mask_edit: bool,
    /// 正在拖的顶点 `(形状下标, 点下标)`。
    pub mask_drag: Option<(usize, usize)>,
}

impl Default for HcyWheelState {
    fn default() -> Self {
        Self::new()
    }
}

impl HcyWheelState {
    pub(crate) fn new() -> Self {
        Self {
            wheel: ColorChanger::new(DRAG_THRESHOLD),
            luma: ColorChanger::new(DRAG_THRESHOLD),
            mask: Vec::new(),
            // `mask_toggle` 的初值来自偏好（`hcywheel.py:108`，默认 False）
            mask_active: false,
            mask_template: None,
            mask_edit: false,
            mask_drag: None,
        }
    }

    /// 套用第 `index` 个 mood 预设 —— `HCYMaskTemplateDialog.__button_clicked_cb`
    /// （`hcywheel.py:1043-1045`）：
    ///
    /// ```python
    /// self.target.set_mask(mask)
    /// self.hide()
    /// ```
    ///
    /// 而 `set_mask()`（`:161-174`）在 `mask is not None` 时会把开关
    /// **置为生效**（`:169`），所以套模板 = 遮罩立刻起作用。
    ///
    /// 返回是否套上了（下标越界返回 false）。
    pub(crate) fn apply_template(
        &mut self,
        index: usize,
        ranges: Option<&'static [HueRange]>,
    ) -> bool {
        let templates = mask_templates();
        let Some(template) = templates.get(index) else {
            return false;
        };
        self.mask = template_mask(template, ranges);
        self.mask_active = true;
        self.mask_template = Some(index);
        true
    }

    /// 清空遮罩 —— `HCYMaskPropertiesDialog.__clear_clicked()`
    /// （`hcywheel.py:1232-1233`）：`self.editor.set_mask([])`。
    ///
    /// 注意源码只清内容、**不动开关**（`set_mask([])` 走的是
    /// `mask is not None` 那条分支，`mask_toggle` 仍然 True）—— 空遮罩
    /// 自然什么也裁不到（`get_mask_voids()` 返回空表，`:183-184`）。
    pub(crate) fn clear_mask(&mut self) {
        self.mask.clear();
        self.mask_template = None;
        self.mask_active = true;
    }

    /// 本帧要画的 void —— `draw_mask()` 的两层过滤（`hcywheel.py:288-295`）。
    pub(crate) fn visible_voids(
        &self,
        geom: &WheelGeometry,
        ranges: Option<&[HueRange]>,
    ) -> Vec<Vec<Point>> {
        visible_mask_voids(&self.mask, geom, ranges, geom.radius)
    }
}

// ---------------------------------------------------------------------------
// 页面
// ---------------------------------------------------------------------------

impl PigmentApp {
    /// **HCY Wheel 页** —— `HCYAdjusterPage`（`hcywheel.py:1268-1334`）的页面体。
    ///
    /// 布局见 [`hcy_page_rects`]。交互：
    /// - 轮上按下/拖动 = 极坐标取色（半径→彩度、角度→色相），遮罩生效时
    ///   走 [`masked_color_at_position`]（`hcywheel.py:206-238`）；
    /// - 轮上滚轮 = 改 HCY 明度（[`hcy_wheel_scroll`]，`:356-367`）；
    /// - 明度滑条 = 通用滑条语义（`adjbases.py:1019-1083`），`SliderKind::HcyY`；
    /// - 页内一行控件 = 遮罩开关 + 模板下拉 + 清空（编辑对话框未移植，记账）。
    pub(crate) fn color_hcy_wheel_page(&mut self, ui: &mut egui::Ui) {
        self.hcy_mask_controls(ui);
        let size = page_size(ui, true);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let parts = hcy_page_rects(rect);
        let managed = self.color_manager.get_color();
        let ranges = self.color_manager.hue_ranges();
        let panel_fill = ui.visuals().panel_fill;
        let painter = ui.painter().with_clip_rect(ui.clip_rect());
        let luma_slider = ComponentSlider::new(SliderKind::HcyY);

        // ---- 绘制（本帧开始时的颜色）----
        paint_wheel_disc(
            &painter,
            parts.wheel,
            &managed,
            ranges,
            WheelStyle::Hcy,
            panel_fill,
        );
        let geom = WheelGeometry::for_size(
            f64::from(parts.wheel.width()),
            f64::from(parts.wheel.height()),
            BORDER_WIDTH,
        );
        if self.hcy_wheel.mask_active && !self.hcy_wheel.mask.is_empty() {
            let voids = self.hcy_wheel.visible_voids(&geom, ranges);
            paint_hcy_mask(
                &painter,
                parts.wheel,
                &managed,
                ranges,
                &voids,
                with_alpha(panel_fill, MASK_DRAW_ALPHA),
                with_alpha(ui.visuals().text_color(), MASK_DRAW_ALPHA),
            );
        }
        if self.hcy_wheel.mask_edit {
            self.hcy_mask_editor(ui, parts.wheel, &geom, ranges, &managed);
        }
        paint_hcy_marker(&painter, parts.wheel, &managed, ranges);

        // 明度滑条：采样段数由控件尺寸决定（`sliders.py:287-292`）
        let samples = hcy_luma_slider_samples(
            f64::from(parts.luma.width()),
            f64::from(parts.luma.height()),
            true,
            BORDER_WIDTH,
        )
        .max(0) as usize;
        let luma_ok = parts.luma.height() > 2.0 * BORDER_WIDTH as f32;
        if luma_ok {
            paint_vertical_slider(
                &painter,
                parts.luma,
                samples,
                |amt| luma_slider.get_color_for_bar_amount(&managed, amt).get_rgb(),
                luma_slider.get_bar_amount_for_color(&managed),
                managed.get_rgb(),
            );
        }

        // ---- 悬停提示（`hcywheel.py:344-347` 与 `sliders.py:282-285`）----
        let _ = ui
            .interact(
                parts.wheel,
                ui.scope_id().with("hcy_wheel_tip"),
                egui::Sense::hover(),
            )
            .on_hover_text(HCY_WHEEL_TOOLTIP);
        if luma_ok {
            let _ = ui
                .interact(
                    parts.luma,
                    ui.scope_id().with("hcy_luma_tip"),
                    egui::Sense::hover(),
                )
                .on_hover_text(LUMA_SLIDER_TOOLTIP);
        }

        // ---- 指针 ----
        //
        // 轮子的取样点要过遮罩：源码的 `get_color_at_position(x, y)` 在
        // `MaskableWheelMixin` 里被覆写（`hcywheel.py:206-238`），开关关掉
        // 或遮罩为空时退回普通取色。
        let frame = PointerFrame::read(ui);
        let mask = &self.hcy_wheel.mask;
        let mask_active = self.hcy_wheel.mask_active && !mask.is_empty();
        let wheel_color = if self.hcy_wheel.mask_edit {
            None
        } else {
            drive_changer(
            &frame,
            &mut self.hcy_wheel.wheel,
            &mut self.color_manager,
            parts.wheel.min,
            (
                f64::from(parts.wheel.width()),
                f64::from(parts.wheel.height()),
            ),
            |p| parts.wheel.contains(p),
            |p| {
                let x = f64::from(p.x - parts.wheel.left());
                let y = f64::from(p.y - parts.wheel.top());
                if mask_active {
                    masked_color_at_position(mask, &geom, x, y, &managed, ranges).map(Color::Hcy)
                } else {
                    hcy_wheel_color_at_position(&geom, x, y, &managed, ranges).map(Color::Hcy)
                }
            },
        )};
        let luma_color = if luma_ok {
            drive_changer(
                &frame,
                &mut self.hcy_wheel.luma,
                &mut self.color_manager,
                parts.luma.min,
                (f64::from(parts.luma.width()), f64::from(parts.luma.height())),
                |p| parts.luma.contains(p),
                |p| {
                    let amt = pigment_engine::color::point_to_amount(
                        f64::from(p.y - parts.luma.top()),
                        f64::from(parts.luma.height()),
                        BORDER_WIDTH,
                        true,
                    );
                    Some(luma_slider.get_color_for_bar_amount(&managed, amt))
                },
            )
        } else {
            None
        };
        if let Some(color) = wheel_color.or(luma_color) {
            self.set_color(color);
        }

        // ---- 滚轮（`hcywheel.py:356-367` / `adjbases.py:1069-1083`）----
        if let Some(direction) = scroll_direction(ui) {
            if ui.rect_contains_pointer(parts.wheel) {
                if let Some(color) = hcy_wheel_scroll(&managed, direction) {
                    self.set_color(color);
                }
            } else if luma_ok && ui.rect_contains_pointer(parts.luma) {
                let color = luma_slider.scroll(&managed, true, direction);
                self.set_color(color);
            }
        }
    }

    /// 遮罩编辑轮的指针交互 —— `HCYMaskEditorWheel`
    /// （`hcywheel.py:370-822`）。
    ///
    /// 源码：拖顶点移动、拖边插入新顶点、拖轮缘整体旋转、Delete 删形状。
    /// 这里实现前两项 + Shift 加顶点 / 右键删顶点；旋转留给后续（记账）。
    fn hcy_mask_editor(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        geom: &WheelGeometry,
        ranges: Option<&[HueRange]>,
        managed: &Color,
    ) {
        let resp = ui.interact(
            rect,
            ui.id().with("hcy_mask_editor_wheel"),
            egui::Sense::click_and_drag(),
        );
        let handle_r = 5.0;
        let grab_r = 11.0;
        let pos_for = |color: &Color| {
            let (x, y) = hcy_wheel_position_for_color(geom, color, ranges);
            rect.min + egui::vec2(x as f32, y as f32)
        };
        let hit_at = |mask: &[MaskShape], p: egui::Pos2| -> Option<(usize, usize)> {
            let mut pts = Vec::new();
            for (si, shape) in mask.iter().enumerate() {
                for (pi, color) in shape.iter().enumerate() {
                    pts.push((si, pi, pos_for(color)));
                }
            }
            nearest_mask_handle(&pts, p, grab_r)
        };

        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.hcy_wheel.mask_drag = hit_at(&self.hcy_wheel.mask, p);
            }
        }
        if resp.dragged() {
            if let (Some((si, pi)), Some(p)) =
                (self.hcy_wheel.mask_drag, resp.interact_pointer_pos())
            {
                let x = f64::from(p.x - rect.left());
                let y = f64::from(p.y - rect.top());
                if let Some(c) = hcy_wheel_color_at_position(geom, x, y, managed, ranges) {
                    if let Some(shape) = self.hcy_wheel.mask.get_mut(si) {
                        if let Some(slot) = shape.get_mut(pi) {
                            *slot = Color::Hcy(c);
                        }
                    }
                }
            }
        }
        if resp.drag_stopped() {
            self.hcy_wheel.mask_drag = None;
        }

        // Shift+点击：在指针处加一个顶点（源码是拖边插入，语义等价）
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let shift = ui.input(|i| i.modifiers.shift);
                if shift && rect.contains(p) {
                    let x = f64::from(p.x - rect.left());
                    let y = f64::from(p.y - rect.top());
                    if let Some(c) = hcy_wheel_color_at_position(geom, x, y, managed, ranges) {
                        if self.hcy_wheel.mask.is_empty() {
                            self.hcy_wheel.mask.push(Vec::new());
                        }
                        let last = self.hcy_wheel.mask.len() - 1;
                        self.hcy_wheel.mask[last].push(Color::Hcy(c));
                    }
                }
            }
        }
        // 右键：删最近的顶点；形状不足 3 点就整组丢掉
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some((si, pi)) = hit_at(&self.hcy_wheel.mask, p) {
                    let shape = &mut self.hcy_wheel.mask[si];
                    if pi < shape.len() {
                        shape.remove(pi);
                    }
                    if shape.len() < 3 {
                        self.hcy_wheel.mask.remove(si);
                    }
                }
            }
        }

        // 顶点手柄
        let painter = ui.painter().with_clip_rect(ui.clip_rect());
        for (si, shape) in self.hcy_wheel.mask.iter().enumerate() {
            for (pi, color) in shape.iter().enumerate() {
                let active = self.hcy_wheel.mask_drag == Some((si, pi));
                let p = pos_for(color);
                painter.circle_filled(
                    p,
                    if active { handle_r + 1.5 } else { handle_r },
                    if active {
                        egui::Color32::from_rgb(255, 220, 80)
                    } else {
                        egui::Color32::from_rgb(240, 240, 240)
                    },
                );
                painter.circle_stroke(
                    p,
                    if active { handle_r + 1.5 } else { handle_r },
                    egui::Stroke::new(1.0, egui::Color32::BLACK),
                );
            }
        }
    }

/// 页内的遮罩控件行 —— `HCYMaskPropertiesDialog` 的两个控件（"Active"
    /// 勾选框 + "New" 模板按钮，`hcywheel.py:1085-1106`）压成一行。
    ///
    /// 源码把它们放在一个 GTK 模态对话框里（从工具页签的"属性"菜单弹出，
    /// `:1299-1304`），egui 的停靠面板没有页签菜单，所以放在页面里。
    fn hcy_mask_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut active = self.hcy_wheel.mask_active;
            if ui
                .checkbox(&mut active, MASK_ACTIVE_LABEL)
                .on_hover_text(MASK_ACTIVE_TOOLTIP)
                .changed()
            {
                self.hcy_wheel.mask_active = active;
            }
            // 遮罩编辑（源码是 `HCYMaskPropertiesDialog` 里的编辑轮，
            // `hcywheel.py:1055-1265`；这里做成页内模式开关）。
            if ui
                .checkbox(&mut self.hcy_wheel.mask_edit, "Edit Mask")
                .on_hover_text("拖动顶点改形状；Shift+点击加顶点；右键删顶点")
                .changed()
                && !self.hcy_wheel.mask_edit
            {
                self.hcy_wheel.mask_drag = None;
            }
            let templates = mask_templates();
            let current = match self.hcy_wheel.mask_template {
                Some(i) => templates.get(i).map(|t| t.name).unwrap_or(MASK_TEMPLATE_LABEL),
                None => MASK_TEMPLATE_LABEL,
            };
            let ranges = self.color_manager.hue_ranges();
            let mut picked = None;
            egui::ComboBox::from_id_salt("hcy_mask_template")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, template) in templates.iter().enumerate() {
                        let selected = self.hcy_wheel.mask_template == Some(i);
                        if ui
                            .selectable_label(selected, template.name)
                            .on_hover_text(template.description)
                            .clicked()
                        {
                            picked = Some(i);
                        }
                    }
                });
            if let Some(index) = picked {
                self.hcy_wheel.apply_template(index, ranges);
            }
            if ui
                .add_enabled(
                    !self.hcy_wheel.mask.is_empty(),
                    egui::Button::new("⌫").small(),
                )
                .on_hover_text(MASK_CLEAR_TOOLTIP)
                .on_disabled_hover_text("遮罩已经是空的")
                .clicked()
            {
                self.hcy_wheel.clear_mask();
            }
        });
    }
}

// ---------------------------------------------------------------------------
// 测试（期望值全部能从上方的源码行号手推；逐位门禁在
// `pigment-engine/tests/wheel_vectors.rs` 的 `hcy_*` 部分）
// ---------------------------------------------------------------------------

/// 在遮罩顶点里找最近的一个（`HCYMaskEditorWheel.__update_active_objects`
/// 的 `__ctrlpoint_grab_radius` 判定，`hcywheel.py:487-505`）。
fn nearest_mask_handle(
    points: &[(usize, usize, egui::Pos2)],
    p: egui::Pos2,
    radius: f32,
) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, f32)> = None;
    for &(si, pi, q) in points {
        let d = q.distance(p);
        if d <= radius && best.map(|b| d < b.2).unwrap_or(true) {
            best = Some((si, pi, d));
        }
    }
    best.map(|b| (b.0, b.1))
}


#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::color::{
        hcy_wheel_slice_rgb, mask_voids, point_in_convex_poly, template_mask, HcyColor,
        WheelType,
    };

    fn rect(w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(w, h))
    }

    #[test]
    fn mask_handle_pick_prefers_the_nearest_within_radius() {
        let pts = [
            (0, 0, egui::pos2(100.0, 100.0)),
            (0, 1, egui::pos2(108.0, 100.0)),
            (1, 0, egui::pos2(200.0, 200.0)),
        ];
        assert_eq!(
            nearest_mask_handle(&pts, egui::pos2(106.0, 101.0), 11.0),
            Some((0, 1))
        );
        assert_eq!(
            nearest_mask_handle(&pts, egui::pos2(101.0, 101.0), 11.0),
            Some((0, 0))
        );
        assert_eq!(nearest_mask_handle(&pts, egui::pos2(150.0, 150.0), 11.0), None);
    }

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    /// HCY 页的表布局 —— `hcywheel.py:1281-1285`。
    ///
    /// 300×200 的分配区、padding 3、行距 6、滑条高度申请 50：
    /// 内高 194 → extra = 194 - 6 - 50 = 138 → 滑条 50 + 69 = 119。
    #[test]
    fn page_rects_follow_gtk_table_allocation() {
        let r = rect(300.0, 200.0);
        let parts = hcy_page_rects(r);
        assert_eq!(parts.luma.left(), r.left() + 3.0);
        assert_eq!(parts.luma.top(), r.top() + 3.0);
        assert_eq!(parts.luma.width(), 18.0, "uimisc.py:25 的宽度申请");
        assert!(
            approx(parts.luma.height(), 119.0, 1e-4),
            "50 + (194-6-50)/2，实得 {}",
            parts.luma.height()
        );
        // 左列 + 两个 3px 的列间 padding = 27
        assert_eq!(parts.wheel.left(), r.left() + 27.0);
        assert_eq!(parts.wheel.width(), 300.0 - 27.0 - 3.0);
        assert_eq!(parts.wheel.height(), 194.0, "跨两行 → 吃满内高");
        // 滑条不会超出内高
        let tall = rect(300.0, 60.0);
        let parts = hcy_page_rects(tall);
        assert!(parts.luma.height() <= 60.0 - 6.0);
        // 退化：不发散、不出现负尺寸
        let tiny = rect(10.0, 10.0);
        let parts = hcy_page_rects(tiny);
        assert!(parts.luma.height() >= 0.0 && parts.wheel.width() >= 0.0);
        assert!(parts.wheel.height() >= 0.0);
    }

    /// 套预设 = 立刻生效 —— `hcywheel.py:1043-1045` + `:161-174`。
    #[test]
    fn applying_a_template_turns_the_mask_on() {
        let mut state = HcyWheelState::new();
        assert!(!state.mask_active, "默认可关（:108）");
        assert!(state.mask.is_empty());
        assert!(state.apply_template(3, None), "Mood and Accent 应可套用");
        assert_eq!(state.mask.len(), 2, "Mood and Accent 有两个形状");
        assert!(state.mask_active, "set_mask 会把开关置为生效（:169）");
        assert_eq!(state.mask_template, Some(3));
        // 越界不改变任何状态
        let before = state.mask.len();
        assert!(!state.apply_template(9, None));
        assert_eq!(state.mask.len(), before);
    }

    /// 清空遮罩只清内容、不动开关 —— `hcywheel.py:1232-1233` +
    /// `set_mask([])` 的分支（`:168-171`）。
    #[test]
    fn clearing_the_mask_keeps_the_toggle_on() {
        let mut state = HcyWheelState::new();
        state.apply_template(0, None);
        state.clear_mask();
        assert!(state.mask.is_empty());
        assert!(state.mask_active, "源码的 set_mask([]) 仍然把开关置 True");
        assert_eq!(state.mask_template, None);
    }

    /// 画出来的 void = 模板的凸包（`hcywheel.py:288-295`）。
    #[test]
    fn visible_voids_come_from_template_geometry() {
        let mut state = HcyWheelState::new();
        state.apply_template(4, None); // Split Complementary：三个 coffin
        let geom = WheelGeometry::for_size(200.0, 200.0, BORDER_WIDTH);
        let got = state.visible_voids(&geom, None);
        assert_eq!(got.len(), 3, "三个形状都够大");
        let expect = mask_voids(&state.mask, &geom, None);
        assert_eq!(got, expect);
        // 每个 void 都是六边形（coffin 的 6 个点不共线）
        for void in &got {
            assert_eq!(void.len(), 6);
        }
    }

    /// 遮罩生效时 void 外的取色被拉到最近边界上 —— `hcywheel.py:206-238`。
    ///
    /// (100, 10) 在 Atmospheric Triad 那个三角之外（三角的三个顶点约在
    /// (190.5,70.5)、(110.5,165.5)、(70.5,41.5)），所以裁剪前后必然不同。
    #[test]
    fn masked_sampling_pulls_outside_points_to_the_mask() {
        let mut state = HcyWheelState::new();
        state.apply_template(0, None); // Atmospheric Triad
        let geom = WheelGeometry::for_size(200.0, 200.0, BORDER_WIDTH);
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        let voids = mask_voids(&state.mask, &geom, None);
        assert!(
            !voids
                .iter()
                .any(|v| point_in_convex_poly((100.0, 10.0), v)),
            "(100, 10) 应在所有 void 之外"
        );
        let plain = hcy_wheel_color_at_position(&geom, 100.0, 10.0, &managed, None).unwrap();
        let masked =
            masked_color_at_position(&state.mask, &geom, 100.0, 10.0, &managed, None).unwrap();
        assert_ne!(plain, masked, "void 外的点应被映射");

        // 映射结果必须落在某个 void 的边界上（源码把候选点钉在最近的垂足或
        // 顶点上，两者都在边界）。颜色 → 像素的反推不可逆（`get_pos_for_color`
        // 有 `int()+0.5` 量化），所以按"离边界的距离 < 2px"判。
        let mapped = {
            let nr = masked.c.powf(1.0 / SAT_GAMMA);
            let rr = geom.radius * nr.clamp(0.0, 1.0);
            let t = masked.h * 2.0 * std::f64::consts::PI;
            (
                (geom.center_x + rr * t.cos()).trunc() + 0.5,
                (geom.center_y + rr * t.sin()).trunc() + 0.5,
            )
        };
        let moved = ((mapped.0 - 100.0).powi(2) + (mapped.1 - 10.0).powi(2)).sqrt();
        assert!(moved > 1.0, "映射点应离开原位置，实得 {moved}");
        let dist = voids
            .iter()
            .map(|v| dist_to_poly_boundary(mapped, v))
            .fold(f64::INFINITY, f64::min);
        assert!(
            dist <= 2.0,
            "映射点 {mapped:?} 应落在某个 void 的边界上，实得离边界 {dist}"
        );
    }

    /// 点到多边形边界的距离（边上的垂足，或顶点）—— 与
    /// `lib/alg.py:162-186` 的候选规则同一个度量。
    fn dist_to_poly_boundary(p: (f64, f64), poly: &[Point]) -> f64 {
        let mut best = f64::INFINITY;
        for (a, b) in pigment_engine::color::pairwise(poly) {
            if let Some(q) = pigment_engine::color::nearest_point_in_segment(a, b, p) {
                best = best.min(((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt());
            }
            best = best.min(((a.0 - p.0).powi(2) + (a.1 - p.1).powi(2)).sqrt());
        }
        best
    }

    /// void 内的点不被裁剪 —— `hcywheel.py:222-224`。
    #[test]
    fn points_inside_a_void_are_not_remapped() {
        let mut state = HcyWheelState::new();
        state.apply_template(0, None);
        let geom = WheelGeometry::for_size(200.0, 200.0, BORDER_WIDTH);
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        let voids = mask_voids(&state.mask, &geom, None);
        let void = &voids[0];
        let n = void.len() as f64;
        let centroid = (
            void.iter().map(|p| p.0).sum::<f64>() / n,
            void.iter().map(|p| p.1).sum::<f64>() / n,
        );
        assert!(point_in_convex_poly(centroid, void), "质心应在凸包内");
        let plain =
            hcy_wheel_color_at_position(&geom, centroid.0, centroid.1, &managed, None).unwrap();
        let masked = masked_color_at_position(
            &state.mask,
            &geom,
            centroid.0,
            centroid.1,
            &managed,
            None,
        )
        .unwrap();
        assert_eq!(plain, masked);
    }

    /// 轮上滚轮只改明度 —— `hcywheel.py:356-367`。
    #[test]
    fn scrolling_the_wheel_moves_luma_only() {
        let managed = Color::Hcy(HcyColor::new(0.4, 0.5, 0.5));
        let Some(Color::Hcy(col)) =
            hcy_wheel_scroll(&managed, pigment_engine::color::ScrollDirection::Up)
        else {
            panic!("向上滚应改明度");
        };
        assert_eq!((col.h, col.c), (0.4, 0.5));
        assert_eq!(col.y, 0.5 + pigment_engine::color::SCROLL_DELTA);
    }

    /// 明度滑条的采样段数跟着控件尺寸走 —— `sliders.py:287-292`。
    #[test]
    fn luma_slider_samples_follow_layout() {
        let r = rect(300.0, 200.0);
        let parts = hcy_page_rects(r);
        let n = hcy_luma_slider_samples(
            f64::from(parts.luma.width()),
            f64::from(parts.luma.height()),
            true,
            BORDER_WIDTH,
        );
        // 119 - 4 = 115 → 115/3 = 38.33 → 38
        assert_eq!(n, 38);
        // 面板被压低到只剩边框 → 负数，页面按 0 处理
        let tiny = rect(30.0, 12.0);
        let parts = hcy_page_rects(tiny);
        let n = hcy_luma_slider_samples(
            f64::from(parts.luma.width()),
            f64::from(parts.luma.height()),
            true,
            BORDER_WIDTH,
        );
        assert!(n <= 0);
    }

    /// 径向渐变的 alpha 表 —— `util.py:27-40`（`SAT_SLICES` + `1/SAT_GAMMA`）。
    #[test]
    fn fade_alpha_matches_fade_stops() {
        assert_eq!(fade_alpha(0.0), 1.0, "盘心全灰");
        assert_eq!(fade_alpha(1.0), 0.0, "盘边不盖灰");
        // 单调不增
        let mut prev = f64::INFINITY;
        for i in 0..=20 {
            let a = fade_alpha(f64::from(i) / 20.0);
            assert!(a <= prev + 1e-15, "alpha 应随半径单调不增");
            prev = a;
        }
        // 与色标表逐段一致（第 1 段：0 → 1/7，alpha 1 → (6/7)^(1/1.5)）
        let stops = distance_fade_stops([0.0; 3], SAT_SLICES, 1.0 / SAT_GAMMA, 1.0);
        let mid = (stops[0].0 + stops[1].0) / 2.0;
        let expect = (stops[0].1[3] + stops[1].1[3]) / 2.0;
        assert!((fade_alpha(mid) - expect).abs() < 1e-15);
    }

    /// 挖洞补画的颜色：盘心是明度灰，边缘是该色相的满彩度色
    /// （`adjbases.py:1173` + `:1208-1209`）。
    #[test]
    fn visible_rgb_blends_slice_and_center_grey() {
        let geom = WheelGeometry::for_size(200.0, 200.0, BORDER_WIDTH);
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        let center = hcy_wheel_visible_rgb(&geom, 100.0, 100.0, &managed, None);
        assert_eq!(center, [0.5, 0.5, 0.5], "盘心 = (y, y, y)");
        // 圆边上没有灰 → 就是扇区色
        let rim = hcy_wheel_visible_rgb(&geom, 198.0, 100.0, &managed, None);
        let slice = hcy_wheel_slice_rgb(&managed, 0.0);
        assert!(
            (rim[0] - slice[0]).abs() < 0.02 && (rim[1] - slice[1]).abs() < 0.02,
            "圆边应接近扇区色：{rim:?} vs {slice:?}"
        );
    }

    /// 半透明等价物 —— `paint_with_alpha(0.666)`（`hcywheel.py:311`）。
    #[test]
    fn with_alpha_rounds_like_the_source_alpha() {
        let c = egui::Color32::from_rgb(0x20, 0x40, 0x60);
        // 不透明时逐位保留
        assert_eq!(
            with_alpha(c, 1.0).to_srgba_unmultiplied(),
            [0x20, 0x40, 0x60, 255]
        );
        // 0.666 × 255 = 169.83 → 170
        let d = with_alpha(c, MASK_DRAW_ALPHA);
        assert_eq!(d.a(), (0.666f64 * 255.0).round() as u8);
        // `Color32` 内部是**预乘** alpha 的，回读会有 ±1 的舍入误差
        let back = d.to_srgba_unmultiplied();
        for (got, want) in back[0..3].iter().zip([0x20u8, 0x40, 0x60].iter()) {
            assert!(
                i32::from(*got) - i32::from(*want) <= 2,
                "预乘回读应在 ±2 内：{back:?}"
            );
        }
        // 夹取
        assert_eq!(with_alpha(c, 2.0).a(), 255);
        assert_eq!(with_alpha(c, -1.0).a(), 0);
    }

    /// 绘制冒烟（不 panic、产出图形）—— 无窗口 `egui::Context` 跑一帧：
    /// 盘面（`WheelStyle::Hcy`）+ 遮罩 + 标记 + 明度滑条。
    #[test]
    fn hcy_painting_produces_shapes_without_a_window() {
        let ctx = egui::Context::default();
        let managed = Color::Hcy(HcyColor::new(0.4, 0.6, 0.5));
        let mut state = HcyWheelState::new();
        state.apply_template(3, None);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 320.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(320.0, 260.0), egui::Sense::hover());
            let parts = hcy_page_rects(r);
            let luma_slider = ComponentSlider::new(SliderKind::HcyY);
            paint_wheel_disc(
                ui.painter(),
                parts.wheel,
                &managed,
                None,
                WheelStyle::Hcy,
                ui.visuals().panel_fill,
            );
            let geom = WheelGeometry::for_size(
                f64::from(parts.wheel.width()),
                f64::from(parts.wheel.height()),
                BORDER_WIDTH,
            );
            let voids = state.visible_voids(&geom, None);
            assert!(!voids.is_empty(), "Mood and Accent 应画出 void");
            paint_hcy_mask(
                ui.painter(),
                parts.wheel,
                &managed,
                None,
                &voids,
                with_alpha(ui.visuals().panel_fill, MASK_DRAW_ALPHA),
                with_alpha(ui.visuals().text_color(), MASK_DRAW_ALPHA),
            );
            paint_hcy_marker(ui.painter(), parts.wheel, &managed, None);
            paint_vertical_slider(
                ui.painter(),
                parts.luma,
                38,
                |amt| luma_slider.get_color_for_bar_amount(&managed, amt).get_rgb(),
                luma_slider.get_bar_amount_for_color(&managed),
                managed.get_rgb(),
            );
        });
        out.textures_delta.clear();
        // 扇区网格 + 径向渐变 + 外框/高光/刻痕 + 盖灰圆 + 描边 + 洞内补画网格
        // + 标记两层 + 滑条渐变/边框/标记三层
        assert!(
            out.shapes.len() >= 15,
            "一帧应产出至少 15 个形状，实得 {}",
            out.shapes.len()
        );
    }

    /// 页签与动作 id —— `color_panel.rs` 的页表与 `resources.xml:1392-1399`。
    #[test]
    fn page_and_action_ids_match_source() {
        use crate::shell::color_panel::{COLOR_PAGES, PAGE_HCY_WHEEL};
        let page = &COLOR_PAGES[PAGE_HCY_WHEEL];
        assert_eq!(page.action, "HCYWheelTool");
        assert_eq!(page.icon_name, "mypaint-tool-hcywheel");
        assert_eq!(page.title, "HCY Wheel");
        assert!(page.ported, "B4-4c 之后这一页应可用");
        let action = crate::shell::actions::by_id("HCYWheelTool").expect("动作表里应有它");
        assert_eq!(action.icon_name, Some("mypaint-tool-hcywheel"));
        assert_eq!(action.label, Some("HCY Wheel"));
        assert!(
            crate::shell::toolbars::icon_png_exists("mypaint-tool-hcywheel"),
            "图标应在 assets/icons/ 里（build.rs 栅格化）"
        );
    }

    /// 遮罩只作用在轮子上：`paint_hcy_mask` 对空 void 表什么都不画。
    #[test]
    fn empty_mask_paints_nothing() {
        let ctx = egui::Context::default();
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        let input = egui::RawInput::default();
        let mut out = ctx.run_ui(input, |ui| {
            paint_hcy_mask(
                ui.painter(),
                rect(200.0, 200.0),
                &managed,
                None,
                &[],
                egui::Color32::GRAY,
                egui::Color32::WHITE,
            );
        });
        out.textures_delta.clear();
        assert_eq!(out.shapes.len(), 0, "空遮罩不应产生任何图形");
    }

    /// 模板遮罩的色相确实过了逆畸变 —— `hcywheel.py:1008-1015`。
    #[test]
    fn template_mask_uses_the_wheel_type() {
        let rgb = template_mask(&mask_templates()[0], WheelType::Rgb.ranges());
        let rygb = template_mask(&mask_templates()[0], WheelType::Rygb.ranges());
        let first = |m: &Vec<MaskShape>| {
            let Color::Hcy(col) = m[0][0] else {
                panic!()
            };
            col
        };
        assert_eq!(first(&rgb).h, 0.95);
        assert_ne!(first(&rygb).h, first(&rgb).h);
        assert_eq!(first(&rygb).c, first(&rgb).c, "彩度不受轮类型影响");
    }
}
