//! 主界面视觉语言：ArrowVortex 的布局密度 + MyPaint 源 GTK 配色。
//!
//! 版式参考作者另一套 ArrowVortex 编辑器升级稿的外壳：画布占满视野，菜单栏与
//! 状态栏只有一行，所有分隔都是 1px 直线，没有圆角卡片、投影或大面积
//! 留白。配色改取 MyPaint 参考机（GTK3 / Adwaita Dark）的实测值：
//! 面板 `#353535`、菜单/弹窗 `#2e2e2e`、输入底 `#2d2d2d`、选中 `#15539e`。
//! 这样既保留参考编辑器的紧凑结构，也不会像纯黑皮肤那样压暗画面。

use eframe::egui;

// ---- 尺寸 ----

/// 菜单栏高度。
pub(crate) const MENUBAR_H: f32 = 26.0;
/// 工具条高度。
pub(crate) const TOOLBAR_H: f32 = 40.0;
/// 底部状态栏高度。
pub(crate) const STATUS_H: f32 = 25.0;
/// 侧栏标题行高度。
pub(crate) const PANEL_HEADER_H: f32 = 30.0;
/// 侧栏内容行高度（图层/历史等）。
pub(crate) const ROW_H: f32 = 24.0;
/// 工具条图标按钮的可点击边长。
pub(crate) const TOOL_BTN: f32 = 30.0;
/// 工具条图标的实际绘制边长。
pub(crate) const TOOL_ICON: f32 = 21.0;
/// 侧栏小图标按钮边长。
pub(crate) const SMALL_BTN: f32 = 22.0;

/// 菜单弹层的配置。
///
/// egui 默认给菜单留 5px 内边距、子菜单与父菜单之间再留 2px 间距，落在
/// ArrowVortex 式的窄行界面里就是“悬浮卡片”和错位的子菜单。这里全部归零：
/// 子菜单与父菜单边框贴合，圆角与投影关闭，只保留 1px 分隔线。
pub(crate) fn menu_config() -> egui::menu::MenuConfig {
    egui::menu::MenuConfig::new()
        .submenu_gap(0.0)
        .style(egui::style::StyleModifier::new(
            |style: &mut egui::Style| {
                style.spacing.menu_margin = egui::Margin::ZERO;
                style.spacing.menu_spacing = 0.0;
                style.spacing.menu_width = 252.0;
                style.spacing.button_padding = egui::vec2(7.0, 2.0);
                style.spacing.interact_size.y = 19.0;
                style.visuals.menu_corner_radius = egui::CornerRadius::ZERO;
                style.visuals.window_corner_radius = egui::CornerRadius::ZERO;
                style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
                style.visuals.window_shadow = egui::epaint::Shadow::NONE;
                style.visuals.window_stroke = egui::Stroke::new(1.0, LINE);
                style.visuals.window_fill = MENU;
                style.visuals.panel_fill = PANEL;
                style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                style.visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
                style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                style.visuals.widgets.hovered.weak_bg_fill = HOVER;
                style.visuals.widgets.hovered.bg_fill = HOVER;
                style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
                style.visuals.widgets.active.weak_bg_fill = ACTIVE;
                style.visuals.widgets.active.bg_fill = ACTIVE;
                style.visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
                style.visuals.widgets.open.weak_bg_fill = HOVER;
                style.visuals.widgets.open.bg_fill = HOVER;
                style.visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
                style.visuals.selection.bg_fill = SELECTED;
            },
        ))
}

// ---- 颜色 ----
//
// 取色来自 MyPaint 参考机（GTK3 / Adwaita Dark）的截图采样：
// 面板 bg #353535、菜单与弹窗 #2e2e2e、列表/输入 base #2d2d2d、
// 选中 #15539e。画布外围用同一套灰阶而不是近黑，避免切到亮色画布时
// 出现“纸片漂在黑洞里”的割裂。

