//! 工具条 —— `gui/toolbar.xml`（框架）+ 8 个可开关片段 + `toolbar2`（S4）。
//!
//! 源码结构（`gui/toolbar.py:45-126` 的 `ToolbarManager`）：
//!
//! - **toolbar1** 由框架声明（占位符 + 内联条目），8 个片段文件
//!   （`toolbar-file.xml` 等）各含一个同名占位符，按偏好
//!   `ui.toolbar_items` 在运行期**合并**进框架（GTK UI manager 的 merge
//!   行为：片段条目追加在占位符内联条目之后）。
//! - **toolbar1 右键**弹 `toolbar1-settings-menu`（框架 XML 里的 popup），
//!   8 个开关动作是 `toolbar.py:87-99` 运行期注册的 `GtkToggleAction`
//!   （不在 `resources.xml`），label 来自 `toolbar.py:26-39` 的
//!   `MERGEABLE_XML`。
//! - **toolbar2** 固定：`ColorAdjusters` `BrushGroups` │ `Toggle×5` │
//!   `Fullscreen`。
//! - 两条工具条都是 `Gtk.ToolbarStyle.ICONS`（纯图标按钮），图标尺寸取
//!   偏好 `ui.toolbar_icon_size`（默认 "large" = LARGE_TOOLBAR，24×24，
//!   `gui/widgets.py:164-172`）。
//! - toolbar1 的 `<separator/>` 全部 `set_draw(False)`
//!   （`toolbar.py:68-70`）—— 组间只有留白，没有可见竖线。
//!
//! **技术栈适配**：
//! - 图标：MyPaint 的 hicolor scalable symbolic SVG（随项目内嵌于
//!   `assets/icons/`，GPL 同源）由 `build.rs` 用 resvg 栅格化成 24×24 PNG；
//!   GTK 的 symbolic 着色用"运行期按前景色重写 RGB、保留 alpha"复现。
//! - `set_show_arrow(True)`（toolbar1 溢出折叠）没有对应物，条目超出宽度
//!   时 egui 按序截断 —— 桌面窗口宽度下不触发，差异记账不阻塞。

use eframe::egui;

/// 工具条节点（生成物用；与 `toolbar*.xml` 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TbNode {
    /// `<toolitem action="…"/>`
    Item(&'static str),
    /// `<separator/>`（源码里 `set_draw(False)`：不可见，只留白）
    Separator,
    /// `<placeholder name="…">`，`items` 为框架内联条目
    Placeholder {
        name: &'static str,
        items: &'static [TbNode],
    },
}

pub(crate) mod generated {
    //! 构建期生成物（`build.rs` → `$OUT_DIR/toolbars_generated.rs`）。
    use super::TbNode;
    include!(concat!(env!("OUT_DIR"), "/toolbars_generated.rs"));
}

pub use generated::{FRAGMENT_ITEMS, TOOLBAR1_FRAMEWORK, TOOLBAR2};

pub(crate) mod icons_generated {
    //! 构建期栅格化的图标 PNG（`build.rs` → `$OUT_DIR/icons/*.png`）。
    include!(concat!(env!("OUT_DIR"), "/icons_generated.rs"));
}
use icons_generated::ICONS;

/// 片段开关（`toolbar.py:26-39` 的 `MERGEABLE_XML`）—— 动作名与显示名。
///
/// 这些动作是运行期注册的 toggle，不在动作表里；序号与
/// [`FRAGMENT_ITEMS`] 的顺序一致（框架 XML 的 settings 菜单按同一顺序引用）。
pub struct Fragment {
    /// 片段开关动作名（`toolbar1_file` 等）。UI 只用 label；这个名字是
    /// 偏好 `ui.toolbar_items` 的键，B6 持久化时启用。
    #[allow(dead_code)]
    pub action: &'static str,
    pub label: &'static str,
}

