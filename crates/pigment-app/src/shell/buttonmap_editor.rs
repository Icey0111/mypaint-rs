//! 偏好窗口 "Buttons" 页 —— `gui/buttonmap.py:220-637` 的
//! `ButtonMappingEditor`。
//!
//! # 源码行为
//!
//! | 源码 | 本实现 |
//! |---|---|
//! | 列表两列：Action / Button press（`:273-307`） | 同样的两列表格 + 首列选中标记 |
//! | Action 用可编辑 ComboBox，选项来自 `ModeRegistry` 的 DragMode 类 + `BUTTON_BINDING_ACTIONS`（`preferenceswindow.py:84-95`） | [`CHOICES`]：同样的动作集合，全部已接处理器 |
//! | Button press 点开模态对话框，在里面"按住修饰键并按下鼠标键"（`:461-637`） | `egui::Modal` 里一块捕获区，原始 `PointerButton` 事件取修饰键 |
//! | 无修饰键的 Button1 不能绑定（`:601-614`） | 同样拒绝，并给出红色提示 |
//! | 与已有绑定冲突时禁止 OK（`:615-631`） | 同样检测，提示"已绑定到 X" |
//! | Add / Remove 工具按钮（`:317-332`） | 列表上方的 Add binding / Remove |
//! | 每次列表变动即重建 `input.button_mapping`（`:403-415`） | [`PigmentApp::sync_button_editor`] 立刻写回 `Prefs`；运行时每次指针事件现读偏好 |
//!
//! # 记账（与源码的差异）
//!
//! - **动作集合与源码一致**：`PickContext`、`BrushChooserPopup`、
//!   `ColorChooserPopup`、`ColorDetailsDialog`、`ColorHistoryPopup`、
//!   `ToggleSubwindows` 都已接上处理器（见 `shell/quickchoice.rs` 与
//!   `PigmentApp::pick_context`），不再有灰条。`supported` 字段保留给
//!   未来新增动作的"未实现"标记。
//! - **没有 GTK 的 `Gtk.TreeView` 列排序/列宽拖拽**；列宽按比例手工分配。
//! - **键串规范化用 `<Control><Shift><Alt>` 顺序**：源码由
//!   `Gtk.accelerator_name()` 生成，GTK 3 在部分平台写 `<Primary>`；
//!   我们的解析两种都收（[`super::button_mapping::parse_binding`]）。
//! - 源码的 `_layer_inserted_cb` 那种"图层移动/撤销后恢复下拉选择"针对填充
//!   选项页，不在这里。

use eframe::egui;

use crate::app::PigmentApp;

use super::button_mapping::{binding_for, parse_binding};

/// 一个可选动作。
struct ActionChoice {
    id: &'static str,
    /// 源码里这个动作有处理器，本移植也接上了。
    supported: bool,
    /// 源码 `get_name()` 的兜底（动作表里没有的 `BrushResizeMode`）。
    fallback_label: &'static str,
}

