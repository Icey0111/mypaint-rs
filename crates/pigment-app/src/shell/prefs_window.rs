//! 偏好窗口 —— `gui/preferenceswindow.py` + `gui/preferenceswindow.glade`（B6-3）。
//!
//! # 为什么是 `egui::Window` 而不是停靠面板
//!
//! 源码里这是 `windowing.Dialog`（`gui/preferenceswindow.py:31` 的基类，
//! `gui/windowing.py:29-43`）—— 一个**独立窗口**：`transient_for=app.drawWindow`
//! 且 `destroy_with_parent=True`（`:43-44`），由 `toggle_window_cb`
//! （`gui/drawwindow.py:364-388`）经 `Application.get_subwindow("PreferencesWindow")`
//! （`gui/application.py:428` / `:734-746`）按需构造并 `show_all()`/`hide()`。
//! 它**不在**工作区的停靠系统里（那是 `MyPaintWorkspace` 的 tool widget，
//! `drawwindow.py:374-375` 的另一条分支）。所以这里用 `egui::Window`，
//! 与 B6-2 的加速键编辑器同一种壳。
//!
//! # 三个方向的语义（逐条标源码行号）
//!
//! | 方向 | 源码 | 本实现 |
//! |---|---|---|
//! | 控件改动 | 回调里直接写 `app.preferences[key] = v`，部分还立刻 `apply_settings()`（`preferenceswindow.py:264-418`） | 同样是"改动即写 [`PigmentApp::prefs`]"（立即模式：每帧从 `Prefs` 读当前值） |
//! | `REVERT`（"Revert to Saved"） | `app.load_settings()` + `app.apply_settings()`（`:133-135`）；**不关窗** | `settings_on_revert()`（= `Prefs::reload`，`application.py:524-539`）+ `apply_settings()` |
//! | `ACCEPT`（"OK"） | `app.save_settings()` + `app.apply_settings()` + `self.hide()`（`:129-132`） | `save_gui_config()`（含 `settings_on_accept()`）+ `apply_settings()` + 关窗 |
//! | 关窗（X） | `delete-event` → `hide_on_delete()`（`windowing.py:43`）：**只藏窗口，不存盘也不回退** | 置 `open = false`，别的什么都不做 |
//!
//! 两点必须说清楚，因为**任务书里的假设与源码不同**：
//!
//! 1. **`REVERT` 不是"恢复到打开窗口时的快照"**，而是"**重读磁盘上的
//!    `settings.json`**"（`application.py:524-539`：先 `clear()` 再用默认表 +
//!    用户文件重建）。打开窗口这个动作本身在源码里不拍快照 —— 窗口是启动时
//!    就构造好的（`apply_settings()` → `preferences_window.update_ui()`，
//!    `application.py:522`、`:725-727`）。所以本实现用 `Prefs::reload()`，
//!    保守侧差异：用户**没**通过对话框改过、但本会话内被别处改过的键（如
//!    `colors.*`）也会被磁盘值覆盖 —— 源码一样（整张 dict 重建）。
//! 2. **关窗（X）不回退改动**。`hide_on_delete()` 只是隐藏；控件回调早已把
//!    改动写进 `app.preferences`，而退出程序时 `quit_cb`（`drawwindow.py:688-690`）
//!    会 `save_gui_config()` → `save_settings()` 把整张 dict 落盘 ——
//!    也就是说"关窗不点 OK"的改动**会在下次退出时照样存下来**。这是源码行为，
//!    本实现照搬（不改 `on_exit` 的语义），并在记账里写明。
//!
//! # 页签与控件
//!
//! glade 的 `prefs_notebook`（`:88`）有 **8 页**，顺序与标题见 [`TABS`]。
//! 每页的控件列在 [`CONTROLS`] 里（顺序 = glade 的 attach 顺序），每条都带：
//! 控件 id、glade 原文标签、绑定的偏好键、**是否已接线**、未接线的原因。
//!
//! 判定标准只有一条：**这个键在当前构建里有没有消费方**。
//! 已接线的偏好键有五条（对应 8 个控件行 —— Color 页三个单选共用
//! `colors.wheels.type`，Keys 页的加速键编辑器没有偏好键）：
//!
//! - `saving.default_format`（`Prefs`/`save_extension`，B6-1）
//! - `ui.hide_cursor_while_painting`（`ModePrefs`，B6-1）
//! - `keyboard.disable_fallbacks`（`AccelMap::lookup` 的兜底开关，B6-2）
//! - `colors.wheels.type`（`ColorManager::set_wheel_type`，B4-4b）
//! - `input.button_mapping`（按钮映射编辑器 + 画布指针分派）
//!
//! 其余控件一律**禁用 + 悬停说明"该子系统未移植"**，绝不伪造行为
//! （与本项目 `shell/color_panel.rs` 对未移植页签的处置一致）。
//!
//! # 记账（本块没做的事）
//!
//! - **glade 的像素级布局**：`border_width=12`、`row_spacing=12`、
//!   `margin_left=12`、`GtkAlignment` 的 padding、`GtkFrame` 的
//!   `label_xalign=0` 等全部按 egui 的默认间距近似，没有逐个搬数值。
//! - **`GtkNotebook` 的交互**：页签可滚动（`scrollable=True`，`:92`）、
//!   中键弹页签列表（`enable_popup=True`，`:93`）没有对应物；页签条用
//!   `selectable_label` 一行画完。
//! - **模态行为**：源码 `windowing.Dialog` 明确**不是**模态（`windowing.py:32-33`
//!   "they ... are not modal"），所以这里也不挡主窗口的全局快捷键；窗口开着时
//!   按 `Ctrl+S` 之类照样会落到画布上（源码亦然）。
//! - **窗口几何持久化**：源码 `Dialog` 只在自己被 `hide()` 时记位置
//!   （`windowing.py:89-91`，且只在 `SubWindow` 那支生效），**不写偏好**；
//!   `workspace.layout`（`application.py:626-627`）是工作区的停靠布局，
//!   与本对话框无关。egui 侧既没有这一层，也不写 `workspace.layout`（B6-1 记账）。
//! - **Esc 关窗**：`Gtk.Dialog` 的 Esc 走 `GTK_RESPONSE_DELETE_EVENT` →
//!   隐藏（等价于点 X，**不回退**）。egui 的 `Window` 没有这个默认键，这里没做
//!   —— 否则要和捕获对话框的 Esc（`accelmap.py:471-480` 的取消）抢事件。
//! - **页面外壳**：`Devices` 页已接线为「数位板工具策略」（`input.tablet_usage`：
//!   Any / Ignore / Non-painting / Navigation only）；winit 0.31 不暴露设备名，
//!   所以策略作用于所有数位板工具，不按设备分组。`Pressure` 页的
//!   `CurveWidget` 是自绘曲线（已接线 `input.global_pressure_mapping`）；
//!   `height_request`）；`Buttons` 页的 `ButtonMappingEditor` 已移植
//!   （[`crate::shell::buttonmap_editor`]），列表/Add/Remove/动作下拉/
//!   鼠标捕获对话框都在。
//! - **`Compatibility` 页**：整页来自 `gui/compatibility.py` 的
//!   `CompatibilityPreferences`（`:272-409`），依赖兼容模式/色彩管理子系统，
//!   全部禁用。
//! - **语言下拉**：`lib/config.py` 的 `supported_locales` + `lib/localecodes.py`
//!   的语言名表没有移植，下拉里没有选项（禁用且显示"未移植"）。
//! - **状态行**：源码存盘失败会抛异常；这里在按钮行右侧显示一条状态文字
//!   （本项目补的，不改任何语义）。

use eframe::egui;

use pigment_engine::prefs::Prefs;

use crate::app::PigmentApp;

// ---------------------------------------------------------------------------
// 与源码逐字相同的界面文本
// ---------------------------------------------------------------------------

/// 窗口标题 —— `gui/preferenceswindow.py:42` 的 `_("Preferences")`。
pub(crate) const TITLE: &str = "Preferences";
/// 按钮：回退到已保存 —— `preferenceswindow.py:46-48` 的
/// `Gtk.STOCK_REVERT_TO_SAVED`（GTK3 的英文标签是 `_Revert to Saved`，
/// 下划线是助记符，egui 没有助记符所以去掉）。
pub(crate) const BUTTON_REVERT: &str = "Revert to Saved";
/// 按钮：确定 —— `preferenceswindow.py:49-50` 的 `Gtk.STOCK_OK`（`_OK`）。
pub(crate) const BUTTON_OK: &str = "OK";
/// 未接线控件的统一措辞（与 `shell/color_panel.rs` 对未移植页签的写法一致）。
pub(crate) const NOT_PORTED: &str = "该子系统未移植（见交付记账）";

/// 压力曲线 x 轴标签 —— glade `:145` 的 `Input Pressure`。
const AXIS_INPUT_PRESSURE: &str = "Input Pressure";
/// 压力曲线 y 轴标签 —— glade `:159` 的 `Brush Pressure`
/// （glade 里 `angle = 90` 竖排；egui 侧按横排画，见 [`curve_placeholder`]）。
const AXIS_BRUSH_PRESSURE: &str = "Brush Pressure";

// ---------------------------------------------------------------------------
// 页签表（glade `prefs_notebook`，`:88`）
// ---------------------------------------------------------------------------

/// 笔记本的一页 —— glade 里一个 `<child type="tab">`。
pub(crate) struct TabDef {
    /// 页签文字（glade 的 `GtkLabel` 的 `label` 属性原文）。
    pub(crate) title: &'static str,
    /// 页内容的 `GtkFrame` 粗体标题（`<child type="label">` 里的
    /// `<b>…</b>`，去掉标记）；`None` = 这一页没有外框
    /// （`Load & Save` 与 `View` 是 `GtkGrid`，标题在页面内部）。
    pub(crate) frame_heading: Option<&'static str>,
    /// glade 行号（`<child type="tab">` 所在行）。
    ///
    /// 元数据：运行时只给测试与文档读（与 `color_panel::ColorPageDef::action`
    /// 同处置），故标 `dead_code`。
    #[allow(dead_code)]
    pub(crate) source: &'static str,
}

/// 八个页签，**顺序 = glade 的 `<child type="tab">` 顺序**
/// （每个 `packing/position` 也印证了同一顺序）。
pub(crate) static TABS: [TabDef; 8] = [
    // glade:257-266（`label15`），外框 `frame7` 的标签在 `:248-254`
    TabDef {
        title: "Pressure",
        frame_heading: Some("Global Pressure Mapping"),
        source: "glade:257-266",
    },
    // glade:333-343（`label32`），外框 `frame3` 的标签在 `:321-327`
    TabDef {
        title: "Devices",
        frame_heading: Some("Device Usage"),
        source: "glade:333-343",
    },
    // glade:410-420（`label16`），外框 `frame6` 的标签在 `:398-404`
    TabDef {
        title: "Buttons",
        frame_heading: Some("Pointer Button Mappings"),
        source: "glade:410-420",
    },
    // glade:485-495（`label29`），外框 `frame2` 的标签在 `:473-479`
    TabDef {
        title: "Keys",
        frame_heading: Some("Keyboard Mappings"),
        source: "glade:485-495",
    },
    // glade:782-793（`label17`，带 `max_width_chars=60`）；内容是 `GtkGrid`
    TabDef {
        title: "Load & Save",
        frame_heading: None,
        source: "glade:782-793",
    },
    // glade:1279-1289（`label18`）；内容是 `GtkGrid`
    TabDef {
        title: "View",
        frame_heading: None,
        source: "glade:1279-1289",
    },
    // glade:1421-1431（`label14`），外框 `frame5` 的标签在 `:1409-1415`
    TabDef {
        title: "Color",
        frame_heading: Some("Color Wheel"),
        source: "glade:1421-1431",
    },
    // glade:1828-1838（`compat_label`），外框 `compat_frame` 的标签在 `:1813-1822`
    TabDef {
        title: "Compatibility",
        frame_heading: Some("Compatibility settings"),
        source: "glade:1828-1838",
    },
];

/// `Keys` 页在 [`TABS`] 里的下标（glade `:485`）。
///
/// 这一页要特殊处理：B6-2 的加速键编辑器自带 `ScrollArea`，不能套在
/// 页面的外层滚动区里（见 [`PigmentApp::prefs_page_body`]）。
pub(crate) const TAB_KEYS: usize = 3;

// ---------------------------------------------------------------------------
// 控件表（顺序 = glade 的 attach 顺序）
// ---------------------------------------------------------------------------

