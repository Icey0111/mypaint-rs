//! 用户偏好 —— `gui/userconfig.py`（默认键表 + `settings.json` 读取）
//! 与 `gui/application.py`（`load_settings` / `save_settings` / `save_gui_config`）。
//!
//! # 先纠正一个前提：唯一事实来源里没有 `lib/settings.py`
//!
//! B6-1 的任务书提到 `lib/settings.py` 的 `Settings` 模型（`sync_pending_changes`、
//! JSON 读写、未知键保留）。`_mypaint_ref/` 里**不存在**这个文件：
//! 本仓库的 MyPaint 版本（master，`fb6f7d6`）里"文档内设置"只是
//! `lib/document.py:343-344` 的 `ObservableDict` + `:448-489` 的 `settings`
//! 属性，它的 JSON 往返已经由 [`crate::document`] 实现（`set_settings_json` /
//! `settings_json`）。`lib/settings.py` 是 1.x 的结构，源码里没有可对照的行。
//!
//! 所以本模块**只照有源码的两处**实现，不伪造一个 `sync_pending_changes`：
//!
//! | 语义 | 源码 |
//! |---|---|
//! | 完整默认键表（含平台分支） | `gui/userconfig.py:50-140` |
//! | `settings.json` 的读取（UTF-8、失败回空字典） | `gui/userconfig.py:25-47` |
//! | 默认表 + 用户文件**浅覆盖**、旧配置修补 | `gui/application.py:524-539` |
//! | 写回（`json.dumps(preferences, indent=2)`，直接覆写） | `gui/application.py:498-512` |
//! | 退出时保存（`save_gui_config`） | `gui/application.py:624-628` |
//! | 偏好对话框 ACCEPT → 保存 + 应用 | `gui/preferenceswindow.py:128-132` |
//!
//! # `settings.json` 在哪
//!
//! 源码的 `user_confpath` 是 GLib 的 user config dir 再拼 `mypaint`
//! （`gui/main.py:120-126`），`settings.json` 就在它下面
//! （`gui/application.py:503`；读取侧 `gui/userconfig.py:32`）：
//!
//! - Windows：`%APPDATA%\mypaint\settings.json`
//!   （GLib 在 Windows 上 config 与 data 同源，`main.py:98-101`）
//! - POSIX：`$XDG_CONFIG_HOME/mypaint/settings.json`，
//!   `$XDG_CONFIG_HOME` 未设时 `~/.config/mypaint/settings.json`
//!
//! 路径由 [`settings_path`] 算；`PIGMENT_CONFIG_DIR`（覆盖 `<user_config>/mypaint`）
//! 与 `PIGMENT_SETTINGS_PATH`（覆盖整条文件路径）是**应用本地扩展**，
//! 与 `PIGMENT_SCRATCHPAD_DIR` / `PIGMENT_BRUSH_PATH` 同风格，源码没有这两个开关。
//!
//! # 写入策略：**直接覆写，没有临时文件**
//!
//! 源码 `save_settings()` 就是 `open(settingspath, "wb")` + `f.write(json_data)`
//! （`gui/application.py:511-512`），不带临时文件、不 rename、不 fsync。
//! 所以这里也直接 [`std::fs::write`] —— **不**自创"原子写"。
//!
//! # 未知键保留
//!
//! `load_settings()` 把整份用户 dict `update()` 进默认表（`application.py:533-534`），
//! `save_settings()` 再把整张表 `json.dumps` 回去（`:505`）。于是源码天然保留
//! 它不认识的键（扩展/新版本写的键）。这里同样：整个 dict 存在
//! [`Prefs::values`] 里，[`Prefs::set`] 只动被点名的那一条。
//!
//! # 与源码的差异（都往保守方向）
//!
//! - **类型化访问器**：源码直接 `prefs["document.autosave_interval"]`，缺失或类型
//!   不对会 `KeyError`/`TypeError`。本模块的 `*_or()` 把这两种情况都回落成调用方
//!   给的默认值（`gui/preferenceswindow.py:104-109` 那种 `.get(key, default)` 用法
//!   是源码里就有的保守写法）。
//! - **键的顺序**：源码 `json.dumps` 按 dict 插入顺序写；这里用 `BTreeMap`，
//!   落盘键序是字典序（语义无关，`json.loads` 回来一样）。
//! - **非 ASCII 转义**：`json.dumps` 默认 `ensure_ascii=True`（\uXXXX），
//!   `serde_json` 写原始 UTF-8。解析回来的字符串相同。
//! - **Windows 的「文档」目录**：源码用 GLib 的 `DIRECTORY_DOCUMENTS`
//!   （`userconfig.py:55-57`，走 `SHGetKnownFolderPath`，会被 OneDrive 重定向）。
//!   这里没有 Win32 API 可调，用约定的 `%USERPROFILE%\Documents`；拿不到
//!   `USERPROFILE` 时退回非 Windows 分支的 `~/MyPaint/scrap`（源码那种情况会
//!   因 `os.path.join(None, ...)` 抛 `TypeError`，我们不去制造崩溃）。
//!
//! # 为什么放在引擎层（而不是 `pigment-app`）
//!
//! 1. **消费方几乎全在引擎**：[`crate::color::ColorManager`]（`colors.*` /
//!    `misc.context_restores_color`）、[`crate::mode::freehand`]（桶旋转三键 +
//!    隐藏光标）、[`crate::scratchpad`]（`saving.default_format` 的扩展名）都
//!    在这里。放 app 会让引擎反向依赖应用层，或者把同一套键读两遍。
//! 2. **平台判定引擎里已经有了**：[`crate::scratchpad::user_data_dir`] 就是
//!    `cfg!(windows)` + `%APPDATA%` / `$XDG_DATA_HOME` 的同一套规则；
//!    `settings.json` 的路径（`gui/main.py:120-126`）与暂存区目录同源，放一处
//!    才不会两套平台判断打架。
//! 3. **JSON 依赖已在引擎**（`serde_json`，`Cargo.toml` 里本来就有；文档
//!    I/O 的 `data/mypaint-settings.json` 也用它），不需要给 app 新增依赖。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

// ---- 文件名与目录（逐条标源码行号）----

