//! MyPaint 的**动作表与键盘模型**。
//!
//! 这一层把 MyPaint 的声明式定义翻成运行时可查的数据结构，是 S3 菜单栏、
//! S4 工具条、B6 按键编辑器的公共底座。**一个字符串都不手抄**：
//!
//! | 来源 | 内容 | 生成/照搬方式 |
//! |---|---|---|
//! | `gui/resources.xml` | 219 个动作、105 条加速键 | `build.rs` 构建期解析 → [`ACTIONS`] |
//! | `gui/document.py:_init_context_actions` | 笔刷槽位键（`0`-`9` / `Ctrl+0`-`9`） | [`DYNAMIC_ACCELS`]，见下 |
//! | `gui/document.py:_init_extra_keys` | 30 条 GIMP/Krita 兼容别名 | [`EXTRA_ACCELS`] |
//! | `gui/keyboard.py` | 优先级与匹配规则 | [`lookup`] |
//!
//! # 键盘模型（`gui/keyboard.py`）
//!
//! MyPaint **不用** GTK 的加速键分发，而是自己维护两张表（`keyboard.py:53-54`）：
//!
//! ```python
//! self.keymap  = {}   # 菜单/用户定义的加速键（Gtk.AccelMap）
//! self.keymap2 = {}   # 代码里硬编码的额外键（add_extra_key）
//! ```
//!
//! 查表顺序（`keyboard.py:139-142`）：先 `keymap`，未命中才查 `keymap2`；
//! 因此 **`resources.xml` 里的加速键优先于代码注册的额外键**。用户在按键编辑器
//! 里改过的键写进 GTK 的 accelmap，也走 `keymap`（优先级 1）。
//!
//! 匹配规则（`keyboard.py:106-139`）：
//! 1. 忽略 CapsLock 之类无关修饰键（`event.state & ~LOCK_MASK`）；
//! 2. 键值**一律小写**存储，按下 Shift 使得键值变成大写时补上 `SHIFT` 修饰位
//!    （`keyval_lower != keyval → modifiers |= SHIFT_MASK`）；
//! 3. 修饰键必须**精确相等**（元组查表），不是"包含"。
//!
//! 另外两条与分发有关的源码事实：
//! - 输入框获得焦点时，键事件先给输入框（`keyboard.py:97`）；
//!   本应用对应 `egui::Context::text_edit_focused()` 的早退。
//! - 松开 Escape 会释放所有按住的键、清空模式栈并解除指针抓取
//!   （`keyboard.py:214-221`）—— 模式栈是 B3 的活，S2 只把键位记在案。
//!
//! # 与 egui 的差异（技术栈适配）
//!
//! - GDK 的 `Menu` 键（0xff67）在 egui 里没有对应键位 → [`Accel::egui_key`]
//!   返回 `None`，该动作暂时没有键（`ShowPopupMenu`）。
//! - egui 的 `Modifiers::command` 在 Windows 上等同 Ctrl，统一并入 `ctrl` 位。
//!
//! # 关于 `dead_code`
//!
//! 本模块是**数据层**：S2 只把表建起来并让已实现的动作走查表分发，
//! 表里的 label / tooltip / 图标名 / 短标签 / 动作路径是给 S3 菜单栏、
//! S4 工具条、B6 按键编辑器消费的。所以整模块允许未使用项，
//! 而不是给十几处逐个加 `#[allow]`（那样反而看不清"这是给谁用的"）。

#![allow(dead_code)]

use std::sync::OnceLock;

use eframe::egui;

/// 一个动作的类别 —— 对应 `resources.xml` 里的 `class` 属性。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `GtkAction`
    Action,
    /// `GtkToggleAction`
    Toggle,
    /// `GtkRadioAction`（同组互斥，见 [`Action::radio_group`] 与 [`Action::value`]）
    Radio,
    /// `GtkRecentAction`（最近文件列表；按键编辑器会跳过它）
    Recent,
    /// `MyPaintFactoryAction`（运行期生成子项的占位动作；按键编辑器会跳过它）
    Factory,
}

/// 修饰键集合。GDK 侧只有三种出现在 `resources.xml` 里。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
    };

    /// 从 egui 的修饰键状态构造。
    ///
    /// `command` 在 macOS 上是 Cmd、在 Windows/Linux 上是 Ctrl；MyPaint 的
    /// accelmap 用 GDK 的 `<Primary>` 表示"平台主修饰键"（GTK 在 Windows 上
    /// 把它映射到 Ctrl），所以这里并入 `ctrl` 位。
    pub fn from_egui(m: &egui::Modifiers) -> Mods {
        Mods {
            ctrl: m.ctrl || m.command,
            alt: m.alt,
            shift: m.shift,
        }
    }

    /// `Gtk.accelerator_get_default_mod_mask()` 覆盖的位在本项目只用这三种；
    /// 其余（Super/Meta/Hyper）忽略。
    pub fn is_empty(&self) -> bool {
        !self.ctrl && !self.alt && !self.shift
    }

    /// 投影到引擎层的修饰键（`accelmap.conf` 与 GTK 加速键字符串在引擎层）。
    pub fn to_accel(self) -> pigment_engine::accelmap::Mods {
        pigment_engine::accelmap::Mods {
            ctrl: self.ctrl,
            shift: self.shift,
            alt: self.alt,
        }
    }
}

/// 引擎层修饰键 → 应用层修饰键（字段名与顺序一致）。
impl From<pigment_engine::accelmap::Mods> for Mods {
    fn from(m: pigment_engine::accelmap::Mods) -> Self {
        Mods {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
        }
    }
}

/// 一条加速键：`<accelerator key="z" modifiers="GDK_CONTROL_MASK"/>`。
#[derive(Clone, Copy, Debug)]
pub struct Accel {
    /// **GDK 键名**（`n` / `F5` / `Page_Up` / `grave` / `bracketleft` …），
    /// 与 `gdk_keyval_name()` 的结果一致。
    pub key: &'static str,
    pub mods: Mods,
}

impl Accel {
    /// 加速键在界面上显示的文字 —— 复刻 `Gtk.accelerator_get_label()`。
    ///
    /// 规则（连同 105 条加速键的 GTK 真值门禁、以及 GTK 的 `key_labels[]`
    /// 特例）在引擎的 [`pigment_engine::accelmap::label`]；这里只做
    /// "本表 → 引擎"的投影，别再抄一遍规则。
    ///
    /// - 修饰键顺序 **Shift → Ctrl → Alt**（`<Shift><Control>s` → `Shift+Ctrl+S`）；
    /// - 单字母一律大写（`f` → `F`），`Page_Up` → `Page Up`；
    /// - GTK 的特例表：`backslash` → `Backslash`、`space` → `Space`、
    ///   `BackSpace` → `Backspace`、`KP_Add` → `+`。
    pub fn label(&self) -> String {
        pigment_engine::accelmap::label(self.key, self.mods.to_accel())
    }

    /// GDK 键名 → egui 键位（**技术栈适配**，手写映射表 —— 见 [`KEY_NAMES`]）。
    ///
    /// `resources.xml` 实际用到的 GDK 键名是一组固定的小集合（字母、`F1`-`F12`、
    /// `Delete/Home/End/Page_Up/Page_Down/Tab/Up/Down/Left/Right/Menu`、
    /// `comma/period/grave/backslash/bracketleft/bracketright`），单测覆盖全集，
    /// 新增时不会静默漏掉。
    ///
    /// 三处**必须知道**的 egui 行为（都读过 `egui-0.37`/`egui-winit` 源码确认）：
    /// - egui 的 `Key` 取自 winit 的**逻辑键**（`egui-winit` 的 `Key::from_name`），
    ///   认不出的字符回落到**物理键**，所以 `Shift+=` 会报成 `Key::Plus`、
    ///   `Shift+,` 报成 `Key::Comma`（`<` 不在 egui 的表里）。
    /// - **egui 不区分小键盘数字键**（`KeyCode::Numpad4 → Key::Num4`），
    ///   而 GDK 的 `KP_4/KP_5/KP_6` 在源码里绑的是旋转/重置旋转。
    ///   本应用选**槽位数字键**（`0`-`9`，源码里是 `Context0N`）—— 因此
    ///   `KP_4/KP_5/KP_6` 无法映射（见 [`UNMAPPABLE_GDK_KEYS`]）。
    /// - `Menu` 映射到 `egui::Key::Menu`（egui-winit 37 会从 ContextMenu 产生）。
    pub fn egui_key(&self) -> Option<egui::Key> {
        egui_key_for(self.key)
    }