/// 控件种类（决定禁用态的绘制形态）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// 页面内的粗体小节标题（glade 里是 `<b>…</b>` 的 `GtkLabel`）。
    Header,
    /// 静态说明文字（glade 的普通 `GtkLabel`）。
    Note,
    /// 控件左侧的行标签（glade 里单独一个 `GtkLabel`，如 `Default file format:`）。
    RowLabel,
    /// `GtkCheckButton`（标签在复选框上）。
    Check,
    /// `GtkSwitch`（glade 里旁边另有一个行标签）。
    Switch,
    /// `GtkRadioButton`。
    Radio,
    /// `GtkComboBox`（选项见 [`options_for`]）。
    Combo,
    /// `GtkEntry`。
    Entry,
    /// `GtkSpinButton`（旁边另有一个行标签；`label` 是控件**后面**的单位文字）。
    Spin,
    /// `CurveWidget`（压力映射曲线，`gui/curve.py`）—— 自绘控件。
    Curve,
    /// `MyPaintDeviceSettingsEditor`（`gui/device.py`）—— 自绘控件。
    DeviceEditor,
    /// `ButtonMappingEditor`（`gui/buttonmap.py`）—— 自绘控件。
    ButtonEditor,
    /// `AccelMapEditor`（`gui/accelmap.py`，B6-2 已移植）—— 嵌入本页。
    AccelEditor,
}

/// 一个控件（或一行静态文字）与偏好键的对应关系。
pub(crate) struct ControlDef {
    /// glade 里的 widget id（静态文字行用 glade 的 label id）。
    pub(crate) id: &'static str,
    /// 在第几页（[`TABS`] 的下标）。
    pub(crate) page: usize,
    pub(crate) kind: Kind,
    /// 界面文字（glade 的 `label` / `title` 原文）。
    pub(crate) label: &'static str,
    /// 悬停提示（glade 的 `tooltip_text`/`tooltip_markup` 原文，去掉标记）。
    pub(crate) tip: Option<&'static str>,
    /// 绑定的偏好键；`None` = 这一行不是偏好控件。
    pub(crate) key: Option<&'static str>,
    /// 该键在当前构建里是否**已接线**（有消费方）。
    pub(crate) wired: bool,
    /// 未接线的原因（只在 `wired == false` 时显示）。
    pub(crate) reason: &'static str,
    /// [`Kind::Radio`] 写进偏好键的取值。
    pub(crate) value: Option<&'static str>,
    /// [`Kind::Spin`] 的取值范围（`Gtk.Adjustment` 的 lower/upper）。
    pub(crate) range: Option<(f64, f64)>,
    /// 复选框/开关在偏好缺失时的回落值（源码的 `.get(key, default)`）。
    pub(crate) default_bool: bool,
    /// 数值控件在偏好缺失时的回落值。
    pub(crate) default_num: f64,
    /// 源码行号（`glade:` 或 `py:`）。
    ///
    /// 元数据：运行时只给测试与文档读，故标 `dead_code`。
    #[allow(dead_code)]
    pub(crate) source: &'static str,
}

impl ControlDef {
    /// 未接线的一行（禁用 + 悬停说明 [`Self::reason`]）。
    const fn unported(
        page: usize,
        kind: Kind,
        id: &'static str,
        label: &'static str,
        key: Option<&'static str>,
        source: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            id,
            page,
            kind,
            label,
            tip: None,
            key,
            wired: false,
            reason,
            value: None,
            range: None,
            default_bool: false,
            default_num: 0.0,
            source,
        }
    }

    /// 已接线的一行（真控件）。
    const fn wired(
        page: usize,
        kind: Kind,
        id: &'static str,
        label: &'static str,
        key: Option<&'static str>,
        source: &'static str,
    ) -> Self {
        Self {
            id,
            page,
            kind,
            label,
            tip: None,
            key,
            wired: true,
            reason: "",
            value: None,
            range: None,
            default_bool: false,
            default_num: 0.0,
            source,
        }
    }

    /// 静态文字行（小节标题 / 说明 / 行标签）——
    /// 源码里它们不是控件，也没有偏好键。
    const fn text(
        page: usize,
        kind: Kind,
        id: &'static str,
        label: &'static str,
        source: &'static str,
    ) -> Self {
        Self::unported(page, kind, id, label, None, source, "")
    }

    /// 加悬停提示（glade 的 tooltip 原文）。
    const fn tip(mut self, tip: &'static str) -> Self {
        self.tip = Some(tip);
        self
    }

    /// 加单选值（[`Kind::Radio`]）。
    const fn value(mut self, value: &'static str) -> Self {
        self.value = Some(value);
        self
    }

    /// 加取值范围（[`Kind::Spin`]）。
    const fn range(mut self, lower: f64, upper: f64) -> Self {
        self.range = Some((lower, upper));
        self
    }

    /// 复选框/开关的回落值。
    const fn default_bool(mut self, value: bool) -> Self {
        self.default_bool = value;
        self
    }

    /// 数值控件的回落值。
    const fn default_num(mut self, value: f64) -> Self {
        self.default_num = value;
        self
    }
}

// ---- 未接线原因（每条都指到"缺哪个子系统"）----

/// `input.device_mode`
/// `lib.i18n.userlocale`