/// 画布外围 / 中央面板底色。
pub(crate) const VOID: egui::Color32 = egui::Color32::from_rgb(0x2d, 0x2d, 0x2d);
/// 侧栏、顶栏、底栏底色（Adwaita dark `bg_color`）。
pub(crate) const PANEL: egui::Color32 = egui::Color32::from_rgb(0x35, 0x35, 0x35);
/// 面板标题 / 次级面板底色。
pub(crate) const PANEL_DARK: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x2e, 0x2e);
/// 菜单 / 弹窗底色（GtkMenu）。
pub(crate) const MENU: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x2e, 0x2e);
/// 工具条底色。
pub(crate) const BAR: egui::Color32 = egui::Color32::from_rgb(0x35, 0x35, 0x35);
/// 输入框、内容格底色（Adwaita dark `base_color`）。
pub(crate) const FIELD: egui::Color32 = egui::Color32::from_rgb(0x2d, 0x2d, 0x2d);
/// 按钮 / 网格格子的悬停底色。
pub(crate) const HOVER: egui::Color32 = egui::Color32::from_rgb(0x4a, 0x4a, 0x4a);
/// 按下态底色。
pub(crate) const ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x2b, 0x2b, 0x2b);
/// 主分隔线。
pub(crate) const LINE: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x1b, 0x1b);
/// 更弱的分隔线（面板内部）。
pub(crate) const LINE_SOFT: egui::Color32 = egui::Color32::from_rgb(0x30, 0x30, 0x30);
/// 主要文字（Adwaita dark `theme_fg_color`）。
pub(crate) const TEXT: egui::Color32 = egui::Color32::from_rgb(0xee, 0xee, 0xec);
/// 次要文字。
pub(crate) const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0xbd, 0xbd, 0xb9);
/// 弱化文字 / 禁用态。
pub(crate) const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x8a, 0x86);
/// 选中底（Adwaita dark 旧版 `selected_bg_color`）。
pub(crate) const SELECTED: egui::Color32 = egui::Color32::from_rgb(0x15, 0x53, 0x9e);
/// 强调色（当前工具下划线、当前图层标记）。
pub(crate) const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x35, 0x84, 0xe4);
/// 强调色的暗调。
pub(crate) const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(0x15, 0x53, 0x9e);
/// 信息色（缩放值、模式名）。
pub(crate) const BLUE: egui::Color32 = egui::Color32::from_rgb(0x62, 0xa0, 0xea);
/// 成功提示。
pub(crate) const OK: egui::Color32 = egui::Color32::from_rgb(0x8f, 0xf0, 0xa4);
/// 警告提示。
pub(crate) const WARN: egui::Color32 = egui::Color32::from_rgb(0xf9, 0xc4, 0x40);
/// 错误提示。
pub(crate) const ERROR: egui::Color32 = egui::Color32::from_rgb(0xff, 0x7b, 0x63);

// ---- 主题 ----

/// MyPaint 源 GTK（Adwaita Dark）取向的皮肤。主窗口、原生子窗口与弹层共用。
pub(crate) fn dark_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = MENU;
    v.faint_bg_color = FIELD;
    v.extreme_bg_color = FIELD;
    v.code_bg_color = FIELD;
    v.selection.bg_fill = SELECTED;
    v.selection.stroke = egui::Stroke::new(1.0, TEXT);
    v.hyperlink_color = BLUE;
    v.window_stroke = egui::Stroke::new(1.0, LINE);
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_corner_radius = egui::CornerRadius::ZERO;
    v.menu_corner_radius = egui::CornerRadius::ZERO;
    v.override_text_color = None;

    v.widgets.noninteractive.bg_fill = FIELD;
    v.widgets.noninteractive.weak_bg_fill = FIELD;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = FIELD;
    v.widgets.inactive.weak_bg_fill = FIELD;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT_DIM);
    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
    v.widgets.active.bg_fill = ACTIVE;
    v.widgets.active.weak_bg_fill = ACTIVE;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, TEXT);
    v
}