/// `ModeRegistry.get_action_names()` 里属于 `DragMode` 的类
/// （`gui/brushmanip.py:165`、`gui/colorpicker.py:34-39`、`gui/fill.py:45`、
/// `gui/framewindow.py`、`gui/linemode.py`、`gui/symmetry.py`、
/// `gui/viewmanip.py`）+ `BUTTON_BINDING_ACTIONS`（`gui/mode.py:40-57`）。
///
/// 顺序按源码"先模式、后固定动作"；显示时按标签排序
/// （`buttonmap.py:344` 的 `sorted(...)`）。
const CHOICES: &[ActionChoice] = &[
    // ---- 模式类（ACTION_NAME）----
    ActionChoice {
        id: "FreehandMode",
        supported: true,
        fallback_label: "Freehand",
    },
    ActionChoice {
        id: "InkingMode",
        supported: true,
        fallback_label: "Inking",
    },
    ActionChoice {
        id: "StraightMode",
        supported: true,
        fallback_label: "Straight Line",
    },
    ActionChoice {
        id: "SequenceMode",
        supported: true,
        fallback_label: "Sequence",
    },
    ActionChoice {
        id: "EllipseMode",
        supported: true,
        fallback_label: "Ellipse",
    },
    ActionChoice {
        id: "FloodFillMode",
        supported: true,
        fallback_label: "Flood Fill",
    },
    ActionChoice {
        id: "LayerMoveMode",
        supported: true,
        fallback_label: "Move Layer",
    },
    ActionChoice {
        id: "FrameEditMode",
        supported: true,
        fallback_label: "Edit Frame",
    },
    ActionChoice {
        id: "SymmetryEditMode",
        supported: true,
        fallback_label: "Edit Symmetry",
    },
    ActionChoice {
        id: "ColorPickMode",
        supported: true,
        fallback_label: "Color Picker",
    },
    ActionChoice {
        id: "ColorPickModeH",
        supported: true,
        fallback_label: "Color Picker (H)",
    },
    ActionChoice {
        id: "ColorPickModeC",
        supported: true,
        fallback_label: "Color Picker (C)",
    },
    ActionChoice {
        id: "ColorPickModeY",
        supported: true,
        fallback_label: "Color Picker (Y)",
    },
    ActionChoice {
        id: "PanViewMode",
        supported: true,
        fallback_label: "Pan View",
    },
    ActionChoice {
        id: "ZoomViewMode",
        supported: true,
        fallback_label: "Zoom View",
    },
    ActionChoice {
        id: "RotateViewMode",
        supported: true,
        fallback_label: "Rotate View",
    },
    // `brushmanip.py:164-165`：不指向 GtkAction，只在注册表里占个名字
    ActionChoice {
        id: "BrushResizeMode",
        supported: true,
        fallback_label: "Resize Brush",
    },
    // ---- BUTTON_BINDING_ACTIONS（mode.py:40-57），已实现的 ----
    ActionChoice {
        id: "ShowPopupMenu",
        supported: true,
        fallback_label: "Popup Main Menu",
    },
    ActionChoice {
        id: "Undo",
        supported: true,
        fallback_label: "Undo",
    },
    ActionChoice {
        id: "Redo",
        supported: true,
        fallback_label: "Redo",
    },
    ActionChoice {
        id: "Bigger",
        supported: true,
        fallback_label: "Larger Brush",
    },
    ActionChoice {
        id: "Smaller",
        supported: true,
        fallback_label: "Smaller Brush",
    },
    ActionChoice {
        id: "MoreOpaque",
        supported: true,
        fallback_label: "More Opaque",
    },
    ActionChoice {
        id: "LessOpaque",
        supported: true,
        fallback_label: "Less Opaque",
    },
    ActionChoice {
        id: "Fullscreen",
        supported: true,
        fallback_label: "Fullscreen",
    },
    ActionChoice {
        id: "PalettePrev",
        supported: true,
        fallback_label: "Previous Palette Color",
    },
    ActionChoice {
        id: "PaletteNext",
        supported: true,
        fallback_label: "Next Palette Color",
    },
    // ---- 源码列出、本移植还没有处理器的动作 ----
    ActionChoice {
        id: "PickContext",
        supported: true,
        fallback_label: "Pick Context",
    },
    ActionChoice {
        id: "ToggleSubwindows",
        supported: true,
        fallback_label: "Toggle Subwindows",
    },
    ActionChoice {
        id: "BrushChooserPopup",
        supported: true,
        fallback_label: "Brush Chooser",
    },
    ActionChoice {
        id: "ColorChooserPopup",
        supported: true,
        fallback_label: "Color Chooser",
    },
    ActionChoice {
        id: "ColorDetailsDialog",
        supported: true,
        fallback_label: "Color Details",
    },
    ActionChoice {
        id: "ColorHistoryPopup",
        supported: true,
        fallback_label: "Color History",
    },
];

/// 动作显示名 —— `buttonmap.py:355-371` 的 `_get_action_label()`：
/// 有 GtkAction 就用它的 label，模式类用 `get_name()`。
fn action_label(id: &str) -> String {
    if let Some(action) = super::actions::by_id(id) {
        if let Some(label) = action.effective_label() {
            return label.to_string();
        }
    }
    CHOICES
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.fallback_label.to_string())
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
fn is_supported(id: &str) -> bool {
    CHOICES
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.supported)
        .unwrap_or(false)
}