/// 全部页面的全部控件（顺序 = glade 的 attach 顺序；同一页内自上而下）。
pub(crate) static CONTROLS: &[ControlDef] = &[
    // ===== 0. Pressure（glade:95-255；外框标题 Global Pressure Mapping）=====
    // CurveWidget `mapping_curve`（glade:123-130，300×215）；偏好读取
    // `input.global_pressure_mapping`（preferenceswindow.py:147），写回 `:276-279`。
    ControlDef::wired(
        0,
        Kind::Curve,
        "mapping_curve",
        "",
        Some("input.global_pressure_mapping"),
        "glade:123-130",
    ),
    // ===== 1. Devices（glade:268-328）=====
    // `MyPaintDeviceSettingsEditor`（glade:286-291，450×350）；`input.device_mode`
    // 的写回函数 `input_mode_combobox_changed_cb`（preferenceswindow.py:268-271）
    // 在 glade 里已经没有对应控件了（只剩 `input_mode_liststore`，glade:1840-1861）。
    ControlDef::wired(
        1,
        Kind::DeviceEditor,
        "devicesettingseditor1",
        "",
        Some("input.tablet_usage"),
        "glade:286-291",
    ),
    ControlDef::text(
        1,
        Kind::Note,
        "label33",
        "Here you can restrict pointing devices to specific tasks on the canvas, \
         or have them ignored completely if they glitch your drawing.",
        "glade:299-309",
    ),
    // ===== 2. Buttons（glade:345-409）=====
    // `ButtonMappingEditor`（glade:364-369，350×250）；`input.button_mapping` 的
    // 断言与写回见 preferenceswindow.py:84-95 / :273-274。
    // 已移植：编辑器读写 `input.button_mapping`，运行时每个指针事件现读。
    ControlDef::wired(
        2,
        Kind::ButtonEditor,
        "button_mapping_editor",
        "",
        Some("input.button_mapping"),
        "glade:364-369",
    ),
    ControlDef::text(
        2,
        Kind::Note,
        "label1",
        "• The space bar can be used like Button2.\n\
         • Some tablet pads' buttons cannot be held down when pressed.",
        "glade:377-386",
    ),
    // ===== 3. Keys（glade:422-484）=====
    // `AccelMapEditor`（glade:439-444）—— B6-2 的编辑器，已移植，嵌入本页。
    // 源码把它放在 "Keys" 页签里，所以这里不另开窗口（preferenceswindow.glade:439）。
    ControlDef::wired(
        TAB_KEYS,
        Kind::AccelEditor,
        "accel_map_editor",
        "",
        None,
        "glade:439-444",
    ),
    // `disable_fallback_bindings_checkbox`（glade:452-461）：初值
    // preferenceswindow.py:150-151（`.get(key, False)`），写回 `:264-266`
    // 并 `apply_settings()`。消费方是 keyboard.py:159-160 的 `fallbacks_disabled()`。
    ControlDef::wired(
        TAB_KEYS,
        Kind::Check,
        "disable_fallback_bindings_checkbox",
        "Disable fallback key bindings",
        Some("keyboard.disable_fallbacks"),
        "glade:452-461",
    )
    .tip(
        "The fallback key bindings are static built-in bindings that are only used \
         if their key combinations are not bound to anything else.",
    ),
    // ===== 4. Load & Save（glade:497-777）=====
    // 小节标题 `<b>Saving</b>`（glade:570-584）
    ControlDef::text(4, Kind::Header, "label7", "Saving", "glade:570-584"),
    ControlDef::text(
        4,
        Kind::RowLabel,
        "label5",
        "Default file format:",
        "glade:503-516",
    ),
    // `default_save_format_combobox`（glade:547-568，模型 glade:14-35）；
    // 初值 preferenceswindow.py:206-209，写回 `:310-312`。
    // 消费方：`prefs::save_extension` → `filehandling.py:1408-1411`。
    ControlDef::wired(
        4,
        Kind::Combo,
        "default_save_format_combobox",
        "",
        Some("saving.default_format"),
        "glade:547-568",
    ),
    ControlDef::text(
        4,
        Kind::RowLabel,
        "label6",
        "Prefix when saving scraps:",
        "glade:518-531",
    ),
    // `scrap_prefix_entry`（glade:533-545）；写回见 preferenceswindow.py:281-285。
    // 消费方：`get_scrap_prefix`（`filehandling.py:1420-1430`）→ Save/Next/Prev Scrap。
    ControlDef::wired(
        4,
        Kind::Entry,
        "scrap_prefix_entry",
        "",
        Some("saving.scrap_prefix"),
        "glade:533-545",
    ),
    // 小节标题 `<b>Automated Backups</b>`（glade:674-689）
    ControlDef::text(
        4,
        Kind::Header,
        "label35",
        "Automated Backups",
        "glade:674-689",
    ),
    ControlDef::text(
        4,
        Kind::RowLabel,
        "label36",
        "Back up unsaved work:",
        "glade:691-706",
    ),
    // `autosave_backups_switch`（glade:743-759）；写回 preferenceswindow.py:391-394。
    ControlDef::wired(
        4,
        Kind::Switch,
        "autosave_backups_switch",
        "",
        Some("document.autosave_backups"),
        "glade:743-759",
    )
    .tip(
        "Backs up work incrementally in the background to MyPaint's disk cache. \
         If the program crashes, you'll have a chance to restore what you were working on.",
    )
    .default_bool(true),
    ControlDef::text(
        4,
        Kind::RowLabel,
        "label38",
        "Time between backups:",
        "glade:761-776",
    ),
    // `autosave_interval_spinbutton`（glade:708-724）；取值范围来自
    // `autosave_interval_adjustment`（glade:6-13：lower=5 upper=300 step=1 page=10
    // value=10）；写回 preferenceswindow.py:396-398；源码只在备份开启时可编辑
    // （`:256` 的 `set_sensitive(autosave)`）。单位文字是 `label37`（glade:726-741）。
    ControlDef::wired(
        4,
        Kind::Spin,
        "autosave_interval_spinbutton",
        "seconds of inactivity",
        Some("document.autosave_interval"),
        "glade:708-724",
    )
    .range(5.0, 300.0)
    .default_num(10.0),
    // 小节标题 `<b>Color Management</b>`（glade:586-601）
    ControlDef::text(
        4,
        Kind::Header,
        "label2",
        "Color Management",
        "glade:586-601",
    ),
    ControlDef::text(
        4,
        Kind::RowLabel,
        "label3",
        "Color space conversions:",
        "glade:603-616",
    ),
    // `display_colorspace_srgb_radiobutton`（glade:625-639）；
    // 初值 preferenceswindow.py:214-219，写回 `:320-324`。
    ControlDef::wired(
        4,
        Kind::Radio,
        "display_colorspace_srgb_radiobutton",
        "Convert to sRGB when loading; mark as sRGB when saving",
        Some("display.colorspace"),
        "glade:625-639",
        )
    .tip(
        "Images containing full colorspace information will be converted to sRGB when \
         they are loaded into MyPaint. When MyPaint saves an image, hints will be added \
         to it so that other programs can render it properly.\n\n\
         This assumes that the display is calibrated to something close to sRGB, the \
         standard colour space for the web and a pessimistic approximation of most PC \
         monitors. If you know your display is radically different from this, use the \
         \"no automatic conversions or tagging\" setting instead.",
    )
    .value("srgb"),
    // `display_colorspace_unknown_radiobutton`（glade:647-665）；写回 `:314-318`。
    ControlDef::wired(
        4,
        Kind::Radio,
        "display_colorspace_unknown_radiobutton",
        "No automatic conversions or tagging",
        Some("display.colorspace"),
        "glade:647-665",
        )
    .tip(
        "No color conversions are performed when loading, and files are saved without \
         color management information.",
    )
    .value("unknown"),
    // ===== 5. View（glade:795-1274）=====
    ControlDef::text(
        5,
        Kind::Header,
        "label8",
        "Zoom and Rendering",
        "glade:802-816",
    ),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "label10",
        "Default zoom level:",
        "glade:818-831",
    ),
    // `default_zoom_combobox`（glade:833-854，模型 glade:36-61）；
    // 初值 preferenceswindow.py:165-169（`"%0.2f" % zoom`），写回 `:287-290`。
    ControlDef::wired(
        5,
        Kind::Combo,
        "default_zoom_combobox",
        "",
        Some("view.default_zoom"),
        "glade:833-854",
    )
    .default_num(1.0),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "label30",
        "When showing transparency:",
        "glade:1030-1043",
    ),
    // `real_alpha_checks_checkbutton`（glade:1010-1028）；初值
    // preferenceswindow.py:197-199，写回 `:306-308`。
    ControlDef::wired(
        5,
        Kind::Check,
        "real_alpha_checks_checkbutton",
        "Use a real checkerboard pattern",
        Some("view.real_alpha_checks"),
        "glade:1010-1028",
    )
    .tip(
        "Draw a real checkerboard pattern to show transparency.\n\
         This slows down the display, but can be less confusing.",
    )
    .default_bool(true),
    // 小节标题 `<b>Interface</b>`（glade:856-871）
    ControlDef::text(5, Kind::Header, "label26", "Interface", "glade:856-871"),
    ControlDef::text(5, Kind::RowLabel, "label41", "Language:", "glade:1245-1258"),
    // `locale_combobox`（glade:1225-1243）；初值 preferenceswindow.py:157-163，
    // 写回 `:412-418`。偏好键是 `lib/i18n.py:24` 的 `USER_LOCALE_PREF`
    // = `lib.i18n.userlocale`（软键，默认表里没有）。
    ControlDef::wired(
        5,
        Kind::Combo,
        "locale_combobox",
        "",
        Some("lib.i18n.userlocale"),
        "glade:1225-1243",
    ),
    // 组合框右边的 `(needs restart)`（glade:1260-1273）
    ControlDef::text(5, Kind::Note, "label42", "(needs restart)", "glade:1260-1273"),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "label9",
        "Toolbar icon size:",
        "glade:873-887",
    ),
    // `toolbar_icon_size_large_radio`（glade:968-977）；写回 preferenceswindow.py:297-300。
    // 消费方：`toolbars::icon_size` → `tool_button` 的图标边长。
    ControlDef::wired(
        5,
        Kind::Radio,
        "toolbar_icon_size_large_radio",
        "Large (requires recent GTK3)",
        Some("ui.toolbar_icon_size"),
        "glade:968-977",
    )
    .value("large"),
    // `toolbar_icon_size_small_radio`（glade:985-1001）；写回 `:292-295`。
    ControlDef::wired(
        5,
        Kind::Radio,
        "toolbar_icon_size_small_radio",
        "Small",
        Some("ui.toolbar_icon_size"),
        "glade:985-1001",
    )
    .value("small"),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "label27",
        "Freehand cursor type:",
        "glade:889-903",
    ),
    // `freehand_cursor_combobox`（glade:1089-1103，模型 glade:62-87）；
    // 初值 preferenceswindow.py:228-231，写回 `:344-380`（同时按预设改
    // `cursor.freehand.*` 六个键）。`cursor.freehand.style` 是软键（`.get(..., "thin")`）。
    ControlDef::wired(
        5,
        Kind::Combo,
        "freehand_cursor_combobox",
        "",
        Some("cursor.freehand.style"),
        "glade:1089-1103",
    ),
    // `hide_cursor_while_painting_checkbutton`（glade:1111-1119）；初值
    // preferenceswindow.py:201-204（`.get(key, False)`），写回 `:404-406`。
    // 消费方：`ModePrefs`（B6-1）→ freehand.py:280。
    ControlDef::wired(
        5,
        Kind::Check,
        "hide_cursor_while_painting_checkbutton",
        "Hide cursor while painting",
        Some("ui.hide_cursor_while_painting"),
        "glade:1111-1119",
    ),
    // `show_crosshair_checkbutton`（glade:1131-1138）；初值
    // preferenceswindow.py:102-106（`.get("cursor.dynamic_crosshair", False)`），
    // 写回 `:385-389`。
    ControlDef::wired(
        5,
        Kind::Check,
        "show_crosshair_checkbutton",
        "Show crosshair for small brush cursors",
        Some("cursor.dynamic_crosshair"),
        "glade:1131-1138",
    ),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "dynamic_crosshair_threshold_label",
        "Radius threshold:",
        "glade:1146-1157",
    ),
    // `dynamic_crosshair_threshold_spinbutton`（glade:1159-1175）；初值
    // preferenceswindow.py:108-119（`.get("cursor.dynamic_crosshair_threshold", 8)`，
    // 下界 `1 + p.get("cursor.freehand.min_size", 4) // 2` = 3，上界是 GDK 的最大
    // 光标尺寸 —— 这里没有 GDK，上界取源码的 `Gtk.Adjustment` 语义（不设），
    // 写回 `:382-383`。
    ControlDef::wired(
        5,
        Kind::Spin,
        "dynamic_crosshair_threshold_spinbutton",
        "",
        Some("cursor.dynamic_crosshair_threshold"),
        "glade:1159-1175",
    )
    .tip(
        "Lines will be shown when the brush circle radius is smaller than this value.",
    )
    .range(3.0, f64::INFINITY)
    .default_num(8.0),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "label11",
        "Dark theme variant:",
        "glade:905-918",
    ),
    // `dark_theme_checkbutton`（glade:920-936）；初值 preferenceswindow.py:182-185，
    // 写回 `:302-304`。
    ControlDef::wired(
        5,
        Kind::Check,
        "dark_theme_checkbutton",
        "If available in GTK3 theme (needs restart)",
        Some("ui.dark_theme_variant"),
        "glade:920-936",
    )
    .tip(
        "Use a light-on-dark theme variant, when it's supported by the GTK theme. \
         This is generally sensible for image editors and media viewers.",
    )
    .default_bool(true),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "label39",
        "Handle smooth scrolling events:",
        "glade:1045-1058",
    ),
    // `smooth_scrolling_checkbutton`（glade:1060-1081）；初值
    // preferenceswindow.py:187-190（`.get("ui.support_smooth_scrolling", True)` ——
    // 软键，默认表里没有），写回 `:400-402`。
    ControlDef::wired(
        5,
        Kind::Check,
        "smooth_scrolling_checkbutton",
        "When devices send them (needs restart)",
        Some("ui.support_smooth_scrolling"),
        "glade:1060-1081",
    )
    .tip(
        "Smooth scrolling support makes scroll gestures and scrollwheel events from \
         some devices smooth rather than in steps. It's good for touchpads, but can be \
         irritating if you're used to mouse scrolling.\n\n\
         If smooth scrolling is supported, stepwise scrolling can be turned back on \
         briefly by holding the shift key down while scrolling.\n\n\
         Scrolling to pan the view needs this setting to be turned on, and for your \
         device to be configured to send bidirectional scroll events.",
    )
    .default_bool(true),
    ControlDef::text(
        5,
        Kind::RowLabel,
        "label40",
        "Blink layers on creation/selection:",
        "glade:1210-1223",
    ),
    // `blink_layers_checkbutton`（glade:1191-1202）；初值
    // preferenceswindow.py:192-195（`.get("ui.blink_layers", True)` —— 软键），
    // 写回 `:408-410`。
    ControlDef::wired(
        5,
        Kind::Check,
        "blink_layers_checkbutton",
        "Enabled",
        Some("ui.blink_layers"),
        "glade:1191-1202",
    )
    .tip("When a layer is created or selected, show only that layer for a short time period.")
    .default_bool(true),
    // "Some of these settings require MyPaint to be restarted."（glade:938-958）
    ControlDef::text(
        5,
        Kind::Note,
        "label28",
        "Some of these settings require MyPaint to be restarted.",
        "glade:938-958",
    ),
    // ===== 6. Color（glade:1291-1416；外框标题 Color Wheel）=====
    // `label12`（glade:1312-1325）
    ControlDef::text(
        6,
        Kind::RowLabel,
        "label12",
        "Primaries are:",
        "glade:1312-1325",
    )
    .tip(
        "The set of colors to show as primary on disk-style color wheels.\n\
         Different traditions have different color harmonies.",
    ),
    // `color_wheel_rgb_radiobutton`（glade:1333-1347）。这三个单选**不是**
    // 偏好控件而是 `cm.set_wheel_type()`（preferenceswindow.py:326-342），
    // 但 `set_wheel_type` 会把结果写进偏好 `colors.wheels.type`
    // （adjbases.py:291-311），所以这里按"偏好键 + 已接线"登记。
    ControlDef::wired(
        6,
        Kind::Radio,
        "color_wheel_rgb_radiobutton",
        "Standard digital Red/Green/Blue",
        Some(pigment_engine::color::PREFS_KEY_WHEEL_TYPE),
        "glade:1333-1347",
    )
    .tip(
        "The red, green, and blue 'computer monitor' primaries, evenly arranged around \
         the circle. Green is opposite magenta.",
    )
    .value("rgb"),
    // `color_wheel_ryb_radiobutton`（glade:1355-1375）
    ControlDef::wired(
        6,
        Kind::Radio,
        "color_wheel_ryb_radiobutton",
        "Traditional artist's Red/Yellow/Blue",
        Some(pigment_engine::color::PREFS_KEY_WHEEL_TYPE),
        "glade:1355-1375",
    )
    .tip(
        "The traditionally arranged red/yellow/blue wheel as used by many artists since \
         the 18th century, approximated with pure display colors evenly arranged around \
         the circle. Green is opposite red.",
    )
    .value("ryb"),
    // `color_wheel_rygb_radiobutton`（glade:1377-1397）
    ControlDef::wired(
        6,
        Kind::Radio,
        "color_wheel_rygb_radiobutton",
        "Red-Green and Blue-Yellow opponent pairs",
        Some(pigment_engine::color::PREFS_KEY_WHEEL_TYPE),
        "glade:1377-1397",
    )
    .tip(
        "Quick and dirty approximation of wheels informed by theories of color which \
         use four 'psychological primaries' or 'unique hues' arranged in opposing pairs \
         at the cardinal points. Red is opposite green, and yellow is opposite blue. \
         CIECAM02 and NCS present a similar model. Here we use pure display red, yellow \
         etc. as the four primaries.",
    )
    .value("rygb"),
    // ===== 7. Compatibility（glade:1433-1827；外框标题 Compatibility settings）=====
    // `def_compat_label`（glade:1454-1467）
    ControlDef::text(
        7,
        Kind::RowLabel,
        "def_compat_label",
        "Default Compatibility Mode:",
        "glade:1454-1467",
    ),
    // `compat_1_x_radiobutton`（glade:1666-1677）；写回
    // compatibility.py:400-401（`preferences[DEFAULT_COMPAT] = mode`）。
    ControlDef::wired(
        7,
        Kind::Radio,
        "compat_1_x_radiobutton",
        "1.x",
        Some("default_compatibility_mode"),
        "glade:1666-1677",
        )
    .tip(
        "When creating new documents, use the legacy 1.x compatibility mode by default. \
         In this mode 1.x .ora files will look the same as they did in older versions, \
         but pigment blending will not work well.",
    )
    .value("1.x"),
    // `compat_2_x_radiobutton`（glade:1685-1700）
    ControlDef::wired(
        7,
        Kind::Radio,
        "compat_2_x_radiobutton",
        "2.x",
        Some("default_compatibility_mode"),
        "glade:1685-1700",
        )
    .tip(
        "When creating new documents, use the regular 2.x compatibility mode by default. \
         Old documents will often look different in this mode.",
    )
    .value("2.x"),
    // `<b>1.x Settings</b>`（glade:1480-1492）
    ControlDef::text(
        7,
        Kind::Header,
        "label_1_x_settings",
        "1.x Settings",
        "glade:1480-1492",
    ),
    ControlDef::text(
        7,
        Kind::RowLabel,
        "pigm_brsh_label",
        "Default Pigment Brush Setting:",
        "glade:1577-1589",
    ),
    // `pigment_setting_switch_1_x`（glade:1520-1529）；写回
    // compatibility.py:407-409（`prefs[COMPAT_SETTINGS]["1.x"]["pigment_on_by_default"]`）。
    ControlDef::wired(
        7,
        Kind::Switch,
        "pigment_setting_switch_1_x",
        "",
        Some("compability_settings"),
        "glade:1520-1529",
        ),
    ControlDef::text(
        7,
        Kind::RowLabel,
        "pigm_brsh_label1",
        "Default New Layer Type:",
        "glade:1531-1544",
    ),
    // `def_new_layer_normal_1_x`（glade:1546-1559）—— 标签与提示由
    // compatibility.py:353-355 用 `MODE_STRINGS[CombineNormal]`（lib/modes.py）替换，
    // 即 "Normal" / "The top layer only, without blending colors."。
    ControlDef::wired(
        7,
        Kind::Radio,
        "def_new_layer_normal_1_x",
        "Normal",
        Some("compability_settings"),
        "glade:1546-1559",
        )
    .tip("The top layer only, without blending colors."),
    // `def_new_layer_pigment_1_x`（glade:1561-1575）—— `MODE_STRINGS[CombineSpectralWGM]`
    ControlDef::wired(
        7,
        Kind::Radio,
        "def_new_layer_pigment_1_x",
        "Pigment",
        Some("compability_settings"),
        "glade:1561-1575",
        )
    .tip("Similar to mixing actual pigments by upsampling to 10 spectral channels."),
    // `<b>2.x Settings</b>`（glade:1494-1506）
    ControlDef::text(
        7,
        Kind::Header,
        "label_2_x_settings",
        "2.x Settings",
        "glade:1494-1506",
    ),
    ControlDef::text(
        7,
        Kind::RowLabel,
        "pigm_brsh_label2",
        "Default Pigment Brush Setting:",
        "glade:1591-1603",
    ),
    // `pigment_setting_switch_2_x`（glade:1650-1660）
    ControlDef::wired(
        7,
        Kind::Switch,
        "pigment_setting_switch_2_x",
        "",
        Some("compability_settings"),
        "glade:1650-1660",
        ),
    ControlDef::text(
        7,
        Kind::RowLabel,
        "pigm_brsh_label3",
        "Default New Layer Type:",
        "glade:1605-1618",
    ),
    // `def_new_layer_normal_2_x`（glade:1620-1633）
    ControlDef::wired(
        7,
        Kind::Radio,
        "def_new_layer_normal_2_x",
        "Normal",
        Some("compability_settings"),
        "glade:1620-1633",
        )
    .tip("The top layer only, without blending colors."),
    // `def_new_layer_pigment_2_x`（glade:1635-1648）
    ControlDef::wired(
        7,
        Kind::Radio,
        "def_new_layer_pigment_2_x",
        "Pigment",
        Some("compability_settings"),
        "glade:1635-1648",
        )
    .tip("Similar to mixing actual pigments by upsampling to 10 spectral channels."),
    // `<b>File loading</b>`（glade:1793-1807）
    ControlDef::text(
        7,
        Kind::Header,
        "compat_frame_label1",
        "File loading",
        "glade:1793-1807",
    ),
    // `compat_file_behavior_label`（glade:1764-1777）
    ControlDef::text(
        7,
        Kind::RowLabel,
        "compat_file_behavior_label",
        "When Mode is Unspecified:",
        "glade:1764-1777",
    )
    .tip(
        "This will only apply to files saved with versions of MyPaint older than 2.0. \
         When you save an OpenRaster file with MyPaint 2.0 or newer the active \
         compatibility mode will be written to the file.",
    ),
    // `compat_file_behavior_combobox`（glade:1779-1791）；选项见
    // compatibility.py:191-224，写回 `:244-246`。
    ControlDef::wired(
        7,
        Kind::Combo,
        "compat_file_behavior_combobox",
        "",
        Some("compat_behavior_when_unknown"),
        "glade:1779-1791",
        ),
    ControlDef::text(
        7,
        Kind::RowLabel,
        "file_warnings",
        "File compatibility warnings:",
        "glade:1735-1748",
    ),
    // `file_compat_warning_mild`（glade:1721-1733）；键 `ui.file_compat_warning_mild`
    // 来自 compatibility.py:28-31 的 `FILE_WARNINGS`，初值 `:363-365`
    // （`.get(key, True)` —— 软键）。
    ControlDef::wired(
        7,
        Kind::Check,
        "file_compat_warning_mild",
        "Show warning for files that may be partially incompatible",
        Some("ui.file_compat_warning_mild"),
        "glade:1721-1733",
        )
    .default_bool(true),
    // `file_compat_warning_severe`（glade:1750-1762）；初值 `:366-368`。
    ControlDef::wired(
        7,
        Kind::Check,
        "file_compat_warning_severe",
        "Show warning for files that may be fully incompatible",
        Some("ui.file_compat_warning_severe"),
        "glade:1750-1762",
        )
    .default_bool(true),
];

