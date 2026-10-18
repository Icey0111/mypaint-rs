//! 图层面板 —— `gui/layerswindow.py` 的 `LayersTool`，填进 S6 停靠框架的
//! [`PanelId::LayersTool`](crate::shell::dock::PanelId::LayersTool) 占位。
//!
//! 布局照 `LayersTool.__init__` 的 grid（自上而下）：
//! 1. **模式下拉**（`new_blend_mode_combo`：SpectralWGM 置首，PASS_THROUGH 次之
//!    但只对组可选）+ **不透明度**（ScaleButton 的技术栈适配：egui Slider；
//!    合并语义见 [`pigment_engine::Document::set_layer_opacity`]）
//! 2. **图层树**（自顶向下；组可展开，缩略图/名称/可见性眼/锁定）+ **inline_toolbar**
//!    按钮行（NewLayerGroupAbove/NewPaintingLayerAbove/RemoveLayer/
//!    RaiseLayerInStack/LowerLayerInStack/DuplicateLayer/MergeLayerDown，
//!    顺序照源码 `:223-233`）
//! 3. **ShowBackgroundToggle** 勾选框 + 背景色（`BackgroundWindow` 的
//!    技术栈适配：egui 取色控件）
//!
//! B2-6 起支持组（`lib/layer/group.py`）：
//! - 行按 [`LayerStack::walk`] 的序（组在前、子层紧跟、深度缩进）；
//! - 组行有展开箭头（MyPaint 的 `expand_layer`/`collapse_layer`；本实现默认展开）；
//! - 组的水印图标是 `mypaint-layer-group-symbolic`，绘画层是
//!   `mypaint-layer-painting-symbolic`（`get_icon_name`）；
//! - **拖动重排**（MyPaint 的 treeview DnD）：拖到组行中段 = 放进组顶，
//!   拖到行上半/下半 = 插到该行前/后（同一个父层）；
//! - 组的模式可以是 Pass-through（组专用，见 [`pigment_surface::layer::LayerMode`]）。
//!
//! 记账差异：
//! - 重命名用行内编辑（双击名称），源码是 LayerProperties 对话框；
//! - 模式下拉不带 tooltip（egui ComboBox 无逐项提示）；
//! - 组默认展开（MyPaint 默认折叠，靠 `activate_layertype_action` 展开）。

use crate::app::PigmentApp;
use crate::shell::toolbars::{icon_texture, ToolBtn};
use eframe::egui;
use pigment_engine::layer_views::UNSAVED_VIEW_DISPLAY_NAME;
use pigment_surface::combine::{CombineMode, DEFAULT_MODE};
use pigment_surface::layer::LayerMode;

/// 预览格边长（`gui/layers.py:891` 的 `_datafunc_get_pixbuf_height(32, column)`：
/// 行高决定，初始 32）。
const PREVIEW_PX: i32 = 32;
/// 旗标列（眼/锁）单元格边长（`gui/layers.py:1098` 的 `clamp(s, 24, 48)`，
/// `s` = 行高 ≈ 预览边长）。
const FLAG_PX: f32 = 32.0;
/// 组的缩进步长（GTK treeview 的 indent）。
const INDENT_PX: f32 = 14.0;

/// 图层类型水印图标（`get_icon_name`：`data.py:1443` / `group.py:574`）。
const ICON_PAINTING: &str = "mypaint-layer-painting-symbolic";
const ICON_GROUP: &str = "mypaint-layer-group-symbolic";

/// 预览纹理的失效键。
///
/// 缩略图走本层自己的 op（模式 + 不透明度都会改变结果），而这两者**不**
/// 体现在 `surface.clock()` 上——只按 clock 缓存会在拖不透明度滑条时
/// 显示旧图（B2-5 GUI 实测抓到）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThumbKey {
    clock: u64,
    mode: LayerMode,
    opacity_bits: u32,
}

