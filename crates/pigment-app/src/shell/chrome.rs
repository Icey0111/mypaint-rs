//! 主界面 chrome —— 菜单、工具条、左右停靠侧栏、状态栏。
//!
//! 结构对齐 MyPaint 的工作区语义（菜单/工具条/停靠面板/画布/状态栏），
//! 但尺寸和交互按 egui 重排：
//!
//! ```text
//! ┌ 菜单栏（9 棵 menu.xml 菜单） ─────────────────────────────┐
//! ├ 工具条（文件/编辑/工具模式/笔刷/颜色/视图/面板） ────────┤
//! │ 笔刷侧栏        │      画布（唯一 expand）    │ 颜色 + 图层 │
//! └ 状态栏（模式 / 图层 / 笔刷 / 颜色 / 缩放） ──────────────┘
//! ```
//!
//! 所有面板都不是浮动卡片：左右侧栏是 `egui::Panel`，面板内部只有 1px
//! 分隔线。布局密度参考作者另一套 ArrowVortex 编辑器升级稿；颜色取自
//! MyPaint 源 GTK / Adwaita Dark（见 [`super::theme`]），避免 egui 默认
//! 圆角/阴影和纯黑底把绘画界面压暗。

use eframe::egui;
use pigment_engine::color::UIColor;
use pigment_engine::hokusai::BrushSetting;
use pigment_surface::combine::CombineMode;
use pigment_surface::layer::LayerMode;

use crate::app::PigmentApp;
use crate::color;
use crate::shell::theme;
use crate::shell::toolbars::{self, IconCache};

/// 画笔图标的纹理缓存键。
const ICON_BRUSHES: &str = "mypaint-brushes-symbolic";
const ICON_COLORS: &str = "mypaint-colors-symbolic";
const ICON_LAYERS: &str = "mypaint-layers-symbolic";
const ICON_EYE_OPEN: &str = "mypaint-object-visible-symbolic";
const ICON_EYE_CLOSED: &str = "mypaint-object-hidden-symbolic";
const ICON_LOCKED: &str = "mypaint-object-locked-symbolic";
const ICON_UNLOCKED: &str = "mypaint-object-unlocked-symbolic";
const ICON_GROUP_OPEN: &str = "mypaint-expander-expanded-symbolic";
const ICON_GROUP_CLOSED: &str = "mypaint-expander-collapsed-symbolic";
const ICON_LAYER: &str = "mypaint-layer-painting-symbolic";
const ICON_LAYER_GROUP: &str = "mypaint-layer-group-symbolic";
const ICON_LAYER_VECTOR: &str = "mypaint-layer-vector-symbolic";

/// 主界面尺寸与配色之外的状态。
///
/// 只放“界面自己”的状态，文档/笔刷/颜色仍归 `PigmentApp` 的原字段。
pub(crate) struct ChromeState {
    pub(crate) left_open: bool,
    pub(crate) color_open: bool,
    pub(crate) layers_open: bool,
    pub(crate) left_width: f32,
    pub(crate) right_width: f32,
    pub(crate) color_height: f32,
    pub(crate) icons: IconCache,
    /// 当前 HSV 方块纹理（随色相变化重建）。
    pub(crate) sv_texture: Option<egui::TextureHandle>,
    pub(crate) sv_hue_deg: i32,
    /// 竖直色相条纹理（只建一次）。
    pub(crate) hue_texture: Option<egui::TextureHandle>,
    /// 偏好窗口（`PreferencesWindow`）与当前页。
    pub(crate) prefs_open: bool,
    pub(crate) prefs_tab: PrefsTab,
    /// 笔刷侧栏使用“最近笔刷/全部笔刷”哪个模式。
    pub(crate) brush_history_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrefsTab {
    General,
    Interface,
    Files,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            left_open: true,
            color_open: true,
            layers_open: true,
            left_width: 238.0,
            right_width: 270.0,
            color_height: 300.0,
            icons: IconCache::default(),
            sv_texture: None,
            sv_hue_deg: -1,
            hue_texture: None,
            prefs_open: false,
            prefs_tab: PrefsTab::General,
            brush_history_only: false,
        }
    }
}

impl ChromeState {
    pub(crate) fn from_prefs(prefs: &pigment_engine::prefs::Prefs) -> Self {
        let mut s = Self::default();
        s.left_open = prefs.bool_or("ui.chrome.left_open", true);
        s.color_open = prefs.bool_or("ui.chrome.color_open", true);
        s.layers_open = prefs.bool_or("ui.chrome.layers_open", true);
        s.left_width = prefs
            .f64_or("ui.chrome.left_width", s.left_width as f64)
            .clamp(220.0, 340.0) as f32;
        s.right_width = prefs
            .f64_or("ui.chrome.right_width", s.right_width as f64)
            .clamp(250.0, 360.0) as f32;
        s.color_height = prefs
            .f64_or("ui.chrome.color_height", s.color_height as f64)
            .clamp(230.0, 430.0) as f32;
        s
    }

    pub(crate) fn store(&self, prefs: &mut pigment_engine::prefs::Prefs) {
        prefs.set_bool("ui.chrome.left_open", self.left_open);
        prefs.set_bool("ui.chrome.color_open", self.color_open);
        prefs.set_bool("ui.chrome.layers_open", self.layers_open);
        prefs.set_f64("ui.chrome.left_width", self.left_width as f64);
        prefs.set_f64("ui.chrome.right_width", self.right_width as f64);
        prefs.set_f64("ui.chrome.color_height", self.color_height as f64);
    }
}

// ---- 通用绘制 ----

