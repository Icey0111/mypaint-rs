//! 底栏 —— `footer_box`（`gui/mypaint.glade:56`）+ 两个行为件
//! （`gui/footer.py` 的笔刷指示器、`gui/colorpreview.py` 的颜色指示器）。
//!
//! 12 个部件按 glade 的 `position` 0→11 排布，顺序表见
//! [`super::FOOTER_WIDGETS`]。位置 8 的 `app_statusbar` 是 `expand=True` 的，
//! 它吃掉剩余宽度，把后面三项推到最右边 —— 所以实际观感是
//!
//! ```text
//! [取色][当前色|上一色][详情][收藏] │ [Z/R] │ [模式] ……状态栏…… │ [笔刷][上下文取色]
//! ```
//!
//! **技术栈适配**：底栏有 5 个 GTK symbolic 图标按钮
//! （`mypaint-pick-color-symbolic` 等）。GTK 自带图标主题，egui 没有，
//! 本项目也不引入图标字体，所以这里用极简矢量图形把同样语义画出来
//! （[`SymbolicIcon`]），**只求形态可辨、命中区域与源码一致**。

use eframe::egui;
use pigment_engine::color::UIColor;

use crate::app::PigmentApp;

use super::{
    mode_icon_tooltip, mode_status_text, scale_label_text, ADJUSTER_BORDER_WIDTH,
    BRUSH_INDICATOR_DEFAULT_NAME, BRUSH_INDICATOR_EDGE_HIGHLIGHT_RGBA, BRUSH_INDICATOR_MASK_STOPS,
    BRUSH_INDICATOR_OUTLINE_RGBA, FOOTER_BRUSH_INDICATOR_WIDTH, FOOTER_COLOR_INDICATOR_WIDTH,
    FOOTER_ICON_SIZE, FOOTER_SCALE_LABEL_TEXT_SCALE, FOOTER_SCALE_LABEL_WIDTH,
    FOOTER_SEPARATOR_PADDING, TIP_COLOR_BOOKMARK, TIP_COLOR_DETAILS, TIP_COLOR_INDICATOR,
    TIP_COLOR_PICKER, TIP_CONTEXT_PICKER, TIP_SCALE_LABEL,
};

/// 底栏一行的高度。
///
/// 源码里没有这个数 —— 它是 GTK 按子项自然高度撑出来的（最高的子项是
/// `GtkStatusbar` 与那两行 0.667 倍字号的缩放标签）。这里显式固定是为了让
/// **笔刷指示器的裁切行为可复现**：`footer.py:_draw_cb` 把 128px 预览等比
/// 缩放到 `max(aw,ah)-2` 后顶部对齐画进 48×这一行高的矩形，超出的部分被
/// `cr.clip()` 裁掉、底部再被渐变蒙版吃掉（源码 docstring 说的
/// "drawn truncated with a cute gradient effect"）。数值待 S5 目视校准时
/// 与真机对齐。
pub(crate) const FOOTER_HEIGHT: f32 = 26.0;

/// 底栏用到的那几个 GTK symbolic 图标（`mypaint-*-symbolic`）。
#[derive(Clone, Copy)]
enum SymbolicIcon {
    /// `mypaint-pick-color-symbolic`（取色模式）
    PickColor,
    /// `mypaint-edit-symbolic`（颜色详情）
    Edit,
    /// `mypaint-bookmark-new-symbolic`（当前色加入调色板）
    BookmarkNew,
    /// `mypaint-brush-symbolic`（当前工具模式）
    Brush,
    /// `mypaint-target-brush-symbolic`（上下文取色）
    TargetBrush,
}

/// 把带 alpha 的 RGBA（0..1）打成 egui 颜色。
fn rgba32(c: [f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c[0] * 255.0).round() as u8,
        (c[1] * 255.0).round() as u8,
        (c[2] * 255.0).round() as u8,
        (c[3] * 255.0).round() as u8,
    )
}

