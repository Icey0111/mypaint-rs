//! 暂存区（Scratchpad）的**文件层** —— 逐行对照 `gui/filehandling.py`
//! 与 `gui/application.py`。
//!
//! # 先纠正两个前提：源码里没有 `lib/scratchpad.py`
//!
//! B5 的任务书假定暂存区的存储规则在 `lib/scratchpad.py`。在唯一事实来源
//! `_mypaint_ref/` 里**不存在这个文件**（`scratch*` 全仓只有
//! `gui/scratchwindow.py`，135 处 `scratch` 命中里也没有任何 `lib/scratchpad`
//! 引用）。暂存区的文件层实际散在这三处：
//!
//! | 语义 | 源码位置 |
//! |---|---|
//! | 目录 = `<user_data>/scratchpads` | `filehandling.py:1431-1438` |
//! | 默认页 `scratchpad_default.ora` | `filehandling.py:1440-1443` |
//! | 自动存档 `autosave.ora` | `filehandling.py:1445-1447` |
//! | 新文件名 `NNN_l.ext` 的递增规则 | `filehandling.py:1362-1418` |
//! | 保存（只在"需要保存"时写盘） | `filehandling.py:1018-1036` |
//! | 打开（同时更新 `<偏好>` 路径） | `filehandling.py:956-976` |
//! | 列目录（含 `special/` 子目录） | `filehandling.py:1453-1469` |
//! | 删除（只允许目录内文件） | `filehandling.py:1587-1607` |
//! | 目录创建（启动时建 `scratchpads`） | `application.py:228-236` |
//! | 目录创建（保存前兜底） | `filehandling.py:1363-1366` |
//! | 文件格式由 `saving.default_format` 决定 | `filehandling.py:1408-1411` |
//! | 启动时按 `<偏好>` 里的路径载回 | `application.py:474-486` |
//!
//! # 存盘规则（源码结论）
//!
//! - **目录**：`os.path.join(app.user_datapath, "scratchpads")`
//!   （`filehandling.py:1433`）。`user_datapath` = GLib 的 user data dir 再拼
//!   `mypaint`（`main.py:120-126`），即 POSIX 的 `$XDG_DATA_HOME/mypaint`、
//!   Windows 的 `%APPDATA%\mypaint`（`main.py:98-101`：Windows 上 config 与
//!   data 是同一个位置）。代码里有 `# TODO allow override via prefs, maybe`
//!   （`:1432`）—— **暂存区目录当时没有偏好项**。任务书提到的
//!   `saving.scrap_prefix` 是**主文档的 scrap**（`filehandling.py:1420-1429`，
//!   默认 `~/MyPaint/scrap`），不是暂存区，别混用。
//! - **格式**：**跟主文档同一个偏好** `saving.default_format`
//!   （：1409-1411 把 `config2saveformat[cfg]` 查到的 `saveformats[...][1]`
//!   当作扩展名）。出厂默认是 `openraster`（`userconfig.py:89`）→
//!   `config2saveformat["openraster"] = ORA`（`filehandling.py:598-602`）→
//!   **`.ora`**。所以"暂存区是 png 还是 ora"取决于偏好，默认 `.ora`。
//! - **文件名**：`prefix + "%03d_a" + ext`，如 `scratchpads/001_a.ora`
//!   （`filehandling.py:1406`）。再次保存时**复用编号、递增字母**
//!   （`001_a` → `001_b`，`:1383-1394`）；字母用完 `z` 才换下一个编号。
//! - **自动存档**：`autosave.ora`（`:1447`），是偏好
//!   `scratchpad.last_opened_scratchpad` 的默认值（`application.py:393-397`）。
//! - **"需要保存"的判据**：`unsaved_painting_time or export or
//!   not os.path.exists(filename)`（`filehandling.py:1019-1023`）——
//!   没画过、也已经有文件时**不写盘**。
//! - **清理**：**源码没有按数量清理暂存区的逻辑**（全仓搜
//!   `N_SCRATCHPAD` / `clean.*scratchpad` 无命中；`delete_scratchpads`
//!   只是"删用户选中的那几个"）。[`prune`] 是本项目补的显式工具，
//!   文档里标明它**不是**源码行为，且默认没有 UI 入口（见记账）。
//!
//! # 路径注入
//!
//! 所有函数都吃 `&Path`（目录 / 文件名），测试用临时目录；
//! [`default_dir`] 只是"按源码规则算出默认目录"的薄壳。

