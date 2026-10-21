//! GTK 加速键的**文本模型** + `accelmap.conf` 的读写 —— `gui/accelmap.py` 与
//! `gui/keyboard.py` 里与 GUI 无关的那一半。
//!
//! # 这一层管什么
//!
//! | 语义 | 源码 |
//! |---|---|
//! | 加速键字符串 ⇄ (键, 修饰键)（`<Primary>z`） | `Gtk.accelerator_parse` / `accelerator_name`，见 `gui/keyboard.py:307`、`gui/accelmap.py:572,590` |
//! | 显示名（`Ctrl+Z` / `Shift+Ctrl+S`） | `Gtk.accelerator_get_label`，见 `gui/accelmap.py:164,272,504` |
//! | `accelmap.conf` 的加载 | `gui/application.py:417` |
//! | `accelmap.conf` 的保存（**整表快照**） | `gui/application.py:625` |
//! | 文件与 `settings.json` 同目录 | `gui/application.py:417,503,625` → [`crate::prefs::config_dir`] |
//!
//! 编辑器 UI（`AccelMapEditor`）、`keymap`/`keymap2` 两级查表、按键捕获都在
//! `pigment-app` 的 `shell::accelmap`；这里只留**能逐字节对照**的文本部分。
//!
//! # 为什么放在引擎层
//!
//! 1. `accelmap.conf` 与 `settings.json` 同目录（`gui/application.py:417` /
//!    `:503` / `:625`），目录规则在 [`crate::prefs::config_dir`]（B6-1）。放一处
//!    才不会两套平台判断打架。
//! 2. 纯文本：不依赖 egui/wgpu，测试不需要 GPU，也不需要 `eframe`（应用层的
//!    `PigmentApp` 构造不出来，见 `app.rs` 的测试注释）。
//! 3. MyPaint 侧这两个函数（`Gtk.*`）本来就是 GTK 的，不属于"界面"逻辑。
//!
//! # 文件格式：`accelmap.conf` 是 GTK 的 rc 文件
//!
//! 真样本由 `crates/pigment-engine/tests/tools/gen-accelmap-fixtures.py` 让
//! **真 GTK 3.24.20 自己写出来**（`Gtk.AccelMap.save()`），存在
//! `tests/fixtures/accelmap_gtk.conf`。逐字节长这样：
//!
//! ```text
//! ; mypaint GtkAccelMap rc-file         -*- scheme -*-
//! ; this file is an automated accelerator map dump
//! ;
//! (gtk_accel_path "<Actions>/FileActions/Save" "<Primary>s")
//! ; (gtk_accel_path "<Actions>/Some/Action" "")
//! ```
//!
//! 三条必须知道的事实（都来自上面那个真样本与 GTK 3.24.20 的实测，不是猜的）：
//!
//! 1. **头三行是固定的**：第一行的 `mypaint` 是 `g_get_prgname()`
//!    （探针：把 prgname 换成 `pigment` 那一行就跟着变；`rc-file` 与
//!    `-*- scheme -*-` 之间有 **9 个空格**）。
//! 2. **保存的是整张表，不是"用户的增量"**：`gtk_accel_map_save()` 遍历
//!    `accel_entries` 全表，所以默认绑定（`resources.xml` 的 105 条 +
//!    `document.py` 的 20 条槽位键）全都会被写进文件。于是"用户文件"其实是
//!    上一次退出时的**全表快照**，加载时逐 path 覆盖默认表
//!    （实测：文件里 `Quit` 改成 `<Primary>w` 后，加载完 `lookup_entry` 拿到
//!    的就是 `<Primary>w`）。
//! 3. **被清空的条目写成注释行**：`key == 0` 的条目写成
//!    `; (gtk_accel_path "path" "")`（实测原文如此）。加载器**跳过注释**，
//!    所以"删掉一个默认绑定"不会跨进程保留 —— 下次启动 UI manager 会重新
//!    注册默认绑定，绑定就回来了。本模块照搬这一行为（见 [`Entry::accel`]）。
//!
//! # 与 GTK 的差异（都往保守方向，逐条记账）
//!
//! - **条目顺序**：GTK 用 `GHashTable` 的哈希序（不可依赖，同一份数据两次
//!   运行的顺序都可能不同）。这里按 path **字典序**写，确定性优先。语义无关：
//!   加载是逐 path 覆盖。
//! - **换行**：GTK 在 Windows 上写出 CRLF（`g_fopen` 的文本模式），POSIX 上是
//!   LF。这里统一写 LF；解析两侧都吃（行尾的 `\r` 会在切行时去掉）。
//! - **一行一个条目**：GTK 的 rc 解析器是**分词**的，`(...)(...)` 挤在一行也能
//!   读。这里按行解析（GTK 自己写的文件永远一行一条）；坏行记进
//!   [`ParsedConf::skipped`] 并从**下一行**继续，这与实测一致（GTK 遇到坏行
//!   `not a valid line` 后，紧随其后的条目仍能加载）。
//! - **修饰键集合只有 Ctrl/Shift/Alt**：GTK 还有 `<Super>`/`<Hyper>`/`<Meta>`
//!   （实测三种都能 parse 并写出）。egui 的 `Modifiers` 没有对应位
//!   （只有 ctrl/alt/shift/mac_cmd/command），所以这三族**明确拒绝**
//!   （[`AccelError::UnsupportedModifier`]），不悄悄折叠成裸键 —— 折叠会让
//!   `<Super>a` 与 `a` 变成"同一个键"，进而造出假冲突。
//! - **键值名不做全表校验**：GTK 认整张 GDK 键值表（2000+ 名字），拼错就拒绝
//!   （实测 `Backspace`/`PageUp`/`Space` 都是非法，只能写 `BackSpace` /
//!   `Page_Up` / `space`）。本模块只做**形状**校验（字母数字下划线），因此那三
//!   个拼写错误会被收下 —— 代价是留一个"惰性绑定"（应用层的 egui 键位映射表
//!   认不出它，按了也没反应），好处是**不丢用户文件里的数据**。单测里把这三个
//!   拼写单列出来断言，别让差异悄悄变大。
//!
//! # 单字符与大小写：以 GTK 实测为准
//!
//! `Gtk.accelerator_parse()` 对**单个字母数字**有特殊处理：`Z` 与 `z` 都得到
//! 键值 `0x7a`、modifiers 为 0，规范名是 `z`（**小写**，且**不会**自动补
//! `<Shift>`）；而单个标点（`,`、`!`、空格）**一律非法**，必须写键值名
//! （`comma` / `exclam` / `space`）。这正是 `gui/keyboard.py:135-138` 那句
//! "key bindings are always stored in lowercase" 的底层原因，也是本模块
//! [`Accel::parse`] 对单字符只收字母数字的依据。
//!
//! # 名字里的修饰键顺序 vs 显示名的修饰键顺序
//!
//! 两者**不一样**，都是实测（别"顺手统一"）：
//! - 规范名：`<Primary>` → `<Shift>` → `<Alt>`（`<Primary><Shift>s`）
//! - 显示名：`Shift` → `Ctrl` → `Alt`（`Shift+Ctrl+S`）

