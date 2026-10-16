//! MyPaint 的**窗口骨架** —— 照搬 `gui/mypaint.glade` 的三段结构。
//!
//! ```text
//! MyPaintDrawWindow
//! └ GtkBox(vertical)                                  id=drawwindow_layout_vbox
//!   ├ MyPaintTopBar      id=app_topbar               菜单栏 + toolbar1 + toolbar2
//!   ├ MyPaintWorkspace   id=app_workspace            可停靠面板系统（左右侧栏 + 中央画布）
//!   │ └ TiledDrawWidget  id=app_canvas               expand=True
//!   └ GtkBox             id=footer_box               12 个部件的底栏
//! ```
//!
//! egui 侧的对应实现是**面板压边**：`Panel::top("topbar")` 与
//! `Panel::bottom("footer")` 先各切走一条，剩下的矩形归
//! [`workspace`]（内部再切左右侧栏，中央留给画布）。
//!
//! **技术栈适配**（唯一一处结构级）：GTK 的 `MyPaintWorkspace` 是一个可停靠
//! 面板系统（`gui/workspace.py` + `gui/toolstack.py`，基于 `Gtk.Notebook` 与
//! 可拖出成独立窗口的工具面板）。egui 没有对应物，S6 会用 `egui_dock` 承接；
//! 在那之前，左右侧栏先用 `Panel::left/right` 直接压边 —— 交互结果等价
//! （面板在画布左侧/右侧、可拖宽、可整块收起），只是缺少"拖出成浮动窗口"
//! 与"用户自定义布局持久化"。
//!
//! 尺寸常量**全部有源码出处**，逐条标在常量定义处。凡是"本项目自定"的
//! 数值一律不在此文件出现。

pub(crate) mod actions;
pub(crate) mod footer;
pub(crate) mod menubar;
pub(crate) mod toolbars;
pub(crate) mod topbar;
pub(crate) mod workspace;

// ---- 顶栏 ----

/// 顶栏最小高度 —— `mypaint.glade:17` 的 `height_request = 24`。
///
/// MyPaint 里这只是个下界（GTK 会按内容撑开），这里用作 egui 的最小行高。
pub(crate) const TOPBAR_MIN_HEIGHT: f32 = 24.0;

// ---- 底栏 ----

/// 底栏部件的**顺序表** —— 逐项照搬 `mypaint.glade` 的 `footer_box` 子项
/// `position` 0 → 11。名字即 glade 里的 widget id（无 id 的分隔符按 glade 里的
/// `separator1` / `separator3` / `separator2` 计）。
///
/// 顺序**不是**可有可无的细节：`app_statusbar` 的 `expand=True` 会把其后三项
/// 推到最右边，构成「左侧颜色组 | 缩放 | 模式图标 | ……状态栏…… | 笔刷组」的布局。
//
// 顺序表由测试与文档引用；实际的横向排布直接写在 `footer::PigmentApp::footer()`
// 里（egui 的横向布局没法像 GTK 那样遍历异质子项）。
#[allow(dead_code)]
pub(crate) const FOOTER_WIDGETS: [&str; 12] = [
    "footer_color_picker_button",             // 0  取色模式
    "colorindicator1",                        // 1  当前色 / 上一色
    "footer_color_details_button",            // 2  颜色详情
    "footer_bookmark_current_color_button",   // 3  当前色加入调色板
    "separator1",                             // 4
    "app_canvas_scale_label",                 // 5  缩放与旋转
    "separator3",                             // 6
    "app_current_mode_icon",                  // 7  当前工具模式图标
    "app_statusbar",                          // 8  expand=True
    "separator2",                             // 9
    "footer_brush_indicator_drawingarea",     // 10 笔刷指示器
    "footer_context_picker_button",           // 11 上下文取色
];

/// 底栏颜色指示器的宽度 —— `mypaint.glade:85` 的 `width_request = 96`。
///
/// 注意 `PreviousCurrentColorAdjuster.__init__` 自己只要 8×8
/// （`gui/colors/adjbases.py:870`：`BORDER_WIDTH*2 + 4`），是 glade 把它覆盖成 96 的。
/// 源码里的 TODO 注释说这是刻意的：「先按 1:5 的长宽比画，看看用户喜不喜欢」。
pub(crate) const FOOTER_COLOR_INDICATOR_WIDTH: f32 = 96.0;

/// 底栏缩放标签的宽度 —— `mypaint.glade:158` 的 `width_request = 48`。
pub(crate) const FOOTER_SCALE_LABEL_WIDTH: f32 = 48.0;

/// 底栏图标尺寸 —— `mypaint.glade` 里所有 `GtkImage` 的 `pixel_size = 16`。
pub(crate) const FOOTER_ICON_SIZE: f32 = 16.0;

/// 底栏笔刷指示器的宽度 —— `mypaint.glade:230` 的 `width_request = 48`。
///
/// 源码注释：「48px 是这块 DrawingArea 的合适宽度」。
pub(crate) const FOOTER_BRUSH_INDICATOR_WIDTH: f32 = 48.0;

