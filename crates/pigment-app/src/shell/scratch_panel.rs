//! 暂存区面板 —— `gui/scratchwindow.py` 的 `MyPaintScratchpadTool`（B5）。
//!
//! # 源码面板的构成（`scratchwindow.py:40-62`）
//!
//! 1. **内嵌工具条**，四个动作（`:46-54` 的 `inline_toolbar`）：
//!    `ScratchNew`（新建）、`ScratchLoad`（打开…）、`ScratchSaveAs`（导出…）、
//!    `ScratchRevert`（回退）。菜单里另有 `ScratchSaveNow`（存到当前文件）
//!    等四个动作（`resources.xml:1110-1154`），本面板一并给出按钮。
//! 2. **画布** = `app.scratchpad_doc.tdw`，最小 64×64（`:55-56`）；外面的
//!    `Gtk.EventBox` 只是"可滚动/事件容器"（`:59-60`），没有别的语义。
//! 3. **存盘时机**：面板收到 `destroy-event` / `delete-event` 时
//!    `save_scratchpad(app.scratchpad_filename)`（`:57-58` + `:64-67`）。
//!    egui 侧对应"面板从停靠系统里被关掉"，由
//!    [`PigmentApp::save_scratchpad_on_close`] 在帧末统一处理。
//!
//! # 渲染方案（为什么另开一个渲染器而不是复用主画布的）
//!
//! [`crate::gpu::TileRenderer`] 本身**不绑定文档**：它只按 tile 坐标缓存纹理，
//! 每帧从 `CanvasPaint` 里收"上传包 + 可见列表 + 视图矩阵"（`gpu.rs:439-489`）。
//! 但它**确实有一份显存缓存**，而暂存区的合成结果与主文档完全不同 —— 共用一份
//! 缓存会互相覆盖对方的 tile。所以这里给暂存区**单独一个 [`TileRenderer`]
//! 实例**（`PigmentApp::scratch_renderer`），同步逻辑与主画布共用
//! [`crate::shell::workspace::prepare_canvas`]。
//!
//! 代价：显存上限翻倍（每个实例 1024 块 × 16 KiB = 16 MiB，`gpu.rs:28`）。
//! 暂存区面板通常只有几百像素见方，实际驻留远低于上限；接受并记账。
//!
//! # 与源码的差异（记账）
//!
//! - **撤销/重做按钮**：源码没有（全局 `Undo`/`Redo` 绑在主控制器上，
//!   见 `crate::scratch` 的模块文档）。暂存区**有自己的撤销栈**，不给入口就
//!   永远退不掉一笔，所以这里补两个按钮，作用范围**只有暂存区**。
//! - **没有文件对话框**：`Gtk.FileChooserDialog`（`:1140` / `:1256`）用应用内嵌
//!   的路径输入弹层替代（与主文档 `Ctrl+S` 那套一致）。

use eframe::egui;

use crate::app::PigmentApp;
use crate::shell::actions;
use crate::shell::toolbars;
use crate::view::View;

/// 面板里画布的最小边长 —— `scratchwindow.py:56` 的 `set_size_request(64, 64)`。
pub(crate) const SCRATCHPAD_MIN_SIZE: f32 = 64.0;

/// 面板工具条上的动作，顺序照 `scratchwindow.py:46-54`；后面两个（存盘、
/// 存成默认页）来自菜单（`resources.xml:1110-1146`），源码面板没有按钮，
/// 这里补上免得只能靠快捷键/菜单。
const SCRATCHPAD_TOOLBAR: [&str; 6] = [
    "ScratchNew",
    "ScratchLoad",
    "ScratchSaveAs",
    "ScratchRevert",
    "ScratchSaveNow",
    "ScratchSaveAsDefault",
];

impl PigmentApp {
    /// 暂存区面板内容（[`crate::shell::dock::PanelId::ScratchpadTool`]）。
    ///
    /// 面板是**一块画布**，所以不能塞进 `ScrollArea`（滚轮/拖拽要留给画布）——
    /// `shell/dock.rs` 对它有特判，与源码"面板里直接放 TDW"一致。
    pub(crate) fn scratchpad_panel_content(&mut self, ui: &mut egui::Ui) {
        self.scratchpad_toolbar(ui);
        self.scratchpad_canvas(ui);
    }

