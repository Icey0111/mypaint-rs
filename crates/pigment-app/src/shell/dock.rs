//! 可停靠面板系统 —— `gui/workspace.py` + `gui/toolstack.py` 的承接（S6）。
//!
//! 源码模型（照搬的关键语义）：
//!
//! - 工作区 = 中央画布 + 左右两条 **ToolStack** 侧栏；每条 ToolStack 是
//!   纵向的 `Paned` 树，叶节点是 `Gtk.Notebook`（"组"：一组带标签页的
//!   面板）；面板（tool widget）是某个组里的一页。
//! - **添加面板**（`Workspace.add_tool_widget`，workspace.py:428-471）：
//!   按 **浮动窗口 → 右栈 → 左栈** 的顺序找第一个放得下的组
//!   （组内页数 < maxpages，从 1 逐步放宽；每栈最多 3 个组，
//!   `maxnotebooks=3`），找到后 reveal（切到该页、展开所在侧栏）。
//!   浮动窗口只算**已经存在的**（用户拖出标签才有，workspace.py:649-）；
//!   没有浮动窗口时新面板落到右侧栏 —— B4-3 修正了 `add` 之前会给任何新
//!   面板新建一个浮动窗口的偏差。
//! - **移除**：`toggle_dockpanel_cb`（drawwindow.py:352-367）——
//!   `Toggle*Tool` 动作 active 时 add、inactive 时 remove；组空了整个
//!   侧栏收起（`_sidebar_stack_hide_cb`，workspace.py:685）。
//! - 面板的标题/图标/描述由各面板类的 `tool_widget_title` 等属性给出。
//!
//! **技术栈适配**（记录在《MyPaint移植路线图.md》）：
//! - 计划中的 `egui_dock` 0.21.1 只支持 egui ^0.36，与本项目锁定的
//!   egui 0.37 不兼容（egui/wgpu 版本是里程碑间统一升级的），改为
//!   egui 原生实现同样的模型：侧栏 = 组列表，组 = 标签页面板集合；
//!   组间分隔条可拖（对应 Paned）。
//! - 标签页暂不支持**拖拽换组/拖出浮动**（GTK 的 tab drag + ToolStackWindow），
//!   以"标签右键 → 弹出为浮动窗口 / 收回侧栏"的菜单替代；
//!   浮动组用 `egui::Window` 承接。
//! - 布局序列化（`get_layout` / `build_from_layout`）与偏好持久化一起归 B6。
//! - `FullscreenAutohide`（全屏下边缘碰撞自动隐藏侧栏）属全屏交互，
//!   本轮只做全屏时隐藏顶栏（偏好 `ui.hide_menubar_in_fullscreen` /
//!   `ui.hide_toolbar_in_fullscreen` 默认均为 True），自动隐藏记账。

use eframe::egui;

use crate::app::PigmentApp;

/// 面板 id —— 对应 MyPaint 的 GType 名（去 `MyPaint` 前缀）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    /// `MyPaintBrushGroupTool` —— 笔刷组面板（`gui/brushselectionwindow.py:347`）。
    /// 源码里每组一个实例（标题 = 组名）；本项目一个面板显示全部组。
    BrushGroups,
    /// 笔刷编辑器。**应用本地**：MyPaint 里它是独立窗口
    /// （`gui/brusheditor.py`，动作 `BrushEditorWindow`），不是停靠面板；
    /// 本项目按停靠面板放在右侧（差异记账）。
    BrushEditor,
    /// `MyPaintModeOptionsTool` —— 工具选项（`gui/optionspanel.py:27`）。
    ModeOptionsTool,
    /// `MyPaintLayersTool` —— 图层（`gui/layerswindow.py:99`）。
    LayersTool,
    /// `MyPaintScratchpadTool` —— 暂存区（`gui/scratchwindow.py:28`）。
    ScratchpadTool,
    /// `MyPaintPreviewTool` —— 预览（`gui/previewwindow.py:183`）。
    PreviewTool,
    /// `MyPaintHistoryPanel` —— 最近笔刷与颜色（`gui/history.py:220`）。
    HistoryPanel,
    /// `MyPaintComponentSlidersTool` —— 分量滑条颜色器
    /// （`gui/colortools.py:99-101` 的 `ComponentSlidersTool` 动作，
    /// 页面 `gui/colors/sliders.py:25-160` 的 `ComponentSlidersAdjusterPage`）。
    ///
    /// 源码里颜色器是**一页一个工具**（`_PageToolAdapter`，`gui/colortools.py:31-101`），
    /// 本块只注册了分量滑条这一页；其余五页在面板内以禁用页签表示
    /// （见 `shell/color_panel.rs` 的 `COLOR_PAGES`）。
    ComponentSlidersTool,
}

