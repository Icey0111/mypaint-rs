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

use crate::app::PigmentApp;
use crate::shell::menubar::MenuNode;

use super::TOPBAR_MIN_HEIGHT;

impl PigmentApp {
    /// 顶栏整体 —— 菜单栏行 + 工具条行。
    ///
    /// 全屏态按偏好隐藏（`ui.hide_menubar_in_fullscreen` /
    /// `ui.hide_toolbar_in_fullscreen` 出厂均为 True，`userconfig.py:70-72`）。
    /// 源码的全屏形态还有 FakeMenuButton（菜单项塞进 toolbar1 首位）与
    /// workspace 的边缘碰撞自动隐藏 —— **记账未移植**，这里直接整条隐藏。
    pub(crate) fn topbar(&mut self, ui: &mut egui::Ui) {
        if self.fullscreen {
            return;
        }
        egui::Panel::top("topbar")
            // 顶栏高度按内容走（源码里 `height_request=24` 只是下界）
            .resizable(false)
            .show(ui, |ui| {
                ui.set_min_height(TOPBAR_MIN_HEIGHT);
                self.menubar_row(ui);
                self.toolbar_row(ui);
            });
    }

    /// 菜单栏行（S3）—— `gui/menu.xml` 的树渲染。
    ///
    /// 树由 `build.rs` 从 menu.xml 生成（每条动作 id 都在构建期校验过存在）；
    /// 条目的 label / tooltip / 加速键显示文本查 S2 的动作表，动态槽位动作
    /// （`Context0N`）的文案照搬 `document.py:467-492` 的注册元组。
    /// 未实现的动作置灰（不消费点击），与 GTK insensitive 语义一致。
    fn menubar_row(&mut self, ui: &mut egui::Ui) {
        use crate::shell::menubar::MENUBAR;
        egui::MenuBar::new().ui(ui, |ui| {
            for node in MENUBAR {
                self.menu_node(ui, node);
            }
        });
    }

    /// 递归渲染一个菜单树节点。
    ///
    /// 也供 `ShowPopupMenu` 的浮动主菜单复用（`gui/resources.xml:1311-1316`
    /// 的 `popupmenu_show_cb` —— 源码弹出的就是同一棵 `menu.xml` 树）。
    pub(crate) fn menu_node(&mut self, ui: &mut egui::Ui, node: &MenuNode) {
        match node {
            MenuNode::Separator => {
                ui.separator();
            }
            MenuNode::Item(id) => self.menu_item(ui, id),
            MenuNode::SubMenu { action, items } => {
                // 子菜单标题取动作 label（menu.xml 无 tooltip/快捷键，见
                // resources.xml 里菜单动作的注释），不置灰（GTK 的菜单标题
                // 只在整组动作 sensitive=false 时才灰，MyPaint 没有这种用法）
                let label = crate::shell::menubar::item_info(action).label;
                ui.menu_button(crate::i18n::tr(&label), |ui| {
                    for n in items.iter() {
                        self.menu_node(ui, n);
                    }
                });
            }
        }
    }

    /// 渲染并处理一条菜单条目。
    fn menu_item(&mut self, ui: &mut egui::Ui, id: &'static str) {
        if id == "OpenRecent" {
            self.open_recent_menu(ui);
            return;
        }
        if id == "OpenLast" {
            let info = crate::shell::menubar::item_info(id);
            let available = !self.recent_files().is_empty();
            let resp = crate::shell::menubar::item(
                ui,
                crate::i18n::tr(&info.label),
                info.accel.as_deref(),
                available,
                self.toggle_state(id),
            );
            if available && resp.clicked() {
                let now = ui.input(|i| i.time);
                self.open_recent(0, now);
                ui.close();
            }
            return;
        }
        let info = crate::shell::menubar::item_info(id);
        let available = self.action_available(id);
        let resp = crate::shell::menubar::item(
            ui,
            crate::i18n::tr(&info.label),
            info.accel.as_deref(),
            available,
            self.toggle_state(id),
        );
        // tooltip 只给可用的条目（GTK 对 insensitive 项不弹 tooltip）
        if available {
            if let Some(tip) = &info.tooltip {
                // on_hover_text 消费 Response，用 clone 保住后面的点击判定
                let _ = resp.clone().on_hover_text(crate::i18n::tr(tip));
            }
        }
        if available && resp.clicked() {
            let ctx = ui.ctx().clone();
            let now = ui.input(|i| i.time);
            self.run_action(id, &ctx, now);
            ui.close();
        }
    }