impl PigmentApp {
    /// 底栏整体 —— 12 个部件按 [`super::FOOTER_WIDGETS`] 的顺序。
    pub(crate) fn footer(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("footer")
            // 高度固定：底栏高度直接决定笔刷指示器的裁切与渐隐位置（见
            // `FOOTER_HEIGHT` 的说明），不能随内容浮动。
            .resizable(false)
            .default_size(FOOTER_HEIGHT)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // 用面板实际给出的内容高度，而不是常量 —— 常量是"面板外高"
                    // （含边框留白），两者差几个像素，直接拿来当行高会让
                    // 笔刷指示器的裁切位置对不上源码。
                    let h = ui.available_height().max(FOOTER_ICON_SIZE);
                    // ---- 0 取色按钮（`ColorPickMode`，属 B3 工具模式）----
                    self.footer_icon_slot(
                        ui,
                        SymbolicIcon::PickColor,
                        h,
                        TIP_COLOR_PICKER,
                        false,
                        "未移植：取色模式 ColorPickMode（属 B3 工具模式）",
                    );
                    // ---- 1 当前色 / 上一色 ----
                    self.footer_color_indicator(ui, h);
                    // ---- 2 颜色详情（MyPaint 弹 `ColorDetailsDialog`，属 B4 颜色器）----
                    self.footer_icon_slot(
                        ui,
                        SymbolicIcon::Edit,
                        h,
                        TIP_COLOR_DETAILS,
                        false,
                        "未移植：颜色详情对话框（属 B4 颜色器）",
                    );
                    // ---- 3 收藏当前色（写进调色板）----
                    // glade 把它接到 `palette_add_current_color_cb`
                    // （`gui/mypaint.glade:128`），与动作 `PaletteAddCurrentColor`
                    // 是**同一个**回调（`drawwindow.py:677-684`）。B4-4a 起可用。
                    // 源码里它还受 `_init_stategroups` 控制：当前色已在调色板里时
                    // 置灰（那套状态组未移植，记账）。
                    let bookmark = self.footer_icon_slot(
                        ui,
                        SymbolicIcon::BookmarkNew,
                        h,
                        TIP_COLOR_BOOKMARK,
                        true,
                        "",
                    );
                    if bookmark.clicked() {
                        let now = ui.input(|i| i.time);
                        // 与动作走同一条数据路径（`PaletteAddCurrentColor`）
                        let len = self.palette_add_current_color();
                        self.dock
                            .reveal(crate::shell::dock::PanelId::ComponentSlidersTool);
                        self.color_page = crate::shell::color_panel::PAGE_PALETTE;
                        self.set_status(format!("当前色已加入调色板（共 {len} 色）"), now);
                    }
                    // ---- 4 separator（padding 4）----
                    footer_separator(ui, h);
                    // ---- 5 缩放 / 旋转标签 ----
                    self.footer_scale_label(ui, h);
                    // ---- 6 separator ----
                    footer_separator(ui, h);
                    // ---- 7 当前工具模式图标 ----
                    self.footer_mode_icon(ui, h);
                    // ---- 8 状态栏（expand=True，吃掉剩余宽度）----
                    self.footer_statusbar(ui, h);
                    // ---- 9..11 靠右三项 ----
                    // 用 right_to_left 布局把这三项贴到右边缘，等价于 GTK 里
                    // 状态栏的 `expand` 把后续子项挤到最右。
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // ---- 11 上下文取色（`PickContext`：取笔刷设置 + 颜色 +
                        //        **图层**，需要图层栈，属 B2）----
                        self.footer_icon_slot(
                            ui,
                            SymbolicIcon::TargetBrush,
                            h,
                            TIP_CONTEXT_PICKER,
                            false,
                            "未移植：上下文取色需要图层（属 B2 图层与混合模式）",
                        );
                        // ---- 10 笔刷指示器 ----
                        self.footer_brush_indicator(ui, h);
                        // ---- 9 separator ----
                        footer_separator(ui, h);
                    });
                });
            });
    }

    /// 一个图标槽（取色 / 颜色详情 / 收藏 / 上下文取色 / 模式图标）。
    ///
    /// `enabled=false` 时用 `Sense::hover()` 分配：不做点击判定，但悬停提示
    /// 照常显示（提示里写明缺什么、归哪一块）。这与笔刷面板里
    /// 「未移植」菜单项的处置方式一致。
    fn footer_icon_slot(
        &self,
        ui: &mut egui::Ui,
        icon: SymbolicIcon,
        row_h: f32,
        tip: &str,
        enabled: bool,
        unported_tip: &str,
    ) -> egui::Response {
        let sense = if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(FOOTER_ICON_SIZE + 6.0, row_h), sense);
        if enabled && (resp.hovered() || resp.clicked()) {
            ui.painter()
                .rect_filled(rect, 3.0, ui.style().interact(&resp).bg_fill);
        }
        let fg = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        draw_symbolic_icon(ui.painter(), rect, icon, fg);
        if enabled {
            resp.on_hover_text(tip)
        } else {
            resp.on_hover_text(format!("{tip}\n{unported_tip}"))
        }
    }

    /// 1 号部件：当前色 / 上一色指示器。
    ///
    /// 控件本体是 `PreviousCurrentColorAdjuster`（`gui/colors/adjbases.py:854-930`），
    /// 底栏里装的是它的子类 `BrushColorIndicator`（`gui/colorpreview.py:18-53`）。
    /// 绘制照 `render_background_cb`（`:875-903`）：左半 = 当前色
    /// （`mgr.get_color()`）、右半 = 上一色（`mgr.get_previous_color()` = 历史
    /// 末项，即"最近一次作画用的色"），先描 `OUTLINE_RGBA` 的 3px 外框、再填两半、
    /// 最后描 `EDGE_HIGHLIGHT_RGBA` 的 1px 内高光，都按 `BORDER_WIDTH = 2` 内缩。
    /// 画法与面板里那一份共用 [`crate::shell::color_panel::paint_previous_current`]。
    ///
    /// 交互语义（**按下**即生效，不是抬起）：
    /// - 右半：`get_color_at_position()` 取上一色（`adjbases.py:918-925`），
    ///   按钮 1 按下时 `set_managed_color()`（`:671-672`）→ **恢复上一色**；
    /// - 左半：取当前色（等于没变），抬起后 `clicked` 事件弹颜色选择器
    ///   （`colorpreview.py:39-53`；`ColorChooserPopup` 未移植，落到状态栏）。
    fn footer_color_indicator(&mut self, ui: &mut egui::Ui, row_h: f32) {
        use crate::shell::color_panel::{hit_half, paint_previous_current, ColorHalf};

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(FOOTER_COLOR_INDICATOR_WIDTH, row_h),
            egui::Sense::click(),
        );
        let current = self.color_manager.get_color();
        let previous = self.color_manager.get_previous_color();
        let b = ADJUSTER_BORDER_WIDTH;
        if rect.width() - b * 2.0 <= 0.0 || rect.height() - b * 2.0 <= 0.0 {
            return;
        }
        paint_previous_current(
            ui.painter(),
            rect,
            current.get_rgb(),
            previous.get_rgb(),
        );

        let tip = format!(
            "{TIP_COLOR_INDICATOR}\n左半 = 当前色 {}（点击弹颜色选择器，属 B4）\n右半 = 上一色 {}（按下即恢复）",
            current.to_hex_str(),
            previous.to_hex_str(),
        );
        let resp = resp.on_hover_text(tip);
        // 按下即恢复上一色（`adjbases.py:671-672` + `:918-925`）
        if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
            if let Some(pos) = resp.interact_pointer_pos() {
                if hit_half(rect, pos) == ColorHalf::Previous {
                    // 恢复上一色：设成当前色并重推笔刷（`set_color` 内会 `derive_active`）
                    self.set_color(previous);
                }
            }
        }
        if resp.clicked() {
            // 照 `BrushColorIndicator._clicked_cb`（colorpreview.py:39-53）：
            // 只有左半响应，右半没有额外动作（恢复已在按下时完成）。
            if let Some(pos) = resp.interact_pointer_pos() {
                if hit_half(rect, pos) == ColorHalf::Current {
                    // B4 的颜色选择器弹层未移植前，让"点了没反应"变成"点了知道为什么"
                    let now = ui.input(|i| i.time);
                    self.set_status(
                        "未移植：颜色选择器 ColorChooserPopup（属 B4 颜色器）".to_string(),
                        now,
                    );
                }
            }
        }
    }

    /// 5 号部件：缩放 / 旋转标签（48px 宽、0.667 倍字号、两行）。
    fn footer_scale_label(&self, ui: &mut egui::Ui, row_h: f32) {
        // 渲染尺寸 = 正文基准字号 × Pango scale（glade 里那个 0.667）
        let base = ui
            .style()
            .text_styles
            .get(&egui::TextStyle::Small)
            .map(|f| f.size)
            .unwrap_or(9.0);
        let text = scale_label_text(self.view.zoom, 0.0);
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(FOOTER_SCALE_LABEL_WIDTH, row_h),
            egui::Sense::hover(),
        );
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(base * FOOTER_SCALE_LABEL_TEXT_SCALE),
            ui.visuals().text_color(),
        );
        resp.on_hover_text(TIP_SCALE_LABEL);
    }

    /// 7 号部件：当前工具模式图标（16px）。
    ///
    /// 值照搬 `drawwindow.py:793-797`：图标取 `mode.get_icon_name()` —— 而
    /// `mode.get_icon_name()`（`gui/mode.py:263`）取的是**关联动作的 icon-name**，
    /// 自由笔模式对应 `resources.xml:1634` 的 `FreehandMode` 动作，
    /// 其 `icon-name` 是 `mypaint-brush-symbolic`。tooltip 模板是
    /// `<b>{name}</b>\n{description}`。
    fn footer_mode_icon(&mut self, ui: &mut egui::Ui, row_h: f32) {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(FOOTER_ICON_SIZE + 6.0, row_h),
            egui::Sense::hover(),
        );
        // B3：图标跟着**当前模式**走（mode.get_icon_name() 取关联动作的
        // icon-name，drawwindow.py:793），取不到时退回自由笔的图标。
        let fg = ui.visuals().text_color();
        let ctx = ui.ctx().clone();
        let tex = self
            .mode_icon_name()
            .and_then(|name| crate::shell::toolbars::icon_texture(&ctx, &mut self.icon_cache, name));
        match tex {
            Some(tex) => {
                ui.painter().image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    fg,
                );
            }
            None => draw_symbolic_icon(ui.painter(), rect, SymbolicIcon::Brush, fg),
        }
        resp.on_hover_text(mode_icon_tooltip());
    }

    /// 8 号部件：状态栏（`GtkStatusbar`，`expand=True`）。
    ///
    /// 这一格的**内容**分两层：
    /// 1. 模式用法文字 —— 照搬 `_update_status_bar_mode_widgets`
    ///    （`drawwindow.py:793`），推送的是 `mode.get_usage()`。
    /// 2. 本项目现有的临时状态文字与当前文件名 —— MyPaint 把文件名放在
    ///    **窗口标题**里（`update_title`，`drawwindow.py:276`），我们暂未改
    ///    窗口标题，先寄存在状态栏（标注为权宜，S5 目视校准时决定去处）。
    fn footer_statusbar(&mut self, ui: &mut egui::Ui, row_h: f32) {
        let now = ui.input(|i| i.time);
        let transient = self
            .status
            .as_ref()
            .filter(|(_, until)| now < *until)
            .map(|(msg, _)| msg.clone());
        let file = self
            .current_file
            .as_ref()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string())
            })
            .unwrap_or_else(|| "未命名".to_string());

        // B3-1 起状态栏第一段用**当前模式**的 usage（`mode.get_usage()`，
        // 与 `_update_status_bar_mode_widgets` 同源），不再写死文案。
        let usage = match self.modes.top().usage() {
            "" => mode_status_text(),
            u => u.to_string(),
        };
        let mut text = format!("{}  —  {usage}", self.modes.top().name());
        if let Some(msg) = &transient {
            text.push_str("  ·  ");
            text.push_str(msg);
        }
        text.push_str("  ·  ");
        text.push_str(&file);

        // 状态栏吃满剩余宽度，文字左对齐（GtkStatusbar 的默认观感）
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, row_h), egui::Sense::hover());
        let base = ui
            .style()
            .text_styles
            .get(&egui::TextStyle::Small)
            .map(|f| f.size)
            .unwrap_or(9.0);
        ui.painter().text(
            egui::pos2(rect.left() + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(base * FOOTER_SCALE_LABEL_TEXT_SCALE),
            ui.visuals().weak_text_color(),
        );
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
    }

    /// 10 号部件：笔刷指示器（48px 宽）。
    ///
    /// 照搬 `BrushIndicatorPresenter._draw_cb`（`gui/footer.py:120-179`）：
    /// 1. 预览等比缩放到 `max(aw,ah) - 2`，水平居中、**顶部对齐 y = 2**；
    /// 2. 裁切矩形是 `(1, 2, aw-2, ah)` —— 底部不裁，所以预览高于 widget 时
    ///    被截断；
    /// 3. 四周先描 `(0,0,0,0.4)` 的 3px 阴影，**底边不描**（矩形从 y=2.5 起算）；
    /// 4. 顶/左/右描 `(1,1,1,0.25)` 的 1px 高光，同样不描底边；
    /// 5. 最后整体套一层纵向渐变蒙版，把底部渐隐掉。
    ///
    /// tooltip 是 `<b>{name}</b>\n{desc}`（无 description 时只有首行）。
    fn footer_brush_indicator(&mut self, ui: &mut egui::Ui, row_h: f32) {
        use crate::thumb::{PREVIEW_H, PREVIEW_W};

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(FOOTER_BRUSH_INDICATOR_WIDTH, row_h),
            egui::Sense::click(),
        );
        let idx = self.brushes.selected;
        let entry = self.brushes.selected_entry().cloned();
        let tex = self.brushes.preview(ui.ctx(), idx);
        let painter = ui.painter();

        // 等比缩放到 max(aw, ah) - 2（`footer.py:154-159`）
        let scale = (rect.width().max(rect.height()) - 2.0) / PREVIEW_W.max(PREVIEW_H) as f32;
        let (pw, ph) = (PREVIEW_W as f32 * scale, PREVIEW_H as f32 * scale);
        let img = egui::Rect::from_min_size(
            egui::pos2(rect.center().x - pw / 2.0, rect.min.y + 2.0),
            egui::vec2(pw, ph),
        );

        // 裁切到 (1, 2, aw-2, ah)：底部溢出被截断（`footer.py:148-149` 的 cr.clip）
        let clip = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 1.0, rect.min.y + 2.0),
            egui::pos2(rect.max.x - 1.0, rect.max.y),
        );
        let painter = painter.with_clip_rect(clip);
        draw_masked_preview(
            &painter,
            tex.id(),
            img,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            rect,
        );

        // 阴影 + 高光（都从 y=2.5 起算，所以底边不描）
        let frame = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 1.5, rect.min.y + 2.5),
            egui::pos2(rect.max.x - 1.5, rect.max.y),
        );
        ui.painter().rect_stroke(
            frame,
            0.0,
            egui::Stroke::new(3.0, rgba32(BRUSH_INDICATOR_OUTLINE_RGBA)),
            egui::StrokeKind::Middle,
        );
        ui.painter().rect_stroke(
            frame,
            0.0,
            egui::Stroke::new(1.0, rgba32(BRUSH_INDICATOR_EDGE_HIGHLIGHT_RGBA)),
            egui::StrokeKind::Middle,
        );

        // tooltip：`<b>{name}</b>\n{desc}`（`footer.py:181-204`）。
        // 源码还会在 tooltip 里贴一张 48px 的预览图（`_TOOLTIP_ICON_SIZE`），
        // egui 的 hover 文本没有图标位，故只保留文字部分。
        let (name, desc) = match &entry {
            Some(e) => (e.mypaint_name(), e.display.clone()),
            None => (BRUSH_INDICATOR_DEFAULT_NAME.to_string(), String::new()),
        };
        let tip = if desc.is_empty() {
            name
        } else {
            format!("{name}\n{desc}")
        };
        let _ = resp.on_hover_text(tip);
    }
}