impl PanelId {
    /// 标签页标题 —— 各面板类的 `tool_widget_title`。
    pub fn title(self) -> &'static str {
        match self {
            // optionspanel.py:45（"Tool Options"）、layerswindow.py:105（"Layers"）、
            // previewwindow.py:200（"Preview"）、scratchwindow.py:34（"Scratchpad"）、
            // history.py:225（"Recent Brushes & Colors"）
            PanelId::ModeOptionsTool => "Tool Options",
            PanelId::LayersTool => "Layers",
            PanelId::PreviewTool => "Preview",
            PanelId::ScratchpadTool => "Scratchpad",
            PanelId::HistoryPanel => "Recent Brushes & Colors",
            // 分量滑条页的 `get_page_title()`（sliders.py:111-113），也是
            // `resources.xml:1435` 的 `ComponentSlidersTool` 动作 label
            PanelId::ComponentSlidersTool => "Component Sliders",
            // BrushGroupTool 的标题 = 组名（brushselectionwindow.py:384）；
            // 本项目单面板显示全部组，用工具条动作的 label
            PanelId::BrushGroups => "Brush Groups",
            // 应用本地（MyPaint 的独立窗口标题语义）
            PanelId::BrushEditor => "Brush Settings Editor",
        }
    }

    /// 标签页图标 —— `tool_widget_icon_name`。
    pub fn icon_name(self) -> &'static str {
        match self {
            // optionspanel.py:44 / layerswindow.py:104 / previewwindow.py:197 /
            // scratchwindow.py:35 / history.py:224
            PanelId::ModeOptionsTool => "mypaint-options-symbolic",
            PanelId::LayersTool => "mypaint-layers-symbolic",
            PanelId::PreviewTool => "mypaint-view-symbolic",
            PanelId::ScratchpadTool => "mypaint-scratchpad-symbolic",
            PanelId::HistoryPanel => "mypaint-history-symbolic",
            // 分量滑条的页面图标（sliders.py:107-109），同
            // `resources.xml:1434` 的 icon-name
            PanelId::ComponentSlidersTool => "mypaint-tool-component-sliders",
            // brushselectionwindow.py:398 的 fallback
            PanelId::BrushGroups => "mypaint-brushes-symbolic",
            // 应用本地
            PanelId::BrushEditor => "mypaint-edit-symbolic",
        }
    }

    /// 标签页描述 —— `tool_widget_description`（tooltip 第二行）。
    pub fn description(self) -> &'static str {
        match self {
            PanelId::ModeOptionsTool => "Specialized settings for the current editing tool",
            PanelId::LayersTool => "Arrange layers and assign effects",
            PanelId::PreviewTool => "Show preview of the whole drawing area",
            PanelId::ScratchpadTool => "Paint sketches and try things out",
            PanelId::HistoryPanel => "Recently used brushes and colors",
            // 页面描述（sliders.py:115-120），同 `resources.xml:1436` 的 tooltip
            PanelId::ComponentSlidersTool => "Adjust individual components of the color.",
            PanelId::BrushGroups => "List of brushes, grouped",
            PanelId::BrushEditor => "Adjust the current brush's settings",
        }
    }
}

/// 一"组"面板 = GTK 的一个 `Gtk.Notebook`：若干标签页 + 当前激活页。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub panels: Vec<PanelId>,
    pub active: usize,
}

/// 每栈组数上限 —— `Workspace.add_tool_widget` 传的 `maxnotebooks=3`。
pub const MAX_GROUPS_PER_STACK: usize = 3;