    /// 面板工具条（`scratchwindow.py:46-54` 的 `inline_toolbar`）。
    fn scratchpad_toolbar(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let ctx = ui.ctx().clone();
        // 可用性在闭包外算好：闭包里只剩 `icon_cache` 的可变借用
        let enabled: Vec<bool> = SCRATCHPAD_TOOLBAR
            .iter()
            .map(|id| self.action_available(id))
            .collect();
        let mut clicked: Option<&'static str> = None;
        ui.horizontal_wrapped(|ui| {
            for (i, id) in SCRATCHPAD_TOOLBAR.iter().enumerate() {
                if action_button(ui, &mut self.icon_cache, id, enabled[i]) {
                    clicked = Some(*id);
                }
            }
            ui.separator();
            // 差异记账：源码没有这两个按钮（见模块文档）
            if ui
                .add_enabled(
                    self.scratch.doc.command_stack.undo_len() > 0,
                    egui::Button::new("↶").small(),
                )
                .on_hover_text("撤销暂存区的一笔（只影响暂存区）")
                .clicked()
            {
                clicked = Some("pigment:scratch-undo");
            }
            if ui
                .add_enabled(
                    self.scratch.doc.command_stack.redo_len() > 0,
                    egui::Button::new("↷").small(),
                )
                .on_hover_text("重做暂存区的一笔（只影响暂存区）")
                .clicked()
            {
                clicked = Some("pigment:scratch-redo");
            }
        });
        let Some(id) = clicked else { return };
        if id == "pigment:scratch-undo" {
            self.flush_scratch_stroke();
            match self.scratch.undo() {
                Some(name) => self.set_status(format!("暂存区撤销：{name}"), now),
                None => self.set_status("暂存区没有可撤销的操作".to_string(), now),
            }
            ctx.request_repaint();
        } else if id == "pigment:scratch-redo" {
            self.flush_scratch_stroke();
            match self.scratch.redo() {
                Some(name) => self.set_status(format!("暂存区重做：{name}"), now),
                None => self.set_status("暂存区没有可重做的操作".to_string(), now),
            }
            ctx.request_repaint();
        } else {
            self.run_action(id, &ctx, now);
        }
    }

    /// 面板里的画布。
    ///
    /// 输入处理与 [`PigmentApp::canvas`] 同构（同一个手势表、同一套事件投影），
    /// 只是把"文档 / 模式栈 / 笔刷状态 / 视图"换成暂存区那一套
    /// —— 这正是源码里"每个 TDW 一个 `Document` 控制器"的对应实现。
    pub(crate) fn scratchpad_canvas(&mut self, ui: &mut egui::Ui) {
        // 面板里铺满剩余空间，但不小于 64×64（`scratchwindow.py:56`）。
        // 浮成独立窗口时 egui 可能报一个非有限/极大的 available_size，夹一下
        // 免得给 `allocate_exact_size` 一个天大的矩形。
        let avail = ui.available_size();
        let bound = |v: f32| {
            if v.is_finite() && v > 0.0 {
                v.max(SCRATCHPAD_MIN_SIZE)
            } else {
                SCRATCHPAD_MIN_SIZE
            }
        };
        let size = egui::vec2(bound(avail.x), bound(avail.y));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

        // 底色 = 暂存区文档的背景色。合成 tile 已经含背景（`composite_tile_rgba8`），
        // 这里兜底"还没有任何 tile"的区域。
        let [br, bg, bb] = self.scratch.doc.layers().background_color();
        ui.painter()
            .rect_filled(rect, 0.0, egui::Color32::from_rgb(br, bg, bb));

        // ---- 平移（中/右键拖拽，或 Space+左键）----
        let pan_delta = if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
        {
            response.drag_delta()
        } else if ui.input(|i| i.key_down(egui::Key::Space))
            && response.dragged_by(egui::PointerButton::Primary)
        {
            response.drag_delta()
        } else {
            egui::Vec2::ZERO
        };
        if pan_delta != egui::Vec2::ZERO {
            self.scratch.view.pan(pan_delta.x, pan_delta.y);
        }

        // ---- 缩放（以光标为锚点）----
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && response.hovered() {
            if let Some(pos) = response.hover_pos() {
                let local = pos - rect.min;
                let factor = (scroll * 0.002).exp();
                self.scratch.view.zoom_at(local.x, local.y, factor);
            }
        }

        // ---- 指针 → 暂存区自己的模式栈 ----
        let space_down = ui.input(|i| i.key_down(egui::Key::Space));
        let modal = self.file_prompt.is_some();
        let now_ms = ui.input(|i| i.time) * 1000.0;
        let mods = self.pointer_modifiers(ui.ctx());
        if !modal && !space_down {
            let mut dispatched_drag = false;
            if response.drag_started_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                self.scratch.stroke_state = pigment_engine::hokusai::BrushState::default();
                if let Some(pos) = response.interact_pointer_pos() {
                    let ev = scratch_pointer_event(
                        &self.scratch.view,
                        pos,
                        rect,
                        now_ms,
                        1,
                        mods,
                    );
                    self.scratch_mode_press(ev);
                }
                self.scratch.stroke_dirty = true;
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                if let Some(pos) = response.interact_pointer_pos() {
                    let ev = scratch_pointer_event(
                        &self.scratch.view,
                        pos,
                        rect,
                        now_ms,
                        0,
                        mods,
                    );
                    self.scratch_mode_motion(ev);
                }
                self.scratch.stroke_dirty = true;
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                if let Some(pos) = response.interact_pointer_pos() {
                    let ev = scratch_pointer_event(
                        &self.scratch.view,
                        pos,
                        rect,
                        now_ms,
                        1,
                        mods,
                    );
                    self.scratch_mode_release(ev);
                }
                // 颜色历史入栈：`gui/brushcolor.py:54`，与主画布同一时机
                if !self.eraser {
                    self.color_manager.push_current_history();
                }
            }
            // 单击也要成对投递 press/release（填充工具的单击就是一粒种子）
            if !dispatched_drag && response.clicked_by(egui::PointerButton::Primary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.scratch.stroke_state = pigment_engine::hokusai::BrushState::default();
                    let press = scratch_pointer_event(
                        &self.scratch.view,
                        pos,
                        rect,
                        now_ms,
                        1,
                        mods,
                    );
                    self.scratch_mode_press(press);
                    let release = scratch_pointer_event(
                        &self.scratch.view,
                        pos,
                        rect,
                        now_ms,
                        1,
                        mods,
                    );
                    self.scratch_mode_release(release);
                    self.scratch.stroke_dirty = true;
                    if !self.eraser {
                        self.color_manager.push_current_history();
                    }
                }
            }
        }

        // ---- 同步显存并绘制（与主画布共用 prepare_canvas）----
        let tiles = crate::shell::workspace::prepare_canvas(
            ui.ctx(),
            &self.scratch_renderer,
            self.scratch.doc.layers(),
            &self.scratch.view,
            rect,
            self.scratch.force_resync,
        );
        self.scratch.force_resync = false;
        self.scratch.pending_uploads = tiles.uploads.len();
        ui.painter()
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                crate::gpu::CanvasPaint {
                    renderer: std::sync::Arc::clone(&self.scratch_renderer),
                    view_matrix: tiles.view_matrix,
                    uploads: tiles.uploads,
                    evict: tiles.evict,
                    visible: tiles.visible,
                },
            ));

        // ---- 笔刷光标预览（压在 tile 之上）----
        if let Some(pos) = response.hover_pos() {
            let r = self
                .brush
                .get(pigment_engine::hokusai::BrushSetting::Radius)
                .base_value
                .exp()
                .max(0.5)
                * self.scratch.view.zoom;
            let color = if self.eraser {
                egui::Color32::from_rgb(200, 80, 80)
            } else {
                egui::Color32::from_gray(110)
            };
            ui.painter()
                .circle_stroke(pos, r.max(1.0), egui::Stroke::new(1.0, color));
        }

        let backlog = self.scratch.pending_uploads >= crate::gpu::MAX_UPLOADS_PER_FRAME;
        if self.scratch.stroke_dirty || backlog {
            self.scratch.stroke_dirty = false;
            ui.ctx().request_repaint();
        }
    }
}