/// 全应用统一字号。正文 16pt 在 1.25× UI 缩放下约 20 物理像素。
pub(crate) fn text_styles() -> std::collections::BTreeMap<egui::TextStyle, egui::FontId> {
    [
        (egui::TextStyle::Heading, egui::FontId::proportional(19.5)),
        (egui::TextStyle::Body, egui::FontId::proportional(16.0)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(14.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(16.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(13.0)),
    ]
    .into()
}

/// 把主题装进一个 egui Context。重复调用无副作用。
pub(crate) fn install(ctx: &egui::Context) {
    ctx.set_visuals(dark_visuals());
    ctx.global_style_mut(|style| {
        style.text_styles = text_styles();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size = egui::vec2(26.0, 22.0);
        style.spacing.slider_width = 96.0;
        style.spacing.combo_width = 120.0;
        style.spacing.menu_width = 252.0;
        style.spacing.menu_margin = egui::Margin::same(2);
        style.spacing.window_margin = egui::Margin::same(10);
        style.animation_time = 0.08;
    });
}

// ---- Frame ----

pub(crate) fn bar_frame() -> egui::Frame {
    egui::Frame::NONE.fill(BAR)
}

pub(crate) fn panel_frame() -> egui::Frame {
    egui::Frame::NONE.fill(PANEL)
}

pub(crate) fn modal_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0, LINE))
        .inner_margin(egui::Margin::same(16))
}

// ---- 小控件 ----

/// 1px 水平线。
pub(crate) fn hline(ui: &mut egui::Ui, color: egui::Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color);
}

/// 1px 垂直线，返回是否真的绘制（父布局里用作组间分隔）。
pub(crate) fn vline(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, LINE);
}

/// 工具条上的纯图标按钮。
///
/// `selected` 用一级明度差 + 底部 2px 强调线表达，而不是 egui 默认的圆角
/// 高亮块；`enabled=false` 时图标降到 35% 亮度且不响应点击。
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    fallback: &str,
    selected: bool,
    enabled: bool,
    tip: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(TOOL_BTN, TOOL_BTN),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hovered = enabled && resp.hovered();
    if selected {
        ui.painter().rect_filled(rect, 0.0, HOVER);
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() + 3.0, rect.bottom() - 2.0),
                egui::pos2(rect.right() - 3.0, rect.bottom()),
            ),
            0.0,
            ACCENT,
        );
    } else if hovered {
        ui.painter().rect_filled(rect, 0.0, HOVER);
    }
    let tint = if !enabled {
        TEXT_FAINT.gamma_multiply(0.55)
    } else if selected || hovered {
        TEXT
    } else {
        TEXT_DIM
    };
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(TOOL_ICON, TOOL_ICON));
    match texture {
        Some(tex) => {
            ui.painter().image(
                tex.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        }
        None => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                fallback,
                egui::FontId::proportional(12.5),
                tint,
            );
        }
    }
    if enabled && !tip.is_empty() {
        resp.clone().on_hover_text(tip);
    }
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 面板标题栏里的 21px 小图标按钮。
pub(crate) fn small_icon_button(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    fallback: &str,
    enabled: bool,
    tip: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(SMALL_BTN, SMALL_BTN),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hovered = enabled && resp.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 0.0, HOVER);
    }
    let tint = if !enabled {
        TEXT_FAINT.gamma_multiply(0.55)
    } else if hovered {
        TEXT
    } else {
        TEXT_DIM
    };
    let icon_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 4.0, rect.top() + 4.0),
        egui::pos2(rect.right() - 4.0, rect.bottom() - 4.0),
    );
    match texture {
        Some(tex) => {
            ui.painter().image(
                tex.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        }
        None => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                fallback,
                egui::FontId::proportional(11.0),
                tint,
            );
        }
    }
    if enabled && !tip.is_empty() {
        resp.clone().on_hover_text(tip);
    }
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 面板标题行：`[icon] 标题            [右侧控件]`。
pub(crate) fn panel_header(
    ui: &mut egui::Ui,
    title: &str,
    texture: Option<&egui::TextureHandle>,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PANEL_HEADER_H),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, PANEL_DARK);
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        egui::vec2(15.0, 15.0),
    );
    let text_x = if let Some(tex) = texture {
        ui.painter().image(
            tex.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            TEXT_DIM,
        );
        rect.left() + 29.0
    } else {
        rect.left() + 11.0
    };
    ui.painter().text(
        egui::pos2(text_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(15.5),
        TEXT,
    );

    // 面板头的关闭按钮：与参考 MyPaint 的 tool widget header 一致。
    // 之前的标题行只有文字，没有可点的关闭入口。
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 13.0, rect.center().y),
        egui::vec2(20.0, PANEL_HEADER_H.min(20.0)),
    );
    let close = ui
        .interact(
            close_rect,
            egui::Id::new(("panel-close", title)),
            egui::Sense::click(),
        )
        .on_hover_text("关闭");
    if close.hovered() {
        ui.painter().rect_filled(close_rect, 0.0, HOVER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let x_color = if close.hovered() { TEXT } else { TEXT_DIM };
    let x_stroke = egui::Stroke::new(1.4, x_color);
    let c = close_rect.center();
    ui.painter().line_segment(
        [c + egui::vec2(-4.0, -4.0), c + egui::vec2(4.0, 4.0)],
        x_stroke,
    );
    ui.painter().line_segment(
        [c + egui::vec2(4.0, -4.0), c + egui::vec2(-4.0, 4.0)],
        x_stroke,
    );

    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 1.0),
            rect.right_bottom(),
        ),
        0.0,
        LINE,
    );
    // 标题之后把剩余空间留给调用方；这里只把光标移到标题行下方。
    ui.add_space(0.0);
    close.clicked()
}