/// 停靠状态：左右两条侧栏（组的纵向列表）+ 浮动组。
#[derive(Clone, Debug, Default)]
pub struct DockState {
    pub left: Vec<Group>,
    pub right: Vec<Group>,
    /// 浮动组（`egui::Window` 承接；GTK 的 ToolStackWindow）。
    pub floating: Vec<Group>,
    /// 组间分隔条位置（纵向，单位 pt，按组索引存；None = 未手动调过，
    /// 平分剩余空间）。与侧栏组列表同长（浮动组不用）。
    pub left_heights: Vec<Option<f32>>,
    pub right_heights: Vec<Option<f32>>,
}

impl DockState {
    /// 面板当前是否已添加（在任何栈里）。对应 `get_tool_widget_added`。
    pub fn is_added(&self, panel: PanelId) -> bool {
        self.locate(panel).is_some()
    }

    /// 面板在哪里：(栈, 组下标, 页下标)。
    fn locate(&self, panel: PanelId) -> Option<(Stack, usize, usize)> {
        for (stack, groups) in [
            (Stack::Floating, &self.floating),
            (Stack::Right, &self.right),
            (Stack::Left, &self.left),
        ] {
            for (gi, g) in groups.iter().enumerate() {
                if let Some(pi) = g.panels.iter().position(|p| *p == panel) {
                    return Some((stack, gi, pi));
                }
            }
        }
        None
    }

    /// 移除面板（`remove_tool_widget`）。空组删除（GTK 里空 notebook 是
    /// 占位符，不显示页签；侧栏全空时渲染层收起 —— `_sidebar_stack_hide_cb`）。
    /// 返回是否移除了。
    pub fn remove(&mut self, panel: PanelId) -> bool {
        let mut removed = false;
        for groups in [&mut self.floating, &mut self.right, &mut self.left] {
            for g in groups.iter_mut() {
                if let Some(pi) = g.panels.iter().position(|p| *p == panel) {
                    g.panels.remove(pi);
                    g.active = g.active.min(g.panels.len().saturating_sub(1));
                    removed = true;
                }
            }
            groups.retain(|g| !g.panels.is_empty());
        }
        self.normalize_heights();
        removed
    }

    /// 添加面板 —— 复刻 `toolstack.py:673-721` 的 `add_tool_widget` 逐栈放宽：
    ///
    /// - 栈顺序 **浮动 → 右 → 左**（`_get_tool_stacks`，workspace.py:642-648）；
    ///   浮动栈只在**已有浮动窗口**时参与：源码里的浮动 `ToolStackWindow` 只能
    ///   由用户把标签拖出去产生（`_tool_tab_drag_begin_cb`，workspace.py:649-），
    ///   `add_tool_widget` 自己不会新建浮动窗口。B4-3 修正了这一点 —— 否则任何
    ///   面板第一次打开都会飘成独立窗口，而源码里它是停进右侧栏的。
    /// - 每轮 maxpages 从 1 逐步放宽，找**第一个页数 < maxpages 的组**；
    /// - 侧栏每栈末尾总有一个空占位组（GTK 的 placeholder notebook），等价于
    ///   "可新建组"——受 `maxnotebooks=3` 约束：**非空组**达到 3 个就不能再建；
    /// - 页数放宽到 3 仍放不下才考虑更松（源码放宽到 100，组数/页数实际上限
    ///   由 maxnotebooks 保证）。
    pub fn add(&mut self, panel: PanelId) {
        if self.is_added(panel) {
            return;
        }
        let mut maxpages = 1usize;
        while maxpages < 100 {
            for stack in [Stack::Floating, Stack::Right, Stack::Left] {
                let groups = self.stack_groups(stack);
                // 浮动"栈"只在有浮动窗口时才存在：源码里 `_get_tool_stacks()`
                // 只 yield 已经存在的 ToolStackWindow（workspace.py:642-648），
                // 而浮动窗口是用户拖出标签才产生的（workspace.py:649-）。
                let stack_exists = stack != Stack::Floating || !self.floating.is_empty();
                if !stack_exists {
                    continue;
                }
                // 已有组：页数 < maxpages 的第一个（空占位组不算，它在下面处理）
                let existing = groups
                    .iter()
                    .position(|g| !g.panels.is_empty() && g.panels.len() < maxpages);
                // 栈尾空占位组：可新建组的前提 = 非空组数 < maxnotebooks
                //
                // 注意 maxpages = 1 时**总**能新建组（空占位组的页数 0 < 1），
                // 所以新面板会先占新组，只有该栈的非空组到 3 个才轮到下一栈
                // （`toolstack.py:700-715`：先找 `get_n_pages() < maxpages` 的
                // notebook，找不到才 split 占位组）。
                let populated = groups.iter().filter(|g| !g.panels.is_empty()).count();
                let can_create = populated < MAX_GROUPS_PER_STACK;
                if let Some(gi) = existing.or(if can_create { Some(groups.len()) } else { None }) {
                    let groups = self.stack_groups_mut(stack);
                    if gi >= groups.len() {
                        groups.push(Group {
                            panels: vec![panel],
                            active: 0,
                        });
                    } else {
                        let g = &mut groups[gi];
                        g.panels.push(panel);
                        g.active = g.panels.len() - 1;
                    }
                    self.normalize_heights();
                    return;
                }
            }
            maxpages += 1;
        }
    }

