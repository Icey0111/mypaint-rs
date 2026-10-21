//! 主窗口内的对话框与浮层。
//!
//! 原生 OS 子窗口路径在 `shell31` 里仍保留，但新 chrome 的主界面浮层走这里：
//! 文件路径、破坏性确认、自动恢复、兼容警告、笔刷包 readme、偏好、关于、
//! 输入测试、笔刷编辑器与图层重命名。所有弹层都使用同一套深灰平面 Frame，
//! 没有默认 egui 的圆角卡片和蓝色高亮。

use eframe::egui;
use pigment_engine::autosave::{self, AutosaveInfo};
use pigment_engine::color::UIColor;

use crate::app::{ConfirmAction, FileAction, PigmentApp};
use crate::shell::chrome::PrefsTab;
use crate::shell::theme;

impl PigmentApp {
    /// 一帧内按优先级绘制所有浮层。
    pub(crate) fn chrome_dialogs(&mut self, ctx: &egui::Context) {
        self.preferences_window(ctx);
        self.about_window(ctx);
        self.input_test_window(ctx);
        self.brush_editor_window(ctx);
        self.layer_rename_dialog(ctx);
        self.recovery_window(ctx);
        self.confirm_dialog(ctx);
        self.file_warning_dialog(ctx);
        self.brushpack_readme_dialog(ctx);
        self.show_file_prompt(ctx);
    }

    /// 是否有阻塞画布的模态浮层（`egui::Modal` 自己也会挡输入，这里给
    /// 画布内的快捷键/悬停判断用）。
    pub(crate) fn modal_active(&self) -> bool {
        self.file_prompt.is_some()
            || self.confirm.is_some()
            || self.file_warning.is_some()
            || self.brushpack_readme.is_some()
    }

    // =====================================================================
    // 偏好
    // =====================================================================