use std::path::{Path, PathBuf};

use crate::prefs;

// ---- 文件名与进程名（逐条标源码行号）----

/// `accelmap.conf` —— `gui/application.py:417`（读）/ `:625`（写）。
pub const FILENAME: &str = "accelmap.conf";

/// 写进文件第一行的进程名 —— `Gtk.AccelMap.save()` 用的是 `g_get_prgname()`
/// （实测：prgname 为 `mypaint` 时第一行是 `; mypaint GtkAccelMap rc-file …`）。
///
/// 沿用 `mypaint` 而不是 `pigment`：这个文件与上游 MyPaint 的同名文件**可以互相
/// 直接读**，第一行只是注释，写谁都不影响解析。
pub const PROGRAM_NAME: &str = "mypaint";

/// `accelmap.conf` 的完整路径（`gui/application.py:417,625` 的
/// `join(self.user_confpath, "accelmap.conf")`）。
///
/// `user_confpath` 就是 `Prefs` 所在的配置目录，所以这里直接复用
/// [`crate::prefs::config_dir`]（`PIGMENT_CONFIG_DIR` 也照旧生效）——
/// 源码里两个文件本来就同目录。
pub fn accelmap_path() -> PathBuf {
    prefs::config_dir().join(FILENAME)
}

// ---- 修饰键 ----

/// 加速键的修饰键集合 —— `Gtk.accelerator_parse()` 结果里的 `GdkModifierType`
/// 只保留 egui 能表达的三位（见模块文档"与 GTK 的差异"）。
///
/// 字段名与 `shell::actions::Mods` 一致，两处各有一份是为了让引擎不依赖 egui
/// （应用层有 `From` 转换）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };

    pub fn new(ctrl: bool, shift: bool, alt: bool) -> Self {
        Mods { ctrl, shift, alt }
    }

    /// 一个修饰键都没按。
    pub fn is_empty(&self) -> bool {
        !self.ctrl && !self.shift && !self.alt
    }
}

// ---- 加速键 ----

/// 一条加速键：键值名（`gdk_keyval_name()` 的拼写）+ 修饰键。
///
/// [`Accel::key`] 是**规范拼写**（小写字母、键值名），可以直接与
/// `gdk_keyval_name()` 的结果比较，也可以直接写进 `accelmap.conf`。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Accel {
    pub key: String,
    pub mods: Mods,
}

/// [`Accel::parse`] 的失败原因。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AccelError {
    /// 空串（GTK 同样解析失败：键值为 0）。
    Empty,
    /// 只有修饰键，没有键（`<Shift>`）—— 实测 GTK 也返回键值 0。
    MissingKey,
    /// 键值名不合法：单个标点、含非法字符……（GTK 侧同样非法）
    BadKey(String),
    /// `<Super>` / `<Hyper>` / `<Meta>`：GTK 收，egui 表达不了（见模块文档）。
    UnsupportedModifier(&'static str),
    /// 不认识的修饰键写法（`<Foo>`）。
    UnknownModifier(String),
}