use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::fileio::FileError;

// ---- 常量（逐条标源码行号）----

/// 用户数据目录下的应用子目录 —— `main.py:122` 的 `appsubdir = "mypaint"`。
pub const APP_SUBDIR: &str = "mypaint";
/// 暂存区子目录 —— `filehandling.py:1433` 的 `"scratchpads"`。
pub const SCRATCHPADS_SUBDIR: &str = "scratchpads";
/// 链接式暂存页的子目录 —— `filehandling.py:1456` 的 `"special"`。
pub const SPECIAL_SUBDIR: &str = "special";
/// 自动存档文件名 —— `filehandling.py:1447`。
pub const AUTOSAVE_NAME: &str = "autosave.ora";
/// 默认暂存页文件名 —— `filehandling.py:1443`。
pub const DEFAULT_NAME: &str = "scratchpad_default.ora";
/// 默认格式的扩展名。`userconfig.py:89` 的 `saving.default_format` 出厂值是
/// `"openraster"`，`filehandling.py:598-602` 把它映射到 `_SaveFormat.ORA`，
/// `:1411` 再取 `saveformats[ORA][1]` = `".ora"`。
///
/// 运行期取扩展名用 [`crate::prefs::save_extension`]（B6-1）；
/// 这里保留的是「没有偏好时」的出厂值。
pub const DEFAULT_EXT: &str = ".ora";
/// 链接式暂存页的文件名前缀 —— `filehandling.py:1371` / `:1467` 的 `_md5`。
pub const MD5_PREFIX: &str = "_md5";
/// 列目录时认的扩展名 —— `filehandling.py:1463` 的
/// `["png", "ora", "jpg", "jpeg"]`（大小写两种写法都认，见 `:1464-1465`）。
pub const LIST_EXTENSIONS: [&str; 4] = ["png", "ora", "jpg", "jpeg"];

/// 暂存区目录的环境变量覆盖 —— **应用本地扩展**（与
/// [`crate::brushlib::default_brush_dirs`] 的 `PIGMENT_BRUSH_PATH` 同风格）。
///
/// 源码没有这个开关（`filehandling.py:1432` 的 TODO 还没做），加它只为开发期
/// 把暂存区指向任意目录，免得动用户真实数据。
pub const DIR_ENV: &str = "PIGMENT_SCRATCHPAD_DIR";

// ---- 目录解析 ----

/// GLib 的 user **data** dir（`g_get_user_data_dir`，`main.py:123`）。
///
/// - Windows：`%APPDATA%`（GLib 在 Windows 上 data 与 config 同源，
///   `main.py:98-101`），退回 `%LOCALAPPDATA%`
/// - POSIX：`$XDG_DATA_HOME`，否则 `$HOME/.local/share`
pub fn user_data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        for var in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(d) = std::env::var_os(var) {
                return Some(PathBuf::from(d));
            }
        }
        None
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    }
}

/// 默认暂存区目录 = `<user_data>/mypaint/scratchpads`
/// （`main.py:122-124` + `filehandling.py:1433`）。
///
/// 环境变量 [`DIR_ENV`] 可覆盖（应用本地扩展）。两者都拿不到时退回系统临时
/// 目录下的固定子目录 —— 只在环境变量被剥光的沙箱里会发生。
pub fn default_dir() -> PathBuf {
    if let Some(v) = std::env::var_os(DIR_ENV) {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    match user_data_dir() {
        Some(base) => base.join(APP_SUBDIR).join(SCRATCHPADS_SUBDIR),
        None => std::env::temp_dir().join("pigment-scratchpads"),
    }
}

/// 建目录（含父级）—— `filehandling.py:1363-1366` 的
/// `if not os.path.exists(prefix_dir): os.makedirs(prefix_dir)`，
/// 也是 `application.py:232-236` 启动时建 `scratchpads` 的行为。已存在是空操作。
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)
}