    fn preferences_window(&mut self, ctx: &egui::Context) {
        if !self.chrome.prefs_open {
            return;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new("偏好")
            .id(egui::Id::unique("pigment_preferences"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(560.0, 430.0))
            .min_width(500.0)
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                dialog_heading(ui, "偏好", "界面、输入与文件 · 即时生效");
                // 标签条
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (PrefsTab::General, "常规"),
                        (PrefsTab::Interface, "界面"),
                        (PrefsTab::Files, "文件"),
                    ] {
                        let selected = self.chrome.prefs_tab == tab;
                        let resp = ui.add(
                            egui::Button::new(egui::RichText::new(label).size(12.5).color(
                                if selected {
                                    theme::TEXT
                                } else {
                                    theme::TEXT_DIM
                                },
                            ))
                            .frame(false)
                            .min_size(egui::vec2(64.0, 26.0)),
                        );
                        if selected {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(resp.rect.left(), resp.rect.bottom() - 2.0),
                                    egui::pos2(resp.rect.right(), resp.rect.bottom()),
                                ),
                                0.0,
                                theme::ACCENT,
                            );
                        }
                        if resp.clicked() {
                            self.chrome.prefs_tab = tab;
                        }
                        ui.add_space(3.0);
                    }
                });
                theme::hline(ui, theme::LINE);
                ui.add_space(8.0);

                match self.chrome.prefs_tab {
                    PrefsTab::General => {
                        let mut locale = self.prefs.str_or("lib.i18n.userlocale", "").to_string();
                        ui.horizontal(|ui| {
                            ui.label(theme::dim("界面语言"));
                            egui::ComboBox::from_id_salt("prefs_locale")
                                .width(180.0)
                                .selected_text(match locale.as_str() {
                                    "" | "auto" => "跟随系统",
                                    "zh_CN" => "简体中文",
                                    "en_US" => "English",
                                    other => other,
                                })
                                .show_ui(ui, |ui| {
                                    for (code, label) in [
                                        ("auto", "跟随系统"),
                                        ("zh_CN", "简体中文"),
                                        ("en_US", "English"),
                                    ] {
                                        if ui.selectable_label(locale == code, label).clicked() {
                                            locale = code.to_string();
                                        }
                                    }
                                });
                        });
                        if locale != self.prefs.str_or("lib.i18n.userlocale", "") {
                            self.prefs.set_str("lib.i18n.userlocale", locale);
                            changed = true;
                        }
                        ui.add_space(4.0);
                        theme::hline(ui, theme::LINE_SOFT);
                        let mut hide_cursor = self
                            .prefs
                            .bool_or(pigment_engine::prefs::KEY_HIDE_CURSOR_WHILE_PAINTING, false);
                        if theme::check_row(
                            ui,
                            "作画时隐藏系统光标（显示自绘笔刷圈）",
                            &mut hide_cursor,
                        )
                        .changed()
                        {
                            self.prefs.set_bool(
                                pigment_engine::prefs::KEY_HIDE_CURSOR_WHILE_PAINTING,
                                hide_cursor,
                            );
                            changed = true;
                        }
                        let mut smooth = self.prefs.bool_or("ui.support_smooth_scrolling", true);
                        if theme::check_row(ui, "平滑滚轮缩放", &mut smooth).changed() {
                            self.prefs.set_bool("ui.support_smooth_scrolling", smooth);
                            changed = true;
                        }
                        let mut restore = self.prefs.bool_or("misc.context_restores_color", true);
                        if theme::check_row(ui, "恢复笔刷槽位时同时还原颜色", &mut restore)
                            .changed()
                        {
                            self.prefs.set_bool("misc.context_restores_color", restore);
                            self.color_manager.set_context_restores_color(restore);
                            changed = true;
                        }
                        let mut recover = self.prefs.bool_or("autorecovery.check_at_startup", true);
                        if theme::check_row(ui, "启动时检查自动备份", &mut recover).changed()
                        {
                            self.prefs
                                .set_bool("autorecovery.check_at_startup", recover);
                            changed = true;
                        }
                        ui.add_space(4.0);
                        theme::hline(ui, theme::LINE_SOFT);
                        let mut autosave_on = self.prefs.bool_or("document.autosave_backups", true);
                        if theme::check_row(ui, "启用自动备份", &mut autosave_on).changed() {
                            self.prefs
                                .set_bool("document.autosave_backups", autosave_on);
                            changed = true;
                        }
                        let mut interval = self
                            .prefs
                            .f64_or("document.autosave_interval", 10.0)
                            .clamp(5.0, 300.0) as f32;
                        ui.horizontal(|ui| {
                            ui.label(theme::faint("备份间隔"));
                            if theme::flat_slider(
                                ui,
                                &mut interval,
                                5.0..=300.0,
                                220.0,
                                theme::TEXT_DIM,
                            )
                            .changed()
                            {
                                self.prefs
                                    .set_f64("document.autosave_interval", interval as f64);
                                changed = true;
                            }
                            ui.label(
                                egui::RichText::new(format!("{interval:.0} 秒"))
                                    .size(10.5)
                                    .color(theme::TEXT_DIM),
                            );
                        });
                        ui.add_space(4.0);
                        theme::hline(ui, theme::LINE_SOFT);
                        ui.label(theme::dim("兼容性"));
                        let mut compat = self.compat_settings;
                        let mut default_mode = pigment_engine::compat::default_mode(&self.prefs)
                            .as_str()
                            .to_string();
                        ui.horizontal(|ui| {
                            ui.label(theme::faint("新建文档默认模式"));
                            egui::ComboBox::from_id_salt("prefs_compat_mode")
                                .width(120.0)
                                .selected_text(match default_mode.as_str() {
                                    "1.x" => "1.x（旧版）",
                                    _ => "2.x（现行）",
                                })
                                .show_ui(ui, |ui| {
                                    for (key, label) in
                                        [("1.x", "1.x（旧版）"), ("2.x", "2.x（现行）")]
                                    {
                                        if ui.selectable_label(default_mode == key, label).clicked()
                                        {
                                            default_mode = key.to_string();
                                        }
                                    }
                                });
                        });
                        if default_mode != self.prefs.str_or("default_compatibility_mode", "2.x") {
                            self.prefs
                                .set_str("default_compatibility_mode", default_mode);
                            changed = true;
                        }
                        if theme::check_row(
                            ui,
                            "1.x：省略 paint_mode 的笔刷默认开颜料",
                            &mut compat.one_x.pigment_on_by_default,
                        )
                        .changed()
                        {
                            changed = true;
                        }
                        if theme::check_row(
                            ui,
                            "2.x：省略 paint_mode 的笔刷默认开颜料",
                            &mut compat.two_x.pigment_on_by_default,
                        )
                        .changed()
                        {
                            changed = true;
                        }
                        if theme::check_row(
                            ui,
                            "1.x：新建图层默认颜料混合",
                            &mut compat.one_x.pigment_layer_is_default,
                        )
                        .changed()
                        {
                            changed = true;
                        }
                        if theme::check_row(
                            ui,
                            "2.x：新建图层默认颜料混合",
                            &mut compat.two_x.pigment_layer_is_default,
                        )
                        .changed()
                        {
                            changed = true;
                        }
                        if compat != self.compat_settings {
                            compat.store(&mut self.prefs);
                        }
                    }
                    PrefsTab::Interface => {
                        ui.label(theme::dim("面板"));
                        let mut left = self.chrome.left_open;
                        if theme::check_row(ui, "笔刷侧栏", &mut left).changed() {
                            self.chrome.left_open = left;
                        }
                        let mut color = self.chrome.color_open;
                        if theme::check_row(ui, "颜色面板", &mut color).changed() {
                            self.chrome.color_open = color;
                        }
                        let mut layers = self.chrome.layers_open;
                        if theme::check_row(ui, "图层面板", &mut layers).changed() {
                            self.chrome.layers_open = layers;
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("恢复默认布局").clicked() {
                                self.chrome.left_open = true;
                                self.chrome.color_open = true;
                                self.chrome.layers_open = true;
                                self.chrome.left_width = 238.0;
                                self.chrome.right_width = 270.0;
                                self.chrome.color_height = 300.0;
                            }
                            if ui.button("收起两侧面板").clicked() {
                                self.chrome.left_open = false;
                                self.chrome.color_open = false;
                                self.chrome.layers_open = false;
                            }
                        });
                        ui.add_space(8.0);
                        theme::hline(ui, theme::LINE_SOFT);
                        ui.add_space(4.0);
                        ui.label(theme::dim("画布"));
                        let mut scale = self.scale_feedback;
                        if theme::check_row(ui, "缩放反馈（右上角 HUD）", &mut scale).changed()
                        {
                            self.scale_feedback = scale;
                            self.prefs.set_bool("ui.feedback.scale", scale);
                            changed = true;
                        }
                        let mut last_pos = self.last_pos_feedback;
                        if theme::check_row(ui, "落笔位置残留标记", &mut last_pos).changed()
                        {
                            self.last_pos_feedback = last_pos;
                            self.prefs.set_bool("ui.feedback.last_pos", last_pos);
                            changed = true;
                        }
                        ui.add_space(8.0);
                        ui.label(theme::faint(
                            "UI 缩放 1.5×，随系统 DPI 放大字体与工具图标。",
                        ));
                    }
                    PrefsTab::Files => {
                        let mut format = self
                            .prefs
                            .str_or("saving.default_format", "openraster")
                            .to_string();
                        ui.horizontal(|ui| {
                            ui.label(theme::dim("默认保存格式"));
                            egui::ComboBox::from_id_salt("prefs_format")
                                .width(200.0)
                                .selected_text(match format.as_str() {
                                    "png-solid" => "PNG（压平）",
                                    "jpeg-90%" => "JPEG 90%",
                                    _ => "OpenRaster（.ora）",
                                })
                                .show_ui(ui, |ui| {
                                    for (key, label) in [
                                        ("openraster", "OpenRaster（.ora）"),
                                        ("png-solid", "PNG（压平）"),
                                        ("jpeg-90%", "JPEG 90%"),
                                    ] {
                                        if ui.selectable_label(format == key, label).clicked() {
                                            format = key.to_string();
                                        }
                                    }
                                });
                        });
                        if format != self.prefs.str_or("saving.default_format", "") {
                            self.prefs.set_str("saving.default_format", format);
                            changed = true;
                        }
                        ui.add_space(6.0);
                        ui.label(theme::faint(
                            "自动备份与调色板文件的位置由 MyPaint 的配置目录约定决定；\n\
                             备份恢复入口：文件 → 恢复自动备份。",
                        ));
                        ui.add_space(8.0);
                        if ui.button("打开画笔与配置文件目录").clicked() {
                            let dir = pigment_engine::prefs::config_dir();
                            let _ = crate::app::open_external_url(&dir.display().to_string());
                        }
                    }
                }

                ui.add_space(10.0);
                theme::hline(ui, theme::LINE);
                ui.horizontal(|ui| {
                    ui.label(theme::faint("改动即时写入偏好；关闭后仍可从菜单重新打开。"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("关闭").clicked() {
                            self.chrome.prefs_open = false;
                        }
                    });
                });
            });
        if !open {
            self.chrome.prefs_open = false;
        }
        if changed {
            self.apply_settings();
            let _ = self.prefs.save();
            ctx.request_repaint();
        }
    }

    // =====================================================================
    // 关于 / 输入测试
    // =====================================================================

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut open = true;
        egui::Window::new("关于 MyPaint")
            .id(egui::Id::unique("pigment_about"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_size(egui::vec2(440.0, 340.0))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("PIGMENT")
                            .size(28.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.label(
                        egui::RichText::new("MyPaint 精神续作 · egui 重构")
                            .size(13.0)
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "版本 {}  ·  {}",
                            env!("CARGO_PKG_VERSION"),
                            pigment_engine::ENGINE_INFO
                        ))
                        .size(11.0)
                        .color(theme::TEXT_FAINT),
                    );
                });
                ui.add_space(8.0);
                theme::hline(ui, theme::LINE);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "画笔引擎与笔刷包来自 MyPaint / libmypaint / hokusai；\n\
                         UI 以 ArrowVortex 的平面、紧凑、画布优先风格重排。\n\
                         本程序为 GPL-3.0-or-later。",
                    )
                    .size(12.0)
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("MyPaint 主页").clicked() {
                        let _ = crate::app::open_external_url("https://mypaint.app/");
                    }
                    if ui.button("关闭").clicked() {
                        self.about_open = false;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(theme::faint("© MyPaint contributors"));
                    });
                });
            });
        if !open {
            self.about_open = false;
        }
    }

    fn input_test_window(&mut self, ctx: &egui::Context) {
        if !self.input_test_open {
            return;
        }
        let mut open = true;
        egui::Window::new("输入设备测试")
            .id(egui::Id::unique("pigment_input_test"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(560.0, 420.0))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                dialog_heading(ui, "输入设备", "数位板压力 / 倾斜 / 笔尾橡皮");
                let t = &self.tablet;
                egui::Grid::new("input_test_grid")
                    .num_columns(2)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(theme::faint("设备"));
                        ui.label(if t.active {
                            "数位板工具"
                        } else {
                            "鼠标 / 未激活"
                        });
                        ui.end_row();
                        ui.label(theme::faint("设备 ID"));
                        ui.label(t.device_id.as_deref().unwrap_or("—"));
                        ui.end_row();
                        ui.label(theme::faint("压力"));
                        ui.label(format!("{:.3}", t.pressure));
                        ui.end_row();
                        ui.label(theme::faint("倾斜"));
                        ui.label(format!("x {:.3}  y {:.3}", t.tilt.0, t.tilt.1));
                        ui.end_row();
                        ui.label(theme::faint("笔尾橡皮"));
                        ui.label(if t.eraser { "按下" } else { "—" });
                        ui.end_row();
                        ui.label(theme::faint("按键映射"));
                        ui.label(format!(
                            "中键 {}，右键 {}",
                            self.prefs.str_or("input.button_mapping", "…"),
                            "平移"
                        ));
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.label(theme::dim("在画布上移动笔尖即可更新这些值。"));
                ui.add_space(6.0);
                theme::hline(ui, theme::LINE);
                let mut print_inputs = self.print_inputs;
                if theme::check_row(ui, "把输入事件打印到日志（调试）", &mut print_inputs).changed()
                {
                    self.run_action("PrintInputs", ctx, ctx.input(|i| i.time));
                }
                ui.add_space(4.0);
                let mut log = String::new();
                for line in &self.input_test_log {
                    log.push_str(line);
                    log.push('\n');
                }
                if log.is_empty() {
                    ui.label(theme::faint("暂无事件记录。"));
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(log).monospace().size(10.0));
                        });
                }
                ui.horizontal(|ui| {
                    if ui.button("清空").clicked() {
                        self.input_test_log.clear();
                    }
                    if ui.button("关闭").clicked() {
                        self.input_test_open = false;
                    }
                });
            });
        if !open {
            self.input_test_open = false;
        }
    }

    // =====================================================================
    // 笔刷编辑器
    // =====================================================================

    fn brush_editor_window(&mut self, ctx: &egui::Context) {
        if !self.brush_editor_open {
            return;
        }
        let mut open = true;
        let sel = self.brushes.selected;
        let icon = self.brushes.icon(ctx, sel);
        let brush_title = self
            .brushes
            .selected_entry()
            .map(|e| format!("{}  ·  {}", e.mypaint_name(), e.group_display))
            .unwrap_or_else(|| self.brush_name.clone());
        egui::Window::new("笔刷编辑器")
            .id(egui::Id::unique("pigment_brush_editor"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(820.0, 680.0))
            .default_pos(egui::pos2(170.0, 86.0))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                dialog_heading(ui, "笔刷编辑器", &brush_title);
                ui.horizontal(|ui| {
                    if ui.button("新建笔刷").clicked() {
                        self.new_brush_from_current();
                    }
                    if ui.button("保存到用户目录").clicked() {
                        self.save_current_brush_to_disk();
                    }
                    if ui.button("重新载入").clicked() {
                        self.reload_selected_brush();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut live = self.live_update;
                        if ui.checkbox(&mut live, "实时重绘上一笔").changed() {
                            self.live_update = live;
                            if live {
                                self.apply_live_update();
                            }
                        }
                    });
                });
                ui.add_space(6.0);
                theme::hline(ui, theme::LINE);
                ui.add_space(4.0);
                // 三个不相交字段可同时可变借用：编辑器 / 当前笔刷 / 库元数据。
                let about = self
                    .brushes
                    .library
                    .entries
                    .get_mut(sel)
                    .map(|e| (&mut e.display, &mut e.notes));
                let edited = self
                    .editor
                    .show(ui, &mut self.base_brush, Some(&icon), about);
                if edited {
                    self.derive_active();
                    if self.live_update {
                        self.apply_live_update();
                    }
                }
            });
        if !open {
            self.brush_editor_open = false;
        }
    }

    // =====================================================================
    // 图层重命名
    // =====================================================================

    fn layer_rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.layer_rename else {
            return;
        };
        let Some(path) = self.doc.layers().path_of_id(id) else {
            self.layer_rename = None;
            return;
        };
        let current = self
            .doc
            .layers()
            .layer_at(&path)
            .map(|l| l.display_name(*path.last().unwrap_or(&0)))
            .unwrap_or_default();
        let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut commit = enter;
        let mut cancel = escape;
        let mut open = true;
        egui::Window::new("重命名图层")
            .id(egui::Id::unique("pigment_layer_rename"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(theme::faint(format!("原名称：{current}")));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.rename_buf)
                        .desired_width(f32::INFINITY)
                        .hint_text("图层名称"),
                );
                if !self.rename_focused {
                    resp.request_focus();
                    self.rename_focused = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    if ui.button("确定").clicked() {
                        commit = true;
                    }
                });
            });
        if cancel {
            self.layer_rename = None;
            self.rename_focused = false;
            return;
        }
        if commit || !open {
            let name = self.rename_buf.trim().to_string();
            if !name.is_empty() {
                self.doc.rename_layer(&path, Some(name));
            }
            self.layer_rename = None;
            self.rename_focused = false;
        }
    }

    // =====================================================================
    // 自动恢复
    // =====================================================================

    fn recovery_window(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.recovery.take() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let mut recover: Option<usize> = None;
        let mut delete: Option<usize> = None;
        let mut close = false;
        let mut open = true;
        egui::Window::new("恢复中断的工作？")
            .id(egui::Id::unique("pigment_recovery"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(620.0, 440.0))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "上次会话有未保存的作画。可以在这里选择一份自动备份继续，\n\
                         恢复后会先询问保存位置。",
                    )
                    .size(12.0)
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(6.0);
                theme::hline(ui, theme::LINE);
                egui::ScrollArea::vertical()
                    .max_height(270.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, entry) in state.entries.iter().enumerate() {
                            let selected = state.selected == Some(i);
                            let thumb_path = entry.thumbnail_path.clone();
                            let tex = state
                                .thumbs
                                .entry(entry.path.clone())
                                .or_insert_with(|| load_thumb(ctx, &thumb_path))
                                .clone();
                            let (row_rect, row_resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 88.0),
                                egui::Sense::click(),
                            );
                            if row_resp.clicked() {
                                state.selected = Some(i);
                            }
                            if selected {
                                ui.painter().rect_filled(row_rect, 0.0, theme::ACTIVE);
                                ui.painter().rect_stroke(
                                    row_rect,
                                    0.0,
                                    egui::Stroke::new(1.0, theme::ACCENT),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            let thumb = egui::Rect::from_min_size(
                                row_rect.min + egui::vec2(7.0, 7.0),
                                egui::vec2(74.0, 74.0),
                            );
                            ui.painter().rect_filled(thumb, 0.0, theme::FIELD);
                            if let Some(t) = &tex {
                                ui.painter().image(
                                    t.id(),
                                    thumb,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            }
                            ui.painter().text(
                                egui::pos2(thumb.right() + 14.0, row_rect.top() + 20.0),
                                egui::Align2::LEFT_CENTER,
                                entry.description(std::time::SystemTime::now()),
                                egui::FontId::proportional(11.0),
                                theme::TEXT_DIM,
                            );
                            if entry.cache_in_use {
                                ui.painter().text(
                                    egui::pos2(thumb.right() + 14.0, row_rect.bottom() - 16.0),
                                    egui::Align2::LEFT_CENTER,
                                    "可能仍被另一个实例占用",
                                    egui::FontId::proportional(10.0),
                                    theme::WARN,
                                );
                            }
                            ui.add_space(4.0);
                        }
                    });
                ui.add_space(4.0);
                let mut check = state.check_at_startup;
                if theme::check_row(ui, "启动时检查自动备份", &mut check).changed() {
                    state.check_at_startup = check;
                    self.prefs.set_bool("autorecovery.check_at_startup", check);
                    let _ = self.prefs.save();
                }
                theme::hline(ui, theme::LINE);
                ui.horizontal(|ui| {
                    let can = state
                        .selected
                        .map(|i| i < state.entries.len() && !state.entries[i].cache_in_use)
                        .unwrap_or(false);
                    if ui.add_enabled(can, egui::Button::new("删除备份")).clicked() {
                        delete = state.selected;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(can, egui::Button::new("恢复并保存…"))
                            .clicked()
                        {
                            recover = state.selected;
                        }
                        if ui.button("稍后决定").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if !open {
            close = true;
        }
        if let Some(i) = delete {
            if let Some(entry) = state.entries.get(i) {
                if let Err(e) = autosave::delete_autosave(entry) {
                    self.set_status(format!("删除备份失败：{e}"), now);
                } else {
                    self.set_status("已删除备份".to_string(), now);
                }
            }
            state.entries = self.recoverable_backups();
            state.selected = None;
        }
        if let Some(i) = recover {
            if let Some(entry) = state.entries.get(i).cloned() {
                self.recover_from(&entry, now);
                close = true;
            }
        }
        if close {
            self.recovery = None;
        } else {
            self.recovery = Some(state);
        }
    }

    // =====================================================================
    // 确认 / 兼容警告 / 笔刷包 readme / 文件路径
    // =====================================================================

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.confirm.take() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let mut run = false;
        let mut cancel = false;
        let secs = self.doc.unsaved_painting_time.max(0.0);
        let risk = autosave::human_time_period_abbr(std::time::Duration::from_secs_f64(secs));
        egui::Modal::new(egui::Id::unique("pigment_confirm"))
            .frame(theme::modal_frame())
            .backdrop_color(egui::Color32::from_black_alpha(120))
            .show(ctx, |ui| {
                ui.set_width(420.0);
                dialog_heading(ui, &state.title, "未保存的作画有丢失风险");
                ui.label(
                    egui::RichText::new(format!("你可能会丢失约 {risk} 的未保存作画。"))
                        .size(12.0)
                        .color(theme::TEXT_DIM),
                );
                ui.add_space(6.0);
                let mut save = state.save_to_scraps;
                if theme::check_row(ui, "先把当前画面保存到 Scraps", &mut save).changed() {
                    state.save_to_scraps = save;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(&state.confirm).clicked() {
                            run = true;
                        }
                    });
                });
            });
        if run {
            if state.save_to_scraps {
                self.save_scrap(now);
            }
            match state.action {
                ConfirmAction::OpenScrap(path) => self.open_scrap_now(&path, now),
                ConfirmAction::Reload(path) => self.open_from_path(&path, now),
                ConfirmAction::Quit => self.quit_now(),
            }
        } else if cancel {
            self.set_status("已取消".to_string(), now);
        } else {
            self.confirm = Some(state);
        }
    }

    fn file_warning_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.file_warning.take() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let mut open_anyway = false;
        let mut cancel = false;
        let key = match state.level {
            pigment_engine::Compatibility::Partially => "ui.file_compat_warning_mild",
            _ => "ui.file_compat_warning_severe",
        };
        egui::Modal::new(egui::Id::unique("pigment_file_warning"))
            .frame(theme::modal_frame())
            .backdrop_color(egui::Color32::from_black_alpha(120))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                dialog_heading(ui, "文件兼容性警告", "文件可能无法完全保真");
                let name = state
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let msg = match state.level {
                    pigment_engine::Compatibility::Partially => format!(
                        "“{name}”可能无法与 MyPaint {} 完全兼容。",
                        pigment_engine::fileio::MYPAINT_VERSION
                    ),
                    _ => format!(
                        "“{name}”由更新版本的 MyPaint 创建，可能无法与 MyPaint {} 兼容。",
                        pigment_engine::fileio::MYPAINT_VERSION
                    ),
                };
                ui.label(egui::RichText::new(msg).size(12.0).color(theme::TEXT_DIM));
                ui.add_space(4.0);
                let mut skip = state.skip;
                if theme::check_row(ui, "不再显示此类警告", &mut skip).changed() {
                    state.skip = skip;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("仍然打开").clicked() {
                            open_anyway = true;
                        }
                    });
                });
            });
        if cancel {
            self.set_status("已取消".to_string(), now);
            return;
        }
        if !open_anyway {
            self.file_warning = Some(state);
            return;
        }
        if state.skip {
            self.prefs.set_bool(key, false);
            let _ = self.prefs.save();
        }
        self.skip_file_warning_once = true;
        let path = state.path;
        self.open_from_path(&path, now);
    }

    fn brushpack_readme_dialog(&mut self, ctx: &egui::Context) {
        let Some((name, readme)) = self.brushpack_readme.take() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let mut import = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("pigment_brushpack_readme"))
            .frame(theme::modal_frame())
            .backdrop_color(egui::Color32::from_black_alpha(120))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                dialog_heading(ui, "导入笔刷包", &name);
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&readme)
                                .monospace()
                                .size(10.5)
                                .color(theme::TEXT_DIM),
                        );
                    });
                theme::hline(ui, theme::LINE);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("导入").clicked() {
                            import = true;
                        }
                    });
                });
            });
        if cancel {
            self.brushpack_pending = None;
            self.set_status("已取消笔刷包导入".to_string(), now);
            return;
        }
        if import {
            if let Some((path, pack)) = self.brushpack_pending.take() {
                self.install_brushpack(&path, pack, now);
            }
            return;
        }
        self.brushpack_readme = Some((name, readme));
    }

    fn show_file_prompt(&mut self, ctx: &egui::Context) {
        let Some(mut prompt) = self.file_prompt.take() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut press_ok = false;
        let mut cancel = escape;
        egui::Modal::new(egui::Id::unique("pigment_file_prompt"))
            .frame(theme::modal_frame())
            .backdrop_color(egui::Color32::from_black_alpha(120))
            .show(ctx, |ui| {
                ui.set_width(560.0);
                dialog_heading(ui, prompt_title(prompt.action), "输入完整路径");
                ui.label(
                    egui::RichText::new(match prompt.action {
                        FileAction::Open => "要打开的文件路径（.ora / .png / .jpg）",
                        FileAction::Export => "导出 PNG 的路径",
                        FileAction::SaveAs => "保存路径（不带后缀时按默认格式补全）",
                        FileAction::ImportLayers => "要导入的图层文件，多个路径用分号或换行分隔",
                        FileAction::ScratchpadOpen => "要打开到暂存区的文件路径",
                        FileAction::ScratchpadSaveAs => "暂存页另存的路径",
                        FileAction::PaletteOpen => "要导入的 GIMP 调色板路径（.gpl）",
                        FileAction::PaletteSaveAs => "导出调色板的路径（.gpl）",
                        FileAction::BrushpackOpen => "要导入的笔刷包路径（.zip）",
                        FileAction::BrushpackExport => "导出笔刷包的路径（.zip）",
                        FileAction::MaskSave => "保存遮罩的 GIMP 调色板路径（.gpl）",
                        FileAction::MaskLoad => "要载入的 GIMP 调色板路径（.gpl）",
                    })
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
                );
                ui.add_space(5.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut prompt.path)
                        .desired_width(f32::INFINITY)
                        .hint_text(r"C:\path\to\picture.ora"),
                );
                if prompt.want_focus {
                    resp.request_focus();
                    prompt.want_focus = false;
                }
                if let Some(err) = &prompt.error {
                    ui.add_space(3.0);
                    ui.label(egui::RichText::new(err).size(11.0).color(theme::ERROR));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("确定").clicked() {
                        press_ok = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(theme::faint("Enter = 确定  ·  Esc = 取消"));
                    });
                });
            });
        if cancel {
            self.set_status("已取消".to_string(), now);
            return;
        }
        self.file_prompt = Some(prompt);
        if enter || press_ok {
            self.apply_file_prompt(now);
        }
    }
}

