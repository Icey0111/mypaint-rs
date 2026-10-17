//! 颜色模型与颜色调整 —— `lib/color.py` + `gui/brushcolor.py` +
//! `gui/document.py` 的颜色/笔刷调整回调（B4-1）。
//!
//! ## 这一块移植了什么
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `lib/color.py:36-267` `UIColor` 基类 | [`UIColor`] trait + [`RgbColor`] 等共享默认实现 |
//! | `lib/color.py:269-353` `RGBColor` | [`RgbColor`] |
//! | `lib/color.py:356-481` `HSVColor` | [`HsvColor`] |
//! | `lib/color.py:484-638` `HCYColor` | [`HcyColor`] |
//! | `lib/color.py:641-742` `YCbCrColor` | [`YcbcrColor`] |
//! | `lib/color.py:759-774` BT601 换算 | [`rgb_to_ycbcr_bt601`] / [`ycbcr_to_rgb_bt601`] |
//! | `lib/color.py:799-916` HCY 换算 + `_HCY_*_LUMA` | `HCY_RED_LUMA` 等 / [`rgb_to_hcy`] / [`hcy_to_rgb`] |
//! | `lib/color.py:65-110` `get_hsv()` 与只读属性 | [`UIColor`] 的默认方法与 `r()`/`g()`/`b()`/`h()`/`s()`/`v()` |
//! | `lib/color.py:114-220` luma/灰度/反差/hex/fill pixel | [`UIColor::get_luma`] 等 |
//! | `lib/color.py:264-716` `interpolate()` 四份实现 | [`UIColor::interpolate`] |
//! | `gui/colors/adjbases.py:62-359` `ColorManager` | [`ColorManager`] |
//! | `gui/brushcolor.py:16-71` `BrushColorManager` | [`ColorManager`] 的注释里逐条对上 |
//! | `gui/document.py:1571-1644` 十个调整回调 | [`ColorAction`] / [`BrushStep`] |
//! | `gui/document.py:1714-1724` `ContextRestoreColor` | [`ColorManager::restore_brush_context`] 等 |
//!
//! B4-2 追加（`color/tools.rs`，颜色工具的模型层）：
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `gui/colors/adjbases.py:360-429` `ColorAdjuster` | [`ColorAdjuster`] trait + [`ColorTool`] |
//! | `gui/colors/adjbases.py:62-359` `ColorManager` 的通知侧 | [`ColorTool::set_color`] / [`ColorTool::push_history`] |
//! | `gui/colors/adjbases.py:432-830` `ColorAdjusterWidget` 指针状态机 | [`ColorChanger`] / [`bend_hcy`] |
//! | `gui/colors/adjbases.py:933-1083` `SliderColorAdjuster` | [`point_to_amount`] / [`scroll_delta`] / [`ComponentSlider`] |
//! | `gui/colors/sliders.py:163-305` 九个分量滑条 | [`SliderKind`] / [`ComponentSlider`] |
//! | `gui/colors/combined.py:20-66` `CombinedAdjusterPage` | [`ColorTool`] 的 adjuster 注册表 |
//!
//! ## 门槛
//!
//! `crates/pigment-engine/tests/color_vectors.rs` 拿
//! `tests/fixtures/color_vectors.txt` 逐点比对，fixture 由
//! `crates/pigment-engine/tests/tools/gen-color-vectors.py` **直接跑真
//! `lib/color.py`** 生成（浮点用 `repr()`，Rust 侧 `assert_eq!` 逐位）。
//! 测试数：23795 条向量。
//!
//! ## 没移植的（记账，见交付说明）
//!
//! - `lib/color.py:222-262` `new_from_pixbuf_average()`：吃 `GdkPixbuf`，
//!   Rust 侧没有对应物（背景色的平均色需要时由调用方自己算）。
//! - `lib/color.py:919-930` 的 doctest 运行器：Python 专属。
//! - `gui/colors/` 的控件（色环/色轮/色立方/滑条/调色板视图）、`palette`、
//!   `colorpreview`、`accent`/`uimisc`：B4-3 / B4-4 范围。
//! - 颜色工具的拖动/滑条几何与 GTK 装配（`gui/colors/adjbases.py` 的
//!   `ColorAdjusterWidget`、`sliders.py` 的 Gtk.Grid 页面、`colortools.py`）：
//!   模型层已在 [`ColorChanger`] / [`ComponentSlider`] 里，控件层属 B4-3。

mod adjust;
mod manager;
mod model;
mod space;
mod tools;

pub use adjust::{
    BrushStep, ColorAction, BRUSH_OPAQUE_FACTOR, BRUSH_RADIUS_STEP,
    CONTEXT_RESTORES_COLOR_DEFAULT, HUE_STEP, LEVEL_CEIL, LEVEL_FLOOR, LEVEL_STEP, OPAQUE_MAX,
    OPAQUE_MIN, RADIUS_LOG_MAX, RADIUS_LOG_MIN,
};
pub use manager::{
    ColorManager, DEFAULT_HIST_HEX, HIST_LEN, PREFS_KEY_COLOR_HISTORY, PREFS_KEY_CONTEXT_RESTORES_COLOR,
    PREFS_KEY_CURRENT_COLOR,
};
pub use model::{
    colors_match, Color, HcyColor, HsvColor, RgbColor, UIColor, YcbcrColor, CONTRASTING_K,
    HEX_DEFAULT_RGB,
};
pub use space::{
    hcy_to_rgb, hsv_to_rgb, rgb_to_hcy, rgb_to_hsv, rgb_to_ycbcr_bt601, ycbcr_to_rgb_bt601,
    HCY_BLUE_LUMA, HCY_GREEN_LUMA, HCY_RED_LUMA,
};
pub use tools::{
    bend_axis, bend_hcy, point_to_amount, scroll_delta, BendAxis, ColorAdjuster, ColorChanger,
    ColorTool, ComponentSlider, DragUpdate, Modifiers, PressOutcome, ScrollDirection, SliderKind,
    BORDER_WIDTH, DEFAULT_MANAGED_COLOR, DRAG_THRESHOLD_CAP, DRAG_THRESHOLD_FACTOR, SCROLL_DELTA,
};