/// 目录的前缀串（带尾分隔符）—— `filehandling.py:1434-1437`：
/// `abspath(prefix)` 之后，**仅当它是个已存在的目录**才补尾分隔符。
///
/// 文件名生成与列目录的匹配都以这个串为前缀（源码是对它做 `glob`/`re`）。
fn prefix_string(dir: &Path) -> String {
    let mut s = dir.to_string_lossy().into_owned();
    if dir.is_dir() {
        let sep = std::path::MAIN_SEPARATOR;
        if !s.ends_with(sep) {
            s.push(sep);
        }
    }
    s
}

/// 自动存档的完整路径 —— `filehandling.py:1445-1447`。
pub fn autosave_path(dir: &Path) -> PathBuf {
    dir.join(AUTOSAVE_NAME)
}

/// 默认暂存页的完整路径 —— `filehandling.py:1440-1443`。
pub fn default_path(dir: &Path) -> PathBuf {
    dir.join(DEFAULT_NAME)
}

// ---- 文件名生成（`save_autoincrement_file`）----

/// 前缀串之后的**前导数字串** —— 对应 `filehandling.py:1379-1381` 的
/// `re.findall(re.escape(prefix) + "([0-9]+)", filename)`。
///
/// 只在 `full` 真的以 `prefix` 开头时才算命中（源码的 `findall` 也是这样，
/// 只是它会扫描整串；我们的 `full` 恒是绝对路径，前缀只在开头出现）。
fn number_after_prefix(prefix: &str, full: &str) -> Option<String> {
    let rest = full.strip_prefix(prefix)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// 目录里"以 `prefix` + `number` + `_` 开头"的文件的名字部分
/// （对应 `filehandling.py:1386` 的 `glob(prefix + number + "_*")`）。
fn collect_letter_files(dir: &Path, number: &str) -> Vec<char> {
    let needle = format!("{number}_");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix(&needle) {
            if let Some(c) = rest.chars().next() {
                out.push(c);
            }
        }
    }
    out
}

/// 目录里所有"`[0-9][0-9][0-9]*`"文件名（`filehandling.py:1398`）的前导数字，
/// 取最大值（`:1397-1405`）。
fn max_three_digit_number(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut maximum = 0u64;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // glob 模式 `[0-9][0-9][0-9]*`：至少三位数字开头
        let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() < 3 {
            continue;
        }
        if let Ok(n) = digits.parse::<u64>() {
            maximum = maximum.max(n);
        }
    }
    maximum
}