/// 编辑器的一行 —— `buttonmap.py:397-399` 的 `(action, bp_name, bpd_name)`。
#[derive(Clone, Debug)]
struct Row {
    /// 规范键串；`None` = 新加的行，还没按鼠标键。
    key: Option<String>,
    action: String,
}

/// 捕获对话框里按下的组合（`buttonmap.py:597-599` 的 `event.button` +
/// `event.state`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Press {
    button: u32,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl Press {
    fn binding(&self) -> super::button_mapping::ButtonBinding {
        binding_for(self.button, self.ctrl, self.shift, self.alt)
    }

    fn key_name(&self) -> String {
        self.binding().key_name()
    }

    fn display_name(&self) -> String {
        self.binding().display_name()
    }
}

/// 鼠标捕获对话框状态（`buttonmap.py:468-568` 的 `dialog.*` 字段）。
pub(crate) struct Capture {
    row: usize,
    action: String,
    attempt: Option<Press>,
    /// 与别的绑定冲突（`buttonmap.py:620-629`）。
    clash: Option<String>,
    /// 固定键/无修饰键等硬错误。
    error: Option<String>,
}

impl Capture {
    fn new(row: usize, action: String) -> Self {
        Self {
            row,
            action,
            attempt: None,
            clash: None,
            error: None,
        }
    }

    fn set_attempt(&mut self, press: Press, rows: &[Row]) {
        self.attempt = Some(press);
        self.clash = None;
        self.error = None;
        // `buttonmap.py:601-614`：无修饰键的 Button1 含义固定
        if press.button == 1 && !press.ctrl && !press.shift && !press.alt {
            self.error = Some(
                "Button1 不加修饰键时含义固定（绘画），不能绑定，抱歉".to_string(),
            );
            return;
        }
        let key = press.key_name();
        if let Some((i, other)) = rows
            .iter()
            .enumerate()
            .find(|(i, r)| *i != self.row && r.key.as_deref() == Some(key.as_str()))
        {
            let _ = i;
            self.clash = Some(format!(
                "{} 已经绑定到 '{}'",
                press.display_name(),
                action_label(&other.action)
            ));
        }
    }

    fn can_accept(&self) -> bool {
        self.attempt.is_some() && self.clash.is_none() && self.error.is_none()
    }
}

/// Buttons 页的界面状态（egui 立即模式：控件没有对象，状态由应用持有）。
#[derive(Default)]
pub(crate) struct ButtonEditorState {
    rows: Vec<Row>,
    selected: Option<usize>,
    pub(crate) capture: Option<Capture>,
    status: Option<String>,
    /// 是否已从偏好载入（窗口每次打开时重置，源码 `update_ui` 同样重读）。
    loaded: bool,
}

impl ButtonEditorState {
    /// 从 `input.button_mapping` 重建行（`buttonmap.py:373-401` 的
    /// `set_bindings()`）。
    pub(crate) fn load(&mut self, prefs: &pigment_engine::prefs::Prefs) {
        self.rows = prefs
            .button_mapping_pairs()
            .into_iter()
            .map(|(key, action)| {
                let canonical = parse_binding(&key, &action)
                    .map(|b| b.key_name())
                    .unwrap_or(key);
                Row {
                    key: Some(canonical),
                    action,
                }
            })
            .collect();
        self.selected = if self.rows.is_empty() { None } else { Some(0) };
        self.capture = None;
        self.loaded = true;
    }

    /// 当前行导出成偏好里的 `(键串, 动作)` 列表。
    fn pairs(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .filter_map(|r| r.key.clone().map(|k| (k, r.action.clone())))
            .collect()
    }

    fn remove_selected(&mut self) {
        if let Some(i) = self.selected {
            if i < self.rows.len() {
                self.rows.remove(i);
                self.selected = if self.rows.is_empty() {
                    None
                } else {
                    Some(i.min(self.rows.len() - 1))
                };
            }
        }
    }
}

impl PigmentApp {
    /// 打开/回退偏好窗口时重读按钮映射（`preferenceswindow.py:222-223`
    /// 的 `set_bindings(p.get("input.button_mapping", {}))`）。
    pub(crate) fn reload_button_editor(&mut self) {
        self.button_editor.load(&self.prefs);
    }

