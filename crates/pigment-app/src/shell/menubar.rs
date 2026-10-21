//! 菜单栏：`gui/menu.xml` 的树 + 紧凑自绘条目。
//!
//! 数据仍是 MyPaint 的九棵菜单，树由 `build.rs` 从 `assets/gui/menu.xml`
//! 生成，构建期会校验每个动作 id 都存在。视觉不再套 GTK 的
//! `GtkMenu` 尺寸，而是 ArrowVortex 的窄行、1px 分隔与右对齐快捷键。

use eframe::egui;

use crate::app::PigmentApp;
use crate::shell::theme;
use crate::shell::toolbars;

/// 菜单树节点（生成物引用它）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuNode {
    Item(&'static str),
    Separator,
    SubMenu {
        action: &'static str,
        items: &'static [MenuNode],
    },
}

pub(crate) mod generated {
    use super::MenuNode;
    include!(concat!(env!("OUT_DIR"), "/menus_generated.rs"));
}

pub use generated::MENUBAR;

/// 菜单条目的展示信息。
pub struct ItemInfo {
    pub label: String,
    pub accel: Option<String>,
    pub tooltip: Option<String>,
}

pub fn item_info(id: &'static str) -> ItemInfo {
    if let Some((slot, store)) = crate::shell::actions::parse_context_action(id) {
        return if store {
            ItemInfo {
                label: format!("保存笔刷到槽位 {slot}"),
                accel: Some(format!("Ctrl+{slot}")),
                tooltip: Some(format!("把当前笔刷设置存进快捷键槽位 {slot}")),
            }
        } else {
            ItemInfo {
                label: format!("恢复笔刷槽位 {slot}"),
                accel: Some(format!("{slot}")),
                tooltip: Some(format!("从快捷键槽位 {slot} 恢复笔刷设置")),
            }
        };
    }
    let a = crate::shell::actions::by_id(id).unwrap_or_else(|| panic!("菜单引用了未知动作：{id}"));
    ItemInfo {
        label: a.effective_label().unwrap_or(id).to_string(),
        accel: a.accel.map(|acc| acc.label()),
        tooltip: a.effective_tooltip().map(str::to_string),
    }
}

impl PigmentApp {
    /// 菜单栏整行（九棵顶级菜单）。
    pub(crate) fn menubar(&mut self, ui: &mut egui::Ui) {
        let height = theme::MENUBAR_H;
        let (row, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(row, 0.0, theme::PANEL_DARK);
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(row.left(), row.bottom() - 1.0),
                row.right_bottom(),
            ),
            0.0,
            theme::LINE,
        );