impl std::fmt::Display for AccelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccelError::Empty => write!(f, "空的加速键字符串"),
            AccelError::MissingKey => write!(f, "只有修饰键，没有键"),
            AccelError::BadKey(s) => write!(f, "键值名不合法：{s:?}"),
            AccelError::UnsupportedModifier(m) => {
                write!(f, "<{m}> 在 egui 里没有对应修饰位，不支持")
            }
            AccelError::UnknownModifier(m) => write!(f, "不认识的修饰键：<{m}>"),
        }
    }
}

impl Accel {
    /// 用规范键值名构造（调用方保证 `key` 已是规范拼写，例如捕获流程产出的
    /// 名字；非法字符不会被检查 —— 需要校验时走 [`Accel::parse`]）。
    pub fn new(key: impl Into<String>, mods: Mods) -> Self {
        Accel {
            key: key.into(),
            mods,
        }
    }

    /// 解析 GTK 的加速键字符串 —— `Gtk.accelerator_parse()`。
    ///
    /// 语法（实测 GTK 3.24.20，见模块文档）：
    /// - `<Primary>` / `<Control>` / `<Ctrl>` 都是 Ctrl（**大小写不敏感**）
    /// - `<Shift>`、`<Alt>` / `<Mod1>`、`<Super>` / `<Hyper>` / `<Meta>`（后三者拒绝）
    /// - 修饰键必须在键之前；键要么是**单个字母数字**（会小写化），要么是
    ///   `gdk_keyval_name()` 的拼写（`Page_Up` / `F5` / `comma` / `space`）
    pub fn parse(spec: &str) -> Result<Accel, AccelError> {
        if spec.is_empty() {
            return Err(AccelError::Empty);
        }
        let mut rest = spec;
        let mut mods = Mods::NONE;
        while let Some(after_lt) = rest.strip_prefix('<') {
            let Some(end) = after_lt.find('>') else {
                return Err(AccelError::BadKey(spec.to_string()));
            };
            let name = &after_lt[..end];
            match name.to_ascii_lowercase().as_str() {
                "primary" | "control" | "ctrl" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" | "mod1" => mods.alt = true,
                "super" => return Err(AccelError::UnsupportedModifier("Super")),
                "hyper" => return Err(AccelError::UnsupportedModifier("Hyper")),
                "meta" => return Err(AccelError::UnsupportedModifier("Meta")),
                _ => return Err(AccelError::UnknownModifier(name.to_string())),
            }
            rest = &after_lt[end + 1..];
        }
        if rest.is_empty() {
            return Err(AccelError::MissingKey);
        }
        Ok(Accel {
            key: canonical_key_name(rest)?,
            mods,
        })
    }

    /// 规范名 —— `Gtk.accelerator_name()`：`<Primary><Shift>s`。
    ///
    /// 顺序是**实测**的：Ctrl → Shift → Alt（`<Shift><Control>s` 解析后再写出来
    /// 是 `<Primary><Shift>s`）。
    pub fn to_gtk_name(&self) -> String {
        let mut s = String::new();
        if self.mods.ctrl {
            s.push_str("<Primary>");
        }
        if self.mods.shift {
            s.push_str("<Shift>");
        }
        if self.mods.alt {
            s.push_str("<Alt>");
        }
        s.push_str(&self.key);
        s
    }

    /// 显示名 —— `Gtk.accelerator_get_label()`，见 [`label`]。
    pub fn label(&self) -> String {
        label(&self.key, self.mods)
    }
}

/// 键值名的规范拼写 —— 单字符只收字母数字并**小写化**，其余必须是
/// `[A-Za-z][A-Za-z0-9_]*` 形状的键值名（详见模块文档的"键值名不做全表校验"）。
fn canonical_key_name(token: &str) -> Result<String, AccelError> {
    let mut chars = token.chars();
    let first = chars.next().expect("调用方保证非空");
    if chars.next().is_none() {
        // 单字符：GTK 只接受字母数字（`Z`/`z` → `z`），标点一律非法
        if first.is_ascii_alphanumeric() {
            return Ok(first.to_ascii_lowercase().to_string());
        }
        return Err(AccelError::BadKey(token.to_string()));
    }
    let shaped =
        first.is_ascii_alphabetic() && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !shaped {
        return Err(AccelError::BadKey(token.to_string()));
    }
    Ok(token.to_string())
}

// ---- 显示名（`Gtk.accelerator_get_label()`）----

