//! 构建期解析 MyPaint 的动作定义，生成 Rust 动作表。
//!
//! 事实来源：`gui/resources.xml`（照搬在 `assets/gui/resources.xml`）。
//! 它是 **MyPaint 所有菜单项、工具条按钮、加速键的唯一声明处**：
//!
//! ```text
//! GtkUIManager(app_ui_manager)
//! └ GtkActionGroup(FileActions | DocumentActions | BrushModifierActions
//!                  | WindowActions | ModeStackActions)
//!   └ GtkAction / GtkToggleAction / GtkRadioAction / GtkRecentAction
//!                              / MyPaintFactoryAction   (id = 动作名)
//!     ├ property label / short-label / tooltip / icon-name / value ...
//!     └ signal   activate|changed|toggled|item-activated  handler=*_cb
//!   └ accelerator key=... modifiers=GDK_*_MASK      ← 与动作同级
//! ```
//!
//! **一个都不能手抄**：219 个动作 / 105 条加速键，手抄必错，而且上游一改
//! 就再也对不上。这里把整棵 XML 在构建期翻成静态表，S3 的菜单栏、S4 的
//! 工具条、B6 的按键编辑器都从这张表渲染，`gui/menu.xml` 与
//! `gui/toolbar*.xml` 只按动作 id 引用。
//!
//! 生成物：`$OUT_DIR/actions_generated.rs`，由 `src/shell/actions.rs` 用
//! `include!` 引入。
//!
//! 与 MyPaint 的对应关系（逐条可查）：
//! - `Gtk.Action.get_label()` → [`Action::label`]（`translatable="yes"` 的原文；
//!   本项目暂不做 i18n，取英文原文，将来接 gettext 时换这一处即可）
//! - `Gtk.Action.get_short_label()` → [`Action::short_label`]（工具条用的短标签）
//! - `Gtk.Action.get_tooltip()` → [`Action::tooltip`]
//! - `Gtk.Action.get_icon_name()` → [`Action::icon_name`]
//!   （源码里两种拼写都出现过：`icon-name` 113 处、`icon_name` 6 处，统一归一化）
//! - 动作路径 `<Actions>/{group}/{id}`（`gui/accelmap.py:169`）

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("assets").join("gui").join("resources.xml");
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed=build.rs");

    let text = std::fs::read_to_string(&src).expect("读不到 assets/gui/resources.xml");
    let doc = roxmltree::Document::parse(&text).expect("resources.xml 解析失败");

    // ---- 动作组（保持 XML 顺序）----
    let mut groups: Vec<&str> = Vec::new();
    let mut rows: Vec<String> = Vec::new();
    let mut accel_count = 0usize;
    let mut unknown_classes: Vec<String> = Vec::new();

    for group_node in doc.descendants().filter(|n| {
        n.is_element() && tag_is(*n, "object") && attr(*n, "class") == Some("GtkActionGroup")
    }) {
        let group = group_node.attribute("id").unwrap_or_default();
        groups.push(group);

        // 组内每个直接子 `child` = 一个动作（+ 紧随其后的 accelerator）
        for child in group_node
            .children()
            .filter(|n| n.is_element() && tag_is(*n, "child"))
        {
            let Some(obj) = child
                .children()
                .find(|n| n.is_element() && tag_is(*n, "object"))
            else {
                continue;
            };
            let class = obj.attribute("class").unwrap_or_default();
            let kind = match class {
                "GtkAction" => "Kind::Action",
                "GtkToggleAction" => "Kind::Toggle",
                "GtkRadioAction" => "Kind::Radio",
                "GtkRecentAction" => "Kind::Recent",
                "MyPaintFactoryAction" => "Kind::Factory",
                other => {
                    unknown_classes.push(format!(
                        "{group}/{}: {other}",
                        attr(obj, "id").unwrap_or("?")
                    ));
                    "Kind::Action"
                }
            };
            let id = obj.attribute("id").unwrap_or_default();
            let prop = |name: &str| -> Option<String> {
                obj.children()
                    .filter(|n| n.is_element() && tag_is(*n, "property"))
                    .find(|n| attr(*n, "name") == Some(name))
                    .and_then(|n| n.text())
                    .map(|t| t.to_string())
            };
            // `icon_name` 是源码里的另一种拼写（见模块文档）
            let icon_name = prop("icon-name").or_else(|| prop("icon_name"));
            let signal = obj
                .children()
                .find(|n| n.is_element() && tag_is(*n, "signal"));
            let handler = signal.and_then(|n| attr(n, "handler"));
            let signal_name = signal.and_then(|n| attr(n, "name"));

            let mut accels: Vec<String> = Vec::new();
            for a in child
                .children()
                .filter(|n| n.is_element() && tag_is(*n, "accelerator"))
            {
                let key = a.attribute("key").unwrap_or_default();
                let mods = a.attribute("modifiers").unwrap_or("");
                accels.push(format!(
                    "Some(Accel {{ key: \"{}\", mods: {} }})",
                    esc(key),
                    mods_expr(mods, group, id)
                ));
                accel_count += 1;
            }
            if accels.len() > 1 {
                println!("cargo:warning={group}/{id} 有多条加速键，本表只取第一条（其余进 EXTRA_ACCELS）");
            }
            let accel = accels.first().cloned().unwrap_or_else(|| "None".into());

            let row = format!(
                concat!(
                    "    Action {{\n",
                    "        id: \"{}\", group: \"{}\", kind: {},\n",
                    "        label: {}, short_label: {}, tooltip: {}, icon_name: {},\n",
                    "        handler: {}, signal: {},\n",
                    "        radio_group: {}, value: {}, current_value: {},\n",
                    "        accel: {},\n",
                    "    }},"
                ),
                esc(id),
                esc(group),
                kind,
                opt_str(prop("label").as_deref()),
                opt_str(prop("short-label").as_deref()),
                opt_str(prop("tooltip").as_deref()),
                opt_str(icon_name.as_deref()),
                opt_str(handler),
                opt_str(signal_name),
                opt_str(prop("group").as_deref()),
                opt_num(prop("value").as_deref()),
                opt_num(prop("current-value").as_deref()),
                accel,
            );
            rows.push(row);
        }
    }

    let mut head = String::new();
    let _ = writeln!(
        head,
        "// @generated by build.rs —— MyPaint 动作表（gui/resources.xml）。请勿手工编辑。\n\
         // 重新生成：改动 assets/gui/resources.xml 后 `cargo build -p pigment-app`。\n"
    );
    let _ = writeln!(
        head,
        "/// `resources.xml` 里 GtkActionGroup 的 id，**保持 XML 中的出现顺序**。\npub static GROUPS: &[&str] = &[{}];\n",
        groups
            .iter()
            .map(|g| format!("\"{}\"", esc(g)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let generated = format!(
        "{head}pub static ACTIONS: &[Action] = &[\n{}\n];\n",
        rows.join("\n")
    );

    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("actions_generated.rs");
    std::fs::write(&dest, generated).expect("写入 actions_generated.rs 失败");

    for c in &unknown_classes {
        println!("cargo:warning=未知动作类（按 GtkAction 处理）：{c}");
    }
    println!(
        "cargo:warning=动作表已生成：{} 组 / {} 个动作 / {} 条加速键（来自 gui/resources.xml）",
        groups.len(),
        rows_count(&dest),
        accel_count
    );

    // ------------------------------------------------------------------
    // 菜单树（gui/menu.xml → menus_generated.rs）
    //
    // menu.xml 是纯声明式的 UI 描述：`<menu action=…>`（子菜单，标题取该动作
    // 的 label）、`<menuitem action=…>`（条目）、`<separator/>`。照搬即翻译。
    generate_menus(&manifest, &actions_by_id(&doc));

    // ------------------------------------------------------------------
    // 工具条（gui/toolbar.xml + 8 个片段 → toolbars_generated.rs）
    generate_toolbars(&manifest, &actions_by_id(&doc));

    // ------------------------------------------------------------------
    // 工具条图标（assets/icons/*.svg → 栅格化为 PNG，内嵌字节）
    generate_icons(&manifest);
}

/// 所有已声明动作的 id 集合（供菜单树校验引用）。
fn actions_by_id(doc: &roxmltree::Document) -> Vec<String> {
    doc.descendants()
        .filter(|n| {
            n.is_element()
                && tag_is(*n, "object")
                && attr(*n, "class").is_some_and(|c| c.ends_with("Action"))
        })
        .filter_map(|n| attr(n, "id"))
        .map(str::to_owned)
        .collect()
}

fn generate_menus(manifest: &std::path::Path, known_actions: &[String]) {
    let src = manifest.join("assets").join("gui").join("menu.xml");
    println!("cargo:rerun-if-changed={}", src.display());
    let text = std::fs::read_to_string(&src).expect("读不到 assets/gui/menu.xml");
    let doc = roxmltree::Document::parse(&text).expect("menu.xml 解析失败");

    // 动态槽位动作（document.py:_init_context_actions 在运行期注册，
    // 不在 resources.xml 里）——菜单里引用它们是合法的。
    let dynamic: Vec<String> = (0..10)
        .flat_map(|n| vec![format!("Context0{n}"), format!("Context0{n}s")])
        .collect();
    let known = |id: &str| -> bool {
        known_actions.iter().any(|a| a == id) || dynamic.iter().any(|a| a == id)
    };

    let menubar = doc
        .descendants()
        .find(|n| n.is_element() && tag_is(*n, "menubar"))
        .expect("menu.xml 里没有 <menubar>");

    let mut item_count = 0usize;
    let mut sep_count = 0usize;
    let mut sub_count = 0usize;

    fn gen(
        node: roxmltree::Node,
        known: &dyn Fn(&str) -> bool,
        item_count: &mut usize,
        sep_count: &mut usize,
        sub_count: &mut usize,
    ) -> Vec<String> {
        node.children()
            .filter(|n| n.is_element())
            .map(|child| match tag_is(child, "menu") {
                true => {
                    let action =
                        attr(child, "action").unwrap_or_else(|| panic!("<menu> 缺 action 属性"));
                    assert!(known(action), "menu.xml 引用了未声明的动作：{action}");
                    *sub_count += 1;
                    let items = gen(child, known, item_count, sep_count, sub_count);
                    format!(
                        "MenuNode::SubMenu {{ action: \"{}\", items: &[{}] }}",
                        esc(action),
                        items.join(", ")
                    )
                }
                false => {
                    assert!(
                        tag_is(child, "menuitem") || tag_is(child, "separator"),
                        "menu.xml 里的意外元素：<{}>",
                        child.tag_name().name()
                    );
                    if tag_is(child, "separator") {
                        *sep_count += 1;
                        "MenuNode::Separator".to_string()
                    } else {
                        let action = attr(child, "action")
                            .unwrap_or_else(|| panic!("<menuitem> 缺 action 属性"));
                        assert!(known(action), "menu.xml 引用了未声明的动作：{action}");
                        *item_count += 1;
                        format!("MenuNode::Item(\"{}\")", esc(action))
                    }
                }
            })
            .collect()
    }

    let top = gen(
        menubar,
        &known,
        &mut item_count,
        &mut sep_count,
        &mut sub_count,
    );
    assert_eq!(top.len(), 9, "顶级菜单数应为 9（menu.xml 的 Menubar）");

    let generated = format!(
        "// @generated by build.rs —— MyPaint 菜单树（gui/menu.xml）。请勿手工编辑。\n\
         // 每个动作 id 都已在构建期校验存在于动作表（或为运行期的槽位动作）。\n\
         pub static MENUBAR: &[MenuNode] = &[\n{}\n];\n",
        top.join(",\n")
    );
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("menus_generated.rs");
    std::fs::write(&dest, generated).expect("写入 menus_generated.rs 失败");

    println!(
        "cargo:warning=菜单树已生成：9 顶级 / {} 子菜单（13 嵌套）/ {item_count} 条目 / {sep_count} 分隔线（来自 gui/menu.xml）",
        sub_count
    );
}

/// 工具条节点。
#[derive(Clone, Debug)]
enum TbNode {
    /// `<toolitem action="…"/>`
    Item(String),
    /// `<separator/>`
    Separator,
    /// `<placeholder name="…">` —— 片段按占位符名合并；`items` 为框架内联条目
    Placeholder { name: String, items: Vec<TbNode> },
}

/// 解析一个 `<ui>` 文件，返回每个 `<toolbar name=…>` 的节点序列。
fn parse_toolbar_xml(path: &std::path::Path) -> Vec<(String, Vec<TbNode>)> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
    let doc = roxmltree::Document::parse(&text)
        .unwrap_or_else(|e| panic!("{} 解析失败：{e}", path.display()));
    let mut out = Vec::new();
    for tb in doc
        .descendants()
        .filter(|n| n.is_element() && tag_is(*n, "toolbar"))
    {
        let name = attr(tb, "name").unwrap_or("?").to_string();
        out.push((name, gen_tb_nodes(tb)));
    }
    out
}

fn gen_tb_nodes(node: roxmltree::Node) -> Vec<TbNode> {
    node.children()
        .filter(|n| n.is_element())
        .map(|child| {
            if tag_is(child, "toolitem") {
                TbNode::Item(attr(child, "action").unwrap_or("?").to_string())
            } else if tag_is(child, "separator") {
                TbNode::Separator
            } else if tag_is(child, "placeholder") {
                let name = attr(child, "name").unwrap_or("?").to_string();
                TbNode::Placeholder {
                    name,
                    items: gen_tb_nodes(child),
                }
            } else {
                panic!("toolbar XML 里的意外元素：<{}>", child.tag_name().name());
            }
        })
        .collect()
}

fn emit_tb(nodes: &[TbNode], depth: usize) -> String {
    let ind = "    ".repeat(depth + 1);
    nodes
        .iter()
        .map(|n| match n {
            TbNode::Item(id) => format!("{ind}TbNode::Item(\"{}\"),", esc(id)),
            TbNode::Separator => format!("{ind}TbNode::Separator,"),
            TbNode::Placeholder { name, items } => format!(
                "{ind}TbNode::Placeholder {{ name: \"{}\", items: &[\n{}\n{ind}]}},",
                esc(name),
                emit_tb(items, depth + 1)
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 解析框架与 8 个片段 → `$OUT_DIR/toolbars_generated.rs`。
///
/// 框架 `toolbar.xml` 声明 toolbar1（占位符 + 内联条目）与 toolbar2；
/// 片段文件各含一个同名占位符（条目在运行期按偏好合并，见
/// `gui/toolbar.py:80-126` 的 UI manager merge 行为）。
/// 每个动作 id 都在构建期校验存在于动作表。
fn generate_toolbars(manifest: &std::path::Path, known_actions: &[String]) {
    let toolbars = parse_toolbar_xml(&manifest.join("assets/gui/toolbar.xml"));
    let get = |name: &str| -> Vec<TbNode> {
        toolbars
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("toolbar.xml 里没有 <toolbar name=\"{name}\">"))
    };
    let framework1 = get("toolbar1");
    let framework2 = get("toolbar2");

    // 8 个片段（toolbar.py:26-39 的 MERGEABLE_XML 列出文件与顺序）
    let mut frag_rows: Vec<String> = Vec::new();
    for file in [
        "toolbar-file.xml",
        "toolbar-scrap.xml",
        "toolbar-edit.xml",
        "toolbar-blendmodes.xml",
        "toolbar-linemodes.xml",
        "toolbar-view-modes.xml",
        "toolbar-view-manips.xml",
        "toolbar-view-resets.xml",
    ] {
        let tbs = parse_toolbar_xml(&manifest.join("assets/gui").join(file));
        assert_eq!(tbs.len(), 1, "{file} 应只含一个 <toolbar> 框架");
        match &tbs[0].1[..] {
            [TbNode::Placeholder { name, items }] => {
                let its: Vec<String> = items
                    .iter()
                    .map(|a| match a {
                        TbNode::Item(id) => format!("\"{}\"", esc(id)),
                        other => panic!("{file} 片段里出现了非条目节点：{other:?}"),
                    })
                    .collect();
                frag_rows.push(format!("    (\"{}\", &[{}]),", esc(name), its.join(", ")));
            }
            _ => panic!("{file} 片段的顶层应是单个 placeholder"),
        }
    }

    let known = |id: &str| -> bool { known_actions.iter().any(|a| a == id) };
    fn validate(nodes: &[TbNode], known: &dyn Fn(&str) -> bool) {
        for n in nodes {
            match n {
                TbNode::Item(id) => assert!(known(id), "工具条引用了未声明的动作：{id}"),
                TbNode::Separator => {}
                TbNode::Placeholder { items, .. } => validate(items, known),
            }
        }
    }
    validate(&framework1, &known);
    validate(&framework2, &known);

    let generated = format!(
        "// @generated by build.rs —— MyPaint 工具条（gui/toolbar*.xml）。请勿手工编辑。\n\
         pub static TOOLBAR1_FRAMEWORK: &[TbNode] = &[\n{}\n];\n\n\
         pub static TOOLBAR2: &[TbNode] = &[\n{}\n];\n\n\
         /// 片段条目（toolbar.py MERGEABLE_XML 所列 8 个文件）：\n\
         /// (placeholder 名, 动作 id…)。运行期按偏好 `ui.toolbar_items` 合并进\n\
         /// 框架的同名占位符（GTK UI manager 的 merge 行为：追加在占位符\n\
         /// 内联条目之后）。\n\
         pub static FRAGMENT_ITEMS: &[(&str, &[&str])] = &[\n{}\n];\n",
        emit_tb(&framework1, 0),
        emit_tb(&framework2, 0),
        frag_rows.join("\n")
    );
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("toolbars_generated.rs");
    std::fs::write(&dest, generated).expect("写入 toolbars_generated.rs 失败");
    println!(
        "cargo:warning=工具条已生成：toolbar1 框架 + 8 片段 + toolbar2（来自 gui/toolbar*.xml）"
    );
}

/// 图标：`assets/icons/*.svg`（MyPaint 的 hicolor scalable symbolic 源文件，
/// GPL，随项目分发）→ resvg 栅格化为 24×24 PNG（`ui.toolbar_icon_size` 的
/// 默认 "large" = LARGE_TOOLBAR，GTK 文档承诺 24×24）→ `$OUT_DIR/icons/`，
/// 生成 `ICONS: &[(&str, &[u8])]`（文件名去 .svg → PNG 字节）。
fn generate_icons(manifest: &std::path::Path) {
    let dir = manifest.join("assets").join("icons");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("读不到 assets/icons")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "svg" || x == "png")
        })
        .map(|e| e.path().file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.dedup();

    const SIZE: u32 = 24;
    let mut rows: Vec<String> = Vec::new();
    let mut opt = resvg::tiny_skia::Pixmap::new(SIZE, SIZE).unwrap();
    for name in &names {
        let svg_path = dir.join(format!("{name}.svg"));
        let png_path = dir.join(format!("{name}.png"));
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap())
            .join("icons")
            .join(format!("{name}.png"));
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        if svg_path.is_file() {
            let text = std::fs::read_to_string(&svg_path)
                .unwrap_or_else(|e| panic!("读不到 {}: {e}", svg_path.display()));
            let tree = resvg::usvg::Tree::from_str(&text, &resvg::usvg::Options::default())
                .unwrap_or_else(|e| panic!("{name}.svg 解析失败：{e}"));
            opt.fill(resvg::tiny_skia::Color::TRANSPARENT);
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(
                    SIZE as f32 / tree.size().width(),
                    SIZE as f32 / tree.size().height(),
                ),
                &mut opt.as_mut(),
            );
            let png = opt
                .encode_png()
                .unwrap_or_else(|e| panic!("{name}.png 编码失败：{e}"));
            std::fs::write(&out, png).unwrap();
        } else {
            // 源码只有 PNG 的图标（如三种旧版颜色器）：原样内嵌。
            let png = std::fs::read(&png_path)
                .unwrap_or_else(|e| panic!("读不到 {}: {e}", png_path.display()));
            std::fs::write(&out, png).unwrap();
        }
        rows.push(format!(
            "    (\"{}\", include_bytes!(concat!(env!(\"OUT_DIR\"), \"/icons/{}.png\"))),",
            esc(name),
            esc(name)
        ));
    }

    let generated = format!(
        "// @generated by build.rs —— MyPaint 工具条图标（assets/icons/*.svg，GPL）。\n\
         // 24×24 PNG（偏好 ui.toolbar_icon_size 默认 large）。\n\
         pub static ICONS: &[(&str, &[u8])] = &[\n{}\n];\n",
        rows.join("\n")
    );
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icons_generated.rs");
    std::fs::write(&dest, generated).expect("写入 icons_generated.rs 失败");
    println!(
        "cargo:warning=图标已生成：{} 枚 24×24（SVG resvg 栅格化 + 源码 PNG 内嵌）",
        names.len()
    );
}