/// 下拉框的一个选项（glade 的 `GtkListStore` 的一行：id + 标签）。
pub(crate) struct OptionDef {
    /// 存进偏好键的 id（`GtkComboBox` 的 `id_column=0`）。
    pub(crate) id: &'static str,
    /// 列表里显示的文字（`id_column` 的下一列）。
    pub(crate) label: &'static str,
}

/// `lib.i18n.userlocale` 的三个候选（`lib/i18n.py` 的 system/en/zh 三类）。
pub(crate) static LOCALES: &[OptionDef] = &[
    OptionDef { id: "", label: "System default" },
    OptionDef { id: "en_US", label: "English" },
    OptionDef { id: "zh_CN", label: "中文（中国）" },
];

/// `saving.default_format` 的三个取值 —— glade `default_save_format_liststore`
/// （`:14-35`）；与 `pigment_engine::prefs::format_extension` 的扩展名同源。
pub(crate) static SAVE_FORMATS: &[OptionDef] = &[
    OptionDef {
        id: "openraster",
        label: "OpenRaster (*.ora)",
    },
    OptionDef {
        id: "png-solid",
        label: "PNG solid with background (*.png)",
    },
    OptionDef {
        id: "jpeg-90%",
        label: "JPEG 90% quality (*.jpg; *.jpeg)",
    },
];

/// `view.default_zoom` 的四个取值 —— glade `default_zoom_level_liststore`
/// （`:36-61`）。id 是 `"%0.2f"` 的字符串（见 [`zoom_id`]）。
pub(crate) static ZOOM_LEVELS: &[OptionDef] = &[
    OptionDef {
        id: "0.25",
        label: "25%",
    },
    OptionDef {
        id: "0.50",
        label: "50%",
    },
    OptionDef {
        id: "1.00",
        label: "100%",
    },
    OptionDef {
        id: "2.00",
        label: "200%",
    },
];

/// `cursor.freehand.style` 的四个取值 —— glade `freehand_cursor_liststore`
/// （`:62-87`）。
pub(crate) static CURSOR_TYPES: &[OptionDef] = &[
    OptionDef {
        id: "thin",
        label: "Circle with light outline",
    },
    OptionDef {
        id: "medium",
        label: "Circle with medium outline",
    },
    OptionDef {
        id: "thick",
        label: "Circle with heavy outline",
    },
    OptionDef {
        id: "crosshair",
        label: "Crosshair",
    },
];

/// `compat_behavior_when_unknown` 的三个取值 —— `gui/compatibility.py:191-224`
/// 的 `_OPTIONS` / `_LABELS`（id 来自 `gui/compatconfig.py:17-19`）。
pub(crate) static COMPAT_FILE_BEHAVIORS: &[OptionDef] = &[
    OptionDef {
        id: "always-1.x",
        label: "Always open in 1.x mode",
    },
    OptionDef {
        id: "always-2.x",
        label: "Always open in 2.x mode",
    },
    OptionDef {
        id: "unless-pigment-layer-1.x",
        label: "Open in 1.x mode unless file contains pigment layers",
    },
];

/// `compability_settings` 的某个控件属于哪个模式（id 末尾 `_1_x` / `_2_x`）。
fn compat_mode_control(id: &str) -> Option<pigment_engine::compat::CompatMode> {
    use pigment_engine::compat::CompatMode;
    if id.ends_with("_1_x") {
        Some(CompatMode::C1X)
    } else if id.ends_with("_2_x") {
        Some(CompatMode::C2X)
    } else {
        None
    }
}

/// 某个下拉框的选项表。
///
/// 空表 = 选项是运行期算出来的：`locale_combobox` 的模型由
/// `setup_locale_combobox()`（`preferenceswindow.py:421-488`）按
/// `lib/config.supported_locales` 现建，本移植没有那份语言表（见记账）。
pub(crate) fn options_for(control_id: &str) -> &'static [OptionDef] {
    match control_id {
        "default_save_format_combobox" => SAVE_FORMATS,
        "default_zoom_combobox" => ZOOM_LEVELS,
        "freehand_cursor_combobox" => CURSOR_TYPES,
        "compat_file_behavior_combobox" => COMPAT_FILE_BEHAVIORS,
        "locale_combobox" => LOCALES,
        _ => &[],
    }
}

/// 浮点偏好在 `default_zoom_combobox` 里的 id 形式 ——
/// `preferenceswindow.py:167` 的 `zoom_idcolstr = "%0.2f" % (zoom_float,)`。
pub(crate) fn zoom_id(zoom: f64) -> String {
    format!("{zoom:.2}")
}

/// 应用一个自由手绘光标预设 —— `preferenceswindow.py:344-380`。
///
/// `thin` 是出厂预设：删掉六个键，让读取处回落到默认值；`medium`/`thick`
/// 写入各自的尺寸与颜色；`crosshair` 只改 `cursor.freehand.style`
/// （源码的 `if/elif` 没有 crosshair 分支）。
pub(crate) fn apply_cursor_preset(prefs: &mut Prefs, cname: &str) {
    const KEYS: [&str; 6] = [
        "cursor.freehand.min_size",
        "cursor.freehand.outer_line_width",
        "cursor.freehand.inner_line_width",
        "cursor.freehand.inner_line_inset",
        "cursor.freehand.outer_line_color",
        "cursor.freehand.inner_line_color",
    ];
    match cname {
        "thin" => {
            for k in KEYS {
                prefs.remove(k);
            }
        }
        "medium" => {
            prefs.set_i64(KEYS[0], 5);
            prefs.set_f64(KEYS[1], 2.666);
            prefs.set_f64(KEYS[2], 1.333);
            prefs.set_i64(KEYS[3], 2);
            prefs.set_f64_array(KEYS[4], &[0.0, 0.0, 0.0, 1.0]);
            prefs.set_f64_array(KEYS[5], &[1.0, 1.0, 1.0, 1.0]);
        }
        "thick" => {
            prefs.set_i64(KEYS[0], 7);
            prefs.set_f64(KEYS[1], 3.75);
            prefs.set_f64(KEYS[2], 2.25);
            prefs.set_i64(KEYS[3], 3);
            prefs.set_f64_array(KEYS[4], &[0.0, 0.0, 0.0, 1.0]);
            prefs.set_f64_array(KEYS[5], &[1.0, 1.0, 1.0, 1.0]);
        }
        _ => {}
    }
}

/// 默认表里**没有**、只能从源码读出默认值的键（"软键"）。
///
/// 判定一条控件的键"是否合法"用：键在 `Prefs` 的默认表里，或在
/// 这张软键表里 —— 两者都不是就是抄错了键名。每条都标源码的行号
/// （默认值就写在那一行的 `.get(key, default)` 里）。
///
/// 运行时只给门禁测试读（与 `shell/mod.rs` 的 `FOOTER_WIDGETS`、
/// `color_panel::ColorPageDef::action` 同处置），故标 `dead_code`。
#[allow(dead_code)]
pub(crate) static SOFT_KEYS: &[(&str, &str)] = &[
    // 本块之前就已经是软键的三条（B6-1/B6-2 的记账）
    (
        "ui.hide_cursor_while_painting",
        "preferenceswindow.py:202 的 .get(key, False)；freehand.py:280",
    ),
    (
        "keyboard.disable_fallbacks",
        "preferenceswindow.py:151 的 .get(key, False)；keyboard.py:159-160",
    ),
    (
        "colors.wheels.type",
        "adjbases.py:134-135 的 .get(key, _DEFAULT_WHEEL_TYPE)；:95 默认 \"rgb\"",
    ),
    // 本块读出来的软键
    (
        "cursor.freehand.style",
        "preferenceswindow.py:229 的 .get(key, \"thin\")",
    ),
    (
        "cursor.dynamic_crosshair",
        "preferenceswindow.py:104 的 .get(key, False)",
    ),
    (
        "cursor.dynamic_crosshair_threshold",
        "preferenceswindow.py:109 的 .get(key, 8)",
    ),
    (
        "ui.support_smooth_scrolling",
        "preferenceswindow.py:188 的 .get(key, True)",
    ),
    (
        "ui.blink_layers",
        "preferenceswindow.py:193 的 .get(key, True)",
    ),
    (
        "lib.i18n.userlocale",
        "lib/i18n.py:24 的 USER_LOCALE_PREF；preferenceswindow.py:159 的 .get(key, None)",
    ),
    (
        "ui.file_compat_warning_mild",
        "compatibility.py:364 的 .get(key, True)",
    ),
    (
        "ui.file_compat_warning_severe",
        "compatibility.py:367 的 .get(key, True)",
    ),
    (
        "input.tablet_usage",
        "本移植新增：gui/device.py AllowedUsage 的全局子集（winit 无设备名）",
    ),
];

