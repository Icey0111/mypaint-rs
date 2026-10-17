//! HSV 三页的控件层 —— `hsvwheel.py` / `hsvsquare.py` / `hsvcube.py`（B4-4b）。
//!
//! ## 源码里这一块是什么
//!
//! 三个页面都是 `CombinedAdjusterPage`（`gui/colors/combined.py:20-66`）：
//!
//! | 页面 | 页面类 | 页面控件由什么组成 |
//! |---|---|---|
//! | HSV Wheel | `HSVAdjusterPage`（`hsvwheel.py:60-102`） | 竖直 `HSVValueSlider` + `HSVHueSaturationWheel`（`Gtk.Table` 1×2，padding 3） |
//! | HSV Square | `HSVSquarePage`（`hsvsquare.py:33-100`） | `HSVSquare`（`Gtk.VBox`）：外环（色相）+ 中央 54% 的内方块（饱和/明度），`Gtk.AspectFrame` 强制正方 |
//! | HSV Cube | `HSVCubePage`（`hsvcube.py:35-115`） | 竖直深度滑条 + 旋转按钮（左列）+ 平面切片（右列跨两行） |
//!
//! 页面本体与两个"主调整器"的**数学**都在引擎
//! （[`pigment_engine::color::wheels`]：极坐标、`SAT_GAMMA`、色相畸变、切片
//! 坐标换算，全部有真源码向量门禁）。本文件只做三件事：布局换算（GTK 容器
//! → egui 矩形）、绘制（Cairo → egui mesh）、指针事件（GTK `GdkEvent` →
//! [`PointerFrame`] + [`pigment_engine::color::ColorChanger`]）。
//!
//! ## 四个"技术栈适配"（都有源码依据，差异逐条记账）
//!
//! 1. **布局**：源码靠 GTK 的 size-request / `Gtk.Table` / `Gtk.AspectFrame`
//!    协商尺寸；egui 没有两轮协商，所以这里把容器语义翻成纯函数
//!    （[`square_rect`] / [`inner_square_rect`] / [`wheel_page_rects`] /
//!    [`cube_page_rects`]），每帧算一次。数值常量照抄源码（`TABLE_PADDING = 3`
//!    来自 `Gtk.Table.attach(..., 3, 3)`，`0.54` 来自 `xscale/yscale`）。
//! 2. **径向去饱和渐变**：Cairo 用 `cairo.RadialGradient` 沿半径插值 alpha
//!    （`adjbases.py:1233-1240`）。egui 没有径向渐变，这里把
//!    [`distance_fade_stops`] 的 8 个色标之间的每一段铺成一圈四边形
//!    （[`paint_radial_fade`]）—— 插值方式仍是"相邻色标之间线性"，与 Cairo 同构。
//! 3. **挖洞**：HSV 方块的外环画完扇区后，用 `OPERATOR_DEST_OUT` 把
//!    `radius*0.8` 的圆盘擦成透明（`hsvsquare.py:271-278`），给内方块的四个
//!    角让位。egui 没有混合运算，这里用面板底色把那个圆盘填掉
//!    （[`paint_wheel_disc`] 的 `panel_fill` 参数）—— 可见结果相同。
//! 4. **扇区内的色相插值**：源码每格画"实心扇区"再叠一层"反向上色渐变"
//!    （`adjbases.py:1210-1230` / `hsvsquare.py:240-261`），合起来是扇区两端
//!    相邻色相之间的过渡。egui 用一个三角形的顶点色插值直接表达（圆心取本格
//!    色相、两条边取相邻两个色相）—— 径向的插值路径与"实心 + 反向渐变"略有
//!    不同，但两端颜色与扇区边界完全一致。
//!
//! ## 没接的（逐条记账，见交付说明）
//!
//! - **DnD**（`adjbases.py:536-601` 的 `application/x-color` 拖出/拖入）：
//!   要 GTK 的选择区与 `drag_begin_with_coordinates()`。
//! - **双击详情对话框**（`adjbases.py:679-695` 的 `HAS_DETAILS_DIALOG`）：
//!   三个页面的控件都没开这个类常量（`HAS_DETAILS_DIALOG=False`），
//!   所以源码里也不会弹 —— 无需移植。
//! - **`get_background_validity()`**（`adjbases.py:1138-1146` 等）：缓存重绘
//!   令牌；egui 每帧重画。
//! - **立方旋转按钮的图标**：源码用 `mypaint-hsv-rotate-symbolic`
//!   （`hsvcube.py:53-57`，`Gtk.IconSize.MENU`）。本项目只栅格化了工具条/面板
//!   用的 SVG（`assets/icons/` 里有这个文件，但 `build.rs` 只按名字导出，
//!   egui 的 `Button` 也不吃图标），所以按钮文字用 "⟳"、提示照抄源码。
//! - **色轮类型切换 UI**：源码在偏好对话框里调
//!   `ColorManager.set_wheel_type()`（`adjbases.py:291-315`）。本块把管理器侧
//!   接好（`ColorManager::set_wheel_type`），但**没有**做切换入口 —— 偏好
//!   对话框属 B6-3，届时接一个下拉即可；当前色轮类型本身已从偏好
//!   `colors.wheels.type` 读入（`ColorManager::from_prefs`，B6-1），
//!   默认 `"rgb"`（恒等）。
//! - **`Mood and Accent` 预设 / 色轮上的鼠标 grab / DnD**：见交付记账
//!   （HCY 轮本身属 B4-4c）。

use eframe::egui;
use pigment_engine::color::{
    depth_slider_amount, depth_slider_color, depth_slider_scroll, distance_fade_stops,
    hcy_wheel_center_rgb, hcy_wheel_position_for_color, hcy_wheel_slice_rgb,
    hsv_ring_color_at_position, hsv_ring_position_for_color, hsv_ring_slice_rgb,
    hsv_wheel_center_rgb, hsv_wheel_color_at_position, hsv_wheel_position_for_color,
    hsv_wheel_scroll, hsv_wheel_slice_rgb, hue_slice_hues, point_to_amount, slice_color_at_position,
    slice_faces, slice_position_for_color, tumble_faces, Color, ColorChanger, ColorManager,
    ComponentSlider, DragUpdate, HsvColor, HsvFace, Modifiers, ScrollDirection, SliderKind, UIColor,
    WheelGeometry, BORDER_WIDTH, CUBE_SLIDER_SAMPLES, EDGE_HIGHLIGHT_RGBA, EDGE_HIGHLIGHT_WIDTH,
    HUE_SLICES, OUTLINE_RGBA, OUTLINE_WIDTH, SAT_GAMMA, SAT_SLICES, SLIDER_MIN_WIDTH, TABLE_PADDING,
};

use crate::app::PigmentApp;
use crate::shell::color_panel::{rgb32, wheel_direction};

// ---------------------------------------------------------------------------
// 常量（逐条对应源码行）
// ---------------------------------------------------------------------------

/// 内方块占外环的比例 —— `hsvsquare.py:115-120` 的
/// `Gtk.Alignment(xscale=0.54, yscale=0.54)`。
///
/// 再往外套了一层 `Gtk.AspectFrame`（`:121-124`），所以内方块永远是正方。
pub(crate) const SQUARE_INNER_SCALE: f32 = 0.54;

/// 立方页旋转按钮的高度 —— `hsvcube.py:53-57` 的
/// `borderless_button(..., size=Gtk.IconSize.MENU)`（GTK3 的 MENU = 16px）
/// 加上 `Gtk.Table.attach(..., 3, 3)` 的上下 padding（`:70-80`）。
pub(crate) const CUBE_ROTATE_BUTTON_HEIGHT: f32 = 16.0 + 2.0 * 3.0;

/// 表格格子的 padding —— `TABLE_PADDING`（引擎里，源码 `hsvwheel.py:76-77` /
/// `hsvcube.py:69-81` 的 `attach(..., 3, 3)`）。
pub(crate) const PAGE_PADDING: f32 = TABLE_PADDING as f32;

/// 拖动阈值 —— `adjbases.py:528-532` 读 GTK 的 `gtk-dnd-drag-threshold`。
///
/// GTK3 的默认值是 8，源码再 `min(4, 8) * 2 = 8`。egui 没有这个设置，用默认值
/// （记账：接偏好系统后改成可配）。
pub(crate) const DRAG_THRESHOLD: f64 = 8.0;

/// 立方页三个子控件的矩形（`hsvcube.py:64-81` 的 `Gtk.Table`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CubeRects {
    /// 左列上半：竖直深度滑条（`attach(s_align, 0, 1, 0, 1, FILL, yopts)`）。
    pub slider: egui::Rect,
    /// 左列下半：旋转按钮（`attach(button, 0, 1, 1, 2, FILL, FILL)`）。
    pub button: egui::Rect,
    /// 右列跨两行：平面切片（`attach(self.__slice, 1, 2, 0, 2, xopts, yopts)`）。
    pub slice: egui::Rect,
}

// ---------------------------------------------------------------------------
// 布局（纯函数，可脱离窗口测试）
// ---------------------------------------------------------------------------

/// 可用区内最大的居中正方形 —— `Gtk.AspectFrame(obey_child=True)` 的等价物
/// （`hsvsquare.py:121-124`；HSV 轮页的 `get_page_widget()` 也套了一个，
/// `hsvwheel.py:94-97`）。
pub(crate) fn square_rect(rect: egui::Rect) -> egui::Rect {
    let side = rect.width().min(rect.height()).max(0.0);
    egui::Rect::from_center_size(rect.center(), egui::vec2(side, side))
}