/// 键名的**显示文字** —— 对应 GTK `gtkaccelgroup.c` 的取名字规则。
///
/// 规则是按本机 GTK 3.24.20 实测出来的（不是推导出来的；105 条加速键的真值
/// 存在 `pigment-app/tests/fixtures/accel_labels.tsv`，源码能碰到的全部键名的
/// 真值在 `tests/fixtures/accel_names.tsv`），顺序为：
///
/// 1. **特殊显示名表优先**：[`SPECIAL_KEY_LABELS`]（GTK 的 `key_labels` 翻译
///    表）。只收"与下面几条的产物不同"的项：`BackSpace` → `Backspace`
///    （**不是** `BackSpace`）、`space` → `Space`、`backslash` → `Backslash`
///    （**不是** `\`）、`ISO_Left_Tab` → `Left Tab`、`Multi_key` → `Multikey`、
///    `KP_Enter` → `Enter (keypad)`。GTK 表里还有 `Tab`/`Return`/`Delete` 之类，
///    但它们的显示名与键名相同，不必单列。
/// 2. **小键盘键**：[`KEYPAD_CHAR_LABELS`]。GTK 走的是"键值 → Unicode"那条路
///    （`KP_4` 的键值映射到 `'4'`），所以 `KP_Add` 显示 `+` 而不是 `KP Add`。
/// 3. 单字符键值（字母）**一律大写** —— 与是否按 Shift 无关，实测 `f` → `F`。
///    对应 GTK 内部先做 `gdk_keyval_to_upper()` 再取字符。
/// 4. 其余**可打印 ASCII 标点**显示字符本身：`period` → `.`、`grave` → `` ` ``、
///    `bracketright` → `]`、`bar` → `|`……（`backslash` 是这条的唯一例外，
///    因为第 1 条先命中）。
/// 5. 剩下的用键名，下划线换空格：`Page_Up` → `Page Up`、
///    `Page_Down` → `Page Down`；`F1`、`Delete`、`Home`、`End`、`Tab`、
///    `Left/Right/Up/Down` 原样。
pub fn key_label(gdk_key: &str) -> String {
    // 1. GTK 的特殊名表
    if let Some((_, label)) = SPECIAL_KEY_LABELS.iter().find(|(k, _)| *k == gdk_key) {
        return (*label).to_string();
    }
    // 2. 小键盘键（GTK 走键值 → Unicode）
    if let Some((_, label)) = KEYPAD_CHAR_LABELS.iter().find(|(k, _)| *k == gdk_key) {
        return (*label).to_string();
    }
    // 3/5. 单字符：字母大写，标点原样
    if gdk_key.chars().count() == 1 {
        return gdk_key.to_ascii_uppercase();
    }
    // 4. 可打印 ASCII 标点的键名 → 字符本身
    if let Some((_, c)) = ASCII_PUNCT_NAMES.iter().find(|(k, _)| *k == gdk_key) {
        return (*c).to_string();
    }
    gdk_key.replace('_', " ")
}

/// 加速键的显示名 —— `Gtk.accelerator_get_label()`：`Shift+Ctrl+S`。
///
/// 修饰键顺序 **Shift → Ctrl → Alt**（与规范名的 Ctrl → Shift → Alt 不同，
/// 两者都是实测，见模块文档）；无修饰键时只显示键名。
pub fn label(key: &str, mods: Mods) -> String {
    let mut parts: Vec<String> = Vec::new();
    if mods.shift {
        parts.push("Shift".into());
    }
    if mods.ctrl {
        parts.push("Ctrl".into());
    }
    if mods.alt {
        parts.push("Alt".into());
    }
    parts.push(key_label(key));
    parts.join("+")
}

/// GTK 为这几个键值准备了专门的显示名（优先于"显示字符本身"与"键名换空格"）。
///
/// 这是 GTK `gtkaccelgroup.c` 里 `key_labels[]` 的**可观测部分**：只收显示名与
/// 键名不同的项（`Tab`/`Return`/`Delete`/`Home` 之类在表里，但显示名与键名
/// 相同，不必单列）。全部由真 GTK 实测（见 `tests/fixtures/accel_names.tsv`）。
const SPECIAL_KEY_LABELS: &[(&str, &str)] = &[
    ("BackSpace", "Backspace"),
    ("space", "Space"),
    ("backslash", "Backslash"),
    ("ISO_Left_Tab", "Left Tab"),
    ("Multi_key", "Multikey"),
    ("KP_Enter", "Enter (keypad)"),
];

/// 小键盘键的显示名 —— GTK 这边走的是"键值 → Unicode 字符"，不是键名。
///
/// 实测（探针 `Gtk.accelerator_get_label`）：`KP_Add` → `+`、`KP_Subtract` → `-`、
/// `KP_4` → `4`、`KP_Decimal` → `.`。只列**验证过**的项；小键盘其余数字键同理
/// （`KP_0`..`KP_9` → `0`..`9`），但源码只用到 `KP_4/KP_5/KP_6` 与
/// `KP_Add/KP_Subtract`，没验过的不编。
const KEYPAD_CHAR_LABELS: &[(&str, &str)] = &[
    ("KP_Add", "+"),
    ("KP_Subtract", "-"),
    ("KP_Decimal", "."),
    ("KP_4", "4"),
    ("KP_5", "5"),
    ("KP_6", "6"),
];

