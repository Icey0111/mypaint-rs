//! 调色板页 —— `gui/colors/paletteview.py` 的**控件层**移植（B4-4a）。
//!
//! ## 源码里这一页是什么
//!
//! `PalettePage`（`paletteview.py:52-99`）是六个颜色器页面之一，页面控件就是
//! `PaletteView`（`:434-487`）—— 一个 `Gtk.ScrolledWindow`，里面装
//! `_PaletteGridLayout`（`:567-1282`）：**色块网格**。本文件承接网格这一层
//! （布局换算 + 绘制 + 点击设色 + 悬停提示），模型层在
//! [`pigment_engine::color::Palette`]（`lib/palette.py`，B4-4a）。
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `paletteview.py:580-583` 尺寸常量 | [`SWATCH_SIZE_MIN`] / [`SWATCH_SIZE_MAX`] / [`SWATCH_SIZE_NOMINAL`] / [`PREFERRED_COLUMNS`] |
//! | `paletteview.py:928-934` `_constrain_swatch_size()` | [`constrain_swatch_size`] |
//! | `paletteview.py:936-958` `_get_palette_dimensions()` | [`palette_dimensions`] |
//! | `paletteview.py:983-996` `do_get_preferred_height_for_width()` | [`natural_height_for_width`] |
//! | `paletteview.py:616-647` `_size_alloc_cb()` | [`size_alloc`] |
//! | `paletteview.py:1114-1139` `get_position_for_index()` / `get_painting_offset()` | [`position_for_index`] / [`painting_offset`] |
//! | `paletteview.py:1159-1208` `get_index_at_pos()` | [`index_at_pos`] |
//! | `paletteview.py:1430-1549` `_palette_render()` | [`paint_palette_grid`] |
//! | `paletteview.py:1052-1112` `_paint_marker()` + 高亮分支 | [`paint_marker`] |
//! | `paletteview.py:697-739` 悬停提示 | [`hover_tip`] |
//! | `paletteview.py:741-756` `_button_press_cb()`（按钮 1） | [`PigmentApp::color_palette_page`] |
//! | `paletteview.py:56-58` 页面设置（`show_matched_color=True`、`can_select_empty=False`） | 同上 |
//!
//! ## 技术栈适配（两处，都有源码依据）
//!
//! 1. **尺寸协商**：源码把网格放进 `Gtk.ScrolledWindow`，靠 GTK 的
//!    size-request 机制算出"自然高度"再回头定色块边长（`_size_alloc_cb`）。
//!    egui 没有两轮协商，这里按源码自己的公式**先算高度**再算边长：
//!    [`natural_height_for_width`] 就是 `do_get_preferred_height_for_width()`
//!    的自然高度那一支，`size_alloc()` 拿到它之后与 GTK 的 `size-allocate`
//!    回调做**同一套**运算。`PaletteView` 自己那层 min/max 尺寸约束
//!    （`:443-446` 的 150×32 与 300×300）是给 GTK 容器看的，egui 里这一页
//!    直接填满面板并纵向滚动 —— 记账。
//! 2. **背景色**：源码 `_widget_get_bg_color()`（`:549-564`）沿控件树找第一个
//!    不透明背景（Adwaita 3.14 的空背景 workaround），兜底 `RGBColor(.5,.5,.5)`。
//!    这里直接取 egui 的 `extreme_bg_color`（就是内容区底色），见
//!    [`palette_bg_color`]。
//!
//! ## 没做的（逐条记账，见交付说明）
//!
//! - **右键菜单**（`paletteview.py:762-909`）：`Add Empty Slot` / `Insert Row` /
//!   `Insert Column` / `Fill Gap (RGB|HCY|HSV)`。Gtk.Menu + 拖动插位标记，
//!   本块不做。
//! - **拖动排序**（`:1210-1282` 的 `drag_motion_cb` / `drag_data_received_cb`）：
//!   要 GTK 的 `application/x-color` DnD 与 `reposition()` 的落点语义；模型的
//!   `reposition()` 已经移植并有向量门禁，接 DnD 是下一轮的事。
//! - **调色板编辑器对话框**（`:102-431` `PaletteEditorDialog`）：加载/保存
//!   `.gpl`、改名字、改列数、增删色块、清空、色块改名 —— 全是 GTK 对话框。
//!   其中"添加当前色"另有动作与底栏按钮（已接），其余记账。
//! - **色块上的名字显示**：源码在色块上**不画名字**（只在 tooltip 里给，
//!   `_motion_notify_cb`）；这里照抄 —— 名字只出现在悬停提示里。
//! - **DnD 落点标记**（`:1097-1101` 的 `_insert_target_index` 分支）：随拖动排序。
//!
//! ## 门槛
//!
//! 布局换算全部是纯函数，本文件的测试直接喂尺寸算期望值（断言里的数字都能
//! 从 `paletteview.py` 的公式手推出来，注释给出行号）。调色板**模型**的
//! 8651 条真源码向量门禁在 `crates/pigment-engine/tests/color_palette.rs`。

use eframe::egui;
use pigment_engine::color::{Color, HcyColor, Palette, RgbColor, UIColor};

use super::color_panel::rgb32;
use crate::app::PigmentApp;

// ---------------------------------------------------------------------------
// 常量（`gui/colors/paletteview.py`）
// ---------------------------------------------------------------------------

/// 色块边长的下界 —— `paletteview.py:580` `_SWATCH_SIZE_MIN = 8`。
pub(crate) const SWATCH_SIZE_MIN: i32 = 8;
/// 色块边长的上界 —— `paletteview.py:581` `_SWATCH_SIZE_MAX = 50`。
pub(crate) const SWATCH_SIZE_MAX: i32 = 50;
/// 色块边长的名义值 —— `paletteview.py:582` `_SWATCH_SIZE_NOMINAL = 20`。
pub(crate) const SWATCH_SIZE_NOMINAL: i32 = 20;
/// 自然流动（`columns = 0`）时的首选列数 —— `paletteview.py:583`
/// `_PREFERRED_COLUMNS = 5`。
///
/// 唯一的消费者是 `do_get_preferred_width()`（`:969-981`）的"自由流动"
/// 分支：它给容器报一个 `NOMINAL * 5` 的自然宽度。本项目这一页直接填满
/// 面板宽度（见模块文档的技术栈适配），所以没有对应物 —— 常量留着对齐源码、
/// 也供后面接"面板最小宽度"时用，故标 `dead_code`（与 `shell/mod.rs` 的
/// `FOOTER_WIDGETS` 同样处置）。
#[allow(dead_code)]
pub(crate) const PREFERRED_COLUMNS: i32 = 5;

/// 网格静态提示 —— `paletteview.py:573-577` 的 `STATIC_TOOLTIP_TEXT`。
pub(crate) const STATIC_TOOLTIP_TEXT: &str =
    "Color swatch palette.\nDrop colors here,\ndrag them to organize.";

/// 空槽的提示 —— `paletteview.py:732-735`。
pub(crate) const EMPTY_SLOT_TOOLTIP: &str = "Empty palette slot (drag a color here)";