fn hsv_color32(hsv: [f32; 3], alpha: u8) -> egui::Color32 {
    let [r, g, b] = color::hsv_to_rgb(hsv);
    egui::Color32::from_rgba_unmultiplied(
        (r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (b.clamp(0.0, 1.0) * 255.0).round() as u8,
        alpha,
    )
}

fn icon_at(
    painter: &egui::Painter,
    cache: &mut IconCache,
    ctx: &egui::Context,
    name: &str,
    rect: egui::Rect,
    tint: egui::Color32,
) {
    if let Some(tex) = toolbars::icon_texture(ctx, cache, name) {
        painter.image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
    }
}

fn icon_texture(
    ctx: &egui::Context,
    cache: &mut IconCache,
    name: &str,
) -> Option<egui::TextureHandle> {
    if name.is_empty() {
        return None;
    }
    toolbars::icon_texture(ctx, cache, name)
}

/// 画布在“无 tile 覆盖区域”的底色。
///
/// MyPaint 源码的 `CanvasRenderer._draw_cb` 在渲染 tile 之前先把**整个
/// allocation** 铺满：背景可见时是背景层（图案或纯色），背景隐藏时是透明
/// 棋盘格。egui 这边 tile 是按需上传的，远景缩放下不可能为视口内每一块
/// 世界 tile 都建纹理，因此中央面板必须先用文档背景色兜底，否则会露出
/// `VOID` 深灰，表现为“无限画布变成一小块纸”。
///
/// 有图案背景时退化为它的平均色（`background_color` 就是均值）；正常缩放下
/// 图案 tile 会盖在上面，远景下平坦底色也不会形成黑洞。
pub(crate) fn canvas_backdrop_rgb(layers: &pigment_surface::LayerStack) -> [u8; 3] {
    // 闪烁预览反转背景可见性（`tree.py:299-300`）；纯色背景没有 tile 覆盖时
    // 靠这里兜底，否则预览期间空区域会露出 VOID。
    let background_shown = if layers.preview_path().is_some() {
        !layers.background_visible()
    } else {
        layers.background_visible()
    };
    if background_shown {
        layers.background_color()
    } else {
        // 隐藏背景时源码铺透明棋盘格；tile 层自身会在有内容处画棋盘格，
        // 这里用深灰兜底，避免把“透明”误显示成纸色。
        [theme::VOID.r(), theme::VOID.g(), theme::VOID.b()]
    }
}

fn canvas_backdrop(layers: &pigment_surface::LayerStack) -> egui::Color32 {
    let [r, g, b] = canvas_backdrop_rgb(layers);
    egui::Color32::from_rgb(r, g, b)
}

impl PigmentApp {
    // =====================================================================
    // 根布局
    // =====================================================================

    /// 新 chrome 的绘制入口。画布仍是 `PigmentApp::canvas`，它的 GPU 路径
    /// 不变；这里只重建它周围的窗口骨架。
    pub(crate) fn chrome_ui_root(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // 全屏自动隐藏：与参考的“画布优先”一致，鼠标不在屏幕顶边时整条
        // 顶栏收起；状态栏保留，退出全屏/恢复面板仍可从菜单或快捷键完成。
        let reveal_top = !(self.fullscreen
            && self.autohide
            && ctx.input(|i| i.pointer.hover_pos().map(|p| p.y > 8.0).unwrap_or(true)));
        if reveal_top {
            self.chrome_topbar(ui);
        }
        self.chrome_statusbar(ui);
        self.chrome_left_dock(ui);
        self.chrome_right_dock(ui);

        // Plan A 的画布 pass 在 egui 之前直接写目标：中央面板必须透明，
        // 否则会把刚画好的画布盖掉。eframe 回退仍由这里铺底色。
        let direct = self.direct_canvas;
        let backdrop = canvas_backdrop(self.doc.layers());
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(if direct {
                egui::Color32::TRANSPARENT
            } else {
                backdrop
            }))
            .show(ui, |ui| {
                if !direct {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, backdrop);
                }
                self.canvas(ui);
            });

        self.chrome_dialogs(&ctx);
    }

    fn chrome_topbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("chrome_topbar")
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(theme::BAR))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                self.menubar(ui);
                self.chrome_toolbar(ui);
            });
    }

    fn chrome_statusbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("chrome_statusbar")
            .resizable(false)
            .show_separator_line(false)
            .exact_size(theme::STATUS_H)
            .frame(egui::Frame::NONE.fill(theme::BAR))
            .show(ui, |ui| {
                let now = ui.input(|i| i.time);
                let transient = self
                    .status
                    .as_ref()
                    .filter(|(_, until)| now <= *until)
                    .map(|(msg, _)| msg.as_str());
                let layer = self.doc.layers().current();
                let layer_name =
                    layer.display_name(self.doc.current_path().last().copied().unwrap_or(0));
                let mode = self
                    .modes
                    .top()
                    .action_name()
                    .and_then(crate::shell::actions::by_id)
                    .and_then(|a| a.effective_label())
                    .map(crate::i18n::tr)
                    .unwrap_or_else(|| self.modes.top().name());
                let hex = self.color_manager.get_color().to_hex_str();
                let zoom = self.view.zoom;
                let frame = self.doc.get_frame();
                let frame_text = if self.doc.get_frame_enabled() && frame.2 > 0 && frame.3 > 0 {
                    format!("{}×{}", frame.2, frame.3)
                } else {
                    "无限画布".to_string()
                };
                let fields = vec![
                    theme::StatusField {
                        label: "图层",
                        value: layer_name,
                        color: theme::TEXT,
                    },
                    theme::StatusField {
                        label: "工具",
                        value: mode.to_string(),
                        color: theme::BLUE,
                    },
                    theme::StatusField {
                        label: "笔刷",
                        value: self.brush_name.clone(),
                        color: theme::TEXT_DIM,
                    },
                    theme::StatusField {
                        label: "颜色",
                        value: hex,
                        color: hsv_color32(self.color_hsv(), 255),
                    },
                    theme::StatusField {
                        label: "缩放",
                        value: format!("{:.0}%", zoom * 100.0),
                        color: theme::TEXT,
                    },
                    theme::StatusField {
                        label: "画布",
                        value: frame_text,
                        color: theme::TEXT_DIM,
                    },
                ];
                let left = transient.map(|m| (m, theme::BLUE));
                let right = if self.doc.unsaved_painting_time > 0.0 {
                    format!("● 未保存  ·  v{}", env!("CARGO_PKG_VERSION"))
                } else {
                    format!("v{}", env!("CARGO_PKG_VERSION"))
                };
                theme::status_bar(ui, left, &fields, &right);
            });
    }

    // =====================================================================
    // 工具条
    // =====================================================================

    fn chrome_toolbar(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::TOOLBAR_H),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(rect, 0.0, theme::BAR);
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                rect.right_bottom(),
            ),
            0.0,
            theme::LINE,
        );
        let mut tb = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(6.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        tb.spacing_mut().item_spacing = egui::vec2(1.0, 0.0);
        let compact = rect.width() < 1030.0;
        let ctx = ui.ctx().clone();

        // 文件与撤销：图标组之间用宽松一点的留白隔开，不画可见竖线
        // （MyPaint 工具条的 separator 也是 set_draw(False)）。
        let file = ["New", "Open", "Save"];
        for id in file {
            let enabled = self.action_available(id);
            let selected = self.action_checked(id).unwrap_or(false);
            if toolbars::action_button(&mut tb, &mut self.chrome.icons, id, selected, enabled)
                .clicked()
            {
                let now = tb.input(|i| i.time);
                self.run_action(id, &ctx, now);
            }
        }
        tb.add_space(7.0);
        theme::vline(&mut tb, 19.0);
        tb.add_space(7.0);
        for id in ["Undo", "Redo"] {
            let enabled = self.action_available(id);
            if toolbars::action_button(&mut tb, &mut self.chrome.icons, id, false, enabled)
                .clicked()
            {
                let now = tb.input(|i| i.time);
                self.run_action(id, &ctx, now);
            }
        }
        tb.add_space(7.0);
        theme::vline(&mut tb, 19.0);
        tb.add_space(7.0);

        // 工具模式。Nine buttons is dense by design; the reference editor
        // spends no vertical space on a second toolbar.
        let modes = [
            "FreehandMode",
            "InkingMode",
            "StraightMode",
            "SequenceMode",
            "EllipseMode",
            "FloodFillMode",
            "LayerMoveMode",
            "FrameEditMode",
            "SymmetryEditMode",
        ];
        for id in modes {
            let enabled = self.action_available(id);
            let selected = self.action_checked(id).unwrap_or(false);
            if toolbars::action_button(&mut tb, &mut self.chrome.icons, id, selected, enabled)
                .clicked()
            {
                let now = tb.input(|i| i.time);
                self.run_action(id, &ctx, now);
            }
        }

        tb.add_space(7.0);
        theme::vline(&mut tb, 19.0);
        tb.add_space(7.0);

        // 笔刷尺寸 / 不透明度。值以“有效笔刷”为准；拖动后写回
        // radius_override / opaque，与 MyPaint 的 brush adjustment 同义。
        let mut log_radius = self
            .brush
            .get(BrushSetting::Radius)
            .base_value
            .clamp(-2.0, 6.0);
        let mut opaque = self
            .base_brush
            .get(BrushSetting::Opaque)
            .base_value
            .clamp(0.0, 1.0);
        if !compact {
            tb.label(
                egui::RichText::new("笔刷")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
            let px = log_radius.exp();
            tb.label(
                egui::RichText::new(format!("{px:>4.0}px"))
                    .size(12.5)
                    .color(theme::TEXT_DIM),
            );
            let resp =
                theme::flat_slider(&mut tb, &mut log_radius, -2.0..=6.0, 70.0, theme::TEXT_DIM);
            if resp.changed() {
                self.radius_override = Some(log_radius);
                self.derive_active();
            }
            tb.add_space(5.0);
            tb.label(
                egui::RichText::new("不透明")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
            tb.label(
                egui::RichText::new(format!("{:>3.0}%", opaque * 100.0))
                    .size(11.5)
                    .color(theme::TEXT_DIM),
            );
            let resp = theme::flat_slider(&mut tb, &mut opaque, 0.0..=1.0, 54.0, theme::TEXT_DIM);
            if resp.changed() {
                self.base_brush.get_mut(BrushSetting::Opaque).base_value = opaque;
                self.derive_active();
            }
            tb.add_space(4.0);
        }

        // 当前色块 + 笔刷预览（点色块开颜色面板，点预览开笔刷面板）。
        let hsv = self.color_hsv();
        let swatch = theme::swatch(&mut tb, hsv_color32(hsv, 255), 22.0, false);
        if swatch.clicked() {
            self.chrome.color_open = true;
        }
        swatch.on_hover_text(format!(
            "当前颜色 {}\n点击打开颜色面板",
            self.color_manager.get_color().to_hex_str()
        ));
        let sel = self.brushes.selected;
        let tex = self.brushes.icon(&ctx, sel);
        let (r, resp) = tb.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
        tb.painter().image(
            tex.id(),
            r.shrink(2.0),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        if resp.clicked() {
            self.chrome.left_open = true;
        }
        resp.on_hover_text(format!("当前笔刷：{}\n点击打开笔刷面板", self.brush_name));

        // 右侧：缩放、适配、面板开关、全屏。右对齐布局从右往左排。
        tb.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let enabled = self.action_available("Fullscreen");
            if toolbars::action_button(
                ui,
                &mut self.chrome.icons,
                "Fullscreen",
                self.fullscreen || self.window_fullscreen,
                enabled,
            )
            .clicked()
            {
                let now = ui.input(|i| i.time);
                self.run_action("Fullscreen", &ctx, now);
            }
            ui.add_space(4.0);
            for id in ["BrushGroups", "ColorAdjusters", "ToggleLayersTool"] {
                let selected = self.action_checked(id).unwrap_or(false);
                let enabled = self.action_available(id);
                if toolbars::action_button(ui, &mut self.chrome.icons, id, selected, enabled)
                    .clicked()
                {
                    let now = ui.input(|i| i.time);
                    self.run_action(id, &ctx, now);
                }
            }
            ui.add_space(5.0);
            theme::vline(ui, 19.0);
            ui.add_space(5.0);
            if toolbars::action_button(
                ui,
                &mut self.chrome.icons,
                "FitView",
                self.saved_view.is_some(),
                true,
            )
            .clicked()
            {
                let now = ui.input(|i| i.time);
                self.run_action("FitView", &ctx, now);
            }
            if toolbars::action_button(ui, &mut self.chrome.icons, "ZoomInCentered", false, true)
                .clicked()
            {
                let now = ui.input(|i| i.time);
                self.run_action("ZoomInCentered", &ctx, now);
            }
            ui.label(
                egui::RichText::new(format!("{:.0}%", self.view.zoom * 100.0))
                    .size(11.5)
                    .monospace()
                    .color(theme::TEXT_DIM),
            );
            if toolbars::action_button(ui, &mut self.chrome.icons, "ZoomOutCentered", false, true)
                .clicked()
            {
                let now = ui.input(|i| i.time);
                self.run_action("ZoomOutCentered", &ctx, now);
            }
        });
    }

    // =====================================================================
    // 左右停靠
    // =====================================================================

    fn chrome_left_dock(&mut self, ui: &mut egui::Ui) {
        if !self.chrome.left_open {
            return;
        }
        let want = self.chrome.left_width;
        let resp = egui::Panel::left("chrome_left_dock")
            .resizable(true)
            .default_size(want)
            .size_range(220.0..=340.0)
            .frame(theme::panel_frame())
            .show(ui, |ui| {
                ui.set_min_width(220.0);
                self.brush_panel(ui);
            });
        let actual = resp.response.rect.width();
        if (actual - self.chrome.left_width).abs() > 0.5 {
            self.chrome.left_width = actual;
        }
    }

    fn chrome_right_dock(&mut self, ui: &mut egui::Ui) {
        if !self.chrome.color_open && !self.chrome.layers_open {
            return;
        }
        let want = self.chrome.right_width;
        let resp = egui::Panel::right("chrome_right_dock")
            .resizable(true)
            .default_size(want)
            .size_range(250.0..=360.0)
            .frame(theme::panel_frame())
            .show(ui, |ui| {
                ui.set_min_width(250.0);
                let total_h = ui.available_height();
                let both = self.chrome.color_open && self.chrome.layers_open;
                if self.chrome.color_open {
                    let h = if both {
                        self.chrome
                            .color_height
                            .clamp(205.0, (total_h - 140.0).max(205.0))
                    } else {
                        total_h
                    };
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_size(egui::vec2(ui.available_width(), h));
                            self.color_panel(ui, h);
                        },
                    );
                }
                if both {
                    let (handle, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 5.0),
                        egui::Sense::drag(),
                    );
                    ui.painter().rect_filled(handle, 0.0, theme::PANEL_DARK);
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(
                            handle.center(),
                            egui::vec2(
                                if resp.hovered() || resp.dragged() {
                                    42.0
                                } else {
                                    20.0
                                },
                                1.0,
                            ),
                        ),
                        0.0,
                        if resp.hovered() || resp.dragged() {
                            theme::ACCENT
                        } else {
                            theme::LINE
                        },
                    );
                    if resp.dragged() {
                        self.chrome.color_height =
                            (self.chrome.color_height + resp.drag_delta().y).clamp(205.0, 430.0);
                    }
                    if resp.hovered() || resp.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                    }
                }
                if self.chrome.layers_open {
                    self.layers_panel(ui);
                }
            });
        let actual = resp.response.rect.width();
        if (actual - self.chrome.right_width).abs() > 0.5 {
            self.chrome.right_width = actual;
        }
    }

    // =====================================================================
    // 笔刷面板
    // =====================================================================

    fn brush_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let icon = icon_texture(&ctx, &mut self.chrome.icons, ICON_BRUSHES);
        if theme::panel_header(ui, "笔刷", icon.as_ref()) {
            self.chrome.left_open = false;
            return;
        }

        // 搜索 + 扫描按钮
        let mut search_changed = false;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let w = (ui.available_width() - 34.0).max(80.0);
            let frame = egui::Frame::NONE
                .fill(theme::FIELD)
                .stroke(egui::Stroke::new(1.0, theme::LINE_SOFT))
                .inner_margin(egui::Margin::symmetric(5, 2));
            frame.show(ui, |ui| {
                let resp = ui.add_sized(
                    [w, 17.0],
                    egui::TextEdit::singleline(&mut self.brushes.search)
                        .frame(egui::Frame::NONE)
                        .hint_text(egui::RichText::new("搜索笔刷…").color(theme::TEXT_FAINT))
                        .text_color(theme::TEXT),
                );
                search_changed = resp.changed();
            });
            if ui
                .add(egui::Button::new("↻").frame(false).small())
                .on_hover_text("重新扫描用户笔刷目录")
                .clicked()
            {
                self.brushes.rescan();
            }
        });
        if search_changed {
            self.brushes.rebuild_visible();
        }

        // 分组选择
        let groups = self.brushes.library.groups.clone();
        let current = groups
            .iter()
            .find(|g| g.id == self.brushes.group)
            .map(|g| {
                format!(
                    "{}  ({})",
                    pigment_engine::brushlib::translate_group_name(&g.display),
                    g.members.len()
                )
            })
            .unwrap_or_else(|| self.brushes.group.clone());
        let mut picked_group = None;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            egui::ComboBox::from_id_salt("chrome_brush_group")
                .width(ui.available_width() - 8.0)
                .selected_text(
                    egui::RichText::new(current)
                        .size(12.5)
                        .color(theme::TEXT_DIM),
                )
                .show_ui(ui, |ui| {
                    for g in &groups {
                        let label = format!(
                            "{}  ({})",
                            pigment_engine::brushlib::translate_group_name(&g.display),
                            g.members.len()
                        );
                        if ui
                            .selectable_label(g.id == self.brushes.group, label)
                            .clicked()
                        {
                            picked_group = Some(g.id.clone());
                            ui.close();
                        }
                    }
                });
        });
        if let Some(group) = picked_group {
            self.brushes.group = group;
            self.brushes.search.clear();
            self.brushes.rebuild_visible();
            self.reload_selected_brush();
        }

        // 笔刷历史固定底栏
        let history_h = 52.0;
        let grid_h = (ui.available_height() - history_h).max(60.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), grid_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                self.brush_grid(
                    ui,
                    if self.chrome.brush_history_only {
                        None
                    } else {
                        Some(grid_h)
                    },
                );
            },
        );
        theme::hline(ui, theme::LINE);
        self.brush_history_strip(ui);
    }

    fn brush_grid(&mut self, ui: &mut egui::Ui, _reserved: Option<f32>) {
        let ctx = ui.ctx().clone();
        let indices = self.brushes.visible_indices().to_vec();
        if indices.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("没有匹配的笔刷").color(theme::TEXT_FAINT));
            });
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("chrome_brush_grid")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(4.0);
                    for idx in indices {
                        let tex = self.brushes.icon(&ctx, idx);
                        let (name, group, key) = self
                            .brushes
                            .library
                            .entries
                            .get(idx)
                            .map(|e| (e.mypaint_name(), e.group_display.clone(), e.key.clone()))
                            .unwrap_or_default();
                        let selected = idx == self.brushes.selected;
                        let (cell, resp) =
                            ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::click());
                        let hovered = resp.hovered();
                        ui.painter().rect_filled(
                            cell,
                            0.0,
                            if selected {
                                theme::HOVER
                            } else if hovered {
                                theme::HOVER
                            } else {
                                theme::PANEL_DARK
                            },
                        );
                        ui.painter().rect_filled(
                            cell.shrink(1.0),
                            0.0,
                            if selected { theme::HOVER } else { theme::FIELD },
                        );
                        ui.painter().image(
                            tex.id(),
                            cell.shrink(4.0),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                        ui.painter().rect_stroke(
                            cell,
                            0.0,
                            egui::Stroke::new(
                                if selected { 2.0 } else { 1.0 },
                                if selected {
                                    theme::ACCENT
                                } else {
                                    theme::LINE_SOFT
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        let tip = format!("{name}\n{group}\n{key}");
                        resp.clone().on_hover_text(tip);
                        if resp.clicked() {
                            self.brushes.selected = idx;
                            self.reload_selected_brush();
                            self.chrome.brush_history_only = false;
                        }
                    }
                });
                ui.add_space(4.0);
            });
    }

    fn brush_history_strip(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left(), ui.cursor().top()),
                egui::vec2(ui.available_width(), 51.0),
            ),
            0.0,
            theme::PANEL_DARK,
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("最近")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
            ui.add_space(2.0);
            let count = self.brushes.history.len();
            for i in 0..count {
                let Some(entry) = self.brushes.history_entry(i).cloned() else {
                    continue;
                };
                let tex = self.brushes.icon(&ctx, entry.preview);
                let (cell, resp) =
                    ui.allocate_exact_size(egui::vec2(31.0, 31.0), egui::Sense::click());
                ui.painter().rect_filled(
                    cell,
                    0.0,
                    if resp.hovered() {
                        theme::HOVER
                    } else {
                        theme::FIELD
                    },
                );
                ui.painter().image(
                    tex.id(),
                    cell.shrink(2.0),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    cell,
                    0.0,
                    egui::Stroke::new(1.0, theme::LINE_SOFT),
                    egui::StrokeKind::Inside,
                );
                resp.clone()
                    .on_hover_text(format!("恢复最近笔刷：{}", entry.name));
                if resp.clicked() {
                    self.select_brush_history(i);
                }
            }
        });
    }

    // =====================================================================
    // 颜色面板
    // =====================================================================

    fn color_panel(&mut self, ui: &mut egui::Ui, height: f32) {
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), height)),
            0.0,
            theme::PANEL,
        );
        let icon = icon_texture(&ctx, &mut self.chrome.icons, ICON_COLORS);
        if theme::panel_header(ui, "颜色", icon.as_ref()) {
            self.chrome.color_open = false;
            return;
        }
        ui.add_space(7.0);

        let hsv = self.color_hsv();
        self.ensure_color_textures(&ctx, hsv[0]);
        let mut new_hsv = hsv;
        let mut changed = false;

        let content_w = ui.available_width() - 16.0;
        let square = (content_w - 14.0 - 8.0 - 76.0).clamp(104.0, 150.0);
        let mut picker_response = None;
        let mut hue_response = None;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            // saturation/value 方块
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(square, square), egui::Sense::click_and_drag());
            if let Some(tex) = self.chrome.sv_texture.as_ref() {
                ui.painter().image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            let marker = egui::pos2(
                rect.left() + hsv[1] * rect.width(),
                rect.top() + (1.0 - hsv[2]) * rect.height(),
            );
            ui.painter().circle_stroke(
                marker,
                5.0,
                egui::Stroke::new(2.0, egui::Color32::from_black_alpha(180)),
            );
            ui.painter()
                .circle_stroke(marker, 5.0, egui::Stroke::new(1.0, egui::Color32::WHITE));
            ui.painter().rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, theme::LINE),
                egui::StrokeKind::Inside,
            );
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            picker_response = Some(resp);

            ui.add_space(6.0);
            // 竖直色相条
            let (bar, resp) =
                ui.allocate_exact_size(egui::vec2(14.0, square), egui::Sense::click_and_drag());
            if let Some(tex) = self.chrome.hue_texture.as_ref() {
                ui.painter().image(
                    tex.id(),
                    bar,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            let y = bar.top() + hsv[0] * bar.height();
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(bar.left() - 1.0, y - 1.0),
                    egui::pos2(bar.right() + 1.0, y + 1.0),
                ),
                0.0,
                egui::Color32::WHITE,
            );
            ui.painter().rect_stroke(
                bar,
                0.0,
                egui::Stroke::new(1.0, theme::LINE),
                egui::StrokeKind::Inside,
            );
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            }
            hue_response = Some(resp);

            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width(72.0);
                let cur = hsv_color32(hsv, 255);
                let prev = self.color_manager.get_previous_color();
                let prev_rgb = prev.get_rgb();
                let prev_c = egui::Color32::from_rgb(
                    (prev_rgb[0] * 255.0).round() as u8,
                    (prev_rgb[1] * 255.0).round() as u8,
                    (prev_rgb[2] * 255.0).round() as u8,
                );
                ui.horizontal(|ui| {
                    theme::swatch(ui, prev_c, 20.0, false).on_hover_text("上一个颜色");
                    let sw = theme::swatch(ui, cur, 20.0, true);
                    sw.on_hover_text("当前颜色");
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(self.color_manager.get_color().to_hex_str())
                        .monospace()
                        .size(11.5)
                        .color(theme::TEXT_DIM),
                );
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new(format!("H {:>3.0}°", hsv[0] * 360.0))
                        .size(11.5)
                        .color(theme::TEXT_FAINT),
                );
                ui.label(
                    egui::RichText::new(format!("S {:>3.0}%", hsv[1] * 100.0))
                        .size(11.5)
                        .color(theme::TEXT_FAINT),
                );
                ui.label(
                    egui::RichText::new(format!("V {:>3.0}%", hsv[2] * 100.0))
                        .size(11.5)
                        .color(theme::TEXT_FAINT),
                );
            });
        });

        if let Some(resp) = picker_response {
            if (resp.clicked() || resp.dragged()) && square > 0.0 {
                if let Some(p) = resp.interact_pointer_pos() {
                    let r = resp.rect;
                    new_hsv[1] = ((p.x - r.left()) / r.width()).clamp(0.0, 1.0);
                    new_hsv[2] = (1.0 - (p.y - r.top()) / r.height()).clamp(0.0, 1.0);
                    changed = true;
                }
            }
        }
        if let Some(resp) = hue_response {
            if (resp.clicked() || resp.dragged()) && square > 0.0 {
                if let Some(p) = resp.interact_pointer_pos() {
                    let r = resp.rect;
                    new_hsv[0] = ((p.y - r.top()) / r.height()).clamp(0.0, 1.0);
                    changed = true;
                }
            }
        }
        if changed {
            self.set_color_hsv(new_hsv);
        }

        // 颜色历史
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("历史")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
            let history: Vec<_> = self.color_manager.get_history().to_vec();
            for (i, c) in history.iter().enumerate() {
                let rgb = c.get_rgb();
                let col = egui::Color32::from_rgb(
                    (rgb[0] * 255.0).round() as u8,
                    (rgb[1] * 255.0).round() as u8,
                    (rgb[2] * 255.0).round() as u8,
                );
                let resp = theme::swatch(ui, col, 17.0, false);
                if resp.clicked() {
                    self.set_color(*c);
                }
                resp.on_hover_text(format!("历史颜色 {}", i + 1));
            }
        });

        // 调色板
        ui.add_space(4.0);
        theme::hline(ui, theme::LINE_SOFT);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("调色板")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if ui
                    .add(egui::Button::new("+").frame(false).small())
                    .on_hover_text("把当前颜色加入调色板")
                    .clicked()
                {
                    self.palette_add_current_color();
                }
                if ui
                    .add(egui::Button::new("›").frame(false).small())
                    .on_hover_text("下一个调色板颜色")
                    .clicked()
                {
                    self.palette_step(1);
                }
                if ui
                    .add(egui::Button::new("‹").frame(false).small())
                    .on_hover_text("上一个调色板颜色")
                    .clicked()
                {
                    self.palette_step(-1);
                }
            });
        });
        ui.add_space(2.0);
        let palette_colors: Vec<_> = self
            .color_manager
            .palette()
            .iter_colors()
            .take(36)
            .filter_map(|c| c)
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(1.0, 1.0);
            ui.add_space(8.0);
            for c in palette_colors {
                let col = egui::Color32::from_rgb(
                    (c.r * 255.0).round() as u8,
                    (c.g * 255.0).round() as u8,
                    (c.b * 255.0).round() as u8,
                );
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
                ui.painter().rect_filled(rect, 0.0, col);
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(
                        1.0,
                        if resp.hovered() {
                            theme::TEXT
                        } else {
                            theme::LINE_SOFT
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                if resp.clicked() {
                    self.set_color(pigment_engine::color::Color::Rgb(c));
                }
                resp.on_hover_text("调色板颜色");
            }
        });
        // 面板底部留一点呼吸，避免色块贴住分隔条。
        ui.add_space(4.0);
    }

    fn ensure_color_textures(&mut self, ctx: &egui::Context, hue: f32) {
        let deg = ((hue.rem_euclid(1.0)) * 360.0).round() as i32 % 360;
        if self.chrome.sv_texture.is_none() || self.chrome.sv_hue_deg != deg {
            const N: usize = 96;
            let mut pixels = Vec::with_capacity(N * N);
            for y in 0..N {
                let v = 1.0 - y as f32 / (N - 1) as f32;
                for x in 0..N {
                    let s = x as f32 / (N - 1) as f32;
                    pixels.push(hsv_color32([deg as f32 / 360.0, s, v], 255));
                }
            }
            let tex = ctx.load_texture(
                format!("pigment-sv-{deg}"),
                egui::ColorImage {
                    size: [N, N],
                    source_size: egui::Vec2::new(N as f32, N as f32),
                    pixels,
                },
                egui::TextureOptions::LINEAR,
            );
            self.chrome.sv_texture = Some(tex);
            self.chrome.sv_hue_deg = deg;
        }
        if self.chrome.hue_texture.is_none() {
            const H: usize = 192;
            let mut pixels = Vec::with_capacity(H);
            for y in 0..H {
                let h = y as f32 / (H - 1) as f32;
                pixels.push(hsv_color32([h, 1.0, 1.0], 255));
            }
            let tex = ctx.load_texture(
                "pigment-hue-bar",
                egui::ColorImage {
                    size: [1, H],
                    source_size: egui::Vec2::new(1.0, H as f32),
                    pixels,
                },
                egui::TextureOptions::LINEAR,
            );
            self.chrome.hue_texture = Some(tex);
        }
    }

    // =====================================================================
    // 图层面板
    // =====================================================================

    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let icon = icon_texture(&ctx, &mut self.chrome.icons, ICON_LAYERS);
        if theme::panel_header(ui, "图层", icon.as_ref()) {
            self.chrome.layers_open = false;
            return;
        }

        // 动作行
        let mut clicked: Option<&'static str> = None;
        ui.horizontal(|ui| {
            ui.add_space(5.0);
            let actions = [
                ("NewPaintingLayerAbove", "新建绘画层"),
                ("NewLayerGroupAbove", "新建图层组"),
                ("DuplicateLayer", "复制当前层"),
                ("RaiseLayerInStack", "上移"),
                ("LowerLayerInStack", "下移"),
                ("MergeLayerDown", "向下合并"),
                ("RemoveLayer", "删除图层"),
            ];
            for (id, tip) in actions {
                let enabled = self.action_available(id);
                let tex = toolbars::action_icon_name(id)
                    .and_then(|n| icon_texture(&ctx, &mut self.chrome.icons, n));
                let fallback = toolbars::action_short_label(id)
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_string();
                let resp = theme::small_icon_button(ui, tex.as_ref(), &fallback, enabled, tip);
                if resp.clicked() {
                    clicked = Some(id);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if ui
                    .add(egui::Button::new("×").frame(false).small())
                    .on_hover_text("关闭图层面板")
                    .clicked()
                {
                    self.chrome.layers_open = false;
                }
            });
        });
        if let Some(id) = clicked {
            let now = ctx.input(|i| i.time);
            self.run_action(id, &ctx, now);
        }
        theme::hline(ui, theme::LINE);

        // 行数据快照：UI 绘制期间不持有 LayerStack 借用（命令需要 &mut doc）。
        #[derive(Clone)]
        struct Row {
            path: Vec<usize>,
            id: u64,
            name: String,
            depth: usize,
            is_group: bool,
            is_vector: bool,
            visible: bool,
            locked: bool,
            children: usize,
            current: bool,
            mode: String,
        }
        let rows: Vec<Row> = {
            let ls = self.doc.layers();
            let current = ls.current_path().to_vec();
            let collapsed = self.layers_collapsed.clone();
            let mut out = Vec::new();
            for (path, layer) in ls.walk() {
                let mut hidden = false;
                for i in 1..path.len() {
                    if collapsed.contains(&path[..i].to_vec()) {
                        hidden = true;
                        break;
                    }
                }
                if hidden {
                    continue;
                }
                out.push(Row {
                    id: layer.id,
                    name: layer.display_name(*path.last().unwrap_or(&0)),
                    depth: path.len().saturating_sub(1),
                    is_group: layer.is_group(),
                    is_vector: layer.is_vector(),
                    visible: layer.visible,
                    locked: layer.locked,
                    children: layer.child_count(),
                    current: path == current,
                    mode: layer.mode.label().to_string(),
                    path,
                });
            }
            out
        };

        let bg_visible = self.doc.layers().background_visible();
        let bg_color = self.doc.layers().background_color();
        let footer_h = 57.0;
        let list_h = (ui.available_height() - footer_h).max(70.0);
        let mut select: Option<Vec<usize>> = None;
        let mut toggle_visible: Option<(Vec<usize>, bool)> = None;
        let mut toggle_locked: Option<(Vec<usize>, bool)> = None;
        let mut collapse: Option<Vec<usize>> = None;
        let mut start_rename: Option<(u64, Vec<usize>, String)> = None;
        let mut toggle_bg = false;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), list_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("chrome_layers_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for row in &rows {
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), theme::ROW_H),
                                egui::Sense::click(),
                            );
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, 0.0, theme::HOVER);
                            }
                            if row.current {
                                ui.painter().rect_filled(rect, 0.0, theme::SELECTED);
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_max(
                                        rect.left_top(),
                                        egui::pos2(rect.left() + 2.0, rect.bottom()),
                                    ),
                                    0.0,
                                    theme::ACCENT,
                                );
                            }
                            let base = rect.left() + 6.0 + row.depth as f32 * 12.0;
                            // 折叠箭头
                            if row.is_group && row.children > 0 {
                                let arrow = egui::Rect::from_center_size(
                                    egui::pos2(base + 5.0, rect.center().y),
                                    egui::vec2(10.0, 10.0),
                                );
                                let collapsed = self.layers_collapsed.contains(&row.path);
                                icon_at(
                                    &ui.painter().with_clip_rect(rect),
                                    &mut self.chrome.icons,
                                    &ctx,
                                    if collapsed {
                                        ICON_GROUP_CLOSED
                                    } else {
                                        ICON_GROUP_OPEN
                                    },
                                    arrow,
                                    theme::TEXT_FAINT,
                                );
                            }
                            // 眼睛 / 锁
                            let eye = egui::Rect::from_center_size(
                                egui::pos2(base + 21.0, rect.center().y),
                                egui::vec2(12.0, 12.0),
                            );
                            icon_at(
                                &ui.painter().with_clip_rect(rect),
                                &mut self.chrome.icons,
                                &ctx,
                                if row.visible {
                                    ICON_EYE_OPEN
                                } else {
                                    ICON_EYE_CLOSED
                                },
                                eye,
                                if row.visible {
                                    theme::TEXT_DIM
                                } else {
                                    theme::TEXT_FAINT
                                },
                            );
                            let lock = egui::Rect::from_center_size(
                                egui::pos2(base + 36.0, rect.center().y),
                                egui::vec2(11.0, 11.0),
                            );
                            icon_at(
                                &ui.painter().with_clip_rect(rect),
                                &mut self.chrome.icons,
                                &ctx,
                                if row.locked {
                                    ICON_LOCKED
                                } else {
                                    ICON_UNLOCKED
                                },
                                lock,
                                if row.locked {
                                    theme::WARN
                                } else {
                                    theme::TEXT_FAINT
                                },
                            );
                            let type_icon = if row.is_group {
                                ICON_LAYER_GROUP
                            } else if row.is_vector {
                                ICON_LAYER_VECTOR
                            } else {
                                ICON_LAYER
                            };
                            icon_at(
                                &ui.painter().with_clip_rect(rect),
                                &mut self.chrome.icons,
                                &ctx,
                                type_icon,
                                egui::Rect::from_center_size(
                                    egui::pos2(base + 52.0, rect.center().y),
                                    egui::vec2(12.0, 12.0),
                                ),
                                if row.current {
                                    theme::TEXT
                                } else {
                                    theme::TEXT_DIM
                                },
                            );
                            let text_x = base + 61.0;
                            let clip = egui::Rect::from_min_max(
                                egui::pos2(text_x, rect.top()),
                                egui::pos2(rect.right() - 6.0, rect.bottom()),
                            );
                            let color = if row.current {
                                theme::TEXT
                            } else {
                                theme::TEXT_DIM
                            };
                            ui.painter().with_clip_rect(clip).text(
                                egui::pos2(text_x, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &row.name,
                                egui::FontId::proportional(13.0),
                                color,
                            );
                            resp.clone()
                                .on_hover_text(format!("{}\n{}\n双击重命名", row.name, row.mode));
                            if resp.clicked() {
                                let p = resp.interact_pointer_pos().unwrap_or(rect.center());
                                if row.is_group && row.children > 0 && p.x < base + 11.0 {
                                    collapse = Some(row.path.clone());
                                } else if p.x < base + 28.0 {
                                    toggle_visible = Some((row.path.clone(), !row.visible));
                                } else if p.x < base + 43.0 {
                                    toggle_locked = Some((row.path.clone(), !row.locked));
                                } else {
                                    select = Some(row.path.clone());
                                }
                            }
                            if resp.double_clicked() {
                                let p = resp.interact_pointer_pos().unwrap_or(rect.center());
                                if p.x >= base + 43.0 {
                                    start_rename =
                                        Some((row.id, row.path.clone(), row.name.clone()));
                                }
                            }
                        }

                        // 背景行
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), theme::ROW_H),
                            egui::Sense::click(),
                        );
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 0.0, theme::HOVER);
                        }
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(rect.left(), rect.top() + 4.0),
                                egui::pos2(rect.left() + 2.0, rect.bottom() - 4.0),
                            ),
                            0.0,
                            theme::LINE,
                        );
                        icon_at(
                            &ui.painter().with_clip_rect(rect),
                            &mut self.chrome.icons,
                            &ctx,
                            if bg_visible {
                                ICON_EYE_OPEN
                            } else {
                                ICON_EYE_CLOSED
                            },
                            egui::Rect::from_center_size(
                                egui::pos2(rect.left() + 27.0, rect.center().y),
                                egui::vec2(12.0, 12.0),
                            ),
                            theme::TEXT_FAINT,
                        );
                        let bg = egui::Color32::from_rgb(bg_color[0], bg_color[1], bg_color[2]);
                        ui.painter().rect_filled(
                            egui::Rect::from_center_size(
                                egui::pos2(rect.left() + 50.0, rect.center().y),
                                egui::vec2(12.0, 12.0),
                            ),
                            0.0,
                            bg,
                        );
                        ui.painter().rect_stroke(
                            egui::Rect::from_center_size(
                                egui::pos2(rect.left() + 50.0, rect.center().y),
                                egui::vec2(12.0, 12.0),
                            ),
                            0.0,
                            egui::Stroke::new(1.0, theme::LINE),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 61.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "背景",
                            egui::FontId::proportional(13.0),
                            theme::TEXT_FAINT,
                        );
                        if resp.clicked() {
                            toggle_bg = true;
                        }
                        resp.on_hover_text("显示/隐藏文档背景");
                    });
            },
        );

        // 应用行命令（绘制期间只收集，避免与 &LayerStack 借用冲突）。
        if let Some(path) = collapse {
            if !self.layers_collapsed.remove(&path) {
                self.layers_collapsed.insert(path);
            }
        }
        if let Some((path, v)) = toggle_visible {
            self.doc.set_layer_visible(&path, v);
            self.force_resync = true;
            self.scratch.force_resync = true;
        }
        if let Some((path, v)) = toggle_locked {
            self.doc.set_layer_locked(&path, v);
        }
        if toggle_bg {
            self.doc.layers_mut().set_background_visible(!bg_visible);
            self.force_resync = true;
            self.scratch.force_resync = true;
        }
        if let Some(path) = select {
            self.doc.select_layer(&path);
        }
        if let Some((id, path, name)) = start_rename {
            self.layer_rename = Some(id);
            self.rename_buf = name;
            self.rename_focused = false;
            self.rename_path = path;
        }

        // 底部：不透明度 + 混合模式
        theme::hline(ui, theme::LINE);
        let current = self.doc.layers().current().clone();
        let path = self.doc.current_path().to_vec();
        ui.add_space(3.0);
        let mut opacity = current.opacity;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("不透明度")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
            let resp = theme::flat_slider(ui, &mut opacity, 0.0..=1.0, 92.0, theme::TEXT_DIM);
            ui.label(
                egui::RichText::new(format!("{:.0}%", opacity * 100.0))
                    .size(11.5)
                    .monospace()
                    .color(theme::TEXT_DIM),
            );
            if resp.changed() {
                self.doc.set_layer_opacity(&path, opacity);
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("模式")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
            let label = current.mode.label();
            let mut chosen = current.mode;
            egui::ComboBox::from_id_salt("chrome_layer_mode")
                .width((ui.available_width() - 12.0).max(100.0))
                .selected_text(egui::RichText::new(label).size(12.5).color(theme::TEXT_DIM))
                .show_ui(ui, |ui| {
                    let mut set = |ui: &mut egui::Ui, mode: LayerMode| {
                        if ui.selectable_label(chosen == mode, mode.label()).clicked() {
                            chosen = mode;
                            ui.close();
                        }
                    };
                    for m in CombineMode::ALL {
                        set(ui, LayerMode::Standard(m));
                    }
                    if current.is_group() {
                        ui.separator();
                        set(ui, LayerMode::PassThrough);
                    }
                });
            if chosen != current.mode {
                self.doc.set_layer_mode(&path, chosen);
            }
        });
        ui.add_space(3.0);
    }

    // =====================================================================
    // 动作可用性 / 勾选状态 / chrome 专属动作
    // =====================================================================

    /// 菜单/工具条的勾选态。`None` = 纯命令，不画勾。
    pub(crate) fn action_checked(&self, id: &str) -> Option<bool> {
        if let Some(f) = crate::display_filter::DisplayFilter::from_action_id(id) {
            return Some(self.display_filter == f);
        }
        match id {
            "Fullscreen" => Some(self.fullscreen || self.window_fullscreen),
            "FullscreenAutohide" => Some(self.autohide),
            "FitView" => Some(self.saved_view.is_some()),
            "ShowBackgroundToggle" => Some(self.doc.layers().background_visible()),
            "FrameToggle" => Some(self.doc.get_frame_enabled()),
            "SymmetryActive" => Some(self.doc.symmetry_active()),
            "SoloLayer" => Some(self.doc.layers().current_layer_solo()),
            "ToggleScaleFeedback" => Some(self.scale_feedback),
            "ToggleLastPosFeedback" => Some(self.last_pos_feedback),
            "PrintInputs" => Some(self.print_inputs),
            "VisualizeRendering" => Some(self.visualize_rendering),
            "NoDoubleBuffereing" => Some(self.no_double_buffering),
            "ContextRestoreColor" => Some(self.color_manager.context_restores_color()),
            "LayerVisibleToggle" => Some(self.doc.layers().current().visible),
            "LayerLockedToggle" => Some(self.doc.layers().current().locked),
            "BlendModeNormal" => Some(!self.eraser && !self.lock_alpha && !self.colorize),
            "BlendModeEraser" => Some(self.eraser),
            "BlendModeLockAlpha" => Some(self.lock_alpha),
            "BlendModeColorize" => Some(self.colorize),
            "BrushGroups" => Some(self.chrome.left_open),
            "ColorAdjusters" | "PaletteTool" | "ComponentSlidersTool" => {
                Some(self.chrome.color_open)
            }
            "ToggleLayersTool" => Some(self.chrome.layers_open),
            "PreferencesWindow" => Some(self.chrome.prefs_open),
            "InputTestWindow" => Some(self.input_test_open),
            "BrushEditorWindow" => Some(self.brush_editor_open),
            "About" => Some(self.about_open),
            // 模式栈顶：MyPaint 的 GtkRadioAction 语义。
            "FreehandMode" | "InkingMode" | "StraightMode" | "SequenceMode" | "EllipseMode"
            | "FloodFillMode" | "FloodFillGCMode" | "LayerMoveMode" | "FrameEditMode"
            | "SymmetryEditMode" | "PanViewMode" | "ZoomViewMode" | "RotateViewMode"
            | "ColorPickMode" | "ColorPickModeH" | "ColorPickModeC" | "ColorPickModeY" => {
                Some(self.modes.top().action_name() == Some(id))
            }
            _ => None,
        }
    }

    /// 新 chrome 专属动作。返回 true 表示已消费。
    ///
    /// 这些动作在源码里对应停靠面板/子窗口的 reveal/toggle；旧移植的
    /// `run_action` 尚未接线，放在这里避免把布局状态塞回 `app.rs`。
    pub(crate) fn chrome_action(&mut self, id: &str) -> bool {
        match id {
            "BrushGroups" => {
                self.chrome.left_open = !self.chrome.left_open;
                true
            }
            "ColorAdjusters" | "PaletteTool" | "ComponentSlidersTool" => {
                self.chrome.color_open = !self.chrome.color_open;
                self.chrome.layers_open = self.chrome.layers_open || !self.chrome.color_open;
                true
            }
            "HCYWheelTool"
            | "HSVWheelTool"
            | "HSVSquareTool"
            | "HSVCubeTool"
            | "WashColorChangerTool"
            | "RingsColorChangerTool"
            | "CrossedBowlColorChangerTool" => {
                self.chrome.color_open = true;
                true
            }
            "ToggleLayersTool" => {
                self.chrome.layers_open = !self.chrome.layers_open;
                self.chrome.color_open = self.chrome.color_open || !self.chrome.layers_open;
                true
            }
            "RevealLayersTool" | "LayerProperties" => {
                self.chrome.layers_open = true;
                true
            }
            "RevealModeOptionsTool" | "ToggleModeOptionsTool" => {
                // 工具选项页尚未在新 chrome 中恢复；保留动作消费，避免
                // 菜单点击后落到旧移植的 no-op 分支。
                true
            }
            "PreferencesWindow" => {
                self.chrome.prefs_open = true;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_prefs(tag: &str) -> pigment_engine::prefs::Prefs {
        let path =
            std::env::temp_dir().join(format!("pigment-chrome-{}-{tag}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        pigment_engine::prefs::Prefs::load(path)
    }

    #[test]
    fn chrome_layout_roundtrips_through_prefs() {
        let mut prefs = fresh_prefs("layout");
        let state = ChromeState {
            left_open: false,
            color_open: true,
            layers_open: false,
            left_width: 222.0,
            right_width: 274.0,
            color_height: 312.0,
            ..ChromeState::default()
        };
        state.store(&mut prefs);
        let back = ChromeState::from_prefs(&prefs);
        assert!(!back.left_open);
        assert!(back.color_open);
        assert!(!back.layers_open);
        assert_eq!(back.left_width, 222.0);
        assert_eq!(back.right_width, 274.0);
        assert_eq!(back.color_height, 312.0);
    }

    #[test]
    fn gtk_colors_apply_to_the_main_surfaces() {
        let v = theme::dark_visuals();
        assert_eq!(v.panel_fill, theme::PANEL);
        assert_eq!(v.window_fill, theme::MENU);
        assert_eq!(v.selection.bg_fill, theme::SELECTED);
        assert_eq!(v.window_corner_radius, egui::CornerRadius::ZERO);
    }
}

#[test]
fn canvas_backdrop_follows_document_background() {
    let mut layers = pigment_surface::LayerStack::new();
    layers.set_background_color([0xED, 0xEF, 0xE3]);
    assert_eq!(
        canvas_backdrop(&layers),
        egui::Color32::from_rgb(0xED, 0xEF, 0xE3),
        "可见背景必须先铺满整个 allocation（源码 _draw_cb 的顺序）"
    );
    layers.set_background_visible(false);
    assert_eq!(
        canvas_backdrop(&layers),
        theme::VOID,
        "背景隐藏时不能把透明区域误显示成纸色"
    );

    // 预览反转背景可见性（tree.py:299-300）
    layers.set_preview_path(Some(vec![0]));
    assert_eq!(
        canvas_backdrop(&layers),
        egui::Color32::from_rgb(0xED, 0xEF, 0xE3),
        "背景隐藏 + 预览 → 预览把背景显示出来"
    );
    layers.set_background_visible(true);
    assert_eq!(
        canvas_backdrop(&layers),
        theme::VOID,
        "背景可见 + 预览 → 预览把背景藏起来，交给棋盘格"
    );
}