/// 底栏分隔符 —— glade 里这几个 `GtkSeparator` 打包时 `padding = 4`。
fn footer_separator(ui: &mut egui::Ui, row_h: f32) {
    ui.add_space(FOOTER_SEPARATOR_PADDING);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, row_h), egui::Sense::hover());
    ui.painter().vline(
        rect.center().x,
        (rect.top() + 2.0)..=(rect.bottom() - 2.0),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    ui.add_space(FOOTER_SEPARATOR_PADDING);
}

/// 画一个 symbolic 图标（**技术栈适配**：GTK 图标主题在 egui 无对应物）。
///
/// 每个图形只求"形态可辨 + 语义对得上源码里的图标名"，不追求像素级一致 ——
/// UI 层走软门禁（见《MyPaint移植路线图.md》第二节）。
fn draw_symbolic_icon(painter: &egui::Painter, slot: egui::Rect, icon: SymbolicIcon, fg: egui::Color32) {
    let r = egui::Rect::from_center_size(
        slot.center(),
        egui::Vec2::splat(FOOTER_ICON_SIZE - 2.0),
    );
    let stroke = egui::Stroke::new(1.4, fg);
    let p = painter;
    match icon {
        // 滴管：一根斜杆 + 头部小圆
        SymbolicIcon::PickColor => {
            p.line_segment(
                [egui::pos2(r.left() + 3.0, r.bottom() - 3.0), egui::pos2(r.right() - 4.0, r.top() + 4.0)],
                stroke,
            );
            p.circle_stroke(egui::pos2(r.right() - 4.0, r.top() + 4.0), 2.4, stroke);
            p.line_segment(
                [egui::pos2(r.left() + 1.0, r.bottom() - 1.0), egui::pos2(r.left() + 4.0, r.bottom() - 3.0)],
                stroke,
            );
        }
        // 铅笔：斜杆 + 笔尖三角
        SymbolicIcon::Edit => {
            p.line_segment(
                [egui::pos2(r.left() + 3.0, r.bottom() - 3.0), egui::pos2(r.right() - 3.0, r.top() + 3.0)],
                stroke,
            );
            p.line_segment(
                [egui::pos2(r.left() + 3.0, r.bottom() - 3.0), egui::pos2(r.left() + 6.0, r.bottom() - 6.0)],
                stroke,
            );
            p.line_segment(
                [egui::pos2(r.right() - 3.0, r.top() + 3.0), egui::pos2(r.right() - 6.0, r.top() + 6.0)],
                stroke,
            );
        }
        // 书签 + 加号
        SymbolicIcon::BookmarkNew => {
            let bw = r.width() * 0.55;
            let bx = r.left() + 1.0;
            let by = r.top() + 1.0;
            let bh = r.height() - 3.0;
            p.line_segment([egui::pos2(bx, by), egui::pos2(bx + bw, by)], stroke);
            p.line_segment(
                [egui::pos2(bx, by), egui::pos2(bx, by + bh)],
                stroke,
            );
            p.line_segment(
                [egui::pos2(bx + bw, by), egui::pos2(bx + bw, by + bh)],
                stroke,
            );
            p.line_segment(
                [
                    egui::pos2(bx, by + bh),
                    egui::pos2(bx + bw / 2.0, by + bh - 3.0),
                ],
                stroke,
            );
            p.line_segment(
                [
                    egui::pos2(bx + bw / 2.0, by + bh - 3.0),
                    egui::pos2(bx + bw, by + bh),
                ],
                stroke,
            );
            // 右上角的小加号
            let c = egui::pos2(r.right() - 3.0, r.top() + 3.0);
            p.line_segment([egui::pos2(c.x - 2.5, c.y), egui::pos2(c.x + 2.5, c.y)], stroke);
            p.line_segment([egui::pos2(c.x, c.y - 2.5), egui::pos2(c.x, c.y + 2.5)], stroke);
        }
        // 画笔：斜杆 + 笔头三角
        SymbolicIcon::Brush => {
            p.line_segment(
                [egui::pos2(r.right() - 3.0, r.top() + 2.0), egui::pos2(r.left() + 4.0, r.bottom() - 4.0)],
                stroke,
            );
            p.line_segment(
                [
                    egui::pos2(r.right() - 3.0, r.top() + 2.0),
                    egui::pos2(r.right() - 6.0, r.top() + 5.0),
                ],
                stroke,
            );
            p.line_segment(
                [egui::pos2(r.left() + 4.0, r.bottom() - 4.0), egui::pos2(r.left() + 1.0, r.bottom() - 1.0)],
                stroke,
            );
        }
        // 靶心 + 斜杆（"取这一笔的上下文"）
        SymbolicIcon::TargetBrush => {
            p.circle_stroke(r.center(), 4.0, stroke);
            p.circle_stroke(r.center(), 1.2, stroke);
            p.line_segment(
                [
                    egui::pos2(r.center().x, r.top()),
                    egui::pos2(r.center().x, r.top() + 2.0),
                ],
                stroke,
            );
        }
    }
}