/// GDK 里对应**可打印 ASCII 字符**的键名（`gdk_keyval_name()` 的名字）→ 字符。
///
/// GTK 对落在 0x21..=0x7e 的键值显示字符本身，而 GDK 的键名是单词形式，
/// 所以这张表就是把两者对起来。`space`（0x20）与 `backslash`（0x5c）不在此表，
/// 它们在 [`SPECIAL_KEY_LABELS`] 里。
pub const ASCII_PUNCT_NAMES: &[(&str, char)] = &[
    ("exclam", '!'),
    ("quotedbl", '"'),
    ("numbersign", '#'),
    ("dollar", '$'),
    ("percent", '%'),
    ("ampersand", '&'),
    ("apostrophe", '\''),
    ("parenleft", '('),
    ("parenright", ')'),
    ("asterisk", '*'),
    ("plus", '+'),
    ("comma", ','),
    ("minus", '-'),
    ("period", '.'),
    ("slash", '/'),
    ("colon", ':'),
    ("semicolon", ';'),
    ("less", '<'),
    ("equal", '='),
    ("greater", '>'),
    ("question", '?'),
    ("at", '@'),
    ("bracketleft", '['),
    ("bracketright", ']'),
    ("asciicircum", '^'),
    ("underscore", '_'),
    ("grave", '`'),
    ("braceleft", '{'),
    ("bar", '|'),
    ("braceright", '}'),
    ("asciitilde", '~'),
];

// ---- `accelmap.conf` 的条目与读写 ----

/// 一条 accelmap 条目：动作路径 + 加速键。
///
/// `accel == None` 对应 GTK 里"存在这个条目但键为 0"（`change_entry(path, 0, 0)`
/// / 文件里写了空串）：**当前没有键**。注意它和"条目不存在"不同 ——
/// 不存在时默认绑定还在，被清空的条目才是真的没键（见模块文档第 3 条：
/// 落盘时写成注释行，于是跨进程不保留）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub path: String,
    pub accel: Option<Accel>,
}

impl Entry {
    pub fn new(path: impl Into<String>, accel: Option<Accel>) -> Self {
        Entry {
            path: path.into(),
            accel,
        }
    }
}

/// 一行没能解析的内容（原文 + 原因），用于日志与单测。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SkippedLine {
    pub line: String,
    pub reason: String,
}

/// [`parse_conf`] 的结果。
#[derive(Clone, Default, Debug)]
pub struct ParsedConf {
    /// 条目，**按文件出现顺序**；同一个 path 出现多次时后者覆盖前者
    /// （GTK 的加载也是逐次 `change_entry`）。
    pub entries: Vec<Entry>,
    /// 没读懂的行（空行与注释不算）。
    pub skipped: Vec<SkippedLine>,
}

impl ParsedConf {
    /// 按 path 找条目（后写覆盖前写，所以从后往前找）。
    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().rev().find(|e| e.path == path)
    }
}

/// 解析 `accelmap.conf` 的全文 —— `gtk_accel_map_load()` 的文本侧。
///
/// 容错（都照实测）：
/// - 空行、`;` 开头的注释行（含 GTK 自己写出的 `; (gtk_accel_path … "")`）跳过；
/// - 词法空白随意（`( gtk_accel_path "a" "b" )` 能读，`;` 后跟尾注也行）；
/// - 坏行记进 [`ParsedConf::skipped`]，从**下一行**继续。
pub fn parse_conf(text: &str) -> ParsedConf {
    let mut out = ParsedConf::default();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        match parse_conf_line(line) {
            Ok(Some(entry)) => {
                // 同一 path 后写覆盖先写
                if let Some(slot) = out.entries.iter_mut().find(|e| e.path == entry.path) {
                    *slot = entry;
                } else {
                    out.entries.push(entry);
                }
            }
            Ok(None) => {}
            Err(reason) => out.skipped.push(SkippedLine {
                line: line.to_string(),
                reason,
            }),
        }
    }
    out
}

/// 解析一行：`Ok(None)` = 空行/注释；`Ok(Some)` = 一条条目。
fn parse_conf_line(line: &str) -> Result<Option<Entry>, String> {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    skip_ws(bytes, &mut i);
    if i >= bytes.len() || bytes[i] == b';' {
        return Ok(None);
    }
    expect(bytes, &mut i, b'(')?;
    skip_ws(bytes, &mut i);
    let ident = read_ident(bytes, &mut i)?;
    if ident != "gtk_accel_path" {
        return Err(format!("不认识的条目类型 {ident:?}（只认 gtk_accel_path）"));
    }
    skip_ws(bytes, &mut i);
    let path = read_quoted(bytes, &mut i)?;
    skip_ws(bytes, &mut i);
    let spec = read_quoted(bytes, &mut i)?;
    skip_ws(bytes, &mut i);
    expect(bytes, &mut i, b')')?;
    skip_ws(bytes, &mut i);
    // 行尾只允许注释
    if i < bytes.len() && bytes[i] != b';' {
        return Err("条目之后还有多余内容".to_string());
    }
    if path.is_empty() {
        return Err("动作路径为空".to_string());
    }
    if spec.is_empty() {
        return Ok(Some(Entry::new(path, None)));
    }
    match Accel::parse(&spec) {
        Ok(accel) => Ok(Some(Entry::new(path, Some(accel)))),
        Err(e) => Err(format!("加速键 {spec:?} 解析失败：{e}")),
    }
}

fn skip_ws(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && (bytes[*i] == b' ' || bytes[*i] == b'\t') {
        *i += 1;
    }
}

