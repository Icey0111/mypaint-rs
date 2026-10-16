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

    for group_node in doc
        .descendants()
        .filter(|n| n.is_element() && tag_is(*n, "object") && attr(*n, "class") == Some("GtkActionGroup"))
    {
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
                    unknown_classes.push(format!("{group}/{}: {other}", attr(obj, "id").unwrap_or("?")));
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
    let generated = format!("{head}pub static ACTIONS: &[Action] = &[\n{}\n];\n", rows.join("\n"));

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