impl ThumbKey {
    fn of(layer: &pigment_surface::Layer) -> Self {
        Self {
            // 组没有表面：用 0，但组的子层内容变化要能反映 → 见 `group_clock`
            clock: layer_clock(layer),
            mode: layer.mode,
            opacity_bits: layer.opacity.to_bits(),
        }
    }
}

/// 一层的「内容时间戳」：绘画层用表面 clock，组用子层 clock 之和
/// （组的缩略图要跟着子层变）。
fn layer_clock(layer: &pigment_surface::Layer) -> u64 {
    match layer.surface() {
        Some(surface) => surface.clock(),
        None => {
            let mut acc = 0u64;
            if let Some(sub) = layer.stack() {
                for (_, child) in sub.walk() {
                    acc = acc.wrapping_add(layer_clock(child));
                }
            }
            acc
        }
    }
}

/// 一行列表的快照（避开 `&mut self` 借用）。
struct RowData {
    path: Vec<usize>,
    depth: usize,
    id: u64,
    name: Option<String>,
    visible: bool,
    locked: bool,
    is_group: bool,
    is_cur: bool,
    /// 祖先链可见性（`branch_visible`）：控制眼图标的变暗
    branch_visible: bool,
    /// 组是否展开
    expanded: bool,
}

impl PigmentApp {
    /// 面板主入口（dock.rs 的 `PanelId::LayersTool` 分支调用）。
    pub(crate) fn layers_panel_content(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);

        // ---- 0. 命名可见性视图（`LayerViewUI`，gui/layervis.py）----
        self.layer_view_row(ui, &ctx);