    /// 把编辑器里的行写回 `input.button_mapping`。
    ///
    /// 源码 `_liststore_updated_cb`（`buttonmap.py:403-415`）在每次列表变动时
    /// 清空并重建同一张 dict；运行时 `ButtonMapping.update()` 由观察者调用。
    /// 我们的运行时每次指针事件现读偏好，所以写回即生效。
    pub(crate) fn sync_button_editor(&mut self) {
        let pairs = self.button_editor.pairs();
        self.prefs.set_button_mapping_pairs(pairs);
    }

    /// "Buttons" 页的页面体。
    pub(crate) fn button_editor_page(&mut self, ui: &mut egui::Ui) {
        if !self.button_editor.loaded {
            self.reload_button_editor();
        }
        let mut add = false;
        let mut remove = false;
        let mut start_capture: Option<usize> = None;
        let mut select: Option<usize> = None;
        let mut action_changed = false;
        let mut sync = false;

        ui.horizontal(|ui| {
            if ui
                .button("Add binding")
                .on_hover_text("添加一条绑定")
                .clicked()
            {
                add = true;
            }
            let can_remove = self.button_editor.selected.is_some();
            if ui
                .add_enabled(can_remove, egui::Button::new("Remove"))
                .on_hover_text("移除当前绑定")
                .clicked()
            {
                remove = true;
            }
            if let Some(status) = &self.button_editor.status {
                ui.label(egui::RichText::new(status).weak().small());
            }
        });
        ui.separator();

        // 列宽照 accelmap 编辑器的处置：动作列占 58%，按键列吃剩余宽度。
        let action_w = (ui.available_width() * 0.58).clamp(220.0, 520.0);
        egui::ScrollArea::vertical()
            .id_salt("buttonmap_list")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 44.0).max(120.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(action_w, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new("Action").strong());
                        },
                    );
                    ui.label(egui::RichText::new("Button press").strong());
                });
                ui.separator();
                for (i, row) in self.button_editor.rows.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let selected = self.button_editor.selected == Some(i);
                        if ui
                            .selectable_label(selected, "▸")
                            .on_hover_text("选中这一行")
                            .clicked()
                        {
                            select = Some(i);
                        }
                        ui.allocate_ui_with_layout(
                            egui::vec2(action_w, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let current = action_label(&row.action);
                                egui::ComboBox::from_id_salt(("buttonmap_action", i))
                                    .selected_text(current)
                                    .width(action_w - 12.0)
                                    .show_ui(ui, |ui| {
                                        // 源码按标签排序（buttonmap.py:344）
                                        let mut choices: Vec<&ActionChoice> =
                                            CHOICES.iter().collect();
                                        choices.sort_by_key(|c| action_label(c.id));
                                        for choice in choices {
                                            let label = if choice.supported {
                                                action_label(choice.id)
                                            } else {
                                                format!(
                                                    "{}（未移植）",
                                                    action_label(choice.id)
                                                )
                                            };
                                            let picked = ui
                                                .add_enabled_ui(choice.supported, |ui| {
                                                    ui.selectable_label(
                                                        row.action == choice.id,
                                                        label,
                                                    )
                                                })
                                                .inner
                                                .clicked();
                                            if picked {
                                                row.action = choice.id.to_string();
                                                action_changed = true;
                                            }
                                        }
                                    });
                            },
                        );
                        let display = row
                            .key
                            .as_deref()
                            .and_then(|k| parse_binding(k, ""))
                            .map(|b| b.display_name())
                            .unwrap_or_else(|| "（未设置）".to_string());
                        if ui
                            .button(display)
                            .on_hover_text("点一下，然后按住修饰键并按下鼠标键")
                            .clicked()
                        {
                            select = Some(i);
                            start_capture = Some(i);
                        }
                    });
                    ui.separator();
                }
            });

        if let Some(i) = select {
            self.button_editor.selected = Some(i);
        }
        if add {
            let action = "FreehandMode".to_string();
            self.button_editor.rows.push(Row { key: None, action });
            self.button_editor.selected = Some(self.button_editor.rows.len() - 1);
            self.button_editor.status =
                Some("新绑定：先选动作，再点「（未设置）」按鼠标键".to_string());
        }
        if remove {
            self.button_editor.remove_selected();
            sync = true;
        }
        if action_changed {
            sync = true;
        }
        if let Some(i) = start_capture {
            if let Some(row) = self.button_editor.rows.get(i) {
                self.button_editor.capture = Some(Capture::new(i, row.action.clone()));
            }
        }
        if sync {
            self.sync_button_editor();
        }
    }

    /// 鼠标捕获对话框 —— `buttonmap.py:461-637`。
    ///
    /// 只在偏好窗口的 Buttons 页开着时由 [`crate::shell::prefs_window`]
    /// 每帧调用（与按键捕获对话框同一条调用链）。
    pub(crate) fn button_capture_modal(&mut self, ctx: &egui::Context) {
        let Some(capture) = self.button_editor.capture.as_mut() else {
            return;
        };
        let title = format!("Edit binding for '{}'", action_label(&capture.action));
        let mut accept = false;
        let mut cancel = false;

        egui::Modal::new(egui::Id::unique("pigment_button_capture")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(&title);
            ui.add_space(6.0);
            egui::Grid::new("pigment_button_capture_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Action:");
                    ui.label(action_label(&capture.action));
                    ui.end_row();
                    ui.label("Button press:");
                    let text = capture
                        .attempt
                        .map(|p| p.display_name())
                        .unwrap_or_else(|| "（还没按）".to_string());
                    ui.label(egui::RichText::new(text).strong());
                    ui.end_row();
                });
            ui.add_space(8.0);

            // 捕获区：源码的 `Gtk.EventBox`（`buttonmap.py:484-492`）。
            // 用原始指针事件而不是 `Response`，因为点击判定要等松开，
            // 而源码是"按下即记录"。
            let (rect, _resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 64.0),
                egui::Sense::hover(),
            );
            ui.painter()
                .rect_stroke(rect, 3.0, ui.visuals().widgets.inactive.bg_stroke, egui::StrokeKind::Inside);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "按住修饰键，然后在这里按下鼠标键",
                egui::FontId::proportional(13.0),
                ui.visuals().text_color(),
            );
            let press = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed: true,
                        modifiers,
                        ..
                    } if rect.contains(*pos) => {
                        let gdk = match button {
                            egui::PointerButton::Primary => 1,
                            egui::PointerButton::Middle => 2,
                            egui::PointerButton::Secondary => 3,
                            _ => return None,
                        };
                        Some(Press {
                            button: gdk,
                            ctrl: modifiers.ctrl || modifiers.command,
                            shift: modifiers.shift,
                            alt: modifiers.alt,
                        })
                    }
                    _ => None,
                })
            });
            if let Some(press) = press {
                capture.set_attempt(press, &self.button_editor.rows);
            }

            ui.add_space(6.0);
            if let Some(error) = &capture.error {
                ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
            } else if let Some(clash) = &capture.clash {
                ui.label(egui::RichText::new(clash).color(egui::Color32::LIGHT_RED));
            } else {
                ui.label(
                    egui::RichText::new(
                        "按住修饰键，把鼠标移到这段文字上并按下按键，即可设置新绑定。",
                    )
                    .small(),
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui
                    .add_enabled(capture.can_accept(), egui::Button::new("OK"))
                    .clicked()
                {
                    accept = true;
                }
            });
        });

        if cancel {
            self.button_editor.capture = None;
            return;
        }
        if accept {
            let Some(capture) = self.button_editor.capture.take() else {
                return;
            };
            let Some(press) = capture.attempt else {
                return;
            };
            if let Some(row) = self.button_editor.rows.get_mut(capture.row) {
                let old = row.key.clone();
                let new_key = press.key_name();
                row.key = Some(new_key.clone());
                // 源码 `_bp_cell_edited_cb` 只改这一行；同一键在别处已被
                // `set_attempt` 拦下，所以这里不会产生重复键。
                self.button_editor.status = Some(format!(
                    "{} → {}{}",
                    press.display_name(),
                    action_label(&capture.action),
                    old.map(|o| if o == new_key { "（未改）" } else { "" })
                        .unwrap_or("")
                ));
                self.sync_button_editor();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::prefs::Prefs;

    fn fresh_prefs(tag: &str) -> Prefs {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "pigment-buttonmap-{}-{tag}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        Prefs::load(p)
    }

    #[test]
    fn choices_cover_the_source_action_set() {
        // `gui/mode.py:40-57` 的 16 个固定动作 + 17 个 DragMode 类
        assert_eq!(CHOICES.len(), 16 + 17);
        // 已实现的动作必须真的出现在动作表或模式表里
        for c in CHOICES.iter().filter(|c| c.supported) {
            assert!(
                super::super::actions::by_id(c.id).is_some()
                    || c.id == "BrushResizeMode"
                    || c.id == "ToggleSubwindows"
                    || c.id.starts_with("ColorPickMode"),
                "{} 既不在资源动作表，也不是已知模式",
                c.id
            );
        }
        // 默认映射用到的动作全部可编辑
        for id in [
            "ShowPopupMenu",
            "PanViewMode",
            "RotateViewMode",
            "ZoomViewMode",
            "FrameEditMode",
        ] {
            assert!(is_supported(id), "{id} 是默认绑定，必须可选");
        }
    }

    #[test]
    fn load_sync_and_remove_round_trip() {
        let mut prefs = fresh_prefs("roundtrip");
        let mut ed = ButtonEditorState::default();
        ed.load(&prefs);
        assert_eq!(ed.rows.len(), prefs.button_mapping_pairs().len());
        assert!(ed.rows.iter().any(|r| r.action == "ShowPopupMenu"));

        // 改动作 + 加一条新绑定
        ed.rows[0].action = "Undo".to_string();
        ed.rows.push(Row {
            key: Some("<Control><Shift>Button2".to_string()),
            action: "BrushResizeMode".to_string(),
        });
        prefs.set_button_mapping_pairs(ed.pairs());
        let pairs = prefs.button_mapping_pairs();
        assert!(pairs
            .iter()
            .any(|(k, a)| k == "<Control><Shift>Button2" && a == "BrushResizeMode"));

        // 重新载入得到同样的行
        let mut again = ButtonEditorState::default();
        again.load(&prefs);
        assert_eq!(again.rows.len(), ed.rows.len());
    }

    #[test]
    fn load_canonicalizes_and_keeps_unparsable_keys() {
        let mut prefs = fresh_prefs("canonical");
        prefs.set_button_mapping_pairs([
            ("ctrl+shift+button2".to_string(), "Undo".to_string()),
            ("not-a-button".to_string(), "Redo".to_string()),
        ]);
        let mut ed = ButtonEditorState::default();
        ed.load(&prefs);
        assert_eq!(ed.rows[0].key.as_deref(), Some("<Control><Shift>Button2"));
        assert_eq!(
            ed.rows[1].key.as_deref(),
            Some("not-a-button"),
            "解析不了的键串应原样保留，不能悄悄丢掉用户的绑定"
        );
    }

    #[test]
    fn capture_rejects_bare_button1_and_duplicate_bindings() {
        let rows = vec![
            Row {
                key: Some("Button3".to_string()),
                action: "PanViewMode".to_string(),
            },
            Row {
                key: None,
                action: "Undo".to_string(),
            },
        ];
        let mut cap = Capture::new(1, "Undo".to_string());
        // 源绑定表里 Button3 → PanViewMode
        cap.set_attempt(
            Press {
                button: 1,
                ctrl: false,
                shift: false,
                alt: false,
            },
            &rows,
        );
        assert!(!cap.can_accept(), "无修饰键的 Button1 不能绑定");
        assert!(cap.error.is_some());

        cap.set_attempt(
            Press {
                button: 3,
                ctrl: false,
                shift: false,
                alt: false,
            },
            &rows,
        );
        assert!(!cap.can_accept(), "重复绑定应被拦下");
        assert!(cap.clash.is_some());

        cap.set_attempt(
            Press {
                button: 2,
                ctrl: true,
                shift: true,
                alt: false,
            },
            &rows,
        );
        assert!(cap.can_accept(), "新组合应可确定");
        assert_eq!(cap.attempt.unwrap().key_name(), "<Control><Shift>Button2");
    }
}
