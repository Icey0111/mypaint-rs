//! 颜色面板 —— 六个颜色器页的**容器**（B4-3 / B4-4a / B4-4b）。
//!
//! 源码里没有"颜色面板"这一个整体，而是**一页一个可停靠工具**
//! （`gui/colortools.py:31-101` 的 `_PageToolAdapter` 子类）：
//!
//! | 源码 | GType（动作名） | 页面类 |
//! |---|---|---|
//! | `gui/colortools.py:74-76` | `MyPaintHCYWheelTool`（`HCYWheelTool`） | `HCYAdjusterPage` |
//! | `gui/colortools.py:79-81` | `MyPaintHSVWheelTool`（`HSVWheelTool`） | `HSVAdjusterPage` |
//! | `gui/colortools.py:84-86` | `MyPaintPaletteTool`（`PaletteTool`） | `PalettePage` |
//! | `gui/colortools.py:89-91` | `MyPaintHSVCubeTool`（`HSVCubeTool`） | `HSVCubePage` |
//! | `gui/colortools.py:94-96` | `MyPaintHSVSquareTool`（`HSVSquareTool`） | `HSVSquarePage` |
//! | `gui/colortools.py:99-101` | `MyPaintComponentSlidersTool`（`ComponentSlidersTool`） | `ComponentSlidersAdjusterPage`（`gui/colors/sliders.py:25`） |
//!
//! 每个动作都是 `GtkToggleAction`（`gui/resources.xml:1392-1462`），activate 走
//! `toggle_window_cb` → `toggle_dockpanel_cb`（`gui/drawwindow.py:345-375`）：
//! active 就 `add_tool_widget`、取消就 `remove_tool_widget`。本文件承接**第六行**
//! 那一页（分量滑条），注册成 [`crate::shell::dock::PanelId::ComponentSlidersTool`]。
//!
//! ## 本块实现的控件
//!
//! - **分量滑条页**：`sliders.py:30-105` 的 `Gtk.Grid`（六行 R/G/B/H/C/Y）
//!   加上 `adjbases.py:933-1083` 的 `SliderColorAdjuster` 绘制与交互。
//!   条值 ↔ 颜色的算术**不在本文件**：那是 B4-2 的 [`ComponentSlider`] /
//!   [`point_to_amount`] / `scroll_delta`（引擎里已有 1788 条真源码向量门禁）。
//! - **当前 / 上一色**：`adjbases.py:854-930` 的 `PreviousCurrentColorAdjuster`。
//!   源码里这份控件装在底栏（`mypaint.glade:85` 的 `colorindicator1`，
//!   `gui/colorpreview.py:18` 的 `BrushColorIndicator` 是它的子类，只改了
//!   `HAS_DETAILS_DIALOG` 与点击行为）。面板里这一份是本项目的摆放（源码的
//!   颜色器页面**没有**这个控件），记账见交付说明。
//! - **HSV 三页**（B4-4b）：页面体（绘制 + 交互 + 布局）在
//!   [`super::wheel_pages`]，本文件只做页签与分派。
//!
//! ## 页面切换
//!
//! 源码里六个页面是六个独立的停靠工具，出现在 `_new_color_adjusters_menu()`
//! 的菜单里（`gui/colortools.py:183-194`）；工作区允许把它们放进同一个
//! `Gtk.Notebook`，于是看起来像"页签"（`gui/toolstack.py`）。
//!
//! B4-3 只移植了 Component Sliders 一个工具，面板里用一条**页签条**
//! （[`COLOR_PAGES`]）把其余五页列出来并**禁用**；B4-4a 把 Palette 页
//! （`gui/colors/paletteview.py`，见 [`super::palette_panel`]）接上，B4-4b 又把
//! HSV Wheel / HSV Square / HSV Cube 三页接上（`gui/colors/hsvwheel.py` /
//! `hsvsquare.py` / `hsvcube.py`，见 [`super::wheel_pages`]）。现在六个页签里
//! 五个可用、可切换（状态在 [`PigmentApp::color_page`]），只剩 HCY Wheel 一页
//! 禁用（B4-4c）。这仍然替代"注册六个独立面板"，记账见交付说明。
//!
//! ## 没接的（逐条记账，见交付说明）
//!
//! - `adjbases.py:536-601` 的 GTK DnD（颜色拖出/拖入）：需要 GTK 的
//!   `drag_begin_with_coordinates()` 与 `application/x-color` 选区。
//! - `adjbases.py:679-695` 的双击详情对话框（`gui/dialogs.py:ask_for_color`）：
//!   三个 HSV 页面的控件都没开 `HAS_DETAILS_DIALOG`（默认 False，
//!   `adjbases.py:454`），所以源码里也不会弹。
//! - `adjbases.py:739-788` 按钮 2/3 的 HCY 弯曲：**HSV 三页已接**
//!   （`shell/wheel_pages.rs` 的 [`super::wheel_pages::drive_changer`]，走 B4-2 的
//!   [`pigment_engine::color::ColorChanger`] / `bend_hcy`）；分量滑条页仍未接
//!   （滑条自身的按下/拖动/滚轮已接，弯曲记账）。
//! - `adjbases.py:640-656` / `sliders.py:166-305` 的 `get_background_validity()`：
//!   缓存重绘令牌；egui 每帧重画，没有这份缓存。
//! - `adjbases.py:414-419` 的 `get_prefs()`：偏好系统属 B6。

use eframe::egui;
use pigment_engine::color::{
    point_to_amount, Color, ColorManager, ComponentSlider, ScrollDirection, SliderKind, UIColor,
    BORDER_WIDTH,
};

use crate::app::PigmentApp;
use crate::shell::{
    ADJUSTER_EDGE_HIGHLIGHT_RGBA, ADJUSTER_EDGE_HIGHLIGHT_WIDTH, ADJUSTER_OUTLINE_RGBA,
    ADJUSTER_OUTLINE_WIDTH,
};

// ---------------------------------------------------------------------------
// 页面布局常量（`gui/colors/sliders.py`）
// ---------------------------------------------------------------------------

/// 页面网格四周的留白 —— `sliders.py:36` `grid.set_border_width(6)`。
pub(crate) const PAGE_BORDER_WIDTH: f32 = 6.0;

/// 行间距 —— `sliders.py:34` `grid.set_row_spacing(6)`。
pub(crate) const PAGE_ROW_SPACING: f32 = 6.0;

/// 页面最小宽度 —— `sliders.py:33` `grid.set_size_request(150, -1)`。
pub(crate) const PAGE_MIN_WIDTH: f32 = 150.0;

/// 单根滑条的高度 —— `sliders.py:87` `adj.set_size_request(100, 22)`。
///
/// 宽度那一半（100）只是下界：`adj.set_hexpand(True)`（`:89`）让滑条吃掉
/// 整行剩余宽度，所以这里只固定高度 22；页面自身的最小宽度见
/// [`PAGE_MIN_WIDTH`]（`:33`，含标签列）。
pub(crate) const SLIDER_HEIGHT: f32 = 22.0;

/// 标签 / 滑条的左右留白 —— `sliders.py:84-85`（标签）与 `:91-92`（滑条）
/// 都是 `margin_start/end = 3`。
pub(crate) const ROW_SIDE_MARGIN: f32 = 3.0;

/// H 行额外的上留白 —— `sliders.py:61`（`HCYHueSlider` 那一行的
/// `margin_top = 12`），把 RGB 组与 HCY 组隔开。
pub(crate) const HUE_ROW_MARGIN_TOP: f32 = 12.0;

/// 当前/上一色控件的宽度 —— `gui/mypaint.glade:85`
/// `colorindicator1` 的 `width_request = 96`。
///
/// 控件自己的申请只有 8×8（`adjbases.py:870-871`：`BORDER_WIDTH*2 + 4`），
/// 是 glade 把它撑成 1:5 左右的横条的。面板里没有 glade 的行高，高度取
/// [`SLIDER_HEIGHT`]（`sliders.py:87`）与滑条对齐 —— 本项目的摆放，记账。
pub(crate) const PREVIOUS_CURRENT_WIDTH: f32 = 96.0;

// 控件绘制用的 `ADJUSTER_*` 类常量（`ColorAdjusterWidget`，`adjbases.py:449-461`）
// 定义在 `shell/mod.rs`，与底栏的当前/上一色指示器共用 —— 源码里它们本来就是
// 同一个类的类常量。

// ---------------------------------------------------------------------------
// 页签表（`gui/colortools.py:183-194`）
// ---------------------------------------------------------------------------