fn expect(bytes: &[u8], i: &mut usize, want: u8) -> Result<(), String> {
    if *i < bytes.len() && bytes[*i] == want {
        *i += 1;
        Ok(())
    } else {
        Err(format!("期望 {:?}", want as char))
    }
}

fn read_ident(bytes: &[u8], i: &mut usize) -> Result<String, String> {
    let start = *i;
    while *i < bytes.len() && (bytes[*i].is_ascii_alphanumeric() || bytes[*i] == b'_') {
        *i += 1;
    }
    if *i == start {
        return Err("期望条目类型名".to_string());
    }
    String::from_utf8(bytes[start..*i].to_vec()).map_err(|e| e.to_string())
}

/// 读双引号字符串。转义按 GTK 的 rc 扫描器：`\\` → `\`，`\"` → `"`；
/// 其余 `\X` 原样取 `X`（我们写出的路径与键名里不会出现别的转义）。
fn read_quoted(bytes: &[u8], i: &mut usize) -> Result<String, String> {
    expect(bytes, i, b'"')?;
    let mut out: Vec<u8> = Vec::new();
    while *i < bytes.len() {
        match bytes[*i] {
            b'\\' => {
                if *i + 1 >= bytes.len() {
                    return Err("字符串里有一个孤立的反斜杠".to_string());
                }
                out.push(bytes[*i + 1]);
                *i += 2;
            }
            b'"' => {
                *i += 1;
                return String::from_utf8(out).map_err(|e| e.to_string());
            }
            b => {
                out.push(b);
                *i += 1;
            }
        }
    }
    Err("字符串没有闭合的引号".to_string())
}