    /// egui 侧的**实测**绑定：`(键位, egui 会报的修饰键)`。
    ///
    /// 与 [`Accel::mods`] 的差别只在一处，但那处是硬差异：**GDK 不把"被消费掉的
    /// Shift"计入修饰键**（`keyboard.py:127-133` 明确要减掉 `consumed_modifiers`），
    /// egui 却照实报 `shift=true`。所以 `+`（Shift+=）、`<`（Shift+,）、
    /// `>`（Shift+.）、`|`（Shift+\）这些键，在 egui 侧要多带一位 Shift
    /// 才能与 `comma` / `period` 这类裸标点区分开 —— 展开成数据就是
    /// [`KEY_NAMES`] 里的 `needs_shift` 列。
    pub fn egui_binding(&self) -> Option<(egui::Key, Mods)> {
        egui_binding_for(self.key, self.mods)
    }
}

/// 明确**无法映射**到 egui 键位的 GDK 键名（单测断言时按字典序比较）。
///
/// - `KP_4` / `KP_5` / `KP_6`：小键盘数字键。egui 把它们与主键盘数字键合并
///   成同一个 `Key`（`egui-winit`：`KeyCode::Numpad4 → Key::Num4`），
///   而主键盘数字键被槽位动作（`Context0N`）占着，故取槽位。
/// - `Menu`：上下文菜单键，egui 没有这个键位。
pub const UNMAPPABLE_GDK_KEYS: &[&str] = &["KP_4", "KP_5", "KP_6"];

/// 一条键位的**双向**映射：GDK 键值名 ⇄ egui 键位（**技术栈适配**表）。
///
/// 三列：
/// - `name`：`gdk_keyval_name()` 的拼写（`accelmap.conf` 里写的就是它）
/// - `key`：egui 的键位
/// - `needs_shift`：这个**字符**在美国布局上是不是要按 Shift 才打得出来
///   （`!` `?` `{` `<` … 是，`,` `.` `;` 不是）
///
/// `needs_shift` 是"GDK 的 consumed_modifiers"这条规则展开成数据的结果：
/// 按 Shift+, 时 GDK 给出的键值是 `less`、修饰键**没有** Shift（Shift 被"消费"
/// 成字符了，`keyboard.py:127-133`），而 egui 报的是 `Key::Comma` + `shift=true`
/// （`<` 不在 egui 的 `Key::from_name` 表里，回落物理键）。所以同一个物理组合
/// 在两边要差一位 Shift —— 查表时补上/去掉，见
/// [`egui_binding_for`] 与 [`capture_accel`]。
///
/// 同一个 egui 键位可以出现两次（`minus` / `underscore` 都是 `Key::Minus`），
/// 靠 `needs_shift` 区分，正好对应 GDK 的两个键值。
struct KeyName {
    name: &'static str,
    key: egui::Key,
    needs_shift: bool,
}

/// 键名 ⇄ egui 键位的全部映射（**唯一的**两张表合一）。
///
/// 覆盖范围是"源码用到的 GDK 键名"（`resources.xml` 105 条 + `document.py` 的
/// 20 条槽位键）加上"捕获流程能产出的一切"（[`capture_accel`] 从 egui 键位
/// 反推 GDK 名）。单测逐项断言：本表里每个 `name` 都被真 GTK 接受
/// （fixture `crates/pigment-engine/tests/fixtures/accel_names.tsv`），
/// 且捕获 → 查表能原路返回。
const KEY_NAMES: &[KeyName] = &[
    // ---- 字母（26）----
    kn("a", egui::Key::A, false),
    kn("b", egui::Key::B, false),
    kn("c", egui::Key::C, false),
    kn("d", egui::Key::D, false),
    kn("e", egui::Key::E, false),
    kn("f", egui::Key::F, false),
    kn("g", egui::Key::G, false),
    kn("h", egui::Key::H, false),
    kn("i", egui::Key::I, false),
    kn("j", egui::Key::J, false),
    kn("k", egui::Key::K, false),
    kn("l", egui::Key::L, false),
    kn("m", egui::Key::M, false),
    kn("n", egui::Key::N, false),
    kn("o", egui::Key::O, false),
    kn("p", egui::Key::P, false),
    kn("q", egui::Key::Q, false),
    kn("r", egui::Key::R, false),
    kn("s", egui::Key::S, false),
    kn("t", egui::Key::T, false),
    kn("u", egui::Key::U, false),
    kn("v", egui::Key::V, false),
    kn("w", egui::Key::W, false),
    kn("x", egui::Key::X, false),
    kn("y", egui::Key::Y, false),
    kn("z", egui::Key::Z, false),
    // ---- 数字（10）----
    kn("0", egui::Key::Num0, false),
    kn("1", egui::Key::Num1, false),
    kn("2", egui::Key::Num2, false),
    kn("3", egui::Key::Num3, false),
    kn("4", egui::Key::Num4, false),
    kn("5", egui::Key::Num5, false),
    kn("6", egui::Key::Num6, false),
    kn("7", egui::Key::Num7, false),
    kn("8", egui::Key::Num8, false),
    kn("9", egui::Key::Num9, false),
    // ---- 功能键（egui 有 F1-F35）----
    kn("F1", egui::Key::F1, false),
    kn("F2", egui::Key::F2, false),
    kn("F3", egui::Key::F3, false),
    kn("F4", egui::Key::F4, false),
    kn("F5", egui::Key::F5, false),
    kn("F6", egui::Key::F6, false),
    kn("F7", egui::Key::F7, false),
    kn("F8", egui::Key::F8, false),
    kn("F9", egui::Key::F9, false),
    kn("F10", egui::Key::F10, false),
    kn("F11", egui::Key::F11, false),
    kn("F12", egui::Key::F12, false),
    kn("F13", egui::Key::F13, false),
    kn("F14", egui::Key::F14, false),
    kn("F15", egui::Key::F15, false),
    kn("F16", egui::Key::F16, false),
    kn("F17", egui::Key::F17, false),
    kn("F18", egui::Key::F18, false),
    kn("F19", egui::Key::F19, false),
    kn("F20", egui::Key::F20, false),
    kn("F21", egui::Key::F21, false),
    kn("F22", egui::Key::F22, false),
    kn("F23", egui::Key::F23, false),
    kn("F24", egui::Key::F24, false),
    kn("F25", egui::Key::F25, false),
    kn("F26", egui::Key::F26, false),
    kn("F27", egui::Key::F27, false),
    kn("F28", egui::Key::F28, false),
    kn("F29", egui::Key::F29, false),
    kn("F30", egui::Key::F30, false),
    kn("F31", egui::Key::F31, false),
    kn("F32", egui::Key::F32, false),
    kn("F33", egui::Key::F33, false),
    kn("F34", egui::Key::F34, false),
    kn("F35", egui::Key::F35, false),
    // ---- 编辑/导航键 ----
    kn("Escape", egui::Key::Escape, false),
    kn("Tab", egui::Key::Tab, false),
    kn("Menu", egui::Key::Menu, false),
    kn("BackSpace", egui::Key::Backspace, false),
    kn("Return", egui::Key::Enter, false),
    kn("space", egui::Key::Space, false),
    kn("Insert", egui::Key::Insert, false),
    kn("Delete", egui::Key::Delete, false),
    kn("Home", egui::Key::Home, false),
    kn("End", egui::Key::End, false),
    kn("Page_Up", egui::Key::PageUp, false),
    kn("Page_Down", egui::Key::PageDown, false),
    kn("Left", egui::Key::ArrowLeft, false),
    kn("Right", egui::Key::ArrowRight, false),
    kn("Up", egui::Key::ArrowUp, false),
    kn("Down", egui::Key::ArrowDown, false),
    // ---- 不需要 Shift 的标点（美国布局）----
    kn("comma", egui::Key::Comma, false),
    kn("period", egui::Key::Period, false),
    kn("slash", egui::Key::Slash, false),
    kn("backslash", egui::Key::Backslash, false),
    kn("minus", egui::Key::Minus, false),
    kn("equal", egui::Key::Equals, false),
    kn("semicolon", egui::Key::Semicolon, false),
    kn("apostrophe", egui::Key::Quote, false),
    kn("grave", egui::Key::Backtick, false),
    kn("bracketleft", egui::Key::OpenBracket, false),
    kn("bracketright", egui::Key::CloseBracket, false),
    // ---- 小键盘：egui 与主键盘合并，只在"读文件"方向能区分 ----
    // `KP_Add` / `KP_Subtract` 与 `plus` / `minus` 撞在同一个 egui 键位上，
    // 靠 Shift 位分开（`KP_Add` 不带 Shift，`plus` 带），见 [`capture_accel`]。
    kn("KP_Add", egui::Key::Plus, false),
    kn("KP_Subtract", egui::Key::Minus, false),
    // ---- 需要 Shift 的标点（美国布局）：GDK 的键值本身是"Shift 后的字符" ----
    kn("exclam", egui::Key::Num1, true),
    kn("at", egui::Key::Num2, true),
    kn("numbersign", egui::Key::Num3, true),
    kn("dollar", egui::Key::Num4, true),
    kn("percent", egui::Key::Num5, true),
    kn("asciicircum", egui::Key::Num6, true),
    kn("ampersand", egui::Key::Num7, true),
    kn("asterisk", egui::Key::Num8, true),
    kn("parenleft", egui::Key::Num9, true),
    kn("parenright", egui::Key::Num0, true),
    kn("underscore", egui::Key::Minus, true),
    kn("plus", egui::Key::Equals, true),
    kn("braceleft", egui::Key::OpenBracket, true),
    kn("braceright", egui::Key::CloseBracket, true),
    kn("bar", egui::Key::Backslash, true),
    kn("colon", egui::Key::Semicolon, true),
    kn("quotedbl", egui::Key::Quote, true),
    kn("less", egui::Key::Comma, true),
    kn("greater", egui::Key::Period, true),
    kn("question", egui::Key::Slash, true),
    kn("asciitilde", egui::Key::Backtick, true),
];

