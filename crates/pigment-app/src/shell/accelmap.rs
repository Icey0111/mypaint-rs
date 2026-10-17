//! 加速键映射与按键编辑器 —— `gui/accelmap.py` + `gui/keyboard.py` 的 GUI 侧
//! （B6-2）。
//!
//! # 三层的链路
//!
//! ```text
//! 默认表                     用户覆盖层                    查表
//! gui/resources.xml  ──┐
//!   （build.rs → ACTIONS）├─→ AccelMap.rows ──┐
//! gui/document.py     ──┘  （+ 20 条槽位键）   ├─→ AccelMap.lookup()
//!                                              │    1. 覆盖层（用户改过的）
//! accelmap.conf ──→ AccelMap.overrides ────────┘    2. 默认表（未被改过的）
//!   （GTK 的全表快照，                       （被改过的 path 按新键命中，
//!     pigtest_engine::accelmap 解析）            旧键不再命中）
//!                                                   3. 硬编码兜底（EXTRA_ACCELS）
//! ```
//!
//! 对应源码的三件事：
//!
//! 1. **默认表** = `resources.xml` 的 `<accelerator>`（`build.rs` 生成 [`actions::ACTIONS`]）
//!    + `gui/document.py:467-492` 的 20 条槽位键。GTK 侧这些绑定由
//!    `GtkUIManager` / `gtk_action_group_add_actions()` 注册进全局
//!    `Gtk.AccelMap`（`gui/application.py:417` 之前就存在）。
//! 2. **用户覆盖** = `Gtk.AccelMap.load(<conf>/accelmap.conf)`（`gui/application.py:417`）。
//!    文件是**全表快照**（见 `pigment_engine::accelmap` 的模块文档），逐 path
//!    覆盖默认表 —— 实测：文件里把 `Quit` 改成 `<Primary>w`，加载后
//!    `lookup_entry` 拿到的就是 `<Primary>w`。
//! 3. **查表** = `gui/keyboard.py:75-89` 的 `_update_keymap()` 把
//!    `(keyval, mods) → GtkAction` 塞进 `self.keymap`（优先级 1），
//!    `keymap` 里没有才查 `self.keymap2`（`add_extra_key` 的硬编码兜底，
//!    优先级 2，见 `keyboard.py:139-142`）。
//!
//! 覆盖层为什么排在默认表前面：`keyboard.py` 的 `keymap` 是一张
//! `(key, mods) → action` 的字典，后注册的覆盖先注册的；GTK 启动时先由
//! `GtkUIManager` 注册默认绑定、**再**加载 `accelmap.conf`（`application.py:417`
//! 在窗口/动作建好之后），所以同一个键位上用户那条是后写入的。
//!
//! # `AccelMap` 的语义（逐条对照 `gui/accelmap.py`）
//!
//! | 行为 | 源码 |
//! |---|---|
//! | 编辑器的动作过滤规则 | `accelmap.py:165-221`（见 [`actions::Action::is_listed_in_accel_editor`]） |
//! | 冲突检测（同键的另一个 path） | `accelmap.py:513-526` |
//! | 设键时**删掉**同键的其它条目 | `accelmap.py:571-597` 的 `_delete_clashing_accelmap_entries` + `_set_accelmap_entry` |
//! | 删除绑定（`Delete`/`BackSpace`） | `accelmap.py:556-562`（`change_entry(path, 0, 0, True)`） |
//! | 保存 | `accelmap.py` 不管保存；`gui/application.py:625` 在退出时 `AccelMap.save()` |
//!
//! # 编辑器做到了什么程度
//!
//! 照 `gui/accelmap.py` 的两列（"Action" / "Key combination"，`:115`/`:132`）、
//! 过滤框（`:83-89`，"Filter"）、按键捕获对话框（`:352-568`：
//! 标题 `Edit Key for '{action}'`、`Action:`/`Path:`/`Key:` 三行、提示
//! `Press keys to update this assignment`、`Delete`/`Cancel`/`OK` 三个按钮、
//! `Return`/`Escape`/`BackSpace` 三个快捷键）。**页面外壳**（偏好对话框的
//! "Keys" 页签、GTK 的树视图列渲染/排序/搜索栏）属 B6-3，见记账清单。
//!
//! # 与源码的差异（记账，都往保守方向）
//!
//! - **GTK 树视图没有移植**：egui 没有 `Gtk.TreeView`/`Gtk.ListStore`，
//!   这里用 `ScrollArea` + 手工分列宽的两列（`allocate_ui_with_layout`）画同样的
//!   内容。排序键仍是源码的 `"{action_label}\n{action_desc}"` 升序
//!   （`accelmap.py:64,140-143`）。**没做**：列宽拖动、点表头改排序
//!   （`:104` 的 `set_headers_clickable`）、树视图的内置搜索栏
//!   （`:105-107` 的 `set_enable_search`/`set_search_column`）、
//!   `Gtk.CellRendererText` 的 markup/省略号（`:110-127` 的 `markup`/`ellipsize`）
//!   —— 文本在 egui 里是普通标签。
//! - **冲突提示**显示在动作名旁边（源码只在捕获对话框里提示，
//!   `accelmap.py:531-544`）；这是我们多出来的一列信息，不改任何语义。
//!   源码**没有**"冲突确认对话框"：`_set_accelmap_entry` 直接删掉对方
//!   （`:571-597`），这里照做，只在捕获对话框里用一句话预告要替换谁。
//! - **重置此项 / 全部重置**：源码编辑器**只有** Delete（`:369` 的
//!   `Gtk.STOCK_DELETE` + `:478` 的 BackSpace），删掉的绑定下次启动会被
//!   UI manager 重新注册（`accelmap.conf` 把空条目写成注释行）。
//!   本编辑器按题目要求补了"回到默认表"的两个动作 —— 它们只丢掉覆盖层，
//!   语义等价于"把 accelmap.conf 里这条删掉再重启"。
//! - **保存时机**：源码只在退出时整表落盘（`application.py:625`）。这里每次
//!   编辑成功后立刻落盘一次（崩溃不丢绑定），退出时仍然整表再写一遍。
//! - **`Gtk.AccelMap` 的 `changed` 信号**：源码让 `KeyboardManager` 订阅它
//!   （`keyboard.py:65-89`）以维护 `(key, mods) → action` 字典；egui 侧没有
//!   全局加速键对象，改成"每次按键现查 `AccelMap`"（结果等价，查询是 O(行数)）。
//! - **输入法 / 按键路由**：`keyboard.py:97` 的 `propagate_key_event`（输入框
//!   优先）与 `:214-221` 的 Escape 紧急释放都没有移植（前者靠
//!   `egui::Context::text_edit_focused()` 早退，后者属模式栈）。编辑器窗口
//!   打开时**不**挡全局快捷键（只有捕获对话框挡，见 `PigmentApp::handle_keys`
//!   的早退）—— GTK 那边是模态对话框，这里不伪造。
//! - **带 Super/Meta/Hyper 的绑定**读不进来（egui 无对应修饰位），
//!   `pigment_engine::accelmap` 会把它们记进"跳过行"。
//! - **GTK 的 `Gtk.TreeView` 的 `_SHOW_ACCEL_PATH` 列**：源码只在捕获对话框里
//!   显示路径（`:59` + `:399-414`），列表里没有路径列 —— 这里一致（路径放在
//!   加速键按钮的悬停提示里）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui;

use pigment_engine::accelmap::{self as conf, Accel, Entry};

use crate::app::PigmentApp;
use crate::shell::actions::{self, Bound, Mods, NUM_CONTEXT_SLOTS};

// ---- 与源码逐字相同的界面文本 ----