pub static FRAGMENTS: &[Fragment] = &[
    Fragment { action: "toolbar1_file", label: "File handling" },
    Fragment { action: "toolbar1_scrap", label: "Scraps switcher" },
    Fragment { action: "toolbar1_edit", label: "Undo and Redo" },
    Fragment { action: "toolbar1_blendmodes", label: "Blend Modes" },
    Fragment { action: "toolbar1_linemodes", label: "Line Modes" },
    Fragment { action: "toolbar1_view_modes", label: "View (Main)" },
    Fragment {
        action: "toolbar1_view_manips",
        label: "View (Alternative/Secondary)",
    },
    Fragment { action: "toolbar1_view_resets", label: "View (Resetting)" },
];

/// 出厂默认的片段开关（`gui/userconfig.py:76-85`）：
/// file / edit / linemodes / view_modes / view_resets 开，其余关。
pub const DEFAULT_FRAGMENTS: [bool; 8] = [true, false, true, false, true, true, false, true];

/// 图标纹理缓存键：图标名（已含 -symbolic 后缀解析）。
pub type IconCache = std::collections::HashMap<&'static str, egui::TextureHandle>;

/// 按图标名取纹理；找不到时试 `-symbolic` 后缀
/// （`Fullscreen` 的 icon-name 是 `mypaint-fullscreen`，文件带后缀）。
pub fn icon_texture(
    ctx: &egui::Context,
    cache: &mut IconCache,
    name: &str,
) -> Option<egui::TextureHandle> {
    if let Some(t) = cache.get(name) {
        return Some(t.clone());
    }
    let (resolved, bytes) = lookup_png(name)
        .map(|b| (name, b))
        .or_else(|| {
            lookup_png(&format!("{name}-symbolic"))
                .map(|b| (Box::leak(format!("{name}-symbolic").into_boxed_str()) as &str, b))
        })?;
    let t = load_png(ctx, resolved, bytes);
    cache.insert(leak(resolved), t.clone());
    Some(t)
}

