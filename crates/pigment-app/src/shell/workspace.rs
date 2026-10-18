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
use pigment_engine::color::UIColor;

use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::{Brush, BrushSetting};
use pigment_surface::LayerStack;

use crate::app::{PigmentApp, ViewTool};
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
    if value_slider(ui, label, &mut value, range) {
        brush.get_mut(setting).base_value = value;
        true
    } else {
        false
    }
}

/// 一行 `label + Slider`（不绑定笔刷设置，用于有效半径这类派生值）。
fn value_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::Slider::new(value, range).show_value(true))
    })
    .inner
    .changed()
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
        rotation: view.rotation,
        mirrored: view.mirrored,
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
        // 背景不可见时每个可见 tile 都要重传（空 tile 也要传成全透明），
        // 否则显存里旧的纸纹 tile 会把棋盘格盖住。
        let transparent = !layers.render_is_opaque();
        let plan = crate::gpu::plan_uploads(
            |c| renderer.uploaded_rev(c),
            &visible,
            crate::gpu::MAX_UPLOADS_PER_FRAME,
            // 纯色背景由面板底色兜底，不必上传；**图案背景**必须上传合成
            // tile，否则整块画布显示的是平均色而不是纸纹。
            |c| transparent || layers.has_paint_at(c.0, c.1) || layers.background_is_image(),
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
            |c| {
                !transparent
                    && (layers.has_paint_at(c.0, c.1) || layers.background_is_image())
            },
        )
    };
    // ---- 视图矩阵：world px → **本回调 viewport** 的裁剪空间 ----
    // egui 的 `PaintCallbackInfo.viewport` 就是回调矩形：NDC 的 [-1,1] 映射到
    // 这块矩形，而不是整窗。所以矩阵用 `rect` 的尺寸与 canvas-local offset，
    // **不能**用 `content_rect` + `rect.min`（那会把画面按整窗缩放/平移，
    // 表现为只有中间一条带子被绘制、笔画长度/位置失真）。
    let view_window = View {
        zoom: view.zoom,
        offset: view.offset,
        rotation: view.rotation,
        mirrored: view.mirrored,
    };
    CanvasTiles {
        uploads,
        evict,
        visible,
        view_matrix: view_window.clip_matrix(rect.width(), rect.height()),
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
            .id_salt("brush_editor_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                        // ---- 笔刷信息 ----
                        if let Some(e) = self.brushes.selected_entry().cloned() {
                            ui.horizontal(|ui| {
                                // 主标题用 MyPaint 的展示名（笔刷名），description 退居 tooltip
                                ui.label(egui::RichText::new(e.mypaint_name()).heading())
                                    .on_hover_text(if e.display.is_empty() {
                                        e.key.clone()
                                    } else {
                                        format!("{}\n{}", e.mypaint_name(), e.display)
                                    });
                                ui.label(egui::RichText::new(&e.group_display).weak().small());
                                if ui
                                    .button("Edit Icon…")
                                    .on_hover_text("编辑 128×128 笔刷预览图标")
                                    .clicked()
                                {
                                    self.open_icon_editor();
                                }
                                if ui
                                    .button("New Brush")
                                    .on_hover_text(
                                        "把当前设置另存为新笔刷（放进 New 组，brusheditor.py:664）",
                                    )
                                    .clicked()
                                {
                                    self.new_brush_from_current();
                                }
                                if ui
                                    .button("Save Brush")
                                    .on_hover_text(
                                        "写回用户目录 .myb + _prev.png（ManagedBrush.save）",
                                    )
                                    .clicked()
                                {
                                    self.save_current_brush_to_disk();
                                }
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
                                // About 元数据直接编辑笔刷库条目（`description`/`notes`）。
                                // `self.brushes`、`self.editor`、`self.base_brush` 是
                                // 三个不相交字段，可以同时可变借用。
                                let about = self
                                    .brushes
                                    .library
                                    .entries
                                    .get_mut(sel)
                                    .map(|e| (&mut e.display, &mut e.notes));
                                let edited = self.editor.show(
                                    ui,
                                    &mut self.base_brush,
                                    Some(&icon),
                                    about,
                                );
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
        self.icon_editor_window(ui.ctx());
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
    /// 未移植：`LineModeOptionsWidget` 的压力-距离曲线、对称的选项页。
    /// 填充选项（`gui/fill.py:433` 的 `FloodFillOptionsWidget`）。
    ///
    /// 分类照源码：Source（Sample Merged / Limit to View）、Target
    /// （New Layer (once) / Blend Mode / Opacity）、形态学（Offset / Feather）、
    /// Gap Detection（未移植，只留说明）。
    fn fill_options_panel(&mut self, ui: &mut egui::Ui) {
        use pigment_surface::combine::CombineMode;
        let Some(mut opt) = self.modes.top().fill_options() else {
            return;
        };
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Tolerance:");
            changed |= ui
                .add(egui::Slider::new(&mut opt.tolerance, 0.0..=1.0).show_value(true))
                .changed();
        });
        ui.separator();
        ui.label(egui::RichText::new("Source").strong());
        // 取样源图层下拉 —— `gui/fill.py:560-603` 的 `FlatLayerList` 组合框。
        //
        // 默认项 "Selected Layer"（`src_path=None`，填当前层）；其余是整棵
        // 图层树的扁平列表，组和子层按 `walk()` 次序排列、按层级缩进。源码把
        // 组画成斜体、用分隔行隔开默认项；egui 的 ComboBox 没有分隔行，
        // 默认项直接放在列表最前（技术栈适配，见模块记账）。
        let layers: Vec<(Vec<usize>, u64, String, bool)> = self
            .doc
            .layers()
            .walk()
            .into_iter()
            .map(|(path, layer)| {
                let is_group = layer.is_group();
                let name = layer.name.clone().unwrap_or_else(|| "Layer".to_string());
                (path, layer.id, name, is_group)
            })
            .collect();
        // 源层被删掉后回到默认项（源码 `_src_combo_changed_cb` 同样复位）
        if let Some(id) = opt.src_layer {
            if !layers.iter().any(|(_, lid, _, _)| *lid == id) {
                opt.src_layer = None;
                changed = true;
            }
        }
        let selected_text = match opt.src_layer {
            None => "Selected Layer".to_string(),
            Some(id) => layers
                .iter()
                .find(|(_, lid, _, _)| *lid == id)
                .map(|(path, _, name, _)| format!("{}{name}", "  ".repeat(path.len() - 1)))
                .unwrap_or_else(|| "Selected Layer".to_string()),
        };
        ui.add_enabled_ui(!opt.sample_merged, |ui| {
            let combo = egui::ComboBox::from_id_salt("fill_source_layer")
                .selected_text(selected_text)
                .width(220.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(opt.src_layer.is_none(), "Selected Layer")
                        .clicked()
                    {
                        opt.src_layer = None;
                        changed = true;
                    }
                    for (path, id, name, is_group) in &layers {
                        let indent = "  ".repeat(path.len() - 1);
                        let text = if *is_group {
                            egui::RichText::new(format!("{indent}{name}")).italics()
                        } else {
                            egui::RichText::new(format!("{indent}{name}"))
                        };
                        if ui.selectable_label(opt.src_layer == Some(*id), text).clicked() {
                            opt.src_layer = Some(*id);
                            changed = true;
                        }
                    }
                });
            combo.response.on_hover_text(
                "选择填充取样所依据的图层\n（默认 Selected Layer = 当前层；勾了 Sample Merged 时不可用）",
            );
        });
        changed |= ui
            .checkbox(&mut opt.sample_merged, "Sample Merged")
            .on_hover_text("考虑填充区域时使用所有可见层的临时合成结果")
            .changed();
        changed |= ui
            .checkbox(&mut opt.limit_to_view, "Limit to View")
            .on_hover_text("把填充限制在当前视口内；旋转视图时取能装下视口的最小画布对齐矩形")
            .changed();
        ui.separator();
        ui.label(egui::RichText::new("Target").strong());
        changed |= ui
            .checkbox(&mut opt.make_new_layer, "New Layer (once)")
            .on_hover_text("把这次填充的结果放到当前层上方的新层；用一次后自动复位")
            .changed();
        egui::ComboBox::from_label("Blend Mode")
            .selected_text(opt.comp_mode.label())
            .show_ui(ui, |ui| {
                for mode in CombineMode::ALL {
                    if ui
                        .selectable_label(opt.comp_mode == mode, mode.label())
                        .clicked()
                    {
                        opt.comp_mode = mode;
                        changed = true;
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label("Opacity:");
            changed |= ui
                .add(egui::Slider::new(&mut opt.opacity, 0.0..=1.0).show_value(true))
                .changed();
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Offset:");
            changed |= ui
                .add(egui::Slider::new(&mut opt.offset, -64..=64).show_value(true))
                .on_hover_text("正值膨胀、负值腐蚀（像素）")
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Feather:");
            changed |= ui
                .add(egui::Slider::new(&mut opt.feather, 0..=64).show_value(true))
                .on_hover_text("填充边缘的高斯模糊半径（像素）")
                .changed();
        });
        ui.separator();
        changed |= ui
            .checkbox(&mut opt.gap_closing, "Use Gap Detection")
            .on_hover_text("检测线稿缺口并避免填充漏过去；比普通填充慢很多")
            .changed();
        ui.add_enabled_ui(opt.gap_closing, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut opt.gap_size, 1..=64).text("Max Gap Size"))
                .changed();
            changed |= ui
                .checkbox(&mut opt.retract_seeps, "Retract Seeps")
                .on_hover_text("从渗漏边缘往回擦，填到缺口前停下")
                .changed();
        });
        if changed {
            self.with_mode(|modes, _ctx| modes.top_mut().set_fill_options(opt.clone()));
            // 源码各回调只写 preferences dict，落盘由 app 关闭时统一做；
            // 这里立即落盘，避免异常退出丢设置。
            self.prefs
                .set_f64("flood_fill.tolerance", opt.tolerance as f64);
            self.prefs
                .set_bool("flood_fill.limit_to_view", opt.limit_to_view);
            self.prefs
                .set_bool("flood_fill.sample_merged", opt.sample_merged);
            self.prefs
                .set_f64("flood_fill.offset", opt.offset as f64);
            self.prefs
                .set_f64("flood_fill.feather", opt.feather as f64);
            self.prefs
                .set_f64("flood_fill.opacity", opt.opacity as f64);
            self.prefs
                .set_bool("flood_fill.gap_closing", opt.gap_closing);
            self.prefs
                .set_f64("flood_fill.gap_size", opt.gap_size as f64);
            self.prefs
                .set_bool("flood_fill.retract_seeps", opt.retract_seeps);
            self.prefs.set_f64(
                "flood_fill.blend_mode",
                CombineMode::ALL
                    .iter()
                    .position(|m| *m == opt.comp_mode)
                    .unwrap_or(0) as f64,
            );
            let _ = self.prefs.save();
            ui.ctx().request_repaint();
        }
    }

    /// 对称轴编辑选项（`symmetry.py:346-545` 的 `SymmetryEditOptionsWidget`）。
    fn symmetry_options(&mut self, ui: &mut egui::Ui) {
        use pigment_surface::symmetry::SymmetryType;
        let mut active = self.doc.symmetry_active();
        if ui.checkbox(&mut active, "Enabled").changed() {
            self.doc.set_symmetry_state(Some(active), None, None, None, None);
        }
        let (cx, cy) = self.doc.symmetry_center();
        let (mut x, mut y) = (cx, cy);
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("X Position:");
            changed |= ui.add(egui::DragValue::new(&mut x)).changed();
            ui.label("Y Position:");
            changed |= ui.add(egui::DragValue::new(&mut y)).changed();
        });
        if changed {
            self.doc.set_symmetry_state(None, Some((x, y)), None, None, None);
        }
        let mut angle = self.doc.symmetry_angle().to_degrees();
        if ui
            .add(egui::Slider::new(&mut angle, -180.0..=180.0).text("Angle"))
            .changed()
        {
            self.doc
                .set_symmetry_state(None, None, None, None, Some(angle.to_radians()));
        }
        let mut alpha = self
            .prefs
            .f64_or("symmetry.line_alpha", 0.333) as f32;
        if ui
            .add(egui::Slider::new(&mut alpha, 0.0..=1.0).text("Alpha"))
            .changed()
        {
            self.prefs
                .set_f64("symmetry.line_alpha", alpha as f64);
        }
        let mut kind = self.doc.symmetry_type();
        egui::ComboBox::from_label("Symmetry Type")
            .selected_text(symmetry_label(kind))
            .show_ui(ui, |ui| {
                for t in SymmetryType::ALL {
                    if ui.selectable_label(kind == t, symmetry_label(t)).clicked() {
                        kind = t;
                    }
                }
            });
        if kind != self.doc.symmetry_type() {
            self.doc.set_symmetry_state(None, None, Some(kind), None, None);
        }
        let mut lines = self.doc.symmetry_lines();
        if ui
            .add(egui::Slider::new(&mut lines, 1..=16).text("Rotational lines"))
            .changed()
        {
            self.doc
                .set_symmetry_state(None, None, None, Some(lines), None);
        }
    }

    /// 画框编辑选项（`framewindow.py:415-560` 的 `FrameEditOptionsWidget`）。
    fn frame_options(&mut self, ui: &mut egui::Ui) {
        let mut enabled = self.doc.get_frame_enabled();
        if ui.checkbox(&mut enabled, "Enabled").changed() {
            self.doc.set_frame_enabled(enabled);
        }
        let (fx, fy, mut w, mut h) = self.doc.get_frame();
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Width:");
            changed |= ui.add(egui::DragValue::new(&mut w).range(1..=32000)).changed();
            ui.label("Height:");
            changed |= ui.add(egui::DragValue::new(&mut h).range(1..=32000)).changed();
        });
        if changed {
            self.doc.set_frame(fx, fy, w, h);
        }
        let mut dpi = self.doc.xres.unwrap_or(96) as i32;
        if ui
            .horizontal(|ui| {
                ui.label("Resolution:");
                ui.add(egui::DragValue::new(&mut dpi).range(1..=9600)).changed()
            })
            .inner
        {
            self.doc.xres = Some(dpi.max(1) as u32);
            self.doc.yres = Some(dpi.max(1) as u32);
        }
        let rgba = crate::shell::overlays::OverlayStyle::from_prefs(&self.prefs).frame_rgba;
        let mut rgb = [
            (rgba[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgba[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgba[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
        ui.horizontal(|ui| {
            ui.label("Color:");
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                self.prefs.set_f64_array(
                    "frame.color_rgba",
                    &[
                        rgb[0] as f64 / 255.0,
                        rgb[1] as f64 / 255.0,
                        rgb[2] as f64 / 255.0,
                        rgba[3] as f64,
                    ],
                );
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Set Frame to Layer").clicked() {
                let (x, y, w, h) = self.doc.current_layer().data_bbox();
                self.doc.set_frame(x, y, w, h);
            }
            if ui.button("Set Frame to Document").clicked() {
                let (x, y, w, h) = self.doc.data_bbox();
                self.doc.set_frame(x, y, w, h);
            }
        });
    }

    /// Inking 的 `OptionsUI`（`gui/inktool.py:1259-1380`）：节点压力/时间/倾角
    /// 滑条 + 插点/删点/简化/剔除。源码是 GTK 的 `point_values_grid`，这里用
    /// egui 的同一组控件。
    fn inking_options(&mut self, ui: &mut egui::Ui) {
        use pigment_engine::mode::ink::InkNodeEdit;
        if !self.modes.top().ink_editing() {
            ui.label(
                egui::RichText::new("Draw a line, then adjust its points")
                    .italics()
                    .weak(),
            );
            return;
        }
        let current = self.modes.top().ink_current_node();
        let len = self.modes.top().ink_nodes().map(|n| n.len()).unwrap_or(0);
        let current = current.filter(|i| *i < len);
        let mut edit = None;
        if let Some(i) = current {
            let node = self.modes.top().ink_nodes().unwrap()[i];
            ui.label(format!("Point {} of {}", i + 1, len));
            let mut pressure = node.pressure;
            if ui
                .add(egui::Slider::new(&mut pressure, 0.0..=1.0).text("Pressure"))
                .changed()
            {
                edit = Some(InkNodeEdit::Pressure(i, pressure));
            }
            let mut xtilt = node.xtilt;
            if ui
                .add(egui::Slider::new(&mut xtilt, -1.0..=1.0).text("X tilt"))
                .changed()
            {
                edit = Some(InkNodeEdit::Tilt(i, xtilt, node.ytilt));
            }
            let mut ytilt = node.ytilt;
            if ui
                .add(egui::Slider::new(&mut ytilt, -1.0..=1.0).text("Y tilt"))
                .changed()
            {
                edit = Some(InkNodeEdit::Tilt(i, node.xtilt, ytilt));
            }
            if i > 0 {
                let dtime = self.with_mode(|modes, _ctx| match modes.top().ink_nodes() {
                    Some(nodes) if i < nodes.len() => {
                        (nodes[i].time - nodes[i - 1].time).max(1.0 / 200.0)
                    }
                    _ => 0.005,
                });
                let mut dtime = dtime;
                if ui
                    .add(egui::Slider::new(&mut dtime, 0.005..=1.0).text("Time"))
                    .changed()
                {
                    edit = Some(InkNodeEdit::Time(i, dtime));
                }
            } else {
                ui.add_enabled(false, egui::Slider::new(&mut 0.0, 0.0..=1.0).text("Time"));
            }
        } else {
            ui.label(egui::RichText::new("No selected point").italics().weak());
        }

        ui.separator();
        ui.horizontal(|ui| {
            let can_insert = current.map(|i| i + 1 < len).unwrap_or(false);
            if ui
                .add_enabled(can_insert, egui::Button::new("Insert Point"))
                .clicked()
            {
                edit = Some(InkNodeEdit::Insert(current.unwrap()));
            }
            let can_delete = current.map(|i| i > 0 && i + 1 < len).unwrap_or(false);
            if ui
                .add_enabled(can_delete, egui::Button::new("Delete Point"))
                .clicked()
            {
                edit = Some(InkNodeEdit::Delete(current.unwrap()));
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(len > 3, egui::Button::new("Simplify Points"))
                .clicked()
            {
                let n = self.with_mode(|modes, ctx| modes.top_mut().simplify_nodes(ctx));
                self.set_status(format!("简化掉 {n} 个点"), ui.input(|i| i.time));
            }
            if ui
                .add_enabled(len > 2, egui::Button::new("Cull Points"))
                .clicked()
            {
                let n = self.with_mode(|modes, ctx| modes.top_mut().cull_nodes(ctx));
                self.set_status(format!("剔除 {n} 个点"), ui.input(|i| i.time));
            }
        });

        if let Some(edit) = edit {
            self.with_mode(|modes, ctx| {
                modes.top_mut().ink_apply(ctx, edit);
            });
            ui.ctx().request_repaint();
        }
    }

    pub(crate) fn mode_options_panel_content(&mut self, ui: &mut egui::Ui) {
        let mode_name = self.modes.top().name().to_string();
        // 选项控件来自哪个模式：`BrushResizeMode.get_options_widget()` 直接
        // 返回上一个模式的控件（`gui/brushmanip.py:296-297`），所以这里用
        // `options_mode()` 而不是栈顶。标题/图标仍按源码：标题是当前模式名
        // （`get_name()`），图标沿用上一个模式（`get_icon_name()`，`:192-193`）。
        let options_mode = self.modes.options_mode();
        let options_name = options_mode.name().to_string();
        let action = options_mode.action_name();
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

        if options_name == "Flood Fill" {
            self.fill_options_panel(ui);
            return;
        }
        if options_name == "Inking" {
            self.inking_options(ui);
            return;
        }
        if options_name == "Edit Symmetry Axis" {
            self.symmetry_options(ui);
            return;
        }
        if options_name == "Edit Frame" {
            self.frame_options(ui);
            return;
        }

        let painting = matches!(
            options_name.as_str(),
            "Freehand Drawing" | "Lines and Curves" | "Connected Lines" | "Ellipses and Circles"
        );
        if !painting {
            ui.label(egui::RichText::new("No options available").italics().weak());
            return;
        }

        // ---- 公共设置（mode.py:591-601）----
        let mut changed = false;
        // Size 滑条绑定**当前有效半径**（`app.brush_adjustment["radius_logarithmic"]`，
        // `document.py:1571-1590`）：`BrushResizeMode` 与 `f`/`d` 都是改这个值。
        // 本应用把"笔刷被改过尺寸"记在 `radius_override` 上，所以这里写覆盖值，
        // 而不是 base_brush —— 否则 resize 之后滑条会失效（被覆盖值盖回去）。
        let mut effective_radius = self.brush.get(BrushSetting::Radius).base_value;
        if value_slider(ui, "Size:", &mut effective_radius, -2.0..=6.0) {
            self.radius_override = Some(effective_radius);
            changed = true;
        }
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
        if options_name == "Freehand Drawing" {
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

        // ---- 直线/曲线压力（`LineModeOptionsWidget`，linemode.py:39-85/814）----
        if matches!(
            options_name.as_str(),
            "Lines and Curves" | "Connected Lines" | "Ellipses and Circles"
        ) {
            ui.separator();
            ui.label(egui::RichText::new("Line pressures").strong());
            let mut s = self.line_settings;
            let mut line_changed = false;
            line_changed |= value_slider(
                ui,
                "Entrance Pressure:",
                &mut s.entry_pressure,
                0.0001..=1.0,
            );
            line_changed |= value_slider(
                ui,
                "Midpoint Pressure:",
                &mut s.midpoint_pressure,
                0.0001..=1.0,
            );
            line_changed |= value_slider(
                ui,
                "Exit Pressure:",
                &mut s.exit_pressure,
                0.0001..=1.0,
            );
            line_changed |= value_slider(ui, "Head:", &mut s.head, 0.0001..=1.0);
            line_changed |= value_slider(ui, "Tail:", &mut s.tail, 0.0001..=1.0);
            if line_changed {
                self.line_settings = s;
                self.prefs.set_f64("linemode.entry_pressure", s.entry_pressure as f64);
                self.prefs.set_f64("linemode.midpoint_pressure", s.midpoint_pressure as f64);
                self.prefs.set_f64("linemode.exit_pressure", s.exit_pressure as f64);
                self.prefs.set_f64("linemode.line_head", s.head as f64);
                self.prefs.set_f64("linemode.line_tail", s.tail as f64);
                self.modes.set_line_settings(s);
                self.scratch.modes.set_line_settings(s);
                changed = true;
            }
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
                // 源码重置的是 `brush_adjustment`（有效值）；`radius_override`
                // 就是它的对应物，所以一并清掉。
                self.radius_override = None;
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

    /// 执行一条 `input.button_mapping` 动作。
    ///
    /// 源码走 `get_handler_object()` 分三类（`gui/buttonmap.py:98-125`）：
    /// 模式类押栈、popup state、普通 GtkAction。这里：
    /// - 三个视图操纵模式要记住"是哪个鼠标键起手"（拖拽期间持续按它）；
    /// - `FrameEditMode` / `BrushResizeMode` 需要在按下点立刻转发一次 press；
    /// - 其余（含 `Undo`/`Bigger`/`FreehandMode`/`ColorPickMode` 等）直接走
    ///   [`PigmentApp::run_action`]，与菜单/加速键同一条路径。
    ///
    /// 返回是否真的消费了这次按钮（未实现的动作返回 `false`）。
    #[allow(clippy::too_many_arguments)]
    fn dispatch_mapped_button(
        &mut self,
        action: &str,
        btn: egui::PointerButton,
        _gdk_button: u32,
        pos: egui::Pos2,
        rect: egui::Rect,
        now_ms: f64,
        mods: pigment_engine::mode::Modifiers,
        ctx: &egui::Context,
    ) -> bool {
        match action {
            "PanViewMode" | "ZoomViewMode" | "RotateViewMode" => {
                let tool = match action {
                    "PanViewMode" => ViewTool::Pan,
                    "ZoomViewMode" => ViewTool::Zoom,
                    _ => ViewTool::Rotate,
                };
                self.view_tool = Some(tool);
                self.view_drag_button = btn;
                self.view_drag_prev = None;
                true
            }
            "ShowPopupMenu" => {
                self.popup_menu = Some(pos);
                self.popup_opened = now_ms / 1000.0;
                true
            }
            "FrameEditMode" => {
                let mode: Box<dyn pigment_engine::mode::InteractionMode> =
                    Box::new(pigment_engine::mode::frameedit::FrameEditMode::new());
                self.with_mode(|modes, mctx| modes.push(mode, mctx));
                self.mapped_mode_button = Some(btn);
                let ev = self.pointer_event(pos, rect, now_ms, 1, mods);
                self.mode_button_press(ev);
                true
            }
            "BrushResizeMode" => {
                self.begin_brush_resize(now_ms / 1000.0);
                true
            }
            _ => self.run_action(action, ctx, now_ms / 1000.0),
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

        // ---- 滚轮（`ScrollableModeMixin.scroll_cb`）----
        // 默认缩放、Ctrl/Alt 反转平移；Shift 走阶梯/离散步进。
        if response.hovered() {
            // `ui.support_smooth_scrolling`（`preferenceswindow.py:400-402`）：
            // 关掉时源码按"离散步进"处理滚轮（`tileddrawwidget` 的事件分类），
            // egui 侧对应 `raw_scroll_delta`（一格 50 点）而不是平滑增量。
            let smooth = self.prefs.bool_or("ui.support_smooth_scrolling", true);
            let (delta, imods) = ui.input(|i| {
                (
                    if smooth {
                        i.smooth_scroll_delta
                    } else {
                        stepwise_wheel(&i.raw.events, i.content_rect().height())
                    },
                    i.modifiers,
                )
            });
            if delta != egui::Vec2::ZERO {
                if let Some(pos) = response.hover_pos() {
                    self.apply_canvas_wheel(rect, pos, imods, delta);
                }
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

        // ---- 指针按钮映射（`input.button_mapping`）----
        // 默认 Windows：中键弹菜单、右键平移；Shift 旋转、Ctrl/Alt 缩放、
        // Ctrl/Alt+Shift 进 FrameEditMode（`userconfig.py:127-139`）。
        let imods = ui.input(|i| i.modifiers);
        for (btn, gdk_button) in [
            (egui::PointerButton::Middle, 2u32),
            (egui::PointerButton::Secondary, 3u32),
        ] {
            let started = response.drag_started_by(btn) || response.clicked_by(btn);
            if !started {
                continue;
            }
            let Some(action) = self.mapped_button_action(
                gdk_button,
                imods.ctrl,
                imods.shift,
                imods.alt,
            ) else {
                continue;
            };
            if let Some(pos) = response.interact_pointer_pos() {
                self.dispatch_mapped_button(
                    &action,
                    btn,
                    gdk_button,
                    pos,
                    rect,
                    now_ms,
                    mods,
                    ui.ctx(),
                );
            }
        }

        // ---- 主键 + 修饰键（源码允许 `<Control>Button1` 这种绑定）----
        // 无修饰键的 Button1 固定给绘画，不查表（`buttonmap.py:601-614` 的
        // 编辑器同样拒绝它）。命中后整段按住的时期都不喂绘画模式。
        let primary_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
        let primary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
        if primary_pressed
            && response.hovered()
            && (imods.ctrl || imods.shift || imods.alt)
        {
            if let Some(action) = self.mapped_button_action(1, imods.ctrl, imods.shift, imods.alt)
            {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.primary_action_active = true;
                    self.dispatch_mapped_button(
                        &action,
                        egui::PointerButton::Primary,
                        1,
                        pos,
                        rect,
                        now_ms,
                        mods,
                        ui.ctx(),
                    );
                }
            }
        }

        // 空格 + 左键 = 临时抓手平移（源码把 space 当假 Button2）。
        if space_down && response.dragged_by(egui::PointerButton::Primary) {
            let d = response.drag_delta();
            self.view.pan(d.x, d.y);
        }

        // 底栏上下文取色：按钮按下后进入"下一次画布点击执行 PickContext"。
        if self.pick_context_pending && primary_pressed && response.hovered() {
            self.pick_context_pending = false;
            self.primary_action_active = true;
            self.pick_context(ui.ctx(), now_ms / 1000.0);
        }

        // ---- 视图操纵工具（`gui/viewmanip.py` 的三个 Oneshot）----
        // 源码是压栈模式，拖拽结束后 pop 回原工具；这里用一次性 view_tool 状态。
        let view_gesture = self.view_tool.is_some();
        if view_gesture {
            let btn = self.view_drag_button;
            if response.drag_started_by(btn) {
                self.view_drag_prev = response.interact_pointer_pos().map(|p| egui::pos2(p.x - rect.min.x, p.y - rect.min.y));
            }
            if response.dragged_by(btn) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let local = egui::pos2(pos.x - rect.min.x, pos.y - rect.min.y);
                    if let Some(prev) = self.view_drag_prev {
                        let c = egui::vec2(rect.width() * 0.5, rect.height() * 0.5);
                        if let Some(tool) = self.view_tool {
                            PigmentApp::apply_view_drag(&mut self.view, tool, prev, local, c);
                        }
                    }
                    self.view_drag_prev = Some(local);
                }
            }
            if response.drag_stopped_by(btn) || response.clicked_by(btn) {
                self.view_tool = None;
                self.view_drag_prev = None;
                self.view_drag_button = egui::PointerButton::Primary;
                let top = self.modes.top();
                let text = format!("{}: {}", top.name(), top.usage());
                self.set_status(text, now_ms / 1000.0);
            }
        }

        // ---- 按钮映射压栈的临时模式（FrameEditMode）----
        let mut mapped_mode_active = false;
        if let Some(btn) = self.mapped_mode_button {
            mapped_mode_active = true;
            let down = ui.input(|i| i.pointer.button_down(btn));
            if down {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let ev = self.pointer_event(pos, rect, now_ms, 0, mods);
                    self.mode_motion(ev);
                }
            } else {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let ev = self.pointer_event(pos, rect, now_ms, 1, mods);
                    self.mode_button_release(ev);
                }
                self.mapped_mode_button = None;
                self.finish_oneshot(mods, now_ms / 1000.0);
                ui.ctx().request_repaint();
            }
        }

        if !modal
            && !space_down
            && !view_gesture
            && !mapped_mode_active
            && !self.primary_action_active
            && self.tablet_allows_painting()
        {
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
                    // Oneshot（取色）拖完按修饰键规则回原工具（mode.py:1302-1317）
                    self.finish_oneshot(mods, now_ms / 1000.0);
                    // 出栈可能让覆盖层（笔刷尺寸圆圈）消失；请求一帧重绘，
                    // 否则窗口会停在"还带着圆圈"的上一帧，直到下次输入。
                    ui.ctx().request_repaint();
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
                    // 对称/画框编辑模式的垃圾桶按钮（`symmetry.py:736-740`、
                    // `framewindow.py:182-187`）优先于绘画。
                    let mut consumed = false;
                    match self.modes.top().action_name() {
                        Some("SymmetryEditMode") => {
                            let d = crate::shell::overlays::symmetry_disable_pos(
                                rect,
                                &self.view,
                                self.doc.symmetry_center(),
                            );
                            if pos.distance(d) <= 16.0 {
                                self.doc.set_symmetry_state(Some(false), None, None, None, None);
                                consumed = true;
                            }
                        }
                        Some("FrameEditMode") => {
                            let d = crate::shell::overlays::frame_remove_pos(rect);
                            if pos.distance(d) <= 16.0 {
                                self.doc.set_frame_enabled(false);
                                consumed = true;
                            }
                        }
                        _ => {}
                    }
                    // Inking 画布 OK/垃圾桶优先于绘画（`inktool.py:317-356`）
                    if let Some(accept) = self.ink_button_at(rect, pos) {
                        consumed = self.with_mode(|modes, ctx| {
                            if accept {
                                modes.top_mut().ink_accept(ctx)
                            } else {
                                modes.top_mut().ink_reject(ctx)
                            }
                        });
                        if consumed {
                            let top = self.modes.top();
                            let text = format!("{}: {}", top.name(), top.usage());
                            self.set_status(text, now_ms / 1000.0);
                            self.stroke_dirty = true;
                        }
                    }
                    if !consumed {
                        self.stroke_state = pigment_engine::hokusai::BrushState::default();
                        let press = self.pointer_event(pos, rect, now_ms, 1, mods);
                        self.mode_button_press(press);
                        let release = self.pointer_event(pos, rect, now_ms, 1, mods);
                        self.mode_button_release(release);
                        self.finish_oneshot(mods, now_ms / 1000.0);
                        self.stroke_dirty = true;
                        if !self.eraser {
                            self.color_manager.push_current_history();
                        }
                        self.record_brush_history();
                    }
                }
            }
            // 无按键悬停：Inking ADJUST 的节点预亮需要它
            // （`inktool.py:356-362` 的 `motion_notify_cb`）。
            if response.hovered() && !ui.input(|i| i.pointer.any_down()) {
                if let Some(pos) = response.hover_pos() {
                    let ev = self.pointer_event(pos, rect, now_ms, 0, mods);
                    self.mode_motion(ev);
                }
            }
        }

        // 主键映射手势在**松开这一帧的输入处理之后**才解锁：否则
        // `drag_stopped_by`/`clicked_by` 会把同一次按键再当绘画喂一遍。
        if !primary_down {
            self.primary_action_active = false;
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
            .add(egui_wgpu::Callback::new_paint_callback(
                rect,
                crate::gpu::CanvasPaint {
                    renderer: std::sync::Arc::clone(&self.renderer),
                    view_matrix: tiles.view_matrix,
                    uploads: tiles.uploads,
                    evict: tiles.evict,
                    visible: tiles.visible,
                    checks: !self.doc.layers().render_is_opaque()
                        && self.prefs.bool_or("view.real_alpha_checks", true),
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
        let overlay_style = crate::shell::overlays::OverlayStyle::from_prefs(&self.prefs);
        crate::shell::overlays::paint_canvas_overlays(
            &ui.painter().with_clip_rect(rect),
            rect,
            &self.view,
            &self.doc,
            overlay_style,
        );

        // ---- 笔刷尺寸圆圈（`gui/brushmanip.py` 的 `BrushSizeOverlay`）----
        // 只在 `BrushResizeMode` 拖拽期间存在；源码把它 append 到
        // `display_overlays`（`:70-71`），所以画在画框/对称轴之上。
        if let Some(state) = self.modes.top().brush_size_overlay() {
            crate::shell::overlays::paint_brush_size(
                &ui.painter().with_clip_rect(rect),
                rect,
                &self.view,
                state,
                overlay_style,
            );
        }

        // ---- 取色预览方块（`ColorPickPreviewOverlay`，`colorpicker.py:253-364`）----
        //
        // 只在取色模式的按键/快捷键按住期间显示（源码刻意避免闪烁）。
        let pick_mode = self
            .modes
            .top()
            .action_name()
            .map(|n| n.starts_with("ColorPickMode") || n.starts_with("FlipColorPickMode"))
            .unwrap_or(false);
        if pick_mode && ui.input(|i| i.pointer.primary_down()) {
            // 拖拽时 `response.hover_pos()` 可能为 None（指针被模式 grab），
            // 用输入层的最近位置；仍限定在画布内。
            let pos = ui.input(|i| i.pointer.interact_pos());
            if let Some(pos) = pos.filter(|p| rect.contains(*p)) {
                let rgb = self.color_manager.get_color().get_rgb();
                crate::shell::overlays::paint_pick_preview(
                    &ui.painter().with_clip_rect(rect),
                    rect,
                    pos,
                    crate::shell::color_panel::rgb32(rgb),
                );
            }
        }

        // ---- 笔刷光标（`gui/cursor.py`）----
        //
        // 源码给每个绘画模式设置一张自绘 pixbuf 光标；egui 侧改为在画布上
        // 直接画内外圈 + 动态十字线，并隐藏系统光标（`CursorIcon::None`）。
        // `ui.hide_cursor_while_painting` 只控制"作画时是否还显示这个自绘光标"。
        let paints = self
            .modes
            .top()
            .pointer_behavior()
            .intersects(pigment_engine::mode::Behavior::PAINT_BRUSH);
        if paints && response.hovered() && !space_down && !view_gesture {
            let painting = ui.input(|i| i.pointer.any_down());
            let hide_now = painting && self.mode_prefs.hide_cursor_while_painting;
            if !hide_now {
                if let Some(pos) = response.hover_pos() {
                    // `tileddrawwidget.py:586-588`：crosshair 预设用系统十字光标，
                    // 不画圆环；其余三个预设都是自绘圆环（宽度/颜色由偏好定）。
                    if crate::shell::overlays::cursor_is_crosshair(&self.prefs) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    } else {
                        let base_r = self.brush.get(BrushSetting::Radius).base_value;
                        let by_random = self.brush.get(BrushSetting::OffsetByRandom).base_value;
                        let visual = base_r.exp() * (1.0 + 2.0 * by_random);
                        // `tileddrawwidget.py:606`：`r = visual_radius * scale + 0.5`
                        let cursor_style =
                            crate::shell::overlays::CursorStyle::from_brush(&self.brush, self.eraser);
                        crate::shell::overlays::paint_brush_cursor(
                            &ui.painter().with_clip_rect(rect),
                            rect,
                            pos,
                            visual * self.view.zoom + 0.5,
                            &overlay_style,
                            cursor_style,
                            &self.prefs,
                        );
                        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    }
                }
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            }
        }

        // ---- Inking ADJUST 节点（`inktool.py:1074-1116`）----
        if self.modes.top().ink_editing() {
            if let Some(nodes) = self.modes.top().ink_nodes() {
                let painter = ui.painter().with_clip_rect(rect);
                let target = self.modes.top().ink_target_node();
                for (i, n) in nodes.iter().enumerate() {
                    let s = self.view.world_to_screen(n.x, n.y);
                    let p = egui::pos2(rect.min.x + s[0], rect.min.y + s[1]);
                    let color = if target == Some(i) {
                        egui::Color32::from_rgb(0xF1, 0xC4, 0x0F)
                    } else {
                        egui::Color32::from_rgb(0xEC, 0xF0, 0xF1)
                    };
                    painter.circle_filled(p, 4.0, color);
                }
            }
            // 画布 OK / 垃圾桶浮动按钮（`inktool.py:1090-1116`）
            if let Some((accept, reject)) = self.ink_button_positions(rect) {
                let painter = ui.painter().with_clip_rect(rect);
                for (local, glyph) in [(accept, "✓"), (reject, "×")] {
                    let p = egui::pos2(rect.min.x + local.x, rect.min.y + local.y);
                    painter.circle_filled(p, 16.0, egui::Color32::from_rgb(0xEC, 0xF0, 0xF1));
                    painter.text(
                        p,
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        egui::FontId::proportional(18.0),
                        egui::Color32::BLACK,
                    );
                }
            }
        }

        // ---- 对称/画框编辑模式的浮动按钮（`symmetry.py:955-975`、
        // `framewindow.py:888-899`）----
        match self.modes.top().action_name() {
            Some("SymmetryEditMode") => {
                let painter = ui.painter().with_clip_rect(rect);
                let center = self.doc.symmetry_center();
                let d = crate::shell::overlays::symmetry_disable_pos(rect, &self.view, center);
                painter.circle_filled(d, 16.0, egui::Color32::from_rgb(0xEC, 0xF0, 0xF1));
                painter.text(
                    d,
                    egui::Align2::CENTER_CENTER,
                    "×",
                    egui::FontId::proportional(18.0),
                    egui::Color32::BLACK,
                );
                let [sx, sy] = self.view.world_to_screen(center.0 as f32, center.1 as f32);
                let c = rect.min + egui::vec2(sx, sy);
                if rect.contains(c) {
                    painter.circle_filled(c, 8.0, egui::Color32::from_rgb(0xF1, 0xC4, 0x0F));
                }
            }
            Some("FrameEditMode") => {
                let painter = ui.painter().with_clip_rect(rect);
                let d = crate::shell::overlays::frame_remove_pos(rect);
                painter.circle_filled(d, 16.0, egui::Color32::from_rgb(0xEC, 0xF0, 0xF1));
                painter.text(
                    d,
                    egui::Align2::CENTER_CENTER,
                    "🗑",
                    egui::FontId::proportional(14.0),
                    egui::Color32::BLACK,
                );
            }
            _ => {}
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

/// 离散步进滚轮增量 —— `ui.support_smooth_scrolling` 关闭时的取值。
///
/// egui 0.37 的 `InputState` 不再暴露 `raw_scroll_delta`，但 `raw.events` 里
/// 仍有后端发来的 `MouseWheel`（单位 Point/Line/Page）。这里按 egui 自己的
/// 换算规则折成"点"：1 行 = 50 点（`InputOptions::line_scroll_speed` 的出厂值），
/// 1 页 = 视口高度。关闭平滑滚动时用它，滚轮就回到整格步进。
fn stepwise_wheel(events: &[egui::Event], page_height: f32) -> egui::Vec2 {
    let mut out = egui::Vec2::ZERO;
    for event in events {
        if let egui::Event::MouseWheel { unit, delta, .. } = event {
            let scale = match unit {
                egui::MouseWheelUnit::Point => 1.0,
                egui::MouseWheelUnit::Line => 50.0,
                egui::MouseWheelUnit::Page => page_height,
            };
            out += *delta * scale;
        }
    }
    out
}

/// 对称类型在选项面板里的显示名（`symmetry.py:368` 的 `Symmetry Type` 条目）。
fn symmetry_label(t: pigment_surface::symmetry::SymmetryType) -> &'static str {
    use pigment_surface::symmetry::SymmetryType as T;
    match t {
        T::Vertical => "Vertical",
        T::Horizontal => "Horizontal",
        T::VertHorz => "Vertical + Horizontal",
        T::Rotational => "Rotational",
        T::Snowflake => "Snowflake",
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(unit: egui::MouseWheelUnit, dx: f32, dy: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(dx, dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn stepwise_wheel_converts_units_to_points() {
        // Point 原样；Line 按 50 点/行；Page 按视口高度
        let events = [
            wheel(egui::MouseWheelUnit::Point, 10.0, -20.0),
            wheel(egui::MouseWheelUnit::Line, 1.0, 2.0),
            wheel(egui::MouseWheelUnit::Page, 0.0, 1.0),
        ];
        let d = stepwise_wheel(&events, 600.0);
        assert_eq!(d, egui::vec2(10.0 + 50.0, -20.0 + 100.0 + 600.0));
        // 没有滚轮事件 → 零（关闭平滑滚动不会凭空产生位移）
        assert_eq!(stepwise_wheel(&[], 600.0), egui::Vec2::ZERO);
        // 非滚轮事件被忽略
        assert_eq!(
            stepwise_wheel(&[egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }], 600.0),
            egui::Vec2::ZERO
        );
    }
}
