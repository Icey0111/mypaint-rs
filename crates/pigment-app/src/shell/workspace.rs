//! 工作区 —— `MyPaintWorkspace`（`gui/workspace.py:32`）。
//!
//! 源码里这块是**可停靠面板系统**：`Gtk.VBox`，中央是画布
//! （`TiledDrawWidget`，glade 里 `expand=True`），四周挂 `ToolStack`
//! （`gui/toolstack.py`），每个工具面板都是一个 `Gtk.Notebook` 的一页，
//! 可以拖到另一侧、也可以拖出来变成独立窗口，布局本身可序列化
//! （`get_layout()` / `build_from_layout()`）。
//!
//! S6 的停靠语义（添加/移除/浮动/标签切换）由 [`crate::shell::dock`]
//! 承接（技术栈适配：egui_dock 0.21 只支持 egui ^0.36，与本项目锁定的
//! egui 0.37 不兼容，改为 egui 原生实现同样的模型）。本文件保留：
//! 中央画布（M2 以来的渲染关键路径）与笔刷编辑器面板的内容。
//!
//! 默认停靠（差异记账）：MyPaint 出厂布局里侧栏是空的
//! （`userconfig.py:104`，`workspace.layout` 只有 position/autohide），
//! 面板靠动作按需 reveal；本项目此前一直默认显示笔刷面板与编辑器，
//! 保留该初始状态：左 = BrushGroupTool，右 = BrushEditor。

use std::sync::{Arc, Mutex};

use eframe::egui;

use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::{Brush, BrushSetting};
use pigment_surface::LayerStack;

use crate::app::PigmentApp;
use crate::gpu::{TileRenderer, TileUpload};
use crate::view::View;

/// 极端缩小时可见范围可达数万 tile，夹到视口中心附近的上限内
/// （空区域由面板白底兜底，不影响观感；后续再上 atlas/mip 方案）。
const MAX_VISIBLE_TILES_PER_AXIS: i32 = 96;

/// 一行 `label + Slider`，返回是否改动；范围来自 `brushsettings.json`。
fn brush_setting_slider(
    ui: &mut egui::Ui,
    brush: &mut Brush,
    label: &str,
    setting: BrushSetting,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let mut value = brush.get(setting).base_value;
    let resp = ui
        .horizontal(|ui| {
            ui.label(label);
            ui.add(egui::Slider::new(&mut value, range).show_value(true))
        })
        .inner;
    if resp.changed() {
        brush.get_mut(setting).base_value = value;
        true
    } else {
        false
    }
}

/// 一帧的画布同步结果：显存增量 + 可见 tile 列表 + 世界→裁剪空间矩阵。
///
/// 被主画布（[`PigmentApp::canvas`]）与暂存区画布
/// （[`crate::shell::scratch_panel::PigmentApp::scratchpad_canvas`]）共用：
/// 两者唯一的区别是"用哪一份 `LayerStack` / `View` / 渲染器"。
pub(crate) struct CanvasTiles {
    /// 本帧需要上传的脏 tile。
    pub(crate) uploads: Vec<TileUpload>,
    /// 本帧需要从显存剔除的 tile（撤销后已不在画布上）。
    pub(crate) evict: Vec<(i32, i32)>,
    /// 画布矩形覆盖的可见 tile 坐标（可能含尚未有内容的空 tile）。
    pub(crate) visible: Vec<(i32, i32)>,
    /// 世界像素 → 全窗口裁剪空间矩阵（窗口物理像素域）。
    pub(crate) view_matrix: [[f32; 4]; 4],
}