/// 内方块：居中的 54% 正方形 —— `hsvsquare.py:115-125`：
///
/// ```python
/// s_align = Gtk.Alignment(xalign=0.5, yalign=0.5, xscale=0.54, yscale=0.54)
/// plz_be_square = Gtk.AspectFrame()
/// s_align.add(plz_be_square); plz_be_square.add(self.__square)
/// self.__ring.add(s_align)
/// ```
///
/// 注意内方块是**环的子控件**，所以它相对环的分配（也就是整个正方区）居中，
/// 而不是相对窗口。
pub(crate) fn inner_square_rect(outer: egui::Rect) -> egui::Rect {
    let side = outer.width().min(outer.height()).max(0.0) * SQUARE_INNER_SCALE;
    egui::Rect::from_center_size(outer.center(), egui::vec2(side, side))
}

/// HSV 轮页的两列 —— `hsvwheel.py:72-77`：
///
/// ```python
/// table = Gtk.Table(n_rows=1, n_columns=2)
/// table.attach(v_align, 0, 1, row, row+1, FILL, yopts, 3, 3)   # 左：值滑条
/// table.attach(self.__hs_adj, 1, 2, row, row+1, xopts, yopts, 3, 3)
/// ```
///
/// 左列宽度取滑条的最小宽度（`uimisc.py:25` 的 18：`SliderColorAdjuster` 在
/// realize 时 `set_size_request(bw, bl)`，`adjbases.py:961-968`）；`xscale=0`
/// 让它不再横向扩张。两格各留 3px padding，所以中间空 6px。
pub(crate) fn wheel_page_rects(rect: egui::Rect) -> (egui::Rect, egui::Rect) {
    let pad = PAGE_PADDING;
    let inner_h = (rect.height() - 2.0 * pad).max(0.0);
    let left_w = SLIDER_MIN_WIDTH as f32;
    let slider = egui::Rect::from_min_size(
        rect.min + egui::vec2(pad, pad),
        egui::vec2(left_w, inner_h),
    );
    let wheel_w = (rect.width() - 2.0 * pad - left_w - 2.0 * pad).max(0.0);
    let wheel = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad + left_w + 2.0 * pad, rect.top() + pad),
        egui::vec2(wheel_w, inner_h),
    );
    (slider, wheel)
}

/// HSV 立方页的三块 —— `hsvcube.py:64-81`（2 行 × 2 列的 `Gtk.Table`）：
///
/// ```text
/// +--------+-------------------+
/// | 滑条    |                   |
/// |（跨行0）|      切片         |
/// +--------+ （跨 0..2 两行）   |
/// | 旋转钮  |                   |
/// +--------+-------------------+
/// ```
pub(crate) fn cube_page_rects(rect: egui::Rect) -> CubeRects {
    let pad = PAGE_PADDING;
    let left_w = SLIDER_MIN_WIDTH as f32;
    let inner_w = (rect.width() - 2.0 * pad).max(0.0);
    let inner_h = (rect.height() - 2.0 * pad).max(0.0);
    let button_h = CUBE_ROTATE_BUTTON_HEIGHT;
    let slider_h = (inner_h - button_h - 2.0 * pad).max(0.0);
    let slider = egui::Rect::from_min_size(
        rect.min + egui::vec2(pad, pad),
        egui::vec2(left_w, slider_h),
    );
    let button = egui::Rect::from_min_size(
        egui::pos2(
            rect.left() + pad,
            rect.top() + pad + slider_h + 2.0 * pad,
        ),
        egui::vec2(left_w, button_h),
    );
    let slice_w = (inner_w - left_w - 2.0 * pad).max(0.0);
    let slice = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad + left_w + 2.0 * pad, rect.top() + pad),
        egui::vec2(slice_w, inner_h),
    );
    CubeRects {
        slider,
        button,
        slice,
    }
}

/// 页面的分配尺寸 —— 面板可纵向滚动，`available_height()` 可能是无穷大
/// （`egui::ScrollArea` 的语义）。源码的页面控件有 100×100 的最小尺寸
/// （`uimisc.py:21-22`），并靠 `AspectFrame` 把轮/方块撑成正方；这里：
///
/// - `square = true`（轮/方块页）：高度不超过宽度（正方区）；
/// - `square = false`（立方页）：填满可用高度；高度不可用时退回宽度。
pub(crate) fn page_size(ui: &egui::Ui, square: bool) -> egui::Vec2 {
    let w = ui.available_width().max(1.0);
    let avail_h = ui.available_height();
    let h = if avail_h.is_finite() && avail_h > 1.0 {
        avail_h
    } else {
        w
    };
    let h = if square { h.min(w) } else { h };
    egui::vec2(w, h.max(1.0))
}

/// 切片逐列绘制的步长 —— `hsvsquare.py:340` / `hsvcube.py:171`：
/// `step = max(1, int(eff_wd // 128))`。
pub(crate) fn slice_column_step(eff_wd: f32) -> f32 {
    // eff_wd 在源码里是 int；这里先截断成整数再整除
    let step = (eff_wd.trunc() as i32) / 128;
    step.max(1) as f32
}

/// 竖直滑条标记线的 y（控件坐标）—— `adjbases.py:1024-1038`：
///
/// ```python
/// amt = float(clamp(amt, 0, 1))
/// bar_size = int((vertical and ht or wd) - 1 - 2 * b)
/// if self.vertical:
///     amt = 1.0 - amt
///     y1 = y2 = int(amt * bar_size) + b + 0.5
/// ```
pub(crate) fn vertical_marker_y(amount: f64, height: f32, border: f32) -> f32 {
    let amt = clamp01(amount);
    let bar_size = (f64::from(height) - 1.0 - 2.0 * f64::from(border)).trunc();
    let amt = 1.0 - amt;
    (amt * bar_size).trunc() as f32 + border + 0.5
}

/// `gui/colors/util.py:18-24` 的 `clamp(v, 0, 1)`（NaN 原样返回）。
fn clamp01(v: f64) -> f64 {
    if v > 1.0 {
        return 1.0;
    }
    if v < 0.0 {
        return 0.0;
    }
    v
}

/// HSV 方块页的命中区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SquareRegion {
    /// 中央内方块（饱和/明度）——`_HSVSquareInnerSquare`。
    Inner,
    /// 外环（只改色相）——`_HSVSquareOuterRing`。
    Ring,
}

/// 指针落在哪个子控件 —— `_HSVSquareInnerSquare.stop_fallthrough`
/// （`hsvsquare.py:319-322`）的语义：内方块是环的子控件，事件先给它；
/// 它 `return True` 吃掉按下事件，环不再收到。
///
/// 环的分配是整个正方区（`HSVSquare` 的 `pack_start(self.__ring, True, True, 0)`，
/// `hsvsquare.py:126`），所以"环"= 正方区减去内方块。
pub(crate) fn square_hit_region(
    outer: egui::Rect,
    inner: egui::Rect,
    pos: egui::Pos2,
) -> Option<SquareRegion> {
    if inner.contains(pos) {
        Some(SquareRegion::Inner)
    } else if outer.contains(pos) {
        Some(SquareRegion::Ring)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 指针事件
// ---------------------------------------------------------------------------

/// 一帧的指针状态（egui 输入 → 源码 `GdkEventButton` / `GdkEventMotion` 的
/// 三个字段：位置、按下的按钮、修饰键）。
///
/// 单独成结构体是为了让 [`drive_changer`] 能脱离 `egui::Ui` 测试：测试里直接
/// 构造它，就能验证"按下 → 落色 / 拖动 → 连续落色 / 按钮>1 → HCY 弯曲"。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PointerFrame {
    /// 指针位置（控件坐标由调用方减去原点）。
    pub pos: Option<egui::Pos2>,
    /// 本帧**按下**的按钮，下标 0/1/2 = 左/中/右（对应源码按钮号 1/2/3）。
    pub pressed: [bool; 3],
    /// 本帧**抬起**的按钮。
    pub released: [bool; 3],
    /// `Gdk.ModifierType.SHIFT_MASK` / `CONTROL_MASK`（`adjbases.py:756-760`）。
    pub modifiers: Modifiers,
}

impl PointerFrame {
    /// 从 egui 的输入状态读一帧。
    pub(crate) fn read(ui: &egui::Ui) -> Self {
        ui.input(|i| {
            let pos = i.pointer.interact_pos().or_else(|| i.pointer.hover_pos());
            Self {
                pos,
                pressed: [
                    i.pointer.button_pressed(egui::PointerButton::Primary),
                    i.pointer.button_pressed(egui::PointerButton::Secondary),
                    i.pointer.button_pressed(egui::PointerButton::Middle),
                ],
                released: [
                    i.pointer.button_released(egui::PointerButton::Primary),
                    i.pointer.button_released(egui::PointerButton::Secondary),
                    i.pointer.button_released(egui::PointerButton::Middle),
                ],
                modifiers: Modifiers {
                    shift: i.modifiers.shift,
                    control: i.modifiers.ctrl,
                },
            }
        })
    }
}

/// 把一个子控件的指针事件喂给 [`ColorChanger`]，返回本帧要落的颜色。
///
/// 对应 `ColorAdjusterWidget` 的三个私有回调（`adjbases.py:660-808`）：
/// - **按下**：先算位置色，再 `start_drag()`（按钮 1 落色 / 按钮 2·3 只记起点）；
/// - **移动**：`update()`，`SamplePosition` 时按新位置落色，`Bended` 时颜色
///   已经写进管理器；
/// - **抬起**：`stop_drag()` 清状态。
///
/// 两个刻意的取舍：
/// 1. **按下那一帧不跑 `update()`** —— 源码的按下与移动是两个事件，按钮 2
///    按下时不该立刻把颜色转成 HCY（`adjbases.py:697-699` 只记起点）。egui 把
///    "按下"与"当帧移动"合并在一次输入里，所以这里显式跳过一次。
/// 2. `rtl` 恒为 `false`：egui 的布局方向不进指针事件，本项目 UI 是 LTR
///    （源码的 `widget.get_direction() == Gtk.TextDirection.RTL` 分支，
///    `adjbases.py:765-767`，记账）。
///
/// `origin` 是子控件左上角（屏幕坐标），`area` 是它的尺寸（HCY 弯曲要用
/// `max(w, h)`，`adjbases.py:748-750`）。
pub(crate) fn drive_changer(
    frame: &PointerFrame,
    changer: &mut ColorChanger,
    manager: &mut ColorManager,
    origin: egui::Pos2,
    area: (f64, f64),
    hit: impl Fn(egui::Pos2) -> bool,
    sample: impl Fn(egui::Pos2) -> Option<Color>,
) -> Option<Color> {
    let local = |p: egui::Pos2| ((p.x - origin.x) as f64, (p.y - origin.y) as f64);
    let mut out = None;
    let mut pressed_now = false;

    if let Some(pos) = frame.pos {
        for (i, pressed) in frame.pressed.iter().enumerate() {
            if *pressed && hit(pos) {
                let color = sample(pos);
                let button = (i + 1) as u8;
                changer.start_drag(manager, button, false, local(pos), color);
                pressed_now = true;
                // 按钮 1 的按下**立刻**落色（`adjbases.py:671-672`）；按钮 2/3
                // 只记起点，不该让页面把同一颜色再写一遍。
                if button == 1 {
                    if let Some(color) = color {
                        out = Some(color);
                    }
                }
            }
        }
        if changer.button_down().is_some() && !pressed_now {
            // HCY 弯曲要除以 `max(w, h)`（`adjbases.py:748-750`），控件退化成
            // 0 尺寸时源码会 ZeroDivisionError；这里直接跳过。
            if area.0.max(area.1) <= 0.0 {
                return out;
            }
            match changer.update(manager, local(pos), area, frame.modifiers, false) {
                // :733-737 —— 非 drag source：连续按位置取色
                DragUpdate::SamplePosition => {
                    if let Some(color) = sample(pos) {
                        manager.set_color(color);
                        out = Some(color);
                    }
                }
                // :739-788 —— 按钮 > 1 的 HCY 弯曲，颜色已写进管理器
                DragUpdate::Bended(color) => out = Some(color),
                DragUpdate::None | DragUpdate::BeginDrag => {}
            }
        }
    }

    // :793-808 —— 抬起：清状态（点没点中不影响，源码也只比对按钮号）
    if let Some(button) = changer.button_down() {
        let idx = usize::from(button).saturating_sub(1);
        if frame.released.get(idx).copied().unwrap_or(false) {
            let pos = frame.pos.map(local).unwrap_or((0.0, 0.0));
            changer.stop_drag(button, pos);
        }
    }
    out
}

/// 本帧的滚轮方向 —— 与分量滑条页共用同一套 GTK 方向映射
/// （`color_panel::wheel_direction`，`adjbases.py:1069-1077`）。
pub(crate) fn scroll_direction(ui: &egui::Ui) -> Option<ScrollDirection> {
    let delta = ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::MouseWheel { delta, .. } => Some(*delta),
            _ => None,
        })
    });
    delta.and_then(wheel_direction)
}