/// 算出下一个暂存页的文件名 —— 照搬 `filehandling.py:1362-1418` 的
/// `save_autoincrement_file`（`main_doc=False` 分支）的**命名部分**。
///
/// 规则（逐条对应源码）：
/// 1. 文件名以 `_md5` 开头 → **原样返回**，不做递增（`:1371-1377`）
/// 2. 当前文件名里能从目录前缀后解析出数字 → 复用该数字，字母从 `a` 起
///    往后找第一个空位（`:1379-1394`）；字母用尽 `z` → 换下一个编号（`:1390-1393`）
/// 3. 否则取目录里最大的三位数编号 +1，字母从 `a` 起（`:1396-1406`）
///
/// 与源码的两处显式差异：
/// - 源码末尾有 `assert not os.path.exists(filename)`（`:1413`），那是开发期
///   断言。这里**不 panic**，也不去重 —— 返回可能已存在的路径，由调用方
///   （保存动作）按"覆盖"处理。
/// - 源码把 `glob` 当"文件名匹配"用；这里直接 `read_dir` + 前缀比较，
///   语义等价（同一个目录、同一套前缀规则）。
pub fn next_scratchpad_path(dir: &Path, current: Option<&Path>, ext: &str) -> PathBuf {
    let prefix = prefix_string(dir);

    // 1) 链接式暂存页：直接存回原文件（`:1371-1377`）
    let mut number: Option<String> = None;
    if let Some(cur) = current {
        let name = cur.file_name().map(|s| s.to_string_lossy().into_owned());
        if let Some(name) = name {
            if name.starts_with(MD5_PREFIX) {
                return cur.to_path_buf();
            }
        }
        // 2a) 从当前文件名解析编号（`:1379-1381`）
        number = number_after_prefix(&prefix, &cur.to_string_lossy());
    }

    let new_name = match number {
        Some(number) => {
            // 2b) 复用编号，找下一个未用的字母（`:1383-1394`）
            let mut char_ = 'a';
            for c in collect_letter_files(dir, &number) {
                if c.is_ascii_lowercase() && c >= char_ {
                    char_ = (c as u8 + 1) as char;
                }
            }
            if char_ > 'z' {
                // 字母用尽 → 换下一个编号（`:1390-1393` 的递归）
                let next = max_three_digit_number(dir) + 1;
                format!("{next:03}_a")
            } else {
                format!("{number}_{char_}")
            }
        }
        None => {
            // 3) 还没有编号：取最大编号 +1，字母从 a 起（`:1396-1406`）
            let next = max_three_digit_number(dir) + 1;
            format!("{next:03}_a")
        }
    };
    dir.join(format!("{new_name}{ext}"))
}

// ---- 列目录 ----

