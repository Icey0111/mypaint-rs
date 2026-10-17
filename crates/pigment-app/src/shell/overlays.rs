//! 画布覆盖层 —— `gui/overlays.py` 的 `Overlay` 抽象在这里的**薄层**替身，
//! 加两个具体覆盖层：对称辅助线（`gui/symmetry.py:607-1033`）与画框
//! （`gui/framewindow.py:657-909`）。
//!
//! # 与源码的对应关系
//!
//! 源码的模型是：每个 `TiledDrawWidget` 有一份 `display_overlays` 列表
//! （`tileddrawwidget.py:273`），列表顺序就是绘制顺序，`drawwindow.py:157-158`
//! 依次挂上 `FrameOverlay`、`SymmetryOverlay`；每帧画完画布后逐个
//! `overlay.paint(cr)`（`tileddrawwidget.py:1150-1156`），`cr` 处于**显示坐标**。
//!
//! egui 是立即模式：画布每帧重画、没有"失效矩形 + 排队重绘"这套机制，
//! 覆盖层因此不需要持有任何缓存状态。所以这里用**枚举 + 绘制函数**表达同一
//! 顺序（[`CANVAS_OVERLAYS`]），而不是为一个方法建 trait 对象。
//!
//! 记账（未移植的 `Overlay` 语义）：
//! - `Overlay.paint(cr)` 的多态与 `FadingOverlay` 的淡出动画（`overlays.py:42-116`）；
//! - 覆盖层自己的失效矩形（`symmetry.py:763-780` / `framewindow.py:702-806`）——
//!   egui 每帧全画，不需要；
//! - 编辑模式下的交互件（拖拽手柄、删除按钮、活动轴高亮，`symmetry.py:955-1026`
//!   与 `framewindow.py:837-899`）——对应的两种编辑模式未移植；
//! - `symmetry.line_alpha` 偏好（`symmetry.py:46-47`）由 [`OverlayStyle`] 读入；
//!   编辑模式下的透明度滑条（`symmetry.py:455-456`/`:595-597`）随编辑模式一起
//!   归 B6-3。
//!
//! # 为什么自己算几何
//!
//! 对称轴线是"从对称中心出发、截到视口矩形里的一段"，源码用
//! `lib/alg.py` 的多边形/射线求交算（`symmetry.py:1036-1065`）。本文件把
//! `point_in_convex_poly`（`alg.py:127-159`）、`_intersects`（`:403-410`）、
//! `intersection_of_vectors`（`:413-498`）、`intersection_of_vector_and_poly`
//! （`:326-400`）逐行搬过来 —— 没有它们就还原不出"旋转对称的轴是**射线**
//! （`LineType.DIRECTIONAL`）而不是整条直线"这个细节。

use eframe::egui;
use pigment_engine::mode::brushresize::BrushSizeOverlayState;
use pigment_engine::prefs::{self, Prefs};
use pigment_engine::Document;
use pigment_surface::symmetry::SymmetryType;

use crate::view::View;

// ---- 源码常量 ----

/// 对称轴线宽 —— `gui/style.py:38` `DRAGGABLE_EDGE_WIDTH = 2`。
const DRAGGABLE_EDGE_WIDTH: f32 = 2.0;

/// 轴线透明度的出厂值 —— `gui/symmetry.py:46` `_DEFAULT_ALPHA = 0.333`
/// （偏好键 `symmetry.line_alpha` 缺失时用它，`:648-649`）。
const DEFAULT_SYMMETRY_ALPHA: f32 = 0.333;

/// 虚线参数 —— `gui/symmetry.py:613-616`：
/// `_DASH_LENGTH = 10`、`_DASH_GAP = 7`、
/// `_DASH_PATTERN = [10, 2*7+10]`（cairo 的图案 = 实线 10、空白 24，周期 34）、
/// `_DASH_OFFSET = 5`。
const DASH_LENGTH: f32 = 10.0;
const DASH_GAP: f32 = 7.0;
/// `_DASH_PATTERN = [_DASH_LENGTH, 2 * _DASH_GAP + _DASH_LENGTH]`（`symmetry.py:615`）。
const DASH_PATTERN: [f32; 2] = [DASH_LENGTH, 2.0 * DASH_GAP + DASH_LENGTH];
const DASH_OFFSET: f32 = 5.0;

/// 轴线主色 —— `gui/style.py:75` `EDITABLE_ITEM_COLOR = #ECF0F1`。
const EDITABLE_ITEM_RGB: [f32; 3] = [
    0xEC as f32 / 255.0,
    0xF0 as f32 / 255.0,
    0xF1 as f32 / 255.0,
];

/// `to_contrasting()` 的默认参数 `k` —— `lib/color.py:145`。
const CONTRASTING_K: f32 = 0.333;

/// 画框描边宽度 —— `gui/framewindow.py:665` `OUTLINE_WIDTH = 1`。
const FRAME_OUTLINE_WIDTH: f32 = 1.0;

/// 画框颜色（遮罩 + 边线）的出厂值 —— `gui/userconfig.py:92`
/// `"frame.color_rgba": (0.12, 0.12, 0.12, 0.92)`。
const DEFAULT_FRAME_RGBA: [f32; 4] = [0.12, 0.12, 0.12, 0.92];

// ---- 笔刷尺寸覆盖层（`gui/brushmanip.py:33-58`）----