/// 蒙版在部件坐标 `y` 处的 alpha（0..1）—— [`BRUSH_INDICATOR_MASK_STOPS`] 的线性插值。
fn mask_alpha_at(widget: egui::Rect, y: f32) -> f32 {
    let t = if widget.height() > 0.0 {
        (y - widget.top()) / widget.height()
    } else {
        0.0
    };
    let stops = BRUSH_INDICATOR_MASK_STOPS;
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        let (p0, a0) = w[0];
        let (p1, a1) = w[1];
        if t <= p1 {
            let f = if p1 > p0 { (t - p0) / (p1 - p0) } else { 0.0 };
            return a0 + (a1 - a0) * f;
        }
    }
    stops[stops.len() - 1].1
}

/// 图像矩形在纵向上的切分点：图像上下端 + 落在图像内部的色标位置。
///
/// 不切开的话，整块只能取两端值做一次线性插值，就把 0.8 / 0.95 两个拐点抹平了。
fn mask_cuts(img: egui::Rect, widget: egui::Rect) -> Vec<f32> {
    let mut cuts = vec![img.top()];
    for (pos, _) in BRUSH_INDICATOR_MASK_STOPS {
        let y = widget.top() + pos * widget.height();
        if y > img.top() + 0.5 && y < img.bottom() - 0.5 {
            cuts.push(y);
        }
    }
    cuts.push(img.bottom());
    cuts
}