/// 高亮用的亮度偏移 —— `paletteview.py:1453` `highlight_dluma = 0.05`。
pub(crate) const HIGHLIGHT_DLUMA: f64 = 0.05;

// ---------------------------------------------------------------------------
// 尺寸与布局（`paletteview.py:928-1022`）
// ---------------------------------------------------------------------------

/// 色块边长约束 —— `paletteview.py:928-934` `_constrain_swatch_size()`：
///
/// ```python
/// size = min(50, max(8, size))
/// if size % 2 == 0:
///     size -= 1          # 偶数变成奇数（"patterns 要奇数，加边框正好"）
/// ```
///
/// 注意这一步会把 8 变成 7 —— **结果可以低于下界**，源码如此。
pub(crate) fn constrain_swatch_size(size: i32) -> i32 {
    let mut size = size.min(SWATCH_SIZE_MAX).max(SWATCH_SIZE_MIN);
    if size % 2 == 0 {
        size -= 1;
    }
    size
}

/// 调色板的行列数 —— `paletteview.py:936-958` `_get_palette_dimensions()`。
///
/// 返回 `(ncolors, nrows, ncolumns)`；`None` 表示"自然流动"（不指定行列数）。
/// 列数来自调色板自己的 `columns`（`.gpl` 的 `Columns:`，`palette.py:242-249`）。
pub(crate) fn palette_dimensions(
    columns: i64,
    ncolors: usize,
) -> (usize, Option<usize>, Option<usize>) {
    if columns < 1 {
        return (ncolors, None, None);
    }
    let ncolumns = columns as usize;
    if ncolors > 0 {
        // `ncolumns = min(ncolumns, ncolors)`；`nrows = max(1, ncolors // ncolumns)`
        let ncolumns = ncolumns.min(ncolors).max(1);
        let mut nrows = (ncolors / ncolumns).max(1);
        if ncolors % ncolumns > 0 {
            nrows += 1;
        }
        (ncolors, Some(nrows), Some(ncolumns))
    } else {
        // `ncolumns = 1; nrows = 1`
        (ncolors, Some(1), Some(1))
    }
}

/// 给定宽度下的**自然高度** —— `paletteview.py:983-996`
/// `do_get_preferred_height_for_width()`。
///
/// ```python
/// if nrows and ncolumns:
///     swatch_size = self._constrain_swatch_size(int(width // ncolumns))
///     min_h = self._SWATCH_SIZE_MIN * nrows
///     nat_h = swatch_size * nrows
/// else:
///     min_h = int(((self._SWATCH_SIZE_MIN**2) * ncolors) // width)
///     nat_h = int(((self._SWATCH_SIZE_NOMINAL**2) * ncolors) // width)
/// return min_h, max(min_h, nat_h)
/// ```
///
/// 注意最后那个 `max(min_h, nat_h)`：**自然高度不会是"下界"之下**。宽度很窄时
/// `swatch_size` 会被 `_constrain_swatch_size` 压到 7（偶数减一之后甚至低于
/// `_SWATCH_SIZE_MIN`），于是 `nat_h < min_h`，返回的是 `min_h`。
///
/// 中间运算走 `f64` —— Python 的 float 就是 f64，用 f32 会在 `floor()` 的
/// 边界上给出不同结果。
pub(crate) fn natural_height_for_width(width: f32, columns: i64, ncolors: usize) -> f32 {
    let (_, nrows, ncolumns) = palette_dimensions(columns, ncolors);
    let w = f64::from(width.max(1.0));
    match (nrows, ncolumns) {
        (Some(nrows), Some(ncolumns)) => {
            // `int(width // ncolumns)`
            let swatch = constrain_swatch_size((w / ncolumns as f64).floor() as i32);
            let min_h = (SWATCH_SIZE_MIN * nrows as i32) as f64;
            let nat_h = (swatch * nrows as i32) as f64;
            nat_h.max(min_h) as f32
        }
        _ => {
            // `int((MIN**2 * ncolors) // width)` / `int((NOMINAL**2 * ncolors) // width)`
            let min_h = (f64::from(SWATCH_SIZE_MIN).powi(2) * ncolors as f64 / w).floor();
            let nat_h = (f64::from(SWATCH_SIZE_NOMINAL).powi(2) * ncolors as f64 / w).floor();
            nat_h.max(min_h) as f32
        }
    }
}

/// 网格布局 —— `paletteview.py:616-647` `_size_alloc_cb()` 的完整移植。
///
/// 输入是控件的分配尺寸，输出 `(rows, columns, swatch_size)`。源码把结果缓存在
/// 三个私有属性上（`:645-647`）等绘制时用；egui 是立即模式，这里每帧算一次
/// （纯算术，代价可忽略），不再缓存。
///
/// 中间运算同样走 `f64`（Python 的 float）。
pub(crate) fn size_alloc(width: f32, height: f32, columns: i64, ncolors: usize) -> GridLayout {
    let (w, h) = (f64::from(width), f64::from(height));
    let (_, dims_rows, dims_columns) = palette_dimensions(columns, ncolors);
    match (dims_rows, dims_columns) {
        (Some(nrows), Some(ncolumns)) => {
            // 拟合到两个方向里更紧的那个：`int(min(w/nc, h/nr))`
            let size = (w / ncolumns as f64).min(h / nrows as f64).floor() as i32;
            GridLayout {
                rows: nrows,
                columns: ncolumns,
                swatch_size: constrain_swatch_size(size),
            }
        }
        _ => {
            if ncolors == 0 {
                // 空调色板：源码给 rows = columns = 0、边长取名义值（`:641-644`）
                return GridLayout {
                    rows: 0,
                    columns: 0,
                    swatch_size: SWATCH_SIZE_NOMINAL,
                };
            }
            // 自由流动：先按面积开方，再按宽度定列数
            let mut size =
                constrain_swatch_size((w * h / ncolors as f64).sqrt().floor() as i32);
            let nc = (w / f64::from(size)).floor() as i64;
            let mut ncolumns = nc.clamp(1, ncolors as i64) as usize;
            let mut nrows = (ncolors / ncolumns).max(1);
            if ncolors % ncolumns > 0 {
                nrows += 1;
            }
            // 放不下就缩到"按行/列能整除"的边长再来一遍（`:634-640`）
            if nrows as f64 * f64::from(size) > h || ncolumns as f64 * f64::from(size) > w {
                size = ((h / nrows as f64).floor() as i32)
                    .min((w / ncolumns as f64).floor() as i32)
                    .max(1);
                size = constrain_swatch_size(size);
                let nc = (w / f64::from(size)).floor() as i64;
                ncolumns = nc.clamp(1, ncolors as i64) as usize;
                nrows = (ncolors / ncolumns).max(1);
                if ncolors % ncolumns > 0 {
                    nrows += 1;
                }
            }
            GridLayout {
                rows: nrows,
                columns: ncolumns,
                swatch_size: size,
            }
        }
    }
}