/// 一个动作的工具条按钮：有 `icon-name` 就画图标，否则用文字
/// （`inline_toolbar` 的按钮语义，`scratchwindow.py:46-54`）。
fn action_button(
    ui: &mut egui::Ui,
    cache: &mut toolbars::IconCache,
    id: &'static str,
    enabled: bool,
) -> bool {
    let Some(action) = actions::by_id(id) else {
        return false;
    };
    let label = action
        .toolbar_label()
        .or_else(|| action.effective_label())
        .unwrap_or(id);
    let button = match action
        .icon_name
        .and_then(|n| toolbars::icon_texture(ui.ctx(), cache, n))
    {
        Some(tex) => egui::Button::image(egui::Image::new(&tex).max_size(egui::vec2(16.0, 16.0))),
        None => egui::Button::new(label).small(),
    };
    let tip = match action.accel.map(|a| a.label()) {
        Some(acc) => format!("{label}\n{acc}"),
        None => label.to_string(),
    };
    ui.add_enabled(enabled, button.min_size(egui::vec2(24.0, 20.0)))
        .on_hover_text(tip)
        .clicked()
}

/// 把 egui 指针位置投影成暂存区的模式事件（与
/// [`PigmentApp::pointer_event`] 同一套规则，只是换 [`View`]）。
fn scratch_pointer_event(
    view: &View,
    pos: egui::Pos2,
    rect: egui::Rect,
    time_ms: f64,
    button: u32,
    modifiers: pigment_engine::mode::Modifiers,
) -> pigment_engine::mode::PointerEvent {
    let local = pos - rect.min;
    let [x, y] = view.screen_to_world(local.x, local.y);
    pigment_engine::mode::PointerEvent {
        time_ms,
        x,
        y,
        pressure: None,
        xtilt: None,
        ytilt: None,
        viewzoom: view.zoom,
        viewrotation: 0.0,
        barrel_rotation: None,
        button,
        modifiers,
        // 暂存区没有独立的填充面板，Limit to View 不会用到它。
        view_bbox: None,
    }
}