const fn kn(name: &'static str, key: egui::Key, needs_shift: bool) -> KeyName {
    KeyName {
        name,
        key,
        needs_shift,
    }
}

/// GDK 键名 → egui 键位（无对应物时 `None`，见 [`UNMAPPABLE_GDK_KEYS`]）。
pub fn egui_key_for(name: &str) -> Option<egui::Key> {
    KEY_NAMES.iter().find(|e| e.name == name).map(|e| e.key)
}

/// GDK 键名 + 修饰键 → egui 侧的 `(键位, 修饰键)`。
///
/// 这就是 [`Accel::egui_binding`] 的实现：`needs_shift` 的键要多带一位 Shift
/// （原因见 [`KeyName`] 的文档）。
pub fn egui_binding_for(name: &str, mods: Mods) -> Option<(egui::Key, Mods)> {
    let entry = KEY_NAMES.iter().find(|e| e.name == name)?;
    let mut mods = mods;
    if entry.needs_shift {
        mods.shift = true;
    }
    Some((entry.key, mods))
}

/// egui 键位（+ 是否按着 Shift）→ GDK 键名 —— [`capture_accel`] 的底层。
///
/// 返回值是 `(键名, 是否把 Shift 交给 GDK 消费)`：
/// - 该键位在本表里有一条 `needs_shift` 与 Shift 状态**相同**的记录 → 用它的名字，
///   且 Shift 已被消费（`capture_accel` 会清掉 SHIFT 位）；
/// - 否则退回该键位的另一条记录（字母、功能键只有 `needs_shift=false` 那条），
///   Shift 保留。
pub fn gdk_name_for_egui(key: egui::Key, shift: bool) -> Option<(&'static str, bool)> {
    if let Some(e) = KEY_NAMES
        .iter()
        .find(|e| e.key == key && e.needs_shift == shift)
    {
        return Some((e.name, e.needs_shift));
    }
    if shift {
        // 字母/功能键/方向键：Shift 不被消费，键名不变、修饰键保留
        return KEY_NAMES
            .iter()
            .find(|e| e.key == key && !e.needs_shift)
            .map(|e| (e.name, false));
    }
    None
}

/// 这个 egui 键位是不是**修饰键本身**（不参与捕获）。
///
/// 对应 `gui/accelmap.py:469` 的 `if event.is_modifier: return False`：
/// GTK 的 `Gdk.EventKey.is_modifier` 就是在问这个；egui 没有 `is_modifier`，
/// 只能按 `Key` 变体判断（`egui-winit` 会把修饰键映射成这些变体）。
pub fn is_modifier_key(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::ShiftLeft
            | egui::Key::ShiftRight
            | egui::Key::ControlLeft
            | egui::Key::ControlRight
            | egui::Key::AltLeft
            | egui::Key::AltRight
            | egui::Key::SuperLeft
            | egui::Key::SuperRight
    )
}

/// 捕获到的按键 → 加速键（**技术栈适配**：GTK 的 `gtk_accel_group` 捕获逻辑的替身）。
///
/// 对应 `gui/accelmap.py:482-503`（"Stolen from GTK 2.24's gtkmenu.c"）：
/// 1. 键值小写化、`mods = state & default_mod_mask & ~consumed_modifiers`
///    —— 在 egui 侧就是 [`gdk_name_for_egui`] 的 `needs_shift` 处理：
///    Shift+, 得到的键是 `less`、修饰键里**没有** Shift；
/// 2. 修饰键只留 Ctrl/Alt/Shift（egui 也没有别的位可留）。
///
/// 返回 `None` 表示这个键位没有 GDK 名字（egui 的浏览器键、媒体键等），
/// 调用方要给出提示而不是悄悄丢。
pub fn capture_accel(key: egui::Key, mods: Mods) -> Option<(&'static str, Mods)> {
    let (name, shift_consumed) = gdk_name_for_egui(key, mods.shift)?;
    let mut mods = mods;
    if shift_consumed {
        mods.shift = false;
    }
    Some((name, mods))
}