/// 编辑器窗口/页签标题 —— `gui/preferenceswindow.glade:489` 的 `Keys`。
pub(crate) const TITLE: &str = "Keys";
/// 过滤框占位文字 —— `accelmap.py:85` 的 `C_("placeholder for keymap filtering", "Filter")`。
const FILTER_PLACEHOLDER: &str = "Filter";
/// 第一列标题 —— `accelmap.py:115`。
const COLUMN_ACTION: &str = "Action";
/// 第二列标题 —— `accelmap.py:132`（`"Key combination"`）。
const COLUMN_ACCEL: &str = "Key combination";
/// 捕获对话框标题模板 —— `accelmap.py:365` 的 `_("Edit Key for '%s'")`。
const EDIT_TITLE: &str = "Edit Key for '{action}'";
/// 捕获对话框的行标签 —— `accelmap.py:390,398,406,421`。
const LABEL_ACTION: &str = "Action:";
const LABEL_PATH: &str = "Path:";
const LABEL_KEY: &str = "Key:";
/// 捕获对话框的提示 —— `accelmap.py:463`。
const HINT_PRESS_KEYS: &str = "Press keys to update this assignment";
/// 冲突提示模板 —— `accelmap.py:535-536`（去掉 `<b>` 标记）。
const HINT_CLASH: &str = "{accel} is already in use for '{action}'. The existing assignment will be replaced.";
/// 冲突里"查不到动作名"的占位 —— `accelmap.py:523`。
const UNKNOWN_ACTION: &str = "Unknown Action";
/// 键名后缀 —— `accelmap.py:543,547`。
const SUFFIX_REPLACE: &str = "(replace)";
const SUFFIX_CHANGED: &str = "(changed)";
/// 三个按钮 —— `accelmap.py:368-375` 的 `Gtk.STOCK_DELETE` / `CANCEL` / `OK`。
const BUTTON_DELETE: &str = "Delete";
const BUTTON_CANCEL: &str = "Cancel";
const BUTTON_OK: &str = "OK";
/// 空加速键单元格的占位（**技术栈适配**：GTK 的空单元格点一下就进编辑，
/// egui 的按钮必须有可见内容，否则点不到）。
const EMPTY_ACCEL: &str = "—";

/// "禁用兜底键位"复选框的标签与提示 —— `gui/preferenceswindow.glade:453,457`
/// （同一个 "Keys" 页签里，`:451-466`）。
const FALLBACKS_LABEL: &str = "Disable fallback key bindings";
const FALLBACKS_TIP: &str = "The fallback key bindings are static built-in bindings that are \
                             only used if their key combinations are not bound to anything else.";

// ---- 默认表 ----

/// 默认表里的一行：一个动作（或一条运行期注册的槽位键）+ 它的默认加速键。
///
/// 字段与 `gui/accelmap.py:49-56` 的 `ListStore` 列一一对应：`path` = `_PATH_COLUMN`、
/// `label`/`desc` 拼出 `_ACTION_LABEL_COLUMN`、`default` 是 `_ACCEL_LABEL_COLUMN`
/// 的数据源（显示名由 `Accel::label()` 现算）。
pub(crate) struct Row {
    /// 动作路径 `<Actions>/{group}/{id}`（`accelmap.py:169`）
    pub(crate) path: String,
    /// 动作名（`run_action` 用的 id）
    pub(crate) action: &'static str,
    /// 显示名（`Gtk.Action.get_label()`，`resources.xml` 的 `label` 原文）
    pub(crate) label: String,
    /// 描述（`Gtk.Action.get_tooltip()`）
    pub(crate) desc: String,
    /// 默认加速键（`resources.xml`/`document.py` 声明的；`None` = 没有）
    pub(crate) default: Option<Accel>,
    /// 是否出现在编辑器里（`accelmap.py:165-221` 的过滤规则）
    pub(crate) listed: bool,
}

impl Row {
    /// 排序键 —— `accelmap.py:64` 的 `"{action_label}\n{action_desc}"`，升序。
    fn sort_key(&self) -> (&str, &str) {
        (&self.label, &self.desc)
    }

    /// 过滤用的干草堆 —— `accelmap.py:67` 的
    /// `"{action_label} {action_desc} {accel_label}"`（小写）。
    fn filter_text(&self, accel: Option<&Accel>) -> String {
        let accel_label = accel.map(|a| a.label()).unwrap_or_default();
        format!("{} {} {}", self.label, self.desc, accel_label).to_lowercase()
    }
}

/// 构造默认表：`resources.xml` 的 219 个动作（**全部**，编辑器只显示其中
/// `listed` 的 170 个）+ `document.py:467-492` 的 20 条槽位键。
///
/// 顺序 = XML 顺序，槽位键排在最后（源码把槽位键 `add_actions` 到
/// `DocumentActions` 组末尾，所以它们在这个组的最后 —— 顺序只影响"同一键位
/// 被绑两次时谁赢"，而当前默认表里没有重复键位，见
/// `actions::no_duplicate_accelerators_within_resources_xml`）。
fn build_rows() -> Vec<Row> {
    let mut rows = Vec::with_capacity(actions::ACTIONS.len() + NUM_CONTEXT_SLOTS * 2);
    for a in actions::ACTIONS {
        rows.push(Row {
            path: a.path(),
            action: a.id,
            // `effective_label/tooltip` 处理 `*Centered` 从源动作复制的规则
            // （`accelmap.py:178-191`）
            label: a.effective_label().unwrap_or("").to_string(),
            desc: a.effective_tooltip().unwrap_or("").to_string(),
            default: a.accel.map(to_engine_accel),
            // `Centered` 那批没有自己的 label/tooltip（复制后立刻 continue），
            // 与 `is_listed_in_accel_editor` 的规则一致
            listed: a.is_listed_in_accel_editor(),
        });
    }
    // 槽位键：`gui/document.py:467-492` 的 `_init_context_actions`。
    // 源码里这两条动作的 label/tooltip 是 gettext 的格式串，这里取英文原文。
    for slot in 0..NUM_CONTEXT_SLOTS {
        rows.push(Row {
            path: format!("<Actions>/DocumentActions/{}", slot_action_id(slot, false)),
            action: actions::context_action_name(slot, false),
            label: format!("Restore Brush {slot}"),
            desc: format!("Load brush settings from shortcut slot {slot}"),
            default: Some(Accel::new(slot.to_string(), conf::Mods::NONE)),
            listed: true,
        });
        rows.push(Row {
            path: format!("<Actions>/DocumentActions/{}", slot_action_id(slot, true)),
            action: actions::context_action_name(slot, true),
            label: format!("Save to Brush {slot}"),
            desc: format!("Store brush settings in shortcut slot {slot}"),
            default: Some(Accel::new(slot.to_string(), conf::Mods::new(true, false, false))),
            listed: true,
        });
    }
    rows
}

/// 槽位动作名（`Context0N` / `Context0Ns`）—— `gui/document.py:473-486`。
fn slot_action_id(slot: usize, store: bool) -> String {
    let (restore, save) = actions::context_action_ids(slot);
    if store {
        save
    } else {
        restore
    }
}

fn to_engine_accel(a: actions::Accel) -> Accel {
    Accel::new(a.key, a.mods.to_accel())
}

fn from_engine_mods(m: conf::Mods) -> Mods {
    Mods::from(m)
}

// ---- AccelMap ----

/// 加速键映射：默认表 + 用户覆盖层（`accelmap.conf`）+ 冲突索引。
///
/// 一个实例对应源码里的**全局** `Gtk.AccelMap`（`Gtk.AccelMap.get()`，
/// `accelmap.py:288`）。
pub(crate) struct AccelMap {
    /// 默认表（`resources.xml` + 槽位键），顺序固定
    rows: Vec<Row>,
    /// 用户覆盖层：path → 加速键；`Some(None)` = 这条被**清空**（GTK 的
    /// `change_entry(path, 0, 0)`），`None`（键不存在）= 没覆盖
    overrides: BTreeMap<String, Option<Accel>>,
    /// 冲突索引：path → 与它同键的另一个 path（`accelmap.py:513-526`）
    conflicts: BTreeMap<String, String>,
    /// `accelmap.conf` 的位置（`gui/application.py:417,625`）
    conf_path: PathBuf,
}