/// 手柄半径 —— `BrushSizeOverlay.HANDLE_RADIUS = 3`（`brushmanip.py:27`）。
const BRUSH_HANDLE_RADIUS: f32 = 3.0;
/// 外圈颜色出厂值 —— `prefs.get("cursor.freehand.outer_line_color", (0,0,0,1))`。
const DEFAULT_BRUSH_OUTER_RGBA: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// 内圈颜色出厂值 —— `prefs.get("cursor.freehand.inner_line_color", (1,1,1,0.75))`。
const DEFAULT_BRUSH_INNER_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 0.75];
/// 内外圈线宽出厂值 —— `prefs.get(..., 1.25)`（`brushmanip.py:44-45`）。
const DEFAULT_BRUSH_LINE_WIDTH: f32 = 1.25;
/// 内圈内缩出厂值 —— `prefs.get("cursor.freehand.inner_line_inset", 2)`。
const DEFAULT_BRUSH_INSET: f32 = 2.0;

/// 覆盖层的外观偏好（B6-1）。
///
/// 两处都有源码：对称轴线透明度读 `symmetry.line_alpha`（`symmetry.py:46-47`
/// 的软键，默认 `_DEFAULT_ALPHA = 0.333`，读取处 `:648-649`），画框颜色读
/// `frame.color_rgba`（`userconfig.py:92` 的默认键，读取处
/// `framewindow.py:502-505` / `:826`，并把每个通道夹到 `[0, 1]`）。
/// 笔刷尺寸圆圈读 `cursor.freehand.*`（`brushmanip.py:37-56`；这些键只有
/// `thin/medium/thick` 光标预设才写，缺省值就在读取处）。
///
/// egui 是立即模式，每帧现算一次即可（源码在 `SymmetryOverlay.__init__`
/// 里读一次、`FrameOverlay.paint` 里每帧读一次，见 `:648` / `:826`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OverlayStyle {
    /// 对称轴线整体透明度（乘法，`symmetry.py:1024-1025`）。
    pub(crate) symmetry_line_alpha: f32,
    /// 画框颜色 RGBA（已夹到 `[0, 1]`）。
    pub(crate) frame_rgba: [f32; 4],
    /// 笔刷尺寸圆圈外圈颜色（RGBA）。
    pub(crate) brush_outer_rgba: [f32; 4],
    /// 笔刷尺寸圆圈内圈颜色（RGBA）。
    pub(crate) brush_inner_rgba: [f32; 4],
    /// 外圈线宽（显示像素）。
    pub(crate) brush_outer_width: f32,
    /// 内圈线宽（显示像素）。
    pub(crate) brush_inner_width: f32,
    /// 内圈相对半径的内缩量（显示像素）。
    pub(crate) brush_inset: f32,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            symmetry_line_alpha: DEFAULT_SYMMETRY_ALPHA,
            frame_rgba: DEFAULT_FRAME_RGBA,
            brush_outer_rgba: DEFAULT_BRUSH_OUTER_RGBA,
            brush_inner_rgba: DEFAULT_BRUSH_INNER_RGBA,
            brush_outer_width: DEFAULT_BRUSH_LINE_WIDTH,
            brush_inner_width: DEFAULT_BRUSH_LINE_WIDTH,
            brush_inset: DEFAULT_BRUSH_INSET,
        }
    }
}

impl OverlayStyle {
    /// 从用户偏好读（缺失/类型不对就取出厂值，见模块文档的保守原则）。
    pub(crate) fn from_prefs(prefs: &Prefs) -> Self {
        let mut style = Self::default();
        // `symmetry.py:648-649`：`float(prefs.get(key, _DEFAULT_ALPHA))`
        style.symmetry_line_alpha = prefs
            .f64_or(prefs::KEY_SYMMETRY_LINE_ALPHA, DEFAULT_SYMMETRY_ALPHA as f64)
            as f32;
        // `framewindow.py:502-505`：`[min(max(c, 0), 1) for c in color_rgba]`
        if let Some(arr) = prefs.get_array(prefs::KEY_FRAME_COLOR_RGBA) {
            for (i, v) in arr.iter().take(4).enumerate() {
                if let Some(f) = v.as_f64() {
                    style.frame_rgba[i] = (f as f32).clamp(0.0, 1.0);
                }
            }
        }
        // `brushmanip.py:40-56`：颜色/线宽/内缩都从 `cursor.freehand.*` 读，
        // 缺键就用与源码相同的默认值。源码没夹颜色通道，这里照样只做
        // `[0,1]` 的安全夹取（绘制函数的 `color32` 也会夹）。
        style.brush_outer_rgba = read_rgba(
            prefs,
            "cursor.freehand.outer_line_color",
            DEFAULT_BRUSH_OUTER_RGBA,
        );
        style.brush_inner_rgba = read_rgba(
            prefs,
            "cursor.freehand.inner_line_color",
            DEFAULT_BRUSH_INNER_RGBA,
        );
        style.brush_outer_width = prefs.f64_or(
            "cursor.freehand.outer_line_width",
            DEFAULT_BRUSH_LINE_WIDTH as f64,
        ) as f32;
        style.brush_inner_width = prefs.f64_or(
            "cursor.freehand.inner_line_width",
            DEFAULT_BRUSH_LINE_WIDTH as f64,
        ) as f32;
        style.brush_inset =
            prefs.f64_or("cursor.freehand.inner_line_inset", DEFAULT_BRUSH_INSET as f64) as f32;
        style
    }
}

/// 读一个 RGBA 数组（JSON 里是 `[r, g, b, a]`，每项数字）。
fn read_rgba(prefs: &Prefs, key: &str, fallback: [f32; 4]) -> [f32; 4] {
    let mut out = fallback;
    if let Some(arr) = prefs.get_array(key) {
        for (i, v) in arr.iter().take(4).enumerate() {
            if let Some(f) = v.as_f64() {
                out[i] = f as f32;
            }
        }
    }
    out
}