    fn stack_groups(&self, stack: Stack) -> &[Group] {
        match stack {
            Stack::Left => &self.left,
            Stack::Right => &self.right,
            Stack::Floating => &self.floating,
        }
    }

    fn stack_groups_mut(&mut self, stack: Stack) -> &mut Vec<Group> {
        match stack {
            Stack::Left => &mut self.left,
            Stack::Right => &mut self.right,
            Stack::Floating => &mut self.floating,
        }
    }

    /// `reveal_tool_widget`：不在则添加，并把所在组切到该页。
    pub fn reveal(&mut self, panel: PanelId) {
        if !self.is_added(panel) {
            self.add(panel);
        }
        // 切到该页（nb.set_current_page）
        for groups in [&mut self.floating, &mut self.right, &mut self.left] {
            for g in groups.iter_mut() {
                if let Some(pi) = g.panels.iter().position(|p| *p == panel) {
                    g.active = pi;
                }
            }
        }
    }

    /// `toggle_dockpanel_cb`：开（add+reveal）或关（remove）。
    pub fn toggle(&mut self, panel: PanelId) {
        if self.is_added(panel) {
            self.remove(panel);
        } else {
            self.reveal(panel);
        }
    }

    fn normalize_heights(&mut self) {
        self.left_heights.resize(self.left.len(), None);
        self.right_heights.resize(self.right.len(), None);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stack {
    Left,
    Right,
    Floating,
}

/// 侧栏在屏幕的哪一侧（渲染用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn groups<'a>(&self, s: &'a DockState) -> &'a [Group] {
        match self {
            Side::Left => &s.left,
            Side::Right => &s.right,
        }
    }
    fn groups_mut<'a>(&self, s: &'a mut DockState) -> &'a mut Vec<Group> {
        match self {
            Side::Left => &mut s.left,
            Side::Right => &mut s.right,
        }
    }
    fn heights<'a>(&self, s: &'a DockState) -> &'a [Option<f32>] {
        match self {
            Side::Left => &s.left_heights,
            Side::Right => &s.right_heights,
        }
    }
    fn heights_mut<'a>(&self, s: &'a mut DockState) -> &'a mut Vec<Option<f32>> {
        match self {
            Side::Left => &mut s.left_heights,
            Side::Right => &mut s.right_heights,
        }
    }
}

impl DockState {
    /// 某侧的组列表（渲染用）。
    pub fn groups(&self, side: Side) -> &[Group] {
        side.groups(self)
    }

    /// 切到某组某页。
    pub fn set_active(&mut self, side: Side, gi: usize, pi: usize) {
        if let Some(g) = side.groups_mut(self).get_mut(gi) {
            g.active = pi;
        }
    }

    /// 把停靠在侧栏的某组弹出为浮动窗口。
    pub fn pop_out_group(&mut self, side: Side, gi: usize) {
        if gi < side.groups(self).len() {
            let g = side.groups_mut(self).remove(gi);
            self.floating.push(g);
            self.normalize_heights();
        }
    }
}

// ---- 渲染 ----