/// 一条动作定义。字段与 `resources.xml` 的属性一一对应。
#[derive(Clone, Copy, Debug)]
pub struct Action {
    /// 动作名（`Gtk.Action.get_name()`）
    pub id: &'static str,
    /// 所属动作组（`FileActions` / `DocumentActions` / …）
    pub group: &'static str,
    pub kind: Kind,
    /// 菜单项文字（`label`；`translatable="yes"` 的英文原文）
    ///
    /// `None` 有明确含义：子菜单容器以外，`Undo`/`Redo` 的文字由命令栈动态生成
    /// （`gui/document.py:190-193,582-591`），`*Centered`
    /// 从同名去掉后缀的动作复制（`gui/accelmap.py:178-191`）。
    pub label: Option<&'static str>,
    /// 工具条的短标签（`short-label`）
    pub short_label: Option<&'static str>,
    /// 鼠标提示（`tooltip`）。**按键编辑器只列有 tooltip 的动作**
    /// （`gui/accelmap.py:34`：`The accel map editor only shows an action if it has a tooltip.`）
    pub tooltip: Option<&'static str>,
    /// 图标名（`icon-name`；源码 6 处写成 `icon_name`，已归一化）
    pub icon_name: Option<&'static str>,
    /// 信号处理函数名（`<signal handler="...">`）
    pub handler: Option<&'static str>,
    /// 信号名（`activate` / `changed` / `toggled` / `item-activated`）
    pub signal: Option<&'static str>,
    /// 单选组（`group` 属性）—— `GtkRadioAction` 的互斥组
    pub radio_group: Option<&'static str>,
    /// 单选值（`value`）—— 同组内**不保证唯一**（源码里 `RotateViewMode`
    /// 与 `ZoomViewMode` 都是 3），所以只当数据带着，别当枚举用
    pub value: Option<i32>,
    /// 初始选中值（`current-value`），只出现在组内第一个动作上
    pub current_value: Option<i32>,
    /// `resources.xml` 里声明的加速键（105 条）
    pub accel: Option<Accel>,
}

impl Action {
    /// 动作路径 `<Actions>/{group}/{id}`（`gui/accelmap.py:169`）。
    ///
    /// 这是 MyPaint 在 GTK accelmap 与按键编辑器里标识一条绑定的键，
    /// 用户的键位覆写文件（`accelmap.conf`）也用这个路径（B6 用得上）。
    pub fn path(&self) -> String {
        format!("<Actions>/{}/{}", self.group, self.id)
    }

    /// 实际显示用的文字 —— 等价于 `Gtk.Action.get_label()`。
    ///
    /// 处理 `accelmap.py:178-191` 那条"给工具条专用动作补文字"的规则：
    /// `ZoomInCentered` 之类没有自己的 label，运行时从 `ZoomIn` 复制。
    pub fn effective_label(&self) -> Option<&'static str> {
        if let Some(l) = self.label {
            return Some(l);
        }
        let src = self.id.strip_suffix("Centered")?;
        by_id(src).and_then(|a| a.label)
    }

    /// 实际显示用的 tooltip —— 同上，`*Centered` 从源动作复制。
    pub fn effective_tooltip(&self) -> Option<&'static str> {
        if let Some(t) = self.tooltip {
            return Some(t);
        }
        let src = self.id.strip_suffix("Centered")?;
        by_id(src).and_then(|a| a.tooltip)
    }

    /// 工具条上显示的文字：优先 `short-label`，否则退到 `label`
    /// （`Gtk.Action` 的语义；`gui/toolbar.py` 用 `tool_item` 的 label 走同一套）。
    pub fn toolbar_label(&self) -> Option<&'static str> {
        self.short_label.or_else(|| self.effective_label())
    }

    /// 是否是"只为菜单结构存在"的动作 —— 无 tooltip 且以 `Menu` 结尾
    /// （`gui/accelmap.py:210-214` 把它们排除在按键编辑器之外）。
    ///
    /// S3 建菜单树时这些 id 是**子菜单的挂点**，不是可点击的动作。
    pub fn is_menu_structure(&self) -> bool {
        self.effective_tooltip().is_none() && self.id.ends_with("Menu")
    }

    /// 是否出现在按键编辑器里（`gui/accelmap.py:165-221` 的过滤规则）。
    ///
    /// 逐条对应源码：
    /// - 跳过 `GtkRecentAction` 与 `*FactoryAction`（`:170-175`）
    /// - **没有 label 的一律跳过**（`:177-191`）。`*Centered` 那一支虽然会给
    ///   动作补上 label/tooltip（供工具条用），但那一轮循环紧接着就 `continue`
    ///   了，所以它们**不会**出现在按键编辑器里 —— 这是实测 170 条的关键。
    /// - id 以 `Mode` 结尾的 `GtkRadioAction` 跳过（用配对的 `Flip*` 更合适，`:194-203`）
    /// - 没有 tooltip 的跳过（`:204-220`）
    pub fn is_listed_in_accel_editor(&self) -> bool {
        if matches!(self.kind, Kind::Recent | Kind::Factory) {
            return false;
        }
        if self.label.is_none() {
            return false;
        }
        if self.id.ends_with("Mode") && self.kind == Kind::Radio {
            return false;
        }
        self.tooltip.is_some()
    }
}

// 构建期生成物（`build.rs` → `$OUT_DIR/actions_generated.rs`）：
//   `pub static GROUPS: &[&str]`  —— `resources.xml` 的动作组 id，保持 XML 顺序
//   `pub static ACTIONS: &[Action]` —— 219 项，保持 XML 顺序
// 生成物的文档在 build.rs 里（避免"生成处的注释与使用处脱节"）。
include!(concat!(env!("OUT_DIR"), "/actions_generated.rs"));