// ---------------------------------------------------------------------------
// 绘制用的颜色小工具
// ---------------------------------------------------------------------------

fn rgba32(rgba: [f64; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (rgba[0] * 255.0).round() as u8,
        (rgba[1] * 255.0).round() as u8,
        (rgba[2] * 255.0).round() as u8,
        (rgba[3] * 255.0).round() as u8,
    )
}

/// 色轮画的是"带中心去饱和的轮"还是"纯色相环"
/// （`hsvwheel.py:53-57` vs `hsvsquare.py:189-192`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WheelStyle {
    /// HSV 轮：半径 = 饱和度，中心叠灰（`adjbases.py:1233-1240`）。
    Wheel,
    /// HSV 方块的外环：纯色相，中心挖洞（`hsvsquare.py:199-283`）。
    Ring,
    /// HCY 轮（B4-4c）：结构与 [`WheelStyle::Wheel`] **完全相同**
    /// （`hcywheel.py:339-342` 的 `HCYHueChromaWheel` 就是
    /// `HueSaturationWheelAdjuster` 的子类，没有覆写 `render_background_cb`），
    /// 只把三个颜色钩子换成 HCY 的：扇区色、盘心灰、纯色刻痕的位置
    /// （`hcywheel.py:328-336`）。
    Hcy,
}

fn style_slice_rgb(style: WheelStyle, managed: &Color, hue: f64) -> [f64; 3] {
    match style {
        WheelStyle::Wheel => hsv_wheel_slice_rgb(managed, hue),
        WheelStyle::Ring => hsv_ring_slice_rgb(managed, hue),
        WheelStyle::Hcy => hcy_wheel_slice_rgb(managed, hue),
    }
}

/// 盘心去饱和渐变的颜色 —— `adjbases.py:1173` 的
/// `color_at_normalized_polar_pos(0, 0)` 套各自的钩子。
fn style_center_rgb(style: WheelStyle, managed: &Color) -> [f64; 3] {
    match style {
        WheelStyle::Hcy => hcy_wheel_center_rgb(managed),
        // 环不画这层渐变（`hsvsquare.py:199-283` 没有径向渐变），取哪个都行
        WheelStyle::Wheel | WheelStyle::Ring => hsv_wheel_center_rgb(managed),
    }
}

/// 盘边纯色小刻痕的位置 —— `adjbases.py:1261-1263` 的
/// `self.get_pos_for_color(col)`，套各自的钩子。
fn style_notch_position(
    style: WheelStyle,
    geom: &WheelGeometry,
    color: &Color,
    ranges: Option<&[pigment_engine::color::HueRange]>,
) -> (f64, f64) {
    match style {
        WheelStyle::Hcy => hcy_wheel_position_for_color(geom, color, ranges),
        WheelStyle::Wheel | WheelStyle::Ring => hsv_wheel_position_for_color(geom, color, ranges),
    }
}

// ---------------------------------------------------------------------------
// 绘制
// ---------------------------------------------------------------------------

/// 画圆盘底色 —— `HueSaturationWheelMixin.render_background_cb()`
/// （`adjbases.py:1169-1285`）与 `_HSVSquareOuterRing.render_background_cb()`
/// （`hsvsquare.py:199-283`）。
///
/// 顺序照抄（两种样式只在中间几步不同）：
/// 1. 外框（`OUTLINE_WIDTH` 的深色圆，`:1192-1196` / `:218-222`）；
/// 2. [`HUE_SLICES`] 个扇区（`:1204-1231` / `:231-261`）；
/// 3. **轮**：中心径向去饱和（`:1233-1240`）；盘边高光（`:1242-1246`）；
///    大于 75px 时画六个纯色小刻痕（`:1248-1283`）；
/// 4. **环**：盘边高光（`:265-269`）；`radius*0.8` 挖洞（`:271-278`）；
///    洞边再描外框 + 高光（`:271-283`）。
///
/// `panel_fill` 只给"环"的挖洞用（见模块文档的技术栈适配 3）。
pub(crate) fn paint_wheel_disc(
    painter: &egui::Painter,
    rect: egui::Rect,
    managed: &Color,
    ranges: Option<&[pigment_engine::color::HueRange]>,
    style: WheelStyle,
    panel_fill: egui::Color32,
) {
    let geom = WheelGeometry::for_size(
        f64::from(rect.width()),
        f64::from(rect.height()),
        BORDER_WIDTH,
    );
    if !geom.is_usable() {
        return;
    }
    let center = egui::pos2(
        rect.left() + geom.center_x as f32,
        rect.top() + geom.center_y as f32,
    );
    let radius = geom.radius as f32;
    let border = BORDER_WIDTH as f32;
    // 裁剪到 radius + border（`adjbases.py:1189-1190`）
    let clip = egui::Rect::from_center_size(
        center,
        egui::vec2(2.0 * (radius + border), 2.0 * (radius + border)),
    )
    .intersect(painter.clip_rect());
    let painter = painter.with_clip_rect(clip);

    // ---- 外框（:1192-1196 / hsvsquare.py:218-222）----
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(OUTLINE_WIDTH as f32, rgba32(OUTLINE_RGBA)),
    );

    // ---- 扇区（:1204-1231 / :231-261）----
    //
    // 源码每格画"实心扇区 + 反向渐变"两层，合起来是扇区内的色相插值；这里用
    // 一个三角形：圆心 = 本格色相、两条边 = 本格与下一格的色相，顶点色插值
    // 得到同样的过渡（见模块文档）。
    let hues = hue_slice_hues(HUE_SLICES, ranges);
    let step = std::f32::consts::TAU / HUE_SLICES as f32;
    let mut mesh = egui::Mesh::default();
    for i in 0..HUE_SLICES {
        let a = i as f32 * step;
        let b = (i + 1) as f32 * step;
        let color_a = rgb32(style_slice_rgb(style, managed, hues[i]));
        let color_b = rgb32(style_slice_rgb(style, managed, hues[i + 1]));
        let rim_a = center + egui::vec2(radius * a.cos(), radius * a.sin());
        let rim_b = center + egui::vec2(radius * b.cos(), radius * b.sin());
        let base = mesh.vertices.len() as u32;
        for (pos, color) in [(center, color_a), (rim_a, color_a), (rim_b, color_b)] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2]);
    }
    painter.add(egui::Shape::mesh(mesh));

    match style {
        WheelStyle::Wheel | WheelStyle::Hcy => {
            // ---- 中心去饱和（:1233-1240）----
            paint_radial_fade(
                &painter,
                center,
                radius,
                style_center_rgb(style, managed),
            );
            // ---- 盘边高光（:1242-1246）----
            painter.circle_stroke(
                center,
                radius,
                egui::Stroke::new(
                    EDGE_HIGHLIGHT_WIDTH as f32,
                    rgba32(EDGE_HIGHLIGHT_RGBA),
                ),
            );
            // ---- 纯色刻痕（:1248-1283）：`if wd > 75 or ht > 75` ----
            if rect.width() > 75.0 || rect.height() > 75.0 {
                let pure = [
                    pigment_engine::color::RgbColor::new(1.0, 0.0, 0.0),
                    pigment_engine::color::RgbColor::new(1.0, 1.0, 0.0),
                    pigment_engine::color::RgbColor::new(0.0, 1.0, 0.0),
                    pigment_engine::color::RgbColor::new(0.0, 1.0, 1.0),
                    pigment_engine::color::RgbColor::new(0.0, 0.0, 1.0),
                    pigment_engine::color::RgbColor::new(1.0, 0.0, 1.0),
                ];
                for rgb in pure {
                    let (x, y) = style_notch_position(style, &geom, &Color::Rgb(rgb), ranges);
                    let p = rect.min + egui::vec2(x as f32, y as f32);
                    painter.circle_filled(
                        p,
                        1.0 + EDGE_HIGHLIGHT_WIDTH as f32,
                        rgba32(EDGE_HIGHLIGHT_RGBA),
                    );
                    painter.circle_filled(
                        p,
                        EDGE_HIGHLIGHT_WIDTH as f32,
                        rgba32(OUTLINE_RGBA),
                    );
                }
            }
        }
        WheelStyle::Ring => {
            // ---- 盘边高光（hsvsquare.py:265-269）----
            painter.circle_stroke(
                center,
                radius,
                egui::Stroke::new(
                    EDGE_HIGHLIGHT_WIDTH as f32,
                    rgba32(EDGE_HIGHLIGHT_RGBA),
                ),
            );
            // ---- radius*0.8 挖洞（:271-278 的 OPERATOR_DEST_OUT）----
            painter.circle_filled(center, radius * 0.8, panel_fill);
            painter.circle_stroke(
                center,
                radius * 0.8,
                egui::Stroke::new(OUTLINE_WIDTH as f32, rgba32(OUTLINE_RGBA)),
            );
            painter.circle_stroke(
                center,
                radius * 0.8,
                egui::Stroke::new(
                    EDGE_HIGHLIGHT_WIDTH as f32,
                    rgba32(EDGE_HIGHLIGHT_RGBA),
                ),
            );
        }
    }
}