// ---------------------------------------------------------------------------
// 界面状态
// ---------------------------------------------------------------------------

/// 偏好窗口的界面状态（egui 立即模式：控件没有对象，状态得由应用持有）。
///
/// 对应源码 `PreferencesWindow.__init__` 里那几个字段
/// （`preferenceswindow.py:53-55` 的 `connect`/`in_update_ui`）：窗口本身的可见性
/// 由 `toggle_window_cb` + `_subwindow_hide_cb`（`application.py:752-756`）维护，
/// 页签由 `GtkNotebook` 自己维护。
#[derive(Default)]
pub(crate) struct PrefsWindowState {
    /// 窗口是否可见（`PreferencesWindow` 动作的开/关状态，见
    /// [`PigmentApp::toolbar_pressed`]）。
    pub(crate) open: bool,
    /// 当前页签下标（[`TABS`] 的下标）。
    pub(crate) tab: usize,
    /// 按钮行右侧的状态文字（本项目补的，见模块文档的记账）。
    pub(crate) status: Option<String>,
    /// 压力曲线正在拖的点（`CurveWidget.grabbed`，`curve.py:49`）。
    pub(crate) curve_drag: Option<usize>,
}

impl PrefsWindowState {
    /// 关窗（X）—— `windowing.py:43` 的 `hide_on_delete()`：**只藏窗口**。
    ///
    /// 不回退（那是 `REVERT` 的事，`preferenceswindow.py:133-135`）、
    /// 也不存盘（那是 `ACCEPT` 的事，`:129-132`）。页签与状态不动。
    pub(crate) fn close(&mut self) {
        self.open = false;
    }
}

// ---------------------------------------------------------------------------
// 三条路径的数据侧（可脱离 GPU 单测）
// ---------------------------------------------------------------------------

/// `ACCEPT` 的存盘那一步 —— `preferenceswindow.py:130` 的 `app.save_settings()`
/// （`application.py:498-512`：`json.dumps(preferences, indent=2)` 直接覆写）。
///
/// 这是 [`PigmentApp::save_gui_config`] 的最后一步；失败时返回给状态行用的文字。
pub(crate) fn settings_on_accept(prefs: &Prefs) -> Result<(), String> {
    prefs
        .save()
        .map_err(|e| format!("保存偏好失败（{}）：{e}", prefs.path().display()))
}

/// `REVERT` 的数据侧 —— `preferenceswindow.py:134` 的 `app.load_settings()`
/// （`application.py:524-539`：清空 + 默认表 + `settings.json` 浅覆盖）。
///
/// 注意它读的是**磁盘**，不是"打开窗口时的快照"（见模块文档第 1 条）。
pub(crate) fn settings_on_revert(prefs: &mut Prefs) {
    prefs.reload();
}

// ---------------------------------------------------------------------------
// 窗口绘制
// ---------------------------------------------------------------------------

impl PigmentApp {
    /// 偏好窗口（[`TITLE`]）—— 入口是 `PreferencesWindow` 动作
    /// （`resources.xml:1330-1335`，`toggle_window_cb`）。
    pub(crate) fn preferences_window(&mut self, ctx: &egui::Context) {
        if self.prefs_window.open {
            let mut open = true;
            egui::Window::new(crate::i18n::tr(TITLE))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                // glade 没有给窗口尺寸（GTK 按内容撑开）；这里给一个够放下
                // 最宽的 Load & Save 页的默认值 —— 本项目自定，记账。
                .default_width(660.0)
                .default_height(540.0)
                .show(ctx, |ui| self.preferences_body(ui));
            if !open {
                // 关窗 = `hide_on_delete()`：只藏起来（见 [`PrefsWindowState::close`]）
                self.prefs_window.close();
                // 捕获对话框跟着一起收（GTK 那边窗口藏了对话框就没了）
                self.key_editor.capture = None;
                self.button_editor.capture = None;
            }
        }
        // 按键捕获对话框（B6-2）浮在偏好窗口之上 —— `accelmap.py:361` 的
        // `set_modal(True)`；没在捕获时它直接返回。
        self.key_capture_modal(ctx);
        // 按钮映射的鼠标捕获对话框（`buttonmap.py:461-637`）同理。
        self.button_capture_modal(ctx);
    }