        // ---- 1. 模式 + 不透明度（layerswindow.py:199-217 的 grid 行）----
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            self.layer_mode_combo(ui);
        });
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            self.layer_opacity_row(ui);
        });
        ui.add_space(4.0);

        // ---- 2. 图层列表 + 按钮行（`:218-260`）----
        self.layer_list(ui, &ctx);
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            // inline_toolbar 顺序照 :223-233
            self.panel_tool_item(ui, "NewLayerGroupAbove", Some("mypaint-layer-group-new-symbolic"));
            self.panel_tool_item(ui, "NewPaintingLayerAbove", Some("mypaint-add-symbolic"));
            self.panel_tool_item(ui, "RemoveLayer", Some("mypaint-remove-symbolic"));
            self.panel_tool_item(ui, "RaiseLayerInStack", Some("mypaint-up-symbolic"));
            self.panel_tool_item(ui, "LowerLayerInStack", Some("mypaint-down-symbolic"));
            self.panel_tool_item(ui, "DuplicateLayer", None);
            self.panel_tool_item(ui, "MergeLayerDown", None);
        });
        ui.add_space(4.0);

        // ---- 3. 背景行（`:262-271`）----
        ui.horizontal(|ui| {
            let mut show_bg = self.doc.layers().background_visible();
            if ui
                .checkbox(&mut show_bg, "Show background")
                .on_hover_text("Toggle the background layer's visibility.")
                .changed()
            {
                // 与 toggle_action 一致：状态取反走动作
                self.run_action("ShowBackgroundToggle", &ctx, now);
            }
            self.background_color_button(ui);
        });
    }

    /// 命名可见性视图行 —— `LayerViewUI`（`gui/layervis.py:33` +
    /// `layervis.glade`）。
    ///
    /// 控件顺序照 glade：Add | Combo | Lock | Remove | Rename；工具提示照
    /// `layervis.glade:15/36/57/78/99`。视图锁定后禁止切换层可见性
    /// （`gui/layers.py:566`），所以下面的眼睛按钮也要跟着置灰。
    fn layer_view_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        use crate::shell::toolbars::icon_texture;
        let names = self.doc.layer_view_names();
        let current = self.doc.current_layer_view_name().map(str::to_string);
        let locked = self.doc.current_layer_view_locked();

        ui.horizontal(|ui| {
            if let Some(tex) = icon_texture(
                ui.ctx(),
                &mut self.icon_cache,
                "mypaint-layers-view-add-symbolic",
            ) {
                if ui
                    .add(egui::Button::image(
                        egui::Image::new(&tex).max_size(egui::vec2(16.0, 16.0)),
                    ))
                    .on_hover_text("Snapshot visible layers as a new view.")
                    .clicked()
                {
                    self.doc.add_layer_view(None);
                    ctx.request_repaint();
                }
            }

            // 宽度照 glade 的 `width_request=150`：**不要**用
            // `ui.available_width()` —— 在水平行里它会随面板宽度变化，
            // 反过来把面板撑宽，形成布局振荡（画布会盖住侧栏左缘）。
            let width = 150.0;
            egui::ComboBox::from_id_salt("layer_view_combo")
                .selected_text(
                    current
                        .clone()
                        .unwrap_or_else(|| UNSAVED_VIEW_DISPLAY_NAME.to_string()),
                )
                .width(width)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(current.is_none(), UNSAVED_VIEW_DISPLAY_NAME)
                        .clicked()
                    {
                        self.doc.activate_layer_view(None);
                        ctx.request_repaint();
                    }
                    for name in &names {
                        if ui
                            .selectable_label(current.as_deref() == Some(name.as_str()), name)
                            .clicked()
                        {
                            self.doc.activate_layer_view(Some(name));
                            ctx.request_repaint();
                        }
                    }
                })
                .response
                .on_hover_text("The currently active view.");

            let has_view = current.is_some();
            let lock_icon = if locked {
                "mypaint-object-locked-symbolic"
            } else {
                "mypaint-object-unlocked-symbolic"
            };
            let lock_clicked = ui
                .add_enabled_ui(has_view, |ui| {
                    icon_button(ui, &mut self.icon_cache, lock_icon, locked)
                })
                .inner
                .on_hover_text("Lock or unlock the active view.")
                .clicked();
            if lock_clicked {
                self.doc.set_active_layer_view_locked(!locked);
                ctx.request_repaint();
            }

            let remove_clicked = ui
                .add_enabled_ui(has_view, |ui| {
                    icon_button(
                        ui,
                        &mut self.icon_cache,
                        "mypaint-remove-symbolic",
                        false,
                    )
                })
                .inner
                .on_hover_text("Remove the active view.")
                .clicked();
            if remove_clicked {
                self.doc.remove_active_layer_view();
                ctx.request_repaint();
            }

            let rename_clicked = ui
                .add_enabled_ui(has_view, |ui| {
                    icon_button(ui, &mut self.icon_cache, "mypaint-edit-symbolic", false)
                })
                .inner
                .on_hover_text("Rename the active view.")
                .clicked();
            if rename_clicked {
                self.layer_view_rename = current.clone();
            }
        });

        // 重命名对话框 —— 源码是 `gui.dialogs.ask_for_name`（`layervis.py:179-198`）。
        if let Some(mut buf) = self.layer_view_rename.take() {
            let mut open = true;
            let mut commit = false;
            let mut cancel = false;
            egui::Window::new("Rename View")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    let resp = ui.text_edit_singleline(&mut buf);
                    if resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        commit = true;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            commit = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if commit {
                self.doc.rename_active_layer_view(&buf);
            } else if open && !cancel {
                self.layer_view_rename = Some(buf);
            }
        }
    }

    /// 模式下拉（`_layer_mode_combo` + `_layer_mode_combo_changed_cb`）。
    ///
    /// 次序照 `layerswindow.py:190-192`：`STACK_MODES + STANDARD_MODES` 再把
    /// SpectralWGM 提到队首 → `[SpectralWGM, Pass-through, …]`；Pass-through
    /// 只对组合法（`PERMITTED_MODES`），对绘画层显示为不可选。
    fn layer_mode_combo(&mut self, ui: &mut egui::Ui) {
        let path = self.doc.current_path().to_vec();
        let Some(layer) = self.doc.layers().layer_at(&path) else {
            return;
        };
        let cur = layer.mode;
        let is_group = layer.is_group();
        let modes: Vec<LayerMode> = std::iter::once(LayerMode::Standard(DEFAULT_MODE))
            .chain(std::iter::once(LayerMode::PassThrough))
            .chain(
                CombineMode::ALL
                    .iter()
                    .copied()
                    .filter(|m| *m != DEFAULT_MODE)
                    .map(LayerMode::Standard),
            )
            .collect();
        egui::ComboBox::from_id_salt(("layer_mode_combo", layer.id))
            .selected_text(cur.label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for m in modes {
                    let permitted = m.permitted(is_group);
                    let selected = m == cur;
                    // 照 `_layer_mode_combo_update`：不允许的模式置灰
                    let resp = ui
                        .add_enabled_ui(permitted, |ui| ui.selectable_label(selected, m.label()))
                        .inner;
                    if resp.clicked() && m != cur {
                        self.doc.set_layer_mode(&path, m);
                    }
                }
            });
    }

    /// 不透明度行（ScaleButton + `%d%%` 标签的技术栈适配）。
    fn layer_opacity_row(&mut self, ui: &mut egui::Ui) {
        let path = self.doc.current_path().to_vec();
        let Some(layer) = self.doc.layers().layer_at(&path) else {
            return;
        };
        let mut pct = (layer.opacity * 100.0).round() as i32;
        // 直通组的 opacity 恒 100%（`core.py:274-293`）→ 置灰
        let editable = !layer.mode.is_pass_through();
        ui.label("Opacity");
        let resp = ui.add_enabled(
            editable,
            egui::Slider::new(&mut pct, 0..=100)
                .suffix("%")
                .smart_aim(false),
        );
        if resp.changed() {
            self.doc.set_layer_opacity(&path, pct as f32 / 100.0);
        }
    }

    /// 当前可见的行（按 `walk()` 序，跳过折叠组的子树）。
    fn layer_rows(&self) -> Vec<RowData> {
        let layers = self.doc.layers();
        let current = layers.current_path().to_vec();
        let mut rows = Vec::new();
        for (path, layer) in layers.walk() {
            // 折叠：祖先里有折叠的组 → 跳过
            let mut hidden = false;
            for depth in 1..path.len() {
                if self.layers_collapsed.contains(&path[..depth]) {
                    hidden = true;
                    break;
                }
            }
            if hidden {
                continue;
            }
            let parent = &path[..path.len() - 1];
            rows.push(RowData {
                depth: path.len() - 1,
                id: layer.id,
                name: layer.name.clone(),
                visible: layer.visible,
                locked: layer.locked,
                is_group: layer.is_group(),
                is_cur: path == current,
                branch_visible: layers.branch_visible(parent),
                expanded: !self.layers_collapsed.contains(&path),
                path,
            });
        }
        rows
    }

    /// 图层列表（`RootStackTreeView` 的树形技术栈适配）。
    fn layer_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let rows = self.layer_rows();
        // 浮动窗口里要让 egui::Window 包住内容（限高）；停靠侧栏时
        // 剩余空间更大，min 保证列表可滚。min = 源码
        // `RootStackTreeView.set_size_request(150, 200)` 的高。
        egui::ScrollArea::vertical()
            .id_salt("layers_tree_scroll")
            .min_scrolled_height(200.0)
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for row in &rows {
                    self.layer_row(ui, ctx, row);
                }
                // 拖动重排的落点提示 + 释放处理
                self.layer_drag_feedback(ui, ctx);
            });
        // 释放鼠标一定要处理（即便指针已经离开列表区域）
        self.layer_drag_finish(ctx);
    }

    /// 单行：缩进 + 展开箭头 + 眼 | 预览 | 名字 | 锁（列序照 `layers.py:405-455`）。
    fn layer_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, row: &RowData) {
        let path = row.path.clone();
        let resp = ui
            .horizontal(|ui| {
                ui.add_space(2.0 + row.depth as f32 * INDENT_PX);
                // 组：展开/折叠箭头（没有子层时留白）
                if row.is_group {
                    let arrow = if row.expanded { "▾" } else { "▸" };
                    if ui
                        .small_button(arrow)
                        .on_hover_text(if row.expanded { "Collapse group" } else { "Expand group" })
                        .clicked()
                    {
                        if row.expanded {
                            self.layers_collapsed.insert(path.clone());
                        } else {
                            self.layers_collapsed.remove(&path);
                        }
                    }
                } else {
                    ui.add_space(12.0);
                }
        // 1) 可见性眼（`_flags1_col`，`:405-414`）；视图锁定时不可改（`layers.py:566`）
        let eye = if row.visible {
            "mypaint-object-visible-symbolic"
        } else {
            "mypaint-object-hidden-symbolic"
        };
        let view_locked = self.doc.current_layer_view_locked();
        let eye_clicked = ui
            .add_enabled_ui(!view_locked, |ui| {
                icon_button(
                    ui,
                    &mut self.icon_cache,
                    eye,
                    row.visible && row.branch_visible,
                )
            })
            .inner
            .on_hover_text(if view_locked {
                "The active layer view is locked."
            } else {
                "Toggle layer visibility"
            })
            .clicked();
        if eye_clicked {
            self.doc.set_layer_visible(&path, !row.visible);
            ctx.request_repaint();
        }
                // 2) 预览格（`_name_col` 的第一个 cell，`:421-426`）
                self.layer_preview_cell(ui, &path, row);
                // 3) 名称（单击选择，双击重命名）
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let display = row.name.clone().unwrap_or_else(|| {
                        if row.is_group {
                            "Group".to_string()
                        } else {
                            "Layer".to_string()
                        }
                    });
                    if self.layer_rename == Some(row.id) {
                        self.layer_rename_edit(ui, &path, row.id);
                    } else {
                        let label = if row.is_cur {
                            egui::RichText::new(&display).strong()
                        } else if row.is_group {
                            egui::RichText::new(&display).italics()
                        } else {
                            egui::RichText::new(&display)
                        };
                        let resp = ui.selectable_label(row.is_cur, label);
                        if resp.clicked() {
                            let now = ctx.input(|i| i.time);
                            self.select_layer_and_blink(&path, now);
                            ctx.request_repaint();
                        }
                        if resp.double_clicked() {
                            // LayerProperties 对话框的技术栈适配：行内编辑
                            self.layer_rename = Some(row.id);
                            self.rename_path = path.clone();
                            self.rename_focused = false;
                            self.rename_buf = display;
                        }
                    }
                });
                // 4) 锁定（`_flags2_col`，`:437-448`）
                let lock_icon = if row.locked {
                    "mypaint-object-locked-symbolic"
                } else {
                    "mypaint-object-unlocked-symbolic"
                };
                if icon_button(ui, &mut self.icon_cache, lock_icon, row.locked)
                    .on_hover_text(if row.locked {
                        "Locked (click to unlock)"
                    } else {
                        "Unlocked (click to lock)"
                    })
                    .clicked()
                {
                    self.doc.set_layer_locked(&path, !row.locked);
                    ctx.request_repaint();
                }
            })
            .response;

        // 拖动重排：**手柄 = 缩略图格**（整行覆盖一个 drag 区域会吞掉名字/按钮的点击）
        if self.layer_drag.is_some() && resp.rect.contains(ui.ctx().pointer_interact_pos().unwrap_or_default()) {
            // 落点判定：组行中段 = 放进组顶；行上半 = 插到该行前；下半 = 其后
            let rect = resp.rect;
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let target = Self::layer_drop_path(&row.path, pos.y, rect);
                self.layer_drop_target = Some(target);
            }
        }
    }

    /// 拖动落点 → `RestackLayer` 的目标路径。