/// 画布覆盖层（顺序 = 绘制顺序）。
///
/// 照 `drawwindow.py:157-158`：先 `FrameOverlay`、后 `SymmetryOverlay`，
/// 所以对称轴线画在画框遮罩之上。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CanvasOverlay {
    /// `GUI/framewindow.py:657` 的 `FrameOverlay`。
    Frame,
    /// `gui/symmetry.py:607` 的 `SymmetryOverlay`。
    Symmetry,
}

/// 全部覆盖层，按源码的挂载顺序。
pub(crate) const CANVAS_OVERLAYS: [CanvasOverlay; 2] =
    [CanvasOverlay::Frame, CanvasOverlay::Symmetry];

/// 把当前该画的覆盖层画到画布上（等价 `tileddrawwidget.py:1153-1156` 的循环）。
///
/// `rect` 是画布在窗口里的矩形（egui 点），`view` 是"画布局部坐标 ↔ 模型坐标"
/// 的变换（见 [`View`]）；两者合起来才等价源码的 `display_to_model`。
/// `style` 是外观偏好（[`OverlayStyle::from_prefs`]）。
pub(crate) fn paint_canvas_overlays(
    painter: &egui::Painter,
    rect: egui::Rect,
    view: &View,
    doc: &Document,
    style: OverlayStyle,
) {
    for overlay in CANVAS_OVERLAYS {
        match overlay {
            CanvasOverlay::Frame => paint_frame(painter, rect, view, doc, style),
            CanvasOverlay::Symmetry => paint_symmetry(painter, rect, view, doc, style),
        }
    }
}

// ---- 画框 ----

/// 画框覆盖层（`gui/framewindow.py:808-845` 的 `FrameOverlay.paint`）。
///
/// 源码画两件事：
/// 1. 用**偶数填充规则**把画框**外面**涂上 `frame.color_rgba`
///    （`:814-830`：外框是视口矩形、内框是画框四角，`FILL_RULE_EVEN_ODD`）；
/// 2. 再把那条内框路径描 1px（`:842-845`，非编辑模式只有这一句）。
fn paint_frame(
    painter: &egui::Painter,
    rect: egui::Rect,
    view: &View,
    doc: &Document,
    style: OverlayStyle,
) {
    // `:811-812`：没启用就什么都不画
    if !doc.get_frame_enabled() {
        return;
    }
    let frame = frame_display_rect(rect, view, doc.get_frame());
    let c = style.frame_rgba;
    let color = color32(c[0], c[1], c[2], c[3]);
    // 画框外遮罩（即使填充规则）
    for mask in outside_rects(rect, frame) {
        if mask.width() > 0.0 && mask.height() > 0.0 {
            painter.rect_filled(mask, 0.0, color);
        }
    }
    // 画框边线
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(FRAME_OUTLINE_WIDTH, color),
        egui::StrokeKind::Middle,
    );
}

/// 画框在**画布局部坐标**里的矩形（源码 `:714-722` 的 `model_to_display` 四角）。
fn frame_display_rect(
    canvas: egui::Rect,
    view: &View,
    frame: (i32, i32, i32, i32),
) -> egui::Rect {
    let (x, y, w, h) = frame;
    let [x0, y0] = view.world_to_screen(x as f32, y as f32);
    let [x1, y1] = view.world_to_screen((x + w) as f32, (y + h) as f32);
    egui::Rect::from_two_pos(
        canvas.min + egui::vec2(x0, y0),
        canvas.min + egui::vec2(x1, y1),
    )
}

/// 画框**之外**（画布之内）的遮罩矩形：上 / 下 / 左 / 右。
///
/// 源码是用一条"外框 + 内框"的偶数填充路径一次涂出来的（`:814-830`）；
/// egui 没有 even-odd 填充，等价地拆成四块矩形。
///
/// 画框完全落在视口外时，even-odd 的结果是"整块画布都被涂"（画布整个在内框
/// 之外），所以那一种情形直接返回画布本身。
fn outside_rects(canvas: egui::Rect, frame: egui::Rect) -> Vec<egui::Rect> {
    let inner = frame.intersect(canvas);
    if inner.width() <= 0.0 || inner.height() <= 0.0 {
        return vec![canvas];
    }
    let (min, max) = (canvas.min, canvas.max);
    vec![
        // 上：从画布顶到内框顶
        egui::Rect::from_min_max(min, egui::pos2(max.x, inner.min.y)),
        // 下：从内框底到画布底
        egui::Rect::from_min_max(egui::pos2(min.x, inner.max.y), max),
        // 左：内框左侧那一条（纵向只取内框的范围）
        egui::Rect::from_min_max(
            egui::pos2(min.x, inner.min.y),
            egui::pos2(inner.min.x, inner.max.y),
        ),
        // 右：内框右侧那一条
        egui::Rect::from_min_max(
            egui::pos2(inner.max.x, inner.min.y),
            egui::pos2(max.x, inner.max.y),
        ),
    ]
}

// ---- 对称辅助线 ----