/// 生成 `accelmap.conf` 的全文 —— `gtk_accel_map_save()` 的文本侧。
///
/// 头三行逐字照搬真 GTK 的输出（`rc-file` 与 `-*- scheme -*-` 之间 9 个空格）；
/// 条目按 path 字典序（GTK 是哈希序，见模块文档）；`accel == None` 的条目写成
/// GTK 那种注释行。
pub fn format_conf(program: &str, entries: &[Entry]) -> String {
    let mut out = String::with_capacity(64 + entries.len() * 64);
    out.push_str(&format!(
        "; {program} GtkAccelMap rc-file         -*- scheme -*-\n"
    ));
    out.push_str("; this file is an automated accelerator map dump\n");
    out.push_str(";\n");
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    for e in sorted {
        match &e.accel {
            Some(accel) => {
                out.push_str(&format!(
                    "(gtk_accel_path \"{}\" \"{}\")\n",
                    escape(&e.path),
                    escape(&accel.to_gtk_name())
                ));
            }
            None => {
                out.push_str(&format!(
                    "; (gtk_accel_path \"{}\" \"\")\n",
                    escape(&e.path)
                ));
            }
        }
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// 读 `accelmap.conf` —— `gui/application.py:417` 的
/// `Gtk.AccelMap.load(join(self.user_confpath, "accelmap.conf"))`。
///
/// 文件不在 → 空结果（首次启动的正常情况，源码那边同样是"没有用户覆盖"）；
/// 其余 I/O 错误往上抛，由调用方决定记哪条日志。
pub fn load_conf(path: &Path) -> std::io::Result<ParsedConf> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(parse_conf(&String::from_utf8_lossy(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ParsedConf::default()),
        Err(e) => Err(e),
    }
}

/// 写 `accelmap.conf` —— `gui/application.py:625` 的
/// `Gtk.AccelMap.save(join(self.user_confpath, "accelmap.conf"))`。
///
/// 源码直接覆写、不带临时文件（`gtk_accel_map_save()` 就是 `open` + `write`），
/// 这里照做；目录不存在时先建（源码在启动时建 `user_confpath`，
/// `gui/application.py:226-231`）。
pub fn save_conf(path: &Path, program: &str, entries: &[Entry]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    std::fs::write(path, format_conf(program, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> Mods {
        Mods::new(true, false, false)
    }

    // ---- 加速键字符串 ⇄ 规范名 ----

    #[test]
    fn name_uses_gtk_modifier_order_and_spelling() {
        // 实测：`<Shift><Control>s` → `<Primary><Shift>s`（Ctrl 在前、Shift 在后）
        assert_eq!(
            Accel::parse("<Shift><Control>s").unwrap().to_gtk_name(),
            "<Primary><Shift>s"
        );
        // 没有 Ctrl 时 Shift 仍在 Alt 之前
        assert_eq!(
            Accel::parse("<Alt><Shift>s").unwrap().to_gtk_name(),
            "<Shift><Alt>s"
        );
        assert_eq!(Accel::parse("<Mod1>r").unwrap().to_gtk_name(), "<Alt>r");
        assert_eq!(
            Accel::parse("<Control>Page_Up").unwrap().to_gtk_name(),
            "<Primary>Page_Up"
        );
        // 裸键：键名原样
        assert_eq!(Accel::parse("comma").unwrap().to_gtk_name(), "comma");
        assert_eq!(Accel::parse("F12").unwrap().to_gtk_name(), "F12");
    }

    #[test]
    fn modifier_aliases_are_case_insensitive() {
        // `gui/document.py:492` 用的就是小写 `<control>`
        for spec in ["<control>0", "<Control>0", "<Ctrl>0", "<PRIMARY>0"] {
            let a = Accel::parse(spec).unwrap();
            assert_eq!(a.to_gtk_name(), "<Primary>0", "{spec} 应规范成 <Primary>0");
            assert_eq!(a.mods, ctrl());
        }
        // `Z` 与 `z` 是同一个键（GTK 对单字符一律小写化，且**不**补 Shift）
        let upper = Accel::parse("Z").unwrap();
        let lower = Accel::parse("z").unwrap();
        assert_eq!(upper, lower);
        assert_eq!(upper.key, "z");
        assert!(upper.mods.is_empty());
        // `<Shift>Z` 才是带 Shift 的
        assert_eq!(Accel::parse("<Shift>Z").unwrap().to_gtk_name(), "<Shift>z");
    }

    #[test]
    fn invalid_specs_are_rejected() {
        // 空串、只有修饰键：GTK 也返回键值 0
        assert_eq!(Accel::parse(""), Err(AccelError::Empty));
        assert_eq!(Accel::parse("<Shift>"), Err(AccelError::MissingKey));
        // 单个标点/空格：GTK 只认键值名（`,` 非法，`comma` 才对）
        assert!(matches!(Accel::parse("!"), Err(AccelError::BadKey(_))));
        assert!(matches!(Accel::parse(" "), Err(AccelError::BadKey(_))));
        assert!(Accel::parse("comma").is_ok());
        // 不认识的修饰键
        assert_eq!(
            Accel::parse("<Foo>x"),
            Err(AccelError::UnknownModifier("Foo".into()))
        );
    }

    #[test]
    fn super_hyper_meta_are_rejected_on_purpose() {
        // GTK 三种都收（实测），但 egui 的 Modifiers 没有对应位：
        // 宁可丢掉这条绑定，也不把它折叠成裸键（会造出假冲突）。
        for (spec, name) in [
            ("<Super>x", "Super"),
            ("<Hyper>x", "Hyper"),
            ("<Meta>x", "Meta"),
        ] {
            assert_eq!(
                Accel::parse(spec),
                Err(AccelError::UnsupportedModifier(name))
            );
        }
    }

    #[test]
    fn labels_follow_gtk_accelerator_get_label() {
        // 显示名的修饰键顺序是 Shift → Ctrl → Alt（与规范名相反，都是实测）
        assert_eq!(
            Accel::parse("<Shift><Control>s").unwrap().label(),
            "Shift+Ctrl+S"
        );
        assert_eq!(Accel::parse("<Control>z").unwrap().label(), "Ctrl+Z");
        assert_eq!(Accel::parse("<Alt>r").unwrap().label(), "Alt+R");
        // 裸字母大写、下划线换空格、标点显示字符本身
        assert_eq!(Accel::parse("f").unwrap().label(), "F");
        assert_eq!(Accel::parse("Page_Up").unwrap().label(), "Page Up");
        assert_eq!(Accel::parse("backslash").unwrap().label(), "Backslash");
        assert_eq!(Accel::parse("plus").unwrap().label(), "+");
        assert_eq!(
            Accel::parse("<Shift>plus").unwrap().label(),
            "Shift++",
            "GTK 的显示就是 Shift++（fixture 里有这一行）"
        );
        assert_eq!(Accel::parse("Menu").unwrap().label(), "Menu");
    }

    // ---- accelmap.conf ----

    #[test]
    fn conf_header_is_byte_identical_to_gtk() {
        let text = format_conf(PROGRAM_NAME, &[]);
        assert_eq!(
            text,
            "; mypaint GtkAccelMap rc-file         -*- scheme -*-\n\
             ; this file is an automated accelerator map dump\n\
             ;\n"
        );
    }

    #[test]
    fn conf_entries_are_sorted_and_cleared_ones_are_comments() {
        let entries = vec![
            Entry::new("<Actions>/B", Some(Accel::parse("<Control>s").unwrap())),
            Entry::new("<Actions>/A", None),
        ];
        let text = format_conf("mypaint", &entries);
        let lines: Vec<&str> = text.lines().skip(3).collect();
        assert_eq!(
            lines,
            vec![
                "; (gtk_accel_path \"<Actions>/A\" \"\")",
                "(gtk_accel_path \"<Actions>/B\" \"<Primary>s\")",
            ]
        );
    }

    #[test]
    fn conf_roundtrip_is_idempotent() {
        let entries = vec![
            Entry::new("<Actions>/Z", Some(Accel::parse("<Shift><Alt>d").unwrap())),
            Entry::new("<Actions>/A/B", Some(Accel::parse("comma").unwrap())),
            Entry::new("<Actions>/M", Some(Accel::parse("<Control>0").unwrap())),
        ];
        let once = format_conf(PROGRAM_NAME, &entries);
        let parsed = parse_conf(&once);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        // 注释掉的空条目不会被读回来（GTK 也一样）
        let twice = format_conf(PROGRAM_NAME, &parsed.entries);
        assert_eq!(once, twice);
        assert_eq!(parsed.entries.len(), 3);
    }

    #[test]
    fn conf_parser_accepts_loose_whitespace_and_trailing_comments() {
        // 实测 GTK 3.24.20 能吃下这四种写法（见模块文档）
        let text = "(gtk_accel_path \"<Actions>/A\" \"<Primary>a\")\n\
                    \x20 (gtk_accel_path   \"<Actions>/B\"   \"<Primary>b\")\n\
                    (gtk_accel_path \"<Actions>/C\" \"<Primary>c\")   ; 尾注\n\
                    ( gtk_accel_path \"<Actions>/D\" \"<Primary>d\" )\n";
        let parsed = parse_conf(text);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        assert_eq!(parsed.entries.len(), 4);
        assert_eq!(
            parsed
                .get("<Actions>/B")
                .unwrap()
                .accel
                .as_ref()
                .unwrap()
                .key,
            "b"
        );
    }

    #[test]
    fn conf_parser_skips_comments_and_broken_lines() {
        let text = "; 注释\n\
                    \n\
                    (gtk_accel_path \"<Actions>/One\" \"<Primary>1\")\n\
                    not a valid line\n\
                    ; (gtk_accel_path \"<Actions>/Gone\" \"\")\n\
                    (gtk_accel_path \"<Actions>/Two\" \"<Shift><Alt>t\")\n\
                    (gtk_accel_path \"<Actions>/Bad\" \"<Super>x\")\n";
        let parsed = parse_conf(text);
        // 两条坏行：不是条目的那一行、以及 <Super>（我们拒绝）
        assert_eq!(parsed.skipped.len(), 2, "{:?}", parsed.skipped);
        assert_eq!(parsed.skipped[0].line, "not a valid line");
        assert!(parsed.skipped[1].line.contains("<Super>x"));
        // 注释行里的空条目**不会**被读回来（"删绑定"不跨进程保留）
        assert_eq!(parsed.entries.len(), 2);
        assert!(parsed.get("<Actions>/Gone").is_none());
        assert_eq!(
            parsed
                .get("<Actions>/Two")
                .unwrap()
                .accel
                .as_ref()
                .unwrap()
                .to_gtk_name(),
            "<Shift><Alt>t"
        );
    }

    #[test]
    fn conf_parser_last_entry_for_a_path_wins() {
        let text = "(gtk_accel_path \"<Actions>/A\" \"a\")\n\
                    (gtk_accel_path \"<Actions>/A\" \"<Primary>a\")\n";
        let parsed = parse_conf(text);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(
            parsed
                .get("<Actions>/A")
                .unwrap()
                .accel
                .as_ref()
                .unwrap()
                .to_gtk_name(),
            "<Primary>a"
        );
    }

    #[test]
    fn conf_quotes_are_escaped_both_ways() {
        // 实测：GTK 把路径里的 `"` 与 `\` 写成 \" 与 \\，加载能还原
        let path = "<Actions>/Probe\"Q\\B/Action";
        let entries = vec![Entry::new(path, Some(Accel::parse("a").unwrap()))];
        let text = format_conf(PROGRAM_NAME, &entries);
        assert!(
            text.contains(r#"(gtk_accel_path "<Actions>/Probe\"Q\\B/Action" "a")"#),
            "{text}"
        );
        let parsed = parse_conf(&text);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].path, path);
    }

    #[test]
    fn empty_accel_string_means_cleared_entry() {
        let parsed = parse_conf("(gtk_accel_path \"<Actions>/A\" \"\")\n");
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].accel, None);
    }

    #[test]
    fn conf_file_roundtrips_through_disk() {
        let dir = std::env::temp_dir().join(format!("pigment-accelmap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FILENAME);
        // 文件不存在 → 空结果（首次启动）
        assert!(load_conf(&path).unwrap().entries.is_empty());
        let entries = vec![Entry::new(
            "<Actions>/FileActions/Save",
            Some(Accel::parse("<Control>s").unwrap()),
        )];
        save_conf(&path, PROGRAM_NAME, &entries).unwrap();
        let back = load_conf(&path).unwrap();
        assert_eq!(back.entries, entries);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accelmap_path_sits_next_to_settings_json() {
        // gui/application.py:417 与 :503 用的是同一个 user_confpath
        let conf = accelmap_path();
        assert_eq!(conf.file_name().unwrap(), FILENAME);
        assert_eq!(conf.parent().unwrap(), prefs::config_dir());
    }

    #[test]
    fn weird_key_names_are_kept_verbatim_not_dropped() {
        // `Backspace` / `PageUp` 是拼写错误（GTK 拒绝），但我们只做形状校验，
        // 于是它们被原样收下 → 应用层的键位表认不出，是条惰性绑定。
        // 这条单测把"差异就这么大"钉住。
        for bad in ["Backspace", "PageUp", "Space"] {
            let a = Accel::parse(bad).expect("形状合法就收下");
            assert_eq!(a.key, bad);
        }
        // 形状不合法的一律拒绝
        for nope in ["a b", "<Actions>", "a-b", "1abc"] {
            assert!(Accel::parse(nope).is_err(), "{nope} 应被拒绝");
        }
    }
}