    /// 窗口内容：页签条 + 页面体 + 底部两个按钮。
    fn preferences_body(&mut self, ui: &mut egui::Ui) {
        self.preferences_tabs(ui);
        // GTK notebook 的页与标签之间有边框，这里用一条分隔线表达
        ui.separator();
        let tab = self.prefs_window.tab;
        self.prefs_page_body(ui, tab);
        ui.separator();
        // 按钮行 —— 源码只有两个按钮：`REVERT` 与 `ACCEPT`
        // （`preferenceswindow.py:46-51` 的 `add_buttons`），**没有 Cancel**。
        let mut accept = false;
        let mut revert = false;
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::tr(BUTTON_REVERT)).clicked() {
                revert = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(crate::i18n::tr(BUTTON_OK)).clicked() {
                    accept = true;
                }
                if let Some(status) = &self.prefs_window.status {
                    ui.label(egui::RichText::new(status).weak().small());
                }
            });
        });
        if accept {
            self.preferences_accept();
        }
        if revert {
            self.preferences_revert();
        }
    }

    /// 页签条 —— 八个页面（[`TABS`]），顺序与标题照 glade。
    fn preferences_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (index, tab) in TABS.iter().enumerate() {
                let selected = self.prefs_window.tab == index;
                // `selectable_label` 的选中态 = `GtkNotebook` 的活动页
                let resp =
                    ui.selectable_label(selected, crate::i18n::tr(tab.title));
                let tip = match tab.frame_heading {
                    Some(h) => format!("{}\n{h}", crate::i18n::tr(tab.title)),
                    None => crate::i18n::tr(tab.title).to_string(),
                };
                if resp.on_hover_text(tip).clicked() {
                    self.prefs_window.tab = index;
                }
            }
        });
    }

    /// 页面体。
    ///
    /// `Keys` 页不套外层滚动区：B6-2 的加速键编辑器自己带一个 `ScrollArea`
    /// 并且要吃掉页面的剩余高度（GTK 里 `AccelMapEditor` 是 `expand=True`
    /// 的树视图，`glade:439-450`）。
    ///
    /// 两条分支都要**先给底部按钮行留出高度** —— GTK 的 `action_area` 在布局里
    /// 天然占位（`GtkDialog` 自己排），egui 的 `ScrollArea` 会把剩余高度全吃掉，
    /// 不留的话按钮会被挤到窗口外。
    fn prefs_page_body(&mut self, ui: &mut egui::Ui, tab: usize) {
        let height = (ui.available_height() - button_row_reserve(ui)).max(60.0);
        if tab == TAB_KEYS {
            let size = egui::vec2(ui.available_width(), height);
            ui.allocate_ui_with_layout(
                size,
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| self.prefs_page_rows(ui, tab),
            );
        } else {
            egui::ScrollArea::vertical()
                .id_salt(("prefs_page_scroll", tab))
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| self.prefs_page_rows(ui, tab));
        }
    }

    /// 一页的全部行（外框标题 + [`CONTROLS`] 里 `page == tab` 的那些）。
    fn prefs_page_rows(&mut self, ui: &mut egui::Ui, tab: usize) {
        if let Some(heading) = TABS.get(tab).and_then(|t| t.frame_heading) {
            ui.label(egui::RichText::new(crate::i18n::tr(heading)).strong());
            ui.separator();
        }
        for control in CONTROLS.iter().filter(|c| c.page == tab) {
            self.prefs_control_row(ui, control);
        }
    }

    /// 画一行控件（或一行静态文字）。
    fn prefs_control_row(&mut self, ui: &mut egui::Ui, c: &ControlDef) {
        match c.kind {
            Kind::Header => {
                ui.add_space(10.0);
                ui.label(egui::RichText::new(crate::i18n::tr(c.label)).strong());
            }
            Kind::Note => {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(c.label))
                        .weak()
                        .small()
                        .italics(),
                );
            }
            Kind::RowLabel => {
                let resp = ui.label(crate::i18n::tr(c.label));
                if let Some(tip) = c.tip {
                    resp.on_hover_text(crate::i18n::tr(tip));
                }
            }
            Kind::AccelEditor => {
                // 已移植的自绘编辑器：直接嵌进来（glade:439 的 AccelMapEditor）
                self.keys_editor_page(ui);
            }
            Kind::Curve => self.prefs_curve_row(ui, c),
            Kind::DeviceEditor => self.device_editor_page(ui),
            Kind::ButtonEditor => self.button_editor_page(ui),
            Kind::Check | Kind::Switch => self.prefs_check_row(ui, c),
            Kind::Radio => self.prefs_radio_row(ui, c),
            Kind::Combo => self.prefs_combo_row(ui, c),
            Kind::Entry => self.prefs_entry_row(ui, c),
            Kind::Spin => self.prefs_spin_row(ui, c),
        }
        ui.add_space(4.0);
    }

    /// Devices 页：数位板工具策略（`gui/device.py::AllowedUsage` 的全局子集）。
    ///
    /// winit 0.31 的 `TabletToolData` 没有设备名/轴范围，无法做 MyPaint 的
    /// 逐设备分组；这里按“所有数位板工具”共享一条策略，鼠标不受影响。
    fn device_editor_page(&mut self, ui: &mut egui::Ui) {
        use crate::device::TabletUsage;
        let before = self.tablet_usage();
        let mut usage = before;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_size(egui::vec2(450.0, 120.0));
            ui.heading("Tablet tool policy");
            ui.label(
                "All tablet tools (pen and eraser end) share this policy. \
                 The mouse is never affected.",
            );
            egui::ComboBox::from_id_salt("tablet_usage")
                .selected_text(usage.label())
                .show_ui(ui, |ui| {
                    for u in TabletUsage::VALUES {
                        ui.selectable_value(&mut usage, u, u.label());
                    }
                });
            if ui.button("Reset to Any Task").clicked() {
                usage = TabletUsage::Any;
            }
        });
        if usage != before {
            self.prefs
                .set_str("input.tablet_usage", usage.as_str());
        }
        ui.add_space(4.0);
        // 当前数位板设备的单独覆盖（winit DeviceId 的原始 i64）。
        if let Some(id) = self.tablet.device_id.clone() {
            let key = format!("input.device_usage.{id}");
            let label = crate::device::device_label(&id);
            let before_specific = self
                .prefs
                .get(&key)
                .and_then(|v| v.as_str())
                .map(TabletUsage::from_str);
            let mut specific = before_specific;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(egui::vec2(450.0, 80.0));
                ui.heading(format!("Device {label}"));
                ui.label(
                    "Per-device override for the current tablet tool. \
                     Unset uses the global policy above.",
                );
                egui::ComboBox::from_id_salt("device_usage")
                    .selected_text(
                        specific.map(TabletUsage::label).unwrap_or("Use global policy"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut specific, None, "Use global policy");
                        for u in TabletUsage::VALUES {
                            ui.selectable_value(&mut specific, Some(u), u.label());
                        }
                    });
            });
            if specific != before_specific {
                match specific {
                    Some(u) => {
                        self.prefs.set_str(&key, u.as_str());
                    }
                    None => {
                        self.prefs.remove(&key);
                    }
                }
            }
        }
        let current = if self.tablet.active {
            if self.tablet.is_eraser() {
                "tablet: eraser end"
            } else {
                "tablet: pen"
            }
        } else {
            "mouse / no tablet input yet"
        };
        ui.monospace(format!("Current input: {current}"));
        ui.label(
            "Device name and axis ranges are not exposed by the windowing backend; \
             overrides are keyed by winit DeviceId, not by device name.",
        );
    }
    /// 控件在当前设置下是否可编辑。
    ///
    /// 照源码的 `set_sensitive` 联动：
    /// - 自动备份间隔只在"备份开启"时可编辑（`preferenceswindow.py:256`）；
    /// - `crosshair` 光标预设下动态十字线不可用（`:352-356`）；
    /// - 阈值滑条只在"圆环光标 + 十字线开启"时可编辑（`:349-356`）。
    fn control_enabled(&self, c: &ControlDef) -> bool {
        if !c.wired {
            return false;
        }
        let circle = self.prefs.str_or("cursor.freehand.style", "thin") != "crosshair";
        match c.id {
            "autosave_interval_spinbutton" => {
                self.prefs.bool_or("document.autosave_backups", true)
            }
            "show_crosshair_checkbutton" => circle,
            "dynamic_crosshair_threshold_spinbutton" => {
                circle && self.prefs.bool_or("cursor.dynamic_crosshair", false)
            }
            _ => true,
        }
    }

    /// 压力曲线一行 —— `CurveWidget`（`gui/curve.py`）。
    fn prefs_curve_row(&mut self, ui: &mut egui::Ui, c: &ControlDef) {
        let key = c.key.unwrap_or_default();
        let enabled = self.control_enabled(c);
        // 轴标签照 glade（`preferenceswindow.glade:145-217`）：竖排的 y 轴标签
        // 在 egui 里改为横排放上方（只有版式差异）。
        ui.label(
            egui::RichText::new(format!("{} ↑", crate::i18n::tr(AXIS_BRUSH_PRESSURE)))
                .weak()
                .small(),
        );
        let mut points = crate::shell::curve::points_from_json(self.prefs.get(key));
        let changed = ui
            .add_enabled_ui(enabled, |ui| {
                crate::shell::curve::curve_editor(
                    ui,
                    &mut points,
                    &mut self.prefs_window.curve_drag,
                )
            })
            .inner;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("0.0").weak().small());
            ui.add_space(40.0);
            ui.label(egui::RichText::new(crate::i18n::tr(AXIS_INPUT_PRESSURE)).weak().small());
            ui.add_space(40.0);
            ui.label(egui::RichText::new("1.0").weak().small());
        });
        if enabled && changed {
            self.prefs
                .set(key, crate::shell::curve::points_to_json(&points));
            self.after_wired_change();
        }
    }

    /// 复选框 / 开关一行。
    fn prefs_check_row(&mut self, ui: &mut egui::Ui, c: &ControlDef) {        let key = c.key.unwrap_or_default();
        // `compability_settings` 是**嵌套对象**：开关写的是某个模式下的
        // `pigment_on_by_default`（`compatibility.py:407-409`），不是整条键。
        if key == "compability_settings" {
            if let Some(mode) = compat_mode_control(c.id) {
                let enabled = self.control_enabled(c);
                let mut value = self.compat_settings.mode(mode).pigment_on_by_default;
                let resp = ui.add_enabled(enabled, egui::Checkbox::new(&mut value, crate::i18n::tr(c.label)));
                let resp = attach_tip(resp, c, enabled);
                if enabled && resp.changed() {
                    self.compat_settings.mode_mut(mode).pigment_on_by_default = value;
                    self.compat_settings.store(&mut self.prefs);
                    self.after_wired_change();
                }
                return;
            }
        }
        let mut value = self.prefs.bool_or(key, c.default_bool);
        let enabled = self.control_enabled(c);
        let resp = ui.add_enabled(enabled, egui::Checkbox::new(&mut value, crate::i18n::tr(c.label)));
        let resp = attach_tip(resp, c, enabled);
        if enabled && resp.changed() {
            self.prefs.set_bool(key, value);
            self.after_wired_change();
        }
    }

    /// 单选按钮一行。
    fn prefs_radio_row(&mut self, ui: &mut egui::Ui, c: &ControlDef) {
        let selected = self.radio_selected(c);
        let enabled = self.control_enabled(c);
        let resp = ui.add_enabled(enabled, egui::RadioButton::new(selected, crate::i18n::tr(c.label)));
        let resp = attach_tip(resp, c, enabled);
        if enabled && resp.clicked() && !selected {
            if let (Some(key), Some(value)) = (c.key, c.value) {
                if key == "compability_settings" {
                    // 新层默认类型：写 `pigment_layer_is_default`
                    // （`compatibility.py:403-405`）
                    if let Some(mode) = compat_mode_control(c.id) {
                        let want = c.id.contains("pigment");
                        self.compat_settings.mode_mut(mode).pigment_layer_is_default = want;
                        self.compat_settings.store(&mut self.prefs);
                        self.after_wired_change();
                    }
                    return;
                }
                if key == pigment_engine::color::PREFS_KEY_WHEEL_TYPE {
                    // 色轮类型走管理器 —— `preferenceswindow.py:326-342` 的三个
                    // 回调调的都是 `cm.set_wheel_type(...)`；写偏好是
                    // `adjbases.py:307` 的副作用，这里照做。
                    self.color_manager.set_wheel_type_by_name(value);
                }
                self.prefs.set_str(key, value);
                self.after_wired_change();
            }
        }
    }

    /// 这一行的单选按钮当前是否被选中。
    ///
    /// 色轮类型以**颜色管理器**为准：源码 `update_ui()` 读的是
    /// `cm.get_wheel_type()`（`preferenceswindow.py:242-247`），不是偏好 dict。
    fn radio_selected(&self, c: &ControlDef) -> bool {
        let Some(key) = c.key else { return false };
        let value = c.value.unwrap_or_default();
        if key == "compability_settings" {
            // `update_ui`（`compatibility.py:376-392`）读嵌套对象
            if let Some(mode) = compat_mode_control(c.id) {
                return self.compat_settings.mode(mode).pigment_layer_is_default
                    == c.id.contains("pigment");
            }
            return false;
        }
        if key == pigment_engine::color::PREFS_KEY_WHEEL_TYPE {
            self.color_manager.wheel_type_name() == value
        } else {
            self.prefs.str_or(key, "") == value
        }
    }

    /// 下拉框一行。
    fn prefs_combo_row(&mut self, ui: &mut egui::Ui, c: &ControlDef) {
        let options = options_for(c.id);
        let current = self.combo_current_id(c);
        // 当前值在选项表里找不到（或选项表为空）时显示的占位文字
        let current_label = options
            .iter()
            .find(|o| o.id == current)
            .map(|o| o.label)
            .unwrap_or(NOT_PORTED);
        let mut chosen: Option<&'static str> = None;
        let enabled = self.control_enabled(c);
        let inner = ui.add_enabled_ui(enabled, |ui| {
            egui::ComboBox::from_id_salt(c.id)
                .selected_text(current_label)
                .width(300.0)
                .show_ui(ui, |ui| {
                    for option in options {
                        if ui.selectable_label(option.id == current, option.label).clicked() {
                            chosen = Some(option.id);
                        }
                    }
                })
        });
        // 悬停说明挂在**组合框自身**的响应上：`on_disabled_hover_text` 要求
        // 响应的 `enabled` 为 false（`Tooltip::for_disabled`），而外层 scope 的
        // 响应不保证带这个旗标。
        attach_tip(inner.inner.response, c, enabled);
        if let (true, Some(value)) = (enabled, chosen) {
            if let Some(key) = c.key {
                match c.id {
                    // 偏好里是浮点，组合框的 id 是字符串（`preferenceswindow.py:287-290`）
                    "default_zoom_combobox" => {
                        if let Ok(z) = value.parse::<f64>() {
                            self.prefs.set_f64(key, z);
                        }
                    }
                    // 四个预设会顺带改写六个 `cursor.freehand.*` 键
                    // （`preferenceswindow.py:344-380`）
                    "freehand_cursor_combobox" => {
                        self.prefs.set_str(key, value);
                        apply_cursor_preset(&mut self.prefs, value);
                    }
                    _ => {
                        self.prefs.set_str(key, value);
                    }
                }
                self.after_wired_change();
            }
        }
    }

    /// 下拉框当前选中项的 id。
    fn combo_current_id(&self, c: &ControlDef) -> String {
        let key = c.key.unwrap_or_default();
        match c.id {
            // `view.default_zoom` 在偏好里是**浮点**（`userconfig.py:68`），
            // 组合框的 id 是 `"%0.2f"` 的字符串（`preferenceswindow.py:166-169`）
            "default_zoom_combobox" => zoom_id(self.prefs.f64_or(key, 1.0)),
            _ => self.prefs.str_or(key, "").to_string(),
        }
    }

    /// 单行文本输入一行。
    fn prefs_entry_row(&mut self, ui: &mut egui::Ui, c: &ControlDef) {
        let key = c.key.unwrap_or_default();
        let mut text = self.prefs.str_or(key, "").to_string();
        let enabled = self.control_enabled(c);
        let resp = ui.add_enabled(
            enabled,
            egui::TextEdit::singleline(&mut text)
                .desired_width(320.0)
                .hint_text(NOT_PORTED),
        );
        let resp = attach_tip(resp, c, enabled);
        if enabled && resp.changed() {
            self.prefs.set_str(key, &text);
            self.after_wired_change();
        }
    }

    /// 数值输入一行（`GtkSpinButton` 的技术栈适配：egui 的 `DragValue`）。
    fn prefs_spin_row(&mut self, ui: &mut egui::Ui, c: &ControlDef) {
        let key = c.key.unwrap_or_default();
        let mut value = self.prefs.i64_or(key, c.default_num as i64) as f64;
        let (lower, upper) = c.range.unwrap_or((0.0, f64::INFINITY));
        let enabled = self.control_enabled(c);
        let resp = ui.add_enabled(
            enabled,
            egui::DragValue::new(&mut value)
                .range(lower..=upper)
                .speed(1.0),
        );
        if !c.label.is_empty() {
            ui.label(crate::i18n::tr(c.label));
        }
        let resp = attach_tip(resp, c, enabled);
        if enabled && resp.changed() {
            self.prefs.set_i64(key, value.round() as i64);
            self.after_wired_change();
        }
    }

    /// 已接线控件改动之后的"应用"钩子。
    ///
    /// 源码里这些回调只写 `app.preferences`（如 `preferenceswindow.py:404-406`
    /// 的隐藏光标），消费方在**每次事件**现读偏好（`freehand.py:280` 的
    /// `.get(...)`）。我们的模式层是 `apply_settings()` 时把偏好快照进
    /// `ModePrefs`（`app.rs` 的记账），所以改完要重推一次 —— 效果等价。
    fn after_wired_change(&mut self) {
        self.apply_settings();
    }

    /// `ACCEPT` —— `preferenceswindow.py:129-132`：
    /// `save_settings()`（存盘）→ `apply_settings()`（应用）→ `hide()`（关窗）。
    fn preferences_accept(&mut self) {
        self.save_gui_config();
        self.apply_settings();
        // `:132` 的 `self.hide()`；捕获对话框跟着收
        self.prefs_window.close();
        self.key_editor.capture = None;
        self.button_editor.capture = None;
    }

    /// `REVERT` —— `preferenceswindow.py:133-135`：
    /// `load_settings()`（重读磁盘）→ `apply_settings()`。**不关窗**。
    fn preferences_revert(&mut self) {
        settings_on_revert(&mut self.prefs);
        self.apply_settings();
        // 重读磁盘后，Buttons 页的列表也要跟着回退
        // （`preferenceswindow.py:222-223` 的 `set_bindings(...)`）。
        self.reload_button_editor();
        self.prefs_window.status = Some("已从 settings.json 重新载入".to_string());
    }
}

// ---------------------------------------------------------------------------
// 绘制辅助
// ---------------------------------------------------------------------------

/// 底部按钮行要预留的高度（一行控件 + 一点间距）。
///
/// **本项目自定**：GTK 的 `GtkDialog` 把按钮放进行动区（`action_area`）由布局
/// 自己排，egui 没有那一层，只能手工让出位置。数值取 egui 的交互高度而不是
/// 抄 glade（glade 里根本没有按钮行 —— 它是 `add_buttons()` 建的）。
fn button_row_reserve(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y + 12.0
}