impl PigmentApp {
    /// 左右侧栏（按停靠状态决定显示与否；`_sidebar_stack_hide_cb`：
    /// 栈空了整个侧栏收起）。
    pub(crate) fn dock_sidebars(&mut self, ui: &mut egui::Ui) {
        let mut pick = false;
        if !self.dock.left.is_empty() {
            egui::Panel::left("dock_left")
                .resizable(true)
                .default_size(240.0)
                // 下界 = `TOOL_WIDGET_MIN_WIDTH`（`gui/toolstack.py:40`）
                .size_range(crate::brushes::TOOL_WIDGET_MIN_WIDTH..=520.0)
                .show(ui, |ui| pick = self.dock_sidebar(ui, Side::Left) || pick);
        }
        if !self.dock.right.is_empty() {
            egui::Panel::right("dock_right")
                .resizable(true)
                .default_size(310.0)
                .size_range(250.0..=520.0)
                .show(ui, |ui| pick = self.dock_sidebar(ui, Side::Right) || pick);
        }
        if pick {
            self.reload_selected_brush();
        }
    }

    /// 渲染一条侧栏：组的纵向堆叠（组间分隔条可拖），返回是否选中了笔刷。
    fn dock_sidebar(&mut self, ui: &mut egui::Ui, side: Side) -> bool {
        let mut pick = false;
        let groups = self.dock.groups(side).to_vec();
        if groups.is_empty() {
            return false;
        }
        let avail_h = ui.available_height();
        let n = groups.len();
        self.dock.normalize_heights();
        for (gi, group) in groups.iter().enumerate() {
            // 组高度：手动调过的用存值，否则平分
            let h_user = side.heights(&self.dock).get(gi).copied().flatten();
            let h = h_user.unwrap_or(avail_h / n as f32);
            let h = h.max(60.0);
            if gi > 0 {
                // 组间分隔条（GTK Paned 的 wide handle）
                let (_, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 6.0),
                    egui::Sense::drag(),
                );
                let resp = resp.on_hover_cursor(egui::CursorIcon::ResizeVertical);
                if resp.drag_delta().y != 0.0 {
                    let prev = side.heights(&self.dock).get(gi - 1).copied().flatten();
                    let base = prev.unwrap_or(h - 6.0);
                    side.heights_mut(&mut self.dock)[gi - 1] =
                        Some((base + resp.drag_delta().y).max(60.0));
                }
            }
            let rect = egui::Rect::from_min_size(
                ui.available_rect_before_wrap().min,
                egui::vec2(ui.available_width(), h.min(ui.available_height())),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                pick = self.dock_group(ui, group, side, Some((side, gi))) || pick;
            });
        }
        pick
    }

    /// 一组面板：标签条 + 当前页内容。
    ///
    /// `dock` = `(Side, 组下标)`（停靠在侧栏时）；`None` = 浮动窗口里。
    fn dock_group(
        &mut self,
        ui: &mut egui::Ui,
        group: &Group,
        _side: Side,
        dock: Option<(Side, usize)>,
    ) -> bool {
        let mut pick = false;
        let active = group.active;
        // ---- 标签条 ----
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            for (pi, panel) in group.panels.iter().enumerate() {
                let selected = pi == active;
                let tex = crate::shell::toolbars::icon_texture(
                    ui.ctx(),
                    &mut self.icon_cache,
                    panel.icon_name(),
                );
                let mut tab = match tex {
                    Some(t) => egui::Button::image(
                        egui::Image::new(&t).max_size(egui::vec2(16.0, 16.0)),
                    )
                    .selected(selected)
                    .small(),
                    None => egui::Button::new(panel.title()).selected(selected).small(),
                };
                tab = tab.min_size(egui::vec2(24.0, 20.0));
                let resp = ui.add(tab);
                let tip = format!("{}\n{}", panel.title(), panel.description());
                let _ = resp.clone().on_hover_text(tip);
                // 标签右键：弹出为浮动窗口（tab drag 的菜单替代，见模块文档）
                resp.context_menu(|ui| {
                    if dock.is_some() && ui.button("弹出为浮动窗口").clicked() {
                        let (side, gi) = dock.unwrap();
                        self.dock.pop_out_group(side, gi);
                        ui.close();
                    }
                });
                if resp.clicked() {
                    if let Some((side, gi)) = dock {
                        self.dock.set_active(side, gi, pi);
                    }
                }
            }
            // 关闭按钮（GTK notebook 的 close button）
            if ui.small_button("✕").on_hover_text("关闭面板").clicked() {
                if let Some(panel) = group.panels.get(active) {
                    let panel = *panel;
                    self.dock.remove(panel);
                }
            }
        });
        // ---- 当前页内容 ----
        if let Some(panel) = group.panels.get(active) {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match panel {
                    PanelId::BrushGroups => {
                        pick = self.brushes.show(ui);
                    }
                    PanelId::BrushEditor => self.brush_editor_content(ui),
                    PanelId::HistoryPanel => self.history_panel_content(ui),
                    PanelId::LayersTool => self.layers_panel_content(ui),
                    PanelId::ComponentSlidersTool => self.color_panel_content(ui),
                    other => placeholder_panel(ui, *other),
                });
        }
        pick
    }

    /// 浮动组（`egui::Window` 承接 GTK 的 `ToolStackWindow`）。
    pub(crate) fn dock_floatings(&mut self, ui: &mut egui::Ui) {
        let n = self.dock.floating.len();
        for gi in 0..n {
            let Some(group) = self.dock.floating.get(gi).cloned() else {
                continue;
            };
            // 标题 = 各面板标题 ", " 连接
            // （floating_window_title_separator 默认 ", "，workspace.py:117）
            let title = group
                .panels
                .iter()
                .map(|p| p.title())
                .collect::<Vec<_>>()
                .join(", ");
            let mut open = true;
            egui::Window::new(title)
                .id(egui::Id::unique(("dock_float", gi)))
                .open(&mut open)
                .default_width(280.0)
                .show(ui, |ui| {
                    self.dock_group(ui, &group, Side::Right, None);
                });
            if !open {
                // 关闭 = 收回右栈尾（GTK 的 delete-event 只是隐藏窗口；
                // 这里简化为放回侧栏，差异记账）
                let g = self.dock.floating.remove(gi);
                self.dock.right.push(g);
            }
        }
    }
}