/// 比对 CPU 合成版本号与显存水位线，算出本帧要上传/剔除的 tile。
///
/// 自愈式同步（判据是状态而非事件流水）的全部逻辑都在这里，主画布与暂存区
/// 走同一条路径 —— 见 [`crate::gpu`] 的模块文档。
pub(crate) fn prepare_canvas(
    ctx: &egui::Context,
    renderer: &Arc<Mutex<TileRenderer>>,
    layers: &LayerStack,
    view: &View,
    rect: egui::Rect,
    force_resync: bool,
) -> CanvasTiles {
    let ppp = ctx.pixels_per_point();
    // ---- 可见 tile 列表（物理像素域）----
    let vp_w = rect.width() * ppp;
    let vp_h = rect.height() * ppp;
    let view_phys = View {
        zoom: view.zoom * ppp,
        offset: [view.offset[0] * ppp, view.offset[1] * ppp],
    };
    let (mut tx0, mut ty0, mut tx1, mut ty1) =
        view_phys.visible_tile_range(vp_w, vp_h, TILE_SIZE as i32);
    if tx1 - tx0 + 1 > MAX_VISIBLE_TILES_PER_AXIS {
        let c = (tx0 + tx1) / 2;
        tx0 = c - MAX_VISIBLE_TILES_PER_AXIS / 2;
        tx1 = tx0 + MAX_VISIBLE_TILES_PER_AXIS - 1;
    }
    if ty1 - ty0 + 1 > MAX_VISIBLE_TILES_PER_AXIS {
        let c = (ty0 + ty1) / 2;
        ty0 = c - MAX_VISIBLE_TILES_PER_AXIS / 2;
        ty1 = ty0 + MAX_VISIBLE_TILES_PER_AXIS - 1;
    }
    let mut visible =
        Vec::with_capacity(((tx1 - tx0 + 1).max(0) * (ty1 - ty0 + 1).max(0)) as usize);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            visible.push((tx, ty));
        }
    }

    // ---- 自愈式增量同步：比对 CPU revision 与显存已上传 revision ----
    // 判据是状态而非事件流水，所以本帧的决定可被安全丢弃后重算，不会丢上传。
    let mut uploads: Vec<TileUpload> = Vec::new();
    let evict: Vec<(i32, i32)> = {
        let mut renderer = renderer.lock().unwrap();
        if force_resync {
            renderer.invalidate_all();
        }
        // B2-3：上传的是**图层栈的合成结果**（含背景与各层混合模式）。
        // 水位线用全局合成版本号：任何影响合成的变化（作画/结构/属性/
        // 背景）都会抬高它。粒度粗于逐 tile revision，但正确性不变；
        // 逐 tile 哈希记账为后续优化。
        let composite_rev = layers.composite_revision();
        let plan = crate::gpu::plan_uploads(
            |c| renderer.uploaded_rev(c),
            &visible,
            crate::gpu::MAX_UPLOADS_PER_FRAME,
            |c| layers.has_paint_at(c.0, c.1),
            composite_rev,
        );
        for coord in plan {
            let Some(pixels) = layers.composite_tile_rgba8(coord.0, coord.1) else {
                continue;
            };
            uploads.push(TileUpload {
                coord,
                revision: composite_rev,
                pixels,
            });
        }
        // 撤销会把画布退回较早状态：那时"多出来"的 tile 已从 CPU 消失，
        // 但显存里还留着旧内容，必须剔除，否则画面残留撤销不掉的笔迹。
        crate::gpu::plan_evictions(
            |c| renderer.uploaded_rev(c).is_some(),
            &visible,
            |c| layers.has_paint_at(c.0, c.1),
        )
    };

    // ---- 视图矩阵：world px → 全窗口裁剪空间 ----
    // 用全窗口 `content_rect` 作参考系，所以要把本面板的原点 `rect.min` 加回去；
    // 画布从"占满窗口"变成"工作区中央"（乃至侧栏里的一块）之后，这一步不能省。
    let win = ctx.content_rect();
    let view_window = View {
        zoom: view_phys.zoom,
        offset: [
            (view.offset[0] + rect.min.x) * ppp,
            (view.offset[1] + rect.min.y) * ppp,
        ],
    };
    CanvasTiles {
        uploads,
        evict,
        visible,
        view_matrix: view_window.clip_matrix(win.width() * ppp, win.height() * ppp),
    }
}