/// 一个文件名是否是暂存页（`filehandling.py:1463-1467` 的三条 glob）：
///
/// - `prefix[0-9]*.<ext>`：目录前缀后**紧接数字**
/// - 同一模式的**大写扩展名**版本（`:1465`，所以 `.ORA` 认、`.Ora` 不认）
/// - `prefix_md5[0-9a-f]*.<ext>`：链接式暂存页（`:1467`，小写十六进制）
fn is_scratchpad_name(name: &str) -> bool {
    for ext in LIST_EXTENSIONS {
        for e in [ext.to_string(), ext.to_uppercase()] {
            let suffix = format!(".{e}");
            let Some(stem) = name.strip_suffix(&suffix) else {
                continue;
            };
            if stem.starts_with(|c: char| c.is_ascii_digit()) {
                return true;
            }
            if let Some(hex) = stem.strip_prefix(MD5_PREFIX) {
                if hex
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// 列一个目录里的暂存页 —— `filehandling.py:1461-1469` 的 `_list_prefixed_dir`。
///
/// 顺序：`filenames.sort()`（`:1468`）。Python 的 `sort()` 比的是**整条路径串**，
/// 所以这里也按 `to_string_lossy()` 排（不要用 `Path` 的组件比较：两者在
/// `special/` 这类跨目录混排时可能给出不同序）。
pub fn list_prefixed_dir(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path.is_file() {
            continue;
        }
        if is_scratchpad_name(&name) {
            files.push(path);
        }
    }
    files.sort_by_key(|p| p.to_string_lossy().into_owned());
    files
}

/// 列暂存区里的全部暂存页 —— `filehandling.py:1453-1459` 的 `list_scratchpads`。
///
/// **注意拼接顺序**：源码先取主目录（已排序），再 `+=` 取 `special/` 子目录
/// （各自内部排序），**整体不再重排**。所以 `special/` 里的文件永远排在
/// 主目录文件之后，哪怕按字符串它该排前面。这里照搬这个行为。
pub fn list_scratchpads(dir: &Path) -> Vec<PathBuf> {
    let mut files = list_prefixed_dir(dir);
    let special = dir.join(SPECIAL_SUBDIR);
    if special.is_dir() {
        files.extend(list_prefixed_dir(&special));
    }
    files
}

/// 最近修改的暂存页（`special/` 也算）。
///
/// **源码没有这个函数**：它靠偏好 `scratchpad.last_opened_scratchpad`
/// （`application.py:393-397`、`:474-486`）记住"上次那张"（B6-1 起由
/// `pigment-app` 的 `Scratchpad::startup_page` 读那条键）。这里按 mtime 取最新，
/// 是给"偏好里没有落点"时的开发期辅助（`mtime` 相同则按路径串倒序取，保证确定性）。
pub fn newest_scratchpad(dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<(std::time::SystemTime, String, PathBuf)> = list_scratchpads(dir)
        .into_iter()
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((mtime, p.to_string_lossy().into_owned(), p))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    files.pop().map(|(_, _, p)| p)
}

// ---- 保存 ----

/// "这次需要写盘吗" —— `filehandling.py:1019-1023`：
///
/// ```python
/// save_needed = (
///     self.app.scratchpad_doc.model.unsaved_painting_time
///     or export
///     or not os.path.exists(filename)
/// )
/// ```
///
/// `unsaved_painting_time` 是 [`Document::unsaved_painting_time`]（秒）：
/// 任何一次作画/撤销都会抬高它，保存成功归零（`document.rs` 的 `save`）。
pub fn save_needed(unsaved_painting_time: f64, export: bool, path: &Path) -> bool {
    unsaved_painting_time != 0.0 || export || !path.exists()
}

/// 保存暂存页 —— `filehandling.py:1017-1036` 的 `save_scratchpad`。
///
/// 与源码一致：**只在 [`save_needed`] 为真时才写盘**（所以"什么都没画、
/// 文件已存在"时它什么也不做）；`.ora` / `.png` 的分派交给
/// [`Document::save`]（按扩展名）。返回值 `true` 表示真的写了文件。
///
/// `export` 只在"另存为/导出"时为真（源码的 `save_scratchpad(..., export=)`）。
pub fn save_scratchpad(
    doc: &mut Document,
    path: &Path,
    export: bool,
) -> Result<bool, FileError> {
    if !save_needed(doc.unsaved_painting_time, export, path) {
        return Ok(false);
    }
    doc.save(path, None)?;
    Ok(true)
}

/// 保存到**指定**路径（"另存为"，`filehandling.py:1256-1266` 的
/// `save_scratchpad_as_dialog` → `save_scratchpad(filename, export=False)`）。
///
/// 与 [`save_scratchpad`] 的唯一区别：这里**不问 [`save_needed`]** —— 用户在
/// 对话框里点了确定就是要写（源码走到这里时文件名是新的，`not
/// os.path.exists` 本来就为真）。
pub fn save_scratchpad_as(doc: &mut Document, path: &Path) -> Result<(), FileError> {
    doc.save(path, None)?;
    Ok(())
}

// ---- 删除 ----

/// 删掉给定的暂存页 —— `filehandling.py:1587-1597` 的 `delete_scratchpads`。
///
/// 源码的安全阀：只有**确实在暂存区目录内**的文件才删
/// （`:1590-1595` 的 `os.path.isfile(filename) and
/// os.path.abspath(filename).startswith(prefix)`）。返回实际删除的个数。
pub fn delete_scratchpads(dir: &Path, files: &[PathBuf]) -> usize {
    // `os.path.abspath` 的等价物：转绝对路径但**不解析符号链接**
    // （`std::fs::canonicalize` 会加 Windows 的 `\\?\` 前缀，前缀比较就失效了）。
    let dir_abs = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut removed = 0;
    for f in files {
        let abs = std::path::absolute(f).unwrap_or_else(|_| f.to_path_buf());
        // 按**路径分量**比较（比源码的字符串 startswith 更严：不会把
        // `scratchpadsX/` 误判成 `scratchpads/` 里面）
        if !(f.is_file() && abs.starts_with(&dir_abs)) {
            continue;
        }
        if std::fs::remove_file(f).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 删默认暂存页 —— `filehandling.py:1599-1602` 的 `delete_default_scratchpad`。
pub fn delete_default(dir: &Path) -> bool {
    let p = default_path(dir);
    if p.is_file() && std::fs::remove_file(&p).is_ok() {
        return true;
    }
    false
}

/// 删自动存档 —— `filehandling.py:1604-1607` 的 `delete_autosave_scratchpad`。
pub fn delete_autosave(dir: &Path) -> bool {
    let p = autosave_path(dir);
    if p.is_file() && std::fs::remove_file(&p).is_ok() {
        return true;
    }
    false
}

/// 只保留最新 `keep` 个暂存页，其余删掉。**本项目扩展，不是源码行为。**
///
/// 源码没有任何按数量清理暂存区的逻辑（见模块文档"清理"一节）；暂存页会
/// 无限增长。这个函数给"以后要不要自动清理"留一个可测的落点，**当前没有
/// 任何 UI / 启动流程调用它**（记账）。`special/` 里的链接式暂存页不参与。
pub fn prune(dir: &Path, keep: usize) -> std::io::Result<usize> {
    let mut files: Vec<(std::time::SystemTime, String, PathBuf)> = list_prefixed_dir(dir)
        .into_iter()
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((mtime, p.to_string_lossy().into_owned(), p))
        })
        .collect();
    // 老 → 新；保留末尾 keep 个
    files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    if files.len() <= keep {
        return Ok(0);
    }
    let drop_n = files.len() - keep;
    let victims: Vec<PathBuf> = files.into_iter().take(drop_n).map(|(_, _, p)| p).collect();
    Ok(delete_scratchpads(dir, &victims))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 每个测试一个干净目录（`target` 下的临时目录，避免撞用户数据）。
    fn tmpdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-scratchpad-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        p
    }

    #[test]
    fn ensure_dir_creates_the_missing_directory() {
        // filehandling.py:1363-1366 / application.py:232-236
        let root = tmpdir("mkdir");
        let dir = root.join("a").join("scratchpads");
        assert!(!dir.exists());
        ensure_dir(&dir).unwrap();
        assert!(dir.is_dir(), "多级目录要一次建出来（makedirs）");
        // 幂等：已存在不报错
        ensure_dir(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn autosave_and_default_names_match_the_source() {
        // filehandling.py:1440-1447
        let dir = Path::new("/tmp/x/scratchpads");
        assert_eq!(autosave_path(dir), dir.join("autosave.ora"));
        assert_eq!(default_path(dir), dir.join("scratchpad_default.ora"));
        assert_eq!(AUTOSAVE_NAME, "autosave.ora");
        assert_eq!(DEFAULT_NAME, "scratchpad_default.ora");
        // 出厂格式 = openraster → .ora（userconfig.py:89 → :598-602 → :1411）
        assert_eq!(DEFAULT_EXT, ".ora");
    }

    #[test]
    fn first_save_lands_on_001_a() {
        // filehandling.py:1396-1406：没有编号时 "%03d_a"
        let dir = tmpdir("first");
        ensure_dir(&dir).unwrap();
        let next = next_scratchpad_path(&dir, None, DEFAULT_EXT);
        assert_eq!(next, dir.join("001_a.ora"));
        // 当前文件名是 autosave.ora（没有编号）→ 走同一条分支
        let next = next_scratchpad_path(&dir, Some(&autosave_path(&dir)), DEFAULT_EXT);
        assert_eq!(next, dir.join("001_a.ora"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_number_walks_the_existing_maximum() {
        // filehandling.py:1396-1406：maximum 从 `prefix[0-9][0-9][0-9]*` 里取
        let dir = tmpdir("maxnum");
        ensure_dir(&dir).unwrap();
        touch(&dir, "001_a.ora");
        touch(&dir, "007_a.ora");
        // 不足三位的名字不参与（glob 要求至少三位数字）
        touch(&dir, "9_a.ora");
        let next = next_scratchpad_path(&dir, None, DEFAULT_EXT);
        assert_eq!(next, dir.join("008_a.ora"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reuses_the_number_and_increments_the_letter() {
        // filehandling.py:1383-1394：复用编号，字母往后找第一个空位
        let dir = tmpdir("letters");
        ensure_dir(&dir).unwrap();
        touch(&dir, "003_a.ora");
        touch(&dir, "003_c.ora");
        let cur = dir.join("003_a.ora");
        let next = next_scratchpad_path(&dir, Some(&cur), DEFAULT_EXT);
        assert_eq!(next, dir.join("003_d.ora"), "字母取现有最大值 +1（不是空位）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wraps_to_a_new_number_when_z_is_exhausted() {
        // filehandling.py:1390-1393：char > "z" → 换下一个编号
        let dir = tmpdir("wrap");
        ensure_dir(&dir).unwrap();
        touch(&dir, "002_z.ora");
        let cur = dir.join("002_z.ora");
        let next = next_scratchpad_path(&dir, Some(&cur), DEFAULT_EXT);
        assert_eq!(next, dir.join("003_a.ora"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn md5_linked_pages_are_saved_directly() {
        // filehandling.py:1371-1377：名字以 _md5 开头就原样存回
        let dir = tmpdir("md5");
        ensure_dir(&dir).unwrap();
        let cur = dir.join("_md5abcdef0123456789.ora");
        let next = next_scratchpad_path(&dir, Some(&cur), DEFAULT_EXT);
        assert_eq!(next, cur);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extension_comes_from_the_callers_format() {
        // filehandling.py:1408-1411：扩展名由 saving.default_format 决定
        let dir = tmpdir("ext");
        ensure_dir(&dir).unwrap();
        assert_eq!(
            next_scratchpad_path(&dir, None, ".png"),
            dir.join("001_a.png")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listed_names_need_a_digit_or_md5_prefix_and_a_known_extension() {
        // filehandling.py:1463-1467 的三条 glob
        assert!(is_scratchpad_name("001_a.ora"));
        assert!(is_scratchpad_name("1.png"));
        assert!(is_scratchpad_name("002_b.ORA"), "大写扩展名也认（:1465）");
        assert!(is_scratchpad_name("_md5abc123.jpg"));
        assert!(is_scratchpad_name("_md5.jpeg"), "hex 段允许空（`[0-9a-f]*`）");
        assert!(!is_scratchpad_name("autosave.ora"), "没有数字前缀");
        assert!(!is_scratchpad_name("scratchpad_default.ora"));
        assert!(!is_scratchpad_name("001_a.Ora"), "只认全小写/全大写");
        assert!(!is_scratchpad_name("001_a.txt"), "扩展名不在名单里");
        assert!(!is_scratchpad_name("_md5ABC123.ora"), "hex 只认小写");
        assert!(!is_scratchpad_name("_md5x.ora"), "x 不是十六进制");
    }

    #[test]
    fn list_is_sorted_per_directory_and_special_comes_last() {
        // filehandling.py:1453-1469：主目录排序后拼 special/，整体不再重排
        let dir = tmpdir("list");
        ensure_dir(&dir).unwrap();
        let special = dir.join(SPECIAL_SUBDIR);
        ensure_dir(&special).unwrap();
        touch(&dir, "002_a.ora");
        touch(&dir, "001_a.ora");
        touch(&dir, "notes.txt");
        touch(&special, "000_a.ora");
        let files = list_scratchpads(&dir);
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, ["001_a.ora", "002_a.ora", "special/000_a.ora"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newest_picks_the_latest_modification_time() {
        let dir = tmpdir("newest");
        ensure_dir(&dir).unwrap();
        touch(&dir, "001_a.ora");
        std::thread::sleep(Duration::from_millis(20));
        touch(&dir, "003_a.ora");
        std::thread::sleep(Duration::from_millis(20));
        touch(&dir, "002_a.ora");
        assert_eq!(newest_scratchpad(&dir), Some(dir.join("002_a.ora")));
        // 空目录 → None
        let empty = tmpdir("newest-empty");
        ensure_dir(&empty).unwrap();
        assert_eq!(newest_scratchpad(&empty), None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn save_needed_follows_the_source_predicate() {
        // filehandling.py:1019-1023
        let dir = tmpdir("needed");
        ensure_dir(&dir).unwrap();
        let p = dir.join("001_a.ora");
        // 没画过 + 文件不存在 → 要存
        assert!(save_needed(0.0, false, &p));
        touch(&dir, "001_a.ora");
        // 没画过 + 文件已存在 → 不存（源码的 False 分支）
        assert!(!save_needed(0.0, false, &p));
        // 画过 → 存
        assert!(save_needed(0.25, false, &p));
        // 导出 → 无论画没画都存
        assert!(save_needed(0.0, true, &p));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_writes_only_when_needed_and_the_document_round_trips() {
        // filehandling.py:1018-1036 + Document::save/load
        let dir = tmpdir("roundtrip");
        ensure_dir(&dir).unwrap();
        let mut doc = Document::new();
        let target = next_scratchpad_path(&dir, None, DEFAULT_EXT);
        // 空白文档：unsaved_painting_time == 0，但文件还不存在 → 写
        assert!(save_scratchpad(&mut doc, &target, false).unwrap());
        assert!(target.is_file());
        // 已存在且没画过 → 不写盘（save_needed 为假）
        assert!(!save_scratchpad(&mut doc, &target, false).unwrap());
        // 画出"画过"状态后 → 写
        doc.unsaved_painting_time = 1.5;
        assert!(save_scratchpad(&mut doc, &target, false).unwrap());
        assert_eq!(doc.unsaved_painting_time, 0.0, "保存成功要归零");
        // 读回
        let mut back = Document::new();
        back.load(&target).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_refuses_files_outside_the_scratchpad_directory() {
        // filehandling.py:1587-1597 的安全阀
        let dir = tmpdir("delete");
        ensure_dir(&dir).unwrap();
        let inside = touch(&dir, "001_a.ora");
        let root = dir.parent().unwrap().to_path_buf();
        let outside = {
            let p = root.join(format!(
                "pigment-scratchpad-{}-delete-outside.ora",
                std::process::id()
            ));
            std::fs::write(&p, b"x").unwrap();
            p
        };
        let removed = delete_scratchpads(&dir, &[inside.clone(), outside.clone()]);
        assert_eq!(removed, 1, "只删暂存区目录内的");
        assert!(!inside.exists());
        assert!(outside.exists(), "目录外的文件必须留着");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_only_the_newest_entries() {
        // 本项目扩展（源码无此规则，见模块文档）
        let dir = tmpdir("prune");
        ensure_dir(&dir).unwrap();
        for (i, name) in ["001_a.ora", "002_a.ora", "003_a.ora", "004_a.ora"]
            .iter()
            .enumerate()
        {
            touch(&dir, name);
            if i + 1 < 4 {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert_eq!(prune(&dir, 2).unwrap(), 2);
        let left: Vec<String> = list_prefixed_dir(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["003_a.ora", "004_a.ora"], "留下最新的两个");
        // 未超额时不动
        assert_eq!(prune(&dir, 5).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_dir_follows_the_source_layout() {
        // main.py:122-124 + filehandling.py:1433
        let dir = default_dir();
        // 有环境变量覆盖时（应用本地扩展）只验"能算出个绝对目录"
        if std::env::var_os(DIR_ENV).is_some() {
            assert!(!dir.as_os_str().is_empty());
            return;
        }
        assert!(dir.ends_with(SCRATCHPADS_SUBDIR), "目录名必须是 scratchpads");
        assert_eq!(
            dir.parent().and_then(|p| p.file_name()),
            Some(std::ffi::OsStr::new(APP_SUBDIR)),
            "父目录是 mypaint（main.py:122）"
        );
    }
}