impl AccelMap {
    /// 建一张只有默认表的映射（`conf_path` 是加载/保存的位置）。
    pub(crate) fn new(conf_path: impl Into<PathBuf>) -> Self {
        let mut map = AccelMap {
            rows: build_rows(),
            overrides: BTreeMap::new(),
            conflicts: BTreeMap::new(),
            conf_path: conf_path.into(),
        };
        map.recompute_conflicts();
        map
    }

    /// 默认配置路径上的映射（真实入口）。
    pub(crate) fn with_default_path() -> Self {
        Self::new(conf::accelmap_path())
    }

    pub(crate) fn conf_path(&self) -> &std::path::Path {
        &self.conf_path
    }

    /// 读 `accelmap.conf` 并**整张覆盖**当前覆盖层 —— `gui/application.py:417`。
    ///
    /// 返回套用的条目数。文件不存在算成功（首次启动）。
    pub(crate) fn load(&mut self) -> std::io::Result<usize> {
        let parsed = conf::load_conf(&self.conf_path)?;
        self.overrides.clear();
        for entry in parsed.entries {
            self.overrides.insert(entry.path, entry.accel);
        }
        for skipped in &parsed.skipped {
            eprintln!(
                "accelmap.conf 有读不懂的行（{}）：{:?}",
                skipped.reason, skipped.line
            );
        }
        self.recompute_conflicts();
        Ok(self.overrides.len())
    }

    /// 写 `accelmap.conf` —— `gui/application.py:625`。
    pub(crate) fn save(&self) -> std::io::Result<()> {
        conf::save_conf(
            &self.conf_path,
            conf::PROGRAM_NAME,
            &self.entries_for_save(),
        )
    }

    /// 落盘内容 = 整张表（默认表 + 覆盖层），外加文件里带来但我们没有的 path。
    ///
    /// 与 GTK 的 `gtk_accel_map_save()` 一致：**全表快照**；"从来没有键"的动作
    /// 在 accelmap 里**没有条目**（GTK 只在真有加速键时才建条目），所以这里也
    /// 不写；被清空的条目写成注释行（见 `pigment_engine::accelmap` 模块文档）。
    pub(crate) fn entries_for_save(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let resolved = self.accel(&row.path);
            if resolved.is_none() && row.default.is_none() {
                // 从来就没绑过键 → 没有条目（写空条目会变成一条无意义的注释）
                continue;
            }
            entries.push(Entry::new(row.path.clone(), resolved.cloned()));
        }
        for (path, over) in &self.overrides {
            if !self.rows.iter().any(|r| &r.path == path) {
                entries.push(Entry::new(path.clone(), over.clone()));
            }
        }
        entries
    }

    /// 默认表的行数（含编辑器里不显示的项）。
    pub(crate) fn row_count(&self) -> usize {
        self.rows().len()
    }

    /// 全部 path（默认表 + 覆盖层里的陌生 path），字典序。
    fn all_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.rows.iter().map(|r| r.path.clone()).collect();
        for path in self.overrides.keys() {
            if !paths.iter().any(|p| p == path) {
                paths.push(path.clone());
            }
        }
        paths.sort();
        paths
    }

    /// 某个 path **当前生效**的加速键（覆盖层优先，其次默认表）。
    pub(crate) fn accel(&self, path: &str) -> Option<&Accel> {
        match self.overrides.get(path) {
            // 被清空的条目：真的没有键（不会退回默认表 —— 对应 GTK 里
            // `accel_key == 0` 的条目）
            Some(over) => over.as_ref(),
            None => self.default_accel(path),
        }
    }

    /// 某个 path 在默认表里的加速键（`resources.xml`/`document.py` 声明的）。
    pub(crate) fn default_accel(&self, path: &str) -> Option<&Accel> {
        self.rows
            .iter()
            .find(|r| r.path == path)
            .and_then(|r| r.default.as_ref())
    }

    /// 当前的键是否与默认表**不同**（= 界面上要标"已改过"）。
    pub(crate) fn is_overridden(&self, path: &str) -> bool {
        self.accel(path) != self.default_accel(path)
    }

    /// 默认表的全部行（`resources.xml` 的动作 + 槽位键，顺序固定）。
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// 动作名 → 动作路径（槽位键也在这张表里）。
    pub(crate) fn path_of_action(&self, action_id: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|r| r.action == action_id)
            .map(|r| r.path.as_str())
    }

    /// 动作名（找不到返回 `None`）。
    pub(crate) fn action_of_path(&self, path: &str) -> Option<&'static str> {
        self.rows.iter().find(|r| r.path == path).map(|r| r.action)
    }

    /// 显示名（找不到 = 陌生 path，返回 `None` → 界面用 [`UNKNOWN_ACTION`]）。
    pub(crate) fn action_label_of_path(&self, path: &str) -> Option<&str> {
        self.rows.iter().find(|r| r.path == path).map(|r| r.label.as_str())
    }

    /// 这条 path 是否认识（不认识 = `accelmap.conf` 里带进来的陌生动作，
    /// 源码对它们只记一条 warning 而不进 `keymap`：`keyboard.py:89`）。
    pub(crate) fn is_known_path(&self, path: &str) -> bool {
        self.rows.iter().any(|r| r.path == path)
    }

    // ---- 编辑 ----

    /// 设一条绑定 —— `accelmap.py:587-597` 的 `_set_accelmap_entry()`：
    /// **先删掉**所有其它 path 上的同键条目，再写这一条。
    ///
    /// 返回被删掉的 path 列表（界面上可以提示"替换了谁"）。
    pub(crate) fn set_accel(&mut self, path: &str, accel: Accel) -> Vec<String> {
        let mut deleted = Vec::new();
        for other in self.all_paths() {
            if other == path {
                continue;
            }
            if self.accel(&other) == Some(&accel) {
                deleted.push(other);
            }
        }
        for p in &deleted {
            self.overrides.insert(p.clone(), None);
        }
        self.overrides.insert(path.to_string(), Some(accel));
        self.recompute_conflicts();
        deleted
    }

    /// 删除一条绑定 —— `accelmap.py:556-562`（`change_entry(path, 0, 0, True)`）。
    ///
    /// 删掉的只是**这一层**：`accelmap.conf` 会把空条目写成注释行，所以下次
    /// 启动默认绑定会回来（GTK 的行为，见 `pigment_engine::accelmap`）。
    pub(crate) fn clear(&mut self, path: &str) {
        if self.is_known_path(path) {
            self.overrides.insert(path.to_string(), None);
        } else {
            self.overrides.remove(path);
        }
        self.recompute_conflicts();
    }

    /// 回到默认表（丢掉这一条的覆盖层）。返回是否真的改动了什么。
    pub(crate) fn reset_one(&mut self, path: &str) -> bool {
        let changed = self.overrides.remove(path).is_some();
        if changed {
            self.recompute_conflicts();
        }
        changed
    }

    /// 全部回到默认表。返回丢掉的覆盖条数。
    pub(crate) fn reset_all(&mut self) -> usize {
        let n = self.overrides.len();
        self.overrides.clear();
        if n > 0 {
            self.recompute_conflicts();
        }
        n
    }

    /// 覆盖层里的条数（含"被清空"的条目）。
    pub(crate) fn override_count(&self) -> usize {
        self.overrides.len()
    }

    // ---- 冲突 ----

    /// 这条 path 的当前绑定是否与**别的**条目撞了；撞了就返回对方的 path
    /// （`accelmap.py:513-526` 的扫描逻辑）。
    pub(crate) fn clash_of(&self, path: &str) -> Option<&str> {
        self.conflicts.get(path).map(String::as_str)
    }

    /// 如果给这条 path 设成 `accel`，会与谁撞（捕获对话框的实时提示，
    /// `accelmap.py:513-526`）。
    pub(crate) fn clash_of_accel(&self, path: &str, accel: &Accel) -> Option<String> {
        self.all_paths()
            .into_iter()
            .find(|other| other != path && self.accel(other) == Some(accel))
    }

    fn recompute_conflicts(&mut self) {
        let paths = self.all_paths();
        // 先算出结果再写回，避免同时借 self 的不可变与可变
        let mut conflicts: BTreeMap<String, String> = BTreeMap::new();
        for path in &paths {
            let Some(accel) = self.accel(path) else {
                continue;
            };
            if let Some(other) = paths
                .iter()
                .find(|o| *o != path && self.accel(o) == Some(accel))
            {
                conflicts.insert(path.clone(), other.clone());
            }
        }
        self.conflicts = conflicts;
    }

    // ---- 查表 ----

    /// 按键查表 —— `gui/keyboard.py:139-142` 的两级查找 + 用户覆盖层。
    ///
    /// 1. **覆盖层**：用户改过的绑定（含从 `accelmap.conf` 读进来的）优先，
    ///    按 path 字典序（GTK 是哈希序，见模块文档的记账）；陌生 path 跳过
    ///    （`keyboard.py:89` 只记 warning）。
    /// 2. **默认表**：[`actions::lookup_menu`]。但命中之后要**再确认**
    ///    这个动作的绑定没被挪走/删掉 —— 否则改了键的动作仍会响应旧键。
    /// 3. **硬编码兜底**：[`actions::lookup_fallback`]（`keymap2`）。
    ///    `allow_fallbacks == false` 时整段跳过 —— `gui/keyboard.py:140,159-160`
    ///    的 `fallbacks_disabled()`（偏好 `keyboard.disable_fallbacks`，
    ///    复选框在 `gui/preferenceswindow.glade:451-466`）。
    pub(crate) fn lookup(
        &self,
        key: egui::Key,
        mods: Mods,
        allow_fallbacks: bool,
    ) -> Option<Bound> {
        let matches = |accel: &Accel| {
            actions::egui_binding_for(&accel.key, from_engine_mods(accel.mods))
                == Some((key, mods))
        };
        // 1. 覆盖层
        for (path, over) in &self.overrides {
            let Some(accel) = over else { continue };
            if !matches(accel) {
                continue;
            }
            if let Some(action) = self.action_of_path(path) {
                return Some(Bound::Action(action));
            }
        }
        // 2. 默认表（被改过的条目按新键命中，旧键不再命中）
        if let Some(bound) = actions::lookup_menu(key, mods) {
            if let Bound::Action(action) = bound {
                let still_bound = self
                    .path_of_action(action)
                    .and_then(|path| self.accel(path))
                    .is_some_and(&matches);
                if !still_bound {
                    return if allow_fallbacks {
                        actions::lookup_fallback(key, mods)
                    } else {
                        None
                    };
                }
            }
            return Some(bound);
        }
        // 3. 硬编码兜底
        if allow_fallbacks {
            actions::lookup_fallback(key, mods)
        } else {
            None
        }
    }

    // ---- 编辑器视图 ----

    /// 编辑器里的一行（`accelmap.py:165-231` 的过滤 + `:140-143` 的排序）。
    ///
    /// 排序键是 `"{action_label}\n{action_desc}"` 升序（字节序，与
    /// `Gtk.ListStore` 的 `str` 列比较一致）。
    pub(crate) fn editor_rows(&self) -> Vec<&Row> {
        let mut rows: Vec<&Row> = self.rows.iter().filter(|r| r.listed).collect();
        rows.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        rows
    }

    /// 过滤（源码的 `_filter_check`，`accelmap.py:305-344`）——
    /// **技术栈适配**：源码用一个古怪的正则把 `Ctrl+Shift+F` 拆词后在
    /// `"{label} {desc} {accel}"` 的拆词结果里找子串；这里简化为
    /// "空白与 `+` 拆词、每个词都必须是整串（小写）的子串"。
    pub(crate) fn row_matches(&self, row: &Row, filter: &str) -> bool {
        let filter = filter.trim();
        if filter.is_empty() {
            return true;
        }
        let haystack = row.filter_text(self.accel(&row.path));
        filter
            .split([' ', '\t', '+'])
            .filter(|w| !w.is_empty())
            .all(|w| haystack.contains(&w.to_lowercase()))
    }
}