/// 粗略校验：生成文件里的 `Action {` 行数。
fn rows_count(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.matches("    Action {").count())
        .unwrap_or(0)
}

fn tag_is(n: roxmltree::Node, tag: &str) -> bool {
    n.tag_name().name() == tag
}

fn attr<'a>(n: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attribute(name)
}

/// GDK 修饰键名 → `Mods` 字面量。`GDK_CONTROL_MASK` 等 6 种，未识别会告警。
fn mods_expr(mods: &str, group: &str, id: &str) -> String {
    let (mut ctrl, mut alt, mut shift, mut other) = (false, false, false, Vec::new());
    for m in mods.split('|').map(str::trim).filter(|m| !m.is_empty()) {
        match m {
            "GDK_CONTROL_MASK" => ctrl = true,
            "GDK_MOD1_MASK" => alt = true,
            "GDK_SHIFT_MASK" => shift = true,
            other_m => other.push(other_m),
        }
    }
    for m in &other {
        println!("cargo:warning={group}/{id}: 未处理的修饰键 {m}（已忽略）");
    }
    format!("Mods {{ ctrl: {ctrl}, alt: {alt}, shift: {shift} }}")
}

fn opt_str(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("Some(\"{}\")", esc(s)),
        None => "None".into(),
    }
}

fn opt_num(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("Some({})", s.trim()),
        None => "None".into(),
    }
}

/// 转义成 Rust 字符串字面量内容。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}