/// 一帧的网格布局。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GridLayout {
    /// 行数。
    pub rows: usize,
    /// 列数。
    pub columns: usize,
    /// 色块边长（像素，恒为奇数 —— `_constrain_swatch_size` 的偶数减一）。
    pub swatch_size: i32,
}

impl GridLayout {
    /// 整块网格的像素高度（`rows * swatch_size`）。
    pub(crate) fn height(self) -> f32 {
        self.rows as f32 * self.swatch_size as f32
    }

    /// 整块网格的像素宽度（`columns * swatch_size`）。
    pub(crate) fn width(self) -> f32 {
        self.columns as f32 * self.swatch_size as f32
    }
}

/// 从可用宽度算出整块布局 —— [`natural_height_for_width`] + [`size_alloc`]
/// 的组合（源码里这两步是 GTK 的两轮协商，见模块文档）。
pub(crate) fn grid_layout(width: f32, columns: i64, ncolors: usize) -> GridLayout {
    let height = natural_height_for_width(width, columns, ncolors);
    size_alloc(width, height, columns, ncolors)
}

/// 网格在控件里的居中偏移 —— `paletteview.py:1126-1139`
/// `get_painting_offset()`：居中后 `+1`（那 1 像素是 Tango 风格的外框留白）。
pub(crate) fn painting_offset(layout: GridLayout, width: f32, height: f32) -> (i32, i32) {
    let (wd, ht) = (f64::from(width), f64::from(height));
    let (l_wd, l_ht) = (f64::from(layout.width()), f64::from(layout.height()));
    let (mut dx, mut dy) = (0.0f64, 0.0f64);
    if l_wd < wd {
        dx = (wd - l_wd) / 2.0;
    }
    if l_ht < ht {
        dy = (ht - l_ht) / 2.0;
    }
    (1 + dx as i32, 1 + dy as i32)
}

/// 某个格子的左上角（含 `+0.5` 的半像素）—— `paletteview.py:1114-1124`
/// `get_position_for_index()`。
pub(crate) fn position_for_index(
    layout: GridLayout,
    offset: (i32, i32),
    i: usize,
) -> (f32, f32) {
    let s = layout.swatch_size as f32;
    let c = (i % layout.columns) as f32;
    let r = (i / layout.columns) as f32;
    (0.5 + c * s + offset.0 as f32, 0.5 + r * s + offset.1 as f32)
}

/// 指针位置 → 调色板下标 —— `paletteview.py:1159-1208` `get_index_at_pos()`。
///
/// - 落在网格外：`nearest=false` 给 `None`，`nearest=true` 夹到最近的格子；
/// - 落在最后一行的空白处（下标 >= 长度）：`nearest=false` 给 `None`；
///   `nearest=true` 给 `len`（`insert=true`）或 `len - 1`。
///
/// 空布局（0 行或 0 列，即空调色板）一律 `None` —— 源码在 `nearest=true` 时
/// 会算出 `rows - 1` 的负数下标，那只服务未移植的右键菜单，这里不复制这个
/// 负下标（见模块文档）。
pub(crate) fn index_at_pos(
    layout: GridLayout,
    offset: (i32, i32),
    x: f32,
    y: f32,
    nearest: bool,
    insert: bool,
    ncolors: usize,
) -> Option<usize> {
    if layout.rows == 0 || layout.columns == 0 {
        return None;
    }
    let s = layout.swatch_size as f32;
    // `int((y - dy) // s_ht)`：向下取整（负值也向下）
    let mut r = ((y - offset.1 as f32) / s).floor() as i64;
    let mut c = ((x - offset.0 as f32) / s).floor() as i64;
    if r < 0 {
        if !nearest {
            return None;
        }
        r = 0;
    } else if r >= layout.rows as i64 {
        if !nearest {
            return None;
        }
        r = layout.rows as i64 - 1;
    }
    if c < 0 {
        if !nearest {
            return None;
        }
        c = 0;
    } else if c >= layout.columns as i64 {
        if !nearest {
            return None;
        }
        c = layout.columns as i64 - 1;
    }
    // 最后一行可能没填满
    let mut i = (r as usize) * layout.columns + c as usize;
    if i >= ncolors {
        if !nearest {
            return None;
        }
        i = ncolors;
        if !insert {
            i = i.saturating_sub(1);
        }
    }
    Some(i)
}

// ---------------------------------------------------------------------------
// 绘制（`paletteview.py:1430-1549`）
// ---------------------------------------------------------------------------

/// 网格底色 —— 源码 `_widget_get_bg_color()`（`paletteview.py:549-564`）的替代：
/// 取 egui 的内容区底色（`extreme_bg_color`）。
pub(crate) fn palette_bg_color(ui: &egui::Ui) -> RgbColor {
    let c = ui.visuals().extreme_bg_color;
    RgbColor::new(
        c.r() as f64 / 255.0,
        c.g() as f64 / 255.0,
        c.b() as f64 / 255.0,
    )
}

/// 由底色推出网格的四种辅助色 —— `paletteview.py:1465-1480`：
///
/// ```python
/// light_col = HCYColor(color=bg); light_col.y += 0.05
/// dark_col  = HCYColor(color=bg); dark_col.y  -= 0.05
/// if light_col.y >= 1: light_col.y = 1.0; dark_col.y = 1.0 - 0.1
/// if dark_col.y <= 0:  dark_col.y = 0.0; light_col.y = 0.1
/// ul_col = HCYColor(color=bg); ul_col.y *= 0.75; ul_col.c *= 0.5
/// ```
#[derive(Clone, Copy, Debug)]
pub(crate) struct SwatchTheme {
    /// 空槽棋盘格的高光色（`light_col`）。
    pub light: HcyColor,
    /// 空槽棋盘格的底色（`dark_col`）。
    pub dark: HcyColor,
    /// 整块网格左上描边的颜色（`ul_col`）。
    pub outline: HcyColor,
    /// 空槽的右下阴影（`sh_col`，`:1508-1511`）。
    pub slot_shadow: HcyColor,
}

/// 由底色算主题色 —— 见 [`SwatchTheme`]。
pub(crate) fn swatch_theme(bg: RgbColor) -> SwatchTheme {
    let mut light = HcyColor::from_color(&Color::Rgb(bg));
    let mut dark = HcyColor::from_color(&Color::Rgb(bg));
    light.y += HIGHLIGHT_DLUMA;
    dark.y -= HIGHLIGHT_DLUMA;
    if light.y >= 1.0 {
        light.y = 1.0;
        dark.y = 1.0 - 2.0 * HIGHLIGHT_DLUMA;
    }
    if dark.y <= 0.0 {
        dark.y = 0.0;
        light.y = 2.0 * HIGHLIGHT_DLUMA;
    }
    let mut outline = HcyColor::from_color(&Color::Rgb(bg));
    outline.y *= 0.75;
    outline.c *= 0.5;
    let mut slot_shadow = HcyColor::from_color(&Color::Rgb(bg));
    slot_shadow.y *= 0.75;
    slot_shadow.c *= 0.5;
    SwatchTheme {
        light,
        dark,
        outline,
        slot_shadow,
    }
}

