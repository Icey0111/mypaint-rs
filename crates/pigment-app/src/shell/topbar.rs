//! 顶栏 —— `MyPaintTopBar`（`gui/topbar.py:26` + `gui/mypaint.glade:15`）。
//!
//! 源码里常规（非全屏）形态是一个 `Gtk.Grid`：
//!
//! ```text
//! 行 0:  [            菜单栏（跨 2 列）            ]
//! 行 1:  [ toolbar1（hexpand=True） ][ toolbar2 ]
//! ```
//!
//! 全屏态会**改结构**：菜单栏被拆成一个个菜单项塞进 `FakeMenuButton`
//! （`_toplevel_state_event_cb`，`topbar.py:129`），菜单按钮插到 toolbar1 首位，
//! 两个工具条被整体搬到行 0。该分支**尚未移植**，等 S3 菜单栏 + S4 工具条
//! 都到位后一起做（它需要菜单项能被"搬家"，而菜单项此时才存在）。
//!
//! 顶栏三块的分工与落地顺序（见《MyPaint移植路线图.md》第三节）：
//! - [`PigmentApp::menubar_row`] —— S3 菜单栏（`gui/menu.xml`）
//! - [`PigmentApp::toolbar1_row`] —— 主工具条，当前承载已有的文件/编辑/笔刷按钮
//! - [`PigmentApp::toolbar2_row`] —— S4 的 `toolbar2`（面板开关 + 全屏）

use eframe::egui;

use crate::app::{FileAction, FilePrompt, PigmentApp};

use super::TOPBAR_MIN_HEIGHT;

