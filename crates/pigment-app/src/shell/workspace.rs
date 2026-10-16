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

use eframe::egui;

use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::BrushSetting;

use crate::app::PigmentApp;
use crate::convert;
use crate::gpu::TileUpload;
use crate::view::View;

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
                                let history: Vec<[f32; 3]> = self.color_history.colors().to_vec();
                                let n = history.len();
                                ui.horizontal_wrapped(|ui| {
                                    for (i, hsv) in history.into_iter().enumerate() {
                                        let tip = format!("颜色历史 {}/{}", i + 1, n);
                                        if crate::app::color_swatch(ui, hsv, false, &tip).clicked()
                                        {
                                            self.set_color(hsv);
                                        }
                                    }
                                    ui.label(
                                        egui::RichText::new("最近用的在最后")
                                            .weak()
                                            .small(),
                                    );
                                });
                                let mut color = self.color;
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
                                    self.set_color(color);
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
                                ui.checkbox(
                                    &mut self.context_restores_color,
                                    "恢复槽位时连颜色一起还原",
                                )
                                .on_hover_text(
                                    "MyPaint 偏好 misc.context_restores_color（默认开）\n\
                                     userconfig.py:93",
                                );
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
        let history: Vec<[f32; 3]> = self.color_history.colors().to_vec();
        let n = history.len();
        ui.horizontal_wrapped(|ui| {
            for (i, hsv) in history.into_iter().enumerate() {
                let tip = format!("颜色历史 {}/{}", i + 1, n);
                if crate::app::color_swatch(ui, hsv, false, &tip).clicked() {
                    self.set_color(hsv);
                }
            }
            if n == 0 {
                ui.label(egui::RichText::new("（画一笔后出现）").weak().small());
            }
        });
        ui.label(egui::RichText::new("最近用的在最后").weak().small());
        ui.separator();
        ui.label(
            egui::RichText::new("最近用过的笔刷属 B3（笔刷历史跟踪）。").weak().small(),
        );
    }

    /// 画布（`TiledDrawWidget`）—— 工作区的中央，`expand=True`。
    ///
    /// 这一段是 M2 打通的渲染关键路径，S1 只是把它从"整窗口中央"
    /// 挪进工作区中央：视图矩阵用的是 `content_rect`（全窗口）而非本面板的
    /// `rect`，所以压边之后**偏移量必须跟着面板原点走** —— 见下面
    /// `view_window.offset` 里那个 `rect.min`。
    pub(crate) fn canvas(&mut self, ui: &mut egui::Ui) {
        let ppp = ui.ctx().pixels_per_point();
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
        // 一次拖拽 = 一条 Brushwork（`gui/mode.py:722` 的 brushwork_begin/commit）：
        // 起笔取画前快照，落笔取画后快照并提交给撤销栈。
        //
        // 文件弹层开着时禁掉画布交互（GTK 的模态对话框也是这个效果）。
        let space_down = ui.input(|i| i.key_down(egui::Key::Space));
        let modal = self.file_prompt.is_some();
        if !modal && !space_down && response.drag_started_by(egui::PointerButton::Primary) {
            // 本应用的笔画开局：新建一份引擎状态（见 app.rs 文件头「撤销模型」注释）
            self.stroke_state = Some(pigment_engine::hokusai::BrushState::default());
            let mut bw = pigment_engine::Brushwork::new();
            // 目标层 = 起笔时的当前层（等价 command.py 的 layer=None → current）
            bw.target_layer = Some(self.doc.layers().current_id());
            self.brushwork = Some(bw);
            if let Some(pos) = response.interact_pointer_pos() {
                let local = pos - rect.min;
                let [wx, wy] = self.view.screen_to_world(local.x, local.y);
                let ev = self.stroke_event(wx, wy, 1.0, 0.01);
                self.paint_event(ev);
            }
            self.stroke_dirty = true;
        }
        if !modal && !space_down && response.dragged_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = pos - rect.min;
                let [wx, wy] = self.view.screen_to_world(local.x, local.y);
                let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05) as f64;
                let ev = self.stroke_event(wx, wy, 1.0, dt);
                self.paint_event(ev);
            }
            self.stroke_dirty = true;
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.commit_stroke();
            // 颜色历史入栈时机对齐 MyPaint：`__input_stroke_ended_cb`
            // （gui/brushcolor.py:54）——每笔结束后记录，且橡皮不记录。
            if !self.eraser {
                self.color_history.push(self.color);
            }
        }

        // ---- 可见 tile 列表（物理像素域）----
        let vp_w = rect.width() * ppp;
        let vp_h = rect.height() * ppp;
        let view_phys = View {
            zoom: self.view.zoom * ppp,
            offset: [self.view.offset[0] * ppp, self.view.offset[1] * ppp],
        };
        let (mut tx0, mut ty0, mut tx1, mut ty1) =
            view_phys.visible_tile_range(vp_w, vp_h, TILE_SIZE as i32);
        // 保护：极端缩小时可见范围可达数万 tile，夹到视口中心附近的上限内
        // （空区域由面板白底兜底，不影响观感；后续再上 atlas/mip 方案）
        const MAX_SPAN: i32 = 96;
        if tx1 - tx0 + 1 > MAX_SPAN {
            let c = (tx0 + tx1) / 2;
            tx0 = c - MAX_SPAN / 2;
            tx1 = tx0 + MAX_SPAN - 1;
        }
        if ty1 - ty0 + 1 > MAX_SPAN {
            let c = (ty0 + ty1) / 2;
            ty0 = c - MAX_SPAN / 2;
            ty1 = ty0 + MAX_SPAN - 1;
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
            let mut renderer = self.renderer.lock().unwrap();
            if self.force_resync {
                renderer.invalidate_all();
                self.force_resync = false;
            }
            // B2-3：上传的是**图层栈的合成结果**（含背景与各层混合模式）。
            // 水位线用全局合成版本号：任何影响合成的变化（作画/结构/属性/
            // 背景）都会抬高它。粒度粗于逐 tile revision，但正确性不变；
            // 逐 tile 哈希记账为后续优化。
            let composite_rev = self.doc.layers().composite_revision();
            let plan = crate::gpu::plan_uploads(
                |c| renderer.uploaded_rev(c),
                &visible,
                crate::gpu::MAX_UPLOADS_PER_FRAME,
                |c| self.doc.layers().has_paint_at(c.0, c.1),
                composite_rev,
            );
            for coord in plan {
                let Some(pixels) = self.doc.layers().composite_tile_rgba8(coord.0, coord.1) else {
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
            // 撤销后 CPU 侧已无笔迹的 tile 要从显存剔除
            crate::gpu::plan_evictions(
                |c| renderer.uploaded_rev(c).is_some(),
                &visible,
                |c| self.doc.layers().has_paint_at(c.0, c.1),
            )
        };
        self.pending_uploads = uploads.len();

        // ---- 视图矩阵：world px → 全窗口裁剪空间 ----
        // 用全窗口 `content_rect` 作参考系，所以要把本面板的原点 `rect.min` 加回去；
        // 画布从"占满窗口"变成"工作区中央"之后，这一步不能省。
        let win = ui.ctx().content_rect();
        let win_w = win.width() * ppp;
        let win_h = win.height() * ppp;
        let view_window = View {
            zoom: view_phys.zoom,
            offset: [
                (self.view.offset[0] + rect.min.x) * ppp,
                (self.view.offset[1] + rect.min.y) * ppp,
            ],
        };
        let view_matrix = view_window.clip_matrix(win_w, win_h);

        ui.painter()
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                crate::gpu::CanvasPaint {
                    renderer: std::sync::Arc::clone(&self.renderer),
                    view_matrix,
                    uploads,
                    evict,
                    visible,
                },
            ));

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
        if self.stroke_state.is_some() || self.stroke_dirty || backlog {
            self.stroke_dirty = false;
            ui.ctx().request_repaint();
        }
    }
}