/// 给一行控件挂悬停说明。
///
/// `enabled` 是控件**当前**是否可编辑（已接线但在当前设置下被联动禁用时
/// 为 false）。未接线控件用 `on_disabled_hover_text` —— egui 的
/// `on_hover_text` 对禁用控件不弹（`color_panel.rs` 的页签条注过同一件事）。
fn attach_tip(resp: egui::Response, c: &ControlDef, enabled: bool) -> egui::Response {
    if enabled {
        match c.tip {
            Some(tip) => resp.on_hover_text(crate::i18n::tr(tip)),
            None => resp,
        }
    } else if c.wired {
        // 已接线但被联动禁用（自动备份间隔 / crosshair 预设下的十字线）
        let text = c
            .tip
            .map(|t| format!("{}\n当前设置下不可用", crate::i18n::tr(t)))
            .unwrap_or_else(|| "当前设置下不可用".to_string());
        resp.on_disabled_hover_text(text)
    } else {
        let mut text = String::new();
        if let Some(tip) = c.tip {
            text.push_str(crate::i18n::tr(tip));
            text.push('\n');
        }
        text.push_str(NOT_PORTED);
        if !c.reason.is_empty() {
            text.push_str("\n未移植：");
            text.push_str(c.reason);
        }
        resp.on_disabled_hover_text(text)
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    // ---- 辅助 ----

    /// 一份"什么都不读"的临时偏好（默认表 + 出厂值）。
    fn fresh_prefs(tag: &str) -> Prefs {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "pigment-prefswin-{}-{tag}/settings.json",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
        Prefs::load(p)
    }

    /// 默认表的键集合（`Prefs::defaults()`）。
    fn default_keys() -> BTreeSet<String> {
        Prefs::load(std::env::temp_dir().join("pigment-prefswin-defaults.json"))
            .defaults()
            .keys()
            .cloned()
            .collect()
    }

    fn soft_key(name: &str) -> bool {
        SOFT_KEYS.iter().any(|(k, _)| *k == name)
    }

    /// 表里用到的全部偏好键。
    fn used_keys() -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = CONTROLS.iter().filter_map(|c| c.key).collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    // ---- 页签 ----

    #[test]
    fn tab_titles_and_order_match_the_glade_notebook() {
        // glade:88 的 prefs_notebook 有 8 个 <child type="tab">，顺序 = 各
        // child 的 packing/position（0..7），标题逐字来自对应的 GtkLabel。
        let titles: Vec<&str> = TABS.iter().map(|t| t.title).collect();
        assert_eq!(
            titles,
            [
                "Pressure",
                "Devices",
                "Buttons",
                "Keys",
                "Load & Save",
                "View",
                "Color",
                "Compatibility",
            ]
        );
        assert_eq!(TABS.len(), 8, "prefs_notebook 的页数");
        // 每一页都要有源码出处
        for tab in &TABS {
            assert!(tab.source.starts_with("glade:"), "{} 缺出处", tab.title);
        }
        // 外框标题（GtkFrame 的 <child type="label">，去掉 <b> 标记）
        let headings: Vec<Option<&str>> = TABS.iter().map(|t| t.frame_heading).collect();
        assert_eq!(
            headings,
            [
                Some("Global Pressure Mapping"),
                Some("Device Usage"),
                Some("Pointer Button Mappings"),
                Some("Keyboard Mappings"),
                None,
                None,
                Some("Color Wheel"),
                Some("Compatibility settings"),
            ]
        );
        // Keys 页的下标常量与表一致
        assert_eq!(TABS[TAB_KEYS].title, "Keys");
    }

    #[test]
    fn every_control_belongs_to_a_tab_and_rows_keep_glade_order() {
        for c in CONTROLS {
            assert!(c.page < TABS.len(), "{} 的页下标越界", c.id);
            assert!(!c.id.is_empty());
            assert!(c.source.starts_with("glade:"), "{} 缺 glade 出处", c.id);
            // 未接线的行必须写原因；已接线的行不该有原因
            if c.wired {
                assert!(c.reason.is_empty(), "{} 已接线却写了未移植原因", c.id);
            } else if !matches!(c.kind, Kind::Header | Kind::Note | Kind::RowLabel) {
                assert!(!c.reason.is_empty(), "{} 未接线却没写原因", c.id);
            }
        }
        // 每页至少一行
        for page in 0..TABS.len() {
            assert!(
                CONTROLS.iter().any(|c| c.page == page),
                "第 {page} 页没有任何行"
            );
        }
    }

    #[test]
    fn control_ids_are_unique() {
        let mut seen = BTreeSet::new();
        for c in CONTROLS {
            assert!(seen.insert(c.id), "控件 id 重复：{}", c.id);
        }
        assert_eq!(CONTROLS.len(), 71, "控件/文字行总数（逐条数 glade）");
    }

    #[test]
    fn per_page_row_and_wiring_counts_match_this_block() {
        // 逐页的行数（控件 + 标题/说明文字），照 glade 各页的 attach 顺序数出来
        let per_page: Vec<usize> = (0..TABS.len())
            .map(|page| CONTROLS.iter().filter(|c| c.page == page).count())
            .collect();
        assert_eq!(per_page, [1, 2, 2, 2, 14, 25, 4, 21]);
        assert_eq!(per_page.iter().sum::<usize>(), CONTROLS.len());
        // 真正的控件（不含小节标题/说明/行标签）与静态文字行
        let widgets = CONTROLS
            .iter()
            .filter(|c| !matches!(c.kind, Kind::Header | Kind::Note | Kind::RowLabel))
            .count();
        assert_eq!(widgets, 37, "控件行数");
        assert_eq!(CONTROLS.len() - widgets, 34, "标题/说明/行标签的行数");
        // 已接线的控件行 = 30 个偏好控件 + Keys 页编辑器 + Buttons 页编辑器
        // + Color 页的 3 个单选
        let wired = CONTROLS.iter().filter(|c| c.wired).count();
        assert_eq!(wired, 37);
        assert_eq!(widgets - wired, 0, "不应再有不接线的控件行");
    }

    // ---- 控件 ↔ 偏好键映射表 ----

    #[test]
    fn every_control_key_is_in_the_default_table_or_a_soft_key() {
        // 这是"映射表完整"的门禁：键名必须在 `Prefs` 的默认表里，
        // 或者在 SOFT_KEYS（源码里以 `.get(key, default)` 读的软键）里。
        let defaults = default_keys();
        for key in used_keys() {
            assert!(
                defaults.contains(key) || soft_key(key),
                "{key} 既不在默认表也不是软键（抄错键名？）"
            );
        }
        // 反向：SOFT_KEYS 里每一条都要真的被某个控件用到，且必须**不在**默认表
        for (key, why) in SOFT_KEYS {
            let (key, why) = (*key, *why);
            assert!(!why.is_empty(), "{key} 缺源码出处");
            assert!(!defaults.contains(key), "{key} 在默认表里，不该登记成软键");
            assert!(
                CONTROLS.iter().any(|c| c.key == Some(key)),
                "{key} 登记了软键却没有控件用它"
            );
        }
    }

    #[test]
    fn wired_controls_are_exactly_the_wired_prefs() {
        let wired: BTreeSet<&str> = CONTROLS
            .iter()
            .filter(|c| c.wired)
            .filter_map(|c| c.key)
            .collect();
        assert_eq!(
            wired,
            BTreeSet::from([
                "saving.default_format",           // B6-1：save_extension
                "ui.hide_cursor_while_painting",   // B6-1：ModePrefs
                "keyboard.disable_fallbacks",      // B6-2：AccelMap::lookup
                "colors.wheels.type",              // B4-4b：ColorManager
                "input.button_mapping",            // Buttons 页：编辑器 + 指针分派
                "view.default_zoom",               // ResetView/ResetZoom + 暂存区复位
                "cursor.freehand.style",           // 光标预设（画布自绘）
                "cursor.dynamic_crosshair",        // 自绘十字线
                "cursor.dynamic_crosshair_threshold", // 自绘十字线阈值
                "ui.dark_theme_variant",           // apply_appearance（egui Visuals）
                "ui.support_smooth_scrolling",     // 画布滚轮增量来源
                "document.autosave_backups",       // AutosaveState + autosave_tick
                "document.autosave_interval",      // 同上（夹在 5..300）
                "saving.scrap_prefix",             // scrap_dir → Save/Next/Prev Scrap
                "ui.toolbar_icon_size",            // toolbars::icon_size
                "default_compatibility_mode",      // 新文档模式
                "compability_settings",            // 两套模式设置（嵌套对象）
                "compat_behavior_when_unknown",    // 打开无 EOTF 文件
                "ui.file_compat_warning_mild",     // 兼容警告开关
                "ui.file_compat_warning_severe",   // 同上
                "display.colorspace",              // ORA 是否写 mypaint:eotf
                "input.global_pressure_mapping",   // 压力曲线编辑器
                "ui.blink_layers",                 // 图层闪烁（LayerStack preview）
                "view.real_alpha_checks",          // 透明棋盘格 + GPU alpha 混合
                "lib.i18n.userlocale",             // 菜单/动作文案语言
                "input.tablet_usage",              // Devices 页：数位板工具策略
            ])
        );
        // 唯一一个没有偏好键的已接线控件是 B6-2 的加速键编辑器
        let no_key: Vec<&str> = CONTROLS
            .iter()
            .filter(|c| c.wired && c.key.is_none())
            .map(|c| c.id)
            .collect();
        assert_eq!(no_key, ["accel_map_editor"]);
    }

    #[test]
    fn disabled_keys_match_the_b6_1_accounting() {
        // B6-1（`prefs.rs` 的 "未接" 清单）与本块禁用集合的一致性：
        // 清单里凡是有对话框控件的键，都必须在这里被标成未接线。
        let disabled: BTreeSet<&str> = CONTROLS
            .iter()
            .filter(|c| !c.wired)
            .filter_map(|c| c.key)
            .collect();
        // Devices 页已接线（input.tablet_usage），不应再有禁用控件。
        assert!(disabled.is_empty(), "仍有禁用控件：{disabled:?}");
        // 没有控件代表的未接键（B6-1 清单里的其余部分）—— 记账，不是遗漏
        let no_control = [
            "workspace.layout",
            "ui.parts",
            "ui.hide_menubar_in_fullscreen",
            "ui.hide_toolbar_in_fullscreen",
            "ui.hide_subwindows_in_fullscreen",
            "ui.feedback.scale",
            "ui.feedback.last_pos",
            "ui.rendered_tile_cache_size",
            "brushmanager.selected_brush",
            "brushmanager.selected_groups",
            "colors.hcywheel.mask.*",
        ];
        let defaults = default_keys();
        for key in no_control {
            if key.ends_with(".*") {
                continue; // 通配前缀：HCY 遮罩的键是运行期算出来的
            }
            assert!(
                defaults.contains(key) || !CONTROLS.iter().any(|c| c.key == Some(key)),
                "{key} 在对话框里没有控件，不该出现在 CONTROLS"
            );
        }
    }

    #[test]
    fn checkbox_defaults_agree_with_the_default_table() {
        // 默认表里的键：源码 `p[key]` 直接取（默认表保证有），
        // 我们表里写的 `default_bool` / `default_num` 必须与默认表一致，
        // 否则键缺失时的回落会与源码不同。
        let mut prefs = fresh_prefs("defaults");
        prefs.reload();
        for c in CONTROLS {
            let Some(key) = c.key else { continue };
            let Some(default) = prefs.default_of(key) else {
                continue; // 软键：默认值来自源码的 .get(...)
            };
            match c.kind {
                Kind::Check | Kind::Switch => match default.as_bool() {
                    Some(want) => assert_eq!(
                        c.default_bool, want,
                        "{} 的 default_bool 与默认表不一致",
                        c.id
                    ),
                    // `compability_settings` 是**嵌套对象**（`compatconfig.py:39-48`：
                    // `{"1.x": {...}, "2.x": {...}}`），开关/单选写的是它下面的
                    // 子键（`compatibility.py:394-409` 的 `prefs[SETTINGS][mode].update`），
                    // 所以这里没有"整条键的布尔值"可比。
                    None => assert_eq!(
                        key, "compability_settings",
                        "{} 的默认值既不是 bool，也不是已知的嵌套对象键",
                        c.id
                    ),
                },
                Kind::Spin => {
                    let want = default
                        .as_i64()
                        .or_else(|| default.as_f64().map(|f| f as i64))
                        .expect("默认表里的数值键应是数字");
                    assert_eq!(
                        c.default_num as i64, want,
                        "{} 的 default_num 与默认表不一致",
                        c.id
                    );
                }
                _ => {}
            }
        }
    }

    // ---- 下拉框的选项表 ----

    #[test]
    fn save_format_choices_match_the_liststore() {
        // glade:14-35
        assert_eq!(
            SAVE_FORMATS.iter().map(|o| o.id).collect::<Vec<_>>(),
            ["openraster", "png-solid", "jpeg-90%"]
        );
        assert_eq!(SAVE_FORMATS[0].label, "OpenRaster (*.ora)");
        assert_eq!(SAVE_FORMATS[1].label, "PNG solid with background (*.png)");
        assert_eq!(SAVE_FORMATS[2].label, "JPEG 90% quality (*.jpg; *.jpeg)");
        // 三个 id 与 `prefs::format_extension` 的三档一一对应
        for option in SAVE_FORMATS {
            let ext = pigment_engine::prefs::format_extension(option.id);
            assert!(
                option.label.contains(ext),
                "{} 的标签应含扩展名 {ext}",
                option.id
            );
        }
    }

    #[test]
    fn zoom_choices_match_the_liststore_and_the_two_decimal_id() {
        // glade:36-61 + preferenceswindow.py:167 的 `"%0.2f" % zoom`
        assert_eq!(
            ZOOM_LEVELS.iter().map(|o| o.id).collect::<Vec<_>>(),
            ["0.25", "0.50", "1.00", "2.00"]
        );
        assert_eq!(
            ZOOM_LEVELS.iter().map(|o| o.label).collect::<Vec<_>>(),
            ["25%", "50%", "100%", "200%"]
        );
        assert_eq!(zoom_id(0.25), "0.25");
        assert_eq!(zoom_id(0.5), "0.50", "Python 的 %0.2f 会补零");
        assert_eq!(zoom_id(1.0), "1.00");
        assert_eq!(zoom_id(2.0), "2.00");
        // 每个选项的 id 都能被 zoom_id 生成（当前值的比对靠它）
        for option in ZOOM_LEVELS {
            let zoom: f64 = option.id.parse().unwrap();
            assert_eq!(&zoom_id(zoom), option.id);
        }
    }

    #[test]
    fn cursor_presets_write_and_remove_the_six_keys_like_the_source() {
        // preferenceswindow.py:344-380
        let mut prefs = fresh_prefs("cursor-preset");
        prefs.set_str("cursor.freehand.style", "medium");
        apply_cursor_preset(&mut prefs, "medium");
        assert_eq!(prefs.i64_or("cursor.freehand.min_size", 0), 5);
        assert!((prefs.f64_or("cursor.freehand.outer_line_width", 0.0) - 2.666).abs() < 1e-9);
        assert!((prefs.f64_or("cursor.freehand.inner_line_width", 0.0) - 1.333).abs() < 1e-9);
        assert_eq!(prefs.i64_or("cursor.freehand.inner_line_inset", 0), 2);
        assert_eq!(
            prefs.get_array("cursor.freehand.inner_line_color").unwrap()[3]
                .as_f64()
                .unwrap(),
            1.0
        );

        apply_cursor_preset(&mut prefs, "thick");
        assert_eq!(prefs.i64_or("cursor.freehand.min_size", 0), 7);
        assert!((prefs.f64_or("cursor.freehand.outer_line_width", 0.0) - 3.75).abs() < 1e-9);
        assert_eq!(prefs.i64_or("cursor.freehand.inner_line_inset", 0), 3);

        // thin 是出厂预设：删掉六个键（读取处回落到默认值）
        apply_cursor_preset(&mut prefs, "thin");
        for key in [
            "cursor.freehand.min_size",
            "cursor.freehand.outer_line_width",
            "cursor.freehand.inner_line_width",
            "cursor.freehand.inner_line_inset",
            "cursor.freehand.outer_line_color",
            "cursor.freehand.inner_line_color",
        ] {
            assert!(prefs.get(key).is_none(), "{key} 应被删除");
        }
        // crosshair 只改 style，不动其余键（源码没有 crosshair 分支）
        apply_cursor_preset(&mut prefs, "crosshair");
        assert!(prefs.get("cursor.freehand.min_size").is_none());
        let _ = std::fs::remove_file(prefs.path());
    }

    #[test]
    fn cursor_and_compat_choices_match_the_source() {
        // glade:62-87（freehand_cursor_liststore）
        assert_eq!(
            CURSOR_TYPES.iter().map(|o| o.id).collect::<Vec<_>>(),
            ["thin", "medium", "thick", "crosshair"]
        );
        assert_eq!(CURSOR_TYPES[0].label, "Circle with light outline");
        assert_eq!(CURSOR_TYPES[3].label, "Crosshair");
        // compatibility.py:191-224（_OPTIONS / _LABELS）
        assert_eq!(
            COMPAT_FILE_BEHAVIORS.iter().map(|o| o.id).collect::<Vec<_>>(),
            ["always-1.x", "always-2.x", "unless-pigment-layer-1.x"]
        );
        assert_eq!(
            COMPAT_FILE_BEHAVIORS[2].label,
            "Open in 1.x mode unless file contains pigment layers"
        );
        // 选项表按控件 id 索引；语言下拉是运行期生成的（空表）
        assert_eq!(options_for("default_save_format_combobox").len(), 3);
        assert_eq!(options_for("default_zoom_combobox").len(), 4);
        assert_eq!(options_for("freehand_cursor_combobox").len(), 4);
        assert_eq!(options_for("compat_file_behavior_combobox").len(), 3);
        assert_eq!(options_for("locale_combobox").len(), 3);
        assert!(options_for("nope").is_empty());
    }

    #[test]
    fn spin_ranges_match_the_glade_adjustments() {
        // `autosave_interval_adjustment`（glade:6-13：lower=5 upper=300 value=10）
        let autosave = CONTROLS
            .iter()
            .find(|c| c.id == "autosave_interval_spinbutton")
            .unwrap();
        assert_eq!(autosave.range, Some((5.0, 300.0)));
        assert_eq!(autosave.default_num, 10.0);
        // `dynamic_crosshair_threshold_spinbutton`：下界 `1 + 4//2`（
        // preferenceswindow.py:112-118），上界是 GDK 的最大光标尺寸 —— 没有对应物
        let crosshair = CONTROLS
            .iter()
            .find(|c| c.id == "dynamic_crosshair_threshold_spinbutton")
            .unwrap();
        assert_eq!(crosshair.range.map(|r| r.0), Some(3.0));
        assert_eq!(crosshair.range.map(|r| r.1), Some(f64::INFINITY));
        assert_eq!(crosshair.default_num, 8.0);
    }

    #[test]
    fn radio_values_match_the_source_strings() {
        // `colors.wheels.type` 的三档 —— adjbases.py:80-95 的表名
        let wheels: Vec<&str> = CONTROLS
            .iter()
            .filter(|c| c.key == Some(pigment_engine::color::PREFS_KEY_WHEEL_TYPE))
            .filter_map(|c| c.value)
            .collect();
        assert_eq!(wheels, ["rgb", "ryb", "rygb"]);
        // `wheel_type_name()` 产出的名字正好是这三个（比对靠它）
        for value in &wheels {
            assert!(
                pigment_engine::color::WheelType::from_name(value).is_some(),
                "{value} 不是 WheelType 的名字"
            );
        }
        // `display.colorspace` 两档（preferenceswindow.py:314-324）
        let colorspace: Vec<&str> = CONTROLS
            .iter()
            .filter(|c| c.key == Some("display.colorspace"))
            .filter_map(|c| c.value)
            .collect();
        assert_eq!(colorspace, ["srgb", "unknown"]);
        // `ui.toolbar_icon_size` 两档（preferenceswindow.py:292-300）
        let icon: Vec<&str> = CONTROLS
            .iter()
            .filter(|c| c.key == Some("ui.toolbar_icon_size"))
            .filter_map(|c| c.value)
            .collect();
        assert_eq!(icon, ["large", "small"]);
        // `default_compatibility_mode` 两档（compatconfig.py:32-33）
        let compat: Vec<&str> = CONTROLS
            .iter()
            .filter(|c| c.key == Some("default_compatibility_mode"))
            .filter_map(|c| c.value)
            .collect();
        assert_eq!(compat, ["1.x", "2.x"]);
    }

    // ---- Keys 页 ----

    #[test]
    fn keys_tab_embeds_the_b6_2_editor_and_the_fallback_checkbox() {
        // glade:439 把 AccelMapEditor 放在 Keys 页签里（不是独立窗口）
        let page: Vec<&ControlDef> = CONTROLS.iter().filter(|c| c.page == TAB_KEYS).collect();
        assert_eq!(page.len(), 2, "Keys 页 = 编辑器 + 兜底复选框");
        assert_eq!(page[0].kind, Kind::AccelEditor);
        assert!(page[0].wired, "B6-2 的编辑器已移植");
        assert_eq!(page[0].key, None, "编辑器改的是 accelmap.conf，不是偏好键");
        assert_eq!(page[1].kind, Kind::Check);
        assert_eq!(page[1].key, Some("keyboard.disable_fallbacks"));
        assert!(page[1].wired);
        assert_eq!(page[1].label, "Disable fallback key bindings");
        // 兜底开关的默认值是 false（preferenceswindow.py:151 的 .get(key, False)）
        assert!(!page[1].default_bool);
    }

    #[test]
    fn window_chrome_matches_the_source() {
        // 标题与两个按钮（preferenceswindow.py:42/46-51）；
        // 源码只有 REVERT 与 ACCEPT，**没有** Cancel
        assert_eq!(TITLE, "Preferences");
        assert_eq!(BUTTON_REVERT, "Revert to Saved");
        assert_eq!(BUTTON_OK, "OK");
        assert!(!["Cancel", "Close", "Apply"].contains(&BUTTON_OK));
        // 禁用控件的措辞（与 color_panel 的未移植页签同款）
        assert!(NOT_PORTED.contains("未移植"));
    }

    // ---- 三条路径 ----

    #[test]
    fn accept_writes_the_settings_file() {
        // preferenceswindow.py:130 的 `app.save_settings()`（application.py:498-512）
        let mut prefs = fresh_prefs("accept");
        prefs.set_str("saving.default_format", "png-solid");
        assert!(!prefs.path().exists(), "还没存过盘");
        settings_on_accept(&prefs).unwrap();
        assert!(prefs.path().is_file(), "ACCEPT 要落盘");
        // 重新读：新值在
        let reloaded = Prefs::load(prefs.path());
        assert_eq!(reloaded.str_or("saving.default_format", "?"), "png-solid");
        let _ = std::fs::remove_dir_all(prefs.path().parent().unwrap());
    }

    #[test]
    fn revert_reloads_from_disk_and_drops_unsaved_changes() {
        // preferenceswindow.py:134 的 `app.load_settings()`（application.py:524-539）
        let mut prefs = fresh_prefs("revert");
        prefs.set_str("saving.default_format", "png-solid");
        settings_on_accept(&prefs).unwrap();
        // 改了但没 ACCEPT
        prefs.set_str("saving.default_format", "jpeg-90%");
        prefs.set_bool("view.real_alpha_checks", false);
        settings_on_revert(&mut prefs);
        assert_eq!(
            prefs.str_or("saving.default_format", "?"),
            "png-solid",
            "REVERT 读的是磁盘上的值"
        );
        assert!(
            prefs.bool_or("view.real_alpha_checks", false),
            "未保存的改动被丢掉，回到磁盘/默认表的值"
        );
        // REVERT **不**存盘：磁盘上还是旧值
        assert_eq!(
            Prefs::load(prefs.path()).str_or("saving.default_format", "?"),
            "png-solid"
        );
        let _ = std::fs::remove_dir_all(prefs.path().parent().unwrap());
    }

    #[test]
    fn revert_without_a_file_falls_back_to_the_default_table() {
        // gui/userconfig.py:42-47：读不到文件 → 只用默认表
        let mut prefs = fresh_prefs("revert-missing");
        prefs.set_str("saving.default_format", "png-solid");
        settings_on_revert(&mut prefs);
        assert_eq!(prefs.str_or("saving.default_format", "?"), "openraster");
        assert_eq!(prefs.len(), 33, "回到完整默认表");
    }

    #[test]
    fn closing_keeps_the_changes_and_writes_nothing() {
        // windowing.py:43 的 `hide_on_delete()`：只藏窗口
        let mut prefs = fresh_prefs("close");
        settings_on_accept(&prefs).unwrap();
        let before = std::fs::read_to_string(prefs.path()).unwrap();
        // 关窗之前用户改了一条（回调已经写进 app.preferences）
        prefs.set_str("saving.default_format", "png-solid");

        let mut state = PrefsWindowState {
            open: true,
            tab: 5,
            status: None,
            curve_drag: None,
        };
        state.close();
        assert!(!state.open);
        assert_eq!(state.tab, 5, "关窗不动页签");
        assert!(state.status.is_none());
        // 关窗既不存盘也不回退：磁盘没变、内存里的改动还在
        assert_eq!(std::fs::read_to_string(prefs.path()).unwrap(), before);
        assert_eq!(
            prefs.str_or("saving.default_format", "?"),
            "png-solid",
            "关窗不回退（源码只 hide；退出时才由 save_gui_config 落盘）"
        );
        let _ = std::fs::remove_dir_all(prefs.path().parent().unwrap());
    }

    #[test]
    fn default_state_is_closed_on_the_first_tab() {
        let state = PrefsWindowState::default();
        assert!(!state.open, "启动时不显示偏好窗口");
        assert_eq!(state.tab, 0, "默认停在第一页（GtkNotebook 的初值）");
        assert!(state.status.is_none());
    }
}