/// 编辑器的界面状态（egui 立即模式：控件没有对象，只能由应用持有）。
#[derive(Default)]
pub(crate) struct KeyEditorState {
    /// 窗口是否打开（入口是 `PreferencesWindow` 动作，见 [`PigmentApp::keybindings_window`]）
    pub(crate) open: bool,
    /// 过滤框内容（`accelmap.py:83-89`）
    pub(crate) filter: String,
    /// 正在捕获新键的动作（`None` = 没在捕获）
    pub(crate) capture: Option<Capture>,
    /// 底部状态文字（保存成功/失败之类）
    pub(crate) status: Option<String>,
}

/// 按键捕获对话框的状态 —— `accelmap.py:352-568` 的 `dialog.*` 那几个属性。
pub(crate) struct Capture {
    /// 正在改的动作路径（`dialog.accel_path`）
    path: String,
    /// 动作显示名（标题里用）
    action_label: String,
    /// 当前按下的键（`dialog.result_keyval/mods`；`None` = 还没按）
    attempt: Option<Accel>,
    /// 与谁冲突（`accelmap.py:513-526` 的 `clash_accel_path`）
    clash: Option<String>,
    /// 提示文字（`dialog.hint_widget`）
    hint: String,
}

impl Capture {
    fn new(map: &AccelMap, path: &str) -> Self {
        Capture {
            path: path.to_string(),
            action_label: map
                .action_label_of_path(path)
                .unwrap_or(UNKNOWN_ACTION)
                .to_string(),
            attempt: None,
            clash: None,
            hint: HINT_PRESS_KEYS.to_string(),
        }
    }

    /// 按下新键之后更新提示 —— `accelmap.py:527-551`：
    /// 与别人冲突 = "(replace) + 警告"；否则标准提示（后缀按"是否与当前键
    /// 相同"在 [`Capture::key_text`] 里区分）。
    fn set_attempt(&mut self, map: &AccelMap, accel: Accel) {
        let clash = map.clash_of_accel(&self.path, &accel);
        match &clash {
            Some(other) => {
                let other_label = map.action_label_of_path(other).unwrap_or(UNKNOWN_ACTION);
                self.hint = HINT_CLASH
                    .replace("{accel}", &accel.label())
                    .replace("{action}", other_label);
            }
            None => self.hint = HINT_PRESS_KEYS.to_string(),
        }
        self.clash = clash;
        self.attempt = Some(accel);
    }

    /// "Key:" 那一行显示的文字 —— `accelmap.py:529-548`（无键时用当前标签）。
    fn key_text(&self, map: &AccelMap) -> String {
        match (&self.attempt, &self.clash) {
            (None, _) => map
                .accel(&self.path)
                .map(|a| a.label())
                .unwrap_or_default(),
            (Some(accel), Some(_)) => format!("{} {}", accel.label(), SUFFIX_REPLACE),
            (Some(accel), None) => {
                if map.accel(&self.path) == Some(accel) {
                    accel.label()
                } else {
                    format!("{} {}", accel.label(), SUFFIX_CHANGED)
                }
            }
        }
    }
}

// ---- 应用接线 ----