fn dialog_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(16.0)
            .strong()
            .color(theme::TEXT),
    );
    ui.label(
        egui::RichText::new(subtitle)
            .size(10.5)
            .color(theme::TEXT_FAINT),
    );
    ui.add_space(4.0);
    theme::hline(ui, theme::LINE);
    ui.add_space(7.0);
}

fn prompt_title(action: FileAction) -> &'static str {
    match action {
        FileAction::SaveAs => "另存为",
        FileAction::Open => "打开",
        FileAction::ImportLayers => "导入图层",
        FileAction::Export => "导出图像",
        FileAction::ScratchpadOpen => "打开暂存页",
        FileAction::ScratchpadSaveAs => "暂存页另存为",
        FileAction::PaletteOpen => "导入调色板",
        FileAction::PaletteSaveAs => "导出调色板",
        FileAction::BrushpackOpen => "导入笔刷包",
        FileAction::BrushpackExport => "导出分组笔刷包",
        FileAction::MaskSave => "保存遮罩为 GIMP 调色板",
        FileAction::MaskLoad => "从 GIMP 调色板载入遮罩",
    }
}

fn load_thumb(ctx: &egui::Context, path: &std::path::Path) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let pixels = img.into_raw();
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(ctx.load_texture(
        format!("dialog-thumb-{}", path.display()),
        color,
        egui::TextureOptions::LINEAR,
    ))
}