/// 按动作名查表。名字在整张表里唯一（构建期保证，见单测）。
pub fn by_id(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// 某个动作组内的动作（保持 XML 顺序）。S3 建菜单树时按组取用。
pub fn in_group(group: &'static str) -> impl Iterator<Item = &'static Action> {
    ACTIONS.iter().filter(move |a| a.group == group)
}

// ---- 代码里注册的键位（不在 resources.xml 里）----

/// 代码注册的加速键 —— 对应 `gui/document.py:_init_context_actions`
/// （笔刷槽位的 20 条）与 `_init_extra_keys`（30 条兼容别名）。
///
/// 与 [`ACTIONS`] 的区别不只是来源：**优先级不同**。前者进 GTK 的 accelmap
/// （`keyboard.py` 的 `keymap`），后者进硬编码兜底表（`keymap2`），
/// 查表时前者优先（`keyboard.py:139-142`）。见 [`lookup`]。
#[derive(Clone, Copy, Debug)]
pub struct KeyDef {
    pub accel: Accel,
    /// 被触发的动作名；`None` 表示该键直接对应一个非动作行为
    /// （源码里是 lambda，如光标键平移）。
    pub action: Option<&'static str>,
    /// 非动作行为的标识（`pan-left` / `zoom-in` / `quit` …）。
    pub behavior: Option<&'static str>,
    /// 源码出处
    pub source: &'static str,
    /// 是"用户可在按键编辑器里改"的菜单级绑定（`keymap`，优先级 1），
    /// 还是硬编码兜底（`keymap2`，优先级 2）
    pub menu_level: bool,
}

const fn k(key: &'static str, mods: Mods, action: &'static str, source: &'static str) -> KeyDef {
    KeyDef {
        accel: Accel { key, mods },
        action: Some(action),
        behavior: None,
        source,
        menu_level: false,
    }
}

/// 笔刷槽位键：`0`-`9` 恢复、`Ctrl+0`-`0` 存入
/// （`gui/document.py:467-492`，动作名 `Context0N` / `Context0Ns`）。
///
/// 这 20 条是**运行期**加进 `DocumentActions` 组的（`ag.add_actions`，
/// `document.py:492`），因此也走 accelmap，优先级与 `resources.xml` 同级。
pub const NUM_CONTEXT_SLOTS: usize = 10;

/// 动态槽位动作名（恢复 / 存入）。
pub fn context_action_ids(slot: usize) -> (String, String) {
    (format!("Context0{slot}"), format!("Context0{slot}s"))
}

/// 把槽位动作名还原成 `(槽位号, 是否是"存入")`。
pub fn parse_context_action(id: &str) -> Option<(usize, bool)> {
    let rest = id.strip_prefix("Context0")?;
    let (digits, store) = match rest.strip_suffix('s') {
        Some(d) => (d, true),
        None => (rest, false),
    };
    if digits.len() != 1 {
        return None;
    }
    let n = digits.parse::<usize>().ok()?;
    (n < NUM_CONTEXT_SLOTS).then_some((n, store))
}

/// 硬编码的额外键 —— 逐条照搬 `gui/document.py:515-548`。
///
/// 顺序即源码顺序：源码注释说"同一个键绑两次，**后一个生效**"
/// （`document.py:512`）。查表时**逆序**遍历本表以复现这一语义（见 [`lookup`]）。
pub static EXTRA_ACCELS: &[KeyDef] = &[
    // GIMP / Photoshop / Painter 兼容
    k("bracketleft", Mods::NONE, "Smaller", "document.py:515"),
    k("bracketright", Mods::NONE, "Bigger", "document.py:516"),
    // Krita 兼容
    k(
        "bracketleft",
        Mods {
            ctrl: true,
            alt: false,
            shift: false,
        },
        "RotateLeft",
        "document.py:517",
    ),
    k(
        "bracketright",
        Mods {
            ctrl: true,
            alt: false,
            shift: false,
        },
        "RotateRight",
        "document.py:518",
    ),
    k("less", Mods::NONE, "LessOpaque", "document.py:519"),
    k("greater", Mods::NONE, "MoreOpaque", "document.py:520"),
    k("equal", Mods::NONE, "ZoomIn", "document.py:521"),
    k("comma", Mods::NONE, "Smaller", "document.py:522"),
    k("period", Mods::NONE, "Bigger", "document.py:523"),
    k("BackSpace", Mods::NONE, "ClearLayer", "document.py:525"),
    // MyPaint 老式键位
    k("z", Mods::NONE, "Undo", "document.py:527"),
    k(
        "y",
        Mods {
            ctrl: true,
            alt: false,
            shift: false,
        },
        "Redo",
        "document.py:528",
    ),
    k("y", Mods::NONE, "Redo", "document.py:529"),
    // Ctrl+W 退出（源码里是 lambda，不是动作）
    KeyDef {
        accel: Accel {
            key: "w",
            mods: Mods {
                ctrl: true,
                alt: false,
                shift: false,
            },
        },
        action: None,
        behavior: Some("quit"),
        source: "document.py:530",
        menu_level: false,
    },
    // 小键盘
    k("KP_Add", Mods::NONE, "ZoomIn", "document.py:531"),
    k("KP_Subtract", Mods::NONE, "ZoomOut", "document.py:532"),
    k("KP_4", Mods::NONE, "RotateLeft", "document.py:533"),
    k("KP_6", Mods::NONE, "RotateRight", "document.py:534"),
    k("KP_5", Mods::NONE, "ResetRotation", "document.py:535"),
    k("plus", Mods::NONE, "ZoomIn", "document.py:536"),
    k("minus", Mods::NONE, "ZoomOut", "document.py:537"),
    k(
        "plus",
        Mods {
            ctrl: true,
            alt: false,
            shift: false,
        },
        "ZoomIn",
        "document.py:538",
    ),
    k(
        "minus",
        Mods {
            ctrl: true,
            alt: false,
            shift: false,
        },
        "ZoomOut",
        "document.py:539",
    ),
    k("bar", Mods::NONE, "SymmetryActive", "document.py:540"),
    // 光标键平移（非动作行为）
    KeyDef {
        accel: Accel {
            key: "Left",
            mods: Mods::NONE,
        },
        action: None,
        behavior: Some("pan-left"),
        source: "document.py:542",
        menu_level: false,
    },
    KeyDef {
        accel: Accel {
            key: "Right",
            mods: Mods::NONE,
        },
        action: None,
        behavior: Some("pan-right"),
        source: "document.py:543",
        menu_level: false,
    },
    KeyDef {
        accel: Accel {
            key: "Down",
            mods: Mods::NONE,
        },
        action: None,
        behavior: Some("pan-down"),
        source: "document.py:544",
        menu_level: false,
    },
    KeyDef {
        accel: Accel {
            key: "Up",
            mods: Mods::NONE,
        },
        action: None,
        behavior: Some("pan-up"),
        source: "document.py:545",
        menu_level: false,
    },
    // 旋转（控件 + 方向键两套；Ctrl+Left/Right 与 XML 的 RotateLeft/Right 重复，
    // 但 XML 那条优先级更高，正好一致）
    k(
        "Left",
        Mods {
            ctrl: true,
            alt: false,
            shift: false,
        },
        "RotateLeft",
        "document.py:547",
    ),
    k(
        "Right",
        Mods {
            ctrl: true,
            alt: false,
            shift: false,
        },
        "RotateRight",
        "document.py:548",
    ),
];

/// 一次查表结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// 命中 `resources.xml` 里的动作（优先级 1）
    Action(&'static str),
    /// 命中代码注册的动作（优先级 2）
    ExtraAction(&'static str),
    /// 命中代码注册的非动作行为（优先级 2）
    Behavior(&'static str),
}

impl Bound {
    pub fn action_id(&self) -> Option<&'static str> {
        match self {
            Bound::Action(id) | Bound::ExtraAction(id) => Some(id),
            Bound::Behavior(_) => None,
        }
    }
}

/// 按键查表 —— 复刻 `gui/keyboard.py:139-142` 的两级查找。
///
/// 顺序：
/// 1. 笔刷槽位键（运行期加进 `DocumentActions` 组，与 menu 级同级）
/// 2. `resources.xml` 的加速键
/// 3. 代码里的硬编码兜底键（**逆序**：源码说同键绑两次时后一个生效）
///
/// 这是**默认表**的查表（`resources.xml` + `document.py` 注册的键）。运行时
/// 应用还要叠一层用户覆盖（`accelmap.conf`），那条链路在
/// [`crate::shell::accelmap::AccelMap::lookup`] —— 它先查覆盖层，再查
/// [`lookup_menu`]，最后查 [`lookup_fallback`]，顺序与源码的两张表一致。
///
/// 应用本地扩展键不在这个顺序里 —— 它们走 [`lookup_app`]，由调用方先查。
pub fn lookup(key: egui::Key, mods: Mods) -> Option<Bound> {
    lookup_menu(key, mods).or_else(|| lookup_fallback(key, mods))
}

/// 菜单级（`keymap`，优先级 1）：槽位键 + `resources.xml` 的加速键。
pub fn lookup_menu(key: egui::Key, mods: Mods) -> Option<Bound> {
    // 1. 槽位键：`0`-`9` 恢复、`Ctrl+0`-`9` 存入
    if mods.alt == false && mods.shift == false {
        if let Some(n) = digit_of(key) {
            return Some(Bound::Action(context_action_name(n, mods.ctrl)));
        }
    }

    // 2. `resources.xml` 的加速键
    if let Some(a) = ACTIONS
        .iter()
        .find(|a| a.accel.is_some_and(|acc| matches(acc, key, mods)))
    {
        return Some(Bound::Action(a.id));
    }
    None
}

/// 硬编码兜底（`keymap2`，优先级 2）—— 逆序：源码"后绑定的生效"。
pub fn lookup_fallback(key: egui::Key, mods: Mods) -> Option<Bound> {
    for d in EXTRA_ACCELS.iter().rev() {
        if matches(d.accel, key, mods) {
            return Some(match (d.action, d.behavior) {
                (Some(id), _) => Bound::ExtraAction(id),
                (None, Some(b)) => Bound::Behavior(b),
                (None, None) => continue,
            });
        }
    }
    None
}

/// `Num0`-`Num9` → `0`-`9`。
fn digit_of(key: egui::Key) -> Option<usize> {
    Some(match key {
        egui::Key::Num0 => 0,
        egui::Key::Num1 => 1,
        egui::Key::Num2 => 2,
        egui::Key::Num3 => 3,
        egui::Key::Num4 => 4,
        egui::Key::Num5 => 5,
        egui::Key::Num6 => 6,
        egui::Key::Num7 => 7,
        egui::Key::Num8 => 8,
        egui::Key::Num9 => 9,
        _ => return None,
    })
}

// ---- 应用本地扩展键（MyPaint 源码里没有）----

/// 一条**应用本地**扩展键。
///
/// 单列一张表、并且每条都写明理由，是为了让"偏离源码"这件事**看得见**：
/// 照搬的键位进 [`ACTIONS`] / [`EXTRA_ACCELS`]，自己加的必须在这里登记，
/// 且只能占用源码没碰过的键位。
#[derive(Clone, Copy, Debug)]
pub struct AppKey {
    /// 应用内部的伪动作名（`pigment:` 前缀表明它不是 MyPaint 动作）
    pub id: &'static str,
    pub accel: Accel,
    /// 为什么需要它 / 为什么用这个键位
    pub note: &'static str,
}

/// 应用本地扩展键表。
///
/// 全部避开了源码用到的键位：
/// - `x`：`resources.xml` 与 `document.py` 的**裸字母键**里唯一没被占用的
///   （a-w、y、z 都各有主人）。
/// - `Ctrl+F1` / `Ctrl+F2`：源码里 `F1` 是 `OnlineHelpIndex`、`F2` 是
///   `LayerProperties`，两个 Ctrl 组合都空着。
/// - `Ctrl+,` / `Ctrl+.`：源码里裸 `,`/`.` 是 ZoomOut/ZoomIn，Ctrl 组合空着。
/// - `Ctrl+Shift+R`：源码里 `r` 的裸/Ctrl/Alt/Shift 各有主人，Ctrl+Shift 空着。
pub static APP_KEYS: &[AppKey] = &[
    AppKey {
        id: "pigment:spectral",
        accel: Accel {
            key: "x",
            mods: Mods::NONE,
        },
        note: "光谱混色（颜料感）开关。MyPaint 里 Pigment 是**笔刷设置**，\
               在笔刷编辑器里（`gui/brusheditor.py:465`，显示名 `Pigment:` 见 \
               `gui/mode.py:601`），没有快捷键；本应用还没接笔刷编辑器的那一列，\
               先用一个空闲键让这个能力可试。裸 `p` 在源码里是 FlipFreehandMode，\
               不能占用。等 B4 接上笔刷的 `paint_mode` 后可以撤掉。",
    },
    AppKey {
        id: "pigment:resync-gpu",
        accel: Accel {
            key: "F1",
            mods: Mods {
                ctrl: true,
                alt: false,
                shift: false,
            },
        },
        note: "诊断：丢弃全部显存 tile 并全量重传（用来区分「CPU 没画上」与\
               「显存没同步」）。源码无此功能（这是本项目实现引入的失效模式）。",
    },
    AppKey {
        id: "pigment:dump-canvas",
        accel: Accel {
            key: "F2",
            mods: Mods {
                ctrl: true,
                alt: false,
                shift: false,
            },
        },
        note: "诊断：把 CPU 侧画布导出成 PNG（与屏幕截图对照，定位渲染问题）。\
               源码无此功能。",
    },
    AppKey {
        id: "pigment:prev-brush",
        accel: Accel {
            key: "comma",
            mods: Mods {
                ctrl: true,
                alt: false,
                shift: false,
            },
        },
        note: "上一支笔刷。**源码里没有这个键位**（MyPaint 靠鼠标在笔刷面板里选），\
               原来是本应用的裸 `,`；但裸 `,` 在源码里是 ZoomOut，所以让位到 Ctrl 变体。",
    },
    AppKey {
        id: "pigment:next-brush",
        accel: Accel {
            key: "period",
            mods: Mods {
                ctrl: true,
                alt: false,
                shift: false,
            },
        },
        note: "下一支笔刷，理由同 `pigment:prev-brush`（裸 `.` 在源码里是 ZoomIn）。",
    },
];

/// 查应用本地扩展键。
pub fn lookup_app(key: egui::Key, mods: Mods) -> Option<&'static AppKey> {
    APP_KEYS.iter().find(|k| matches(k.accel, key, mods))
}

/// 动态动作名是 `format!` 出来的，查表却要 `&'static str`。
///
/// 槽位动作名总共只有 `2 * NUM_CONTEXT_SLOTS` 个，首次用到时一次性 intern
/// 成静态串（不用 `Box::leak`：那个写法每按一次键就漏一小块）。
///
/// B6-2 的默认表（`shell::accelmap::build_rows`）也用这个函数拿槽位动作名，
/// 所以它是 `pub(crate)`。
pub(crate) fn context_action_name(slot: usize, store: bool) -> &'static str {
    static NAMES: OnceLock<[String; NUM_CONTEXT_SLOTS * 2]> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        std::array::from_fn(|i| {
            let (restore, save) = context_action_ids(i % NUM_CONTEXT_SLOTS);
            if i < NUM_CONTEXT_SLOTS {
                restore
            } else {
                save
            }
        })
    });
    &names[if store {
        slot + NUM_CONTEXT_SLOTS
    } else {
        slot
    }]
}