/// 一个颜色器页签。
pub(crate) struct ColorPageDef {
    /// 页面对应的动作名（`resources.xml` 的 `GtkToggleAction` id）。
    ///
    /// 运行时渲染只用 [`Self::title`] / [`Self::description`]；这一列给动作
    /// 分派与测试用（`app.rs` 的 `HSVWheelTool` 等分支），与
    /// `shell/mod.rs` 的 `FOOTER_WIDGETS` 同样处置。
    #[allow(dead_code)]
    pub action: &'static str,
    /// 页面图标名 —— 页面类的 `get_page_icon_name()`。
    ///
    /// 页签条目前只画文字（egui 的 `Button` 不吃图标），这一列是**元数据**：
    /// 测试拿它核对 `resources.xml` 的动作表与 `assets/icons/` 里的图标，
    /// 后续页签要画图标时直接取用。运行时还没读，故标 `dead_code`。
    #[allow(dead_code)]
    pub icon_name: &'static str,
    /// 页面标题 —— 页面类的 `get_page_title()`（也是页签文字）。
    pub title: &'static str,
    /// 页面描述 —— 页面类的 `get_page_description()`（页签 tooltip 第二行）。
    pub description: &'static str,
    /// 本移植是否接上了这一页。
    pub ported: bool,
}

/// 六个颜色器页面，顺序照 `_new_color_adjusters_menu()` 的动作表
/// （`gui/colortools.py:183-194`）。
///
/// 注意源码里这是一份**菜单**（"Available color adjusters"，
/// `resources.xml:198-204` 的工厂动作），不是笔记本的页签顺序 —— 工作区
/// 里页签顺序取决于用户添加的顺序（`Workspace.add_tool_widget`）。这里借用
/// 菜单顺序，并在交付说明里记账。
pub(crate) static COLOR_PAGES: [ColorPageDef; 6] = [
    // gui/colors/hcywheel.py:1307-1322
    ColorPageDef {
        action: "HCYWheelTool",
        icon_name: "mypaint-tool-hcywheel",
        title: "HCY Wheel",
        description: "Set the color using cylindrical hue/chroma/luma space. \
                      The circular slices are equiluminant.",
        ported: false,
    },
    // gui/colors/hsvwheel.py:82-91
    ColorPageDef {
        action: "HSVWheelTool",
        icon_name: "mypaint-tool-hsvwheel",
        title: "HSV Wheel",
        description: "Saturation and Value color changer.",
        ported: true,
    },
    // gui/colors/paletteview.py:74-83
    ColorPageDef {
        action: "PaletteTool",
        icon_name: "mypaint-tool-color-palette",
        title: "Palette",
        description: "Set the color from a loadable, editable palette.",
        ported: true,
    },
    // gui/colors/hsvsquare.py:50-59
    ColorPageDef {
        action: "HSVSquareTool",
        icon_name: "mypaint-tool-hsvsquare",
        title: "HSV Square",
        description: "An HSV Square which can be rotated to show different hues.",
        ported: true,
    },
    // gui/colors/hsvcube.py:86-95
    ColorPageDef {
        action: "HSVCubeTool",
        icon_name: "mypaint-tool-hsvcube",
        title: "HSV Cube",
        description: "An HSV cube which can be rotated to show different planar slices.",
        ported: true,
    },
    // gui/colors/sliders.py:108-120
    ColorPageDef {
        action: "ComponentSlidersTool",
        icon_name: "mypaint-tool-component-sliders",
        title: "Component Sliders",
        description: "Adjust individual components of the color.",
        ported: true,
    },
];

// ---------------------------------------------------------------------------
// 滑条行表（`gui/colors/sliders.py:42-73`）
// ---------------------------------------------------------------------------

/// 分量滑条页的一行。
pub(crate) struct SliderRowDef {
    /// 标签文字 —— `sliders.py:44/49/54/59/64/69` 的 `C_()` 译文
    /// （i18n 上下文分别是 "color sliders panel: red/green/blue: slider label"
    /// 与 "color sliders panel: hue/chroma/luma: slider label"）。
    pub label: &'static str,
    /// 悬停提示 —— 滑条类的 `STATIC_TOOLTIP_TEXT`。
    ///
    /// `sliders.py:79` 把它同时挂在**标签**上，滑条在构造时自己也会设一份
    /// （`adjbases.py:525-526`）。
    pub tooltip: &'static str,
    /// 滑条种类（B4-2 的 [`SliderKind`]）。
    pub kind: SliderKind,
    /// 这一行的上留白 —— `sliders.py:83` / `:90` 的 `set_margin_top()`。
    pub margin_top: f32,
}

/// 六行，顺序即 `sliders.py:42-73` 的 `row_defs`。
///
/// 源码这一页**只摆六根**：R/G/B 用 `RGBRedSlider`/`RGBGreenSlider`/
/// `RGBBlueSlider`，H/C/Y 用 `HCYHueSlider`/`HCYChromaSlider`/`HCYLumaSlider`。
/// HSV 的三根（`HSVHueSlider` 等，`sliders.py:214-252`）**不在页面上**
/// （B4-2 的模块文档也记了这一条）。本移植照抄，不擅自加行。
pub(crate) static SLIDER_ROWS: [SliderRowDef; 6] = [
    // sliders.py:43-47
    SliderRowDef {
        label: "R",
        tooltip: "RGB Red",
        kind: SliderKind::RgbR,
        margin_top: 0.0,
    },
    // sliders.py:48-52
    SliderRowDef {
        label: "G",
        tooltip: "RGB Green",
        kind: SliderKind::RgbG,
        margin_top: 0.0,
    },
    // sliders.py:53-57
    SliderRowDef {
        label: "B",
        tooltip: "RGB Blue",
        kind: SliderKind::RgbB,
        margin_top: 0.0,
    },
    // sliders.py:58-62 —— 这一行 margin_top = 12
    SliderRowDef {
        label: "H",
        tooltip: "HCY Hue",
        kind: SliderKind::HcyH,
        margin_top: HUE_ROW_MARGIN_TOP,
    },
    // sliders.py:63-67
    SliderRowDef {
        label: "C",
        tooltip: "HCY Chroma",
        kind: SliderKind::HcyC,
        margin_top: 0.0,
    },
    // sliders.py:68-72
    SliderRowDef {
        label: "Y",
        tooltip: "HCY Luma (Y')",
        kind: SliderKind::HcyY,
        margin_top: 0.0,
    },
];

// ---------------------------------------------------------------------------
// 采样 / 渐变 / 标记线：纯函数（可脱离窗口测试）
// ---------------------------------------------------------------------------

/// `gui/colors/util.py:18-24` 的 `clamp(v, 0, 1)`，逐行照抄
/// （与 `f64::clamp` 的差别：NaN 原样返回、区间反了也不报错）。
fn clamp01(v: f64) -> f64 {
    if v > 1.0 {
        return 1.0;
    }
    if v < 0.0 {
        return 0.0;
    }
    v
}

/// 滑条的"额外采样数" —— `SliderColorAdjuster.samples`
/// （`adjbases.py:947` 默认 0，各子类覆盖）。
///
/// - `sliders.py:216` `HSVHueSlider.samples = 4`；
/// - `sliders.py:256` `HCYHueSlider.samples = 4`；
/// - `sliders.py:287-292` `HCYLumaSlider.samples` 是**按长度算的属性**：
///   `len = (vertical and height or width) - BORDER_WIDTH*2`，
///   `min(int(len // 3), 64)`。
///
/// `total_length` 传控件的总长度（含两侧边框）—— 与属性里扣边框之前的
/// `alloc.width` 一致。
pub(crate) fn slider_samples(kind: SliderKind, total_length: f32) -> usize {
    match kind {
        // 基类默认 0（adjbases.py:947）
        SliderKind::RgbR
        | SliderKind::RgbG
        | SliderKind::RgbB
        | SliderKind::HsvS
        | SliderKind::HsvV
        | SliderKind::HcyC => 0,
        // 色相条多采样（sliders.py:216 / :256）
        SliderKind::HsvH | SliderKind::HcyH => 4,
        // 亮度条（sliders.py:287-292）
        SliderKind::HcyY => {
            let len = f64::from(total_length) - 2.0 * BORDER_WIDTH;
            // Python 的 `int(len // 3)`：先向下取整再截断（正数时相同）
            let n = (len / 3.0).floor();
            if n <= 0.0 {
                // 源码这里会得到 <= 0 的 samples，后面 xrange 会炸；
                // 控件有最小尺寸（uimisc.py:25-26），真实调用走不到
                0
            } else {
                (n as usize).min(64)
            }
        }
    }
}

/// 渐变实际写入的**色标数** —— `adjbases.py:983-984`：
///
/// ```python
/// samples = self.samples + 2
/// for s in xrange(samples + 1):    # s = 0..samples（含）
/// ```
///
/// 即 `self.samples + 3` 个色标。默认 `samples = 0` 时只有 3 个
/// （0 / 0.5 / 1）；色相条 `samples = 4` 时 7 个。
pub(crate) fn gradient_stop_count(samples: usize) -> usize {
    samples + 3
}

/// 第 `stop` 个色标的条值 —— `adjbases.py:985` `p = s / samples`
/// （`samples` 是已经 +2 过的局部变量）。
///
/// 竖直条在 `:988-989` 会把 `p` 取反成 `1 - p`；本页六根都是横条
/// （`SliderColorAdjuster.vertical = False`，`adjbases.py:946`），
/// 所以这里只给横条的条值，竖直分支留给需要竖排滑条的块。
pub(crate) fn gradient_stop_amount(samples: usize, stop: usize) -> f64 {
    stop as f64 / (samples + 2) as f64
}