/// 径向去饱和渐变 —— `adjbases.py:1233-1240` + `util.py:27-40`。
///
/// 源码是一个 `cairo.RadialGradient(0,0,0, 0,0,radius)`，色标由
/// [`distance_fade_stops`] 给出（`SAT_SLICES` 段、`gamma = 1/SAT_GAMMA`）。
/// egui 没有径向渐变，这里把相邻色标之间铺成一圈四边形 —— 颜色与 alpha 都在
/// 相邻色标之间线性插值，与 Cairo 的插值方式相同。
fn paint_radial_fade(painter: &egui::Painter, center: egui::Pos2, radius: f32, rgb: [f64; 3]) {
    let stops = distance_fade_stops(rgb, SAT_SLICES, 1.0 / SAT_GAMMA, 1.0);
    const SEGMENTS: usize = 64;
    let mut mesh = egui::Mesh::default();
    for pair in stops.windows(2) {
        let (r0, c0) = (pair[0].0 as f32 * radius, rgba32(pair[0].1));
        let (r1, c1) = (pair[1].0 as f32 * radius, rgba32(pair[1].1));
        for i in 0..SEGMENTS {
            let a0 = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let dir0 = egui::vec2(a0.cos(), a0.sin());
            let dir1 = egui::vec2(a1.cos(), a1.sin());
            let base = mesh.vertices.len() as u32;
            for (pos, color) in [
                (center + dir0 * r0, c0),
                (center + dir0 * r1, c1),
                (center + dir1 * r1, c1),
                (center + dir1 * r0, c0),
            ] {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos,
                    uv: egui::epaint::WHITE_UV,
                    color,
                });
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// 圆形标记 —— `gui/colors/util.py:43-53` `draw_marker_circle()`：
///
/// ```python
/// cr.set_source_rgb(0, 0, 0); cr.set_line_width(size + 2)
/// cr.arc(x, y, (2 * size) + 0.5, 0, 2 * math.pi); cr.stroke_preserve()
/// cr.set_source_rgb(1, 1, 1); cr.set_line_width(size); cr.stroke()
/// ```
///
/// Cairo 的描边以路径为中心，所以是"黑 4px 圆环 + 白 2px 圆环"，同一个半径。
pub(crate) fn paint_marker_circle(painter: &egui::Painter, pos: egui::Pos2, size: f32) {
    let radius = 2.0 * size + 0.5;
    painter.circle_stroke(
        pos,
        radius,
        egui::Stroke::new(size + 2.0, egui::Color32::BLACK),
    );
    painter.circle_stroke(pos, radius, egui::Stroke::new(size, egui::Color32::WHITE));
}

/// HSV 轮的标记 —— `adjbases.py:1317-1326` 的 `paint_foreground_cb()`：
/// `draw_marker_circle(..., size=2)`，位置由 [`hsv_wheel_position_for_color`]
/// 给出（含 `distort_hue`）。
pub(crate) fn paint_wheel_marker(
    painter: &egui::Painter,
    rect: egui::Rect,
    managed: &Color,
    ranges: Option<&[pigment_engine::color::HueRange]>,
) {
    let geom = WheelGeometry::for_size(
        f64::from(rect.width()),
        f64::from(rect.height()),
        BORDER_WIDTH,
    );
    if !geom.is_usable() {
        return;
    }
    let (x, y) = hsv_wheel_position_for_color(&geom, managed, ranges);
    let clip = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + geom.center_x as f32,
            rect.top() + geom.center_y as f32,
        ),
        egui::vec2(2.0 * geom.radius as f32, 2.0 * geom.radius as f32),
    );
    let painter = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
    paint_marker_circle(
        &painter,
        rect.min + egui::vec2(x as f32, y as f32),
        2.0,
    );
}

/// HSV 方块外环的标记 —— `hsvsquare.py:285-310` `paint_foreground_cb()`：
///
/// ```python
/// col = HSVColor(color=self.get_managed_color()); col.s = 1.0
/// x, y = self.get_pos_for_color(col)
/// col.s = 0.70
/// ex, ey = self.get_pos_for_color(col)
/// 黑 5px → 白 3.5px → 当前色 0.25px 的三层线
/// ```
///
/// 也就是说外环的标记是**沿半径的一小段线**（从满饱和到 0.7 饱和），而不是
/// 圆点 —— 因为环只表示色相，标记要指向"哪个色相"，不表示饱和度。
///
/// egui 的 `line_segment` 没有圆头（Cairo 是 `LINE_CAP_ROUND`），线端形状略
/// 有差异（记账）。
pub(crate) fn paint_ring_marker(
    painter: &egui::Painter,
    rect: egui::Rect,
    managed: &Color,
    ranges: Option<&[pigment_engine::color::HueRange]>,
) {
    let geom = WheelGeometry::for_size(
        f64::from(rect.width()),
        f64::from(rect.height()),
        BORDER_WIDTH,
    );
    if !geom.is_usable() {
        return;
    }
    let mut col = HsvColor::from_color(managed);
    col.s = 1.0;
    let (x1, y1) = hsv_ring_position_for_color(&geom, &Color::Hsv(col), ranges);
    col.s = 0.70;
    let (x2, y2) = hsv_ring_position_for_color(&geom, &Color::Hsv(col), ranges);
    let p1 = rect.min + egui::vec2(x1 as f32, y1 as f32);
    let p2 = rect.min + egui::vec2(x2 as f32, y2 as f32);
    let clip = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + geom.center_x as f32,
            rect.top() + geom.center_y as f32,
        ),
        egui::vec2(2.0 * (geom.radius as f32 + 1.0), 2.0 * (geom.radius as f32 + 1.0)),
    );
    let painter = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
    for (width, color) in [
        (5.0_f32, egui::Color32::BLACK),
        (3.5, egui::Color32::WHITE),
        (0.25, rgb32(managed.get_rgb())),
    ] {
        painter.line_segment([p1, p2], egui::Stroke::new(width, color));
    }
}

/// 画平面切片（方块内块 / 立方切片）—— `_HSVSquareInnerSquare.render_background_cb()`
/// （`hsvsquare.py:331-375`）与 `HSVCubeSlice.render_background_cb()`
/// （`hsvcube.py:162-206`，两份相同）：
///
/// ```python
/// eff_wd = int(wd - 2*b); eff_ht = int(ht - 2*b)
/// step = max(1, int(eff_wd // 128))
/// for x in xrange(0, eff_wd, step):
///     amt = x / eff_wd
///     setattr(col, f1, amt)
///     setattr(col, f2, 1.0)        # 顶部
///     渐变 0 → col.get_rgb()
///     setattr(col, f2, 0.0)        # 底部
///     渐变 1 → col.get_rgb()
/// cr.rectangle(...); stroke OUTLINE
/// cr.set_source(slice_patt); paint()
/// cr.rectangle(...); stroke EDGE_HIGHLIGHT
/// ```
///
/// 每列是一条**竖向线性渐变**（顶 = f2 为 1、底 = f2 为 0），列与列之间按
/// 步长切分 —— egui 用一块网格（每个色标列两个顶点色）表达同一件事。
pub(crate) fn paint_slice(
    painter: &egui::Painter,
    rect: egui::Rect,
    managed: &Color,
    faces: [HsvFace; 3],
) {
    let b = BORDER_WIDTH as f32;
    let eff_wd = (rect.width() - 2.0 * b).trunc();
    let eff_ht = (rect.height() - 2.0 * b).trunc();
    if eff_wd < 1.0 || eff_ht < 1.0 {
        return;
    }
    let (f1, f2) = slice_faces(faces);
    let frame = egui::Rect::from_min_size(
        rect.min + egui::vec2(b + 0.5, b + 0.5),
        egui::vec2(eff_wd - 1.0, eff_ht - 1.0),
    );
    // 外框先画，随后被切片填充盖住内侧一半（源码顺序）
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(OUTLINE_WIDTH as f32, rgba32(OUTLINE_RGBA)),
        egui::StrokeKind::Middle,
    );

    let mut mesh = egui::Mesh::default();
    let step = slice_column_step(eff_wd);
    let mut x = 0.0_f32;
    while x < eff_wd {
        let w = step.min(eff_wd - x);
        let amt = f64::from(x) / f64::from(eff_wd);
        let mut top = HsvColor::from_color(managed);
        f1.set(&mut top, amt);
        f2.set(&mut top, 1.0);
        let mut bottom = HsvColor::from_color(managed);
        f1.set(&mut bottom, amt);
        f2.set(&mut bottom, 0.0);
        let (top_color, bottom_color) = (rgb32(top.get_rgb()), rgb32(bottom.get_rgb()));
        let left = rect.left() + b + x;
        let right = left + w;
        let (y0, y1) = (rect.top() + b, rect.top() + b + eff_ht);
        let base = mesh.vertices.len() as u32;
        for (pos, color) in [
            (egui::pos2(left, y0), top_color),
            (egui::pos2(right, y0), top_color),
            (egui::pos2(right, y1), bottom_color),
            (egui::pos2(left, y1), bottom_color),
        ] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        x += step;
    }
    painter.add(egui::Shape::mesh(mesh));

    // 高光最后画（源码 :371-375）
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(
            EDGE_HIGHLIGHT_WIDTH as f32,
            rgba32(EDGE_HIGHLIGHT_RGBA),
        ),
        egui::StrokeKind::Middle,
    );
}