/// 自定义平直滑条：4px 轨道、2px 已填充段、3×11px 方块手柄。
///
/// 返回被拖动后的值。ArrowVortex 没有圆形手柄，这里也保持方角。
pub(crate) fn flat_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    width: f32,
    accent: egui::Color32,
) -> egui::Response {
    let (rect, mut resp) =
        ui.allocate_exact_size(egui::vec2(width, 16.0), egui::Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let clamped = value.clamp(lo.min(hi), hi.max(lo));
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
            *value = lo + (hi - lo) * t;
            resp.mark_changed();
        }
    }
    let v = value.clamp(lo.min(hi), hi.max(lo));
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 1.0, rect.center().y - 2.0),
        egui::pos2(rect.right() - 1.0, rect.center().y + 2.0),
    );
    ui.painter().rect_filled(track, 0.0, FIELD);
    let fill_right = track.left() + track.width() * t;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(track.left_top(), egui::pos2(fill_right, track.bottom())),
        0.0,
        accent,
    );
    let handle = egui::Rect::from_center_size(
        egui::pos2(fill_right, rect.center().y),
        egui::vec2(3.0, 11.0),
    );
    ui.painter().rect_filled(
        handle,
        0.0,
        if resp.hovered() || resp.dragged() {
            TEXT
        } else {
            TEXT_DIM
        },
    );
    // 滑条不设左右双箭头：GTK 的 GtkScale 用的是普通指针，双箭头只在
    // 面板分隔条上有意义。这里保留默认光标，避免悬停笔刷大小/不透明度时
    // 出现误导性的 ↔。
    let _ = clamped;
    resp
}

/// 紧凑复选行（用于弹层与偏好页）。
pub(crate) fn check_row(ui: &mut egui::Ui, label: &str, checked: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, HOVER);
    }
    let box_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 14.0, rect.center().y),
        egui::vec2(12.0, 12.0),
    );
    ui.painter()
        .rect_filled(box_rect, 0.0, if *checked { ACCENT } else { FIELD });
    ui.painter().rect_stroke(
        box_rect,
        0.0,
        egui::Stroke::new(1.0, LINE),
        egui::StrokeKind::Inside,
    );
    if *checked {
        ui.painter().line_segment(
            [
                egui::pos2(box_rect.left() + 2.5, box_rect.center().y),
                egui::pos2(box_rect.center().x, box_rect.bottom() - 3.0),
            ],
            egui::Stroke::new(1.4, egui::Color32::WHITE),
        );
        ui.painter().line_segment(
            [
                egui::pos2(box_rect.center().x, box_rect.bottom() - 3.0),
                egui::pos2(box_rect.right() - 2.0, box_rect.top() + 3.0),
            ],
            egui::Stroke::new(1.4, egui::Color32::WHITE),
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + 28.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(15.5),
        TEXT_DIM,
    );
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 色彩方块按钮（无圆角，选中时 1px 白 + 1px 强调色）。
pub(crate) fn swatch(
    ui: &mut egui::Ui,
    color: egui::Color32,
    size: f32,
    selected: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_rgb(0x0a, 0x0b, 0x0c));
    let inner = rect.shrink(1.0);
    ui.painter().rect_filled(inner, 0.0, color);
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, if selected { ACCENT } else { LINE }),
        egui::StrokeKind::Inside,
    );
    resp
}