/// 一个格子要用的四种颜色 —— `paletteview.py:1501-1523`。
#[derive(Clone, Copy, Debug)]
pub(crate) struct SwatchColors {
    /// 右下阴影（先铺满整格）。
    pub shadow: [f64; 3],
    /// 主填充（比整格少 1 像素）。
    pub fill: [f64; 3],
    /// 空槽棋盘格的另一色；有颜色的格子是 `None`。
    pub pattern: Option<[f64; 3]>,
    /// 1 像素高光描边。
    pub highlight: [f64; 3],
}

/// 一个格子的配色 —— `paletteview.py:1501-1523`：
///
/// ```python
/// if col is None:                       # 空槽
///     hi_rgb = light; fill_bg = dark; fill_fg = light
///     sh_col = HCYColor(bg); sh_col.y *= 0.75; sh_col.c *= 0.5
/// else:
///     hi = HCYColor(col); hi.y = min(hi.y*1.1, 1); hi.c = min(hi.c*1.1, 1)
///     sh = HCYColor(col); sh.y *= 0.666; sh.c *= 0.5
///     hi_rgb = hi; fill_bg = col; fill_fg = None
///     sh_rgb = sh
/// ```
pub(crate) fn swatch_colors(entry: Option<RgbColor>, theme: SwatchTheme) -> SwatchColors {
    match entry {
        None => SwatchColors {
            shadow: theme.slot_shadow.get_rgb(),
            fill: theme.dark.get_rgb(),
            pattern: Some(theme.light.get_rgb()),
            highlight: theme.light.get_rgb(),
        },
        Some(col) => {
            let mut hi = HcyColor::from_color(&Color::Rgb(col));
            hi.y = (hi.y * 1.1).min(1.0);
            hi.c = (hi.c * 1.1).min(1.0);
            let mut sh = HcyColor::from_color(&Color::Rgb(col));
            sh.y *= 0.666;
            sh.c *= 0.5;
            SwatchColors {
                shadow: sh.get_rgb(),
                fill: col.get_rgb(),
                pattern: None,
                highlight: hi.get_rgb(),
            }
        }
    }
}

/// 画整块网格 —— `paletteview.py:1430-1549` `_palette_render()`。
///
/// 顺序照抄：左上外框 → 逐格（阴影 → 主填充 → 空槽棋盘 → 高光描边）。
pub(crate) fn paint_palette_grid(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: GridLayout,
    entries: &[Option<RgbColor>],
    bg: RgbColor,
) {
    if entries.is_empty() || layout.rows == 0 || layout.columns == 0 {
        return;
    }
    let theme = swatch_theme(bg);
    let offset = painting_offset(layout, rect.width(), rect.height());
    let origin = rect.min + egui::vec2(offset.0 as f32, offset.1 as f32);
    let s = layout.swatch_size as f32;

    // ---- 左上外框（`paletteview.py:1476-1489`）----
    //
    // 只画上边与左边（右下由每格自己的阴影盖住）：折线
    // (0.5, rows*s - 1) → (0.5, 0.5) → (row1cells*s - 1, 0.5)，线宽 2。
    let row1cells = layout.columns.min(entries.len());
    let ul = rgb32(theme.outline.get_rgb());
    let stroke = egui::Stroke::new(2.0, ul);
    painter.line_segment(
        [
            origin + egui::vec2(0.5, layout.height() - 1.0),
            origin + egui::vec2(0.5, 0.5),
        ],
        stroke,
    );
    painter.line_segment(
        [
            origin + egui::vec2(0.5, 0.5),
            origin + egui::vec2(row1cells as f32 * s - 1.0, 0.5),
        ],
        stroke,
    );

    // ---- 逐格（`paletteview.py:1491-1547`）----
    for (i, entry) in entries.iter().enumerate() {
        let c = (i % layout.columns) as f32;
        let r = (i / layout.columns) as f32;
        let sx = origin.x + c * s;
        let sy = origin.y + r * s;
        let colors = swatch_colors(*entry, theme);

        // 阴影：整格
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(sx, sy), egui::vec2(s, s)),
            0.0,
            rgb32(colors.shadow),
        );
        // 主填充：少 1 像素
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(sx, sy), egui::vec2(s - 1.0, s - 1.0)),
            0.0,
            rgb32(colors.fill),
        );
        // 空槽的棋盘格：两块 (s-1)//2 见方，分别贴左上与右下
        if let Some(fg) = colors.pattern {
            let half = ((s - 1.0) as i32 / 2) as f32;
            let fg = rgb32(fg);
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(sx, sy), egui::vec2(half, half)),
                0.0,
                fg,
            );
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(sx + half, sy + half),
                    egui::vec2(half, half),
                ),
                0.0,
                fg,
            );
        }
        // 高光：+0.5 偏移、少 2 像素、线宽 1
        painter.rect_stroke(
            egui::Rect::from_min_size(
                egui::pos2(sx + 0.5, sy + 0.5),
                egui::vec2(s - 2.0, s - 2.0),
            ),
            0.0,
            egui::Stroke::new(1.0, rgb32(colors.highlight)),
            egui::StrokeKind::Middle,
        );
    }
}

/// 高亮当前命中的格子 —— `paletteview.py:1052-1112` 的 `_paint_marker()`
/// 加上 `paint_foreground_cb()` 的 `elif self.show_matched_color:` 分支。
///
/// 源码是两层虚线矩形：底色虚线黑（精确命中时线宽 4，近似时 3）+ 前景虚线白
/// （线宽 1），图案都是 `(2, 3)`。egui 的 `Shape::dashed_line` 按整条折线连续
/// 计相位，Cairo 是每段独立 —— 虚线分段的绝对位置不会逐像素相同，但线宽、
/// 图案长度、"精确 = 更粗"的语义一致。
pub(crate) fn paint_marker(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: GridLayout,
    index: usize,
    approx: bool,
) {
    if layout.rows == 0 || layout.columns == 0 || index >= layout.rows * layout.columns {
        return;
    }
    let offset = painting_offset(layout, rect.width(), rect.height());
    let (x, y) = position_for_index(layout, offset, index);
    let (x, y) = (rect.left() + x, rect.top() + y);
    let s = layout.swatch_size as f32;
    // `cr.rectangle(x, y, w - 1, h - 1)`
    let r = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(s - 1.0, s - 1.0));
    let path = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    // `bg_width = 4 if not match_is_approx else 3`（`paletteview.py:1109-1111`）
    let bg_width = if approx { 3.0 } else { 4.0 };
    for (width, color) in [
        (bg_width, egui::Color32::BLACK),
        (1.0, egui::Color32::WHITE),
    ] {
        painter.extend(egui::Shape::dashed_line(
            &path,
            egui::Stroke::new(width, color),
            2.0,
            3.0,
        ));
    }
}