impl PigmentApp {
    /// 启动时加载 `accelmap.conf` —— `gui/application.py:417`。
    ///
    /// 在默认表（`resources.xml` + 槽位键）建好之后调用，效果同源码：
    /// 用户文件逐 path 覆盖默认表。
    pub(crate) fn load_accel_map(&mut self) {
        self.accel_map = AccelMap::with_default_path();
        match self.accel_map.load() {
            Ok(n) => eprintln!(
                "加速键：从 {} 载入 {n} 条覆盖",
                self.accel_map.conf_path().display()
            ),
            Err(e) => eprintln!(
                "加速键：读不到 {}（{e}），只用默认表",
                self.accel_map.conf_path().display()
            ),
        }
    }

    /// 把加速键表落盘（退出、每次编辑成功后各一次；见模块文档的记账）。
    pub(crate) fn save_accel_map(&mut self) {
        match self.accel_map.save() {
            Ok(()) => {
                self.key_editor.status = Some(format!(
                    "已保存到 {}",
                    self.accel_map.conf_path().display()
                ));
            }
            Err(e) => {
                let msg = format!("加速键保存失败：{e}");
                eprintln!("{msg}");
                self.key_editor.status = Some(msg);
            }
        }
    }

    /// "Keys" 窗口（[`crate::shell::accelmap::TITLE`]）。
    ///
    /// 入口是 `PreferencesWindow` 动作（`resources.xml:1330-1335` 的
    /// "Edit Preferences"）：源码把 accelmap 编辑器放在偏好对话框的 "Keys"
    /// 页签里（`gui/preferenceswindow.glade:439` 的 `AccelMapEditor`），
    /// 偏好对话框整体属 B6-3，这里先只给这一页。
    pub(crate) fn keybindings_window(&mut self, ctx: &egui::Context) {
        if self.key_editor.open {
            let mut open = true;
            egui::Window::new(TITLE)
                .open(&mut open)
                .resizable(true)
                .default_width(780.0)
                .default_height(500.0)
                .show(ctx, |ui| self.keybindings_ui(ui));
            if !open {
                self.key_editor.open = false;
                self.key_editor.capture = None;
            }
        }
        if self.key_editor.capture.is_some() {
            self.capture_modal(ctx);
        }
    }

    fn keybindings_ui(&mut self, ui: &mut egui::Ui) {
        // 过滤框（`accelmap.py:83-89`）
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.key_editor.filter)
                    .hint_text(FILTER_PLACEHOLDER)
                    .desired_width(200.0),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} 项 / 默认表 {} 条 / 覆盖 {} 条",
                    self.accel_map.editor_rows().len(),
                    self.accel_map.row_count(),
                    self.accel_map.override_count()
                ))
                .weak()
                .small(),
            );
        });
        // "Keys" 页签里的复选框（`preferenceswindow.glade:451-466`）——
        // 偏好键 `keyboard.disable_fallbacks`（`keyboard.py:159-160`）
        let mut no_fallbacks = self
            .prefs
            .bool_or(pigment_engine::prefs::KEY_DISABLE_FALLBACKS, false);
        if ui
            .checkbox(&mut no_fallbacks, FALLBACKS_LABEL)
            .on_hover_text(FALLBACKS_TIP)
            .changed()
        {
            self.prefs.set_bool(
                pigment_engine::prefs::KEY_DISABLE_FALLBACKS,
                no_fallbacks,
            );
        }
        ui.separator();

        // 两列表格（`accelmap.py:110-143`）
        //
        // **技术栈适配**：GTK 的 `Gtk.TreeView` 自己管列宽（可拖、可排序）；
        // egui 没有列模型，这里按"动作列占 62%，其余给按键列"手工分配宽度
        // （`allocate_ui_with_layout` 的定宽分配），否则长描述会把布局挤成
        // 一列单字换行。
        let mut start_capture: Option<String> = None;
        let mut reset_one: Option<String> = None;
        let action_w = (ui.available_width() * 0.62).clamp(260.0, 560.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 40.0).max(80.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // 表头也按同一套列宽分配，否则 `add_sized` 会把文字居中
                    ui.allocate_ui_with_layout(
                        egui::vec2(action_w, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new(COLUMN_ACTION).strong());
                        },
                    );
                    ui.label(egui::RichText::new(COLUMN_ACCEL).strong());
                });
                ui.separator();
                let filter = self.key_editor.filter.clone();
                for row in self.accel_map.editor_rows() {
                    if !self.accel_map.row_matches(row, &filter) {
                        continue;
                    }
                    let accel = self.accel_map.accel(&row.path);
                    ui.horizontal_top(|ui| {
                        // 动作名 + 描述（源码是 `<b>label</b><small>\ndesc</small>`）
                        ui.allocate_ui_with_layout(
                            egui::vec2(action_w, 0.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                let mut label = egui::RichText::new(&row.label).strong();
                                if self.accel_map.is_overridden(&row.path) {
                                    label = label.color(egui::Color32::LIGHT_BLUE);
                                }
                                ui.label(label);
                                if !row.desc.is_empty() {
                                    ui.label(egui::RichText::new(&row.desc).weak().small());
                                }
                                if let Some(other) = self.accel_map.clash_of(&row.path) {
                                    let who = self
                                        .accel_map
                                        .action_label_of_path(other)
                                        .unwrap_or(UNKNOWN_ACTION);
                                    ui.label(
                                        egui::RichText::new(format!("冲突：{who}"))
                                            .color(egui::Color32::LIGHT_RED)
                                            .small(),
                                    );
                                }
                            },
                        );
                        // 加速键单元格 = 可点的按钮（源码里点它进编辑，
                        // `accelmap.py:125-129` 的 `editable=True` + `edited` 信号）
                        ui.allocate_ui_with_layout(
                            egui::vec2(0.0, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let text = accel
                                    .map(|a| a.label())
                                    .unwrap_or_else(|| EMPTY_ACCEL.to_string());
                                if ui
                                    .button(egui::RichText::new(text).strong())
                                    .on_hover_text(format!("{}\n{}", row.label, row.path))
                                    .clicked()
                                {
                                    start_capture = Some(row.path.clone());
                                }
                                // "重置此项"：源码编辑器没有这个动作（只有 Delete），
                                // 这里补的是"丢掉覆盖层、回到默认表"，只在改过时出现。
                                if self.accel_map.is_overridden(&row.path)
                                    && ui
                                        .button(egui::RichText::new("重置").small())
                                        .on_hover_text(format!(
                                            "回到默认值：{}",
                                            self.accel_map
                                                .default_accel(&row.path)
                                                .map(|a| a.label())
                                                .unwrap_or_else(|| "（无）".to_string())
                                        ))
                                        .clicked()
                                {
                                    reset_one = Some(row.path.clone());
                                }
                            },
                        );
                    });
                    ui.separator();
                }
            });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("保存").clicked() {
                self.save_accel_map();
            }
            if ui.button("重新载入").clicked() {
                self.load_accel_map();
            }
            if ui.button("全部重置").clicked() {
                let n = self.accel_map.reset_all();
                self.key_editor.status = Some(format!("已重置 {n} 条覆盖"));
                self.save_accel_map();
            }
            if let Some(status) = &self.key_editor.status {
                ui.label(egui::RichText::new(status).weak().small());
            }
        });

        if let Some(path) = start_capture {
            self.key_editor.capture = Some(Capture::new(&self.accel_map, &path));
        }
        if let Some(path) = reset_one {
            self.accel_map.reset_one(&path);
            self.save_accel_map();
        }
    }

    /// 按键捕获对话框 —— `accelmap.py:352-568`。
    ///
    /// 用 `egui::Modal`（遮罩 + 挡住背后输入）对应 GTK 的 `set_modal(True)`
    /// （`accelmap.py:361`）。
    fn capture_modal(&mut self, ctx: &egui::Context) {
        if self.key_editor.capture.is_none() {
            return;
        }
        // 先取按键事件（Return/Escape/BackSpace 三个"对话框默认键"优先，
        // `accelmap.py:466-480` 的 `_USE_NORMAL_DIALOG_KEYS` 分支）
        let events: Vec<(egui::Key, Mods, bool)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } => Some((*key, Mods::from_egui(modifiers), *repeat)),
                    _ => None,
                })
                .collect()
        });

        let mut action: Option<CaptureAction> = None;
        let mut attempt: Option<Accel> = None;
        let mut bad_key: Option<egui::Key> = None;
        for (key, mods, repeat) in events {
            if repeat {
                continue;
            }
            // `accelmap.py:469` 的 `if event.is_modifier: return False`
            if actions::is_modifier_key(key) {
                continue;
            }
            // `accelmap.py:471-480`：三个对话框默认键（它们因此无法被绑定 ——
            // 源码 `_USE_NORMAL_DIALOG_KEYS = True` 就是这个代价）
            match key {
                egui::Key::Enter => {
                    action = Some(CaptureAction::Ok);
                    break;
                }
                egui::Key::Escape => {
                    action = Some(CaptureAction::Cancel);
                    break;
                }
                egui::Key::Backspace => {
                    action = Some(CaptureAction::Delete);
                    break;
                }
                _ => {}
            }
            match actions::capture_accel(key, mods) {
                Some((name, accel_mods)) => attempt = Some(Accel::new(name, accel_mods.to_accel())),
                None => bad_key = Some(key),
            }
            break;
        }

        // 先把要显示的东西全部取出来（之后才能可变地动 `self`）
        let (title, path, action_label, key_text, hint) = {
            let capture = self.key_editor.capture.as_mut().expect("上面判过");
            if let Some(accel) = attempt {
                capture.set_attempt(&self.accel_map, accel);
            }
            if let Some(key) = bad_key {
                capture.hint = format!(
                    "这个键没有 GDK 键值名（egui 的浏览器键/媒体键），记不下来：{key:?}"
                );
            }
            (
                EDIT_TITLE.replace("{action}", &capture.action_label),
                capture.path.clone(),
                capture.action_label.clone(),
                capture.key_text(&self.accel_map),
                capture.hint.clone(),
            )
        };

        egui::Modal::new(egui::Id::unique("pigment_key_capture")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(&title);
            ui.add_space(6.0);
            egui::Grid::new("pigment_key_capture_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(LABEL_ACTION);
                    ui.label(&action_label);
                    ui.end_row();
                    // `_SHOW_ACCEL_PATH = True`（`accelmap.py:59,399-414`）
                    ui.label(LABEL_PATH);
                    ui.label(egui::RichText::new(&path).weak());
                    ui.end_row();
                    ui.label(LABEL_KEY);
                    ui.label(egui::RichText::new(&key_text).strong());
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.label(egui::RichText::new(&hint).small());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(BUTTON_DELETE).clicked() {
                    action = Some(CaptureAction::Delete);
                }
                if ui.button(BUTTON_CANCEL).clicked() {
                    action = Some(CaptureAction::Cancel);
                }
                if ui.button(BUTTON_OK).clicked() {
                    action = Some(CaptureAction::Ok);
                }
            });
        });

        match action {
            None => {}
            Some(CaptureAction::Cancel) => {
                // `accelmap.py:553-568`：取消 = 什么都不改
                self.key_editor.capture = None;
            }
            Some(CaptureAction::Delete) => {
                // `accelmap.py:556-562`：REJECT = 删掉这条绑定
                self.accel_map.clear(&path);
                self.key_editor.capture = None;
                self.key_editor.status = Some(format!("已删除 {path} 的按键"));
                self.save_accel_map();
            }
            Some(CaptureAction::Ok) => {
                // `accelmap.py:563-566`：OK = 写新键（先删同键的其它条目）
                if let Some(accel) = self
                    .key_editor
                    .capture
                    .as_ref()
                    .and_then(|c| c.attempt.clone())
                {
                    let deleted = self.accel_map.set_accel(&path, accel.clone());
                    let mut status = format!("{path} → {}", accel.to_gtk_name());
                    if !deleted.is_empty() {
                        status.push_str(&format!("（替换了 {} 条）", deleted.len()));
                    }
                    self.key_editor.status = Some(status);
                    self.save_accel_map();
                }
                self.key_editor.capture = None;
            }
        }
    }
}