/// 切片的标记 —— `hsvsquare.py:414-416` / `hsvcube.py:245-247`：
/// `draw_marker_circle(cr, x, y)`（默认 size = 2）。
pub(crate) fn paint_slice_marker(
    painter: &egui::Painter,
    rect: egui::Rect,
    managed: &Color,
    faces: [HsvFace; 3],
) {
    let b = BORDER_WIDTH as f32;
    if rect.width() <= 2.0 * b || rect.height() <= 2.0 * b {
        return;
    }
    let (x, y) = slice_position_for_color(
        managed,
        f64::from(rect.width()),
        f64::from(rect.height()),
        BORDER_WIDTH,
        faces,
    );
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    paint_marker_circle(
        &painter,
        rect.min + egui::vec2(x as f32, y as f32),
        2.0,
    );
}

/// 画竖直滑条 —— `SliderColorAdjuster.render_background_cb()`
/// （`adjbases.py:970-1009`）+ `paint_foreground_cb()`（`:1024-1053`）。
///
/// 几何与分量滑条页的横条**同一份代码**，差别只有两处（`:979-990`）：
/// - 渐变端点从 `(b,0)-(b+len,0)` 变成 `(0,b)-(0,b+len)`；
/// - 竖直条把每个色标的位置取反（`if self.vertical: p = 1 - p`），所以
///   "条值 1" 在顶部。
///
/// `samples` 是 `SliderColorAdjuster.samples`（值滑条 0，立方深度滑条 4，
/// `hsvcube.py:122`）；`color_for` 是子类的 `get_color_for_bar_amount()`。
pub(crate) fn paint_vertical_slider(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: usize,
    color_for: impl Fn(f64) -> [f64; 3],
    amount: f64,
    marker_rgb: [f64; 3],
) {
    let b = BORDER_WIDTH as f32;
    let bar_length = rect.height() - b - b;
    let b_w = rect.width() - b - b - 1.0;
    if bar_length <= 0.0 || b_w <= 0.0 {
        return;
    }
    let frame = egui::Rect::from_min_size(
        rect.min + egui::vec2(b + 0.5, b + 0.5),
        egui::vec2(b_w, bar_length - 1.0),
    );
    // 外框（:993-997）
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(OUTLINE_WIDTH as f32, rgba32(OUTLINE_RGBA)),
        egui::StrokeKind::Middle,
    );

    // 渐变（:983-990）：竖直条 stop = 1 - p
    let n = samples + 2;
    let mut points: Vec<(f32, egui::Color32)> = Vec::with_capacity(n + 1);
    for s in 0..=n {
        let p = s as f64 / n as f64;
        let color = rgb32(color_for(p));
        let stop = 1.0 - p;
        points.push((rect.top() + b + stop as f32 * bar_length, color));
    }
    let mut mesh = egui::Mesh::default();
    for pair in points.windows(2) {
        let ((y0, c0), (y1, c1)) = (pair[0], pair[1]);
        // windows 里 y0 > y1（s 增大则 stop 减小），交换成上→下
        let (top, bottom, ctop, cbottom) = if y0 <= y1 {
            (y0, y1, c0, c1)
        } else {
            (y1, y0, c1, c0)
        };
        let base = mesh.vertices.len() as u32;
        for (pos, color) in [
            (egui::pos2(rect.left() + b, top), ctop),
            (egui::pos2(rect.left() + b + b_w, top), ctop),
            (egui::pos2(rect.left() + b + b_w, bottom), cbottom),
            (egui::pos2(rect.left() + b, bottom), cbottom),
        ] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    painter.add(egui::Shape::mesh(mesh));

    // 内高光（:1004-1009）：只有条够宽/高才画
    if b_w > 5.0 && bar_length > 5.0 {
        painter.rect_stroke(
            frame,
            0.0,
            egui::Stroke::new(
                EDGE_HIGHLIGHT_WIDTH as f32,
                rgba32(EDGE_HIGHLIGHT_RGBA),
            ),
            egui::StrokeKind::Middle,
        );
    }

    // 标记线（:1024-1053）：黑 5px / 白 3.5px / 当前色 0.25px
    let y = rect.top() + vertical_marker_y(amount, rect.height(), b);
    let x0 = rect.left() + b + 0.5;
    let x1 = rect.right() - b - 0.5;
    if x1 <= x0 {
        return;
    }
    for (width, color) in [
        (5.0_f32, egui::Color32::BLACK),
        (3.5, egui::Color32::WHITE),
        (0.25, rgb32(marker_rgb)),
    ] {
        painter.add(egui::Shape::line_segment(
            [egui::pos2(x0, y), egui::pos2(x1, y)],
            egui::Stroke::new(width, color),
        ));
    }
}

// ---------------------------------------------------------------------------
// 立方页的 tooltip 文案（`hsvcube.py:44-49`）
// ---------------------------------------------------------------------------

/// 深度滑条的悬停提示 —— `hsvcube.py:44` 的 `_slider_tooltip_map`，按
/// `_faces[0]`（当前深度面）索引：
///
/// ```python
/// _slider_tooltip_map = dict(h=_("HSV Hue"), s=_("HSV Saturation"),
///                            v=_("HSV Value"))
/// ```
pub(crate) fn cube_slider_tooltip(face: HsvFace) -> &'static str {
    match face {
        HsvFace::H => "HSV Hue",
        HsvFace::S => "HSV Saturation",
        HsvFace::V => "HSV Value",
    }
}

/// 切片的悬停提示 —— `hsvcube.py:45-49` 的 `_slice_tooltip_map`：
///
/// ```python
/// _slice_tooltip_map = dict(h=_("HSV Saturation and Value"),
///                           s=_("HSV Hue and Value"),
///                           v=_("HSV Hue and Saturation"))
/// ```
pub(crate) fn cube_slice_tooltip(face: HsvFace) -> &'static str {
    match face {
        HsvFace::H => "HSV Saturation and Value",
        HsvFace::S => "HSV Hue and Value",
        HsvFace::V => "HSV Hue and Saturation",
    }
}

// ---------------------------------------------------------------------------
// 页面状态（每页一个 `ColorChanger`，对应源码每个控件一个 `ColorAdjusterWidget`）
// ---------------------------------------------------------------------------

/// 三个 HSV 页共用的可变状态。
///
/// 源码里每个控件（轮、外环、内方块、深度滑条、切片）都是独立的
/// `ColorAdjusterWidget`，各持一份 `__button_down` 等状态
/// （`adjbases.py:512-515`）。egui 是立即模式，这些状态只能存在应用里，
/// 所以按控件分成六个 [`ColorChanger`]，外加立方的面顺序。
pub(crate) struct WheelPagesState {
    /// HSV 轮（`hsvwheel.py:25-57`）。
    pub wheel: ColorChanger,
    /// HSV 轮页的竖直值滑条（`HSVValueSlider`，`sliders.py:242-252`）。
    pub wheel_value_slider: ColorChanger,
    /// HSV 方块的外环（`_HSVSquareOuterRing`，`hsvsquare.py:138-310`）。
    pub ring: ColorChanger,
    /// HSV 方块的内方块（`_HSVSquareInnerSquare`，`hsvsquare.py:313-416`）。
    pub square_slice: ColorChanger,
    /// HSV 立方的深度滑条（`HSVCubeSlider`，`hsvcube.py:118-142`）。
    pub cube_slider: ColorChanger,
    /// HSV 立方的切片（`HSVCubeSlice`，`hsvcube.py:145-247`）。
    pub cube_slice: ColorChanger,
    /// 立方的面顺序 —— `HSVCubePage._faces = ["h", "s", "v"]`
    /// （`hsvcube.py:52`），`tumble()` 左旋（`:100-105`）。
    pub cube_faces: [HsvFace; 3],
}

impl Default for WheelPagesState {
    fn default() -> Self {
        Self::new()
    }
}

impl WheelPagesState {
    /// 全部控件用同一个拖动阈值（`adjbases.py:528-532` 的默认值）。
    pub(crate) fn new() -> Self {
        Self {
            wheel: ColorChanger::new(DRAG_THRESHOLD),
            wheel_value_slider: ColorChanger::new(DRAG_THRESHOLD),
            ring: ColorChanger::new(DRAG_THRESHOLD),
            square_slice: ColorChanger::new(DRAG_THRESHOLD),
            cube_slider: ColorChanger::new(DRAG_THRESHOLD),
            cube_slice: ColorChanger::new(DRAG_THRESHOLD),
            cube_faces: [HsvFace::H, HsvFace::S, HsvFace::V],
        }
    }
}