/// 对称覆盖层（`gui/symmetry.py:951-1026` 的 `SymmetryOverlay.paint` 主线）。
///
/// 源码的绘制分两遍虚线：同一个虚线段各用
/// `_LINE_COLS[0]` / `_LINE_COLS[1]` 描一次，第二遍的 dash 偏移正好落进第一遍
/// 的空隙里（`:1007-1016`），最后整体乘 `line_alpha`（`:1024-1025`）。
fn paint_symmetry(
    painter: &egui::Painter,
    rect: egui::Rect,
    view: &View,
    doc: &Document,
    style: OverlayStyle,
) {
    // `:953`：只在对称开启时画
    if !doc.symmetry_active() {
        return;
    }
    // `:882` / `:990`：透明度为 0（或负）时整层不画
    if style.symmetry_line_alpha <= 0.0 {
        return;
    }
    let (cx, cy) = doc.symmetry_center();
    let corners = view_corners_model(rect, view);
    let axes = viewport_intersections(
        doc.symmetry_type(),
        cx as f32,
        cy as f32,
        doc.symmetry_angle(),
        doc.symmetry_lines(),
        &corners,
    );
    if axes.is_empty() {
        return;
    }
    // `:813-815`：先转显示坐标再取整（cairo 的像素对齐）
    let to_view = |p: (f32, f32)| {
        let [sx, sy] = view.world_to_screen(p.0, p.1);
        rect.min + egui::vec2(sx.round(), sy.round())
    };
    let colors = axis_line_colors(style.symmetry_line_alpha);
    for (offs, color) in colors.iter().enumerate() {
        // `:1009-1011`：第二遍的偏移加了 `DASH_LENGTH + DASH_GAP`（= 半个周期），
        // 于是它的实线段正好落进第一遍的空白里 —— 形成双色虚线。
        let dash_offset = DASH_OFFSET + (DASH_LENGTH + DASH_GAP) * offs as f32;
        let stroke = egui::Stroke::new(DRAGGABLE_EDGE_WIDTH, *color);
        for (_, a, b) in &axes {
            dashed_segment(
                painter,
                to_view(*a),
                to_view(*b),
                DASH_PATTERN[0],
                DASH_PATTERN[1],
                dash_offset,
                stroke,
            );
        }
    }
}

/// 笔刷尺寸圆圈（`gui/brushmanip.py:100-156` 的 `BrushSizeOverlay.paint`）。
///
/// 与其它覆盖层不同，它**不是**常驻在 `display_overlays` 里的：源码只在
/// `BrushResizeMode.drag_start_cb` 里新建、`leave()` 里 `cleanup()`
/// （`:242-253` / `:264-267`）。所以调用方只在模式返回状态时画。
///
/// 几何全部在显示坐标里：中心 = 起点的显示位置，半径 = `visual_radius *
/// scale`（引擎状态里已经是显示像素），手柄 = 中心 + 手柄显示位移。
/// 源码的 `_x/_y` 是整数（`:29-30` 的 `int(round(...))`），这里也取整。
pub(crate) fn paint_brush_size(
    painter: &egui::Painter,
    rect: egui::Rect,
    view: &View,
    state: BrushSizeOverlayState,
    style: OverlayStyle,
) {
    let [sx, sy] = view.world_to_screen(state.center.0, state.center.1);
    let center = rect.min + egui::vec2(sx.round(), sy.round());
    let r0 = state.radius_px.max(0.0);

    // 外圈：`r = r0 - line_width_outer / 2`（`:109-112`）
    let outer = style.brush_outer_width.max(0.0);
    if outer > 0.0 {
        let stroke = egui::Stroke::new(
            outer,
            color32(
                style.brush_outer_rgba[0],
                style.brush_outer_rgba[1],
                style.brush_outer_rgba[2],
                style.brush_outer_rgba[3],
            ),
        );
        painter.circle_stroke(center, (r0 - outer * 0.5).max(0.0), stroke);
    }
    // 内圈：`r = r0 - inset + line_width_inner / 2`（`:114-118`）
    let inner = style.brush_inner_width.max(0.0);
    if inner > 0.0 {
        let stroke = egui::Stroke::new(
            inner,
            color32(
                style.brush_inner_rgba[0],
                style.brush_inner_rgba[1],
                style.brush_inner_rgba[2],
                style.brush_inner_rgba[3],
            ),
        );
        let r = (r0 - style.brush_inset + inner * 0.5).max(0.0);
        painter.circle_stroke(center, r, stroke);
    }

    // 手柄：外圈色实心圆 + 内圈色小一圈（`:121-126`）
    let handle = center + egui::vec2(state.handle.0, state.handle.1);
    painter.circle_filled(
        handle,
        BRUSH_HANDLE_RADIUS,
        color32(
            style.brush_outer_rgba[0],
            style.brush_outer_rgba[1],
            style.brush_outer_rgba[2],
            style.brush_outer_rgba[3],
        ),
    );
    painter.circle_filled(
        handle,
        BRUSH_HANDLE_RADIUS - 1.0,
        color32(
            style.brush_inner_rgba[0],
            style.brush_inner_rgba[1],
            style.brush_inner_rgba[2],
            style.brush_inner_rgba[3],
        ),
    );
}

/// 视口四角在**模型坐标**里的位置 `[TL, TR, BR, BL]`
/// （`tileddrawwidget.py:341-358` 的 `get_corners_model_coords`）。
fn view_corners_model(canvas: egui::Rect, view: &View) -> [(f32, f32); 4] {
    let (w, h) = (canvas.width(), canvas.height());
    let at = |x: f32, y: f32| {
        let [wx, wy] = view.screen_to_world(x, y);
        (wx, wy)
    };
    [at(0.0, 0.0), at(w, 0.0), at(w, h), at(0.0, h)]
}

/// 两条轴线颜色 —— `symmetry.py:610-612`：
/// `_LINE_COL1 = gui.style.EDITABLE_ITEM_COLOR`、`_LINE_COL2 = _LINE_COL1.to_contrasting()`。
///
/// `alpha` 是偏好 `symmetry.line_alpha`（`symmetry.py:989-990` / `:1024-1025`：
/// 两遍虚线都乘同一个 `line_alpha`）。
fn axis_line_colors(alpha: f32) -> [egui::Color32; 2] {
    let c1 = EDITABLE_ITEM_RGB;
    let c2 = contrasting(c1, CONTRASTING_K);
    [
        color32(c1[0], c1[1], c1[2], alpha),
        color32(c2[0], c2[1], c2[2], alpha),
    ]
}