        let mut bar = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row.shrink2(egui::vec2(4.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        bar.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        bar.spacing_mut().button_padding = egui::vec2(9.0, 3.0);
        bar.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        bar.visuals_mut().widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
        bar.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
        bar.visuals_mut().widgets.hovered.weak_bg_fill = theme::HOVER;
        bar.visuals_mut().widgets.hovered.bg_fill = theme::HOVER;
        bar.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::NONE;
        bar.visuals_mut().widgets.active.weak_bg_fill = theme::ACTIVE;
        bar.visuals_mut().widgets.active.bg_fill = theme::ACTIVE;

        let ctx = ui.ctx().clone();
        bar.scope(|ui| {
            egui::MenuBar::new()
                .config(theme::menu_config())
                .ui(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for node in MENUBAR {
                        let MenuNode::SubMenu { action, items } = node else {
                            continue;
                        };
                        let info = item_info(action);
                        let label = crate::i18n::tr(&info.label).to_string();
                        let resp = ui.menu_button(
                            egui::RichText::new(&label)
                                .size(14.5)
                                .color(theme::TEXT_DIM),
                            |ui| {
                                // 固定菜单宽度：否则 egui 会按可用空间把每个
                                // 菜单撑到半屏，文件菜单尤其明显。
                                ui.set_width(252.0);
                                ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
                                // 菜单行本身只有 19pt 高，默认的 8pt 纵向
                                // item_spacing 会把 Layer 这类长菜单撑出屏幕，
                                // 底部条目变成点不到的死区。行距交给分隔线。
                                ui.spacing_mut().item_spacing.y = 0.0;
                                self.menu_items(ui, items, &ctx);
                            },
                        );
                        resp.response
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                });
        });

        // 右侧放当前文档名与修饰色点，像 ArrowVortex 的状态签名位；不抢菜单宽度。
        let title = self.window_title();
        let dirty = self.doc.unsaved_painting_time > 0.0;
        let suffix = if dirty { " *" } else { "" };
        let right = format!("{title}{suffix}");
        let galley =
            ui.painter()
                .layout_no_wrap(right, egui::FontId::proportional(11.0), theme::TEXT_FAINT);
        let x = row.right() - galley.size().x - 12.0;
        if x > 420.0 {
            ui.painter().galley(
                egui::pos2(x, row.center().y - galley.size().y * 0.5),
                galley,
                theme::TEXT_FAINT,
            );
        }
    }

    /// 递归绘制一个菜单树。`ctx` 传值以避开闭包里再次借用 UI。
    fn menu_items(&mut self, ui: &mut egui::Ui, nodes: &'static [MenuNode], ctx: &egui::Context) {
        for node in nodes {
            match node {
                MenuNode::Separator => {
                    ui.add_space(3.0);
                    theme::hline(ui, theme::LINE_SOFT);
                    ui.add_space(3.0);
                }
                MenuNode::Item(id) => {
                    self.menu_item(ui, id, ctx);
                }
                MenuNode::SubMenu { action, items } => {
                    let info = item_info(action);
                    let label = crate::i18n::tr(&info.label).to_string();
                    let resp = ui.menu_button(
                        egui::RichText::new(&label).size(14.5).color(theme::TEXT),
                        |ui| {
                            ui.set_width(240.0);
                            ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            self.menu_items(ui, items, ctx);
                        },
                    );
                    resp.response
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            }
        }
    }

    fn menu_item(&mut self, ui: &mut egui::Ui, id: &'static str, ctx: &egui::Context) {
        if id == "OpenRecent" {
            let info = item_info(id);
            let label = crate::i18n::tr(&info.label).to_string();
            ui.menu_button(
                egui::RichText::new(&label).size(14.5).color(theme::TEXT),
                |ui| {
                    ui.set_width(280.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let files = self.recent_files().to_vec();
                    if files.is_empty() {
                        menu_row(ui, "没有最近文件", None, false, None);
                    }
                    for (i, path) in files.iter().enumerate() {
                        let name = std::path::Path::new(path)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(path);
                        let resp = menu_row(ui, name, None, true, None);
                        if resp.clicked() {
                            let now = ui.input(|i| i.time);
                            self.open_recent(i, now);
                            ui.close();
                        }
                    }
                },
            );
            return;
        }

        let info = item_info(id);
        let available = self.action_available(id);
        let checked = self.action_checked(id);
        let label = crate::i18n::tr(&info.label).to_string();
        let accel = info.accel.as_deref();
        let resp = menu_row(ui, &label, accel, available, checked);
        if resp.clicked() && available {
            let now = ctx.input(|i| i.time);
            self.run_action(id, ctx, now);
            ui.close();
        }
    }
}

/// 一行自绘菜单条目。返回点击响应。
///
/// 左侧固定复选槽：toggle 动作（`checked` 为 `Some`）无论勾选与否都画一个
/// GTK 风格方框，未选为空框；普通命令不画框。标签、快捷键与禁用态的基线
/// 保持一致，展开菜单时不会左右跳动。
pub(crate) fn menu_row(
    ui: &mut egui::Ui,
    label: &str,
    accel: Option<&str>,
    enabled: bool,
    checked: Option<bool>,
) -> egui::Response {
    let width = ui.available_width().max(210.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(width, 19.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, theme::HOVER);
    }
    let color = if enabled {
        theme::TEXT
    } else {
        theme::TEXT_FAINT
    };
    let label_x = if let Some(on) = checked {
        // 与 MyPaint 原版一致：toggle 项未选也保留空框，而不是只显示勾。
        let box_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 12.0, rect.center().y),
            egui::vec2(12.0, 12.0),
        );
        ui.painter().rect_filled(
            box_rect,
            0.0,
            if on { theme::SELECTED } else { theme::FIELD },
        );
        ui.painter().rect_stroke(
            box_rect,
            0.0,
            egui::Stroke::new(
                1.0,
                if on {
                    theme::ACCENT
                } else {
                    egui::Color32::from_rgb(0x6a, 0x6a, 0x66)
                },
            ),
            egui::StrokeKind::Inside,
        );
        if on {
            let c = box_rect.center();
            ui.painter().line_segment(
                [c + egui::vec2(-3.0, 0.2), c + egui::vec2(-0.8, 2.8)],
                egui::Stroke::new(1.4, egui::Color32::WHITE),
            );
            ui.painter().line_segment(
                [c + egui::vec2(-0.8, 2.8), c + egui::vec2(4.0, -3.0)],
                egui::Stroke::new(1.4, egui::Color32::WHITE),
            );
        }
        rect.left() + 28.0
    } else {
        rect.left() + 23.0
    };
    ui.painter().text(
        egui::pos2(label_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        color,
    );
    if let Some(accel) = accel {
        let acc_color = if enabled {
            theme::TEXT_FAINT
        } else {
            theme::TEXT_FAINT.gamma_multiply(0.6)
        };
        ui.painter().text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            accel,
            egui::FontId::monospace(10.5),
            acc_color,
        );
    }
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 工具条用不到，但保留给菜单测试的图标存在性检查。
#[allow(dead_code)]
pub(crate) fn icon_for_action(id: &str) -> Option<egui::TextureHandle> {
    let name = toolbars::action_icon_name(id)?;
    let _ = name;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menubar_shape_matches_menu_xml() {
        let top: Vec<_> = MENUBAR
            .iter()
            .map(|n| match n {
                MenuNode::SubMenu { action, .. } => *action,
                other => panic!("顶级必须是子菜单，实得 {other:?}"),
            })
            .collect();
        assert_eq!(
            top,
            [
                "FileMenu",
                "EditMenu",
                "ViewMenu",
                "BrushMenu",
                "ColorMenu",
                "LayerMenu",
                "ScratchMenu",
                "WindowMenu",
                "HelpMenu",
            ]
        );
    }

    #[test]
    fn every_menu_item_resolves_to_display_info() {
        fn walk(nodes: &'static [MenuNode]) {
            for n in nodes {
                match n {
                    MenuNode::Item(id) => {
                        assert!(!item_info(id).label.is_empty(), "{id} 的 label 为空");
                    }
                    MenuNode::SubMenu { action, items } => {
                        assert!(item_info(action).label.len() > 1);
                        walk(items);
                    }
                    MenuNode::Separator => {}
                }
            }
        }
        walk(MENUBAR);
    }
}