/// `settings.json` —— `gui/userconfig.py:32` / `gui/application.py:503`。
pub const SETTINGS_FILENAME: &str = "settings.json";
/// 用户数据/配置目录下的应用子目录 —— `gui/main.py:122` 的 `appsubdir = "mypaint"`。
pub const APP_SUBDIR: &str = "mypaint";

/// 覆盖 `<user_config>/mypaint` 这个应用配置目录 —— **应用本地扩展**。
pub const CONFIG_DIR_ENV: &str = "PIGMENT_CONFIG_DIR";
/// 覆盖整条 `settings.json` 路径 —— **应用本地扩展**。
pub const SETTINGS_PATH_ENV: &str = "PIGMENT_SETTINGS_PATH";

// ---- 本块接线的偏好键（键名照抄；默认值/取值处在各行注明）----

/// 暂存页自动存档的路径 —— `gui/userconfig.py:102`（默认 `""`）；
/// 读取处 `gui/application.py:475-480`，写回处 `filehandling.py:965-974`、`:1034-1036`。
pub const KEY_LAST_OPENED_SCRATCHPAD: &str = "scratchpad.last_opened_scratchpad";
/// 新文件/新建暂存页的扩展名来源 —— `gui/userconfig.py:89`（默认 `"openraster"`）；
/// 使用处 `gui/filehandling.py:1408-1411`、`:1301-1302`。
pub const KEY_SAVING_DEFAULT_FORMAT: &str = "saving.default_format";
/// 画的时候隐藏系统光标 —— **默认表里没有这个键**（软键）：读取处
/// `gui/freehand.py:280`、对话框 `gui/preferenceswindow.py:202` 都是 `.get(..., False)`。
pub const KEY_HIDE_CURSOR_WHILE_PAINTING: &str = "ui.hide_cursor_while_painting";
/// 桶旋转偏移 —— `gui/userconfig.py:67`（默认 0.5）；读取处 `gui/freehand.py:461`。
pub const KEY_BARREL_ROTATION_OFFSET: &str = "input.barrel_rotation_offset";
/// 桶旋转是否减去倾角方位角 —— `gui/userconfig.py:66`（默认 true）；
/// 读取处 `gui/freehand.py:467-469`。
pub const KEY_BARREL_ROTATION_SUBTRACT_ASCENSION: &str =
    "input.barrel_rotation_subtract_ascension";
/// 是否使用桶旋转 —— `gui/userconfig.py:65`（默认 true）；读取处 `gui/freehand.py:475-477`。
pub const KEY_USE_BARREL_ROTATION: &str = "input.use_barrel_rotation";
/// 对称轴线透明度 —— **默认表里没有这个键**（软键）：默认值
/// `gui/symmetry.py:46` 的 `_DEFAULT_ALPHA = 0.333`，读取处 `:648-649`（`.get(key, 0.333)`）。
pub const KEY_SYMMETRY_LINE_ALPHA: &str = "symmetry.line_alpha";
/// 画框颜色（RGBA）—— `gui/userconfig.py:92`（默认 `(0.12, 0.12, 0.12, 0.92)`）；
/// 读取处 `gui/framewindow.py:502-505` / `:826`，写回处 `:618`。
pub const KEY_FRAME_COLOR_RGBA: &str = "frame.color_rgba";
/// 是否禁用硬编码兜底键位 —— **默认表里没有这个键**（软键）：读取处
/// `gui/keyboard.py:159-160` 的 `fallbacks_disabled()`（`.get(..., False)`），
/// 复选框在偏好对话框的 "Keys" 页签（`gui/preferenceswindow.glade:451-466`，
/// 初值见 `gui/preferenceswindow.py:151`，写回见 `:265`）。
pub const KEY_DISABLE_FALLBACKS: &str = "keyboard.disable_fallbacks";

/// 平台分支 —— `gui/userconfig.py:54` 的 `sys.platform == "win32"`。
///
/// 做成显式参数（而不是只靠 `cfg!(windows)`）是为了让默认表门禁能在**同一个
/// 构建**里同时对照两个分支的 fixture（fixture 由源码函数跑两次生成）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// `sys.platform == "win32"` 那支。
    Windows,
    /// 其余平台（源码的 else 支）。
    Posix,
}

/// 当前构建跑在哪个分支上（等价 `sys.platform == "win32"`）。
pub fn current_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Posix
    }
}

// ---- 目录解析（GLib 的 user dirs，`gui/main.py:120-126`）----

/// GLib 的 user **config** dir（`g_get_user_config_dir`，`gui/main.py:125`）。
///
/// - Windows：`%APPDATA%`，退回 `%LOCALAPPDATA%`（GLib 在 Windows 上
///   config 与 data 同源，`main.py:98-101`）
/// - POSIX：`$XDG_CONFIG_HOME`，否则 `$HOME/.config`
pub fn user_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        for var in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(d) = std::env::var_os(var) {
                return Some(PathBuf::from(d));
            }
        }
        None
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
}

/// Windows 的「文档」目录 —— GLib `DIRECTORY_DOCUMENTS` 的保守替身
/// （`gui/userconfig.py:55-57`）。
///
/// 只用 `%USERPROFILE%\Documents`（没有 Win32 API 可调，见模块文档）。
pub fn documents_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents"))
}

/// 路径解析的**纯函数**版（不读环境变量，便于测试）。
///
/// 优先级：`settings_path_override` > `config_dir_override` >
/// `<user_config>/mypaint` > 系统临时目录下的固定兜底。
pub fn resolve_settings_path(
    config_dir_override: Option<&Path>,
    settings_path_override: Option<&Path>,
    user_config: Option<&Path>,
) -> PathBuf {
    if let Some(p) = settings_path_override {
        return p.to_path_buf();
    }
    let dir = match config_dir_override {
        Some(d) => d.to_path_buf(),
        None => match user_config {
            Some(c) => c.join(APP_SUBDIR),
            // 环境变量被剥光的沙箱里才会走到；与 `scratchpad::default_dir` 同风格。
            None => std::env::temp_dir().join("pigment-config"),
        },
    };
    dir.join(SETTINGS_FILENAME)
}