impl PigmentApp {
    /// 顶栏整体 —— 菜单栏行 + 工具条行。
    pub(crate) fn topbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("topbar")
            // 顶栏高度按内容走（源码里 `height_request=24` 只是下界）
            .resizable(false)
            .show(ui, |ui| {
                ui.set_min_height(TOPBAR_MIN_HEIGHT);
                self.menubar_row(ui);
                self.toolbar_row(ui);
            });
    }

    /// 菜单栏行。
    ///
    /// 源码是 `gui/menu.xml` 声明的 9 个顶级菜单，动作定义在
    /// `gui/resources.xml`（161 个 `GtkAction` + 105 条加速键）。
    /// **S3 落地前这里不画任何东西** —— 画一行空菜单栏既占地方又没有意义，
    /// 而这个函数的存在保证了结构接缝已经就位。
    fn menubar_row(&mut self, _ui: &mut egui::Ui) {
        // S3：按 menu.xml 的菜单树渲染，未实现的动作置灰。
    }

    /// 工具条行 —— 左 toolbar1 / 右 toolbar2。
    fn toolbar_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.toolbar1_row(ui);
            // toolbar2 的 `hexpand=False`：贴在右侧（源码里它不抢横向空间）
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.toolbar2_row(ui);
            });
        });
    }

    /// `toolbar1` —— 主工具条。
    ///
    /// 源码由 `gui/toolbar.xml` 的框架 + 8 个可开关片段
    /// （`toolbar-file.xml` / `toolbar-scrap.xml` / …，见 `gui/toolbar.py:26`
    /// 的 `MERGEABLE_XML`）拼成。S4 会把它改成"数据驱动 + 片段开关"；
    /// 当下先把已经能用的动作摆上，条目集合与源码的对应关系逐个标在下面。
    fn toolbar1_row(&mut self, ui: &mut egui::Ui) {
        // ⚠️ 本项目新增：MyPaint 没有"显示/隐藏面板"的按钮（面板开关是 toolbar2
        // 的 `Toggle*Tool` 动作族）。S4/S6 到位后这枚 ☰ 应当被替换掉。
        if ui
            .selectable_label(self.show_panels, "☰")
            .on_hover_text("显示/隐藏面板（Tab）")
            .clicked()
        {
            self.show_panels = !self.show_panels;
        }
        ui.separator();

        // ---- file-toolitems（`toolbar-file.xml`：New / Open / Save）----
        // 源码里还有 SaveAs / Export 的菜单项，工具条片段只放这三个。
        // 加速键见 `gui/resources.xml`（Ctrl+N / Ctrl+O / Ctrl+S）。
        let now = ui.input(|i| i.time);
        if ui.button("新建").on_hover_text("新建文档（Ctrl+N）").clicked() {
            self.new_document(now);
        }
        if ui
            .button("打开")
            .on_hover_text("打开 .ora / .png（Ctrl+O）")
            .clicked()
        {
            self.file_prompt = Some(FilePrompt::new(FileAction::Open, None));
        }
        if ui
            .button("保存")
            .on_hover_text("保存（Ctrl+S；未命名时等同另存为）")
            .clicked()
        {
            self.save_command(now);
        }
        // SaveAs / Export 在源码里只出现在菜单栏，工具条片段没有 —— 但在 S3
        // 菜单栏落地前，删掉它们会让功能完全没有入口，所以暂留（标注为权宜）。
        if ui
            .button("另存为")
            .on_hover_text("另存为（Ctrl+Shift+S）· 源码里只在菜单栏")
            .clicked()
        {
            self.file_prompt = Some(FilePrompt::new(
                FileAction::SaveAs,
                self.current_file.as_deref(),
            ));
        }
        if ui
            .button("导出")
            .on_hover_text("导出 PNG（Ctrl+Shift+E）· 源码里只在菜单栏")
            .clicked()
        {
            let seed = self.current_file.as_ref().map(|p| p.with_extension("png"));
            self.file_prompt = Some(FilePrompt::new(FileAction::Export, seed.as_deref()));
        }
        ui.separator();

        // ---- edit-toolitems（`toolbar-edit.xml`：Undo / Redo）----
        // 文案与置灰规则照搬 `gui/document.py:582-591` 的 `_update_undo_redo`：
        // 有命令时显示 "Undo <命令名>"，否则 "Undo"。
        let undo_label = match self.doc.get_last_command() {
            Some(cmd) => format!("↶ 撤销 {}", cmd.display_name()),
            None => "↶ 撤销".to_string(),
        };
        let redo_label = match self.doc.command_stack.next_redo_name() {
            Some(name) => format!("↷ 重做 {name}"),
            None => "↷ 重做".to_string(),
        };
        let can_undo = self.doc.command_stack.undo_len() > 0;
        let can_redo = self.doc.command_stack.redo_len() > 0;
        if ui
            .add_enabled(can_undo, egui::Button::new(undo_label))
            .on_hover_text("撤销（Ctrl+Z / z）")
            .clicked()
        {
            self.undo(now);
        }
        if ui
            .add_enabled(can_redo, egui::Button::new(redo_label))
            .on_hover_text("重做（Ctrl+Shift+Z / Ctrl+Y / y）")
            .clicked()
        {
            self.redo(now);
        }
        ui.separator();

        // ---- blendmodes-toolitems（`toolbar-blendmodes.xml`）----
        // 片段里的条目是 BlendModeEraser / BlendModeLockAlpha / BlendModeColorize，
        // 而框架 `toolbar.xml` 默认只把 Eraser 放上去。这里的 `E 橡皮` 正是
        // `BlendModeEraser`；LockAlpha / Colorize 属 B2（图层与混合模式）。
        if ui.selectable_label(self.eraser, "E 橡皮").clicked() {
            self.eraser = !self.eraser;
            self.derive_active();
        }
        let mut paint = self.paint_active();
        if ui
            .checkbox(&mut paint, "P 光谱混色")
            .on_hover_text("关掉即退回 RGB 插值，便于并排对比")
            .changed()
        {
            self.paint_override = Some(paint);
            self.derive_active();
        }
        ui.separator();

        // ---- 本项目新增的诊断读数（MyPaint 无此控件）----
        ui.label(format!(
            "半径 {:.0}px",
            self.brush.get(pigment_engine::hokusai::BrushSetting::Radius).base_value.exp()
        ));
        let resident = self.renderer.lock().unwrap().resident();
        ui.label(format!(
            "tiles {} · {:.1} MiB · 显存 {resident}",
            self.doc.surface().tile_count(),
            self.doc.surface().memory_bytes() as f64 / 1048576.0
        ));
    }

    /// `toolbar2` —— 面板开关与全屏。
    ///
    /// 源码内容（`toolbar.xml:47`）：
    /// `ColorAdjusters` `BrushGroups` │ `ToggleModeOptionsTool` `ToggleLayersTool`
    /// `ToggleScratchpadTool` `TogglePreviewTool` `ToggleHistoryPanel` │ `Fullscreen`。
    /// 其中 `Fullscreen` 已经能用（对应本项目现有的 F11），但**面板类动作要等
    /// S6 的停靠面板系统**才有作用对象，所以 S4 之前此处留空。
    fn toolbar2_row(&mut self, _ui: &mut egui::Ui) {
        // S4：按 toolbar.xml 的 toolbar2 排布，未实现的动作置灰。
    }
}
