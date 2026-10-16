//! 图层面板 —— `gui/layerswindow.py` 的 `LayersTool`，填进 S6 停靠框架的
//! [`PanelId::LayersTool`](crate::shell::dock::PanelId::LayersTool) 占位。
//!
//! 布局照 `LayersTool.__init__` 的 grid（自上而下）：
//! 1. **模式下拉**（`new_blend_mode_combo`：SpectralWGM 置首，其余按枚举序）
//!    + **不透明度**（ScaleButton 的技术栈适配：egui Slider；合并语义见
//!    [`pigment_engine::Document::set_layer_opacity`]）
//! 2. **图层列表**（自顶向下；缩略图/名称/可见性眼/锁定）+ **inline_toolbar**
//!    按钮行（NewLayerGroupAbove/NewPaintingLayerAbove/RemoveLayer/
//!    RaiseLayerInStack/LowerLayerInStack/DuplicateLayer/MergeLayerDown，
//!    顺序照源码 `:223-233`）
//! 3. **ShowBackgroundToggle** 勾选框 + 背景色（`BackgroundWindow` 的
//!    技术栈适配：egui 取色控件）
//!
//! 记账差异：
//! - LayerViewUI 一行（「命名可见性视图」辅助功能）未移植；
//! - 重命名用行内编辑（双击名称），源码是 LayerProperties 对话框；
//! - 模式下拉不带 tooltip（egui ComboBox 无逐项提示）。

use crate::shell::toolbars::{icon_texture, ToolBtn};
use crate::app::PigmentApp;
use eframe::egui;
use pigment_surface::combine::{CombineMode, DEFAULT_MODE};

/// 预览格边长（`gui/layers.py:891` 的 `_datafunc_get_pixbuf_height(32, column)`：
/// 行高决定，初始 32）。
const PREVIEW_PX: i32 = 32;
/// 旗标列（眼/锁）单元格边长（`gui/layers.py:1098` 的 `clamp(s, 24, 48)`，
/// `s` = 行高 ≈ 预览边长）。
const FLAG_PX: f32 = 32.0;

/// 图层类型水印图标（`lib/layer/data.py:1443` `SimplePaintingLayer.get_icon_name`）。
const LAYER_ICON: &str = "mypaint-layer-painting-symbolic";

/// 预览纹理的失效键。
///
/// 缩略图走本层自己的 op（模式 + 不透明度都会改变结果），而这两者**不**
/// 体现在 `surface.clock()` 上——只按 clock 缓存会在拖不透明度滑条时
/// 显示旧图（B2-5 GUI 实测抓到）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThumbKey {
    clock: u64,
    mode: CombineMode,
    opacity_bits: u32,
}

impl ThumbKey {
    fn of(layer: &pigment_surface::Layer) -> Self {
        Self {
            clock: layer.surface.clock(),
            mode: layer.mode,
            opacity_bits: layer.opacity.to_bits(),
        }
    }
}

impl PigmentApp {
    /// 面板主入口（dock.rs 的 `PanelId::LayersTool` 分支调用）。
    pub(crate) fn layers_panel_content(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);

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

