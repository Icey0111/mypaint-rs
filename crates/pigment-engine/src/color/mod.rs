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
//! B4-4a 追加（`color/palette.rs`，调色板的模型层）：
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `lib/palette.py:36-859` `Palette` | [`Palette`] / [`Swatch`] |
//! | `lib/palette.py:865-897` `_outwards_from()` / `_color_distance()` | [`outwards_from`] / [`color_distance`] |
//! | `lib/palette.py:831-859` 偏好的简单字典 | [`SimplePaletteDict`] |
//! | `gui/colors/adjbases.py:139-147` 默认调色板（读 `.gpl`） | [`Palette::mypaint_default`] + [`DEFAULT_PALETTE_GPL`] |
//! | `gui/colors/adjbases.py:219` `set_color()` 里的 `match_color` | [`ColorManager::set_color`] / [`ColorManager::palette`] |
//!
//! ## 门槛
//!
//! `crates/pigment-engine/tests/color_vectors.rs` 拿
//! `tests/fixtures/color_vectors.txt` 逐点比对，fixture 由
//! `crates/pigment-engine/tests/tools/gen-color-vectors.py` **直接跑真
//! `lib/color.py`** 生成（浮点用 `repr()`，Rust 侧 `assert_eq!` 逐位）。
//! 测试数：23795 条向量。
//!
//! B4-4b 追加（`color/wheels.rs`，色轮 / 色方块 / 色立方的模型层）：
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `gui/colors/adjbases.py:1086-1338` `HueSaturationWheelMixin` | [`WheelGeometry`] / [`wheel_radius`] / [`wheel_center`] / [`hue_from_angle`] / [`saturation_from_radius`] |
//! | `gui/colors/adjbases.py:80-95` + `:289-345` 色相畸变表 | [`WheelType`] / [`distort_hue`] / [`undistort_hue`] + [`ColorManager`] 的同名方法 |
//! | `gui/colors/adjbases.py:1233-1240` + `util.py:27-40` 径向去饱和 | [`distance_fade_stops`] |
//! | `gui/colors/hsvwheel.py:35-57` HSV 轮 | [`hsv_wheel_polar_for_color`] / [`hsv_wheel_color_at_polar`] / [`hsv_wheel_scroll`] |
//! | `gui/colors/hsvsquare.py:138-412` HSV 方块（环 + 内方块） | [`hsv_ring_color_at_polar`] / [`slice_color_at_position`] / [`slice_position_for_color`] |
//! | `gui/colors/hsvcube.py:100-247` HSV 立方（切片 + 深度滑条） | [`tumble_faces`] / [`depth_slider_color`] / [`depth_slider_amount`] |
//! | `gui/colors/uimisc.py:21-26` 最小尺寸 | [`PRIMARY_ADJUSTERS_MIN_WIDTH`] 等 |
//!
//! 控件层（三页的绘制与指针事件）在
//! `crates/pigment-app/src/shell/wheel_pages.rs`。
//!
//! ## 门槛（B4-4b）
//!
//! `crates/pigment-engine/tests/wheel_vectors.rs` 拿
//! `tests/fixtures/wheel_vectors.txt` 逐位比对；fixture 由
//! `crates/pigment-engine/tests/tools/gen-wheel-vectors.py` 用 `ast` 从真源码
//! 抽出函数体后 `exec` 生成（几何、畸变表、两种色轮的钩子、方块/立方的
//! 坐标换算、深度滑条、滚轮、径向渐变色标）。
//!
//! ## 没移植的（B4-4b 记账）
//!
//! - `adjbases.py:1169-1285` 的 Cairo 绘制、`:536-601` 的 DnD、
//!   `:679-695` 的双击详情对话框：控件层，见 `shell/wheel_pages.rs` 的记账。
//!
//! B4-4c 追加（`color/hcywheel.rs`，HCY 轮 + 色域遮罩的模型层）：
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `gui/colors/hcywheel.py:320-336` `HCYHueChromaWheelMixin` | [`hcy_wheel_polar_for_color`] / [`hcy_wheel_color_at_polar`] |
//! | `gui/colors/hcywheel.py:180-244` 遮罩 void（凸包、尺寸） | [`mask_voids`] / [`colors_to_mask_void`] / [`visible_mask_voids`] / [`void_size`] |
//! | `gui/colors/hcywheel.py:206-238` 遮罩取色裁剪 | [`masked_color_at_position`] |
//! | `gui/colors/hcywheel.py:868-988` 五个 mood 预设 | [`mask_templates`] / [`template_mask`] / [`MaskTemplate`] |
//! | `gui/colors/hcywheel.py:356-367` 轮上滚轮改明度 | [`hcy_wheel_scroll`] |
//! | `gui/colors/hcywheel.py:43-67` / `:303-311` / `:390-396` 常量 | [`MIN_SHAPE_SIZE`] / [`MASK_DRAW_ALPHA`] 等 |
//! | `lib/alg.py:25-186` 多边形算法子集 | [`convex_hull`] / [`poly_area`] / [`point_in_convex_poly`] / [`nearest_point_in_segment`] / [`pairwise`] |
//! | `gui/colors/sliders.py:287-292` `HCYLumaSlider.samples` | [`hcy_luma_slider_samples`] |
//!
//! 控件层（盘面绘制、遮罩画灰、指针事件）在
//! `crates/pigment-app/src/shell/hcy_wheel_page.rs`。
//! **未移植**：遮罩编辑交互（`HCYMaskEditorWheel`，`hcywheel.py:370-822`）、
//! `HCYMaskPropertiesDialog` / `HCYMaskTemplateDialog`（`:860-1265`）、
//! 遮罩的偏好持久化与 GIMP 调色板 I/O —— 逐条见 `hcywheel.rs` 模块文档。
//!
//! ## 门槛（B4-4c）
//!
//! 同一份 `tests/fixtures/wheel_vectors.txt`，`hcy_*` 行由
//! `tests/tools/gen-wheel-vectors.py` 从 `hcywheel.py` / `sliders.py` 抽方法体、
//! **真 import** `lib/alg.py`（只依赖 `math`）生成；断言在
//! `tests/wheel_vectors.rs` 的 HCY 部分。