/// 渐变各色标的条值序列（测试与绘制共用）。
pub(crate) fn gradient_stop_amounts(samples: usize) -> Vec<f64> {
    (0..gradient_stop_count(samples))
        .map(|s| gradient_stop_amount(samples, s))
        .collect()
}

/// 渐变各色标的颜色（`adjbases.py:986-987`：`col.get_rgb()`）。
///
/// 逐点调用 B4-2 的 [`ComponentSlider::get_color_for_bar_amount`]，所以颜色
/// 完全由源码的"条值 → 分量"映射决定，本文件不碰色彩空间。
pub(crate) fn gradient_stop_colors(
    slider: &ComponentSlider,
    samples: usize,
    managed: &Color,
) -> Vec<[f64; 3]> {
    gradient_stop_amounts(samples)
        .into_iter()
        .map(|p| {
            slider
                .get_color_for_bar_amount(managed, p)
                .get_rgb()
        })
        .collect()
}

/// 把一串色标铺成**横向**渐变网格（`adjbases.py:983-990` 的
/// `cairo.LinearGradient` + `add_color_stop_rgb`）。
///
/// `fill` 是最终填充的矩形，`(x0, x1)` 是渐变端点（`adjbases.py:982`：
/// `LinearGradient(b, 0, b + bar_length, 0)`）。色标按条值 `p` **均匀**落在
/// `[x0, x1]` 上，颜色在相邻色标之间线性插值 —— 与 Cairo 的行为一致。
///
/// 源码的填充矩形（`:1001`）比渐变两端各差半像素（左边多 0.5、右边少 0.5），
/// Cairo 默认 `EXTEND_PAD` 用端点色补齐，这里也补上端点顶点。
pub(crate) fn gradient_mesh(
    fill: egui::Rect,
    x0: f32,
    x1: f32,
    stops: &[egui::Color32],
) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    if stops.is_empty() {
        return mesh;
    }
    let n = stops.len();
    let span = x1 - x0;
    // (横坐标, 颜色) 序列：先按色标位置生成，再按需补端点
    let mut points: Vec<(f32, egui::Color32)> = Vec::with_capacity(n + 2);
    if fill.left() < x0 {
        points.push((fill.left(), stops[0]));
    }
    for (i, color) in stops.iter().enumerate() {
        let t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
        points.push((x0 + span * t, *color));
    }
    if n == 1 {
        // 只有一个色标 = 纯色（Cairo 同样会用这一个色标铺满整段）
        points.push((x1, stops[0]));
    }
    if fill.right() > x1 {
        points.push((fill.right(), stops[n - 1]));
    }

    let (top, bottom) = (fill.top(), fill.bottom());
    for w in points.windows(2) {
        let ((xa, ca), (xb, cb)) = (w[0], w[1]);
        if xb <= xa {
            continue;
        }
        let base = mesh.vertices.len() as u32;
        // 四个顶点：左上、右上、右下、左下（颜色沿横向插值）
        for (pos, color) in [
            (egui::pos2(xa, top), ca),
            (egui::pos2(xb, top), cb),
            (egui::pos2(xb, bottom), cb),
            (egui::pos2(xa, bottom), ca),
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
    mesh
}

/// 本帧的滚轮事件 → 源码的滚动方向（`Gdk.ScrollDirection`，`adjbases.py:1073-1077`）。
///
/// GTK 每个 `scroll-event` 固定走一格 `SCROLL_DELTA`（`:1070`）；egui 的滚轮
/// 是 `Event::MouseWheel` 的连续量，这里只取**方向**，把"一格 = 一次事件"
/// 的语义保住。取位移较大的那个轴（`Vec2::UP` 为正 = 向上滚）。
pub(crate) fn wheel_direction(delta: egui::Vec2) -> Option<ScrollDirection> {
    if delta.y.abs() >= delta.x.abs() {
        if delta.y > 0.0 {
            Some(ScrollDirection::Up)
        } else if delta.y < 0.0 {
            Some(ScrollDirection::Down)
        } else {
            None
        }
    } else if delta.x > 0.0 {
        Some(ScrollDirection::Right)
    } else {
        Some(ScrollDirection::Left)
    }
}

/// 指针位置 → 条值 —— `SliderColorAdjuster.point_to_amount()`
/// （`adjbases.py:1055-1067`）。
///
/// 把控件矩形换算成源码用的两个量（长度 = 宽度/高度、位置 = 相对控件的
/// `x`/`y`），再交给 B4-2 的 [`point_to_amount`]（含边框扣减、夹取、
/// 竖直反向）。
pub(crate) fn amount_at(
    rect: egui::Rect,
    pos: egui::Pos2,
    vertical: bool,
    border: f32,
) -> f64 {
    let length = f64::from(if vertical { rect.height() } else { rect.width() });
    let local = f64::from(if vertical {
        pos.y - rect.top()
    } else {
        pos.x - rect.left()
    });
    point_to_amount(local, length, f64::from(border), vertical)
}

/// 标记线的横坐标（控件坐标）—— `adjbases.py:1024-1038`：
///
/// ```python
/// amt = float(clamp(amt, 0, 1))                  # :1028
/// bar_size = int((vertical and ht or wd) - 1 - 2 * b)   # :1029
/// x1 = x2 = int(amt * bar_size) + b + 0.5        # :1036
/// ```
///
/// 返回**控件坐标**（0 = 控件左边缘），调用方自己加 `rect.left()`。
pub(crate) fn marker_x(amount: f64, width: f32, border: f32) -> f32 {
    let amt = clamp01(amount);
    let bar_size = (f64::from(width) - 1.0 - 2.0 * f64::from(border)).trunc();
    (amt * bar_size).trunc() as f32 + border + 0.5
}

// ---------------------------------------------------------------------------
// 当前 / 上一色（`adjbases.py:854-930`）
// ---------------------------------------------------------------------------

/// 指针在控件的哪一半 —— `PreviousCurrentColorAdjuster.get_color_at_position`
/// （`adjbases.py:918-925`）：`if x < alloc.width // 2` 取当前色，否则上一色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColorHalf {
    /// 左半 = 当前色（`mgr.get_color()`）。
    Current,
    /// 右半 = 上一色（`mgr.get_previous_color()`）。
    Previous,
}

/// 两半的填充矩形 —— `adjbases.py:892-897`：
///
/// ```python
/// cr.rectangle(b, b, int(eff_wd // 2), eff_ht)              # 当前色
/// cr.rectangle(wd // 2, b, eff_wd - int(eff_wd // 2), eff_ht)  # 上一色
/// ```
///
/// 注意右半的起点是 `wd // 2`，**不是** `b + int(eff_wd // 2)` —— 两个表达式
/// 在偶数宽度下相等、奇数宽度下差 1px。源码就是这么写的，照抄。
pub(crate) fn previous_current_halves(
    rect: egui::Rect,
    border: f32,
) -> (egui::Rect, egui::Rect) {
    let b = border;
    let eff_wd = rect.width() - 2.0 * b;
    let eff_ht = rect.height() - 2.0 * b;
    // int(eff_wd // 2)：正的 eff_wd 下 floor 与截断相同
    let half = ((eff_wd as i32) / 2) as f32;
    // wd // 2
    let mid = ((rect.width() as i32) / 2) as f32;
    let left = egui::Rect::from_min_size(rect.min + egui::vec2(b, b), egui::vec2(half, eff_ht));
    let right = egui::Rect::from_min_size(
        egui::pos2(rect.left() + mid, rect.top() + b),
        egui::vec2(eff_wd - half, eff_ht),
    );
    (left, right)
}

/// 指针落在哪半 —— `adjbases.py:921-924`（整数除法 `alloc.width // 2`）。
pub(crate) fn hit_half(rect: egui::Rect, pos: egui::Pos2) -> ColorHalf {
    let mid = ((rect.width() as i32) / 2) as f32;
    if pos.x - rect.left() < mid {
        ColorHalf::Current
    } else {
        ColorHalf::Previous
    }
}

/// 某半区对应的颜色 —— `get_color_at_position()` 的两个分支
/// （`adjbases.py:921-925`，`deepcopy` 在 Rust 里就是 `Color` 的 `Copy`）。
pub(crate) fn half_color(manager: &ColorManager, half: ColorHalf) -> Color {
    match half {
        ColorHalf::Current => manager.get_color(),
        ColorHalf::Previous => manager.get_previous_color(),
    }
}