// ---------------------------------------------------------------------------
// 三个页面
// ---------------------------------------------------------------------------

impl PigmentApp {
    /// **HSV Wheel 页** —— `HSVAdjusterPage`（`hsvwheel.py:60-102`）的页面体。
    ///
    /// 布局（`:72-77`）：左列竖直 `HSVValueSlider`、右列色轮，`Gtk.Table`
    /// 一格 3px padding。交互：轮上按下/拖动 = 极坐标取色（半径→饱和度、
    /// 角度→色相）；轮上滚轮 = 改明度（`hsvwheel.py:35-47`）；值滑条
    /// 按下/拖动/滚轮走通用滑条语义（`adjbases.py:1019-1083`）。
    pub(crate) fn color_hsv_wheel_page(&mut self, ui: &mut egui::Ui) {
        let size = page_size(ui, true);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let (slider_rect, wheel_rect) = wheel_page_rects(rect);
        let managed = self.color_manager.get_color();
        let ranges = self.color_manager.hue_ranges();
        let panel_fill = ui.visuals().panel_fill;
        let painter = ui.painter().with_clip_rect(ui.clip_rect());

        // ---- 绘制（本帧开始时的颜色）----
        paint_wheel_disc(
            &painter,
            wheel_rect,
            &managed,
            ranges,
            WheelStyle::Wheel,
            panel_fill,
        );
        paint_wheel_marker(&painter, wheel_rect, &managed, ranges);
        let value_slider = ComponentSlider::new(SliderKind::HsvV);
        paint_vertical_slider(
            &painter,
            slider_rect,
            0,
            |amt| value_slider.get_color_for_bar_amount(&managed, amt).get_rgb(),
            value_slider.get_bar_amount_for_color(&managed),
            managed.get_rgb(),
        );

        // ---- 悬停提示（`hsvwheel.py:28` 与 `sliders.py:243`）----
        let _ = ui
            .interact(
                wheel_rect,
                ui.scope_id().with("hsv_wheel_tip"),
                egui::Sense::hover(),
            )
            .on_hover_text("HSV Hue and Saturation");
        let _ = ui
            .interact(
                slider_rect,
                ui.scope_id().with("hsv_value_tip"),
                egui::Sense::hover(),
            )
            .on_hover_text("HSV Value");

        // ---- 指针 ----
        let frame = PointerFrame::read(ui);
        let geom = WheelGeometry::for_size(
            f64::from(wheel_rect.width()),
            f64::from(wheel_rect.height()),
            BORDER_WIDTH,
        );
        let wheel_color = drive_changer(
            &frame,
            &mut self.wheel_pages.wheel,
            &mut self.color_manager,
            wheel_rect.min,
            (f64::from(wheel_rect.width()), f64::from(wheel_rect.height())),
            |p| wheel_rect.contains(p),
            |p| {
                hsv_wheel_color_at_position(
                    &geom,
                    f64::from(p.x - wheel_rect.left()),
                    f64::from(p.y - wheel_rect.top()),
                    &managed,
                    ranges,
                )
                .map(Color::Hsv)
            },
        );
        let slider_ok = slider_rect.height() > 2.0 * BORDER_WIDTH as f32;
        let slider_color = if slider_ok {
            drive_changer(
                &frame,
                &mut self.wheel_pages.wheel_value_slider,
                &mut self.color_manager,
                slider_rect.min,
                (f64::from(slider_rect.width()), f64::from(slider_rect.height())),
                |p| slider_rect.contains(p),
                |p| {
                    let amt = point_to_amount(
                        f64::from(p.y - slider_rect.top()),
                        f64::from(slider_rect.height()),
                        BORDER_WIDTH,
                        true,
                    );
                    Some(value_slider.get_color_for_bar_amount(&managed, amt))
                },
            )
        } else {
            None
        };
        if let Some(color) = wheel_color.or(slider_color) {
            self.set_color(color);
        }

        // ---- 滚轮（`hsvwheel.py:35-47` / `adjbases.py:1069-1083`）----
        if let Some(direction) = scroll_direction(ui) {
            if ui.rect_contains_pointer(wheel_rect) {
                if let Some(color) = hsv_wheel_scroll(&managed, direction) {
                    self.set_color(color);
                }
            } else if slider_ok && ui.rect_contains_pointer(slider_rect) {
                let color = value_slider.scroll(&managed, true, direction);
                self.set_color(color);
            }
        }
    }

    /// **HSV Square 页** —— `HSVSquarePage`（`hsvsquare.py:33-100`）的页面体。
    ///
    /// 布局：`Gtk.AspectFrame` 强制正方（`:121-124`），外环铺满、内方块居中
    /// 占 54%（`:115-125`）。交互：内方块的按下事件先被它吃掉
    /// （`stop_fallthrough`，`:319-322`），所以命中判定优先内方块。
    pub(crate) fn color_hsv_square_page(&mut self, ui: &mut egui::Ui) {
        let size = page_size(ui, true);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let outer = square_rect(rect);
        let inner = inner_square_rect(outer);
        let managed = self.color_manager.get_color();
        let ranges = self.color_manager.hue_ranges();
        let panel_fill = ui.visuals().panel_fill;
        let painter = ui.painter().with_clip_rect(ui.clip_rect());
        let faces = [HsvFace::H, HsvFace::S, HsvFace::V];

        // ---- 绘制 ----
        paint_wheel_disc(
            &painter,
            outer,
            &managed,
            ranges,
            WheelStyle::Ring,
            panel_fill,
        );
        paint_ring_marker(&painter, outer, &managed, ranges);
        paint_slice(&painter, inner, &managed, faces);
        paint_slice_marker(&painter, inner, &managed, faces);

        // ---- 悬停提示（`hsvsquare.py:133-135` 的 `_update_tooltips()`）----
        let _ = ui
            .interact(inner, ui.scope_id().with("hsv_square_tip"), egui::Sense::hover())
            .on_hover_text("HSV Saturation and Value");
        let _ = ui
            .interact(outer, ui.scope_id().with("hsv_ring_tip"), egui::Sense::hover())
            .on_hover_text("HSV Hue");

        // ---- 指针（先内方块，后外环：`stop_fallthrough` 的语义）----
        let frame = PointerFrame::read(ui);
        let geom = WheelGeometry::for_size(
            f64::from(outer.width()),
            f64::from(outer.height()),
            BORDER_WIDTH,
        );
        let slice_color = drive_changer(
            &frame,
            &mut self.wheel_pages.square_slice,
            &mut self.color_manager,
            inner.min,
            (f64::from(inner.width()), f64::from(inner.height())),
            |p| square_hit_region(outer, inner, p) == Some(SquareRegion::Inner),
            |p| {
                slice_color_at_position(
                    &managed,
                    f64::from(p.x - inner.left()),
                    f64::from(p.y - inner.top()),
                    f64::from(inner.width()),
                    f64::from(inner.height()),
                    BORDER_WIDTH,
                    faces,
                )
                .map(Color::Hsv)
            },
        );
        let ring_color = drive_changer(
            &frame,
            &mut self.wheel_pages.ring,
            &mut self.color_manager,
            outer.min,
            (f64::from(outer.width()), f64::from(outer.height())),
            |p| square_hit_region(outer, inner, p) == Some(SquareRegion::Ring),
            |p| {
                hsv_ring_color_at_position(
                    &geom,
                    f64::from(p.x - outer.left()),
                    f64::from(p.y - outer.top()),
                    &managed,
                    ranges,
                )
                .map(Color::Hsv)
            },
        );
        if let Some(color) = slice_color.or(ring_color) {
            self.set_color(color);
        }

        // 源码这两个控件都没有 scroll-event 处理（`ColorAdjusterWidget` 基类
        // 不接滚轮，只有 `SliderColorAdjuster` 接），所以这里也不接。
    }