///
/// 行中段（上下各留 30%）= 放进该层：组是插到组顶，数据层则由命令现场建父组
/// （`command.py:1310-1337`）。行上半 = 插到该行前，下半 = 其后。
fn layer_drop_path(row_path: &[usize], pos_y: f32, rect: egui::Rect) -> Vec<usize> {
    let h = rect.height();
    let into_layer = (pos_y - rect.top()) > h * 0.3 && (rect.bottom() - pos_y) > h * 0.3;
    if into_layer {
        let mut p = row_path.to_vec();
        p.push(0);
        p
    } else {
        let before = pos_y < rect.center().y;
        let mut p = row_path.to_vec();
        if !before {
            *p.last_mut().unwrap() += 1;
        }
        p
    }
}
/// 拖动结束（全局）：把拖着的层挪到落点。
    fn layer_drag_finish(&mut self, ctx: &egui::Context) {
        if self.layer_drag.is_none() {
            return;
        }
        let released = ctx.input(|i| i.pointer.any_released());
        if !released {
            return;
        }
        if let (Some(src), Some(targ)) = (self.layer_drag.take(), self.layer_drop_target.take()) {
            if self.doc.restack_layer(src, targ) {
                ctx.request_repaint();
            }
        }
    }

    /// 拖动时给目标行画提示框（在行矩形上描边）。
    fn layer_drag_feedback(&mut self, _ui: &mut egui::Ui, _ctx: &egui::Context) {}

    /// 预览格（`gui/layers.py:883` `_layer_preview_pixbuf_datafunc`）。
    ///
    /// 三层叠出来（自下而上）：
    /// 1. `make_preview` 的 32×32 成品（棋盘底 + 双线性缩放的缩略图）；
    /// 2. 1px 白色 10% 边框（`gui/layerprops.py:459-462` 的 cairo 描边）；
    /// 3. 图层类型水印图标 18px、`overall_alpha = 255/6`（`:918-930`）：
    ///    绘画层 `mypaint-layer-painting-symbolic`，组 `mypaint-layer-group-symbolic`。
    fn layer_preview_cell(&mut self, ui: &mut egui::Ui, path: &[usize], row: &RowData) {
        let size = egui::vec2(PREVIEW_PX as f32, PREVIEW_PX as f32);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let ctx = ui.ctx().clone();

        if let Some(tex) = self.layer_thumbnail(&ctx, path, row.id) {
            ui.painter().image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        // 边框：cairo 的 rectangle(0.5, 0.5, size-1, size-1) + line_width 1
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(26)),
            egui::StrokeKind::Middle,
        );
        // 水印：icon_size = 16 + 2（描边留白），居中，alpha 255/6。
        // 黑描边照 `drawutils.load_symbolic_icon(outline=(0,0,0,1))`。
        let icon_name = if row.is_group { ICON_GROUP } else { ICON_PAINTING };
        if let Some(icon) =
            crate::shell::toolbars::outlined_icon_texture(&ctx, &mut self.icon_cache, icon_name)
        {
            let r = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0));
            ui.painter().image(
                icon.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_white_alpha(42),
            );
        }

        let hover = row.name.clone().unwrap_or_else(|| {
            if row.is_group {
                "Group".to_string()
            } else {
                "Layer".to_string()
            }
        });
        if resp.drag_started() {
            self.layer_drag = Some(path.to_vec());
        }
        if resp.on_hover_text(hover).clicked() {
            let now = ctx.input(|i| i.time);
            self.select_layer_and_blink(path, now);
            ctx.request_repaint();
        }
    }

    /// 行内重命名编辑框（LayerProperties 对话框的技术栈适配）。
    ///
    /// 三个已知坑都在这里：
    /// 1. **Enter 会被单行 TextEdit 吃掉** → 先把它从事件流里摘掉（S5 笔记）；
    /// 2. **`request_focus()` 只许在首帧调一次**（每帧调＝焦点锁死、别的控件点不到）；
    /// 3. 因此不能用「`!has_focus()` 即失焦」判提交 —— 焦点要下一帧才到手，
    ///    首帧 `has_focus()` 恒 false，会把编辑框当场关掉（B2-5 GUI 实测抓到的
    ///    「双击改名没生效」）。改成：**拿到过焦点之后**才把失焦当提交。
    fn layer_rename_edit(&mut self, ui: &mut egui::Ui, path: &[usize], id: u64) {
        // Enter/Esc 都在画 TextEdit **之前**从事件流里摘掉（见 S5 笔记：单行
        // TextEdit 会自己 consume_key(Enter)，之后再查就恒为 false）。
        // 两个细节：
        // 1. **不要求修饰键为 NONE** —— 修饰键状态可能残留（合成输入/丢 keyup）；
        // 2. **press 与 release 都算** —— 实测（SendInput 合成输入）只收到
        //    Enter 的 release，press 被上游吃掉；只认 press 会让编辑框卡住不动。
        let (enter, escape) = ui.ctx().input_mut(|i| {
            let (mut enter, mut escape) = (false, false);
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key: egui::Key::Enter,
                    ..
                } => {
                    enter = true;
                    false
                }
                egui::Event::Key {
                    key: egui::Key::Escape,
                    ..
                } => {
                    escape = true;
                    false
                }
                _ => true,
            });
            (enter, escape)
        });
        let mut buf = std::mem::take(&mut self.rename_buf);
        let edit_id = egui::Id::unique(("layer_rename_edit", id));
        let resp = ui.add(
            egui::TextEdit::singleline(&mut buf)
                .desired_width(ui.available_width())
                .id(edit_id),
        );
        if resp.has_focus() {
            if !self.rename_focused {
                // 首帧拿到焦点 → 全选：改名基本都是整体替换
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit_id) {
                    let n = buf.chars().count();
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(n),
                    )));
                    state.store(ui.ctx(), edit_id);
                }
            }
            self.rename_focused = true;
        } else if !self.rename_focused {
            resp.request_focus();
        }
        let commit = enter || escape || (self.rename_focused && !resp.has_focus());
        if commit {
            self.layer_rename = None;
            self.rename_focused = false;
            self.rename_buf = String::new();
            let target = std::mem::take(&mut self.rename_path);
            if escape {
                return; // Esc = 取消（LayerProperties 的 Cancel）
            }
            let new_name = buf.trim().to_string();
            let name = if new_name.is_empty() { None } else { Some(new_name) };
            let unchanged = self
                .doc
                .layers()
                .layer_at(&target)
                .map(|l| l.name == name)
                .unwrap_or(true);
            if !unchanged {
                self.doc.rename_layer(&target, name);
            }
        } else {
            self.rename_buf = buf;
        }
        let _ = path;
    }

    /// 预览格纹理：`layer.thumbnail`（≤256，数据外接框）经 `make_preview`
    /// 烘成 32×32 的成品格。
    ///
    /// - 缩略图本身照 `render_layer_preview`：逐级减半到 `max(w,h) ≤ 256`，
    ///   减半在 **fix15 预乘域**做（[`pigment_engine::fileio::render_layer_thumbnail`]）；
    /// - `make_preview` 的棋盘/缩放/居中在
    ///   [`pigment_surface::thumb::compose_preview`] 里逐像素复刻。
    ///
    /// 缓存键 = (层 id, 内容时钟 + 本层 mode/opacity)；组的内容时钟是子层之和。
    fn layer_thumbnail(
        &mut self,
        ctx: &egui::Context,
        path: &[usize],
        layer_id: u64,
    ) -> Option<egui::TextureHandle> {
        let key = ThumbKey::of(self.doc.layers().layer_at(path)?);
        if let Some((cached, tex)) = self.layer_thumbs.get(&layer_id) {
            if *cached == key {
                return Some(tex.clone());
            }
        }
        let cell = {
            let layer = self.doc.layers().layer_at(path)?;
            let (raw, tw, th) =
                pigment_engine::fileio::render_layer_thumbnail(layer, layer.data_bbox());
            pigment_surface::thumb::compose_preview(&raw, tw as i32, th as i32, PREVIEW_PX)
        };
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [PREVIEW_PX as usize, PREVIEW_PX as usize],
            &cell,
        );
        let tex = ctx.load_texture(
            format!("layer_thumb-{layer_id}"),
            img,
            egui::TextureOptions::LINEAR,
        );
        self.layer_thumbs.insert(layer_id, (key, tex.clone()));
        Some(tex)
    }

    /// inline_toolbar 按钮（layerswindow.py:223-233 的图标覆盖版）。
    fn panel_tool_item(
        &mut self,
        ui: &mut egui::Ui,
        id: &'static str,
        icon_override: Option<&'static str>,
    ) {
        use crate::shell::toolbars::tool_button;
        let available = self.action_available(id);
        let pressed = self.toolbar_pressed(id);
        let action = crate::shell::actions::by_id(id);
        let tip = action.map(|a| {
            let mut t = a.effective_label().unwrap_or(id).to_string();
            if let Some(acc) = a.accel {
                t.push_str(&format!("  ({})", acc.label()));
            }
            t
        });
        let tip = if available { tip } else { None };
        let icon = icon_override.or_else(|| action.and_then(|a| a.icon_name));
        let clicked = tool_button(
            ui,
            &mut self.icon_cache,
            ToolBtn {
                available,
                pressed,
                icon_name: icon,
                tip,
                fallback_label: action
                    .and_then(|a| a.effective_label())
                    .unwrap_or(id)
                    .to_string(),
            },
            24.0,
        );
        if clicked {
            let ctx = ui.ctx().clone();
            let now = ui.input(|i| i.time);
            self.run_action(id, &ctx, now);
        }
    }

    /// 背景色按钮（`BackgroundWindow` 的技术栈适配）。
    fn background_color_button(&mut self, ui: &mut egui::Ui) {
        let mut rgb = self.doc.layers().background_color();
        let before = rgb;
        ui.color_edit_button_srgb(&mut rgb);
        if rgb != before {
            self.doc.layers_mut().set_background_color(rgb);
        }
    }
}