/// 在 `rect` 里画出预览纹理，顶点 alpha 按 [`BRUSH_INDICATOR_MASK_STOPS`] 取值。
///
/// 对应 `footer.py:172-179` 的 `cr.pop_group_to_source()` + `cr.mask(mask)`：
/// 蒙版是一个纵向上 `(0,0)-(0,ah)` 的线性渐变，坐标是**部件坐标**（不是图像坐标），
/// 所以这里也按部件矩形来采样，并把图像矩形按色标位置切成若干条带，
/// 每条带两端取蒙版值 —— 线性插值下与遮罩等价。
fn draw_masked_preview(
    painter: &egui::Painter,
    texture: egui::TextureId,
    img: egui::Rect,
    uv: egui::Rect,
    widget: egui::Rect,
) {
    let cuts = mask_cuts(img, widget);
    let mut mesh = egui::Mesh::with_texture(texture);
    for w in cuts.windows(2) {
        let (y0, y1) = (w[0], w[1]);
        let a0 = mask_alpha_at(widget, y0);
        let a1 = mask_alpha_at(widget, y1);
        let v0 = if img.height() > 0.0 {
            uv.top() + uv.height() * (y0 - img.top()) / img.height()
        } else {
            uv.top()
        };
        let v1 = if img.height() > 0.0 {
            uv.top() + uv.height() * (y1 - img.top()) / img.height()
        } else {
            uv.bottom()
        };
        let c0 = egui::Color32::from_white_alpha((a0.clamp(0.0, 1.0) * 255.0).round() as u8);
        let c1 = egui::Color32::from_white_alpha((a1.clamp(0.0, 1.0) * 255.0).round() as u8);
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(img.left(), y0),
            uv: egui::pos2(uv.left(), v0),
            color: c0,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(img.right(), y0),
            uv: egui::pos2(uv.right(), v0),
            color: c0,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(img.right(), y1),
            uv: egui::pos2(uv.right(), v1),
            color: c1,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(img.left(), y1),
            uv: egui::pos2(uv.left(), v1),
            color: c1,
        });
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 蒙版采样：色标位置上的取值必须与源码一致，且内部线性插值。
    #[test]
    fn mask_alpha_follows_footer_py_stops() {
        let w = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(48.0, 26.0));
        for (pos, a) in BRUSH_INDICATOR_MASK_STOPS {
            let y = w.top() + pos * w.height();
            let got = mask_alpha_at(w, y);
            assert!((got - a).abs() < 1e-3, "位置 {pos} 应为 {a}，实得 {got}");
        }
        // 0.8 → 0.95 的中点应是两端均值（线性插值）
        let mid = w.top() + 0.875 * w.height();
        let got = mask_alpha_at(w, mid);
        assert!((got - 0.75).abs() < 1e-3, "中点应约 0.75，实得 {got}");
    }

    /// 切分点：落在图像范围内的色标位置必须被切开，否则渐变退化成两段线性。
    ///
    /// 注意这里有个反直觉之处：预览按 `max(aw,ah)-2 = 46` 缩放后是 **46px 高**，
    /// 而底栏一行只有 26px —— 图像底部溢出部件，所以 0.8 / 0.95 / 1.0 三个色标
    /// **全都**落在图像范围内，被切成 4 条带。溢出的部分由裁切矩形吃掉。
    #[test]
    fn mask_cuts_include_inner_stops() {
        let w = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(48.0, 26.0));
        let img = egui::Rect::from_min_size(egui::pos2(1.0, 2.0), egui::vec2(46.0, 46.0));
        let cuts = mask_cuts(img, w);
        let expect = [
            img.top(),
            w.top() + 0.8 * w.height(),
            w.top() + 0.95 * w.height(),
            w.bottom(),
            img.bottom(),
        ];
        assert_eq!(cuts.len(), expect.len(), "切分点：{cuts:?}");
        for (got, want) in cuts.iter().zip(expect.iter()) {
            assert!((got - want).abs() < 1e-3, "切分点 {got} 应为 {want}");
        }
    }
}