/// 未实现面板的占位内容 —— 差异记账，不阻塞推进。
fn placeholder_panel(ui: &mut egui::Ui, panel: PanelId) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(egui::RichText::new(panel.title()).heading());
        ui.label(egui::RichText::new(panel.description()).weak());
        ui.add_space(8.0);
        let note = match panel {
            PanelId::LayersTool => "图层栈与 22 种混合模式属 B2（图层与混合模式）。",
            PanelId::ModeOptionsTool => "工具模式选项属 B3（工具模式）。",
            PanelId::PreviewTool => "整图预览依赖图层栈合成，属 B2。",
            PanelId::ScratchpadTool => "暂存区属 B5。",
            _ => "待移植。",
        };
        ui.label(egui::RichText::new(format!("此面板尚未移植。{note}")).small());
    });
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_docks_into_sidebars_and_never_creates_floating_windows() {
        // workspace.py:642-648 的 _get_tool_stacks 顺序是"浮动 → 右 → 左"，但
        // 浮动栈只能由用户拖出标签产生（workspace.py:649-），`add_tool_widget`
        // 不会新建浮动窗口 —— 所以新面板第一次打开落在**右侧栏**。
        let mut d = DockState::default();
        d.add(PanelId::ComponentSlidersTool);
        assert!(d.floating.is_empty(), "新增面板不得自建浮动窗口");
        assert_eq!(d.right.len(), 1, "右栈接收");
        assert_eq!(d.right[0].panels, vec![PanelId::ComponentSlidersTool]);
        d.add(PanelId::PreviewTool);
        assert_eq!(d.right.len(), 2, "maxpages=1 时新面板各占一组");
        // 已有浮动窗口（用户拖出的组）时，浮动态排在最前、可再开一个组
        // （`toolstack.py:700-715`：maxpages=1 时新面板落"空占位组"）；
        // 浮动窗口非空组满 3 个后才轮到右栈。
        let mut d2 = DockState::default();
        d2.floating.push(Group {
            panels: vec![PanelId::LayersTool],
            active: 0,
        });
        d2.add(PanelId::PreviewTool);
        assert_eq!(d2.floating.len(), 2, "浮动窗口里新开一个组");
        assert!(d2.right.is_empty(), "右栈此时不该被用到");
        // 浮动栈满 3 组 → 右栈
        let mut d3 = DockState::default();
        for _ in 0..MAX_GROUPS_PER_STACK {
            d3.floating.push(Group {
                panels: vec![PanelId::PreviewTool; 3],
                active: 0,
            });
        }
        d3.add(PanelId::LayersTool);
        assert_eq!(d3.right.len(), 1, "右栈接收");
        // 右栈也满 → 左栈
        for _ in 0..MAX_GROUPS_PER_STACK {
            d3.right.push(Group {
                panels: vec![PanelId::PreviewTool; 3],
                active: 0,
            });
        }
        d3.add(PanelId::ScratchpadTool);
        assert_eq!(d3.left.len(), 1, "左栈接收");
    }

    #[test]
    fn add_relaxes_maxpages_when_stacks_are_full() {
        // 三个栈的非空组都满 3 → 放宽 maxpages：先找 1 页的组，再 2 页……
        let mut d = DockState::default();
        let full = || Group {
            panels: vec![PanelId::PreviewTool; 3],
            active: 0,
        };
        for _ in 0..MAX_GROUPS_PER_STACK {
            d.floating.push(full());
            d.right.push(full());
        }
        d.left = vec![
            Group {
                panels: vec![PanelId::PreviewTool, PanelId::PreviewTool],
                active: 0,
            },
            Group {
                panels: vec![PanelId::PreviewTool],
                active: 0,
            },
            full(),
        ];
        d.add(PanelId::LayersTool);
        // maxpages=1：无空组；maxpages=2：左栈 1 页的组接收
        assert_eq!(d.left[1].panels.len(), 2, "页数放宽后落到 1 页的组");
        assert!(d.left[1].panels.ends_with(&[PanelId::LayersTool]));
    }

    #[test]
    fn remove_prunes_empty_groups() {
        let mut d = DockState::default();
        d.right = vec![
            Group { panels: vec![PanelId::LayersTool], active: 0 },
            Group { panels: vec![PanelId::PreviewTool], active: 0 },
        ];
        d.left_heights = vec![None, None];
        d.remove(PanelId::LayersTool);
        assert_eq!(d.right.len(), 1, "空组删除");
        assert_eq!(d.right[0].panels, vec![PanelId::PreviewTool]);
        assert_eq!(d.right_heights.len(), 1, "高度表随组数归一");
    }

    #[test]
    fn panel_metadata_follows_source() {
        // optionspanel.py:44-52 / layerswindow.py:104-106 / previewwindow.py:197-202 /
        // scratchwindow.py:34-36 / history.py:224-228 / brushselectionwindow.py:398
        assert_eq!(PanelId::ModeOptionsTool.title(), "Tool Options");
        assert_eq!(PanelId::ModeOptionsTool.icon_name(), "mypaint-options-symbolic");
        assert_eq!(PanelId::LayersTool.title(), "Layers");
        assert_eq!(PanelId::PreviewTool.title(), "Preview");
        assert_eq!(PanelId::ScratchpadTool.title(), "Scratchpad");
        assert_eq!(PanelId::HistoryPanel.title(), "Recent Brushes & Colors");
        assert_eq!(PanelId::HistoryPanel.icon_name(), "mypaint-history-symbolic");
        // B4-3：分量滑条页（sliders.py:107-120）
        assert_eq!(PanelId::ComponentSlidersTool.title(), "Component Sliders");
        assert_eq!(
            PanelId::ComponentSlidersTool.icon_name(),
            "mypaint-tool-component-sliders"
        );
        // 图标文件都存在（toolsbars 的内嵌表）
        for p in [
            PanelId::BrushGroups,
            PanelId::BrushEditor,
            PanelId::ModeOptionsTool,
            PanelId::LayersTool,
            PanelId::ScratchpadTool,
            PanelId::PreviewTool,
            PanelId::HistoryPanel,
            PanelId::ComponentSlidersTool,
        ] {
            let name = p.icon_name();
            assert!(
                crate::shell::toolbars::icon_png_exists(name)
                    || crate::shell::toolbars::icon_png_exists(&format!("{name}-symbolic")),
                "{name} 缺图标文件"
            );
        }
    }
}