fn matches(a: Accel, key: egui::Key, mods: Mods) -> bool {
    a.egui_binding() == Some((key, mods))
}

/// 该 GDK 键名是否被本项目的映射表覆盖（单测用：保证没有静默漏掉的键）。
pub fn is_mapped_gdk_key(name: &'static str) -> bool {
    Accel {
        key: name,
        mods: Mods::NONE,
    }
    .egui_key()
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const CTRL: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: false,
    };
    const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: true,
    };
    const SHIFT: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: true,
    };

    #[test]
    fn table_size_and_groups_match_resources_xml() {
        // 实测数字（`resources.xml` 逐个数出来的）：
        //   219 个动作 / 5 个组 / 105 条加速键
        assert_eq!(ACTIONS.len(), 219, "动作数应与 resources.xml 一致");
        assert_eq!(GROUPS.len(), 5);
        assert_eq!(GROUPS[0], "FileActions");
        assert_eq!(GROUPS[4], "ModeStackActions");
        assert_eq!(
            ACTIONS.iter().filter(|a| a.accel.is_some()).count(),
            105,
            "加速键条数应与 resources.xml 一致"
        );
    }

    #[test]
    fn app_keys_do_not_shadow_source_accels() {
        // `BrushResizeMode` 现在只经 `input.button_mapping` 进入（源码同样没有
        // 加速键）；这里仍守住"应用扩展键不能与源码键表串台"。
        assert_eq!(lookup(egui::Key::R, CTRL_SHIFT), None);
        // 表内不能有重复键位（先到先得会悄悄吃掉后一条）
        let mut seen = HashSet::new();
        for k in APP_KEYS {
            assert!(
                seen.insert((k.accel.key, format!("{:?}", k.accel.mods))),
                "应用扩展键重复: {}",
                k.id
            );
        }
    }

    #[test]
    fn class_histogram_matches_resources_xml() {
        let count = |k: Kind| ACTIONS.iter().filter(|a| a.kind == k).count();
        assert_eq!(count(Kind::Action), 156);
        assert_eq!(count(Kind::Toggle), 37);
        assert_eq!(count(Kind::Radio), 22);
        assert_eq!(count(Kind::Factory), 3);
        assert_eq!(count(Kind::Recent), 1);
    }

    #[test]
    fn action_ids_are_unique() {
        let mut seen = HashSet::new();
        for a in ACTIONS {
            assert!(seen.insert(a.id), "动作名重复：{}", a.id);
        }
    }

    #[test]
    fn every_accel_key_is_mapped_to_an_egui_key() {
        let mut unmapped: Vec<&str> = ACTIONS
            .iter()
            .filter_map(|a| a.accel)
            .chain(EXTRA_ACCELS.iter().map(|d| d.accel))
            .filter(|acc| acc.egui_key().is_none())
            .map(|acc| acc.key)
            .collect();
        unmapped.sort_unstable();
        unmapped.dedup();
        assert_eq!(
            unmapped, UNMAPPABLE_GDK_KEYS,
            "无法映射的键名集合应与 UNMAPPABLE_GDK_KEYS（含文档说明）一致"
        );
    }

    /// 加速键显示名与 **本机 GTK 的真值** 逐条比对。
    ///
    /// fixture 由 `.workbuddy/tools/gen-accel-labels.py` 用
    /// `/e/mypaint-w64/mingw64/bin/python.exe`（PyGObject + GTK 3.24.20）
    /// 跑 `Gtk.accelerator_get_label()` 生成，格式：`动作名<TAB>GTK 显示名`。
    /// 这条门禁把"我以为 GTK 会显示成什么"换成了"GTK 实际显示什么"。
    #[test]
    fn accel_labels_match_gtk() {
        let fixture = include_str!("../../tests/fixtures/accel_labels.tsv");
        let mut checked = 0;
        for line in fixture.lines() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (id, expected) = line.split_once('\t').expect("fixture 行格式：id<TAB>label");
            let a = by_id(id).unwrap_or_else(|| panic!("fixture 里的动作不在表里：{id}"));
            let got = a.accel.expect("fixture 里的动作应有加速键").label();
            assert_eq!(got, expected, "{id} 的加速键显示名与 GTK 不符");
            checked += 1;
        }
        assert_eq!(checked, 105, "fixture 应覆盖全部 105 条加速键");
    }

    #[test]
    fn centered_actions_copy_label_and_tooltip() {
        let z = by_id("ZoomInCentered").unwrap();
        assert_eq!(z.label, None, "源码里 Centered 动作没有自己的 label");
        assert_eq!(z.effective_label(), Some("Zoom In"));
        assert_eq!(
            z.effective_tooltip(),
            Some("Increase magnification."),
            "accelmap.py:178-191 从 ZoomIn 复制 tooltip"
        );
    }

    #[test]
    fn accel_editor_filter_follows_accelmap_py() {
        let listed: Vec<&str> = ACTIONS
            .iter()
            .filter(|a| a.is_listed_in_accel_editor())
            .map(|a| a.id)
            .collect();
        // 实测 170 条（跳过后见 accelmap.py 的四条规则）
        assert_eq!(listed.len(), 170, "按键编辑器可见动作数");
        for skipped in ["OpenRecent", "BrushGroups", "LayerMode", "FileMenu", "Undo"] {
            assert!(
                !listed.contains(&skipped),
                "{skipped} 不该出现在按键编辑器里"
            );
        }
        // 菜单结构动作：22 个（无 tooltip 且以 Menu 结尾）
        let menu_only = ACTIONS.iter().filter(|a| a.is_menu_structure()).count();
        assert_eq!(menu_only, 22, "菜单结构动作数（S3 的子菜单挂点）");
        assert!(by_id("ContextMenu").unwrap().is_menu_structure());
    }

    #[test]
    fn undo_redo_labels_are_dynamic_in_source() {
        // 源码里 Undo/Redo 没有静态 label，由命令栈生成 "Undo <命令名>"
        // （document.py:190-193,582-591）—— S3 的菜单要按这条来渲染。
        assert_eq!(by_id("Undo").unwrap().label, None);
        assert_eq!(by_id("Redo").unwrap().label, None);
        assert_eq!(by_id("Undo").unwrap().accel.unwrap().label(), "Ctrl+Z");
    }

    /// B3-5：对称开关的快捷键就是 `u`（`resources.xml:854-860` 里那条
    /// `<accelerator key="u"/>`）；`FrameToggle` 在源码里**没有**加速键
    /// （`resources.xml:862-869` 只有 label/tooltip/icon）。
    #[test]
    fn symmetry_and_frame_toggles_bindings() {
        // label 是给菜单显示的文本（单键大写化，与 GTK 的加速键标签一致）
        assert_eq!(by_id("SymmetryActive").unwrap().accel.unwrap().label(), "U");
        assert_eq!(
            lookup(egui::Key::U, Mods::NONE),
            Some(Bound::Action("SymmetryActive"))
        );
        assert!(by_id("FrameToggle").unwrap().accel.is_none());
    }

    #[test]
    fn lookup_prefers_xml_accel_over_hardcoded_fallback() {
        // `[` / `]`：XML 绑的是 Mirror{Vertical,Horizontal}，硬编码兜底想绑
        // Smaller/Bigger（GIMP 习惯）。菜单级优先 → **镜像是 X M 的键，
        // 笔刷尺寸不是**。（源码 document.py:510 那句"菜单里指定的优先"）
        assert_eq!(
            lookup(egui::Key::OpenBracket, Mods::NONE),
            Some(Bound::Action("MirrorVertical"))
        );
        assert_eq!(
            lookup(egui::Key::CloseBracket, Mods::NONE),
            Some(Bound::Action("MirrorHorizontal"))
        );
        // comma：XML 是 ZoomOut，硬编码兜底想绑 Smaller → 同样是菜单级优先
        assert_eq!(
            lookup(egui::Key::Comma, Mods::NONE),
            Some(Bound::Action("ZoomOut"))
        );
        // 只在兜底表里的那些才轮到
        assert_eq!(
            lookup(egui::Key::Backspace, Mods::NONE),
            Some(Bound::ExtraAction("ClearLayer"))
        );
        // 裸键与带修饰键互不串台（曾因 egui 的 key_pressed 不看修饰键踩过）
        assert_eq!(
            lookup(egui::Key::E, Mods::NONE),
            Some(Bound::Action("BlendModeEraser"))
        );
        assert_eq!(
            lookup(egui::Key::E, CTRL),
            Some(Bound::Action("BlendModeLockAlpha"))
        );
        assert_eq!(lookup(egui::Key::S, CTRL), Some(Bound::Action("Save")));
        assert_eq!(
            lookup(egui::Key::S, CTRL_SHIFT),
            Some(Bound::Action("SaveAs"))
        );
        // 笔刷尺寸在源码里是 f/d（Bigger/Smaller），不是 [ ]
        assert_eq!(
            lookup(egui::Key::F, Mods::NONE),
            Some(Bound::Action("Bigger"))
        );
        assert_eq!(
            lookup(egui::Key::D, Mods::NONE),
            Some(Bound::Action("Smaller"))
        );
    }

    /// Shift+符号键：GDK 不把被消费掉的 Shift 计入修饰键，egui 却照实报。
    #[test]
    fn shifted_symbol_keys_get_an_implicit_shift() {
        // `<` = Shift+, —— 与裸 `,`（ZoomOut）必须区分得开
        assert_eq!(
            lookup(egui::Key::Comma, Mods::NONE),
            Some(Bound::Action("ZoomOut"))
        );
        assert_eq!(
            lookup(egui::Key::Comma, SHIFT),
            Some(Bound::ExtraAction("LessOpaque"))
        );
        assert_eq!(
            lookup(egui::Key::Period, SHIFT),
            Some(Bound::ExtraAction("MoreOpaque"))
        );
        assert_eq!(
            lookup(egui::Key::Backslash, SHIFT),
            Some(Bound::ExtraAction("SymmetryActive"))
        );
        // 主键盘 `+` = Shift+=；egui-winit 报 Equals，不是 Plus。
        assert_eq!(
            lookup(egui::Key::Equals, SHIFT),
            Some(Bound::ExtraAction("ZoomIn"))
        );
        // 小键盘 `+` 在 egui 里不带 Shift，同样命中 ZoomIn
        assert_eq!(
            lookup(egui::Key::Plus, Mods::NONE),
            Some(Bound::ExtraAction("ZoomIn"))
        );
    }

    #[test]
    fn lookup_handles_context_slots_and_extra_behaviors() {
        assert_eq!(
            lookup(egui::Key::Num1, Mods::NONE),
            Some(Bound::Action("Context01"))
        );
        assert_eq!(
            lookup(egui::Key::Num1, CTRL),
            Some(Bound::Action("Context01s"))
        );
        // 光标键在 XML 里有对应的动作（`Left` → PanLeft，`FlipPanViewMode`
        // 之外还单列了四个 Pan 动作），兜底表里的同名 lambda 被它压住
        assert_eq!(
            lookup(egui::Key::ArrowLeft, Mods::NONE),
            Some(Bound::Action("PanLeft"))
        );
        // Ctrl+Left 有两条来源：XML 的 RotateLeft 与兜底表的同名动作 → 都是它
        assert_eq!(
            lookup(egui::Key::ArrowLeft, CTRL),
            Some(Bound::Action("RotateLeft"))
        );
        assert_eq!(lookup(egui::Key::W, CTRL), Some(Bound::Behavior("quit")));
    }

    /// 取色快捷键（B3-4）：`r` / `Ctrl+r` / `Shift+r` / `Alt+r`
    /// （`gui/resources.xml:1896-1930` 的四个 `FlipColorPickMode*`）。
    ///
    /// 这条把"快捷键来自资源表"钉住：`app.run_action` 只按动作名分派，
    /// 一旦动作表生成出偏差，这里先红。
    #[test]
    fn color_pick_shortcuts_are_bound_like_the_source() {
        const ALT: Mods = Mods {
            ctrl: false,
            alt: true,
            shift: false,
        };
        assert_eq!(
            lookup(egui::Key::R, Mods::NONE),
            Some(Bound::Action("FlipColorPickMode"))
        );
        assert_eq!(
            lookup(egui::Key::R, CTRL),
            Some(Bound::Action("FlipColorPickModeH"))
        );
        assert_eq!(
            lookup(egui::Key::R, SHIFT),
            Some(Bound::Action("FlipColorPickModeC"))
        );
        assert_eq!(
            lookup(egui::Key::R, ALT),
            Some(Bound::Action("FlipColorPickModeY"))
        );
        // 四个单选动作（工具条的 Current Mode 组）：都是 RadioAction + 有图标
        for id in [
            "ColorPickMode",
            "ColorPickModeH",
            "ColorPickModeC",
            "ColorPickModeY",
        ] {
            let a = by_id(id).unwrap_or_else(|| panic!("动作表里没有 {id}"));
            assert_eq!(a.kind, Kind::Radio, "{id} 应是 GtkRadioAction");
            assert!(
                a.icon_name.is_some(),
                "{id} 应有图标（`mode_icon_name` 要用）"
            );
        }
    }

    /// `resources.xml` 内部没有重复的（修饰键, 键）组合 —— 查表因此无歧义。    ///
    /// 这条是**上游变更哨兵**：若将来 MyPaint 引入冲突绑定，GTK 那边靠
    /// accelmap 的"后写的删掉先写的"来裁决，我们必须显式复现，不能靠表序。
    #[test]
    fn no_duplicate_accelerators_within_resources_xml() {
        let mut seen: Vec<(egui::Key, Mods)> = Vec::new();
        for a in ACTIONS {
            let Some(acc) = a.accel else { continue };
            let Some(binding) = acc.egui_binding() else {
                continue; // ShowPopupMenu 的 Menu 键（无 egui 对应物）
            };
            if let Some(_prev) = seen.iter().find(|e| **e == binding) {
                panic!("{} 的加速键与表里更早的动作重复：{binding:?}", a.id);
            }
            seen.push(binding);
        }
        assert_eq!(seen.len(), 105, "105 条加速键现在全部可映射（含 Menu）");
    }

    /// 源码用到的 GDK 键名必须全部落在**已知集合**里，且除有意不映射的几条外
    /// 都能落到 egui 键位上。这条挡住"上游新增了一个键名而映射表没跟上"。
    #[test]
    fn every_gdk_key_name_used_by_the_source_is_known() {
        let mut names: Vec<&str> = ACTIONS
            .iter()
            .filter_map(|a| a.accel)
            .map(|a| a.key)
            .chain(EXTRA_ACCELS.iter().map(|d| d.accel.key))
            .collect();
        names.sort_unstable();
        names.dedup();
        let expected_known = [
            "BackSpace",
            "Delete",
            "Down",
            "End",
            "F1",
            "F10",
            "F11",
            "F12",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "Home",
            "KP_4",
            "KP_5",
            "KP_6",
            "KP_Add",
            "KP_Subtract",
            "Left",
            "Menu",
            "Page_Down",
            "Page_Up",
            "Right",
            "Tab",
            "Up",
            "a",
            "b",
            "backslash",
            "bar",
            "bracketleft",
            "bracketright",
            "c",
            "comma",
            "d",
            "e",
            "equal",
            "f",
            "g",
            "grave",
            "greater",
            "h",
            "i",
            "j",
            "k",
            "l",
            "less",
            "m",
            "minus",
            "n",
            "o",
            "p",
            "period",
            "plus",
            "q",
            "r",
            "s",
            "t",
            "u",
            "v",
            "w",
            "x",
            "y",
            "z",
        ];
        for n in &names {
            assert!(
                expected_known.contains(n),
                "{n} 是本轮新增的 GDK 键名，请补上 egui 键位映射与这里的断言"
            );
        }
        for n in &names {
            if UNMAPPABLE_GDK_KEYS.contains(n) {
                assert!(!is_mapped_gdk_key(n), "{n} 在 UNMAPPABLE 表里却映射上了");
                continue;
            }
            assert!(is_mapped_gdk_key(n), "{n} 没有 egui 键位映射");
        }
    }

    /// 内置的 GUI XML 必须与参考树**逐字节一致**。
    ///
    /// 动作表是从 `assets/gui/resources.xml` 构建期生成的，而这个文件是
    /// `_mypaint_ref/gui/` 的副本（见 `.workbuddy/tools/sync-gui-assets.sh`）。
    /// 一旦有人改了参考树忘了同步，这张表就会悄悄落后于源码 —— 所以在这里
    /// 卡一道。参考树不在（换台机器 / 只 clone 了 `pigment/`）时跳过。
    #[test]
    fn vendored_gui_xml_matches_reference_tree() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ref_dir = root.join("../../../_mypaint_ref/gui");
        if !ref_dir.is_dir() {
            eprintln!("Skipped: reference tree {} not found", ref_dir.display());
            return;
        }
        let vendored = root.join("assets/gui");
        let mut checked = 0;
        for entry in std::fs::read_dir(&vendored).expect("读不到 assets/gui") {
            let path = entry.expect("目录项").path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".xml") {
                continue;
            }
            let ours = std::fs::read(&path).expect("读不到内置副本");
            let theirs = std::fs::read(ref_dir.join(&name))
                .unwrap_or_else(|e| panic!("参考树里没有 {name}: {e}"));
            assert_eq!(
                ours.len(),
                theirs.len(),
                "{name} 与 _mypaint_ref 不同（长度不符）—— 跑 .workbuddy/tools/sync-gui-assets.sh"
            );
            assert_eq!(
                ours, theirs,
                "{name} 与 _mypaint_ref 不同 —— 跑 .workbuddy/tools/sync-gui-assets.sh"
            );
            checked += 1;
        }
        assert!(checked >= 11, "内置 GUI XML 应有 11 个，实得 {checked}");
    }
}