impl PigmentApp {
    /// 工作区：左右停靠侧栏 + 中央画布 + 浮动面板窗口。
    pub(crate) fn workspace(&mut self, ui: &mut egui::Ui) {
        // 左右侧栏（栈空自动收起；宽度由 egui 记忆，可拖）
        self.dock_sidebars(ui);

        egui::CentralPanel::default()
            // 画布底色 = MyPaint 默认背景（米白纸色），不是纯白
            // 面板底色 = 文档背景层颜色（照 tree.py 的背景 BLIT 语义；
            // 默认白 (255,255,255)，随「背景色」对话框变化）
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(
                self.doc.layers().background_color()[0],
                self.doc.layers().background_color()[1],
                self.doc.layers().background_color()[2],
            )))
            .show(ui, |ui| self.canvas(ui));

        // 浮动面板窗口（ToolStackWindow 的承接）
        self.dock_floatings(ui);
    }

    /// 笔刷编辑器面板内容（[`crate::shell::dock::PanelId::BrushEditor`]）。
    ///
    /// 源码里这三块都是可停靠工具（颜色器在 `gui/colortools.py`，
    /// 笔刷编辑器是独立窗口 `gui/brusheditor.py`），此处先按侧栏堆叠。
    pub(crate) fn brush_editor_content(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                        // ---- 笔刷信息 ----
                        if let Some(e) = self.brushes.selected_entry() {
                            ui.horizontal(|ui| {
                                // 主标题用 MyPaint 的展示名（笔刷名），description 退居 tooltip
                                ui.label(egui::RichText::new(e.mypaint_name()).heading())
                                    .on_hover_text(if e.display.is_empty() {
                                        e.key.clone()
                                    } else {
                                        format!("{}\n{}", e.mypaint_name(), e.display)
                                    });
                                ui.label(egui::RichText::new(&e.group_display).weak().small());
                            });
                            ui.label(
                                egui::RichText::new(&e.key)
                                    .weak()
                                    .small()
                                    .monospace(),
                            );
                            if !e.notes.is_empty() {
                                egui::CollapsingHeader::new("笔刷说明 / 授权")
                                    .id_salt("brush-notes")
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new(&e.notes).small());
                                    });
                            }
                        }
                        ui.separator();

                        // ---- 颜色历史 + HSV 调整 ----
                        egui::CollapsingHeader::new("颜色")
                            .id_salt("color")
                            .default_open(true)
                            .show(ui, |ui| {
                                let history: Vec<[f32; 3]> = self
                                    .color_manager
                                    .get_history()
                                    .iter()
                                    .map(crate::color::hsv_of)
                                    .collect();
                                let n = history.len();
                                ui.horizontal_wrapped(|ui| {
                                    for (i, hsv) in history.into_iter().enumerate() {
                                        let tip = format!("颜色历史 {}/{}", i + 1, n);
                                        if crate::app::color_swatch(ui, hsv, false, &tip).clicked()
                                        {
                                            self.set_color_hsv(hsv);
                                        }
                                    }
                                    ui.label(
                                        egui::RichText::new("最近用的在最后")
                                            .weak()
                                            .small(),
                                    );
                                });
                                let mut color = self.color_hsv();
                                let mut changed = false;
                                ui.horizontal(|ui| {
                                    ui.label("色相");
                                    changed |= ui
                                        .add(egui::Slider::new(&mut color[0], 0.0..=1.0).show_value(true))
                                        .changed();
                                });
                                ui.horizontal(|ui| {
                                    ui.label("饱和");
                                    changed |= ui
                                        .add(egui::Slider::new(&mut color[1], 0.0..=1.0).show_value(true))
                                        .changed();
                                });
                                ui.horizontal(|ui| {
                                    ui.label("明度");
                                    changed |= ui
                                        .add(egui::Slider::new(&mut color[2], 0.0..=1.0).show_value(true))
                                        .changed();
                                });
                                if changed {
                                    self.set_color_hsv(color);
                                }
                            });

                        // ---- 笔刷快捷键（MyPaint `BrushManager.contexts`）----
                        egui::CollapsingHeader::new("笔刷快捷键 1–0")
                            .id_salt("brushkeys")
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(
                                        "数字键恢复槽位；Ctrl+数字把当前设置存进槽位。\n\
                                         槽位内容 = 整份笔刷设置（含颜色），与 MyPaint 一致。",
                                    )
                                    .weak()
                                    .small(),
                                );
                                let now = ui.input(|i| i.time);
                                ui.horizontal_wrapped(|ui| {
                                    for i in 0..pigment_engine::brushlib::NUM_BRUSHKEYS {
                                        let label = i.to_string();
                                        let slot = self.brushkeys[i].as_ref();
                                        let tip = match slot {
                                            Some(s) => format!("{} ← {}", i, s.name),
                                            None => format!("{i} ← 空槽位"),
                                        };
                                        let resp = ui
                                            .add(egui::Button::new(label).min_size(egui::vec2(
                                                22.0, 22.0,
                                            )))
                                            .on_hover_text(format!(
                                                "{tip}\n左键恢复 / Ctrl+左键存入"
                                            ));
                                        if !resp.clicked() {
                                            continue;
                                        }
                                        if ui.input(|i| i.modifiers.ctrl) {
                                            self.store_brushkey(i, now);
                                        } else {
                                            self.restore_brushkey(i, now);
                                        }
                                    }
                                });
                                // 开关由颜色管理器持有（偏好 `misc.context_restores_color`，
                                // `userconfig.py:93` 默认 True）。
                                let mut restores = self.color_manager.context_restores_color();
                                if ui
                                    .checkbox(&mut restores, "恢复槽位时连颜色一起还原")
                                    .on_hover_text(
                                        "MyPaint 偏好 misc.context_restores_color（默认开）\n\
                                         userconfig.py:93",
                                    )
                                    .changed()
                                {
                                    self.color_manager.set_context_restores_color(restores);
                                }
                            });

                        // ---- 笔刷编辑器 ----
                        egui::CollapsingHeader::new("笔刷编辑器")
                            .id_salt("editor")
                            .default_open(true)
                            .show(ui, |ui| {
                                // 「实时更新最近一笔」= MyPaint 笔刷编辑器的 live_update
                                // （`gui/brusheditor.py:1119-1139`）：改设置就用新笔
                                // 重绘上一笔，靠的是 `Brushwork.update()`
                                ui.checkbox(&mut self.live_update, "实时更新最近一笔")
                                    .on_hover_text(
                                        "开着时改笔刷设置会用新笔重绘上一笔\n\
                                         （MyPaint 笔刷编辑器的 Live Update）",
                                    );
                                // 编辑器顶部图标 = 当前笔刷的预览图（MyPaint `brusheditor.py:786`）
                                let sel = self.brushes.selected;
                                let icon = self.brushes.icon(ui.ctx(), sel);
                                let edited =
                                    self.editor.show(ui, &mut self.base_brush, Some(&icon));
                                if edited {
                                    self.derive_active();
                                    if self.live_update {
                                        self.apply_live_update();
                                    }
                                }
                            });

                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "编辑只作用于当前会话的这支笔刷（尚未回写 .myb）\n\
                                 用户笔刷目录：%APPDATA%/mypaint/brushes\n\
                                 或用 PIGMENT_BRUSH_PATH 指定",
                            )
                            .weak()
                            .small(),
                        );
                    });
    }

    /// 「最近笔刷与颜色」面板内容
    /// （[`crate::shell::dock::PanelId::HistoryPanel`]，`gui/history.py:220`）。
    ///
    /// 源码面板上半是最近用过的笔刷（BrushHistoryView），下半是颜色历史
    /// （ColorHistoryView）。本应用目前只跟踪颜色历史，笔刷历史记账 B3。
    pub(crate) fn history_panel_content(&mut self, ui: &mut egui::Ui) {
        ui.heading("Recent Brushes & Colors");
        ui.separator();
        ui.label("Colors");
        let history: Vec<[f32; 3]> = self
            .color_manager
            .get_history()
            .iter()
            .map(crate::color::hsv_of)
            .collect();
        let n = history.len();
        ui.horizontal_wrapped(|ui| {
            for (i, hsv) in history.into_iter().enumerate() {
                let tip = format!("颜色历史 {}/{}", i + 1, n);
                if crate::app::color_swatch(ui, hsv, false, &tip).clicked() {
                    self.set_color_hsv(hsv);
                }
            }
            if n == 0 {
                ui.label(egui::RichText::new("（画一笔后出现）").weak().small());
            }
        });
        ui.label(egui::RichText::new("最近用的在最后").weak().small());
        ui.separator();
        // 下半：笔刷历史 —— `BrushHistoryView`（gui/history.py:33-76），
        // 5 格、32px、点一下切回该笔刷（含当时编辑器里的设置）。
        ui.label("Brushes");
        let brush_history: Vec<(usize, String)> = self
            .brushes
            .history
            .iter()
            .map(|e| (e.preview, e.name.clone()))
            .collect();
        ui.horizontal_wrapped(|ui| {
            for (i, (preview, name)) in brush_history.into_iter().enumerate() {
                let tex = self.brushes.icon(ui.ctx(), preview);
                let img = egui::Image::new(&tex).fit_to_exact_size(egui::vec2(32.0, 32.0));
                let resp = ui
                    .add(egui::Button::image(img).frame(false))
                    .on_hover_text(&name);
                if resp.clicked() {
                    self.select_brush_history(i);
                }
            }
        });
        ui.label(
            egui::RichText::new("每笔结束后记录；同一支笔只保留最新设置").weak().small(),
        );
    }

    /// 「工具选项」面板 —— `ModeOptionsTool`（`gui/optionspanel.py:29`）+
    /// `PaintingModeOptionsWidgetBase`（`gui/mode.py:588`）+
    /// `FreehandOptionsWidget`（`gui/freehand.py:630`）。
    ///
    /// 顶部是当前模式名/图标；绘画类模式显示五个公共笔刷设置（Size / Opaque /
    /// Sharp / Gain / Pigment），自由手绘再追加三个（Smooth / Pressure / Twist）。
    ///
    /// 范围取 `_libmypaint_ref/brushsettings.json` 的 `minimum`/`maximum`；
    /// 源码的 `InputSlider` 不显示数值（`set_draw_value(False)`），这里显示
    /// 数值属于技术栈适配（egui 没有动态 tooltip 的等价物）。
    ///
    /// 未移植：`LineModeOptionsWidget` 的压力-距离曲线、填充/对称的选项页。
    pub(crate) fn mode_options_panel_content(&mut self, ui: &mut egui::Ui) {
        let mode_name = self.modes.top().name().to_string();
        let action = self.modes.top().action_name();
        let icon_name = action
            .and_then(crate::shell::actions::by_id)
            .and_then(|a| a.icon_name);
        ui.horizontal(|ui| {
            if let Some(name) = icon_name {
                if let Some(tex) =
                    crate::shell::toolbars::icon_texture(ui.ctx(), &mut self.icon_cache, name)
                {
                    ui.add(egui::Image::new(&tex).max_size(egui::vec2(16.0, 16.0)));
                }
            }
            ui.label(egui::RichText::new(&mode_name).strong());
        });
        ui.separator();

        let painting = matches!(
            mode_name.as_str(),
            "Freehand Drawing" | "Lines and Curves" | "Connected Lines" | "Ellipses and Circles"
        );
        if !painting {
            ui.label(egui::RichText::new("No options available").italics().weak());
            return;
        }

        // ---- 公共设置（mode.py:591-601）----
        let mut changed = false;
        changed |= brush_setting_slider(
            ui,
            &mut self.base_brush,
            "Size:",
            BrushSetting::Radius,
            -2.0..=6.0,
        );
        changed |= brush_setting_slider(
            ui,
            &mut self.base_brush,
            "Opaque:",
            BrushSetting::Opaque,
            0.0..=2.0,
        );
        changed |= brush_setting_slider(
            ui,
            &mut self.base_brush,
            "Sharp:",
            BrushSetting::Hardness,
            0.0..=1.0,
        );
        changed |= brush_setting_slider(
            ui,
            &mut self.base_brush,
            "Gain:",
            BrushSetting::PressureGainLog,
            -1.8..=1.8,
        );
        changed |= brush_setting_slider(
            ui,
            &mut self.base_brush,
            "Pigment:",
            BrushSetting::Paint,
            0.0..=1.0,
        );

        // ---- 自由手绘专用（freehand.py:633-695）----
        if mode_name == "Freehand Drawing" {
            changed |= brush_setting_slider(
                ui,
                &mut self.base_brush,
                "Smooth:",
                BrushSetting::SlowTracking,
                0.0..=10.0,
            );
            ui.horizontal(|ui| {
                ui.label("Pressure:");
                if ui
                    .add(
                        egui::Slider::new(&mut self.mode_prefs.fakepressure, 0.0..=1.0)
                            .show_value(true),
                    )
                    .changed()
                {
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Twist:");
                if ui
                    .add(
                        egui::Slider::new(&mut self.mode_prefs.fakerotation, 0.0..=1.0)
                            .show_value(true),
                    )
                    .changed()
                {
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "鼠标没有压感/桶旋转：Pressure 与 Twist 是模拟值（不进 settings.json）。",
                )
                .weak()
                .small(),
            );
        }

        if changed {
            self.derive_active();
            // 让下一帧的笔迹生效（无需重建模式栈：mode_inputs 每事件都读缓存）
        }

        ui.separator();
        if ui.button("Reset").clicked() {
            // `reset_button_clicked_cb`（mode.py:647-656）：只重置本页暴露的设置。
            if let Ok(parent) = self.brushes.load_selected() {
                for s in [
                    BrushSetting::Radius,
                    BrushSetting::Opaque,
                    BrushSetting::Hardness,
                    BrushSetting::PressureGainLog,
                    BrushSetting::Paint,
                    BrushSetting::SlowTracking,
                ] {
                    let v = parent.get(s).base_value;
                    self.base_brush.get_mut(s).base_value = v;
                }
                self.derive_active();
            }
        }
    }

    /// 预览面板 —— `PreviewTool`（`gui/previewwindow.py:183`）。
    ///
    /// 核心行为逐条对应源码：
    /// - 预览画布按**有效外接框**（画框启用则画框，否则内容框）缩放居中，
    ///   四周留 12px 边（`_update_preview_transformation`，`:653-664`）；
    /// - 视口框 = 主画布四角投到预览坐标（`_update_viewport_overlay`，`:492-533`），
    ///   四角有 `1/4·PHI` 长的角标；
    /// - 框内拖动平移主视图、框外单击把主视图中心挪到点击处
    ///   （`:409-451`）；滚轮缩放主视图（`:382-407`）。
    ///
    /// 未移植：旋转/镜像方向点、`preview.show_viewfinder` 在偏好窗口里的条目。
    pub(crate) fn preview_panel_content(&mut self, ui: &mut egui::Ui) {
        const PAD: f32 = 6.0;
        /// `previewwindow.py:42`：`PHI = (1.0 + sqrt(2)) / 2.0`（不是黄金比）。
        const PHI: f32 = (1.0 + 1.414_213_6) / 2.0;

        let mut show = self
            .prefs
            .get_bool("preview.show_viewfinder")
            .unwrap_or(true);
        let avail = ui.available_size();
        let canvas_h = (avail.y - 26.0).clamp(48.0, 220.0);
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(avail.x.max(32.0), canvas_h),
            egui::Sense::click_and_drag(),
        );
        let inner = rect.shrink(PAD);

        // ---- 有效外接框（`get_effective_bbox`）----
        let mut bbox = if self.doc.get_frame_enabled() {
            self.doc.get_frame()
        } else {
            self.doc.data_bbox()
        };
        if bbox.2 <= 0 {
            bbox.2 = 64;
        }
        if bbox.3 <= 0 {
            bbox.3 = 64;
        }

        // ---- 预览缩略图（mipmap 级刚好塞进面板）----
        let rev = self.doc.layers().composite_revision();
        let max_dim = inner.width().max(inner.height()).max(1.0) as i32;
        let mut level = 0u32;
        while level < 8 {
            let shift = 1i32 << level;
            let w = (bbox.2 + shift - 1) / shift;
            let h = (bbox.3 + shift - 1) / shift;
            if w.max(h) <= max_dim {
                break;
            }
            level += 1;
        }
        let key = (rev, [bbox.0, bbox.1, bbox.2, bbox.3], level);
        if self.preview.key != Some(key) {
            let (rgba, w, h) = self.doc.layers().render_thumbnail_rgba8(bbox, level);
            if w > 0 && h > 0 {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    &rgba,
                );
                self.preview.tex = Some(ui.ctx().load_texture(
                    format!("preview-{rev}-{level}"),
                    img,
                    egui::TextureOptions::LINEAR,
                ));
                self.preview.size = [w, h];
            }
            self.preview.key = Some(key);
        }

        // ---- 摆放（等比放进 inner）----
        let [tw, th] = self.preview.size;
        let image_rect = if tw > 0 && th > 0 {
            let s = (inner.width() / tw as f32)
                .min(inner.height() / th as f32)
                .max(1e-6);
            egui::Rect::from_center_size(
                inner.center(),
                egui::vec2(tw as f32 * s, th as f32 * s),
            )
        } else {
            inner
        };
        let sx = image_rect.width() / bbox.2 as f32;
        let sy = image_rect.height() / bbox.3 as f32;
        let to_panel = |w: [f32; 2]| {
            egui::pos2(
                image_rect.min.x + (w[0] - bbox.0 as f32) * sx,
                image_rect.min.y + (w[1] - bbox.1 as f32) * sy,
            )
        };
        let to_world = |p: egui::Pos2| -> [f32; 2] {
            [
                bbox.0 as f32 + (p.x - image_rect.min.x) / sx,
                bbox.1 as f32 + (p.y - image_rect.min.y) / sy,
            ]
        };

        // ---- 画预览 + 外框阴影 ----
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            image_rect.expand(1.0),
            0.0,
            egui::Color32::from_black_alpha(70),
        );
        if let Some(tex) = self.preview.tex.clone() {
            painter.image(
                tex.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // ---- 视口框（主画布四角 + 角标）----
        let mut viewport_rect: Option<egui::Rect> = None;
        if show {
            if let Some(cr) = self.canvas_rect {
                let tl = self.view.screen_to_world(cr.min.x, cr.min.y);
                let br = self.view.screen_to_world(cr.max.x, cr.max.y);
                let vp = egui::Rect::from_two_pos(
                    to_panel([tl[0], tl[1]]),
                    to_panel([br[0], br[1]]),
                );
                viewport_rect = Some(vp);
                let hovered = resp.hover_pos().is_some_and(|p| vp.contains(p));
                let line = if hovered {
                    egui::Color32::from_rgb(255, 255, 255)
                } else {
                    egui::Color32::from_rgb(190, 190, 190)
                };
                let clip = painter.with_clip_rect(rect.intersect(image_rect));
                // 投影 + 主线（源码 `render_drop_shadow` + EDITABLE/ACTIVE 颜色）
                clip.rect_stroke(
                    vp,
                    0.0,
                    egui::Stroke::new(2.0, egui::Color32::from_black_alpha(90)),
                    egui::StrokeKind::Middle,
                );
                clip.rect_stroke(
                    vp,
                    0.0,
                    egui::Stroke::new(1.0, line),
                    egui::StrokeKind::Middle,
                );
                // 四角角标：`nw = w/4*PHI` 是**主画布**的显示像素长度，
                // 换算到模型再投到预览（源码就是在主画布坐标里裁的）。
                let ax = (cr.width() / 4.0 * PHI * sx / self.view.zoom).min(vp.width() * 0.5);
                let ay = (cr.height() / 4.0 * PHI * sy / self.view.zoom).min(vp.height() * 0.5);
                let (l, t, r, b) = (vp.left(), vp.top(), vp.right(), vp.bottom());
                let pts = [
                    ([l, t], [l + ax, t]),
                    ([l, t], [l, t + ay]),
                    ([r, t], [r - ax, t]),
                    ([r, t], [r, t + ay]),
                    ([l, b], [l + ax, b]),
                    ([l, b], [l, b - ay]),
                    ([r, b], [r - ax, b]),
                    ([r, b], [r, b - ay]),
                ];
                for (a, c) in pts {
                    clip.line_segment(
                        [egui::pos2(a[0], a[1]), egui::pos2(c[0], c[1])],
                        egui::Stroke::new(1.0, line),
                    );
                }
            }
        }

        // ---- 交互 ----
        let canvas_rect = self.canvas_rect;
        if resp.drag_started() {
            if let (Some(p), Some(cr), Some(vp)) =
                (resp.interact_pointer_pos(), canvas_rect, viewport_rect)
            {
                if vp.contains(p) {
                    let start_world = to_world(p);
                    let center = self.view.screen_to_world(cr.center().x, cr.center().y);
                    self.preview.drag = Some((start_world, center));
                }
            }
        }
        if resp.dragged() {
            if let (Some((start_world, start_center)), Some(p), Some(cr)) =
                (self.preview.drag, resp.interact_pointer_pos(), canvas_rect)
            {
                let now = to_world(p);
                let center = [
                    start_center[0] + now[0] - start_world[0],
                    start_center[1] + now[1] - start_world[1],
                ];
                self.preview_recenter(center[0], center[1], cr);
                ui.ctx().request_repaint();
            }
        }
        if resp.drag_stopped() {
            self.preview.drag = None;
        }
        if resp.clicked() && self.preview.drag.is_none() {
            if let (Some(p), Some(cr)) = (resp.interact_pointer_pos(), canvas_rect) {
                let w = to_world(p);
                self.preview_recenter(w[0], w[1], cr);
                ui.ctx().request_repaint();
            }
        }
        // 滚轮：缩放主视图，锚点 = 预览里指针对应的世界点（`previewwindow.py:382`）
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 1.0 {
                if let (Some(p), Some(_cr)) = (resp.hover_pos(), canvas_rect) {
                    let w = to_world(p);
                    let anchor = self.view.world_to_screen(w[0], w[1]);
                    let z = self.view.stepped_zoom(scroll > 0.0);
                    self.view.set_zoom_at(z, anchor[0], anchor[1]);
                    ui.ctx().request_repaint();
                }
            }
        }

        if ui.checkbox(&mut show, "Show Viewfinder").changed() {
            self.prefs.set_bool("preview.show_viewfinder", show);
        }
    }

    /// 把主视图中心挪到模型坐标 `(wx, wy)` —— `recenter_on_model_coords`。
    pub(crate) fn preview_recenter(&mut self, wx: f32, wy: f32, canvas: egui::Rect) {
        let c = canvas.center();
        self.view.offset = [c.x - wx * self.view.zoom, c.y - wy * self.view.zoom];
    }

    /// 画布（`TiledDrawWidget`）—— 工作区的中央，`expand=True`。
    ///
    /// 这一段是 M2 打通的渲染关键路径，S1 只是把它从"整窗口中央"
    /// 挪进工作区中央：视图矩阵用的是 `content_rect`（全窗口）而非本面板的
    /// `rect`，所以压边之后**偏移量必须跟着面板原点走** —— 见下面
    /// `view_window.offset` 里那个 `rect.min`。
    pub(crate) fn canvas(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        // 记下画布矩形：键盘缩放要用它算锚点（`handle_keys` 在画布之前跑，
        // 只能拿到上一帧的值 —— 与源码用"最后一次指针事件"同构）
        self.canvas_rect = Some(rect);

        // ---- 平移 ----
        let pan_delta = if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
        {
            response.drag_delta()
        } else {
            let space = ui.input(|i| i.key_down(egui::Key::Space));
            if space && response.dragged_by(egui::PointerButton::Primary) {
                response.drag_delta()
            } else {
                egui::Vec2::ZERO
            }
        };
        if pan_delta != egui::Vec2::ZERO {
            self.view.pan(pan_delta.x, pan_delta.y);
        }

        // ---- 缩放（以光标为锚点）----
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && response.hovered() {
            if let Some(pos) = response.hover_pos() {
                let local = pos - rect.min;
                let factor = (scroll * 0.002).exp();
                self.view.zoom_at(local.x, local.y, factor);
            }
        }

        // ---- 绘制 ----
        // B3-1：输入交给交互模式栈（`gui/mode.py`）。栈顶（默认 FreehandMode）
        // 自己排队、做压力/倾角插值，再由 `BrushworkModeMixin` 原子提交
        // （一次拖拽 = 一条 Brushwork，`lib/command.py:250`）。
        //
        // 文件弹层开着时禁掉画布交互（GTK 的模态对话框也是这个效果）；
        // 按住空格是"抓手"平移，不算绘画（`DragMode.key_press_cb` 的 space 分支
        // 在本移植里由 view 平移直接处理）。
        let space_down = ui.input(|i| i.key_down(egui::Key::Space));
        let modal = self.file_prompt.is_some();
        // 指针时间戳：用应用启动以来的毫秒（没有 GTK 事件时间可用）
        let now_ms = ui.input(|i| i.time) * 1000.0;
        let mods = self.pointer_modifiers(ui.ctx());
        if !modal && !space_down {
            // 本帧是否已经按"拖拽"语义喂过事件（见下面的 click 分支）
            let mut dispatched_drag = false;
            if response.drag_started_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                // 起笔：重置笔刷引擎状态（源码 `brush.reset()` + 新 stroke）
                self.stroke_state = pigment_engine::hokusai::BrushState::default();
                if let Some(pos) = response.interact_pointer_pos() {
                    let ev = self.pointer_event(pos, rect, now_ms, 1, mods);
                    self.mode_button_press(ev);
                }
                self.stroke_dirty = true;
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                if let Some(pos) = response.interact_pointer_pos() {
                    let ev = self.pointer_event(pos, rect, now_ms, 0, mods);
                    self.mode_motion(ev);
                }
                self.stroke_dirty = true;
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                if let Some(pos) = response.interact_pointer_pos() {
                    let ev = self.pointer_event(pos, rect, now_ms, 1, mods);
                    self.mode_button_release(ev);
                }
                // 颜色历史入栈时机对齐 MyPaint：`__input_stroke_ended_cb`
                // （gui/brushcolor.py:54）——每笔结束后记录，且橡皮不记录。
                if !self.eraser {
                    self.color_manager.push_current_history();
                }
                // 笔刷历史同理（`brushmanager.py:1064`），由自由手绘模式发出。
                self.record_brush_history();
            }
            // 单击（按下即松开、指针几乎没动）：
            //
            // egui 的 `Sense::click_and_drag` 会先观望"这是点击还是拖拽"
            // （`Response::drag_started` 的文档：同时感知 click 时只有指针移动
            // 一小段之后才算 drag），所以一次干脆的点击**不会**走上面的
            // drag_started/drag_stopped。但 MyPaint 的 GTK 事件流里
            // ButtonPress/ButtonRelease 一定成对到达每一个模式
            // （`gui/mode.py` 的 `button_press_cb`/`button_release_cb`），
            // 填充工具的单击就是一粒种子 + 一次填充（`gui/fill.py:161-178`）。
            // 因此这里把"被判定为点击"的那一次补成一对 press/release，
            // 否则单击填充永远不会触发。
            //
            // 上面的拖拽分支与 `clicked_by` 互斥（egui 在判定为拖拽后不置
            // CLICKED），`dispatched_drag` 只是防御性双保险。
            if !dispatched_drag && response.clicked_by(egui::PointerButton::Primary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.stroke_state = pigment_engine::hokusai::BrushState::default();
                    let press = self.pointer_event(pos, rect, now_ms, 1, mods);
                    self.mode_button_press(press);
                    let release = self.pointer_event(pos, rect, now_ms, 1, mods);
                    self.mode_button_release(release);
                    self.stroke_dirty = true;
                    if !self.eraser {
                        self.color_manager.push_current_history();
                    }
                    self.record_brush_history();
                }
            }
        }

        // ---- 同步显存并绘制合成结果（与暂存区画布共用 prepare_canvas）----
        let tiles = prepare_canvas(
            ui.ctx(),
            &self.renderer,
            self.doc.layers(),
            &self.view,
            rect,
            self.force_resync,
        );
        self.force_resync = false;
        self.pending_uploads = tiles.uploads.len();

        ui.painter()
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                crate::gpu::CanvasPaint {
                    renderer: std::sync::Arc::clone(&self.renderer),
                    view_matrix: tiles.view_matrix,
                    uploads: tiles.uploads,
                    evict: tiles.evict,
                    visible: tiles.visible,
                },
            ));

        // ---- 覆盖层（B3-5）：画框 + 对称辅助线 ----
        // 源码在 `TiledDrawWidget` 里先画画布、再逐个 `overlay.paint(cr)`
        // （`tileddrawwidget.py:1150-1156`，挂载顺序见 `drawwindow.py:157-158`）。
        // 这里的 wgpu callback 就是"画布"，覆盖层排在它之后加进同一帧的
        // 绘制列表，于是自然压在 tile 之上。
        //
        // 裁剪到画布矩形：画框/轴线在视口外还有几何，不裁会画到侧栏上
        // （源码的 cairo 上下文本来就被裁剪到 TDW 的分配区域）。
        crate::shell::overlays::paint_canvas_overlays(
            &ui.painter().with_clip_rect(rect),
            rect,
            &self.view,
            &self.doc,
            crate::shell::overlays::OverlayStyle::from_prefs(&self.prefs),
        );

        // ---- 笔刷光标预览：放在 callback 之后，保证压在 tile 之上可见 ----
        if let Some(pos) = response.hover_pos() {
            let r = self
                .brush
                .get(BrushSetting::Radius)
                .base_value
                .exp()
                .max(0.5)
                * self.view.zoom;
            let color = if self.eraser {
                egui::Color32::from_rgb(200, 80, 80)
            } else {
                egui::Color32::from_gray(110)
            };
            ui.painter()
                .circle_stroke(pos, r.max(1.0), egui::Stroke::new(1.0, color));
        }

        // 绘制中持续重绘（dab 沿笔画插值）；上传有欠账（命中每帧上限）时也要继续，
        // 否则剩下的 tile 会一直停在旧状态
        let backlog = self.pending_uploads >= crate::gpu::MAX_UPLOADS_PER_FRAME;
        if self.stroke_dirty || backlog {
            self.stroke_dirty = false;
            ui.ctx().request_repaint();
        }
    }
}