fn lookup_png(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// 解码 PNG 并按 symbolic 语义着色：RGB 一律重写为前景色（亮灰 215，
/// 与 egui 暗色的可交互文本一致），保留原 alpha —— 复现 GTK 主题对
/// symbolic 图标的着色。
fn load_png(ctx: &egui::Context, name: &str, bytes: &[u8]) -> egui::TextureHandle {
    let img = image::load_from_memory(bytes).expect("图标 PNG 解码失败").to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<egui::Color32> = img
        .pixels()
        .map(|p| {
            let [_, _, _, a] = p.0;
            // 原图 fill=#bebebe；alpha（含抗锯齿边缘）就是形状
            egui::Color32::from_rgba_unmultiplied(215, 215, 215, a)
        })
        .collect();
    ctx.load_texture(
        leak(&format!("icon-{name}")),
        egui::ColorImage {
            size: [w, h],
            source_size: egui::Vec2::new(w as f32, h as f32),
            pixels,
        },
        egui::TextureOptions::LINEAR,
    )
}

/// 动态纹理名不是热路径（每图标只加载一次），一次性泄漏换 `'static` 键。
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// 把片段按开关状态合并进框架，得到 toolbar1 的**有效条目序列**。
///
/// 顺序语义照 GTK UI manager：占位符 = 内联条目 + （片段启用时的）片段条目。
pub fn expand(framework: &'static [TbNode], fragments: &[bool; 8]) -> Vec<&'static TbNode> {
    let mut out = Vec::new();
    for node in framework {
        match node {
            TbNode::Placeholder { name, items } => {
                for it in items.iter() {
                    out.push(it);
                }
                // 该占位符对应的片段（按 FRAGMENT_ITEMS 的顺序对位）
                if let Some(i) = FRAGMENT_ITEMS.iter().position(|f| f.0 == *name) {
                    if fragments[i] {
                        for id in FRAGMENT_ITEMS[i].1 {
                            // 每个片段条目包一层 Item 节点
                            out.push(emit_item(id));
                        }
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

// leak 一个 Item 节点（expand 的产出是 &'static TbNode）
fn emit_item(id: &'static str) -> &'static TbNode {
    Box::leak(Box::new(TbNode::Item(id)))
}

/// 画一条 toolitem：24×24 图标按钮（ICONS 风格）。
///
/// 入参全是**数据快照**（可用性/按下态/icon-name/tooltip 由调用方从
/// `PigmentApp` 取好，避免 `&mut self` 与图标缓存的双重借用）：
/// - 置灰条目图标 alpha 减半、不显示 tooltip（GTK insensitive 不弹提示）；
/// - toggle 语义的动作（`pressed`）用 `Button::selected` 的高亮。
///
/// 返回是否被点击；执行动作是调用方的事（`run_action`）。
pub struct ToolBtn {
    pub available: bool,
    pub pressed: bool,
    pub icon_name: Option<&'static str>,
    /// tooltip 文本（label，可含加速键）。`None` = 置灰，不弹。
    pub tip: Option<String>,
    /// 图标缺失时的文本回退（动作 short-label）。
    pub fallback_label: String,
}

pub fn tool_button(
    ui: &mut egui::Ui,
    cache: &mut IconCache,
    b: ToolBtn,
) -> bool {
    let mut btn = match b.icon_name.and_then(|n| icon_texture(ui.ctx(), cache, n)) {
        Some(tex) => {
            let mut img = egui::Image::new(&tex).max_size(egui::vec2(24.0, 24.0));
            if !b.available {
                img = img.tint(egui::Color32::from_rgba_unmultiplied(215, 215, 215, 90));
            }
            egui::Button::image(img).selected(b.pressed)
        }
        None => {
            // 图标缺失：退回 short-label 文本（GTK 画"缺图"占位，文字更能表达动作）
            let mut btn = egui::Button::new(b.fallback_label).selected(b.pressed);
            if !b.available {
                btn = btn.small();
            }
            btn
        }
    };
    btn = btn.min_size(egui::vec2(30.0, 30.0));
    let resp = ui.add(btn);
    if b.available {
        if let Some(tip) = b.tip {
            // on_hover_text 消费 Response，clone 保住点击判定
            let _ = resp.clone().on_hover_text(tip);
        }
    }
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_nodes<'a>(nodes: impl IntoIterator<Item = &'a TbNode>) -> (usize, usize) {
        // (toolitem 数, separator 数)，占位符递归
        let mut items = 0;
        let mut seps = 0;
        for n in nodes {
            match n {
                TbNode::Item(_) => items += 1,
                TbNode::Separator => seps += 1,
                TbNode::Placeholder { items: inner, .. } => {
                    let (i, s) = count_nodes(inner.iter());
                    items += i;
                    seps += s;
                }
            }
        }
        (items, seps)
    }

    #[test]
    fn framework_matches_toolbar_xml() {
        // toolbar.xml 的 toolbar1：3 个占位符为空 + blendmodes/linemodes 各 1 内联
        // + 5 个内联 toolitem + 3 个空占位符
        let (items, seps) = count_nodes(TOOLBAR1_FRAMEWORK.iter());
        assert_eq!(items, 7, "框架内联 toolitem（BlendModeEraser、FreehandMode、ColorPickMode、FloodFillMode、LayerMoveMode、FrameEditMode、SymmetryEditMode）");
        assert_eq!(seps, 5, "框架分隔线数（toolbar.xml 实测）");
        // toolbar2：ColorAdjusters BrushGroups | sep | Toggle×5 | sep | Fullscreen
        let (items2, seps2) = count_nodes(TOOLBAR2.iter());
        assert_eq!(items2, 8, "toolbar2 条目数");
        assert_eq!(seps2, 2, "toolbar2 分隔线数");
    }

    #[test]
    fn fragments_table_aligns_with_mergeable_xml() {
        // toolbar.py:26-39 的 8 个片段，顺序与 FRAGMENT_ITEMS 一一对应
        assert_eq!(FRAGMENTS.len(), 8);
        assert_eq!(FRAGMENT_ITEMS.len(), 8);
        let expected_actions = [
            "toolbar1_file",
            "toolbar1_scrap",
            "toolbar1_edit",
            "toolbar1_blendmodes",
            "toolbar1_linemodes",
            "toolbar1_view_modes",
            "toolbar1_view_manips",
            "toolbar1_view_resets",
        ];
        for (i, frag) in FRAGMENTS.iter().enumerate() {
            assert_eq!(frag.action, expected_actions[i]);
            assert!(!frag.label.is_empty());
        }
        // 片段内容与源文件一致（toolbar-file.xml: New Open Save 等）
        assert_eq!(FRAGMENT_ITEMS[0].1, &["New", "Open", "Save"]);
        assert_eq!(FRAGMENT_ITEMS[2].1, &["Undo", "Redo"]);
        assert_eq!(
            FRAGMENT_ITEMS[4].1,
            &["StraightMode", "SequenceMode", "EllipseMode", "InkingMode"]
        );
        // 片段引用的动作都在动作表里
        for (_, ids) in FRAGMENT_ITEMS {
            for id in *ids {
                assert!(
                    crate::shell::actions::by_id(id).is_some(),
                    "片段引用了未知动作：{id}"
                );
            }
        }
    }

    #[test]
    fn expand_respects_fragment_switches() {
        // 全关 = 只有框架内联条目
        let all_off = expand(TOOLBAR1_FRAMEWORK, &[false; 8]);
        let (items, _) = count_nodes(all_off.iter().copied());
        assert_eq!(items, 7);
        // 出厂默认：+file(3) +edit(2) +linemodes(4) +view_modes(4) +view_resets(2)
        let defaults = expand(TOOLBAR1_FRAMEWORK, &DEFAULT_FRAGMENTS);
        let (items, _) = count_nodes(defaults.iter().copied());
        assert_eq!(items, 7 + 3 + 2 + 4 + 4 + 2, "出厂默认的工具条条目数");
        // 顺序：file 片段条目追加在 file 占位符内联条目（空）之后，
        // 即 toolbar1 的最前面是 New Open Save
        let first = match defaults.first() {
            Some(TbNode::Item(id)) => *id,
            other => panic!("首条目应是 Item，实得 {other:?}"),
        };
        assert_eq!(first, "New");
    }

    #[test]
    fn every_toolbar_action_has_an_icon_file() {
        // toolbar1/toolbar2 + 片段引用的每个动作的 icon-name 都要能解析出
        // PNG（精确名或 -symbolic 后缀）——缺图的按钮会退化成文字，这里卡死
        fn check(id: &str) {
            let a = crate::shell::actions::by_id(id).unwrap_or_else(|| panic!("未知动作 {id}"));
            if let Some(icon) = a.icon_name {
                let hit = ICONS.iter().any(|(n, _)| *n == icon)
                    || ICONS.iter().any(|(n, _)| *n == format!("{icon}-symbolic"));
                assert!(hit, "{id} 的图标 {icon} 缺文件");
            }
        }
        fn walk<'a>(nodes: impl IntoIterator<Item = &'a TbNode>) {
            for n in nodes {
                match n {
                    TbNode::Item(id) => check(id),
                    TbNode::Separator => {}
                    TbNode::Placeholder { items, .. } => walk(items.iter()),
                }
            }
        }
        walk(TOOLBAR1_FRAMEWORK.iter());
        walk(TOOLBAR2.iter());
        for (_, ids) in FRAGMENT_ITEMS {
            for id in *ids {
                check(id);
            }
        }
    }

    #[test]
    fn default_fragments_match_userconfig_py() {
        // userconfig.py:76-85 的出厂默认
        assert_eq!(
            DEFAULT_FRAGMENTS,
            [true, false, true, false, true, true, false, true]
        );
    }
}