mod adjust;
mod hcywheel;
mod manager;
mod model;
mod palette;
mod space;
mod tools;
mod wheels;

pub use adjust::{
    BrushStep, ColorAction, BRUSH_OPAQUE_FACTOR, BRUSH_RADIUS_STEP, CONTEXT_RESTORES_COLOR_DEFAULT,
    HUE_STEP, LEVEL_CEIL, LEVEL_FLOOR, LEVEL_STEP, OPAQUE_MAX, OPAQUE_MIN, RADIUS_LOG_MAX,
    RADIUS_LOG_MIN,
};
pub use hcywheel::{
    colors_to_mask_void, convex_hull, editor_luma_y, hcy_luma_slider_samples, hcy_wheel_center_rgb,
    hcy_wheel_color_at_polar, hcy_wheel_color_at_position, hcy_wheel_polar_for_color,
    hcy_wheel_position_for_color, hcy_wheel_scroll, hcy_wheel_slice_rgb, mask_from_flat,
    mask_from_palette, mask_templates, mask_to_flat, mask_to_palette, mask_voids,
    masked_color_at_position, nearest_point_in_segment, pairwise, point_in_convex_poly, poly_area,
    rotate_mask, template_mask, visible_mask_voids, void_size, MaskShape, MaskTemplate, Point,
    EDITOR_CTRLPOINT_GRAB_RADIUS, EDITOR_CTRLPOINT_RADIUS, EDITOR_MAX_LUMA, EDITOR_MAX_NUM_SHAPES,
    EDITOR_MIN_LUMA, HCY_WHEEL_TOOLTIP, MASK_ACTIVE_LABEL, MASK_ACTIVE_TOOLTIP, MASK_DRAW_ALPHA,
    MASK_EDGE_LINE_WIDTH, MASK_EDITOR_TOOLTIP, MASK_MIN_VOID_POINTS, MIN_SHAPE_SIZE,
    PREFS_ACTIVE_KEY, PREFS_MASK_KEY,
};
pub use manager::{
    ColorManager, DEFAULT_HIST_HEX, HIST_LEN, PREFS_KEY_COLOR_HISTORY,
    PREFS_KEY_CONTEXT_RESTORES_COLOR, PREFS_KEY_CURRENT_COLOR, PREFS_KEY_PALETTE,
    PREFS_KEY_WHEEL_TYPE,
};
pub use model::{
    colors_match, Color, HcyColor, HsvColor, RgbColor, UIColor, YcbcrColor, CONTRASTING_K,
    HEX_DEFAULT_RGB,
};
pub use palette::{
    color_distance, outwards_from, Palette, PaletteLoadError, SimplePaletteDict, Swatch,
    DATAPATH_PALETTES_SUBDIR, DEFAULT_PALETTE_FILE, DEFAULT_PALETTE_GPL, EMPTY_SLOT_NAME,
    EMPTY_SLOT_RGB, MATCH_APPROX_DISTANCE, PREFS_PALETTE_DICT_KEY,
};
pub use space::{
    hcy_to_rgb, hsv_to_rgb, rgb_to_hcy, rgb_to_hsv, rgb_to_ycbcr_bt601, ycbcr_to_rgb_bt601,
    HCY_BLUE_LUMA, HCY_GREEN_LUMA, HCY_RED_LUMA,
};
pub use tools::{
    bend_axis, bend_hcy, point_to_amount, scroll_delta, BendAxis, ColorAdjuster, ColorChanger,
    ColorSink, ColorTool, ComponentSlider, DragUpdate, Modifiers, PressOutcome, ScrollDirection,
    SliderKind, BORDER_WIDTH, DEFAULT_MANAGED_COLOR, DRAG_THRESHOLD_CAP, DRAG_THRESHOLD_FACTOR,
    SCROLL_DELTA,
};
pub use wheels::{
    depth_slider_amount, depth_slider_color, depth_slider_scroll, distance_fade_stops, distort_hue,
    hsv_ring_color_at_polar, hsv_ring_color_at_position, hsv_ring_polar_for_color,
    hsv_ring_position_for_color, hsv_ring_slice_rgb, hsv_wheel_center_rgb,
    hsv_wheel_color_at_polar, hsv_wheel_color_at_position, hsv_wheel_polar_for_color,
    hsv_wheel_position_for_color, hsv_wheel_scroll, hsv_wheel_slice_rgb, hue_from_angle,
    hue_slice_hues, saturation_from_radius, slice_color_at_position, slice_faces,
    slice_position_for_color, tumble_faces, undistort_hue, wheel_center, wheel_radius, HsvFace,
    HueRange, WheelGeometry, WheelType, CUBE_SLIDER_SAMPLES, DEFAULT_WHEEL_TYPE,
    EDGE_HIGHLIGHT_RGBA, EDGE_HIGHLIGHT_WIDTH, HUE_SLICES, OUTLINE_RGBA, OUTLINE_WIDTH,
    PRIMARY_ADJUSTERS_MIN_HEIGHT, PRIMARY_ADJUSTERS_MIN_WIDTH, RING_SAMPLES, RYB_HUE_RANGES,
    RYGB_HUE_RANGES, SAT_GAMMA, SAT_SLICES, SLIDER_MIN_LENGTH, SLIDER_MIN_WIDTH, TABLE_PADDING,
};
