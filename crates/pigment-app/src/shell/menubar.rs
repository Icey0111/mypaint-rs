//! 菜单栏 —— `gui/menu.xml` 的渲染（S3）。
//!
//! menu.xml 是**纯声明式**的：`<menu action=…>`（子菜单，标题取该动作的
//! label）、`<menuitem action=…>`（条目，label/tooltip/加速键全部查 S2 的
//! 动作表）、`<separator/>`。树本身由 `build.rs` 生成到 `$OUT_DIR/
//! menus_generated.rs`（[`generated::MENUBAR`]），本文件只提供两样东西：
//!
//! 1. [`MenuNode`] —— 树节点的 Rust 形态（生成物引用它）；
//! 2. 条目渲染（[`item`]）—— egui 没有带"右对齐加速键文本 + 左侧复选槽"
//!    的菜单条目部件（GTK 是 `GtkAccelLabel` 自带的），**技术栈适配**自绘。
//!
//! 置灰规则：条目是否可用由 `PigmentApp::action_available` 决定 ——
//! 已实现的动作可用，未实现的置灰（不消费点击）。这与 GTK 的 insensitive
//! 语义一致；MyPaint 动态调整 sensitivity 的部分（如 Undo/Redo）也走同一入口。
//!
//! 菜单动作（`FileMenu` 这类）在 `resources.xml` 里有注释
//! "NO TOOLTIP OR SHORTCUT: this is a menu" —— 子菜单标题不显示 tooltip。

use eframe::egui;

/// 菜单树节点（构建期生成 [`generated::MENUBAR`]，保持 menu.xml 的顺序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuNode {
    /// `<menuitem action='…'/>`
    Item(&'static str),
    /// `<separator/>`
    Separator,
    /// `<menu action='…'>`，标题取该动作的 label
    SubMenu {
        action: &'static str,
        items: &'static [MenuNode],
    },
}

pub(crate) mod generated {
    //! 构建期生成物（`build.rs` → `$OUT_DIR/menus_generated.rs`）。
    use super::MenuNode;
    include!(concat!(env!("OUT_DIR"), "/menus_generated.rs"));
}

pub use generated::MENUBAR;

/// 菜单条目的展示信息（label / 加速键显示文本 / tooltip）。
///
/// 大部分来自 `resources.xml`（经 S2 的动作表），唯二的例外是
/// `Context0N` / `Context0Ns` 槽位动作 —— 它们**不在** resources.xml 里，
/// 是 `document.py:467-492` 用 `ag.add_actions` 元组在运行期注册的：
///
/// ```python
/// r = ("Context0%d", None, _("Restore Brush %d"), "%d",  rt, cb)
/// s = ("Context0%ds", None, _("Save to Brush %d"), "<control>%d", st, cb)
/// ```
///
/// GTK 会把这里的加速键规格接进 UI manager 的 accel group，因此菜单里
/// 也显示这些快捷键 —— 我们照搬同一组文案与键位。
pub struct ItemInfo {
    pub label: String,
    pub accel: Option<String>,
    pub tooltip: Option<String>,
}

/// 解析一条菜单条目的展示信息；`id` 必须可解析（构建期已校验）。
pub fn item_info(id: &'static str) -> ItemInfo {
    // 动态槽位动作：文案与加速键照搬 document.py 的元组
    if let Some((slot, store)) = crate::shell::actions::parse_context_action(id) {
        return if store {
            ItemInfo {
                label: format!("Save to Brush {slot}"),
                accel: Some(format!("Ctrl+{slot}")),
                tooltip: Some(format!("Store brush settings in shortcut slot {slot}")),
            }
        } else {
            ItemInfo {
                label: format!("Restore Brush {slot}"),
                accel: Some(format!("{slot}")),
                tooltip: Some(format!("Load brush settings from shortcut slot {slot}")),
            }
        };
    }
    let a = crate::shell::actions::by_id(id)
        .unwrap_or_else(|| panic!("菜单引用了未知动作：{id}"));
    ItemInfo {
        label: a.effective_label().unwrap_or(id).to_string(),
        accel: a.accel.map(|acc| acc.label()),
        tooltip: a.effective_tooltip().map(str::to_string),
    }
}