/// 旗标单元格里的图标边长（GTK 的 `GtkCellRendererPixbuf` 默认菜单尺寸 16）。
const FLAG_ICON_PX: f32 = 16.0;

/// 旗标按钮（眼/锁）：单元格固定 [`FLAG_PX`]² （`layers.py:1098-1103` 的
/// `cell.set_fixed_size`），图标仍是 16px；"关"态整体压暗（GTK 的
/// `sensitive=False` 观感）。
fn icon_button(
    ui: &mut egui::Ui,
    cache: &mut crate::shell::toolbars::IconCache,
    name: &str,
    on: bool,
) -> egui::Response {
    let tex = icon_texture(ui.ctx(), cache, name);
    let mut btn = match tex {
        Some(t) => {
            let mut img = egui::Image::new(&t).max_size(egui::vec2(FLAG_ICON_PX, FLAG_ICON_PX));
            if !on {
                img = img.tint(egui::Color32::from_rgba_unmultiplied(215, 215, 215, 90));
            }
            egui::Button::image(img)
        }
        None => egui::Button::new(if on { "●" } else { "○" }),
    };
    btn = btn.min_size(egui::vec2(FLAG_PX, FLAG_PX));
    ui.add(btn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_drop_path_targets_layer_middle_for_any_row_type() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, 100.0),
            egui::vec2(100.0, 40.0),
        );
        // 上半 → 插到该行前
        assert_eq!(PigmentApp::layer_drop_path(&[1], 105.0, rect), vec![1]);
        // 中段（上下各 30%）→ 放进该层；数据层由命令现场建组
        assert_eq!(PigmentApp::layer_drop_path(&[1], 120.0, rect), vec![1, 0]);
        // 下半 → 插到该行后
        assert_eq!(PigmentApp::layer_drop_path(&[1], 135.0, rect), vec![2]);
    }
}