/// `RGBColor.to_contrasting(k)`（`lib/color.py:145-155`）：
/// 用感知加权的 luma 绕一圈取灰度 —— `c = (luma + k) % 1.0`。
///
/// `get_luma` 的权重是 `0.299 r + 0.587 g + 0.114 b`（`lib/color.py:114-128`）。
fn contrasting(rgb: [f32; 3], k: f32) -> [f32; 3] {
    let luma = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
    let c = (luma + k) % 1.0;
    [c, c, c]
}

fn color32(r: f32, g: f32, b: f32, a: f32) -> egui::Color32 {
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgba_unmultiplied(to8(r), to8(g), to8(b), to8(a))
}

/// 一条虚线（cairo `set_dash` 的等价物）。
///
/// 参数与 `cr.set_dash(pattern, offset)` 一致：`dash`/`gap` 是图案长度，
/// `offset` 是图案起点相对线段起点的偏移（cairo 里正值会把图案往前推）。
fn dashed_segment(
    painter: &egui::Painter,
    p0: egui::Pos2,
    p1: egui::Pos2,
    dash: f32,
    gap: f32,
    offset: f32,
    stroke: egui::Stroke,
) {
    let v = p1 - p0;
    let len = v.length();
    if len <= 0.0 || dash <= 0.0 {
        return;
    }
    let dir = v / len;
    for (a, b) in dash_intervals(len, dash, gap, offset) {
        painter.line_segment([p0 + dir * a, p0 + dir * b], stroke);
    }
}

/// 虚线在 `[0, len)` 上的实线段（沿线的参数区间）。
///
/// 图案周期是 `dash + gap`，第一段从 `-offset` 开始（cairo 的约定），
/// 与 `dash` 相交的段才返回。
fn dash_intervals(len: f32, dash: f32, gap: f32, offset: f32) -> Vec<(f32, f32)> {
    let period = dash + gap;
    if period <= 0.0 || len <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut t = -offset.rem_euclid(period);
    while t < len {
        let a = t.max(0.0);
        let b = (t + dash).min(len);
        if b > a {
            out.push((a, b));
        }
        t += period;
    }
    out
}

// ---- 视口求交（`lib/alg.py` 的移植） ----

/// 线段/射线的解释方式 —— `lib/alg.py:16-19` 的 `LineType`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineType {
    /// 双向无限直线（`LINE = 0`）。
    Line,
    /// 从第一点单方向无限（`DIRECTIONAL = 1`）。
    Directional,
    /// 有限线段（`SEGMENT = 2`）。
    Segment,
}

/// `pairwise()`（`lib/alg.py:576-597`）：`(p0,p1) (p1,p2) … (pn,p0)`。
///
/// 与 Python 版的差别只在 `seq` 长度 ≤ 1 时（Python 不产出任何一对）；
/// 这里的调用方都是四边形，不受影响。
fn pairwise(poly: &[(f32, f32)]) -> impl Iterator<Item = ((f32, f32), (f32, f32))> + '_ {
    (0..poly.len()).map(move |i| (poly[i], poly[(i + 1) % poly.len()]))
}

/// `point_in_convex_poly()`（`lib/alg.py:127-159`）：点在凸多边形内（含边界）。
fn point_in_convex_poly(point: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let (x, y) = point;
    let mut seen_left = false;
    let mut seen_right = false;
    for ((x0, y0), (x1, y1)) in pairwise(poly) {
        let det = (y - y0) * (x1 - x0) - (x - x0) * (y1 - y0);
        if det < 0.0 {
            if seen_left {
                return false;
            }
            seen_right = true;
        } else if det > 0.0 {
            if seen_right {
                return false;
            }
            seen_left = true;
        }
    }
    true
}

/// `_intersects()`（`lib/alg.py:403-410`）：参数 `k` 是否落在该类型允许的范围内。
fn line_type_intersects(line_type: LineType, k: f32) -> bool {
    match line_type {
        LineType::Line => true,
        LineType::Directional => k >= 0.0,
        LineType::Segment => (0.0..=1.0).contains(&k),
    }
}

/// `intersection_of_vectors()`（`lib/alg.py:413-498`）：两条参数化直线的交点。
fn intersection_of_vectors(
    a1: (f32, f32),
    a2: (f32, f32),
    b1: (f32, f32),
    b2: (f32, f32),
    a_type: LineType,
    b_type: LineType,
) -> Option<(f32, f32)> {
    let ((x0, y0), (x1, y1)) = (a1, a2);
    let ((x2, y2), (x3, y3)) = (b1, b2);
    let d = (y3 - y2) * (x1 - x0) - (x3 - x2) * (y1 - y0);
    if d == 0.0 {
        return None;
    }
    let u1 = ((x3 - x2) * (y0 - y2) - (y3 - y2) * (x0 - x2)) / d;
    let u2 = ((x1 - x0) * (y0 - y2) - (y1 - y0) * (x0 - x2)) / d;
    if !line_type_intersects(a_type, u1) || !line_type_intersects(b_type, u2) {
        return None;
    }
    Some((u1 * (x1 - x0) + x0, u1 * (y1 - y0) + y0))
}