/// 画一条菜单条目，返回点击响应。
///
/// 版式照 GTK 菜单条目：左侧固定宽度的复选槽（勾选标记画在这里）、
/// 标签、右对齐的加速键文本。置灰条目不响应 hover 高亮，文字用
/// inactive 前景色（与 GTK insensitive 一致）。
///
/// `arrow`：子菜单入口右侧画"▶"（`ui.menu_button` 自己会画，这里只给
/// 测试与未来复用留位）。
pub fn item(
    ui: &mut egui::Ui,
    label: &str,
    accel: Option<&str>,
    enabled: bool,
    checked: Option<bool>,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let row_h = (font.size * 1.8).max(ui.spacing().interact_size.y);
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::click());

    let hovered = resp.hovered() && enabled;
    if hovered {
        ui.painter().rect_filled(rect, 3.0, visuals.widgets.hovered.bg_fill);
    }

    // egui dark 主题里 `Visuals::text_color()` 是非交互文本（灰 140），
    // 而可点击文本（按钮）是 `widgets.inactive.fg_stroke`（灰 215）；
    // 置灰条目用 weak（≈84，明显黯淡）。
    let text_color = if enabled {
        visuals.widgets.inactive.fg_stroke.color
    } else {
        visuals.weak_text_color()
    };

    // 左侧复选槽：有 checkbox 语义的条目（GtkToggleAction）才画
    const GUTTER: f32 = 20.0;
    if let Some(on) = checked {
        let c = egui::pos2(rect.left() + GUTTER * 0.5, rect.center().y);
        let stroke = egui::Stroke::new(1.8, text_color);
        if on {
            // GTK 的勾选标记
            ui.painter()
                .line_segment([c + egui::vec2(-4.0, 0.5), c + egui::vec2(-1.0, 3.5)], stroke);
            ui.painter()
                .line_segment([c + egui::vec2(-1.0, 3.5), c + egui::vec2(4.5, -3.0)], stroke);
        } else {
            // 未勾选的 toggle：空槽不画（GTK 亦如此）
        }
    }

    // 标签
    ui.painter().text(
        egui::pos2(rect.left() + GUTTER, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        text_color,
    );

    // 右对齐的加速键文本（GtkAccelLabel）；比标签弱一级，与 GTK 的次要文本一致
    if let Some(acc) = accel {
        let acc_color = if enabled {
            text_color.gamma_multiply(0.75)
        } else {
            text_color
        };
        ui.painter().text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            acc,
            font,
            acc_color,
        );
    }

    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 树的统计（递归）。
    fn count(nodes: &'static [MenuNode]) -> (usize, usize, usize) {
        let mut items = 0;
        let mut seps = 0;
        let mut subs = 0;
        for n in nodes {
            match n {
                MenuNode::Item(_) => items += 1,
                MenuNode::Separator => seps += 1,
                MenuNode::SubMenu { items: inner, .. } => {
                    subs += 1;
                    let (i, s, m) = count(inner);
                    items += i;
                    seps += s;
                    subs += m;
                }
            }
        }
        (items, seps, subs)
    }

    #[test]
    fn menubar_shape_matches_menu_xml() {
        // 顶级菜单：9 个，顺序 = menu.xml 的 Menubar
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
                "FileMenu", "EditMenu", "ViewMenu", "BrushMenu", "ColorMenu", "LayerMenu",
                "ScratchMenu", "WindowMenu", "HelpMenu",
            ]
        );
        // 子菜单共 22 个 = 9 顶级 + 13 嵌套（menu.xml 的 <menu> 总数）
        let (_, seps, subs) = count(MENUBAR);
        assert_eq!(subs, 22, "子菜单总数（9 顶级 + 13 嵌套）");
        assert_eq!(seps, 49, "分隔线数（照 menu.xml 数）");
    }

    #[test]
    fn every_menu_item_resolves_to_display_info() {
        // 递归解析每个条目：动作表查不到（且不是槽位动作）会 panic
        fn walk(nodes: &'static [MenuNode]) {
            for n in nodes {
                match n {
                    MenuNode::Item(id) => {
                        let info = item_info(id);
                        assert!(!info.label.is_empty(), "{id} 的 label 为空");
                    }
                    MenuNode::SubMenu { action, items } => {
                        // 菜单动作本身必有 label（它是子菜单标题）
                        assert!(item_info(action).label.len() > 1, "{action} 的 label 异常");
                        walk(items);
                    }
                    MenuNode::Separator => {}
                }
            }
        }
        walk(MENUBAR);
    }

    #[test]
    fn context_menu_holds_the_dynamic_slot_actions() {
        // Edit 菜单没有子菜单；Brush 菜单的 ContextMenu 子菜单 = 3 条目 + 分隔
        // + 20 个槽位动作（Context00..09 / Context00s..09s，menu.xml 逐行列出）
        let submenu = MENUBAR
            .iter()
            .find_map(|n| match n {
                MenuNode::SubMenu { action: "BrushMenu", items } => {
                    items.iter().find_map(|n| match n {
                        MenuNode::SubMenu { action: "ContextMenu", items } => Some(*items),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("BrushMenu 下应有 ContextMenu 子菜单");
        let slot_items: Vec<_> = submenu
            .iter()
            .filter_map(|n| match n {
                MenuNode::Item(id) => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(slot_items.len(), 23, "ContextMenu 条目数（3 + 20 槽位）");
        for n in 0..10 {
            let (restore, store) = (format!("Context0{n}"), format!("Context0{n}s"));
            assert!(slot_items.contains(&restore.as_str()));
            assert!(slot_items.contains(&store.as_str()));
        }
    }

    #[test]
    fn slot_action_display_follows_document_py() {
        // document.py:477-486：label 与加速键规格
        let restore = item_info("Context00");
        assert_eq!(restore.label, "Restore Brush 0");
        assert_eq!(restore.accel.as_deref(), Some("0"));
        let store = item_info("Context03s");
        assert_eq!(store.label, "Save to Brush 3");
        assert_eq!(store.accel.as_deref(), Some("Ctrl+3"));
    }

    #[test]
    fn submenu_titles_have_no_tooltip_or_accel() {
        // resources.xml 里菜单动作的注释："NO TOOLTIP OR SHORTCUT: this is a menu"
        for n in MENUBAR {
            if let MenuNode::SubMenu { action, .. } = n {
                let info = item_info(action);
                assert!(info.tooltip.is_none(), "{action} 不应有 tooltip");
                assert!(info.accel.is_none(), "{action} 不应有加速键");
            }
        }
    }
}