/// 应用配置目录（`settings.json` 与 `accelmap.conf` 都在这；`gui/main.py:125-126`）。
///
/// 即 `resolve_settings_path(...)` 的"目录"部分；`accelmap.conf` 也在同一层
/// （`gui/application.py:417` / `:625`），所以单独暴露给 B6-2 的按键编辑器用。
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os(CONFIG_DIR_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    match user_config_dir() {
        Some(c) => c.join(APP_SUBDIR),
        None => std::env::temp_dir().join("pigment-config"),
    }
}

/// `settings.json` 的完整路径（真实入口，读环境变量）。
pub fn settings_path() -> PathBuf {
    let env_dir = std::env::var_os(CONFIG_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let env_path = std::env::var_os(SETTINGS_PATH_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    resolve_settings_path(
        env_dir.as_deref(),
        env_path.as_deref(),
        user_config_dir().as_deref(),
    )
}

// ---- 默认键表（`gui/userconfig.py:50-140`）----

/// Linux 支的 `input.button_mapping` —— `gui/userconfig.py:111-124`。
///
/// 注释掉的三行（`<Shift>Button1` 等）在源码里也是注释，照抄其"不存在"。
fn linux_button_mapping() -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    let entries = [
        ("Button2", "PanViewMode"),
        ("<Shift>Button2", "RotateViewMode"),
        ("<Control>Button2", "ZoomViewMode"),
        ("<Alt>Button2", "ZoomViewMode"),
        ("<Control><Shift>Button2", "FrameEditMode"),
        ("<Alt><Shift>Button2", "FrameEditMode"),
        ("Button3", "ShowPopupMenu"),
    ];
    for (bp, act) in entries {
        m.insert(bp.to_string(), json!(act));
    }
    m
}

/// Windows 支的 `input.button_mapping` —— `gui/userconfig.py:127-139`。
///
/// 逐键做 `Button2 → ButtonTMP → Button3`、`Button3 → Button2` 的互换
/// （Linux 的 wacom 驱动把笔侧键号反了，`:128-132` 的注释）。
fn windows_button_mapping() -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    for (bp, act) in linux_button_mapping() {
        // 与源码同序的三个 replace（中间的 TMP 防止两次替换互相踩）
        let bp = bp.replace("Button2", "ButtonTMP");
        let bp = bp.replace("Button3", "Button2");
        let bp = bp.replace("ButtonTMP", "Button3");
        m.insert(bp, act);
    }
    m
}

/// `saving.scrap_prefix` 的 Windows 值 —— `os.path.join(ud_docs, "MyPaint", "scrap")`
/// （`gui/userconfig.py:55-58`）。
///
/// 用 `\` 拼（Windows 路径），去掉文档目录末尾的分隔符（否则会出现 `Documents\\MyPaint`）。
fn windows_scrap_prefix(docs: &Path) -> String {
    let s = docs.to_string_lossy();
    let s = s.trim_end_matches(['\\', '/']);
    format!("{s}\\MyPaint\\scrap")
}

/// 完整默认键表 —— `gui/userconfig.py:50-140` 的 `default_configuration()`。
///
/// `platform` 选择平台分支（`sys.platform == "win32"`），`documents_dir` 是
/// Windows 支的「文档」目录（`lib.glib.get_user_special_dir(DIRECTORY_DOCUMENTS)`）。
/// 返回 33 条键：30 条来自 `:61-125` 的字面量，3 条来自 `COMPAT_CONFIG`
/// （`gui/compatconfig.py:36-49`，`userconfig.py:126` 的 `update()`）。
///
/// Windows 支只改**两个值**、不增删键：`saving.scrap_prefix`（`:54-58`）与
/// `input.button_mapping`（`:127-139`）。
pub fn default_configuration(
    platform: Platform,
    documents_dir: Option<&Path>,
) -> BTreeMap<String, Value> {
    let scrappre = match (platform, documents_dir) {
        (Platform::Windows, Some(docs)) => windows_scrap_prefix(docs),
        // 拿不到 USERPROFILE 时的保守兜底（源码会抛 TypeError，见模块文档）。
        _ => "~/MyPaint/scrap".to_string(),
    };
    let button_mapping = match platform {
        Platform::Windows => windows_button_mapping(),
        Platform::Posix => linux_button_mapping(),
    };

    let mut c: BTreeMap<String, Value> = BTreeMap::new();
    // `:62`
    c.insert("saving.scrap_prefix".into(), json!(scrappre));
    // `:63`
    c.insert("input.device_mode".into(), json!("screen"));
    // `:64`
    c.insert(
        "input.global_pressure_mapping".into(),
        json!([[0.0, 1.0], [1.0, 0.0]]),
    );
    // `:65-67`
    c.insert("input.use_barrel_rotation".into(), json!(true));
    c.insert(
        "input.barrel_rotation_subtract_ascension".into(),
        json!(true),
    );
    c.insert("input.barrel_rotation_offset".into(), json!(0.5));
    // `:68-69`
    c.insert("view.default_zoom".into(), json!(1.0));
    c.insert("view.real_alpha_checks".into(), json!(true));
    // `:70-72`
    c.insert("ui.hide_menubar_in_fullscreen".into(), json!(true));
    c.insert("ui.hide_toolbar_in_fullscreen".into(), json!(true));
    c.insert("ui.hide_subwindows_in_fullscreen".into(), json!(true));
    // `:73`
    c.insert(
        "ui.parts".into(),
        json!({"main_toolbar": true, "menubar": true}),
    );
    // `:74-75`
    c.insert("ui.feedback.scale".into(), json!(false));
    c.insert("ui.feedback.last_pos".into(), json!(false));
    // `:76-85`
    c.insert(
        "ui.toolbar_items".into(),
        json!({
            "toolbar1_file": true,
            "toolbar1_scrap": false,
            "toolbar1_edit": true,
            "toolbar1_blendmodes": false,
            "toolbar1_linemodes": true,
            "toolbar1_view_modes": true,
            "toolbar1_view_manips": false,
            "toolbar1_view_resets": true,
        }),
    );
    // `:86-88`
    c.insert("ui.toolbar_icon_size".into(), json!("large"));
    c.insert("ui.dark_theme_variant".into(), json!(true));
    c.insert("ui.rendered_tile_cache_size".into(), json!(16384));
    // `:89-91`
    c.insert("saving.default_format".into(), json!("openraster"));
    c.insert("brushmanager.selected_brush".into(), Value::Null);
    c.insert("brushmanager.selected_groups".into(), json!([]));
    // `:92`
    c.insert("frame.color_rgba".into(), json!([0.12, 0.12, 0.12, 0.92]));
    // `:93-95`
    c.insert("misc.context_restores_color".into(), json!(true));
    c.insert("document.autosave_backups".into(), json!(true));
    c.insert("document.autosave_interval".into(), json!(10));
    // `:97-98`（`display.colorspace_EOTF` 的出厂值是 `lib/eotf.py:16` 的
    // `DEFAULT_EOTF = 2.2`）
    c.insert("display.colorspace_EOTF".into(), json!(2.2));
    c.insert("display.colorspace".into(), json!("srgb"));
    // `:102`
    c.insert(KEY_LAST_OPENED_SCRATCHPAD.into(), json!(""));
    // `:104-107`
    c.insert(
        "workspace.layout".into(),
        json!({
            "position": {"x": 50, "y": 32, "w": -50, "h": -100},
            "autohide": true,
        }),
    );
    // `:111-124` / Windows 支 `:127-139`
    c.insert(
        "input.button_mapping".into(),
        Value::Object(button_mapping.into_iter().collect()),
    );
    // `:126`：`default_config.update(COMPAT_CONFIG)` —— 兼容模式的三条键
    // （`gui/compatconfig.py:36-49`；注意源码把 `COMPAT_SETTINGS` 拼成
    // `compability_settings`，拼写照抄）。
    c.insert(
        "compat_behavior_when_unknown".into(),
        json!("unless-pigment-layer-1.x"),
    );
    c.insert("default_compatibility_mode".into(), json!("2.x"));
    c.insert(
        "compability_settings".into(),
        json!({
            "1.x": {"pigment_on_by_default": false, "pigment_layer_is_default": false},
            "2.x": {"pigment_on_by_default": true, "pigment_layer_is_default": true},
        }),
    );
    c
}