/// `intersection_of_vector_and_poly()`（`lib/alg.py:326-400`）：
/// 直线/射线/线段与凸多边形相交得到的那一段（否则 `None`）。
fn intersection_of_vector_and_poly(
    poly: &[(f32, f32)],
    p1: (f32, f32),
    p2: (f32, f32),
    line_type: LineType,
) -> Option<((f32, f32), (f32, f32))> {
    let mut result: Vec<(f32, f32)> = Vec::new();
    let mut prev: Option<(f32, f32)> = None;
    for (q1, q2) in pairwise(poly) {
        let inter = intersection_of_vectors(p1, p2, q1, q2, line_type, LineType::Segment);
        if let Some(inter) = inter {
            if Some(inter) != prev {
                result.push(inter);
                prev = Some(inter);
            }
        }
    }
    if result.is_empty() && line_type == LineType::Segment {
        if point_in_convex_poly(p1, poly) && point_in_convex_poly(p2, poly) {
            result = vec![p1, p2];
        }
    } else if result.len() == 1 {
        let inside_p1 = point_in_convex_poly(p1, poly) && Some(p1) != prev;
        match line_type {
            LineType::Directional => {
                if inside_p1 {
                    result.insert(0, p1);
                }
            }
            LineType::Segment => {
                if inside_p1 {
                    result.insert(0, p1);
                } else if point_in_convex_poly(p2, poly) && Some(p2) != prev {
                    result.push(p2);
                }
            }
            LineType::Line => {}
        }
    }
    if result.len() != 2 {
        None
    } else {
        Some((result[0], result[1]))
    }
}