/// 底栏分隔符的左右留白 —— `mypaint.glade` 里 `GtkSeparator` 打包时的 `padding = 4`。
pub(crate) const FOOTER_SEPARATOR_PADDING: f32 = 4.0;

/// 缩放标签的字号缩放 —— `mypaint.glade:166` 的 Pango `<attribute name="scale">`
/// 值 `0.66666666666665997`。
pub(crate) const FOOTER_SCALE_LABEL_TEXT_SCALE: f32 = 2.0 / 3.0;

/// 底栏各文本的鼠标提示，**逐字**照搬 `mypaint.glade` 的 `tooltip_text`。
pub(crate) const TIP_COLOR_PICKER: &str = "Pick color";
pub(crate) const TIP_COLOR_DETAILS: &str = "Edit the current color in detail";
pub(crate) const TIP_COLOR_BOOKMARK: &str = "Add the current color to the palette";
pub(crate) const TIP_SCALE_LABEL: &str = "Canvas scale and rotation";
pub(crate) const TIP_CONTEXT_PICKER: &str =
    "Pick brushstroke settings, stroke color, and layer";
/// 颜色指示器的提示 —— `adjbases.py:862` 的 `STATIC_TOOLTIP_TEXT`。
pub(crate) const TIP_COLOR_INDICATOR: &str =
    "Current brush color, and the color most recently used for painting";

// ---- 颜色指示器的绘制常量（`gui/colors/adjbases.py`）----

/// `adjbases.py:455` `BORDER_WIDTH = 2`。
pub(crate) const COLOR_INDICATOR_BORDER_WIDTH: f32 = 2.0;
/// `adjbases.py:457` `OUTLINE_WIDTH = 3`。
pub(crate) const COLOR_INDICATOR_OUTLINE_WIDTH: f32 = 3.0;
/// `adjbases.py:458` `OUTLINE_RGBA = (0, 0, 0, 0.4)`。
pub(crate) const COLOR_INDICATOR_OUTLINE_RGBA: [f32; 4] = [0.0, 0.0, 0.0, 0.4];
/// `adjbases.py:459` `EDGE_HIGHLIGHT_WIDTH = 1.0`。
pub(crate) const COLOR_INDICATOR_HIGHLIGHT_WIDTH: f32 = 1.0;
/// `adjbases.py:460` `EDGE_HIGHLIGHT_RGBA = (1, 1, 1, 0.25)`。
pub(crate) const COLOR_INDICATOR_HIGHLIGHT_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 0.25];

// ---- 笔刷指示器的绘制常量（`gui/footer.py`）----

/// `footer.py:63` `_EDGE_HIGHLIGHT_RGBA = (1, 1, 1, 0.25)`。
///
/// （源码里还有个 `_TOOLTIP_ICON_SIZE = 48`，指 tooltip 里那张预览图的边长；
///  egui 的悬停文本没有图标位，暂不涉及，等 S5 决定 tooltip 形态时再补。）
pub(crate) const BRUSH_INDICATOR_EDGE_HIGHLIGHT_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 0.25];
/// `footer.py:64` `_OUTLINE_RGBA = (0, 0, 0, 0.4)`。
pub(crate) const BRUSH_INDICATOR_OUTLINE_RGBA: [f32; 4] = [0.0, 0.0, 0.0, 0.4];
/// `footer.py:65` `_DEFAULT_BRUSH_DISPLAY_NAME = _("Unknown Brush")`。
pub(crate) const BRUSH_INDICATOR_DEFAULT_NAME: &str = "Unknown Brush";

/// 笔刷指示器底部渐隐蒙版的四个色标 —— `footer.py:174-178`。
///
/// `(位置, alpha)`：0.0 与 0.8 全不透明，0.95 半透，1.0 近透。预览过高时
/// 底部会被这个蒙版吃掉（正是那行 `FIXME` 想要的「可爱渐变」）。
pub(crate) const BRUSH_INDICATOR_MASK_STOPS: [(f32, f32); 4] =
    [(0.0, 1.0), (0.8, 1.0), (0.95, 0.5), (1.0, 0.1)];

// ---- 当前工具模式（只有自由笔，其余随 B3 工具模式移植）----

/// 模式名 —— `gui/freehand.py:55` `FreehandMode.get_name()`。
pub(crate) const MODE_NAME: &str = "Freehand Drawing";
/// 状态栏文字 —— `gui/freehand.py:58` `FreehandMode.get_usage()`。
///
/// `drawwindow.py:794` 的 `_update_status_bar_mode_widgets` 往状态栏推的就是
/// 这个串（源码里 `"{usage!s}".format(name=..., usage=...)` 只用到 `usage`）。
pub(crate) const MODE_USAGE: &str = "Paint free-form brush strokes";
/// 模式图标的 tooltip description —— `gui/resources.xml:1637` 里 `FreehandMode`
/// 动作的 tooltip（`drawwindow.py:807-815` 优先取动作 tooltip，取不到才退回 usage）。
pub(crate) const MODE_DESCRIPTION: &str =
    "Freehand: draw and paint freely, without geometric constraints.";