// ---- `saving.default_format` → 扩展名（`gui/filehandling.py:498-602`+`:1409-1411`）----

/// `config2saveformat` 的三档取值 —— `gui/filehandling.py:598-602`。
///
/// 源码是 `self.saveformats[self.config2saveformat[cfg]][1]`（`:1409-1411`），
/// 即 `saveformats`（`:500-590`）里的扩展名：`openraster → ".ora"`、
/// `jpeg-90% → ".jpg"`（`:575-585` 的 "JPEG 90% quality (*.jpg; *.jpeg)" 那项
/// 取的是 `.jpg`）、`png-solid → ".png"`。
///
/// 未知取值源码会 `KeyError`；这里保守回落到出厂格式 `.ora`（模块文档的
/// "差异"一节）。
pub fn format_extension(config: &str) -> &'static str {
    match config {
        "jpeg-90%" => ".jpg",
        "png-solid" => ".png",
        _ => ".ora",
    }
}

/// 当前偏好的默认扩展名（带点，如 `".ora"`）。
pub fn save_extension(prefs: &Prefs) -> &'static str {
    format_extension(prefs.str_or(KEY_SAVING_DEFAULT_FORMAT, "openraster"))
}

// ---- Prefs 本体 ----

/// 用户偏好的运行时形态 —— `app.preferences`（`gui/application.py:280` 的
/// `ObservableDict`）的替身。
///
/// 内部就是**一整张 dict**：默认表 + 用户文件的浅覆盖（`load_settings`，
/// `application.py:524-539`），保存时整张写回（`save_settings`，`:498-512`），
/// 于是未知键天然保留。
#[derive(Clone, Debug)]
pub struct Prefs {
    /// `settings.json` 的路径（`gui/application.py:503`）。
    path: PathBuf,
    /// 当前偏好（默认表 + 用户文件 + 本会话改动）。
    values: BTreeMap<String, Value>,
    /// 本平台的完整默认表（`default_configuration()` 的结果，重置时用）。
    defaults: BTreeMap<String, Value>,
}

