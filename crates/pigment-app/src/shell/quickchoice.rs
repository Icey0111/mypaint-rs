//! 快速选择弹层与颜色详情对话框。
//!
//! 对应源码三块：
//!
//! | 源码 | 本文件 |
//! |---|---|
//! | `gui/quickchoice.py:62-218` 的 `QuickBrushChooser` / `BrushChooserPopup` | [`QuickChoiceKind::Brush`] 弹层：分组下拉 + 48px 笔刷网格 |
//! | `gui/quickchoice.py:221-359` 的 `QuickColorChooser` / `ColorChooserPopup` | [`QuickChoiceKind::Color`] 弹层：调色板色块网格 |
//! | `gui/historypopup.py:30-146` 的 `HistoryPopup` | [`HistoryPopupState`]：最近颜色时间线，重复按动作会往前翻并立即换色 |
//! | `gui/dialogs.py:ask_for_color`（`drawwindow.py:499-508`） | [`ColorDetailsState`]：`egui::Modal` 里的取色器 + 十六进制输入 |
//!
//! # 技术栈适配
//!
//! - 源码的 `windowing.ChooserPopup` 是 `Gtk.Window` 定位到鼠标；egui 侧用
//!   `Area` 固定在鼠标位置，位置不持久化（源码的 `config_name` 只记窗口尺寸）。
//! - 颜色快速选择只搬了**调色板页**（源码默认页，也是唯一"单击即可完成选择"的
//!   页）；轮盘/方块等页仍留在右侧颜色器面板里，弹层不重复实现。
//! - 笔刷弹层按分组列出**该组全部笔刷**（源码 `PixbufList` 是竖向滚动网格，
//!   这里用 4 列 × 48px 网格）；点图标立即选笔刷并关闭。

use eframe::egui;
use pigment_engine::color::{Color, UIColor};

use crate::app::PigmentApp;
use crate::color;

/// 快速选择弹层的类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuickChoiceKind {
    Brush,
    Color,
}

/// 快速选择弹层状态（`windowing.ChooserPopup` 的可见性 + 位置 + 当前组）。
pub(crate) struct QuickChoiceState {
    pub kind: QuickChoiceKind,
    pub pos: egui::Pos2,
    pub group: String,
    pub opened: f64,
}

/// 颜色历史弹层状态（`HistoryPopup.selection`）。
pub(crate) struct HistoryPopupState {
    pub pos: egui::Pos2,
    pub selection: usize,
}

/// 颜色详情对话框状态（`dialogs.ask_for_color` 的局部值）。
pub(crate) struct ColorDetailsState {
    /// 当前编辑中的 sRGB8。
    pub color: [u8; 3],
    /// 十六进制输入框（`#rrggbb`）。
    pub hex: String,
}

/// 弹层里"选中了某个笔刷/颜色"的结果，UI 画完后再落到应用状态。
enum Chosen {
    Brush(usize),
    Color([f32; 3]),
}