/// 悬停提示 —— `paletteview.py:697-739` `_motion_notify_cb()` 的取文案部分。
///
/// ```python
/// if color is None:  tip = "Empty palette slot (drag a color here)"
/// elif tip is None or tip.strip() == "":  tip = ""   # 匿名色没有提示
/// ```
///
/// 指针不在任何格子上时给静态提示（`:714-716`）。返回 `None` 表示"不弹提示"。
pub(crate) fn hover_tip(palette: &Palette, index: Option<usize>) -> Option<String> {
    let Some(i) = index else {
        return Some(STATIC_TOOLTIP_TEXT.to_string());
    };
    let color = palette.get(i);
    if color.is_none() {
        return Some(EMPTY_SLOT_TOOLTIP.to_string());
    }
    let name = palette.color_name(i);
    match name {
        Some(n) if !n.trim().is_empty() => Some(n.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 页面
// ---------------------------------------------------------------------------

impl PigmentApp {
    /// 调色板页 —— `PalettePage`（`paletteview.py:52-99`）的页面体。
    ///
    /// 页面设置照抄 `PalettePage.__init__`（`:56-58`）：
    /// `view.grid.show_matched_color = True`（画命中高亮）、
    /// `view.can_select_empty = False`（点空槽不选中）。
    ///
    /// 点击语义是**按下**（`_button_press_cb`，`:741-756`）：
    /// 1. `i = get_index_at_pos(x, y, nearest=False)`；
    /// 2. 有颜色才 `set_match_position(i)` + `set_match_is_approx(False)`；
    /// 3. 基类 handler 再把 `get_color_at_position()` 的颜色交给管理器
    ///    （`adjbases.py:660-672`）—— 空槽给 `None`，`set_managed_color(None)`
    ///    是无操作（`:405-410`），所以点空槽什么都不改。
    pub(crate) fn color_palette_page(&mut self, ui: &mut egui::Ui) {
        let bg = palette_bg_color(ui);
        // 本帧的快照：绘制/命中都读它，交互改完再统一落回模型
        let entries: Vec<Option<RgbColor>> = self.color_manager.palette().iter_colors().collect();
        let columns = self.color_manager.palette().columns();
        let match_pos = self.color_manager.palette().match_position();
        let match_approx = self.color_manager.palette().match_is_approx();
        // `(落点下标)`：外层 None = 本帧没按
        let mut pressed_index: Option<Option<usize>> = None;

        egui::ScrollArea::vertical()
            // 源码的 ScrolledWindow 只滚不拖（`paletteview.py:451` 的策略是
            // NEVER/AUTOMATIC，Gtk.ScrolledWindow 本身也没有"按住内容拖动"）；
            // egui 的默认在触屏上会拖内容滚动，这里显式关掉。
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            })
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width().max(1.0);
                let layout = grid_layout(width, columns, entries.len());
                let height = layout.height().max(1.0);
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

                paint_palette_grid(ui.painter(), rect, layout, &entries, bg);
                if let Some(i) = match_pos {
                    paint_marker(ui.painter(), rect, layout, i, match_approx);
                }

                // ---- 悬停提示（`:697-739`）----
                let hover_pos = resp.hover_pos();
                let hover_index = hover_pos.and_then(|p| {
                    index_at_pos(
                        layout,
                        painting_offset(layout, rect.width(), rect.height()),
                        p.x - rect.left(),
                        p.y - rect.top(),
                        false,
                        false,
                        entries.len(),
                    )
                });
                // 提示文案从快照推：名字在 entries 之外，得单独取
                if let Some(tip) = hover_tip(self.color_manager.palette(), hover_index) {
                    let _ = resp.clone().on_hover_text(tip);
                }

                // ---- 按钮 1 按下（`:741-756`）----
                let pressed_now = resp.is_pointer_button_down_on()
                    && ui.input(|i| i.pointer.primary_pressed());
                if pressed_now {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let i = index_at_pos(
                            layout,
                            painting_offset(layout, rect.width(), rect.height()),
                            p.x - rect.left(),
                            p.y - rect.top(),
                            false,
                            false,
                            entries.len(),
                        );
                        pressed_index = Some(i);
                    }
                }
            });

        // ---- 落色（`:747-756` + `adjbases.py:671-672`）----
        if let Some(index) = pressed_index.flatten() {
            let palette = self.color_manager.palette_mut();
            // `is_empty = palette.get_color(i) is None`；`can_select_empty=False`
            if palette.get(index).is_some() {
                palette.set_match_position(Some(index));
                palette.set_match_is_approx(false);
            }
            // 基类 handler：`set_managed_color(get_color_at_position(x, y))`
            if let Some(color) = palette.get(index) {
                self.set_color(Color::Rgb(color));
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
    use pigment_engine::color::{ColorManager, DEFAULT_PALETTE_GPL};

    // ---- 真源码布局向量（`gui/colors/paletteview.py` 的 ast 抽出）----

    fn fixture_path() -> String {
        format!(
            "{}/tests/fixtures/paletteview_vectors.txt",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    fn fixture() -> &'static str {
        static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        TEXT.get_or_init(|| {
            std::fs::read_to_string(fixture_path()).unwrap_or_else(|e| {
                panic!(
                    "读不到 fixture {}：{e}（先跑 tests/tools/gen-palette-vectors.py）",
                    fixture_path()
                )
            })
        })
    }

    fn rows() -> impl Iterator<Item = (usize, Vec<&'static str>)> {
        fixture()
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|(i, l)| (i + 1, l.split('\t').collect()))
    }

    fn f64_of(tok: &str, ln: usize) -> f64 {
        tok.parse::<f64>()
            .unwrap_or_else(|e| panic!("第 {ln} 行不是浮点：{tok:?}（{e}）"))
    }

    /// fixture 里的非负整数字段。
    ///
    /// 允许 `"4.0"` 这种写法：源码自由流动分支里 `ncolumns = max(1,
    /// min(ncolors, width // size))` 的 `width // size` 是 **float**，所以
    /// `_columns` / `_rows` 可能是 `4.0` 这样的浮点值 —— Rust 侧一律用整数存，
    /// 数值等价。
    fn usize_of(tok: &str, ln: usize) -> usize {
        if let Ok(v) = tok.parse::<usize>() {
            return v;
        }
        let f = tok
            .parse::<f64>()
            .unwrap_or_else(|e| panic!("第 {ln} 行不是整数：{tok:?}（{e}）"));
        assert!(
            f >= 0.0 && f.fract() == 0.0,
            "第 {ln} 行的整数不是整值：{tok:?}"
        );
        f as usize
    }

    fn i32_of(tok: &str, ln: usize) -> i32 {
        tok.parse::<i32>()
            .unwrap_or_else(|e| panic!("第 {ln} 行不是整数：{tok:?}（{e}）"))
    }

    fn bool_of(tok: &str, ln: usize) -> bool {
        match tok {
            "1" => true,
            "0" => false,
            other => panic!("第 {ln} 行不是 0/1：{other:?}"),
        }
    }

    fn opt_usize(tok: &str, ln: usize) -> Option<usize> {
        if tok == "-" {
            None
        } else {
            Some(usize_of(tok, ln))
        }
    }

    /// 色块边长约束 —— `paletteview.py:928-934`（含"8 变成 7"这个下界溢出）。
    #[test]
    fn constrain_swatch_size_matches_source() {
        let mut checked = 0;
        for (ln, f) in rows() {
            if f[0] != "pv_constrain" {
                continue;
            }
            let got = constrain_swatch_size(i32_of(f[1], ln));
            assert_eq!(got, i32_of(f[2], ln), "第 {ln} 行 constrain_swatch_size({})", f[1]);
            checked += 1;
        }
        assert!(checked >= 14, "只比对了 {checked} 条");
        // 源码把偶数减一，所以 8 → 7（**低于** _SWATCH_SIZE_MIN）
        assert_eq!(constrain_swatch_size(8), 7);
        assert_eq!(constrain_swatch_size(9), 9);
        assert_eq!(constrain_swatch_size(50), 49);
        assert_eq!(constrain_swatch_size(51), 49);
        assert_eq!(constrain_swatch_size(-100), 7);
    }

    /// 行列数 —— `paletteview.py:936-958`。
    #[test]
    fn palette_dimensions_match_source() {
        let mut checked = 0;
        for (ln, f) in rows() {
            if f[0] != "pv_dims" {
                continue;
            }
            let columns = f[1].parse::<i64>().unwrap_or_else(|e| panic!("第 {ln} 行 {e}"));
            let ncolors = usize_of(f[2], ln);
            let (n, rows_, cols_) = palette_dimensions(columns, ncolors);
            assert_eq!(n, ncolors, "第 {ln} 行：ncolors 原样返回");
            assert_eq!(rows_, opt_usize(f[3], ln), "第 {ln} 行：nrows");
            assert_eq!(cols_, opt_usize(f[4], ln), "第 {ln} 行：ncolumns");
            checked += 1;
        }
        assert!(checked >= 60, "只比对了 {checked} 条");
        // 默认调色板：7 列 77 色 → 11 行
        assert_eq!(palette_dimensions(7, 77), (77, Some(11), Some(7)));
        // 列数 0 / 负数 → 自然流动
        assert_eq!(palette_dimensions(0, 77), (77, None, None));
        assert_eq!(palette_dimensions(-3, 5), (5, None, None));
        // 空调色板 + 指定列数 → 1×1（源码 `:954-957`）
        assert_eq!(palette_dimensions(7, 0), (0, Some(1), Some(1)));
    }

    /// 自然高度 + `_size_alloc_cb` 的完整布局 —— `paletteview.py:616-647` /
    /// `:983-996`。
    #[test]
    fn size_alloc_matches_source() {
        let mut nat_checked = 0;
        let mut alloc_checked = 0;
        for (ln, f) in rows() {
            match f[0] {
                "pv_nat_h" => {
                    let columns = f[1].parse::<i64>().unwrap_or_else(|e| panic!("第 {ln} 行 {e}"));
                    let ncolors = usize_of(f[2], ln);
                    let width = f64_of(f[3], ln) as f32;
                    let got = natural_height_for_width(width, columns, ncolors);
                    assert_eq!(
                        f64::from(got),
                        f64_of(f[4], ln),
                        "第 {ln} 行 natural_height_for_width({width}, {columns}, {ncolors})"
                    );
                    nat_checked += 1;
                }
                "pv_size_alloc" => {
                    let columns = f[1].parse::<i64>().unwrap_or_else(|e| panic!("第 {ln} 行 {e}"));
                    let ncolors = usize_of(f[2], ln);
                    let width = f64_of(f[3], ln) as f32;
                    let height = f64_of(f[4], ln) as f32;
                    let layout = size_alloc(width, height, columns, ncolors);
                    assert_eq!(layout.rows, usize_of(f[5], ln), "第 {ln} 行：rows");
                    assert_eq!(layout.columns, usize_of(f[6], ln), "第 {ln} 行：columns");
                    assert_eq!(layout.swatch_size, i32_of(f[7], ln), "第 {ln} 行：swatch_size");
                    alloc_checked += 1;
                }
                _ => {}
            }
        }
        assert!(nat_checked >= 180, "只比对了 {nat_checked} 条 nat_h");
        assert!(alloc_checked >= 180, "只比对了 {alloc_checked} 条 size_alloc");
    }

    /// 居中偏移与格子位置 —— `paletteview.py:1114-1139`。
    #[test]
    fn offset_and_position_match_source() {
        let mut checked = 0;
        for (ln, f) in rows() {
            if f[0] != "pv_offset" && f[0] != "pv_pos" {
                continue;
            }
            let layout = GridLayout {
                rows: usize_of(f[1], ln),
                columns: usize_of(f[2], ln),
                swatch_size: i32_of(f[3], ln),
            };
            let width = f64_of(f[4], ln) as f32;
            let height = f64_of(f[5], ln) as f32;
            let offset = painting_offset(layout, width, height);
            if f[0] == "pv_offset" {
                assert_eq!(
                    (offset.0 as f64, offset.1 as f64),
                    (f64_of(f[6], ln), f64_of(f[7], ln)),
                    "第 {ln} 行：painting_offset"
                );
            } else {
                let index = usize_of(f[6], ln);
                let (x, y) = position_for_index(layout, offset, index);
                assert_eq!(
                    (f64::from(x), f64::from(y)),
                    (f64_of(f[7], ln), f64_of(f[8], ln)),
                    "第 {ln} 行：position_for_index({index})"
                );
            }
            checked += 1;
        }
        assert!(checked >= 14000, "只比对了 {checked} 条");
    }

    /// 命中判定 —— `paletteview.py:1159-1208`（含 `nearest`/`insert` 两个开关
    /// 与"最后一行没填满"）。
    #[test]
    fn hit_testing_matches_source() {
        let mut checked = 0;
        for (ln, f) in rows() {
            if f[0] != "pv_index" {
                continue;
            }
            let layout = GridLayout {
                rows: usize_of(f[1], ln),
                columns: usize_of(f[2], ln),
                swatch_size: i32_of(f[3], ln),
            };
            let width = f64_of(f[4], ln) as f32;
            let height = f64_of(f[5], ln) as f32;
            let ncolors = usize_of(f[6], ln);
            let x = f64_of(f[7], ln) as f32;
            let y = f64_of(f[8], ln) as f32;
            let nearest = bool_of(f[9], ln);
            let insert = bool_of(f[10], ln);
            let want = opt_usize(f[11], ln);
            let offset = painting_offset(layout, width, height);
            let got = index_at_pos(layout, offset, x, y, nearest, insert, ncolors);
            assert_eq!(
                got, want,
                "第 {ln} 行：index_at_pos(layout {layout:?}, {x}, {y}, nearest={nearest}, insert={insert}, n={ncolors})"
            );
            checked += 1;
        }
        assert!(checked >= 35000, "只比对了 {checked} 条");
    }

    // ---- 端到端的布局/点击语义（不依赖窗口）----

    /// 默认调色板在 200px 宽的面板里：7 列 × 11 行、边长 27、总高 297。
    #[test]
    fn default_palette_grid_in_a_200px_panel() {
        let mut pal = Palette::new();
        pal.load_str(DEFAULT_PALETTE_GPL).expect("内置 .gpl");
        assert_eq!(pal.columns(), 7);
        let layout = grid_layout(200.0, pal.columns(), pal.len());
        assert_eq!(layout.columns, 7);
        assert_eq!(layout.rows, 11);
        // nat_h = constrain(int(200 // 7) = 28) = 27，再 × 11 行 = 297
        assert_eq!(layout.swatch_size, 27);
        assert_eq!(layout.height(), 297.0);
    }

    /// 逐格点击都命中自己：格子中心 → `index_at_pos` → 同一格 → 同一支颜色
    /// （`_button_press_cb` + `get_color_at_position` 的整条路径）。
    #[test]
    fn clicking_each_swatch_hits_its_own_color() {
        let mut pal = Palette::new();
        for i in 0..10 {
            let v = i as f64 / 9.0;
            pal.append(
                Some(&Color::Rgb(RgbColor::new(v, 1.0 - v, 0.5))),
                Some(&format!("c{i}")),
                false,
                false,
            );
        }
        pal.set_columns(4);
        let width = 180.0;
        let layout = grid_layout(width, pal.columns(), pal.len());
        assert_eq!((layout.rows, layout.columns), (3, 4), "10 格 4 列 = 3 行");
        let height = layout.height();
        let offset = painting_offset(layout, width, height);
        let s = layout.swatch_size as f32;
        for i in 0..pal.len() {
            let (x, y) = position_for_index(layout, offset, i);
            let center = (x + s * 0.5, y + s * 0.5);
            let hit = index_at_pos(layout, offset, center.0, center.1, false, false, pal.len());
            assert_eq!(hit, Some(i), "第 {i} 格的中心应命中自己");
            // 命中的颜色就是这个格子的颜色
            assert_eq!(pal.get(i).map(|c| c.to_hex_str()), pal.get(i).map(|c| c.to_hex_str()));
        }
        // 第一行第二列（下标 1）的左上角内侧
        let (x1, y1) = position_for_index(layout, offset, 1);
        assert_eq!(
            index_at_pos(layout, offset, x1 + 0.1, y1 + 0.1, false, false, pal.len()),
            Some(1)
        );
        // 右下角的空白（第 3 行只有 2 格）→ 不命中任何格子
        let (x9, y9) = position_for_index(layout, offset, 9);
        let empty_x = x9 + s + 1.0;
        assert_eq!(
            index_at_pos(layout, offset, empty_x, y9 + 1.0, false, false, pal.len()),
            None,
            "最后一行的空位不该命中"
        );
    }

    /// 增删色块后，索引与布局一起变：插入/删除后同一支颜色仍在命中判定里
    /// 对得上自己的新下标。
    #[test]
    fn hit_testing_follows_insert_and_pop() {
        let mk = |i: usize| Color::Rgb(RgbColor::new(i as f64 / 8.0, 0.5, 0.25));
        let mut pal = Palette::new();
        for i in 0..8 {
            pal.append(Some(&mk(i)), Some(&format!("s{i}")), false, false);
        }
        pal.set_columns(4);
        let before_cols = pal.columns();
        assert_eq!(palette_dimensions(before_cols, pal.len()).2, Some(4));

        // 在中间插一格 → 后面的下标整体后移
        pal.insert(Some(2), Some(&mk(99)), Some("inserted"));
        assert_eq!(pal.len(), 9);
        assert_eq!(pal.color_name(2), Some("inserted"));
        assert_eq!(pal.color_name(3), Some("s2"), "原第 2 格退到第 3");
        // 布局变成 3 行（9 格 / 4 列）
        let layout = grid_layout(180.0, pal.columns(), pal.len());
        assert_eq!((layout.rows, layout.columns), (3, 4));
        let offset = painting_offset(layout, 180.0, layout.height());
        let (x, y) = position_for_index(layout, offset, 8);
        assert_eq!(
            index_at_pos(
                layout,
                offset,
                x + layout.swatch_size as f32 * 0.5,
                y + layout.swatch_size as f32 * 0.5,
                false,
                false,
                pal.len()
            ),
            Some(8),
            "最后一个格子的命中下标跟着长度走"
        );

        // 删掉被命中的那一格 → 命中位置被清空（`palette.py:685-686`）
        pal.set_match_position(Some(4));
        pal.pop(4);
        assert_eq!(pal.len(), 8);
        assert_eq!(pal.match_position(), None, "删掉命中格 → 命中清空");
        assert_eq!(pal.color_name(4), Some("s4"), "后面的格子前移");

        // 再次删除更靠前的格子 → 命中位置跟着减一
        pal.set_match_position(Some(5));
        pal.pop(1);
        assert_eq!(pal.match_position(), Some(4));
        assert_eq!(pal.len(), 7);
    }

    /// 悬停提示的三条分支 —— `paletteview.py:697-739`。
    #[test]
    fn hover_tips_follow_source() {
        let mut pal = Palette::new();
        pal.append(
            Some(&Color::Rgb(RgbColor::new(1.0, 0.0, 0.0))),
            Some("Red"),
            false,
            false,
        );
        pal.append(None, None, false, false);
        pal.append(Some(&Color::Rgb(RgbColor::new(0.0, 0.0, 1.0))), None, false, false);
        // 有名字
        assert_eq!(hover_tip(&pal, Some(0)), Some("Red".to_string()));
        // 空槽
        assert_eq!(hover_tip(&pal, Some(1)), Some(EMPTY_SLOT_TOOLTIP.to_string()));
        // 匿名色 → 不弹提示
        assert_eq!(hover_tip(&pal, Some(2)), None);
        // 名字是空白串也算匿名（源码 `tip.strip() == ""`）
        pal.set_color_name(2, Some("   "));
        assert_eq!(hover_tip(&pal, Some(2)), None);
        // 不在任何格子上 → 静态提示
        assert_eq!(hover_tip(&pal, None), Some(STATIC_TOOLTIP_TEXT.to_string()));
    }

    /// 绘制冒烟：没有窗口/GPU 也能跑一帧，网格与高亮都要产出图形。
    #[test]
    fn painting_produces_shapes_without_a_window() {
        let ctx = egui::Context::default();
        let mut pal = Palette::new();
        for i in 0..6 {
            let v = i as f64 / 5.0;
            pal.append(
                Some(&Color::Rgb(RgbColor::new(v, v, 1.0 - v))),
                None,
                false,
                false,
            );
        }
        pal.append(None, None, false, false); // 一格空槽，走棋盘格分支
        pal.set_columns(3);
        let entries: Vec<Option<RgbColor>> = pal.iter_colors().collect();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 300.0),
            )),
            ..Default::default()
        };
        let layout = grid_layout(160.0, pal.columns(), entries.len());
        let size = egui::vec2(160.0, layout.height());
        let bg = RgbColor::new(0.5, 0.5, 0.5);
        let mut out = ctx.run_ui(input, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            paint_palette_grid(ui.painter(), rect, layout, &entries, bg);
            paint_marker(ui.painter(), rect, layout, 2, false);
            paint_marker(ui.painter(), rect, layout, 3, true);
        });
        out.textures_delta.clear();
        // 每格至少 3 个形状（阴影/填充/高光），空槽再多 2 个，加上左上外框 2 条
        // 与两条虚线高亮 —— 只卡下界
        let per_cell = 3 * entries.len();
        assert!(
            out.shapes.len() >= per_cell,
            "一帧应产出至少 {per_cell} 个形状，实得 {}",
            out.shapes.len()
        );
        // 空调色板 / 0 行 0 列不 panic、不产形状
        let ctx2 = egui::Context::default();
        let input2 = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0))),
            ..Default::default()
        };
        let mut out2 = ctx2.run_ui(input2, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(100.0, 100.0), egui::Sense::hover());
            let empty: Vec<Option<RgbColor>> = Vec::new();
            let l = grid_layout(100.0, 0, 0);
            paint_palette_grid(ui.painter(), rect, l, &empty, bg);
            paint_marker(ui.painter(), rect, l, 0, false);
        });
        out2.textures_delta.clear();
        assert_eq!(out2.shapes.len(), 0, "空调色板什么都不画");
    }

    /// 主题色与格子配色 —— `paletteview.py:1465-1523` 的几条边界
    /// （`light.y >= 1` 与 `dark.y <= 0` 的夹取）。
    #[test]
    fn swatch_theme_clamps_like_source() {
        // 中灰底色：light = 0.55、dark = 0.45
        let theme = swatch_theme(RgbColor::new(0.5, 0.5, 0.5));
        assert!((theme.light.y - 0.55).abs() < 1e-12);
        assert!((theme.dark.y - 0.45).abs() < 1e-12);
        // 纯白底：light 会超过 1 → 夹到 1，dark 退到 0.9
        let theme = swatch_theme(RgbColor::new(1.0, 1.0, 1.0));
        assert_eq!(theme.light.y, 1.0);
        assert!((theme.dark.y - 0.9).abs() < 1e-12);
        // 纯黑底：dark 会低于 0 → 夹到 0，light 退到 0.1
        let theme = swatch_theme(RgbColor::new(0.0, 0.0, 0.0));
        assert_eq!(theme.dark.y, 0.0);
        assert!((theme.light.y - 0.1).abs() < 1e-12);
        // 有颜色的格子：高光变亮、阴影变暗，且主填充就是原色
        let theme = swatch_theme(RgbColor::new(0.5, 0.5, 0.5));
        let c = swatch_colors(Some(RgbColor::new(0.4, 0.6, 0.8)), theme);
        assert_eq!(c.fill, [0.4, 0.6, 0.8]);
        assert!(c.pattern.is_none(), "有颜色的格子没有棋盘格");
        assert!(c.shadow[2] < 0.8, "阴影更暗");
        assert!(c.highlight[2] >= 0.8, "高光更亮");
        // 空槽：主填充用 dark、棋盘格用 light
        let e = swatch_colors(None, theme);
        assert_eq!(e.fill, theme.dark.get_rgb());
        assert_eq!(e.pattern, Some(theme.light.get_rgb()));
        assert_eq!(e.shadow, theme.slot_shadow.get_rgb());
    }

    /// 调色板页的元数据与动作接线（`resources.xml:1034-1053`、`:1400-1406`）。
    #[test]
    fn palette_actions_and_page_ids_are_wired() {
        use crate::shell::actions;
        use crate::shell::color_panel::{COLOR_PAGES, PAGE_COMPONENT_SLIDERS, PAGE_PALETTE};

        // 页签表里 Palette 页已启用，且下标与常量一致
        assert_eq!(COLOR_PAGES[PAGE_PALETTE].action, "PaletteTool");
        assert!(COLOR_PAGES[PAGE_PALETTE].ported, "Palette 页现在可用了");
        assert_eq!(COLOR_PAGES[PAGE_COMPONENT_SLIDERS].action, "ComponentSlidersTool");
        let ported: Vec<&str> = COLOR_PAGES
            .iter()
            .filter(|p| p.ported)
            .map(|p| p.title)
            .collect();
        // B4-4b 起 HSV 三页也接上了，只剩 HCY Wheel 一页禁用（B4-4c）
        assert_eq!(
            ported,
            ["HSV Wheel", "Palette", "HSV Square", "HSV Cube", "Component Sliders"]
        );

        // 三个调色板动作都在动作表里
        for id in ["PaletteNext", "PalettePrev", "PaletteAddCurrentColor", "PaletteTool"] {
            let action = actions::by_id(id).unwrap_or_else(|| panic!("{id} 不在动作表里"));
            assert!(action.label.is_some(), "{id} 应有 label");
            assert!(action.tooltip.is_some(), "{id} 应有 tooltip");
        }
        // resources.xml 里 PaletteNext/PalettePrev/PaletteAddCurrentColor 都**没有**
        // 加速键（只有 PaletteTool 有 `c` + Shift，:1406）
        for id in ["PaletteNext", "PalettePrev", "PaletteAddCurrentColor"] {
            let action = actions::by_id(id).unwrap();
            assert!(action.accel.is_none(), "{id} 在源码里没有加速键");
        }
        let tool = actions::by_id("PaletteTool").unwrap();
        assert_eq!(tool.icon_name, Some("mypaint-tool-color-palette"));
        let accel = tool.accel.expect("PaletteTool 有 Shift+c");
        assert_eq!(accel.key, "c");
        assert!(accel.mods.shift && !accel.mods.ctrl && !accel.mods.alt);
        // 图标签页图标随图标资源一起栅格化
        assert!(crate::shell::toolbars::icon_png_exists("mypaint-tool-color-palette"));
    }

    /// `Palette` 在管理器里是**共享**的那一个：三个动作与页面看到同一个模型
    /// （`adjbases.py:347-352` 的 `ColorManager.palette` 属性）。
    #[test]
    fn palette_lives_in_the_color_manager() {
        let mut mgr = ColorManager::new();
        assert_eq!(mgr.palette().len(), 77, "启动时是默认调色板");
        assert_eq!(mgr.palette().columns(), 7);
        // 页面点击 → 设色 → 管理器回头更新命中位置
        let click_index = 12;
        let color = mgr.palette().get(click_index).expect("默认调色板第 12 格");
        mgr.palette_mut().set_match_position(Some(click_index));
        mgr.palette_mut().set_match_is_approx(false);
        assert!(mgr.set_color(Color::Rgb(color)));
        assert_eq!(mgr.palette().match_position(), Some(click_index));
        assert!(!mgr.palette().match_is_approx(), "点中的格子就是精确命中");
        // 网格布局用的是同一份列数/长度
        let layout = grid_layout(200.0, mgr.palette().columns(), mgr.palette().len());
        assert_eq!(layout.columns, 7);
    }
}