impl Prefs {
    /// 载入 `path`：默认表 → 用户文件浅覆盖 → 旧配置修补（`application.py:524-539`）。
    ///
    /// 文件不存在/读不出/不是 JSON 对象时只用默认表（源码 `get_json_config()`
    /// 也是记日志后回 `{}`，`gui/userconfig.py:42-47`）。
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let mut prefs = Self {
            path: path.into(),
            values: BTreeMap::new(),
            defaults: default_configuration(current_platform(), documents_dir().as_deref()),
        };
        prefs.reload();
        prefs
    }

    /// 用一份现成的 dict 建偏好（默认表 + 覆盖）—— 不碰文件系统，测试与
    /// "偏好来自别处"的构造用。
    pub fn from_values(values: BTreeMap<String, Value>) -> Self {
        Self {
            path: settings_path(),
            values,
            defaults: default_configuration(current_platform(), documents_dir().as_deref()),
        }
    }

    /// 重新从磁盘载入（偏好对话框的 Revert：`gui/preferenceswindow.py:133-135`
    /// 的 `app.load_settings()`）。
    pub fn reload(&mut self) {
        // `application.py:531-532`：`preferences.clear()` + 默认表 copy
        self.values = self.defaults.clone();
        // `application.py:533-534`：`preferences.update(user_config)`（浅覆盖）
        let user = read_json_object(&self.path);
        self.values.extend(user);
        self.fix_legacy_button_mapping();
    }

    /// 旧配置修补 —— `gui/application.py:535-539`。
    ///
    /// 老版本把 `ColorPickerPopup` 当成按键映射的一个动作；升级后
    /// Ctrl-Click 取色会坏，所以整张映射重置回默认表。
    fn fix_legacy_button_mapping(&mut self) {
        let key = "input.button_mapping";
        let legacy = match self.values.get(key) {
            Some(Value::Object(m)) => m
                .values()
                .any(|v| v.as_str() == Some("ColorPickerPopup")),
            _ => false,
        };
        if legacy {
            if let Some(default) = self.defaults.get(key) {
                self.values.insert(key.to_string(), default.clone());
            }
        }
    }

    /// `settings.json` 路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 改路径（"载入另一份偏好"用；`Prefs::load` 之外只有测试与宿主会用到）。
    pub fn set_path(&mut self, path: impl Into<PathBuf>) {
        self.path = path.into();
    }

    /// 当前偏好整张表（`app.preferences`）。
    pub fn values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }

    /// 本平台的默认表（`default_configuration()` 的结果）。
    pub fn defaults(&self) -> &BTreeMap<String, Value> {
        &self.defaults
    }

    /// 某条键的出厂值（没有这条键时 `None`）。
    pub fn default_of(&self, key: &str) -> Option<&Value> {
        self.defaults.get(key)
    }

    /// 保存 —— `gui/application.py:498-512` 的 `save_settings()`：
    /// `json.dumps(preferences, indent=2)` 后**直接覆写**同一个文件。
    ///
    /// 目录不存在时先建（源码在启动时建，`application.py:228-231`；
    /// 效果等价，但省得调用方各自记得先建）。
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let obj = Value::Object(self.values.clone().into_iter().collect());
        let json = serde_json::to_string_pretty(&obj)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // `:511-512`：源码直接覆写，无临时文件（见模块文档）。
        std::fs::write(&self.path, json)
    }

    // ---- 原始读写（对应 dict 的 `[]` / `in` / `.pop()`）----

    /// `prefs[key]`（缺失 → `None`）。
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// `key in prefs`（`gui/preferenceswindow.py:84` 那种断言）。
    pub fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// 全部键（顺序是字典序，见模块文档）。
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.values.keys()
    }

    /// 键数。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否一条键都没有（`Prefs` 永远有默认表，正常构建下不成立）。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// `prefs[key] = value`（`gui/preferenceswindow.py:285` 那种写法）。
    pub fn set(&mut self, key: &str, value: Value) -> Option<Value> {
        self.values.insert(key.to_string(), value)
    }

    /// `prefs[key] = True/False`。
    pub fn set_bool(&mut self, key: &str, value: bool) -> Option<Value> {
        self.set(key, json!(value))
    }

    /// `prefs[key] = <int>`。
    pub fn set_i64(&mut self, key: &str, value: i64) -> Option<Value> {
        self.set(key, json!(value))
    }

    /// `prefs[key] = <float>`。
    pub fn set_f64(&mut self, key: &str, value: f64) -> Option<Value> {
        self.set(key, json!(value))
    }

    /// `prefs[key] = "<str>"`。
    pub fn set_str(&mut self, key: &str, value: impl Into<String>) -> Option<Value> {
        self.set(key, json!(value.into()))
    }

    /// `prefs[key] = [<str>, ...]`（颜色历史那种列表）。
    pub fn set_str_array<'a>(
        &mut self,
        key: &str,
        values: impl IntoIterator<Item = &'a str>,
    ) -> Option<Value> {
        let arr: Vec<Value> = values.into_iter().map(|s| json!(s)).collect();
        self.set(key, Value::Array(arr))
    }

    /// `prefs.pop(key, None)`（`gui/preferenceswindow.py:361-366`）。
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.values.remove(key)
    }

    // ---- 类型化访问器（缺失/类型不对 → 回落；见模块文档"差异"）----

    /// 取 bool；缺失或不是 bool → `None`。
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.values.get(key)?.as_bool()
    }

    /// 取 bool，缺失/类型不对回落 `fallback`。
    pub fn bool_or(&self, key: &str, fallback: bool) -> bool {
        self.get_bool(key).unwrap_or(fallback)
    }

    /// 取整数；JSON 的浮点也算（`10` 与 `10.0` 都收，因为源码只要求
    /// `int(...)` 能过——`gui/preferenceswindow.py:253` 就是 `int(p[...])`）。
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        let v = self.values.get(key)?;
        v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
    }

    /// 取整数，回落。
    pub fn i64_or(&self, key: &str, fallback: i64) -> i64 {
        self.get_i64(key).unwrap_or(fallback)
    }

    /// 取浮点；JSON 的整数也收（`serde_json` 的 `as_f64` 对整数有效）。
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.values.get(key)?.as_f64()
    }

    /// 取浮点，回落。
    pub fn f64_or(&self, key: &str, fallback: f64) -> f64 {
        self.get_f64(key).unwrap_or(fallback)
    }

    /// 取字符串；不是字符串 → `None`。
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.values.get(key)?.as_str()
    }

    /// 取字符串，回落。
    /// `input.button_mapping` 的 `(键串, 动作)` 列表（只保留字符串值）。
    ///
    /// 键串形如 `<Control><Shift>Button2`；解析在应用的
    /// `shell::button_mapping`（引擎不依赖 egui）。
    pub fn button_mapping_pairs(&self) -> Vec<(String, String)> {
        let Some(Value::Object(map)) = self.get("input.button_mapping") else {
            return Vec::new();
        };
        map.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect()
    }

    /// 写回 `input.button_mapping`（`gui/buttonmap.py:403-415` 的
    /// `_liststore_updated_cb`：整张 dict 用编辑器里的行重建）。
    ///
    /// 键串保持调用方给的形式（编辑器的 `button_press_name()` 已归一化）；
    /// 值必须是动作名，否则跳过。
    pub fn set_button_mapping_pairs(
        &mut self,
        pairs: impl IntoIterator<Item = (String, String)>,
    ) -> Option<Value> {
        let mut map = serde_json::Map::new();
        for (key, action) in pairs {
            let key = key.trim();
            let action = action.trim();
            if key.is_empty() || action.is_empty() {
                continue;
            }
            map.insert(key.to_string(), json!(action));
        }
        self.set("input.button_mapping", Value::Object(map))
    }

    pub fn str_or<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.get_str(key).unwrap_or(fallback)
    }

    /// 取数组；不是数组 → `None`。
    pub fn get_array(&self, key: &str) -> Option<&Vec<Value>> {
        self.values.get(key)?.as_array()
    }

    /// 取数组，回落（返回借用，缺失时借用 `fallback`）。
    pub fn array_or<'a>(&'a self, key: &str, fallback: &'a [Value]) -> &'a [Value] {
        self.get_array(key)
            .map(Vec::as_slice)
            .unwrap_or(fallback)
    }

    /// 写一个浮点数组（例如 `frame.color_rgba`）。
    pub fn set_f64_array(&mut self, key: &str, values: &[f64]) -> Option<Value> {
        self.set(key, serde_json::json!(values))
    }

    /// 取对象；不是对象 → `None`。
    pub fn get_object(&self, key: &str) -> Option<&serde_json::Map<String, Value>> {
        self.values.get(key)?.as_object()
    }
}