/// 居中状态栏文字片段：`标签` 暗、`值` 亮。
pub(crate) struct StatusField {
    pub label: &'static str,
    pub value: String,
    pub color: egui::Color32,
}

/// 画一整条状态栏，字段在视觉上严格居中（参考 ArrowVortex 的底栏）。
pub(crate) fn status_bar(
    ui: &mut egui::Ui,
    left: Option<(&str, egui::Color32)>,
    fields: &[StatusField],
    right: &str,
) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, BAR);
    ui.painter().rect_filled(
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + 1.0)),
        0.0,
        LINE,
    );
    let font = egui::FontId::proportional(13.0);
    let dim = TEXT_FAINT;
    let sep = "  ·  ";
    let mut pieces: Vec<(String, egui::Color32)> = Vec::new();
    for f in fields {
        pieces.push((format!("{} ", f.label), dim));
        pieces.push((f.value.clone(), f.color));
        pieces.push((sep.to_string(), LINE.gamma_multiply(1.6)));
    }
    if let Some((_, _)) = pieces.last() {
        // 去掉最后一个分隔符
        pieces.pop();
    }
    let mut widths = Vec::new();
    for (text, _) in &pieces {
        widths.push(
            ui.painter()
                .layout_no_wrap(text.clone(), font.clone(), TEXT)
                .size()
                .x,
        );
    }
    let total: f32 = widths.iter().sum::<f32>();
    let mut x = (rect.center().x - total * 0.5).max(rect.left() + 150.0);
    for ((text, color), w) in pieces.iter().zip(widths) {
        ui.painter().text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font.clone(),
            *color,
        );
        x += w;
    }
    if let Some((text, color)) = left {
        ui.painter().text(
            egui::pos2(rect.left() + 9.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font.clone(),
            color,
        );
    }
    ui.painter().text(
        egui::pos2(rect.right() - 9.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        right,
        font,
        TEXT_FAINT,
    );
}

/// 给标题/状态文字加一点字距的辅助：egui 无 letter-spacing，用小号字 +
/// 手动空格在中文界面里会很怪，因此保留原样，仅作为语义入口。
pub(crate) fn faint(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(11.5)
        .color(TEXT_FAINT)
}

pub(crate) fn dim(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(12.0).color(TEXT_DIM)
}

pub(crate) fn strong(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(12.0).color(TEXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_matches_source_gtk_adwaita_dark() {
        // 参考机截图采样值：面板/菜单/输入/选中。
        assert_eq!(PANEL, egui::Color32::from_rgb(0x35, 0x35, 0x35));
        assert_eq!(MENU, egui::Color32::from_rgb(0x2e, 0x2e, 0x2e));
        assert_eq!(FIELD, egui::Color32::from_rgb(0x2d, 0x2d, 0x2d));
        assert_eq!(SELECTED, egui::Color32::from_rgb(0x15, 0x53, 0x9e));
        assert_eq!(TEXT, egui::Color32::from_rgb(0xee, 0xee, 0xec));
        // 不再是第一阶段那种近黑底（#0f/#1a）。
        assert!(PANEL.r() > 0x30);
        assert!(FIELD.r() > 0x20);
    }

    #[test]
    fn compact_type_scale_stays_readable() {
        let t = text_styles();
        assert!(t[&egui::TextStyle::Body].size >= 15.0);
        assert!(t[&egui::TextStyle::Small].size >= 12.0);
        assert!(TOOL_ICON >= 20.0);
    }
}