impl PigmentApp {
    fn popup_anchor(&self, ctx: &egui::Context) -> egui::Pos2 {
        ctx.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()))
            .or_else(|| self.canvas_rect.map(|r| r.center()))
            .unwrap_or(egui::pos2(120.0, 120.0))
    }

    /// 打开/推进笔刷快速选择（`quick_chooser_popup_cb`，`drawwindow.py:486-497`）。
    pub(crate) fn open_brush_chooser(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(state) = self.quick_choice.as_mut() {
            if state.kind == QuickChoiceKind::Brush {
                // `Advanceable.advance()`：再按一次动作键 = 下一组
                let groups = self.brushes.library.groups();
                if !groups.is_empty() {
                    let i = groups.iter().position(|g| g.id == state.group).unwrap_or(0);
                    state.group = groups[(i + 1) % groups.len()].id.clone();
                }
                return;
            }
        }
        let group = self
            .brushes
            .selected_entry()
            .map(|e| e.group.clone())
            .unwrap_or_default();
        self.quick_choice = Some(QuickChoiceState {
            kind: QuickChoiceKind::Brush,
            pos: self.popup_anchor(ctx),
            group,
            opened: now,
        });
    }

    /// 打开/推进颜色快速选择（`ColorChooserPopup` / `ColorChooserPopupFastSubset`）。
    pub(crate) fn open_color_chooser(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(state) = self.quick_choice.as_ref() {
            if state.kind == QuickChoiceKind::Color {
                // 源码默认落在 Palette 页；本项目只移植了这一页，再按一次动作
                // 没有页可翻，保持打开即可。
                return;
            }
        }
        self.quick_choice = Some(QuickChoiceState {
            kind: QuickChoiceKind::Color,
            pos: self.popup_anchor(ctx),
            group: String::new(),
            opened: now,
        });
    }

    /// 画快速选择弹层；每帧由 `eframe::App::ui` 调用。
    #[allow(deprecated)] // 稳定 Area id 需要 Id::new（与 app::popup_menu 同处置）
    pub(crate) fn quick_choice_popup(&mut self, ctx: &egui::Context, now: f64) {
        let Some(mut state) = self.quick_choice.take() else {
            return;
        };
        let pos = state.pos;
        let mut chosen: Option<Chosen> = None;
        let mut close = false;

        let area = egui::Area::new(egui::Id::new("quick-choice-popup"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        match state.kind {
                            QuickChoiceKind::Brush => {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Brush").strong());
                                    let groups = self.brushes.library.groups();
                                    let current = groups
                                        .iter()
                                        .find(|g| g.id == state.group)
                                        .map(|g| g.title().to_string())
                                        .unwrap_or_else(|| "Brushes".to_string());
                                    egui::ComboBox::from_id_salt("quick-brush-group")
                                        .selected_text(current)
                                        .width(150.0)
                                        .show_ui(ui, |ui| {
                                            for g in &groups {
                                                if ui
                                                    .selectable_label(
                                                        state.group == g.id,
                                                        g.title(),
                                                    )
                                                    .clicked()
                                                {
                                                    state.group = g.id.clone();
                                                }
                                            }
                                        });
                                });
                                ui.separator();
                                let members = self.brushes.library.group_members(&state.group);
                                if members.is_empty() {
                                    ui.label(
                                        egui::RichText::new("（这一组没有笔刷）")
                                            .weak()
                                            .small(),
                                    );
                                }
                                egui::ScrollArea::vertical()
                                    .id_salt("quick-brush-scroll")
                                    .max_height(260.0)
                                    .show(ui, |ui| {
                                        let selected = self.brushes.selected;
                                        ui.horizontal_wrapped(|ui| {
                                            for idx in members {
                                                let tex = self.brushes.icon(ui.ctx(), idx);
                                                let name = self
                                                    .brushes
                                                    .library
                                                    .entries
                                                    .get(idx)
                                                    .map(|e| e.mypaint_name())
                                                    .unwrap_or_default();
                                                let resp = ui
                                                    .add(
                                                        egui::Button::image(
                                                            egui::Image::new(&tex)
                                                                .fit_to_exact_size(egui::vec2(
                                                                    48.0, 48.0,
                                                                )),
                                                        )
                                                        .frame(idx == selected)
                                                        .selected(idx == selected),
                                                    )
                                                    .on_hover_text(name);
                                                if resp.clicked() {
                                                    chosen = Some(Chosen::Brush(idx));
                                                    close = true;
                                                }
                                            }
                                        });
                                    });
                            }
                            QuickChoiceKind::Color => {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Color").strong());
                                    ui.label(
                                        egui::RichText::new(
                                            self.color_manager
                                                .palette()
                                                .name()
                                                .unwrap_or("Palette"),
                                        )
                                        .weak()
                                        .small(),
                                    );
                                });
                                ui.separator();
                                let current = self.color_manager.get_color();
                                let current_hsv = color::hsv_of(&current);
                                let n = self.color_manager.palette().len();
                                let columns = self
                                    .color_manager
                                    .palette()
                                    .columns()
                                    .max(1) as usize;
                                egui::ScrollArea::vertical()
                                    .id_salt("quick-color-scroll")
                                    .max_height(260.0)
                                    .show(ui, |ui| {
                                        // 调色板页的列数来自 `palette.columns`
                                        // （`palette.py` 的 Columns:，默认 7）。
                                        egui::Grid::new("quick-color-grid")
                                            .spacing([3.0, 3.0])
                                            .show(ui, |ui| {
                                                for i in 0..n {
                                                    if i > 0 && i % columns == 0 {
                                                        ui.end_row();
                                                    }
                                                    let Some(rgb) =
                                                        self.color_manager
                                                            .palette()
                                                            .get(i)
                                                    else {
                                                        ui.label("");
                                                        continue;
                                                    };
                                                    let hsv =
                                                        color::hsv_of(&Color::Rgb(rgb));
                                                    let name = self
                                                        .color_manager
                                                        .palette()
                                                        .color_name(i)
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let selected = hsv == current_hsv;
                                                    let resp =
                                                        crate::app::color_swatch(
                                                            ui, hsv, selected, &name,
                                                        );
                                                    if resp.clicked() {
                                                        chosen =
                                                            Some(Chosen::Color(hsv));
                                                        close = true;
                                                    }
                                                }
                                            });
                                    });
                            }
                        }
                    });
            });

        if let Some(chosen) = chosen {
            match chosen {
                Chosen::Brush(idx) => {
                    self.brushes.selected = idx;
                    self.reload_selected_brush();
                }
                Chosen::Color(hsv) => self.set_color_hsv(hsv),
            }
        }

        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let inside = pointer
            .map(|p| area.response.rect.contains(p))
            .unwrap_or(false);
        let clicked_out = ctx.input(|i| i.pointer.any_click())
            && !inside
            && now - state.opened > 0.05;
        if close || clicked_out || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.quick_choice = None;
        } else {
            self.quick_choice = Some(state);
        }
    }

    /// 颜色历史弹层（`ColorHistoryPopup`，`resources.xml:1019-1025`）。
    ///
    /// 源码由 `StateGroup` 管理：每次按键不是"打开窗口"，而是**往前翻一格并
    /// 立即换色**，窗口跟着选中色移动。这里照同一语义：重复调用本函数
    /// （动作键再按一次）会推进 selection。
    pub(crate) fn open_color_history(&mut self, ctx: &egui::Context, now: f64) {
        let history = self.color_manager.get_history().to_vec();
        if history.is_empty() {
            self.set_status("颜色历史还是空的（画一笔后出现）".to_string(), now);
            return;
        }
        let pos = self.popup_anchor(ctx);
        let mut selection = match self.color_history_popup.as_ref() {
            Some(state) => (state.selection + history.len() - 1) % history.len(),
            None => {
                let mut i = history.len() - 1;
                if history.get(i) == Some(&self.color_manager.get_color()) {
                    i = i.saturating_sub(1);
                }
                i
            }
        };
        if selection >= history.len() {
            selection = 0;
        }
        self.color_manager.set_color(history[selection].clone());
        self.derive_active();
        self.color_history_popup = Some(HistoryPopupState { pos, selection });
    }

    /// 画颜色历史弹层。
    #[allow(deprecated)] // 稳定 Area id 需要 Id::new
    pub(crate) fn color_history_popup(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.color_history_popup.take() else {
            return;
        };
        let history = self.color_manager.get_history().to_vec();
        if history.is_empty() {
            return;
        }
        let mut pick: Option<usize> = None;
        let mut close = false;
        let area = egui::Area::new(egui::Id::new("color-history-popup"))
            .fixed_pos(state.pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (i, c) in history.iter().enumerate() {
                                let selected = i == state.selection;
                                let size = if selected { 52.0 } else { 26.0 };
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(size, size),
                                    egui::Sense::click(),
                                );
                                let [r, g, b] = c.get_rgb();
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    egui::Color32::from_rgb(
                                        (r * 255.0).round() as u8,
                                        (g * 255.0).round() as u8,
                                        (b * 255.0).round() as u8,
                                    ),
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    2.0,
                                    egui::Stroke::new(1.5, egui::Color32::BLACK),
                                    egui::StrokeKind::Inside,
                                );
                                if selected {
                                    ui.painter().rect_stroke(
                                        rect,
                                        2.0,
                                        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                                        egui::StrokeKind::Outside,
                                    );
                                }
                                let tip = format!("{i} / {}", history.len());
                                let resp = resp.on_hover_text(tip);
                                if resp.clicked() {
                                    pick = Some(i);
                                }
                            }
                        });
                    });
            });
        if let Some(i) = pick {
            self.color_manager.set_color(history[i].clone());
            self.derive_active();
            state.selection = i;
            close = true;
        }
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let inside = pointer
            .map(|p| area.response.rect.contains(p))
            .unwrap_or(false);
        if !close
            && ctx.input(|i| i.pointer.any_click())
            && !inside
        {
            close = true;
        }
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.color_history_popup = None;
        } else {
            self.color_history_popup = Some(state);
        }
    }

    /// 打开颜色详情对话框（`ColorDetailsDialog`）。
    pub(crate) fn open_color_details(&mut self) {
        let [r, g, b] = self.color_manager.get_color().get_rgb();
        let rgb = [
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8,
        ];
        self.color_details = Some(ColorDetailsState {
            color: rgb,
            hex: format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]),
        });
    }

    /// 颜色详情对话框（`dialogs.ask_for_color`）。
    pub(crate) fn color_details_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.color_details.take() else {
            return;
        };
        let mut accept = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("color-details-dialog")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Set current color");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Color:");
                if ui.color_edit_button_srgb(&mut state.color).changed() {
                    state.hex = format!(
                        "#{:02x}{:02x}{:02x}",
                        state.color[0], state.color[1], state.color[2]
                    );
                }
                let preview = egui::Color32::from_rgb(
                    state.color[0],
                    state.color[1],
                    state.color[2],
                );
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(42.0, 24.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3.0, preview);
            });
            ui.horizontal(|ui| {
                ui.label("Hex:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.hex)
                        .desired_width(120.0)
                        .hint_text("#rrggbb"),
                );
                if resp.changed() {
                    if let Some(c) = parse_hex(&state.hex) {
                        state.color = c;
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("OK 应用当前颜色；Cancel 不改。")
                    .weak()
                    .small(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.button("OK").clicked() {
                    accept = true;
                }
            });
        });
        if cancel {
            return;
        }
        if accept {
            let [r, g, b] = state.color;
            let rgb = [r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0];
            let hsv = pigment_engine::color::rgb_to_hsv(rgb);
            self.set_color_hsv([hsv[0] as f32, hsv[1] as f32, hsv[2] as f32]);
        } else {
            self.color_details = Some(state);
        }
    }
}

/// 解析 `#rrggbb` / `rrggbb`（颜色详情对话框的文本入口）。
fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&t[0..2], 16).ok()?;
    let g = u8::from_str_radix(&t[2..4], 16).ok()?;
    let b = u8::from_str_radix(&t[4..6], 16).ok()?;
    Some([r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::color::RgbColor;

    #[test]
    fn hex_parser_accepts_hash_and_bare_forms() {
        assert_eq!(parse_hex("#336699"), Some([0x33, 0x66, 0x99]));
        assert_eq!(parse_hex("336699"), Some([0x33, 0x66, 0x99]));
        assert_eq!(parse_hex("  #ABCDEF "), Some([0xab, 0xcd, 0xef]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gggggg"), None);
    }

    #[test]
    fn rgb_color_converts_to_hsv_for_the_dialog() {
        let c = Color::Rgb(RgbColor::new_from_hex_str("#336699"));
        let rgb = c.get_rgb();
        assert!((rgb[0] - 0.2).abs() < 1e-6);
        let hsv = color::hsv_of(&c);
        assert!(hsv[1] > 0.5 && hsv[2] > 0.5);
    }
}