/// 捕获对话框里的三个按钮 / 三个快捷键。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureAction {
    Delete,
    Cancel,
    Ok,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::actions::{self as a, Bound, Mods};

    const CTRL: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: false,
    };
    const SHIFT: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: true,
    };

    /// 一份不碰真实 `accelmap.conf` 的映射。
    fn fresh(tag: &str) -> AccelMap {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-accelmap-{}-{tag}.conf", std::process::id()));
        let _ = std::fs::remove_file(&p);
        AccelMap::new(p)
    }

    /// 按 path 取默认表的一行。
    fn row<'a>(map: &'a AccelMap, path: &str) -> &'a Row {
        map.rows()
            .iter()
            .find(|r| r.path == path)
            .unwrap_or_else(|| panic!("默认表里没有 {path}"))
    }

    // ---- 默认表 ----

    #[test]
    fn defaults_cover_the_action_table_and_the_slot_keys() {
        let map = fresh("rows");
        // 219 个 XML 动作 + 20 条槽位键
        assert_eq!(map.rows().len(), 219 + 20);
        // 编辑器可见：170（`accelmap.py` 的过滤）+ 20 条槽位键
        assert_eq!(map.editor_rows().len(), 190);
        // 默认绑定数与 `resources.xml` 一致（105 条）+ 20 条槽位键
        let with_accel = map.rows().iter().filter(|r| r.default.is_some()).count();
        assert_eq!(with_accel, 105 + 20);
        // 逐条与 actions.rs 的加速键表对齐（含修饰键）
        for action in a::ACTIONS {
            let path = action.path();
            let r = row(&map, &path);
            assert_eq!(r.action, action.id);
            assert_eq!(
                r.default.as_ref().map(|x| x.to_gtk_name()),
                action.accel.map(|acc| to_engine_accel(acc).to_gtk_name()),
                "{path} 的默认加速键与 actions.rs 不一致"
            );
            assert_eq!(r.listed, action.is_listed_in_accel_editor());
        }
        // 槽位键的默认值（`document.py:478-485`）
        let restore = row(&map, "<Actions>/DocumentActions/Context03");
        assert_eq!(restore.default.as_ref().unwrap().to_gtk_name(), "3");
        assert_eq!(restore.label, "Restore Brush 3");
        let store = row(&map, "<Actions>/DocumentActions/Context03s");
        assert_eq!(store.default.as_ref().unwrap().to_gtk_name(), "<Primary>3");
        assert_eq!(store.label, "Save to Brush 3");
    }

    #[test]
    fn editor_rows_are_sorted_by_the_source_sort_key() {
        let map = fresh("sort");
        let rows = map.editor_rows();
        // `accelmap.py:64,140-143`：按 "label\ndesc" 升序
        let mut sorted = rows.clone();
        sorted.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        assert_eq!(
            rows.iter().map(|r| &r.path).collect::<Vec<_>>(),
            sorted.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
        // 第一条不该是 `Undo`（它没有 label/tooltip → 不在编辑器里）
        assert!(!rows.iter().any(|r| r.action == "Undo"));
        assert!(rows.iter().any(|r| r.action == "Save"));
    }

    #[test]
    fn filter_follows_the_accel_editor_semantics() {
        let map = fresh("filter");
        let save = row(&map, "<Actions>/FileActions/Save");
        // 动作名、描述、加速键显示名都参与匹配（`accelmap.py:67,305-344`）
        assert!(map.row_matches(save, "save"));
        assert!(map.row_matches(save, "Ctrl+S"));
        assert!(map.row_matches(save, "ctrl"));
        assert!(map.row_matches(save, "work"));
        assert!(!map.row_matches(save, "zzz"));
        assert!(map.row_matches(save, ""), "空过滤 = 全过");
    }

    // ---- 覆盖层 / 查表 ----

    #[test]
    fn lookup_hits_the_default_table_first() {
        let map = fresh("lookup-default");
        assert_eq!(
            map.lookup(egui::Key::Z, CTRL, true),
            Some(Bound::Action("Undo"))
        );
        assert_eq!(
            map.lookup(egui::Key::S, CTRL, true),
            Some(Bound::Action("Save"))
        );
        // 没有绑定的组合
        assert_eq!(map.lookup(egui::Key::Z, SHIFT, true), None);
    }

    #[test]
    fn lookup_uses_the_override_and_drops_the_old_key() {
        let mut map = fresh("lookup-override");
        let path = "<Actions>/DocumentActions/Undo";
        // F13 在默认表里没有任何动作占用
        let new = Accel::new("F13", conf::Mods::NONE);
        let deleted = map.set_accel(path, new);
        assert!(deleted.is_empty(), "F13 没被占用：{deleted:?}");
        // 新键命中
        assert_eq!(
            map.lookup(egui::Key::F13, Mods::NONE, true),
            Some(Bound::Action("Undo"))
        );
        // 旧键**不再**命中（默认表的 Ctrl+Z 被覆盖层压住）
        assert_eq!(map.lookup(egui::Key::Z, CTRL, true), None);
        assert!(map.is_overridden(path));
    }

    #[test]
    fn setting_a_key_deletes_the_clashing_entry() {
        // `accelmap.py:571-597`：设键前先删同键的其它条目
        let mut map = fresh("clash");
        let save = "<Actions>/FileActions/Save";
        let undo = "<Actions>/DocumentActions/Undo";
        let deleted = map.set_accel(save, Accel::new("z", conf::Mods::new(true, false, false)));
        assert_eq!(deleted, vec![undo.to_string()]);
        // Undo 被清空，Save 拿到 Ctrl+Z
        assert_eq!(map.accel(undo), None);
        assert_eq!(map.clash_of(undo), None, "清空之后不再冲突");
        assert_eq!(
            map.lookup(egui::Key::Z, CTRL, true),
            Some(Bound::Action("Save"))
        );
    }

    #[test]
    fn clear_and_reset_one_and_reset_all() {
        let mut map = fresh("reset");
        let save = "<Actions>/FileActions/Save";
        let default = map.default_accel(save).cloned().unwrap();
        // 清空：没有键了
        map.clear(save);
        assert_eq!(map.accel(save), None);
        assert_eq!(map.lookup(egui::Key::S, CTRL, true), None);
        // 重置此项：回到默认表
        assert!(map.reset_one(save));
        assert_eq!(map.accel(save), Some(&default));
        assert_eq!(
            map.lookup(egui::Key::S, CTRL, true),
            Some(Bound::Action("Save"))
        );
        // 全部重置：覆盖层清空（F13/F14 都是空闲键，不会连累别的动作）
        map.set_accel(save, Accel::new("F13", conf::Mods::NONE));
        map.set_accel("<Actions>/DocumentActions/Redo", Accel::new("F14", conf::Mods::NONE));
        assert_eq!(map.override_count(), 2);
        assert_eq!(map.reset_all(), 2);
        assert_eq!(map.override_count(), 0);
        assert_eq!(
            map.lookup(egui::Key::S, CTRL, true),
            Some(Bound::Action("Save"))
        );
    }

    #[test]
    fn conflicts_are_reported_with_the_other_path() {
        let mut map = fresh("conflicts");
        // 手工制造冲突（正常编辑路径会先删掉对方，见上一个测试）
        map.set_accel(
            "<Actions>/FileActions/SaveAs",
            Accel::new("s", conf::Mods::new(true, false, false)),
        );
        // SaveAs 抢了 Save 的 Ctrl+S，Save 的条目被清空 → 没有冲突
        assert_eq!(map.clash_of("<Actions>/FileActions/SaveAs"), None);
        // 直接问"设成这个键会撞谁"（捕获对话框的实时提示）
        assert_eq!(
            map.clash_of_accel(
                "<Actions>/FileActions/Export",
                &Accel::new("s", conf::Mods::new(true, false, false))
            ),
            Some("<Actions>/FileActions/SaveAs".to_string())
        );
        assert_eq!(
            map.clash_of_accel(
                "<Actions>/FileActions/Export",
                &Accel::new("F7", conf::Mods::NONE)
            ),
            Some("<Actions>/FileActions/SaveScrap".to_string())
        );
        assert_eq!(
            map.clash_of_accel(
                "<Actions>/FileActions/Export",
                &Accel::new("Pause", conf::Mods::NONE)
            ),
            None
        );
    }

    #[test]
    fn fallbacks_can_be_disabled() {
        // `keyboard.py:140,159-160` 的 `fallbacks_disabled()`
        let map = fresh("fallback");
        // `BackSpace` 只在硬编码兜底表里（`document.py:525` 的 ClearLayer）
        assert_eq!(
            map.lookup(egui::Key::Backspace, Mods::NONE, true),
            Some(Bound::ExtraAction("ClearLayer"))
        );
        assert_eq!(map.lookup(egui::Key::Backspace, Mods::NONE, false), None);
        // 菜单级绑定不受影响
        assert_eq!(
            map.lookup(egui::Key::S, CTRL, false),
            Some(Bound::Action("Save"))
        );
    }

    #[test]
    fn unknown_paths_in_the_file_are_kept_but_never_dispatch() {
        // `keyboard.py:89`：路径对不上任何已注册动作 → 只记 warning
        let mut map = fresh("unknown");
        map.overrides
            .insert("<Actions>/GoneActions/Gone".to_string(), Some(Accel::new("F9", conf::Mods::NONE)));
        map.recompute_conflicts();
        assert!(map.is_known_path("<Actions>/GoneActions/Gone") == false);
        // 落盘时保留（不然用户的文件会被悄悄改小）
        let entries = map.entries_for_save();
        assert!(entries
            .iter()
            .any(|e| e.path == "<Actions>/GoneActions/Gone" && e.accel.is_some()));
        // 但它不参与分发
        assert_ne!(
            map.lookup(egui::Key::F9, Mods::NONE, true),
            Some(Bound::Action("Gone"))
        );
    }

    // ---- 落盘 / 启动加载 ----

    #[test]
    fn save_writes_the_whole_table_and_reload_keeps_overrides() {
        let mut map = fresh("save");
        // F13 不被任何默认动作占用 → 不会顺带删掉别人（那会多出一条注释行）
        map.set_accel("<Actions>/DocumentActions/Undo", Accel::new("F13", conf::Mods::NONE));
        map.save().unwrap();
        let text = std::fs::read_to_string(map.conf_path()).unwrap();
        assert!(text.starts_with("; mypaint GtkAccelMap rc-file"));
        // 整表快照：105 + 20 条默认 + 覆盖上去的 Undo
        assert!(text.contains(r#"(gtk_accel_path "<Actions>/DocumentActions/Undo" "F13")"#));
        assert!(text.contains(r#"(gtk_accel_path "<Actions>/FileActions/Save" "<Primary>s")"#));
        // 重新加载：覆盖层还在
        let mut map2 = AccelMap::new(map.conf_path().to_path_buf());
        let n = map2.load().unwrap();
        assert_eq!(n, 125, "整表快照 = 105 + 20 条");
        assert_eq!(
            map2.lookup(egui::Key::F13, Mods::NONE, true),
            Some(Bound::Action("Undo"))
        );
        assert_eq!(map2.lookup(egui::Key::Z, CTRL, true), None);
        let _ = std::fs::remove_file(map.conf_path());
    }

    #[test]
    fn a_missing_config_file_means_defaults_only() {
        let mut map = fresh("missing");
        assert_eq!(map.load().unwrap(), 0);
        assert_eq!(map.override_count(), 0);
        assert_eq!(
            map.lookup(egui::Key::Z, CTRL, true),
            Some(Bound::Action("Undo"))
        );
    }

    #[test]
    fn cleared_bindings_do_not_survive_a_restart() {
        // GTK 把"清空的条目"写成注释行；注释行不进解析结果 → 默认绑定回来
        let mut map = fresh("cleared");
        map.clear("<Actions>/FileActions/Save");
        assert_eq!(map.lookup(egui::Key::S, CTRL, true), None);
        map.save().unwrap();
        let text = std::fs::read_to_string(map.conf_path()).unwrap();
        assert!(
            text.contains(r#"; (gtk_accel_path "<Actions>/FileActions/Save" "")"#),
            "清空的条目应写成注释行"
        );
        let mut map2 = AccelMap::new(map.conf_path().to_path_buf());
        map2.load().unwrap();
        assert_eq!(
            map2.lookup(egui::Key::S, CTRL, true),
            Some(Bound::Action("Save")),
            "重启之后默认绑定回来了（GTK 的行为，见模块文档）"
        );
        let _ = std::fs::remove_file(map.conf_path());
    }

    // ---- 捕获流程 ----

    #[test]
    fn capture_round_trips_for_every_mapped_key() {
        // 捕获 → 查表必须回到同一个 (键位, 修饰键)：这是"改完立刻能按"的门禁
        let keys = [
            egui::Key::A,
            egui::Key::Z,
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num6,
            egui::Key::F1,
            egui::Key::F35,
            egui::Key::Comma,
            egui::Key::Period,
            egui::Key::Slash,
            egui::Key::Semicolon,
            egui::Key::Quote,
            egui::Key::Backtick,
            egui::Key::Minus,
            egui::Key::Equals,
            egui::Key::Plus,
            egui::Key::Pipe,
            egui::Key::Backslash,
            egui::Key::OpenBracket,
            egui::Key::CloseBracket,
            egui::Key::Space,
            egui::Key::Enter,
            egui::Key::Escape,
            egui::Key::Tab,
            egui::Key::Backspace,
            egui::Key::Delete,
            egui::Key::Home,
            egui::Key::End,
            egui::Key::PageUp,
            egui::Key::PageDown,
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
        ];
        let mods_cases = [
            Mods::NONE,
            SHIFT,
            CTRL,
            Mods {
                ctrl: false,
                alt: true,
                shift: false,
            },
            Mods {
                ctrl: true,
                alt: false,
                shift: true,
            },
        ];
        for key in keys {
            let mut capturable = 0;
            for mods in mods_cases {
                // 有些 (键位, Shift) 组合在物理上不存在：`|` 只能是 Shift+\、
                // `+`（主键盘）只能是 Shift+= —— 这类组合直接跳过。
                let Some((name, captured)) = actions::capture_accel(key, mods) else {
                    continue;
                };
                capturable += 1;
                assert_eq!(
                    actions::egui_binding_for(name, captured),
                    Some((key, mods)),
                    "捕获 {key:?} + {mods:?} 得到 {name:?}，但查表回不去"
                );
            }
            assert!(capturable > 0, "{key:?} 一个组合都捕获不了，映射表缺项");
        }
    }

    #[test]
    fn capture_produces_gtk_style_names_for_shifted_symbols() {
        // GDK 的"Shift 被消费成字符"：Shift+1 → `exclam`（无 Shift），
        // Shift+, → `less`（无 Shift），Shift+a → `a` + Shift（字母不消费）
        let (name, mods) = actions::capture_accel(egui::Key::Num1, SHIFT).unwrap();
        assert_eq!((name, mods), ("exclam", Mods::NONE));
        let (name, mods) = actions::capture_accel(egui::Key::Comma, SHIFT).unwrap();
        assert_eq!((name, mods), ("less", Mods::NONE));
        let (name, mods) = actions::capture_accel(egui::Key::A, SHIFT).unwrap();
        assert_eq!((name, mods), ("a", SHIFT));
        // 小键盘与主键盘在 egui 里合并：不带 Shift 的 Plus 只能是 KP_Add
        let (name, _) = actions::capture_accel(egui::Key::Plus, Mods::NONE).unwrap();
        assert_eq!(name, "KP_Add");
        let (name, _) = actions::capture_accel(egui::Key::Plus, SHIFT).unwrap();
        assert_eq!(name, "plus");
    }

    #[test]
    fn modifier_keys_and_unknown_keys_are_not_capturable() {
        // `accelmap.py:469`：修饰键本身不参与捕获
        for key in [
            egui::Key::ControlLeft,
            egui::Key::ShiftRight,
            egui::Key::AltLeft,
            egui::Key::SuperLeft,
        ] {
            assert!(actions::is_modifier_key(key), "{key:?} 应算修饰键");
        }
        assert!(!actions::is_modifier_key(egui::Key::A));
        // 浏览器/媒体键没有 GDK 键值名 → 捕获要说"记不下来"而不是瞎记
        assert_eq!(actions::capture_accel(egui::Key::BrowserBack, Mods::NONE), None);
        assert_eq!(actions::capture_accel(egui::Key::MediaPlayPause, Mods::NONE), None);
    }

    #[test]
    fn capture_state_reports_clash_replace_and_change() {
        let map = fresh("capture-state");
        let export = "<Actions>/FileActions/Export";
        let mut capture = Capture::new(&map, export);
        // 显示名就是 `resources.xml` 的 label 原文（Export 带省略号）
        assert_eq!(capture.action_label, "Export…");
        assert_eq!(capture.key_text(&map), "Shift+Ctrl+E", "还没按 = 显示当前键");
        assert_eq!(capture.hint, HINT_PRESS_KEYS);

        // 按下一个没被占用的键 → "(changed)" + 标准提示
        capture.set_attempt(&map, Accel::new("F13", conf::Mods::NONE));
        assert_eq!(capture.hint, HINT_PRESS_KEYS);
        assert_eq!(capture.key_text(&map), "F13 (changed)");

        // 按下一个被占用的键 → "(replace)" + 警告（`accelmap.py:532-544`）
        capture.set_attempt(&map, Accel::new("s", conf::Mods::new(true, false, false)));
        assert_eq!(capture.key_text(&map), "Ctrl+S (replace)");
        assert!(capture.hint.contains("already in use for 'Save'"), "{}", capture.hint);

        // 按回它自己的键 → 没有后缀（`accelmap.py:527-530`）
        // Export 的默认键是 `<Primary><Shift>e`（Ctrl+Shift+E）
        capture.set_attempt(
            &map,
            Accel::new("e", conf::Mods::new(true, true, false)),
        );
        assert_eq!(capture.key_text(&map), "Shift+Ctrl+E");
        assert_eq!(capture.hint, HINT_PRESS_KEYS);
    }

    #[test]
    fn capture_ok_writes_through_and_clears_the_old_binding() {
        // OK 的完整语义（`accelmap.py:563-566` → `_set_accelmap_entry`）
        let mut map = fresh("capture-ok");
        let save = "<Actions>/FileActions/Save";
        let mut capture = Capture::new(&map, save);
        capture.set_attempt(&map, Accel::new("j", conf::Mods::NONE));
        let deleted = map.set_accel(save, capture.attempt.clone().unwrap());
        // 裸 `j` 在默认表里是 FlipSequenceMode
        assert_eq!(deleted, vec!["<Actions>/ModeStackActions/FlipSequenceMode".to_string()]);
        assert_eq!(
            map.lookup(egui::Key::J, Mods::NONE, true),
            Some(Bound::Action("Save"))
        );
        assert_eq!(
            map.lookup(egui::Key::S, CTRL, true),
            None,
            "Save 原来的 Ctrl+S 已让出"
        );
    }
}