/// 视口内可见的对称轴 —— `gui/symmetry.py:1036-1065` 的
/// `get_viewport_intersections`。
///
/// 返回 `[(下标, 端点0, 端点1), …]`；下标是 `intersections` 列表里**过滤前**的
/// 位置（源码 `enumerate` 后按真假过滤），所以可能是跳号的（`0, 2, 3`）。
///
/// 关键细节：水平/垂直/双向轴用 `LineType::Line`（整条直线），
/// 旋转/雪花用 `LineType::Directional`（从中心出发的**射线**，`symmetry.py:1064`）。
pub(crate) fn viewport_intersections(
    symm_type: SymmetryType,
    x: f32,
    y: f32,
    angle_deg: f32,
    num_lines: i32,
    corners_m: &[(f32, f32)],
) -> Vec<(usize, (f32, f32), (f32, f32))> {
    let mut intersections: Vec<Option<((f32, f32), (f32, f32))>> = Vec::new();
    let p1 = (x, y);
    // `:1047` `angle = math.pi * ((angle % 360) / 180)`
    let angle = std::f32::consts::PI * ((angle_deg % 360.0) / 180.0);

    // `:1049-1054`：`append` 闭包。注意方向是 `(x + cos(a), y - sin(a))`
    // —— 模型坐标 y 向下，所以这里减 sin 才是"屏幕上逆时针"。
    let mut append = |a: f32, line_type: LineType| {
        let p2 = (x + a.cos(), y - a.sin());
        intersections.push(intersection_of_vector_and_poly(corners_m, p1, p2, line_type));
    };

    // `:1056-1064` 的三段 if/elif 结构照抄（`elif` 绑在第二个 if 上）
    if matches!(symm_type, SymmetryType::Horizontal | SymmetryType::VertHorz) {
        append(angle, LineType::Line);
    }
    if matches!(symm_type, SymmetryType::Vertical | SymmetryType::VertHorz) {
        append(angle + std::f32::consts::PI / 2.0, LineType::Line);
    } else if matches!(symm_type, SymmetryType::Rotational | SymmetryType::Snowflake) {
        let delta = (std::f32::consts::PI * 2.0) / num_lines as f32;
        for i in 0..num_lines {
            append(angle + delta * i as f32, LineType::Directional);
        }
    }

    intersections
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| p.map(|(a, b)| (i, a, b)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::Document;

    /// 一个 800×600 的视口，模型坐标与画布局部坐标 1:1（zoom=1、offset=0）。
    fn unit_view() -> (egui::Rect, View) {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        (rect, View::default())
    }

    /// 无头跑一帧，返回本帧加入绘制列表的图形数。
    ///
    /// `egui::Context` 不依赖窗口/GPU：`run_ui()` 得到的 `FullOutput.shapes`
    /// 就是本帧的全部图形。面板自身会留 1 个空图形（背景/占位），所以
    /// "覆盖层什么都没画"的判据是"与 `paint=false` 的基线相同"，
    /// 而不是硬编码 0。
    fn frame_shape_count(doc: &Document, paint: bool) -> usize {
        let ctx = egui::Context::default();
        let (rect, view) = unit_view();
        let input = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    if paint {
                        paint_canvas_overlays(
                            &ui.painter().with_clip_rect(rect),
                            rect,
                            &view,
                            doc,
                            OverlayStyle::default(),
                        );
                    }
                });
        });
        // 没接渲染器，字体图集增量没人消费；丢掉前必须显式清空
        // （epaint 在 Drop 里会因此 panic）。
        output.textures_delta.clear();
        output.shapes.len()
    }

    /// 外观偏好：默认值 = 源码出厂值；`symmetry.line_alpha` 与
    /// `frame.color_rgba` 从偏好来，且画框颜色每通道夹到 `[0, 1]`
    /// （`symmetry.py:46-47`、`framewindow.py:502-505`）。
    #[test]
    fn overlay_style_reads_prefs_and_clamps_the_frame_color() {
        let dir = std::env::temp_dir().join(format!(
            "pigment-overlay-prefs-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        // 没有文件 → 全部出厂值
        let style = OverlayStyle::from_prefs(&Prefs::load(&path));
        assert_eq!(style, OverlayStyle::default());
        assert!((style.symmetry_line_alpha - 0.333).abs() < 1e-6);
        assert_eq!(style.frame_rgba, [0.12, 0.12, 0.12, 0.92]);

        // 有偏好 → 用它；越界的通道被夹住
        std::fs::write(
            &path,
            r#"{"symmetry.line_alpha": 0.5,
                "frame.color_rgba": [1.5, -0.5, 0.25, 0.5]}"#,
        )
        .unwrap();
        let style = OverlayStyle::from_prefs(&Prefs::load(&path));
        assert!((style.symmetry_line_alpha - 0.5).abs() < 1e-6);
        assert_eq!(style.frame_rgba, [1.0, 0.0, 0.25, 0.5]);

        // 类型不对（数组太短/元素不是数）→ 缺的通道保持出厂值
        std::fs::write(&path, r#"{"frame.color_rgba": ["x", 0.5]}"#).unwrap();
        let style = OverlayStyle::from_prefs(&Prefs::load(&path));
        assert_eq!(style.frame_rgba, [0.12, 0.5, 0.12, 0.92]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 对称关闭时覆盖层**一个图形都不加**；开启后画出虚线轴（两遍颜色）。
    #[test]
    fn symmetry_overlay_draws_only_when_active() {
        let mut doc = Document::new();
        // 先给一个真实中心，免得默认 (0,0) 落在视口角上
        doc.set_symmetry_state(
            Some(false),
            Some((400, 300)),
            Some(SymmetryType::Vertical),
            Some(2),
            Some(0.0),
        );
        let baseline = frame_shape_count(&doc, false);
        assert_eq!(
            frame_shape_count(&doc, true),
            baseline,
            "对称关闭 → 覆盖层不该加任何图形"
        );

        doc.set_symmetry_state(Some(true), None, None, None, None);
        let on = frame_shape_count(&doc, true);
        println!("  对称轴线：基线 {baseline} → 开启后 {on} 个图形");
        assert!(on > baseline, "对称开启后应画出轴线：{baseline} → {on}");
        // 一条竖轴、两遍颜色、虚线按 34px 周期切成若干段：
        // 600px 高 → 每遍约 18 段，两遍加起来远多于 2 段。
        assert!(on - baseline >= 8, "虚线轴应切成多段，实得 {} 段", on - baseline);
    }

    /// 画框关闭时覆盖层**一个图形都不加**；开启后画出 4 块遮罩 + 边线。
    #[test]
    fn frame_overlay_draws_only_when_enabled() {
        let mut doc = Document::new();
        doc.set_frame(100, 100, 300, 200);
        assert!(!doc.get_frame_enabled(), "画框默认关闭");
        let baseline = frame_shape_count(&doc, false);
        assert_eq!(
            frame_shape_count(&doc, true),
            baseline,
            "关闭 → 覆盖层不该加任何图形"
        );

        doc.set_frame_enabled(true);
        let on = frame_shape_count(&doc, true);
        println!("  画框：基线 {baseline} → 开启后 {on} 个图形");
        assert!(on - baseline >= 4, "四块遮罩至少 4 个图形：{baseline} → {on}");
    }

    fn corners(rect: egui::Rect, view: &View) -> Vec<(f32, f32)> {
        view_corners_model(rect, view).to_vec()
    }

    /// 水平轴 = 过中心的一整条横线，截到视口左右边缘。
    #[test]
    fn horizontal_axis_spans_the_viewport() {
        let (rect, view) = unit_view();
        let axes = viewport_intersections(
            SymmetryType::Horizontal,
            400.0,
            300.0,
            0.0,
            2,
            &corners(rect, &view),
        );
        assert_eq!(axes.len(), 1);
        let (_, a, b) = axes[0];
        assert!((a.1 - 300.0).abs() < 1e-3 && (b.1 - 300.0).abs() < 1e-3, "y 应恒为 300");
        let mut xs = [a.0, b.0];
        xs.sort_by(|p, q| p.partial_cmp(q).unwrap());
        assert!(xs[0].abs() < 1e-3 && (xs[1] - 800.0).abs() < 1e-3, "x 应到视口两端：{xs:?}");
    }

    /// 垂直轴 = 过中心的一整条竖线（`angle` 再加 90°，`symmetry.py:1058`）。
    #[test]
    fn vertical_axis_is_perpendicular() {
        let (rect, view) = unit_view();
        let axes = viewport_intersections(
            SymmetryType::Vertical,
            300.0,
            250.0,
            0.0,
            2,
            &corners(rect, &view),
        );
        assert_eq!(axes.len(), 1);
        let (_, a, b) = axes[0];
        assert!((a.0 - 300.0).abs() < 1e-3 && (b.0 - 300.0).abs() < 1e-3, "x 应恒为 300");
        let mut ys = [a.1, b.1];
        ys.sort_by(|p, q| p.partial_cmp(q).unwrap());
        assert!(ys[0].abs() < 1e-3 && (ys[1] - 600.0).abs() < 1e-3, "y 应到视口两端：{ys:?}");
    }

    /// 垂直 + 水平 = 两条轴（`VertHorz`）。
    #[test]
    fn verthorz_gives_two_axes() {
        let (rect, view) = unit_view();
        let axes = viewport_intersections(
            SymmetryType::VertHorz,
            400.0,
            300.0,
            0.0,
            2,
            &corners(rect, &view),
        );
        assert_eq!(axes.len(), 2);
        assert_eq!(axes[0].0, 0, "第一条是水平轴（下标 0）");
        assert_eq!(axes[1].0, 1, "第二条是垂直轴（下标 1）");
    }

    /// 旋转对称的每条轴是**从中心出发的射线**（`LineType.DIRECTIONAL`），
    /// 所以其中一个端点必然就是对称中心。
    #[test]
    fn rotational_axes_are_rays_from_the_centre() {
        let (rect, view) = unit_view();
        let axes = viewport_intersections(
            SymmetryType::Rotational,
            400.0,
            300.0,
            0.0,
            4,
            &corners(rect, &view),
        );
        assert_eq!(axes.len(), 4, "4 条线 = 4 条射线");
        for (i, a, b) in &axes {
            let starts_at_centre = (a.0 - 400.0).abs() < 1e-3 && (a.1 - 300.0).abs() < 1e-3;
            let ends_at_centre = (b.0 - 400.0).abs() < 1e-3 && (b.1 - 300.0).abs() < 1e-3;
            assert!(
                starts_at_centre || ends_at_centre,
                "第 {i} 条射线必须从中心出发：{a:?} → {b:?}"
            );
            // 端点在视口边界上
            let on_border = |p: &(f32, f32)| {
                p.0.abs() < 1e-2
                    || (p.0 - 800.0).abs() < 1e-2
                    || p.1.abs() < 1e-2
                    || (p.1 - 600.0).abs() < 1e-2
            };
            assert!(on_border(a) || on_border(b), "射线另一端应落在视口边上");
        }
    }

    /// `symmetry_angle` 把轴整体转过去：水平轴转 90° 变成竖线。
    #[test]
    fn angle_rotates_the_axis() {
        let (rect, view) = unit_view();
        let axes = viewport_intersections(
            SymmetryType::Horizontal,
            400.0,
            300.0,
            90.0,
            2,
            &corners(rect, &view),
        );
        assert_eq!(axes.len(), 1);
        let (_, a, b) = axes[0];
        assert!((a.0 - 400.0).abs() < 1e-2 && (b.0 - 400.0).abs() < 1e-2, "应变成竖线：{a:?} {b:?}");
    }

    /// 中心在视口外时，无限直线照样截出一段（这就是用 `LineType::Line` 的意义）。
    #[test]
    fn centre_outside_the_viewport_still_clips() {
        let (rect, view) = unit_view();
        let axes = viewport_intersections(
            SymmetryType::Horizontal,
            -500.0,
            300.0,
            0.0,
            2,
            &corners(rect, &view),
        );
        assert_eq!(axes.len(), 1, "中心在视口外也该有可见的一段");
        let (_, a, b) = axes[0];
        assert!((a.1 - 300.0).abs() < 1e-3 && (b.1 - 300.0).abs() < 1e-3);
    }

    /// 画框矩形：模型坐标经视图变换到画布局部坐标。
    #[test]
    fn frame_rect_maps_through_the_view() {
        let canvas = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
        let view = View {
            zoom: 2.0,
            offset: [10.0, 20.0],
            ..View::default()
        };
        let frame = frame_display_rect(canvas, &view, (0, 0, 100, 50));
        assert_eq!(frame.min, egui::pos2(110.0, 70.0));
        assert_eq!(frame.max, egui::pos2(310.0, 170.0));
    }

    /// 画框外的遮罩：互不重叠、都不落在画框内、拼起来正好是
    /// "画布减去画框"。
    #[test]
    fn outside_rects_cover_exactly_the_frame_exterior() {
        let canvas = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let frame = egui::Rect::from_min_size(egui::pos2(200.0, 100.0), egui::vec2(300.0, 200.0));
        let rects = outside_rects(canvas, frame);
        assert_eq!(rects.len(), 4, "画框在画布内 → 上/下/左/右四块");
        // 总面积 = 画布面积 - 画框面积
        let covered: f32 = rects.iter().map(|r| r.width() * r.height()).sum();
        let expected = canvas.width() * canvas.height() - frame.width() * frame.height();
        assert!((covered - expected).abs() < 1e-3, "覆盖 {covered} ≠ {expected}");
        // 没有一块碰到画框内部
        for r in &rects {
            let overlap = r.intersect(frame);
            assert!(
                overlap.width() <= 0.0 || overlap.height() <= 0.0,
                "遮罩与画框重叠：{r:?}"
            );
        }

        // 画框整个在画布外：等价于"整块画布都被涂"（cairo even-odd 的结果）
        let far = egui::Rect::from_min_size(egui::pos2(2000.0, 2000.0), egui::vec2(100.0, 100.0));
        assert_eq!(outside_rects(canvas, far), vec![canvas]);

        // 画框只露出一角：四块遮罩里的"上"只有露出部分的高度
        let corner = egui::Rect::from_min_size(egui::pos2(700.0, 500.0), egui::vec2(300.0, 300.0));
        let rects = outside_rects(canvas, corner);
        assert_eq!(rects[0].height(), 500.0, "上遮罩到画框顶为止");
        assert_eq!(rects[2].width(), 700.0, "左遮罩到画框左边缘为止");
    }

    /// 虚线区间：cairo 的 `set_dash(pattern, offset)` 语义。
    ///
    /// 图案 `[10, 24]`（周期 34）、两遍偏移 5 与 5+(10+7)=22。
    /// 22 正好是 5 + 半个周期，所以第二遍的实线段落在第一遍的空白正中间，
    /// 两者永不重叠 —— 这就是 `symmetry.py:1008-1012` 的双色虚线效果。
    #[test]
    fn dash_intervals_match_cairo_offsets() {
        let first = dash_intervals(100.0, DASH_PATTERN[0], DASH_PATTERN[1], DASH_OFFSET);
        assert_eq!(
            first,
            vec![(0.0, 5.0), (29.0, 39.0), (63.0, 73.0), (97.0, 100.0)]
        );
        let second_offset = DASH_OFFSET + (DASH_LENGTH + DASH_GAP);
        let second = dash_intervals(100.0, DASH_PATTERN[0], DASH_PATTERN[1], second_offset);
        assert_eq!(second, vec![(12.0, 22.0), (46.0, 56.0), (80.0, 90.0)]);
        for (a0, a1) in &first {
            for (b0, b1) in &second {
                assert!(a1 <= b0 || b1 <= a0, "[{a0},{a1}] 与 [{b0},{b1}] 重叠");
            }
        }
        // 第一遍的末尾（5）到第二遍的开头（12）正好是 _DASH_GAP
        assert!(((second[0].0 - first[0].1) - DASH_GAP).abs() < 1e-4);
    }
}