    /// **HSV Cube 页** —— `HSVCubePage`（`hsvcube.py:35-115`）的页面体。
    ///
    /// 布局（`:64-81`）：左列是竖直深度滑条（上）+ 旋转按钮（下），右列跨两行
    /// 是平面切片。交互：切片 = x→面1、y→面2；深度滑条 = 面0；按钮 =
    /// `tumble()`（`:100-105`）左旋面顺序，切片/滑条的提示跟着变
    /// （`:107-110`）。
    pub(crate) fn color_hsv_cube_page(&mut self, ui: &mut egui::Ui) {
        let size = page_size(ui, false);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let parts = cube_page_rects(rect);
        let managed = self.color_manager.get_color();
        let faces = self.wheel_pages.cube_faces;
        let face0 = faces[0];
        let painter = ui.painter().with_clip_rect(ui.clip_rect());

        // ---- 绘制 ----
        paint_slice(&painter, parts.slice, &managed, faces);
        paint_slice_marker(&painter, parts.slice, &managed, faces);
        paint_vertical_slider(
            &painter,
            parts.slider,
            CUBE_SLIDER_SAMPLES,
            |amt| depth_slider_color(&managed, face0, amt).get_rgb(),
            depth_slider_amount(&managed, face0),
            managed.get_rgb(),
        );

        // ---- 旋转按钮（`hsvcube.py:53-58`）----
        //
        // 源码是 `borderless_button(icon_name="mypaint-hsv-rotate-symbolic",
        // size=Gtk.IconSize.MENU, tooltip=_("Rotate cube (show different axes)"))`。
        // 图标未接（见模块文档），用 "⟳" 代替，提示逐字照抄。
        let button = ui.put(
            parts.button,
            egui::Button::new("⟳").small().frame(false),
        );
        if button
            .on_hover_text("Rotate cube (show different axes)")
            .clicked()
        {
            // `tumble()`（:100-105）：面顺序左旋一格
            tumble_faces(&mut self.wheel_pages.cube_faces);
        }

        // ---- 悬停提示（`:107-110` 的 `_update_tooltips()`）----
        let _ = ui
            .interact(
                parts.slice,
                ui.scope_id().with("hsv_cube_slice_tip"),
                egui::Sense::hover(),
            )
            .on_hover_text(cube_slice_tooltip(face0));
        let _ = ui
            .interact(
                parts.slider,
                ui.scope_id().with("hsv_cube_slider_tip"),
                egui::Sense::hover(),
            )
            .on_hover_text(cube_slider_tooltip(face0));

        // ---- 指针 ----
        let frame = PointerFrame::read(ui);
        let slice_color = drive_changer(
            &frame,
            &mut self.wheel_pages.cube_slice,
            &mut self.color_manager,
            parts.slice.min,
            (
                f64::from(parts.slice.width()),
                f64::from(parts.slice.height()),
            ),
            |p| parts.slice.contains(p),
            |p| {
                slice_color_at_position(
                    &managed,
                    f64::from(p.x - parts.slice.left()),
                    f64::from(p.y - parts.slice.top()),
                    f64::from(parts.slice.width()),
                    f64::from(parts.slice.height()),
                    BORDER_WIDTH,
                    faces,
                )
                .map(Color::Hsv)
            },
        );
        let slider_ok = parts.slider.height() > 2.0 * BORDER_WIDTH as f32;
        let slider_color = if slider_ok {
            drive_changer(
                &frame,
                &mut self.wheel_pages.cube_slider,
                &mut self.color_manager,
                parts.slider.min,
                (
                    f64::from(parts.slider.width()),
                    f64::from(parts.slider.height()),
                ),
                |p| parts.slider.contains(p),
                |p| {
                    let amt = point_to_amount(
                        f64::from(p.y - parts.slider.top()),
                        f64::from(parts.slider.height()),
                        BORDER_WIDTH,
                        true,
                    );
                    Some(depth_slider_color(&managed, face0, amt))
                },
            )
        } else {
            None
        };
        if let Some(color) = slice_color.or(slider_color) {
            self.set_color(color);
        }

        // ---- 深度滑条的滚轮（`hsvcube.py:118-142` 继承
        // `SliderColorAdjuster.__scroll_cb`，`adjbases.py:1069-1083`）----
        if slider_ok {
            if let Some(direction) = scroll_direction(ui) {
                if ui.rect_contains_pointer(parts.slider) {
                    let color = depth_slider_scroll(&managed, face0, true, direction);
                    self.set_color(color);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::color::{bend_hcy, HcyColor, RgbColor};

    fn rect(w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(w, h))
    }

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    /// `Gtk.AspectFrame` 的等价：宽扁/高瘦的可用区都取居中的正方形
    /// （`hsvsquare.py:121-124`、`hsvwheel.py:94-97`）。
    #[test]
    fn square_rect_centers_largest_square() {
        // 宽扁
        let r = rect(300.0, 100.0);
        let s = square_rect(r);
        assert!(approx(s.width(), 100.0, 1e-4));
        assert!(approx(s.height(), 100.0, 1e-4));
        assert!(approx(s.center().x, r.center().x, 1e-4));
        assert!(approx(s.center().y, r.center().y, 1e-4));
        // 高瘦
        let r = rect(80.0, 400.0);
        let s = square_rect(r);
        assert!(approx(s.width(), 80.0, 1e-4));
        assert!(approx(s.height(), 80.0, 1e-4));
    }

    /// 内方块 = 居中的 54% —— `hsvsquare.py:115-120`。
    #[test]
    fn inner_square_is_54_percent() {
        let outer = rect(200.0, 200.0);
        let inner = inner_square_rect(outer);
        assert!(approx(inner.width(), 108.0, 1e-4), "200 * 0.54");
        assert!(approx(inner.height(), 108.0, 1e-4));
        assert!(approx(inner.center().x, outer.center().x, 1e-4));
        assert!(approx(inner.center().y, outer.center().y, 1e-4));
    }

    /// HSV 轮页的两列：左列 18px（`uimisc.py:25`）、两格各 3px padding
    /// （`hsvwheel.py:76-77`）。
    #[test]
    fn wheel_page_columns_follow_table_padding() {
        let r = rect(300.0, 200.0);
        let (slider, wheel) = wheel_page_rects(r);
        assert!(approx(slider.left(), r.left() + 3.0, 1e-4));
        assert!(approx(slider.width(), 18.0, 1e-4));
        assert!(approx(slider.height(), 194.0, 1e-4), "上下各 3px");
        // 左列 3 + 18 + 3 + 3（相邻两格的 padding）
        assert!(approx(wheel.left(), r.left() + 27.0, 1e-4));
        assert!(approx(wheel.width(), 300.0 - 27.0 - 3.0, 1e-4));
        assert!(approx(wheel.height(), 194.0, 1e-4));
        // 退化：极窄时轮区宽度夹到 0，不出现负矩形
        let tiny = rect(20.0, 20.0);
        let (_, wheel) = wheel_page_rects(tiny);
        assert!(wheel.width() >= 0.0 && wheel.height() >= 0.0);
    }

    /// 立方页的三块 —— `hsvcube.py:64-81`。
    #[test]
    fn cube_page_grid_follows_table_padding() {
        let r = rect(300.0, 200.0);
        let p = cube_page_rects(r);
        // 左列：滑条在上（18 宽），按钮在下（22 高）
        assert!(approx(p.slider.left(), r.left() + 3.0, 1e-4));
        assert!(approx(p.slider.width(), 18.0, 1e-4));
        assert!(approx(p.slider.top(), r.top() + 3.0, 1e-4));
        assert!(approx(p.slider.height(), 200.0 - 6.0 - 22.0 - 6.0, 1e-4));
        assert!(approx(p.button.top(), r.top() + 3.0 + p.slider.height() + 6.0, 1e-4));
        assert!(approx(p.button.height(), CUBE_ROTATE_BUTTON_HEIGHT, 1e-4));
        // 右列跨两行
        assert!(approx(p.slice.left(), r.left() + 3.0 + 18.0 + 6.0, 1e-4));
        assert!(approx(p.slice.height(), 194.0, 1e-4));
        assert!(approx(p.slice.width(), 300.0 - 27.0 - 3.0, 1e-4));
        // 退化
        let tiny = rect(10.0, 10.0);
        let p = cube_page_rects(tiny);
        assert!(p.slice.width() >= 0.0 && p.slice.height() >= 0.0);
        assert_eq!(p.slider.height(), 0.0);
    }

    /// 逐列步长 —— `hsvsquare.py:340` / `hsvcube.py:171` 的
    /// `max(1, int(eff_wd // 128))`。
    #[test]
    fn slice_column_step_follows_source() {
        assert_eq!(slice_column_step(96.0), 1.0);
        assert_eq!(slice_column_step(127.0), 1.0);
        assert_eq!(slice_column_step(128.0), 1.0);
        assert_eq!(slice_column_step(129.0), 1.0);
        assert_eq!(slice_column_step(255.0), 1.0);
        assert_eq!(slice_column_step(256.0), 2.0);
        assert_eq!(slice_column_step(1000.0), 7.0);
        assert_eq!(slice_column_step(0.0), 1.0, "下限是 1");
    }

    /// 竖直滑条标记 —— `adjbases.py:1024-1038`（条值 1 在顶部）。
    #[test]
    fn vertical_slider_marker_follows_paint_foreground() {
        // 高 100、边框 2：bar_size = int(100 - 1 - 4) = 95
        assert_eq!(vertical_marker_y(0.0, 100.0, 2.0), 2.5 + 95.0);
        assert_eq!(vertical_marker_y(1.0, 100.0, 2.0), 2.5);
        // 中点：int(0.5 * 95) = 47 → 2 + 0.5 + 47
        assert_eq!(vertical_marker_y(0.5, 100.0, 2.0), 49.5);
        // 越界夹取（源码先 clamp 再取反）
        assert_eq!(vertical_marker_y(2.0, 100.0, 2.0), 2.5);
        assert_eq!(vertical_marker_y(-1.0, 100.0, 2.0), 97.5);
    }

    /// 命中：内方块优先，环 = 正方区减去内方块 ——
    /// `hsvsquare.py:126` + `:319-322`。
    #[test]
    fn square_hit_regions_prefer_inner_square() {
        let outer = rect(200.0, 200.0);
        let inner = inner_square_rect(outer);
        assert_eq!(
            square_hit_region(outer, inner, inner.center()),
            Some(SquareRegion::Inner)
        );
        // 内方块四角之外、正方区之内 = 环
        let corner = egui::pos2(inner.left() - 1.0, inner.top() - 1.0);
        assert_eq!(
            square_hit_region(outer, inner, corner),
            Some(SquareRegion::Ring)
        );
        // 正方区之外 = 无
        assert_eq!(
            square_hit_region(outer, inner, egui::pos2(5.0, 5.0)),
            None
        );
    }

    /// 按下按钮 1：立刻按位置落色 —— `adjbases.py:671-672`。
    #[test]
    fn primary_press_picks_wheel_color() {
        let mut manager = ColorManager::new();
        let start = HsvColor::new(0.4, 0.3, 0.7);
        manager.set_color(Color::Hsv(start));
        let mut changer = ColorChanger::new(DRAG_THRESHOLD);
        let geom = WheelGeometry::for_size(100.0, 100.0, BORDER_WIDTH);
        // 正下方、圆上：色相 0.25、饱和度 1
        let frame = PointerFrame {
            pos: Some(egui::pos2(50.0, 98.5)),
            pressed: [true, false, false],
            released: [false, false, false],
            modifiers: Modifiers::NONE,
        };
        let sampled = drive_changer(
            &frame,
            &mut changer,
            &mut manager,
            egui::pos2(0.0, 0.0),
            (100.0, 100.0),
            |_| true,
            |p| {
                hsv_wheel_color_at_position(
                    &geom,
                    f64::from(p.x),
                    f64::from(p.y),
                    &Color::Hsv(start),
                    None,
                )
                .map(Color::Hsv)
            },
        );
        let got = manager.get_color();
        assert_eq!(sampled.map(|c| c.repr()), Some(got.repr()));
        let hsv = HsvColor::from_color(&got);
        assert!(approx(hsv.h as f32, 0.25, 1e-6));
        assert!(approx(hsv.s as f32, 1.0, 1e-6));
        assert!(approx(hsv.v as f32, 0.7, 1e-6), "明度不变");
    }

    /// 按住移动：连续按位置取色 —— `adjbases.py:733-737`。
    #[test]
    fn drag_keeps_sampling_positions() {
        let mut manager = ColorManager::new();
        let start = HsvColor::new(0.4, 0.3, 0.7);
        manager.set_color(Color::Hsv(start));
        let mut changer = ColorChanger::new(DRAG_THRESHOLD);
        let geom = WheelGeometry::for_size(100.0, 100.0, BORDER_WIDTH);
        let sample = |p: egui::Pos2| {
            hsv_wheel_color_at_position(
                &geom,
                f64::from(p.x),
                f64::from(p.y),
                &Color::Hsv(start),
                None,
            )
            .map(Color::Hsv)
        };
        // 按下在正下方
        let press = PointerFrame {
            pos: Some(egui::pos2(50.0, 98.5)),
            pressed: [true, false, false],
            released: [false, false, false],
            modifiers: Modifiers::NONE,
        };
        drive_changer(&press, &mut changer, &mut manager, egui::pos2(0.0, 0.0), (100.0, 100.0), |_| true, sample);
        assert_eq!(HsvColor::from_color(&manager.get_color()).s, 1.0);
        // 拖到正右方（色相 0、圆上）
        let mv = PointerFrame {
            pos: Some(egui::pos2(98.5, 50.0)),
            pressed: [false, false, false],
            released: [false, false, false],
            modifiers: Modifiers::NONE,
        };
        drive_changer(&mv, &mut changer, &mut manager, egui::pos2(0.0, 0.0), (100.0, 100.0), |_| true, sample);
        let hsv = HsvColor::from_color(&manager.get_color());
        assert!(approx(hsv.h as f32, 0.0, 1e-6));
        assert!(approx(hsv.s as f32, 1.0, 1e-6));
    }

    /// 按钮 2 的按下**不**改色，拖动才做 HCY 弯曲 ——
    /// `adjbases.py:697-699` + `:739-788`（模型层 [`bend_hcy`] 的结果一致）。
    #[test]
    fn secondary_drag_bends_hcy_like_b2() {
        let mut manager = ColorManager::new();
        let start = Color::Rgb(RgbColor::new(0.2, 0.5, 0.8));
        manager.set_color(start);
        let before = manager.get_color();
        let mut changer = ColorChanger::new(DRAG_THRESHOLD);
        let press = PointerFrame {
            pos: Some(egui::pos2(50.0, 50.0)),
            pressed: [false, true, false],
            released: [false, false, false],
            modifiers: Modifiers::SHIFT,
        };
        drive_changer(
            &press,
            &mut changer,
            &mut manager,
            egui::pos2(0.0, 0.0),
            (100.0, 100.0),
            |_| true,
            |_| Some(start),
        );
        assert_eq!(
            manager.get_color().repr(),
            before.repr(),
            "按钮 2 按下只记起点（:697-699）"
        );
        // 拖动：Shift = 弯曲彩度（:756-758）
        let mv = PointerFrame {
            pos: Some(egui::pos2(60.0, 50.0)),
            pressed: [false, false, false],
            released: [false, false, false],
            modifiers: Modifiers::SHIFT,
        };
        drive_changer(
            &mv,
            &mut changer,
            &mut manager,
            egui::pos2(0.0, 0.0),
            (100.0, 100.0),
            |_| true,
            |_| Some(start),
        );
        let expect = bend_hcy(
            start,
            (50.0, 50.0),
            (60.0, 50.0),
            (100.0, 100.0),
            Modifiers::SHIFT,
            false,
        );
        assert_eq!(
            manager.get_color().repr(),
            Color::Hcy(expect).repr(),
            "与 B4-2 的 bend_hcy 逐位一致"
        );
        assert_eq!(
            manager.get_color().class_name(),
            "HCYColor",
            "弯曲结果总是 HCY（:747）"
        );
        // 顺便确认 HCY 值确实变了（不是原地不动）
        let hcy = HcyColor::from_color(&manager.get_color());
        assert!(hcy.c > 0.0);
    }

    /// 抬起清状态 —— `adjbases.py:793-808`。
    #[test]
    fn release_clears_changer_state() {
        let mut manager = ColorManager::new();
        let mut changer = ColorChanger::new(DRAG_THRESHOLD);
        let press = PointerFrame {
            pos: Some(egui::pos2(50.0, 50.0)),
            pressed: [true, false, false],
            released: [false, false, false],
            modifiers: Modifiers::NONE,
        };
        drive_changer(
            &press,
            &mut changer,
            &mut manager,
            egui::pos2(0.0, 0.0),
            (100.0, 100.0),
            |_| true,
            |_| Some(Color::Hsv(HsvColor::new(0.1, 0.2, 0.3))),
        );
        assert_eq!(changer.button_down(), Some(1));
        let release = PointerFrame {
            pos: Some(egui::pos2(50.0, 50.0)),
            pressed: [false, false, false],
            released: [true, false, false],
            modifiers: Modifiers::NONE,
        };
        drive_changer(
            &release,
            &mut changer,
            &mut manager,
            egui::pos2(0.0, 0.0),
            (100.0, 100.0),
            |_| true,
            |_| None,
        );
        assert_eq!(changer.button_down(), None);
        assert_eq!(changer.drag_start_pos(), None);
        assert_eq!(changer.drag_start_color(), None);
    }

    /// 绘制冒烟（不 panic、产出图形）—— 用无窗口的 `egui::Context` 跑一帧。
    #[test]
    fn wheel_painting_produces_shapes_without_a_window() {
        let ctx = egui::Context::default();
        let managed = Color::Hsv(HsvColor::new(0.4, 0.6, 0.8));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(300.0, 200.0), egui::Sense::hover());
            let (slider, wheel) = wheel_page_rects(rect);
            paint_wheel_disc(
                ui.painter(),
                wheel,
                &managed,
                None,
                WheelStyle::Wheel,
                ui.visuals().panel_fill,
            );
            paint_wheel_marker(ui.painter(), wheel, &managed, None);
            paint_vertical_slider(
                ui.painter(),
                slider,
                0,
                |amt| ComponentSlider::new(SliderKind::HsvV)
                    .get_color_for_bar_amount(&managed, amt)
                    .get_rgb(),
                0.8,
                managed.get_rgb(),
            );
        });
        out.textures_delta.clear();
        // 扇区网格 1 + 径向渐变网格 1 + 外框/高光/刻痕若干 + 滑条 3~5 个
        assert!(
            out.shapes.len() >= 8,
            "一帧应产出至少 8 个形状，实得 {}",
            out.shapes.len()
        );
    }