    /// `File → Open Recent` 子菜单（`GtkRecentAction`，`filehandling.py:1497`）。
    fn open_recent_menu(&mut self, ui: &mut egui::Ui) {
        let label = crate::shell::menubar::item_info("OpenRecent").label;
        ui.menu_button(crate::i18n::tr(&label), |ui| {
            let files = self.recent_files().to_vec();
            if files.is_empty() {
                ui.add_enabled(false, egui::Button::new("No Recent Files"));
                return;
            }
            let mut pick = None;
            for (i, p) in files.iter().enumerate() {
                let name = std::path::Path::new(p)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(p);
                if ui.button(name).on_hover_text(p).clicked() {
                    pick = Some(i);
                }
            }
            if let Some(i) = pick {
                let now = ui.input(|i| i.time);
                self.open_recent(i, now);
                ui.close();
            }
        });
    }

    /// 工具条行 —— 左 toolbar1 / 右 toolbar2；整行右键弹片段设置菜单。
    fn toolbar_row(&mut self, ui: &mut egui::Ui) {
        let container = ui.horizontal(|ui| {
            self.toolbar1_row(ui);
            // toolbar2 的 `hexpand=False`：贴在右侧（源码里它不抢横向空间）
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.toolbar2_row(ui);
            });
        });
        // 右键弹片段设置菜单（toolbar.py:101-108 的 popup-context-menu）。
        // `container_context_menu` 连按钮上的右键也算（与 GTK 一致）。
        container
            .response
            .container_context_menu(|ui| self.fragment_settings_menu(ui));
    }

    /// `toolbar1-settings-menu` 的内容 —— 8 个片段开关
    /// （`toolbar.py:110-126`：勾选 = merge 进 toolbar1，取消 = remove）。
    fn fragment_settings_menu(&mut self, ui: &mut egui::Ui) {
        use crate::shell::toolbars::FRAGMENTS;
        for (i, frag) in FRAGMENTS.iter().enumerate() {
            let mut on = self.toolbar_fragments[i];
            if ui.checkbox(&mut on, frag.label).changed() {
                self.toolbar_fragments[i] = on;
                // `ui.toolbar_items` 立即写回（源码的 toggle 动作最终落到偏好，
                // 退出时统一 save_gui_config）。
                let fragments = self.toolbar_fragments;
                crate::shell::toolbars::fragments_to_prefs(&mut self.prefs, &fragments);
            }
        }
    }

    /// `toolbar1` —— 主工具条（S4：数据驱动）。
    ///
    /// 有效条目 = 框架（`toolbar.xml`）+ 按偏好开关合并的片段；渲染为
    /// 24×24 图标按钮（Gtk.ToolbarStyle.ICONS），组间分隔线只留白
    /// （`toolbar.py:68-70` 的 `set_draw(False)`）。
    fn toolbar1_row(&mut self, ui: &mut egui::Ui) {
        use crate::shell::toolbars::{self, TbNode, TOOLBAR1_FRAMEWORK};
        let nodes = toolbars::expand(TOOLBAR1_FRAMEWORK, &self.toolbar_fragments);
        ui.horizontal(|ui| {
            for node in &nodes {
                match node {
                    TbNode::Separator => {
                        // 源码 set_draw(False)：不画线，只留组间距
                        ui.add_space(6.0);
                    }
                    TbNode::Item(id) => self.tool_item(ui, id),
                    TbNode::Placeholder { .. } => unreachable!("expand 不产出占位符"),
                }
            }
        });
    }

    /// 渲染并处理一个 toolitem（数据快照 → [`toolbars::tool_button`]）。
    fn tool_item(&mut self, ui: &mut egui::Ui, id: &'static str) {
        use crate::shell::toolbars::{tool_button, ToolBtn};
        let available = self.action_available(id);
        let pressed = self.toolbar_pressed(id);
        let action = crate::shell::actions::by_id(id);
        let tip = action.map(|a| {
            let mut t =
                crate::i18n::tr(a.effective_label().unwrap_or(id)).to_string();
            if let Some(acc) = a.accel {
                t.push_str(&format!("  ({})", acc.label()));
            }
            t
        });
        let tip = if available { tip } else { None };
        let clicked = tool_button(
            ui,
            &mut self.icon_cache,
            ToolBtn {
                available,
                pressed,
                icon_name: action.and_then(|a| a.icon_name),
                tip,
                fallback_label: action
                    .and_then(|a| a.toolbar_label())
                    .map(crate::i18n::tr)
                    .unwrap_or(id)
                    .to_string(),
            },
            crate::shell::toolbars::icon_size(&self.prefs),
        );
        if clicked {
            let ctx = ui.ctx().clone();
            let now = ui.input(|i| i.time);
            self.run_action(id, &ctx, now);
        }
    }

    /// `toolbar2` —— 面板开关与全屏（S4：数据驱动）。
    ///
    /// 源码内容（`toolbar.xml:47-58`）：`ColorAdjusters` `BrushGroups` │
    /// `ToggleModeOptionsTool` `ToggleLayersTool` `ToggleScratchpadTool`
    /// `TogglePreviewTool` `ToggleHistoryPanel` │ `Fullscreen`。
    /// 外层容器是 right_to_left 布局（toolbar2 贴右），所以**逆序**迭代
    /// 得到与 toolbar.xml 相同的从左到右顺序；本项目新增的诊断读数
    /// （MyPaint 无此控件）画在最左。
    fn toolbar2_row(&mut self, ui: &mut egui::Ui) {
        use crate::shell::toolbars::{TbNode, TOOLBAR2};
        // 诊断读数（最先画 = RTL 里最靠右？不——RTL 首个在最右；
        // 诊断放最左 = 最后画）
        let draw_diag = |ui: &mut egui::Ui, self_: &mut Self| {
            ui.separator();
            ui.label(format!(
                "半径 {:.0}px",
                self_
                    .brush
                    .get(pigment_engine::hokusai::BrushSetting::Radius)
                    .base_value
                    .exp()
            ));
            let resident = self_.renderer.lock().unwrap().resident();
            // B2-6：组没有表面 → 统计整棵树（各绘画层相加）
            let (tiles, bytes) = self_
                .doc
                .layers()
                .walk()
                .into_iter()
                .filter_map(|(_, l)| l.surface())
                .fold((0usize, 0usize), |(t, b), s| {
                    (t + s.tile_count(), b + s.memory_bytes())
                });
            ui.label(format!(
                "tiles {} · {:.1} MiB · 显存 {resident}",
                tiles,
                bytes as f64 / 1048576.0
            ));
        };
        // RTL：先画的在最右。诊断最左 → 最后画；工具条条目逆序迭代。
        fn node_rev(ui: &mut egui::Ui, app: &mut PigmentApp, node: &TbNode) {
            match node {
                TbNode::Separator => ui.add_space(6.0),
                TbNode::Item(id) => app.tool_item(ui, id),
                TbNode::Placeholder { items, .. } => {
                    for n in items.iter().rev() {
                        match n {
                            TbNode::Item(id) => app.tool_item(ui, id),
                            TbNode::Separator => ui.add_space(6.0),
                            TbNode::Placeholder { .. } => unreachable!("toolbar2 无嵌套占位符"),
                        }
                    }
                }
            }
        }
        for node in TOOLBAR2.iter().rev() {
            node_rev(ui, self, node);
        }
        draw_diag(ui, self);
    }
}