/// 读 `settings.json` —— `gui/userconfig.py:25-47` 的 `get_json_config()`。
///
/// 读失败/解析失败/顶层不是对象时回空 dict（源码 `:42-47` 也是回 `{}` 后
/// 用默认表）；UTF-8 由 `read` + `serde_json` 负责（源码 `:40-41` 显式 UTF-8）。
fn read_json_object(path: &Path) -> BTreeMap<String, Value> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => map.into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 默认表门禁 ----

    /// fixture 里的一行 `default`。
    struct FixtureRow {
        key: String,
        type_label: String,
        value: Value,
    }

    /// 解析 `tests/fixtures/prefs_defaults.txt`：
    /// 返回 `(docs_dir_win32, {platform: (count, [rows])})`。
    fn load_fixture() -> (PathBuf, BTreeMap<String, (usize, Vec<FixtureRow>)>) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("prefs_defaults.txt");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 fixture {}: {e}", path.display()));
        let mut docs = PathBuf::new();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut rows: BTreeMap<String, Vec<FixtureRow>> = BTreeMap::new();
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            match cols[0] {
                "header" => {
                    if cols[1] == "docs_dir_win32" {
                        docs = PathBuf::from(cols[2]);
                    }
                }
                "count" => {
                    counts.insert(cols[1].to_string(), cols[2].parse().unwrap());
                }
                "default" => {
                    let value: Value = serde_json::from_str(cols[4])
                        .unwrap_or_else(|e| panic!("第 4 列不是 JSON：{} ({e})", cols[4]));
                    rows.entry(cols[1].to_string()).or_default().push(FixtureRow {
                        key: cols[2].to_string(),
                        type_label: cols[3].to_string(),
                        value,
                    });
                }
                other => panic!("fixture 里有未知行类型 {other:?}: {line}"),
            }
        }
        let per_platform = counts
            .into_iter()
            .map(|(p, n)| {
                let rows = rows.remove(&p).unwrap_or_default();
                (p, (n, rows))
            })
            .collect();
        (docs, per_platform)
    }

    /// JSON 值的"可见类型名"，与生成器 `type_label()` 一一对应。
    fn json_type_label(v: &Value) -> &'static str {
        match v {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(n) if n.is_i64() || n.is_u64() => "int",
            Value::Number(_) => "float",
            Value::String(_) => "str",
            Value::Array(_) => "list",
            Value::Object(_) => "dict",
        }
    }

    /// 门禁：默认表**逐条**等于 `gui/userconfig.py` 的 `default_configuration()`。
    ///
    /// 断言四件事（每个平台各一遍）：键数、键集合、每条的值、每条的类型。
    /// 期望值来自 fixture（由 `tests/tools/gen-prefs-defaults.py` 用 `ast`
    /// 抽出源码函数后**真的执行**得到），不是手抄。
    fn assert_platform_matches(
        platform: Platform,
        platform_name: &str,
        docs: &Path,
        fixture: &BTreeMap<String, (usize, Vec<FixtureRow>)>,
    ) {
        let (count, rows) = fixture
            .get(platform_name)
            .unwrap_or_else(|| panic!("fixture 里没有 {platform_name} 平台"));
        let ours = default_configuration(platform, Some(docs));
        assert_eq!(
            ours.len(),
            *count,
            "{platform_name}: 默认键数应与源码一致"
        );
        for row in rows {
            let v = ours
                .get(&row.key)
                .unwrap_or_else(|| panic!("{platform_name}: 缺默认键 {}", row.key));
            assert_eq!(
                json_type_label(v),
                row.type_label,
                "{platform_name}: 键 {} 的类型不对",
                row.key
            );
            assert_eq!(v, &row.value, "{platform_name}: 键 {} 的值不对", row.key);
        }
        // 反向：我们没有多出源码没有的键
        for key in ours.keys() {
            assert!(
                rows.iter().any(|r| &r.key == key),
                "{platform_name}: 多出源码没有的默认键 {key}"
            );
        }
    }

    #[test]
    fn default_table_matches_source_fixture_win32() {
        let (docs, fixture) = load_fixture();
        assert_eq!(
            docs,
            PathBuf::from(r"C:\Users\test\Documents"),
            "fixture 的「文档」目录替身（见生成器 --docs）"
        );
        assert_platform_matches(Platform::Windows, "win32", &docs, &fixture);
    }

    #[test]
    fn default_table_matches_source_fixture_linux() {
        let (docs, fixture) = load_fixture();
        assert_platform_matches(Platform::Posix, "linux", &docs, &fixture);
    }

    #[test]
    fn default_table_key_count_is_33_and_platform_differs_only_in_values() {
        // 30 条字面量（userconfig.py:61-125）+ 3 条 COMPAT_CONFIG（:126）
        let docs = Path::new(r"C:\Users\test\Documents");
        let win = default_configuration(Platform::Windows, Some(docs));
        let posix = default_configuration(Platform::Posix, Some(docs));
        assert_eq!(win.len(), 33);
        assert_eq!(posix.len(), 33);
        assert_eq!(
            win.keys().collect::<Vec<_>>(),
            posix.keys().collect::<Vec<_>>(),
            "两个平台只改值、不增删键（userconfig.py:127-139 只重写 button_mapping）"
        );
        let differing: Vec<&String> = win
            .keys()
            .filter(|k| win[*k] != posix[*k])
            .collect();
        assert_eq!(
            differing,
            vec!["input.button_mapping", "saving.scrap_prefix"],
            "平台差异只有这两条（userconfig.py:54-58 / :127-139）"
        );
        // 具体值：Windows 的 Button2/Button3 互换（:133-139）
        let bm = win["input.button_mapping"].as_object().unwrap();
        assert_eq!(bm["Button2"], json!("ShowPopupMenu"));
        assert_eq!(bm["Button3"], json!("PanViewMode"));
        assert_eq!(
            win["saving.scrap_prefix"],
            json!(r"C:\Users\test\Documents\MyPaint\scrap")
        );
        // Linux 支保持原样
        let bm = posix["input.button_mapping"].as_object().unwrap();
        assert_eq!(bm["Button2"], json!("PanViewMode"));
        assert_eq!(bm["Button3"], json!("ShowPopupMenu"));
        assert_eq!(posix["saving.scrap_prefix"], json!("~/MyPaint/scrap"));
    }

    #[test]
    fn windows_branch_without_documents_dir_falls_back_conservatively() {
        // 源码会 os.path.join(None, ...) 抛 TypeError；我们退到非 Windows 默认值。
        let win = default_configuration(Platform::Windows, None);
        assert_eq!(win["saving.scrap_prefix"], json!("~/MyPaint/scrap"));
        assert_eq!(win.len(), 33);
    }

    // ---- settings.json 路径 ----

    #[test]
    fn settings_path_is_user_config_mypaint_settings_json() {
        // gui/main.py:125-126 + gui/application.py:503
        let user_config = Path::new(r"C:\Users\me\AppData\Roaming");
        assert_eq!(
            resolve_settings_path(None, None, Some(user_config)),
            user_config.join("mypaint").join("settings.json")
        );
        // 覆盖目录
        assert_eq!(
            resolve_settings_path(Some(Path::new(r"D:\cfg")), None, Some(user_config)),
            Path::new(r"D:\cfg").join("settings.json")
        );
        // 覆盖整条路径（优先级最高）
        assert_eq!(
            resolve_settings_path(
                Some(Path::new(r"D:\cfg")),
                Some(Path::new(r"E:\x\prefs.json")),
                Some(user_config)
            ),
            PathBuf::from(r"E:\x\prefs.json")
        );
        // 一个都没有 → 临时目录兜底（不 panic、不返回相对路径）
        let fallback = resolve_settings_path(None, None, None);
        assert!(fallback.is_absolute(), "兜底路径必须是绝对路径");
        assert_eq!(fallback.file_name().unwrap(), "settings.json");
    }

    #[test]
    fn settings_path_honours_env_overrides() {
        // 两个开关都是应用本地扩展；同一个测试里顺序设置，避免测试并行时互相踩。
        let saved_dir = std::env::var_os(CONFIG_DIR_ENV);
        let saved_path = std::env::var_os(SETTINGS_PATH_ENV);
        std::env::remove_var(SETTINGS_PATH_ENV);
        std::env::set_var(CONFIG_DIR_ENV, r"D:\pigment-test-cfg");
        assert_eq!(
            settings_path(),
            Path::new(r"D:\pigment-test-cfg").join("settings.json")
        );
        assert_eq!(config_dir(), Path::new(r"D:\pigment-test-cfg"));

        std::env::set_var(SETTINGS_PATH_ENV, r"E:\pigment-test\prefs.json");
        assert_eq!(settings_path(), PathBuf::from(r"E:\pigment-test\prefs.json"));

        // 复原
        match saved_dir {
            Some(v) => std::env::set_var(CONFIG_DIR_ENV, v),
            None => std::env::remove_var(CONFIG_DIR_ENV),
        }
        match saved_path {
            Some(v) => std::env::set_var(SETTINGS_PATH_ENV, v),
            None => std::env::remove_var(SETTINGS_PATH_ENV),
        }
    }

    // ---- 载入 / 保存 / 未知键 ----

    fn tmpdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-prefs-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn missing_file_uses_defaults_and_save_creates_the_file() {
        // gui/userconfig.py:42-47（读不到就只用默认表）+ application.py:511-512
        let dir = tmpdir("missing");
        let path = dir.join("settings.json");
        let prefs = Prefs::load(&path);
        assert_eq!(prefs.len(), 33, "默认表 33 条全在");
        assert_eq!(prefs.str_or("saving.default_format", "?"), "openraster");
        prefs.save().unwrap();
        assert!(path.is_file(), "保存要连目录一起建出来");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('{'), "json.dumps(indent=2) 的形状");
        assert!(text.contains("\n  \"saving.default_format\": \"openraster\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_overrides_defaults_shallowly_and_keeps_unknown_keys() {
        // application.py:533-534（浅 update）+ :505（整张写回）
        let dir = tmpdir("unknown");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{
  "saving.default_format": "png-solid",
  "frame.color_rgba": [0.5, 0.5, 0.5, 1.0],
  "workspace.layout": {"autohide": false},
  "ext.from.the.future": {"a": [1, 2, 3]}
}"#,
        )
        .unwrap();
        let prefs = Prefs::load(&path);
        assert_eq!(
            prefs.str_or("saving.default_format", "?"),
            "png-solid",
            "用户值覆盖默认值"
        );
        assert_eq!(
            prefs.get("frame.color_rgba"),
            Some(&json!([0.5, 0.5, 0.5, 1.0]))
        );
        // 浅覆盖：整块 dict 被换掉，不是逐键合并（源码 dict.update 的语义）
        assert_eq!(
            prefs.get("workspace.layout"),
            Some(&json!({"autohide": false}))
        );
        assert_eq!(
            prefs.default_of("workspace.layout").unwrap()["autohide"],
            json!(true),
            "默认表本身没被改"
        );
        // 未知键原样在
        assert_eq!(
            prefs.get("ext.from.the.future"),
            Some(&json!({"a": [1, 2, 3]}))
        );
        assert_eq!(prefs.len(), 34, "33 条默认 + 1 条用户新键（另 3 条是覆盖）");

        // 存回去再读：未知键还在，值不变
        prefs.save().unwrap();
        let again = Prefs::load(&path);
        assert_eq!(
            again.get("ext.from.the.future"),
            Some(&json!({"a": [1, 2, 3]}))
        );
        assert_eq!(again.str_or("saving.default_format", "?"), "png-solid");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_json_falls_back_to_defaults_instead_of_failing() {
        // gui/userconfig.py:44-47（`except Exception` 后回 {}）
        let dir = tmpdir("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let prefs = Prefs::load(&path);
        assert_eq!(prefs.len(), 33);
        assert_eq!(prefs.str_or("saving.default_format", "?"), "openraster");

        // 顶层是数组也不算
        std::fs::write(&path, b"[1, 2]").unwrap();
        assert_eq!(Prefs::load(&path).len(), 33);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typed_accessors_fall_back_on_missing_or_wrong_type() {
        // 源码直接 prefs[...] 会抛的地方，我们回落到调用方的默认值
        let mut values = BTreeMap::new();
        values.insert("ok.bool".to_string(), json!(true));
        values.insert("ok.int".to_string(), json!(7));
        values.insert("ok.float".to_string(), json!(1.25));
        values.insert("ok.str".to_string(), json!("hi"));
        values.insert("ok.array".to_string(), json!(["#010203", "#040506"]));
        values.insert("bad.bool".to_string(), json!("yes"));
        values.insert("bad.int".to_string(), json!("10"));
        values.insert("bad.float".to_string(), json!(null));
        values.insert("bad.str".to_string(), json!(42));
        values.insert("bad.array".to_string(), json!({"a": 1}));
        let prefs = Prefs::from_values(values);

        assert!(prefs.bool_or("ok.bool", false));
        assert_eq!(prefs.i64_or("ok.int", 0), 7);
        assert!((prefs.f64_or("ok.float", 0.0) - 1.25).abs() < 1e-12);
        assert_eq!(prefs.str_or("ok.str", ""), "hi");
        assert_eq!(prefs.array_or("ok.array", &[]).len(), 2);

        // 类型不对 → 回落（不是 panic）
        assert!(!prefs.bool_or("bad.bool", false));
        assert_eq!(prefs.i64_or("bad.int", 10), 10);
        assert_eq!(prefs.f64_or("bad.float", 0.5), 0.5);
        assert_eq!(prefs.str_or("bad.str", "dflt"), "dflt");
        assert!(prefs.array_or("bad.array", &[]).is_empty());
        // 缺失 → 回落
        assert!(!prefs.bool_or("nope", false));
        assert_eq!(prefs.i64_or("nope", 3), 3);
        assert_eq!(prefs.get("nope"), None);
        // 整数也能当浮点读（JSON 里写 5 表示 5.0 的情况）
        assert_eq!(prefs.f64_or("ok.int", 0.0), 7.0);
    }

    #[test]
    fn set_remove_and_reload_round_trip() {
        let dir = tmpdir("mutate");
        let path = dir.join("settings.json");
        let mut prefs = Prefs::load(&path);
        prefs.set_str("colors.current", "#123456");
        prefs.set_str_array("colors.history", ["#111111", "#222222"]);
        prefs.set_bool("misc.context_restores_color", false);
        prefs.set("colors.palette", json!({"name": null, "columns": 0, "entries": []}));
        assert_eq!(prefs.len(), 36);
        let removed = prefs.remove("ui.feedback.scale");
        assert_eq!(removed, Some(json!(false)));
        assert!(!prefs.contains_key("ui.feedback.scale"));
        prefs.save().unwrap();

        let mut again = Prefs::load(&path);
        assert_eq!(again.str_or("colors.current", ""), "#123456");
        assert_eq!(
            again.get("colors.history"),
            Some(&json!(["#111111", "#222222"]))
        );
        assert!(!again.bool_or("misc.context_restores_color", true));
        assert!(
            again.contains_key("ui.feedback.scale"),
            "默认键删了也会在下次启动被默认表补回来（load_settings 从默认表开始，\
             application.py:530-532）；只有**默认表里没有**的键（如 cursor.freehand.*）\
             才是真正被删掉的"
        );
        // reload 丢掉本会话未保存的改动（偏好对话框的 Revert 语义）
        again.set_str("colors.current", "#ffffff");
        again.reload();
        assert_eq!(again.str_or("colors.current", ""), "#123456");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn button_mapping_setter_round_trips_and_drops_empty_rows() {
        let dir = tmpdir("button-map");
        let path = dir.join("settings.json");
        let mut prefs = Prefs::load(&path);
        prefs.set_button_mapping_pairs([
            ("<Control><Shift>Button2".to_string(), "ShowPopupMenu".to_string()),
            ("Button3".to_string(), "PanViewMode".to_string()),
            ("".to_string(), "Undo".to_string()),
            ("Button9".to_string(), "".to_string()),
        ]);
        // 空键/空动作不进 dict（编辑器的"尚未按鼠标键"行）
        assert_eq!(
            prefs.button_mapping_pairs(),
            vec![
                (
                    "<Control><Shift>Button2".to_string(),
                    "ShowPopupMenu".to_string()
                ),
                ("Button3".to_string(), "PanViewMode".to_string()),
            ]
        );
        // 落盘再读回格式保持
        prefs.save().unwrap();
        let again = Prefs::load(&path);
        assert_eq!(again.button_mapping_pairs(), prefs.button_mapping_pairs());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_color_picker_popup_mapping_is_reset_to_default() {
        // gui/application.py:535-539
        let dir = tmpdir("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"input.button_mapping": {"<Control>Button1": "ColorPickerPopup"}}"#,
        )
        .unwrap();
        let prefs = Prefs::load(&path);
        let default = default_configuration(current_platform(), documents_dir().as_deref());
        assert_eq!(
            prefs.get("input.button_mapping"),
            default.get("input.button_mapping"),
            "老配置整张映射重置回默认表"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- saving.default_format ----

    #[test]
    fn default_format_maps_to_the_source_extensions() {
        // gui/filehandling.py:598-602 + :1409-1411
        assert_eq!(format_extension("openraster"), ".ora");
        assert_eq!(format_extension("jpeg-90%"), ".jpg");
        assert_eq!(format_extension("png-solid"), ".png");
        // 源码未知取值会 KeyError；我们保守回落出厂格式
        assert_eq!(format_extension("nope"), ".ora");

        let mut values = BTreeMap::new();
        values.insert(
            KEY_SAVING_DEFAULT_FORMAT.to_string(),
            json!("png-solid"),
        );
        assert_eq!(save_extension(&Prefs::from_values(values)), ".png");
    }
}