    /// HSV 方块页的绘制冒烟（环的挖洞 + 内方块 + 两个标记）。
    #[test]
    fn square_painting_produces_shapes_without_a_window() {
        let ctx = egui::Context::default();
        let managed = Color::Hsv(HsvColor::new(0.1, 0.5, 0.5));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 300.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(240.0, 240.0), egui::Sense::hover());
            let outer = square_rect(rect);
            let inner = inner_square_rect(outer);
            paint_wheel_disc(
                ui.painter(),
                outer,
                &managed,
                None,
                WheelStyle::Ring,
                ui.visuals().panel_fill,
            );
            paint_ring_marker(ui.painter(), outer, &managed, None);
            paint_slice(ui.painter(), inner, &managed, [HsvFace::H, HsvFace::S, HsvFace::V]);
            paint_slice_marker(
                ui.painter(),
                inner,
                &managed,
                [HsvFace::H, HsvFace::S, HsvFace::V],
            );
        });
        out.textures_delta.clear();
        assert!(out.shapes.len() >= 8, "实得 {}", out.shapes.len());
    }

    /// HSV 立方页的绘制冒烟（切片 + 深度滑条 + 标记）。
    #[test]
    fn cube_painting_produces_shapes_without_a_window() {
        let ctx = egui::Context::default();
        let managed = Color::Hsv(HsvColor::new(0.6, 0.4, 0.3));
        let faces = [HsvFace::H, HsvFace::S, HsvFace::V];
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 240.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(280.0, 220.0), egui::Sense::hover());
            let parts = cube_page_rects(rect);
            paint_slice(ui.painter(), parts.slice, &managed, faces);
            paint_slice_marker(ui.painter(), parts.slice, &managed, faces);
            paint_vertical_slider(
                ui.painter(),
                parts.slider,
                CUBE_SLIDER_SAMPLES,
                |amt| depth_slider_color(&managed, faces[0], amt).get_rgb(),
                depth_slider_amount(&managed, faces[0]),
                managed.get_rgb(),
            );
        });
        out.textures_delta.clear();
        assert!(out.shapes.len() >= 5, "实得 {}", out.shapes.len());
    }

    /// 立方页的 tooltip 映射逐字等于源码 —— `hsvcube.py:44-49`
    /// （fixture 由 `gen-wheel-vectors.py` 从源码的类属性求值而来）。
    #[test]
    fn cube_tooltips_match_source() {
        let path = format!(
            "{}/../pigment-engine/tests/fixtures/wheel_vectors.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("读不到 {path}：{e}（先跑 tests/tools/gen-wheel-vectors.py）")
        });
        let mut seen = 0;
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.first() != Some(&"wh_tooltip") {
                continue;
            }
            let face = HsvFace::from_name(f[1]).expect("fixture 面名");
            assert_eq!(cube_slider_tooltip(face), f[2], "face {}", f[1]);
            assert_eq!(cube_slice_tooltip(face), f[3], "face {}", f[1]);
            seen += 1;
        }
        assert_eq!(seen, 3, "fixture 应有 h/s/v 三条 tooltip");
    }
}