    /// 模式下拉（`_layer_mode_combo` + `_layer_mode_combo_changed_cb`）。
    fn layer_mode_combo(&mut self, ui: &mut egui::Ui) {
        let cur = self.doc.layers().current().mode;
        // modes = STACK_MODES + STANDARD_MODES，去 SpectralWGM 再插回队首
        // （layerswindow.py:201-203）；平栈没有 PASS_THROUGH（组专用）。
        let modes: Vec<CombineMode> = std::iter::once(DEFAULT_MODE)
            .chain(CombineMode::ALL.iter().copied().filter(|m| *m != DEFAULT_MODE))
            .collect();
        egui::ComboBox::from_id_salt(("layer_mode_combo", self.doc.layers().current_id()))
            .selected_text(cur.label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for m in modes {
                    let selected = ui
                        .selectable_label(m == cur, m.label())
                        .clicked();
                    if selected && m != cur {
                        let i = self.doc.layers().current_index();
                        self.doc.set_layer_mode(i, m);
                    }
                }
            });
    }

    /// 不透明度行（ScaleButton + `%d%%` 标签的技术栈适配）。
    fn layer_opacity_row(&mut self, ui: &mut egui::Ui) {
        let i = self.doc.layers().current_index();
        let Some(layer) = self.doc.layers().try_layer(i) else {
            return;
        };
        let mut pct = (layer.opacity * 100.0).round() as i32;
        ui.label("Opacity");
        let resp = ui.add(
            egui::Slider::new(&mut pct, 0..=100)
                .suffix("%")
                .smart_aim(false),
        );
        if resp.changed() {
            self.doc.set_layer_opacity(i, pct as f32 / 100.0);
        }
    }

    /// 图层列表（`RootStackTreeView` 的平栈技术栈适配）。
    fn layer_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let n = self.doc.layers().len();
        let current = self.doc.layers().current_index();
        // 收集快照，避开 &mut self 借用
        let rows: Vec<(u64, Option<String>, bool, bool, bool)> = (0..n)
            .map(|i| {
                let l = self.doc.layers().layer(i);
                (
                    l.id,
                    l.name.clone(),
                    l.visible,
                    l.locked,
                    i == current,
                )
            })
            .collect();

        // 浮动窗口里要让 egui::Window 包住内容（限高）；停靠侧栏时
        // 剩余空间更大，min 保证列表可滚。min = 源码
        // `RootStackTreeView.set_size_request(150, 200)` 的高。
        egui::ScrollArea::vertical()
            .min_scrolled_height(200.0)
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("layer_list_rows")
                    .num_columns(4)
                    .spacing([6.0, 2.0])
                    .show(ui, |ui| {
                        for (i, (id, name, visible, locked, is_cur)) in
                            rows.iter().enumerate()
                        {
                            let id = *id;
                            let (visible, locked, is_cur) = (*visible, *locked, *is_cur);

                            // 1) 可见性眼（`_flags1_col`，`:405-414`；
                            //    图标名按 visible/hidden 切换，隐藏时变暗）
                            let eye = if visible {
                                "mypaint-object-visible-symbolic"
                            } else {
                                "mypaint-object-hidden-symbolic"
                            };
                            if icon_button(ui, &mut self.icon_cache, eye, visible).clicked() {
                                self.doc.set_layer_visible(i, !visible);
                                ctx.request_repaint();
                            }

                            // 2) 预览格（`_name_col` 的第一个 cell，`:421-426`）
                            self.layer_preview_cell(ui, i, id, &name);

                            // 3) 名称（单击选择，双击重命名）
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                let display = name.clone().unwrap_or_else(|| "Layer".to_string());
                                if self.layer_rename == Some(id) {
                                    self.layer_rename_edit(ui, id, i);
                                } else {
                                    let label = if is_cur {
                                        egui::RichText::new(&display).strong()
                                    } else {
                                        egui::RichText::new(&display)
                                    };
                                    let resp = ui.selectable_label(is_cur, label);
                                    if resp.clicked() {
                                        self.doc.select_layer(i);
                                        ctx.request_repaint();
                                    }
                                    if resp.double_clicked() {
                                        // LayerProperties 对话框的技术栈适配：行内编辑
                                        self.layer_rename = Some(id);
                                        self.rename_focused = false;
                                        self.rename_buf = display;
                                    }
                                }
                            });

                            // 4) 锁定（`_flags2_col`，`:437-448`）
                            let lock_icon = if locked {
                                "mypaint-object-locked-symbolic"
                            } else {
                                "mypaint-object-unlocked-symbolic"
                            };
                            if icon_button(ui, &mut self.icon_cache, lock_icon, locked)
                                .on_hover_text(if locked {
                                    "Locked (click to unlock)"
                                } else {
                                    "Unlocked (click to lock)"
                                })
                                .clicked()
                            {
                                self.doc.set_layer_locked(i, !locked);
                                ctx.request_repaint();
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    /// 行内重命名编辑框（LayerProperties 对话框的技术栈适配）。
    ///
    /// 三个已知坑都在这里：
    /// 1. **Enter 会被单行 TextEdit 吃掉** → 先把它从事件流里摘掉（S5 笔记）；
    /// 2. **`request_focus()` 只许在首帧调一次**（每帧调＝焦点锁死、别的控件点不到）；
    /// 3. 因此不能用「`!has_focus()` 即失焦」判提交 —— 焦点要下一帧才到手，
    ///    首帧 `has_focus()` 恒 false，会把编辑框当场关掉（B2-5 GUI 实测抓到的
    ///    「双击改名没生效」）。改成：**拿到过焦点之后**才把失焦当提交。
    fn layer_rename_edit(&mut self, ui: &mut egui::Ui, id: u64, index: usize) {
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
                // （GTK 的 LayerProperties 对话框里 entry 也是全选态）
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
            if escape {
                return; // Esc = 取消（LayerProperties 的 Cancel）
            }
            let new_name = buf.trim().to_string();
            let name = if new_name.is_empty() { None } else { Some(new_name) };
            if name != self.doc.layers().layer(index).name {
                self.doc.rename_layer(index, name);
            }
        } else {
            self.rename_buf = buf;
        }
    }

    /// 预览格（`gui/layers.py:883` `_layer_preview_pixbuf_datafunc`）。
    ///
    /// 三层叠出来（自下而上）：
    /// 1. `make_preview` 的 32×32 成品（棋盘底 + 双线性缩放的缩略图）；
    /// 2. 1px 白色 10% 边框（`gui/layerprops.py:459-462` 的 cairo 描边）；
    /// 3. 图层类型水印图标 18px、`overall_alpha = 255/6`（`:918-930`）。
    fn layer_preview_cell(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        layer_id: u64,
        name: &Option<String>,
    ) {
        let size = egui::vec2(PREVIEW_PX as f32, PREVIEW_PX as f32);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let ctx = ui.ctx().clone();

        if let Some(tex) = self.layer_thumbnail(&ctx, index, layer_id) {
            ui.painter().image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        // 边框：cairo 的 rectangle(0.5, 0.5, size-1, size-1) + line_width 1
        // → 像素中心线，即「内侧 1px」
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(26)),
            egui::StrokeKind::Middle,
        );
        // 水印：icon_size = 16 + 2（描边留白），居中，alpha 255/6
        if let Some(icon) = icon_texture(&ctx, &mut self.icon_cache, LAYER_ICON) {
            let r = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0));
            ui.painter().image(
                icon.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_white_alpha(42),
            );
        }

        if resp.on_hover_text(layer_hover_name(name)).clicked() {
            self.doc.select_layer(index);
            ctx.request_repaint();
        }
    }

    /// 预览格纹理：`layer.thumbnail`（≤256，数据外接框）经 `make_preview`
    /// 烘成 32×32 的成品格。
    ///
    /// - 缩略图本身照 `render_layer_preview`：逐级减半到 `max(w,h) ≤ 256`，
    ///   减半在 **fix15 预乘域**做（[`pigment_engine::fileio::render_layer_thumbnail`]）；
    /// - `make_preview` 的棋盘/缩放/居中在
    ///   [`pigment_surface::thumb::compose_preview`] 里逐像素复刻。
    ///
    /// 缓存键 = (层 id, 表面 clock)；clock 不变直接复用纹理。
    fn layer_thumbnail(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        layer_id: u64,
    ) -> Option<egui::TextureHandle> {
        let key = ThumbKey::of(self.doc.layers().try_layer(index)?);
        if let Some((cached, tex)) = self.layer_thumbs.get(&layer_id) {
            if *cached == key {
                return Some(tex.clone());
            }
        }
        let cell = {
            let layer = self.doc.layers().try_layer(index)?;
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

fn layer_hover_name(name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| "Layer".to_string())
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