/// 画当前/上一色两半 + 边框 —— `adjbases.py:875-903`。
///
/// 顺序照抄：先描 3px 的 `OUTLINE_RGBA` 外框（`:886-890`），再填两半
/// （`:892-897`），最后描 1px 的 `EDGE_HIGHLIGHT_RGBA` 内高光（`:899-903`）；
/// 三步都按 `BORDER_WIDTH = 2` 内缩。
pub(crate) fn paint_previous_current(
    painter: &egui::Painter,
    rect: egui::Rect,
    current: [f64; 3],
    previous: [f64; 3],
) {
    let b = BORDER_WIDTH as f32;
    let eff_wd = rect.width() - 2.0 * b;
    let eff_ht = rect.height() - 2.0 * b;
    if eff_wd <= 0.0 || eff_ht <= 0.0 {
        return;
    }
    let (left, right) = previous_current_halves(rect, b);
    // 描边矩形：rectangle(b + 0.5, b + 0.5, eff_wd - 1, eff_ht - 1)（:886）
    let frame = egui::Rect::from_min_size(
        rect.min + egui::vec2(b + 0.5, b + 0.5),
        egui::vec2(eff_wd - 1.0, eff_ht - 1.0),
    );
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(ADJUSTER_OUTLINE_WIDTH, rgba32(ADJUSTER_OUTLINE_RGBA)),
        egui::StrokeKind::Middle,
    );
    painter.rect_filled(left, 0.0, rgb32(current));
    painter.rect_filled(right, 0.0, rgb32(previous));
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(ADJUSTER_EDGE_HIGHLIGHT_WIDTH, rgba32(ADJUSTER_EDGE_HIGHLIGHT_RGBA)),
        egui::StrokeKind::Middle,
    );
}

// ---------------------------------------------------------------------------
// 颜色转换（显示用）
// ---------------------------------------------------------------------------

/// `[f64; 3]` 的 0..1 分量 → egui 颜色。
///
/// **技术栈适配**：Cairo 用浮点源色（`set_source_rgb`），egui 只有 8 位色。
/// 这里四舍五入到最近的 1/255 —— 注意与 `to_hex_str()` 的**截断**
/// （`lib/color.py:205-208`）不同，本函数只用于屏幕显示，不参与颜色语义。
pub(crate) fn rgb32(rgb: [f64; 3]) -> egui::Color32 {
    fn q(c: f64) -> u8 {
        (c.clamp(0.0, 1.0) * 255.0).round() as u8
    }
    egui::Color32::from_rgb(q(rgb[0]), q(rgb[1]), q(rgb[2]))
}

fn rgba32(rgba: [f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (rgba[0] * 255.0).round() as u8,
        (rgba[1] * 255.0).round() as u8,
        (rgba[2] * 255.0).round() as u8,
        (rgba[3] * 255.0).round() as u8,
    )
}

// ---------------------------------------------------------------------------
// 绘制
// ---------------------------------------------------------------------------

/// 画一根分量滑条 —— `SliderColorAdjuster.render_background_cb()`
/// （`adjbases.py:970-1009`）+ `paint_foreground_cb()`（`:1024-1053`）。
///
/// 几何逐条对应（`b = BORDER_WIDTH = 2`）：
///
/// ```python
/// bar_length = wd - b - b                                   # :972
/// b_x = b + 0.5; b_y = b + 0.5                              # :973-974
/// b_w = wd - b - b - 1; b_h = ht - b - b - 1                 # :975-976
/// bar_gradient = LinearGradient(b, 0, b + bar_length, 0)     # :982
/// samples = self.samples + 2;  s in 0..=samples              # :983-984
/// p = s / samples; col = get_color_for_bar_amount(p)         # :985-986
/// cr.rectangle(b_x, b_y, b_w, b_h); stroke OUTLINE           # :996-997
/// cr.rectangle(b_x-0.5, b_y-0.5, b_w+1, b_h+1); fill         # :1001-1002
/// if b_w > 5 and b_h > 5: stroke EDGE_HIGHLIGHT              # :1005-1009
/// ```
///
/// 绘制顺序也照抄：**先描外框、再填渐变、最后描内高光**（所以外框被填充盖住
/// 内侧一半），标记线最后画（`paint_foreground_cb` 是 GTK 的 foreground 层）。
pub(crate) fn paint_slider(
    painter: &egui::Painter,
    rect: egui::Rect,
    kind: SliderKind,
    managed: &Color,
) {
    let b = BORDER_WIDTH as f32;
    let width = rect.width();
    let height = rect.height();
    if width <= 2.0 * b || height <= 2.0 * b {
        return;
    }
    let slider = ComponentSlider::new(kind);
    let bar_length = width - b - b;
    let (b_x, b_y) = (b + 0.5, b + 0.5);
    let (b_w, b_h) = (width - b - b - 1.0, height - b - b - 1.0);

    // ---- 渐变色标（:983-990）----
    let samples = slider_samples(kind, width);
    let stops: Vec<egui::Color32> = gradient_stop_colors(&slider, samples, managed)
        .into_iter()
        .map(rgb32)
        .collect();

    // ---- 外框（:993-997）----
    let frame = egui::Rect::from_min_size(
        rect.min + egui::vec2(b_x, b_y),
        egui::vec2(b_w, b_h),
    );
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(ADJUSTER_OUTLINE_WIDTH, rgba32(ADJUSTER_OUTLINE_RGBA)),
        egui::StrokeKind::Middle,
    );

    // ---- 渐变填充（:999-1002）----
    let fill = egui::Rect::from_min_size(
        rect.min + egui::vec2(b_x - 0.5, b_y - 0.5),
        egui::vec2(b_w + 1.0, b_h + 1.0),
    );
    let x0 = rect.left() + b;
    let x1 = x0 + bar_length;
    painter.add(egui::Shape::mesh(gradient_mesh(fill, x0, x1, &stops)));

    // ---- 内高光（:1004-1009）：只有条足够宽/高才画 ----
    if b_w > 5.0 && b_h > 5.0 {
        painter.rect_stroke(
            frame,
            0.0,
            egui::Stroke::new(ADJUSTER_EDGE_HIGHLIGHT_WIDTH, rgba32(ADJUSTER_EDGE_HIGHLIGHT_RGBA)),
            egui::StrokeKind::Middle,
        );
    }

    // ---- 标记线（:1024-1053）----
    //
    // 三层同一条竖线：黑 5px、白 3.5px、当前色 0.25px（:1040-1053）。
    // Cairo 用 `LINE_CAP_ROUND`，圆头会向外多出半个线宽；egui 的圆角矩形
    // 把圆头含在矩形内，所以这里把高度加一个线宽、圆角取半宽来复现同样的
    // 可见范围。
    let amt = slider.get_bar_amount_for_color(managed);
    let x = rect.left() + marker_x(amt, width, b);
    let y1 = rect.top() + b + 0.5;
    let y2 = rect.bottom() - b - 0.5;
    let len = y2 - y1;
    if len <= 0.0 {
        return;
    }
    let mid_y = (y1 + y2) * 0.5;
    for (w, color) in [
        (5.0_f32, egui::Color32::BLACK),
        (3.5, egui::Color32::WHITE),
        (0.25, rgb32(managed.get_rgb())),
    ] {
        painter.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(x, mid_y),
                egui::vec2(w, len + w),
            ),
            w * 0.5,
            color,
        );
    }
}

// ---------------------------------------------------------------------------
// 页面索引（[`COLOR_PAGES`] 的下标）
// ---------------------------------------------------------------------------

/// 分量滑条页在 [`COLOR_PAGES`] 里的下标（`sliders.py:108-120`）。
pub(crate) const PAGE_COMPONENT_SLIDERS: usize = 5;

/// 调色板页在 [`COLOR_PAGES`] 里的下标（`paletteview.py:74-83`）。
pub(crate) const PAGE_PALETTE: usize = 2;

/// HSV 轮页在 [`COLOR_PAGES`] 里的下标（`hsvwheel.py:82-91`）。
pub(crate) const PAGE_HSV_WHEEL: usize = 1;

/// HSV 方块页在 [`COLOR_PAGES`] 里的下标（`hsvsquare.py:50-59`）。
pub(crate) const PAGE_HSV_SQUARE: usize = 3;

/// HSV 立方页在 [`COLOR_PAGES`] 里的下标（`hsvcube.py:86-95`）。
pub(crate) const PAGE_HSV_CUBE: usize = 4;

// ---------------------------------------------------------------------------
// 面板
// ---------------------------------------------------------------------------

impl PigmentApp {
    /// 面板主入口（`dock.rs` 的 `PanelId::ComponentSlidersTool` 分支调用）。
    pub(crate) fn color_panel_content(&mut self, ui: &mut egui::Ui) {
        // ---- 页签条（工作区笔记本标签的等价物）----
        self.color_page_tabs(ui);
        // GTK notebook 的页与标签之间有边框，这里用一条分隔线表达
        ui.separator();
        // ---- 页面体 ----
        match self.color_page {
            // 调色板页（B4-4a）：`get_page_widget()` 返回的就是 PaletteView 自己，
            // 页面没有额外留白（`paletteview.py:84-88`）
            PAGE_PALETTE => self.color_palette_page(ui),
            // HSV 三页（B4-4b）：布局与留白由各自页面自己的 `Gtk.Table`
            // attach padding 决定（`hsvwheel.py:76-77`、`hsvcube.py:69-81`）
            PAGE_HSV_WHEEL => self.color_hsv_wheel_page(ui),
            PAGE_HSV_SQUARE => self.color_hsv_square_page(ui),
            PAGE_HSV_CUBE => self.color_hsv_cube_page(ui),
            // 其余（当前只有分量滑条）都按 `grid.set_border_width(6)` 留白
            _ => self.color_sliders_page(ui),
        }
    }