/// 底栏状态栏显示的模式文字 —— 照搬 `drawwindow.py:793-797`。
pub(crate) fn mode_status_text() -> String {
    MODE_USAGE.to_string()
}

/// 底栏模式图标的 tooltip —— 模板 `<b>{name}</b>\n{description}`
/// （`drawwindow.py:77` 的 `_MODE_ICON_TEMPLATE`）。egui 无 markup，去掉 `<b>`。
pub(crate) fn mode_icon_tooltip() -> String {
    format!("{MODE_NAME}\n{MODE_DESCRIPTION}")
}

/// 缩放/旋转标签的文字 —— 照搬 `gui/drawwindow.py:765-776`。
///
/// 源码逻辑（逐行对应）：
/// ```python
/// scale = renderer.scale * 100.0
/// rotation = (renderer.rotation / (2 * math.pi)) % 1.0
/// if rotation > 0.5: rotation -= 1.0        # → (-0.5, 0.5]
/// rotation *= 360.0                          # → (-180°, 180°]
/// label.set_text(template.format(scale=scale, rotation=rotation))
/// ```
/// 模板本身来自 glade（`Z: {scale:.1f}%\nR: {rotation:+.1f}°`）。
///
/// `rotation_rad` 目前恒为 0：画布旋转（`tileddrawwidget`）尚未移植，
/// 而 MyPaint 未旋转时它本来就是 0，因此这里的输出与源码一致。
pub(crate) fn scale_label_text(scale: f32, rotation_rad: f32) -> String {
    let mut rotation = (rotation_rad / (2.0 * std::f32::consts::PI)).rem_euclid(1.0);
    if rotation > 0.5 {
        rotation -= 1.0;
    }
    rotation *= 360.0;
    format!("Z: {:.1}%\nR: {:+.1}°", scale * 100.0, rotation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_widget_order_matches_glade_positions() {
        // 顺序即 glade 的 position 0..11；statusbar 必须排在 brush indicator 之前，
        // 否则它的 expand 会把笔刷组推到中间去。
        assert_eq!(FOOTER_WIDGETS.len(), 12);
        assert_eq!(FOOTER_WIDGETS[0], "footer_color_picker_button");
        assert_eq!(FOOTER_WIDGETS[1], "colorindicator1");
        assert_eq!(FOOTER_WIDGETS[2], "footer_color_details_button");
        assert_eq!(FOOTER_WIDGETS[3], "footer_bookmark_current_color_button");
        assert_eq!(FOOTER_WIDGETS[5], "app_canvas_scale_label");
        assert_eq!(FOOTER_WIDGETS[7], "app_current_mode_icon");
        assert_eq!(FOOTER_WIDGETS[8], "app_statusbar");
        assert_eq!(FOOTER_WIDGETS[10], "footer_brush_indicator_drawingarea");
        assert_eq!(FOOTER_WIDGETS[11], "footer_context_picker_button");
        let statusbar = FOOTER_WIDGETS.iter().position(|w| *w == "app_statusbar").unwrap();
        let brush = FOOTER_WIDGETS
            .iter()
            .position(|w| *w == "footer_brush_indicator_drawingarea")
            .unwrap();
        assert!(statusbar < brush, "状态栏的 expand 要把它后面的项推到最右");
    }

    #[test]
    fn scale_label_follows_drawwindow_normalization() {
        // 未旋转 → +0.0°（Python 的 "{:+.1f}".format(0.0) 同样是 "+0.0"）
        assert_eq!(scale_label_text(1.0, 0.0), "Z: 100.0%\nR: +0.0°");
        // 90° → 归一化到 (-180, 180] 内仍是 +90.0
        let quarter = std::f32::consts::FRAC_PI_2;
        assert_eq!(scale_label_text(0.5, quarter), "Z: 50.0%\nR: +90.0°");
        // 270° 顺时针 → -90.0（源码那句 `if rotation > 0.5: rotation -= 1.0`）
        let three_quarter = 3.0 * std::f32::consts::FRAC_PI_2;
        assert_eq!(scale_label_text(2.0, three_quarter), "Z: 200.0%\nR: -90.0°");
        // 180° 的边界：0.5 不触发减一 → +180.0
        assert_eq!(scale_label_text(1.0, std::f32::consts::PI), "Z: 100.0%\nR: +180.0°");
    }

    #[test]
    fn brush_indicator_mask_stops_are_the_source_ones() {
        assert_eq!(BRUSH_INDICATOR_MASK_STOPS[0], (0.0, 1.0));
        assert_eq!(BRUSH_INDICATOR_MASK_STOPS[1], (0.8, 1.0));
        assert_eq!(BRUSH_INDICATOR_MASK_STOPS[2], (0.95, 0.5));
        assert_eq!(BRUSH_INDICATOR_MASK_STOPS[3], (1.0, 0.1));
    }
}