    /// 分量滑条页的页面体（B4-3）：`grid.set_border_width(6)`（`sliders.py:36`）。
    fn color_sliders_page(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .inner_margin(egui::Margin::same(PAGE_BORDER_WIDTH as i8))
            .show(ui, |ui| {
                // 当前 / 上一色（源码里这份控件在底栏，见模块文档）
                self.color_panel_previous_current(ui);
                ui.add_space(PAGE_ROW_SPACING);
                // 六行分量滑条
                self.color_panel_sliders(ui);
            });
    }

    /// 页签条 —— 六个颜色器页面（[`COLOR_PAGES`]），已移植的两页可切换。
    ///
    /// 未移植的页签画成禁用按钮并带悬停说明：GTK 里它们各自是独立的
    /// 停靠工具（`gui/colortools.py:74-101`），本块没有注册成面板。
    fn color_page_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (index, page) in COLOR_PAGES.iter().enumerate() {
                if page.ported {
                    // 当前页：`selectable_label` 的选中态 = 笔记本的活动页
                    let selected = self.color_page == index;
                    let resp = ui.selectable_label(selected, page.title);
                    let resp =
                        resp.on_hover_text(format!("{}\n{}", page.title, page.description));
                    if resp.clicked() {
                        self.color_page = index;
                    }
                } else {
                    // 禁用态：egui 的 `on_hover_text` 对 disabled 控件不弹
                    // （`Response::hovered()` 只在 enabled 时置位），要用
                    // `on_disabled_hover_text`。
                    let resp = ui.add_enabled(false, egui::Button::new(page.title).small());
                    let _ = resp.on_disabled_hover_text(format!(
                        "{}\n{}\n未移植：{} 页留给后续块（见交付记账）",
                        page.title, page.description, page.title
                    ));
                }
            }
        });
    }

    /// 当前 / 上一色一行 —— `PreviousCurrentColorAdjuster`（`adjbases.py:854-930`）。
    ///
    /// 交互语义照源码：**按下**时用 `get_color_at_position()` 取色并
    /// `set_managed_color()`（`:671-672` 的三条支路里按钮 1 那一条），
    /// 于是按右半 = 恢复上一色、按左半 = 保持当前色（`:918-925`）。
    /// `HAS_DETAILS_DIALOG = True`（`:861`）时的双击详情对话框未移植
    /// （`gui/dialogs.py` 的 GTK 对话框），这里只落一条状态栏说明。
    fn color_panel_previous_current(&mut self, ui: &mut egui::Ui) {
        let w = PREVIOUS_CURRENT_WIDTH
            .min(ui.available_width())
            .max(1.0);
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(w, SLIDER_HEIGHT),
            egui::Sense::click(),
        );
        let current = self.color_manager.get_color();
        let previous = self.color_manager.get_previous_color();
        paint_previous_current(
            ui.painter(),
            rect,
            current.get_rgb(),
            previous.get_rgb(),
        );
        let tip = format!(
            "{}\n左半 = 当前色 {}\n右半 = 上一色 {}（按下即恢复）",
            crate::shell::TIP_COLOR_INDICATOR,
            current.to_hex_str(),
            previous.to_hex_str(),
        );
        let resp = resp.on_hover_text(tip);

        // 按下即动作（`:671-672`）：`is_pointer_button_down_on` 在按下的那一帧
        // 就是 true，配合 `primary_pressed()` 把它限制成"按下这一帧"。
        let pressed_now =
            resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed());
        if pressed_now {
            if let Some(pos) = resp.interact_pointer_pos() {
                // 源码无条件 `set_managed_color(get_color_at_position(x, y))`：
                // 左半取当前色（管理器会因相等短路，`:213-214`），
                // 右半取上一色 → 恢复。本应用的 `set_color` 会重推笔刷。
                let color = half_color(&self.color_manager, hit_half(rect, pos));
                self.set_color(color);
            }
        }
        // 双击详情对话框（`adjbases.py:679-695`）—— 未移植，给出原因
        if resp.double_clicked() {
            let now = ui.input(|i| i.time);
            self.set_status(
                "未移植：颜色详情对话框（adjbases.py:679-695 的 HAS_DETAILS_DIALOG）".to_string(),
                now,
            );
        }
    }

    /// 六行分量滑条 —— `ComponentSlidersAdjusterPage.__init__`
    /// （`sliders.py:30-105`）。
    ///
    /// 布局：`row_spacing = 6`（`:34`），H 行 `margin_top = 12`（`:61`），
    /// 每行 = 标签（`margin 3`，`valign 0.0` 顶部对齐）+ 滑条
    /// （`hexpand=True`，左右各 3px 留白，`:86-93`）。
    fn color_panel_sliders(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(PAGE_MIN_WIDTH);
        // 行距完全交给源码的 `row_spacing = 6`（`sliders.py:34`）：
        // 把 egui 的默认 item_spacing 归零，免得 6 变成 6 + 默认值。
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, row) in SLIDER_ROWS.iter().enumerate() {
            if i > 0 {
                ui.add_space(PAGE_ROW_SPACING);
            }
            if row.margin_top > 0.0 {
                ui.add_space(row.margin_top);
            }
            self.component_slider(ui, row);
        }
    }

    /// 一根分量滑条：布局 + 绘制 + 交互。
    ///
    /// 交互照 `ColorAdjusterWidget` 的指针处理（`adjbases.py:660-808`）：
    /// - 按钮 1 **按下即设色**（`:671-672`），按住移动继续按位置取色
    ///   （`:733-737`，滑条不是 drag source）；
    /// - 滚轮一格 ±`SCROLL_DELTA` 并夹在 0..1（`:1069-1083`）。
    ///
    /// 按钮 2/3 的 HCY 弯曲（`:739-788`）与 DnD（`:536-601`）没接，见模块文档。
    fn component_slider(&mut self, ui: &mut egui::Ui, row: &SliderRowDef) {
        let managed = self.color_manager.get_color();
        let slider = ComponentSlider::new(row.kind);
        // 本帧要落的颜色（闭包里不碰 `self`，避免借两次）
        let mut new_color: Option<Color> = None;

        ui.horizontal_top(|ui| {
            // 横向间距同样全部按源码的 margin 走：归零 item_spacing，只留显式留白
            ui.spacing_mut().item_spacing.x = 0.0;
            // 标签的 margin_start = 3（sliders.py:84）
            ui.add_space(ROW_SIDE_MARGIN);
            // 标签：`label.set_valign(0.0)`（:82）→ 顶部对齐，`horizontal_top` 即此
            let label = ui.add(egui::Label::new(row.label).selectable(false));
            let _ = label.on_hover_text(row.tooltip);
            // 标签的 margin_end = 3（:85）+ 滑条的 margin_left = 3（:91）
            ui.add_space(ROW_SIDE_MARGIN * 2.0);

            // 滑条（`set_hexpand(True)` + `set_size_request(100, 22)`，:87-89）；
            // 右侧再留 3px（`set_margin_right(3)`，:92）
            let w = (ui.available_width() - ROW_SIDE_MARGIN).max(1.0);
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(w, SLIDER_HEIGHT),
                egui::Sense::click_and_drag(),
            );
            // 指针悬停/拖动时的提示：滑条类的 STATIC_TOOLTIP_TEXT
            // （`adjbases.py:525-526`）
            let resp = resp.on_hover_text(row.tooltip);

            // 先画（用本帧开始时的颜色），再处理交互 —— 与 GTK 的
            // "事件改颜色 → queue_draw → 下一帧画" 同构，本帧的视觉滞后一帧
            // 无所谓（egui 拖动时会自动持续重绘）。
            paint_slider(ui.painter(), rect, row.kind, &managed);

            // ---- 按下 / 按住拖动（adjbases.py:671-672 + :733-737）----
            //
            // 宽度不足"两侧边框"时 `point_to_amount` 会除零（源码同处会抛
            // `ZeroDivisionError`）；控件有最小尺寸（`uimisc.py:24-26` 的
            // `SLIDER_MIN_WIDTH/LENGTH`），这里只是兜底不 panic。
            let bar_ok = rect.width() > 2.0 * BORDER_WIDTH as f32;
            if bar_ok && resp.is_pointer_button_down_on() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let amt = amount_at(rect, pos, false, BORDER_WIDTH as f32);
                    new_color = Some(slider.get_color_for_bar_amount(&managed, amt));
                }
            }

            // ---- 滚轮（adjbases.py:1069-1083）----
            //
            // GTK 的每个 scroll-event 固定走一格 SCROLL_DELTA；egui 把滚轮折算成
            // 一串 `Event::MouseWheel`，这里取本帧**第一个**事件的方向，一个方向
            // 只走一格（不按滚动量缩放）。
            //
            // 记账：面板内容在外层 `ScrollArea` 里，内容超出可视高度时这一次
            // 滚轮也会被 ScrollArea 消费（GTK 里事件被滑条吃掉、不传给滚动容器）。
            if resp.hovered() {
                let delta = ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } => Some(*delta),
                        _ => None,
                    })
                });
                if let Some(dir) = delta.and_then(wheel_direction) {
                    // 横条：`scroll_delta()` 会先取反（`:1071-1072`），与源码一致
                    new_color = Some(slider.scroll(&managed, false, dir));
                }
            }
        });

        if let Some(color) = new_color {
            // 管理器自己会在"颜色没变"时短路（adjbases.py:213-214）；
            // 变了就重推笔刷（BrushColorManager.set_color 的语义）。
            self.set_color(color);
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::color::{hsv_to_rgb, HcyColor, HsvColor, RgbColor, UIColor};

    fn rect(w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(w, h))
    }

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    /// 滑条行表 = `sliders.py:42-73` 的 `row_defs`：六行、标签、提示、
    /// H 行的 12px 上留白、种类顺序。
    #[test]
    fn slider_rows_match_sliders_py() {
        assert_eq!(SLIDER_ROWS.len(), 6, "页面只摆六根（RGB 三根 + HCY 三根）");
        let labels: Vec<&str> = SLIDER_ROWS.iter().map(|r| r.label).collect();
        assert_eq!(labels, ["R", "G", "B", "H", "C", "Y"]);
        let tips: Vec<&str> = SLIDER_ROWS.iter().map(|r| r.tooltip).collect();
        assert_eq!(
            tips,
            ["RGB Red", "RGB Green", "RGB Blue", "HCY Hue", "HCY Chroma", "HCY Luma (Y')"]
        );
        let kinds: Vec<SliderKind> = SLIDER_ROWS.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                SliderKind::RgbR,
                SliderKind::RgbG,
                SliderKind::RgbB,
                SliderKind::HcyH,
                SliderKind::HcyC,
                SliderKind::HcyY,
            ]
        );
        // 只有 H 行有 margin_top = 12（sliders.py:61）
        let margins: Vec<f32> = SLIDER_ROWS.iter().map(|r| r.margin_top).collect();
        assert_eq!(margins, [0.0, 0.0, 0.0, 12.0, 0.0, 0.0]);
        // HSV 的三根不在页面上（sliders.py:214-252 只定义类）
        assert!(!kinds.contains(&SliderKind::HsvH));
        assert!(!kinds.contains(&SliderKind::HsvS));
        assert!(!kinds.contains(&SliderKind::HsvV));
    }

    /// 页签表：六个页面、顺序照 `colortools.py:183-194`、只有 Sliders 页可用。
    #[test]
    fn page_tabs_follow_colortools_menu_order() {
        let titles: Vec<&str> = COLOR_PAGES.iter().map(|p| p.title).collect();
        assert_eq!(
            titles,
            [
                "HCY Wheel",
                "HSV Wheel",
                "Palette",
                "HSV Square",
                "HSV Cube",
                "Component Sliders",
            ]
        );
        let actions: Vec<&str> = COLOR_PAGES.iter().map(|p| p.action).collect();
        assert_eq!(
            actions,
            [
                "HCYWheelTool",
                "HSVWheelTool",
                "PaletteTool",
                "HSVSquareTool",
                "HSVCubeTool",
                "ComponentSlidersTool",
            ]
        );
        let ported: Vec<&str> = COLOR_PAGES
            .iter()
            .filter(|p| p.ported)
            .map(|p| p.title)
            .collect();
        assert_eq!(
            ported,
            ["HSV Wheel", "Palette", "HSV Square", "HSV Cube", "Component Sliders"],
            "B4-4b 起只剩 HCY Wheel 一页禁用（B4-4c）"
        );
        // 每个页面都有描述（页签 tooltip 的第二行）
        assert!(COLOR_PAGES.iter().all(|p| !p.description.is_empty()));
        // 页面对应的动作都在 B2 的动作表里（resources.xml 生成）
        for p in COLOR_PAGES.iter() {
            assert!(
                crate::shell::actions::by_id(p.action).is_some(),
                "{} 不在动作表里",
                p.action
            );
        }
    }

    /// 五个已移植页面的图标名 = 源码的 `get_page_icon_name()`，且与动作表一致。
    ///
    /// 图标文件：`mypaint-tool-hsvwheel` / `mypaint-tool-hsvcube` /
    /// `mypaint-tool-component-sliders` / `mypaint-tool-color-palette` 都在
    /// `assets/icons/`（`build.rs` 栅格化）；`mypaint-tool-hsvsquare` 在源码里
    /// **只有 PNG**（`desktop/icons/hicolor/*/actions/`，没有 scalable SVG），
    /// 本项目没搬那个二进制，所以只断言名字 —— 记账见交付说明。
    #[test]
    fn page_icons_match_source_page_classes() {
        let icons: Vec<&str> = COLOR_PAGES.iter().map(|p| p.icon_name).collect();
        assert_eq!(
            icons,
            [
                "mypaint-tool-hcywheel",
                "mypaint-tool-hsvwheel",
                "mypaint-tool-color-palette",
                "mypaint-tool-hsvsquare",
                "mypaint-tool-hsvcube",
                "mypaint-tool-component-sliders",
            ],
            "顺序同 COLOR_PAGES（get_page_icon_name 的返回值）"
        );
        // 动作表的 icon-name 与页面元数据必须一致（resources.xml 是同一份数据）
        for p in COLOR_PAGES.iter() {
            let action = crate::shell::actions::by_id(p.action).expect("动作应在表里");
            assert_eq!(action.icon_name, Some(p.icon_name), "{}", p.action);
        }
        for icon in [
            "mypaint-tool-hsvwheel",
            "mypaint-tool-hsvcube",
            "mypaint-tool-color-palette",
            "mypaint-tool-component-sliders",
        ] {
            assert!(
                crate::shell::toolbars::icon_png_exists(icon),
                "缺图标文件 {icon}"
            );
        }
        // hsvsquare 的图标资源在源码里只有 PNG，本项目未搬（记账）
        assert!(!crate::shell::toolbars::icon_png_exists("mypaint-tool-hsvsquare"));
    }

    /// 页签下标常量与 `COLOR_PAGES` 一一对应（B4-4b 接三页时最易错的地方）。
    #[test]
    fn page_indices_match_color_pages() {
        for (index, expect) in [
            (PAGE_HSV_WHEEL, "HSV Wheel"),
            (PAGE_PALETTE, "Palette"),
            (PAGE_HSV_SQUARE, "HSV Square"),
            (PAGE_HSV_CUBE, "HSV Cube"),
            (PAGE_COMPONENT_SLIDERS, "Component Sliders"),
        ] {
            assert_eq!(COLOR_PAGES[index].title, expect);
            assert!(COLOR_PAGES[index].ported, "{expect} 应可用");
        }
        assert!(!COLOR_PAGES[0].ported, "HCY Wheel 留给 B4-4c");
    }

    /// 面板 id 的注册与元数据 —— `sliders.py:107-120` + `resources.xml:1433-1438`。
    #[test]
    fn panel_id_metadata_matches_component_sliders_tool() {
        use crate::shell::dock::{DockState, PanelId};
        let p = PanelId::ComponentSlidersTool;
        assert_eq!(p.title(), "Component Sliders");
        assert_eq!(p.icon_name(), "mypaint-tool-component-sliders");
        assert_eq!(p.description(), "Adjust individual components of the color.");
        // 图标文件随工具条资源一起栅格化（`assets/icons/`）
        assert!(
            crate::shell::toolbars::icon_png_exists("mypaint-tool-component-sliders"),
            "缺图标文件 mypaint-tool-component-sliders"
        );
        // 动作表里的 toggle 动作（resources.xml:1433）
        let action = crate::shell::actions::by_id("ComponentSlidersTool")
            .expect("ComponentSlidersTool 应在动作表里");
        assert_eq!(action.icon_name, Some("mypaint-tool-component-sliders"));
        // 停靠状态：能加、能切、能移除（toggle_dockpanel_cb 的语义）
        let mut dock = DockState::default();
        assert!(!dock.is_added(p));
        dock.toggle(p);
        assert!(dock.is_added(p));
        dock.toggle(p);
        assert!(!dock.is_added(p));
    }

    /// `point_to_amount` 的边界：用 egui 的 `Rect` 直接算（不需要窗口）。
    #[test]
    fn point_to_amount_boundaries_through_rect() {
        // 横条：宽 100，边框 2 → 有效长度 96
        let r = rect(100.0, SLIDER_HEIGHT);
        let y = r.center().y;
        let at = |x: f32| amount_at(r, egui::pos2(x, y), false, 2.0);
        assert_eq!(at(r.left()), 0.0, "左边缘（扣边框后夹到 0）");
        assert_eq!(at(r.left() + 2.0), 0.0, "边框内侧仍是 0");
        assert!(approx(at(r.left() + 50.0), 0.5, 1e-12), "中点 0.5");
        assert_eq!(at(r.right() - 2.0), 1.0, "右边缘内侧是 1");
        assert_eq!(at(r.right()), 1.0, "右边缘夹到 1");
        assert_eq!(at(r.right() + 500.0), 1.0, "越界夹到 1");
        // 竖直条：GTK 的 y 向下，条值向上增长（adjbases.py:1065-1066）
        let v = rect(SLIDER_HEIGHT, 100.0);
        let x = v.center().x;
        let vat = |y0: f32| amount_at(v, egui::pos2(x, y0), true, 2.0);
        assert_eq!(vat(v.top()), 1.0, "顶部 = 1");
        assert_eq!(vat(v.bottom()), 0.0, "底部 = 0");
        assert!(approx(vat(v.center().y), 0.5, 1e-12));
    }

    /// 条值 → 颜色 → 条值的往返（用引擎的 `ComponentSlider`）。
    ///
    /// 覆盖页面上的六根与其余三个 HSV 类（源码里存在但不在页面上）。
    #[test]
    fn bar_amount_round_trips_through_component_slider() {
        for kind in SliderKind::ALL {
            let slider = ComponentSlider::new(kind);
            let mut managed = Color::Rgb(RgbColor::new(0.3, 0.6, 0.8));
            for amt in [0.0, 0.2, 0.5, 0.75, 1.0] {
                let color = slider.get_color_for_bar_amount(&managed, amt);
                let back = slider.get_bar_amount_for_color(&color);
                assert!(
                    approx(back, amt, 1e-9),
                    "{}：条值 {amt} 往返成了 {back}",
                    kind.class_name()
                );
                // 写回的颜色类型跟着滑条所在的色彩空间变
                assert_eq!(color.class_name(), kind.color_class_name());
                managed = color;
            }
        }
    }

    /// 滑条只改自己那一个分量，其余分量保持（`sliders.py:171-174` 等）。
    #[test]
    fn component_slider_only_touches_its_own_channel() {
        let managed = Color::Rgb(RgbColor::new(0.3, 0.6, 0.8));
        // RGB 红：只改 r
        let c = ComponentSlider::new(SliderKind::RgbR).get_color_for_bar_amount(&managed, 0.25);
        let rgb = c.get_rgb();
        assert!(approx(rgb[0], 0.25, 1e-12));
        assert!(approx(rgb[1], 0.6, 1e-12));
        assert!(approx(rgb[2], 0.8, 1e-12));
        // HCY 彩度：只改 c（h/y 保持），且 **不夹到 0..1**（源码也不夹）
        let hcy0 = HcyColor::from_color(&managed);
        let c = ComponentSlider::new(SliderKind::HcyC).get_color_for_bar_amount(&managed, 0.5);
        let Color::Hcy(hcy) = c else {
            panic!("HCY 彩度条应回写 HCYColor");
        };
        assert!(approx(hcy.c, 0.5, 1e-12));
        assert!(approx(hcy.h, hcy0.h, 1e-12));
        assert!(approx(hcy.y, hcy0.y, 1e-12));
        // HSV 色相：只改 h
        let hsv0 = HsvColor::from_color(&managed);
        let c = ComponentSlider::new(SliderKind::HsvH).get_color_for_bar_amount(&managed, 0.125);
        let Color::Hsv(hsv) = c else {
            panic!("HSV 色相条应回写 HSVColor");
        };
        assert!(approx(hsv.h, 0.125, 1e-12));
        assert!(approx(hsv.s, hsv0.s, 1e-12));
        assert!(approx(hsv.v, hsv0.v, 1e-12));
    }

    /// 采样数与色标数 —— `adjbases.py:947/983-984` + `sliders.py:216/256/287-292`。
    #[test]
    fn gradient_sample_counts_match_source() {
        // 默认 samples = 0（基类），色标 0 / 0.5 / 1 三个
        assert_eq!(slider_samples(SliderKind::RgbR, 100.0), 0);
        assert_eq!(gradient_stop_count(0), 3);
        // 色相条 samples = 4 → 7 个色标
        assert_eq!(slider_samples(SliderKind::HcyH, 100.0), 4);
        assert_eq!(slider_samples(SliderKind::HsvH, 100.0), 4);
        assert_eq!(gradient_stop_count(4), 7);
        // 亮度条：min(int((len - 2*BORDER) // 3), 64)
        assert_eq!(slider_samples(SliderKind::HcyY, 100.0), 32, "(100-4)//3");
        assert_eq!(slider_samples(SliderKind::HcyY, 22.0), 6, "(22-4)//3");
        assert_eq!(slider_samples(SliderKind::HcyY, 300.0), 64, "上限 64");
        assert_eq!(gradient_stop_count(32), 35);
        // 色标位置均匀铺在 0..=1（p = s / (samples + 2)）
        let amounts = gradient_stop_amounts(4);
        assert_eq!(amounts.len(), 7);
        assert!(approx(amounts[0], 0.0, 1e-12));
        assert!(approx(amounts[6], 1.0, 1e-12));
        for (s, a) in amounts.iter().enumerate() {
            assert!(approx(*a, s as f64 / 6.0, 1e-12), "第 {s} 个色标");
        }
        let amounts = gradient_stop_amounts(0);
        assert_eq!(amounts, vec![0.0, 0.5, 1.0]);
    }

    /// 色标颜色 = 条值取色的采样（`adjbases.py:986-987`）。
    #[test]
    fn gradient_stop_colors_are_samples_of_the_bar() {
        // RGB 红条：三个色标的 r 分量就是 0 / 0.5 / 1
        let managed = Color::Rgb(RgbColor::new(0.0, 0.5, 0.25));
        let slider = ComponentSlider::new(SliderKind::RgbR);
        let colors = gradient_stop_colors(&slider, 0, &managed);
        assert_eq!(colors.len(), 3);
        for (i, rgb) in colors.iter().enumerate() {
            assert!(approx(rgb[0], i as f64 / 2.0, 1e-12), "第 {i} 个色标的 r");
            assert!(approx(rgb[1], 0.5, 1e-12));
            assert!(approx(rgb[2], 0.25, 1e-12));
        }
        // HCY 色相条：7 个色标，h = s/6，颜色 = hcy_to_rgb(h, c, y)
        let managed = Color::Hcy(HcyColor::new(0.5, 0.4, 0.6));
        let slider = ComponentSlider::new(SliderKind::HcyH);
        let colors = gradient_stop_colors(&slider, 4, &managed);
        assert_eq!(colors.len(), 7);
        for (s, rgb) in colors.iter().enumerate() {
            let want = HcyColor::new(s as f64 / 6.0, 0.4, 0.6).get_rgb();
            for c in 0..3 {
                assert!(
                    approx(rgb[c], want[c], 1e-12),
                    "第 {s} 个色标分量 {c}：{} != {}",
                    rgb[c],
                    want[c]
                );
            }
        }
        // HSV 色相条的颜色与 hsv_to_rgb 一致（h 决定色相）
        let managed = Color::Hsv(HsvColor::new(0.0, 0.5, 0.75));
        let slider = ComponentSlider::new(SliderKind::HsvH);
        let colors = gradient_stop_colors(&slider, 4, &managed);
        for (s, rgb) in colors.iter().enumerate() {
            let want = hsv_to_rgb([s as f64 / 6.0, 0.5, 0.75]);
            for c in 0..3 {
                assert!(approx(rgb[c], want[c], 1e-12));
            }
        }
    }

    /// 渐变网格：色标顶点位置/颜色正确，端点用 `EXTEND_PAD` 补齐
    /// （`adjbases.py:982` 的渐变端点 + `:1001` 的填充矩形）。
    #[test]
    fn gradient_mesh_places_stops_and_pads_ends() {
        let stops = [
            egui::Color32::from_rgb(0, 0, 0),
            egui::Color32::from_rgb(128, 0, 0),
            egui::Color32::from_rgb(255, 0, 0),
        ];
        // 填充矩形 = 渐变范围（22..118）→ 不需要补端点顶点
        let fill = egui::Rect::from_min_size(egui::pos2(22.0, 20.0), egui::vec2(96.0, 18.0));
        let mesh = gradient_mesh(fill, 22.0, 118.0, &stops);
        // 3 个色标 → 2 段 → 8 个顶点 / 12 个索引
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.indices.len(), 12);
        // 色标按 0 / 0.5 / 1 落在 22 / 70 / 118（p = s / (samples + 2)）
        let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.pos.x).collect();
        assert_eq!(xs, vec![22.0, 70.0, 70.0, 22.0, 70.0, 118.0, 118.0, 70.0]);
        // 每段左右顶点的颜色 = 相邻两个色标
        assert_eq!(mesh.vertices[0].color, stops[0]);
        assert_eq!(mesh.vertices[1].color, stops[1]);
        assert_eq!(mesh.vertices[4].color, stops[1]);
        assert_eq!(mesh.vertices[5].color, stops[2]);
        // 上下两端贴住填充矩形
        assert_eq!(mesh.vertices[0].pos.y, 20.0);
        assert_eq!(mesh.vertices[2].pos.y, 38.0);

        // 填充矩形比渐变宽（左边多 0.5、右边多 0.5，照 :1001 的半像素）时：
        // Cairo 的 EXTEND_PAD 用端点色补齐
        let wide = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(100.0, 18.0));
        let mesh = gradient_mesh(wide, 22.0, 118.0, &stops);
        let min_x = mesh.vertices.iter().map(|v| v.pos.x).fold(f32::MAX, f32::min);
        let max_x = mesh.vertices.iter().map(|v| v.pos.x).fold(f32::MIN, f32::max);
        assert_eq!(min_x, 20.0, "左端补齐到填充矩形左边缘");
        assert_eq!(max_x, 120.0, "右端补齐到填充矩形右边缘");
        let first = &mesh.vertices[0];
        let last = mesh.vertices.last().unwrap();
        assert_eq!(first.color, stops[0], "左补点用左端色");
        assert_eq!(last.color, stops[2], "右补点用右端色");
    }

    /// 滚轮方向 → `Gdk.ScrollDirection`（`adjbases.py:1069-1083` 的第一步）。
    #[test]
    fn wheel_direction_maps_to_gdk_scroll_direction() {
        assert_eq!(
            wheel_direction(egui::vec2(0.0, 15.0)),
            Some(ScrollDirection::Up)
        );
        assert_eq!(
            wheel_direction(egui::vec2(0.0, -15.0)),
            Some(ScrollDirection::Down)
        );
        // 横向滚轮：左/右
        assert_eq!(
            wheel_direction(egui::vec2(10.0, 2.0)),
            Some(ScrollDirection::Right)
        );
        assert_eq!(
            wheel_direction(egui::vec2(-10.0, 2.0)),
            Some(ScrollDirection::Left)
        );
        // 没有滚动
        assert_eq!(wheel_direction(egui::Vec2::ZERO), None);
        // 换算成条值增量：横条 `scroll_delta` 先取反（:1071-1072），
        // 再对 DOWN/LEFT 取反（:1073-1077）→ 向上滚 = 条值变小
        use pigment_engine::color::scroll_delta;
        assert!(approx(scroll_delta(false, ScrollDirection::Up), -0.015, 1e-12));
        assert!(approx(scroll_delta(false, ScrollDirection::Down), 0.015, 1e-12));
        assert!(approx(scroll_delta(false, ScrollDirection::Left), 0.015, 1e-12));
    }

    /// 标记线位置 —— `adjbases.py:1028-1038`，含"HCY 彩度只能到一半"的夹取。
    #[test]
    fn marker_position_follows_paint_foreground_cb() {
        // 宽 100、边框 2：bar_size = int(100 - 1 - 4) = 95
        assert_eq!(marker_x(0.0, 100.0, 2.0), 2.5);
        assert_eq!(marker_x(0.5, 100.0, 2.0), 49.5, "int(47.5)=47 → +2+0.5");
        assert_eq!(marker_x(1.0, 100.0, 2.0), 97.5);
        // 超过 1 的条值（HCY 彩度的合法域到 2）被夹到右端 —— 源码 :1028
        let hcy = Color::Hcy(HcyColor::new(0.0, 1.5, 0.5));
        let amt = ComponentSlider::new(SliderKind::HcyC).get_bar_amount_for_color(&hcy);
        assert!(approx(amt, 1.5, 1e-12));
        assert_eq!(marker_x(amt, 100.0, 2.0), 97.5, "参考值先夹到 1");
        // 负值夹到左端
        assert_eq!(marker_x(-0.25, 100.0, 2.0), 2.5);
    }

    /// 绘制冒烟：在没有窗口/GPU 的 `egui::Context` 里跑一帧，滑条与当前/上一色
    /// 都要产出图形且不 panic。
    ///
    /// 这是唯一一条真跑绘制路径的测试（`Context::run_ui` 只做布局与形状生成，
    /// 不需要 surface），能挡住"渐变网格空、尺寸退化成 0、除零"之类的崩溃。
    #[test]
    fn painting_produces_shapes_without_a_window() {
        let ctx = egui::Context::default();
        let managed = Color::Rgb(RgbColor::new(0.2, 0.5, 0.7));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(120.0, SLIDER_HEIGHT), egui::Sense::hover());
            for kind in [SliderKind::RgbR, SliderKind::HcyH, SliderKind::HcyY] {
                paint_slider(ui.painter(), rect, kind, &managed);
            }
            let (indicator, _) =
                ui.allocate_exact_size(egui::vec2(96.0, SLIDER_HEIGHT), egui::Sense::hover());
            paint_previous_current(
                ui.painter(),
                indicator,
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
            );
        });
        // 没有渲染后端来消费纹理增量；`TexturesDelta` 在 Drop 时会为未消费的
        // 增量 panic（epaint 的防御），测试里显式清掉。
        out.textures_delta.clear();
        // 每根滑条 = 1 个外框 + 1 个渐变网格 + 1 个内高光 + 3 层标记线 = 6 个
        // 形状；当前/上一色 = 外框 + 两半 + 内高光 = 4 个（描边可能合并，故只卡下界）
        assert!(
            out.shapes.len() >= 3 * 6,
            "一帧应产出至少 18 个形状，实得 {}",
            out.shapes.len()
        );
    }

    /// 当前/上一色的两半几何与命中判定 —— `adjbases.py:892-897` + `:918-925`。
    #[test]
    fn previous_current_halves_and_hit_testing() {
        let r = rect(96.0, 22.0); // 底栏/面板里的典型宽度（mypaint.glade:85）
        let (left, right) = previous_current_halves(r, 2.0);
        // eff_wd = 92 → int(92//2) = 46；左半从 b=2 起
        assert!(approx(left.left() as f64, (r.left() + 2.0) as f64, 1e-6));
        assert!(approx(left.width() as f64, 46.0, 1e-6));
        // 右半从 wd//2 = 48 起，宽 eff_wd - 46 = 46
        assert!(approx(right.left() as f64, (r.left() + 48.0) as f64, 1e-6));
        assert!(approx(right.width() as f64, 46.0, 1e-6));
        // 两半等高：eff_ht = 22 - 4
        assert!(approx(left.height() as f64, 18.0, 1e-6));
        assert!(approx(right.height() as f64, 18.0, 1e-6));
        // 命中：x < alloc.width // 2 → 当前色，否则上一色
        let y = r.center().y;
        assert_eq!(hit_half(r, egui::pos2(r.left(), y)), ColorHalf::Current);
        assert_eq!(hit_half(r, egui::pos2(r.left() + 47.9, y)), ColorHalf::Current);
        assert_eq!(hit_half(r, egui::pos2(r.left() + 48.0, y)), ColorHalf::Previous);
        assert_eq!(hit_half(r, egui::pos2(r.left() + 95.0, y)), ColorHalf::Previous);
        // 奇数宽度：中点向下取整（源码是整数除法）
        let odd = rect(95.0, 22.0);
        assert_eq!(hit_half(odd, egui::pos2(odd.left() + 47.0, y)), ColorHalf::Previous);
    }

    /// 半区取色 = 管理器的当前色 / 上一色；恢复上一色只动当前色、不动历史
    /// （`adjbases.py:918-925` 的 `get_color_at_position`）。
    #[test]
    fn half_color_reads_current_and_previous_from_manager() {
        let mut manager = ColorManager::new();
        let before = manager.get_color();
        // 启动时当前色 = 历史末项，两半同色
        assert_eq!(
            half_color(&manager, ColorHalf::Current).to_hex_str(),
            half_color(&manager, ColorHalf::Previous).to_hex_str()
        );
        // 换一个颜色（不落历史 —— 历史只在笔画结束时压入，brushcolor.py:50-57）
        let new = Color::Rgb(RgbColor::new(0.1, 0.2, 0.3));
        assert!(manager.set_color(new));
        assert_eq!(half_color(&manager, ColorHalf::Current).to_hex_str(), new.to_hex_str());
        assert_eq!(
            half_color(&manager, ColorHalf::Previous).to_hex_str(),
            before.to_hex_str(),
            "上一色仍是历史末项"
        );
        // 按右半 → 把上一色设成当前色
        let restore = half_color(&manager, ColorHalf::Previous);
        assert!(manager.set_color(restore));
        assert_eq!(manager.get_color().to_hex_str(), before.to_hex_str());
        assert_eq!(manager.get_history().len(), 5, "恢复颜色不改历史");
    }
}
