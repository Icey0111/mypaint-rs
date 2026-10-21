//! 国际化（最小可用面）—— `lib/i18n.py` + `gui/application.py` 的 locale 选择。
//!
//! ## 做法
//!
//! 1. **安装版 `.mo`（首选）**：按 `lib.i18n.userlocale`（或系统 locale）在
//!    安装目录 `share/locale/<code>/LC_MESSAGES/mypaint.mo` 里查表；支持
//!    `PIGMENT_LOCALE_DIR` 覆盖与可执行文件旁的 `share/locale`。
//! 2. **内嵌 `zh_CN.po`（回退）**：仓库带官方 `po/zh_CN.po`
//!    （`assets/i18n/zh_CN.po`），安装版不在时仍提供中文菜单。
//! 3. 两者都没有的 msgid 回退英文原文。
//!
//! 覆盖范围是**所有走 [`tr`] 的字符串**（菜单/动作/tooltip/面板标题等）；
//! 项目里少数手写中文文案没有进 gettext 的，切到非中文时仍是中文（记账）。
//!
//! ## 偏好
//!
//! `lib.i18n.userlocale`：`""`/`auto` = 跟随系统；`zh_CN` 中文；
//! `en_US` 英文；其它代码（`fr_FR` 等）查安装版 `.mo`。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Locale {
    System,
    English,
    Chinese,
    /// 其它语言：优先查安装版 `mypaint.mo`，没有则回退英文。
    Other,
}

static LOCALE: AtomicU8 = AtomicU8::new(0);
/// 完整 locale 代码（`fr_FR`、`pt_BR`…），供 `.mo` 查找。
static CODE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn code_slot() -> &'static Mutex<Option<String>> {
    CODE.get_or_init(|| Mutex::new(None))
}

fn encode(locale: Locale) -> u8 {
    match locale {
        Locale::System => 0,
        Locale::English => 1,
        Locale::Chinese => 2,
        Locale::Other => 3,
    }
}

/// 按偏好值设置当前 locale（`lib.i18n.userlocale`）。
pub(crate) fn set_from_pref(value: &str) {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("auto") || v.eq_ignore_ascii_case("system") {
        *code_slot().lock().unwrap() = None;
        LOCALE.store(encode(Locale::System), Ordering::Relaxed);
        return;
    }
    let code = v.replace('-', "_");
    let lower = code.to_ascii_lowercase();
    let locale = if lower.starts_with("zh") {
        Locale::Chinese
    } else if lower.starts_with("en") || lower == "c" || lower == "posix" {
        Locale::English
    } else {
        Locale::Other
    };
    *code_slot().lock().unwrap() = Some(code);
    LOCALE.store(encode(locale), Ordering::Relaxed);
}

/// 当前生效的 locale（`System` 时按 POSIX 变量，再按 Windows UI 语言）。
pub(crate) fn active() -> Locale {
    match LOCALE.load(Ordering::Relaxed) {
        2 => Locale::Chinese,
        1 => Locale::English,
        3 => Locale::Other,
        _ => {
            // POSIX 变量优先（源码 `set_i18n_envvars` 只在未设置时回填）。
            for var in ["LANG", "LANGUAGE"] {
                if let Ok(lang) = std::env::var(var) {
                    if !lang.trim().is_empty() {
                        return if lang.to_ascii_lowercase().contains("zh") {
                            Locale::Chinese
                        } else if lang.to_ascii_lowercase().starts_with("en") {
                            Locale::English
                        } else {
                            Locale::Other
                        };
                    }
                }
            }
            if system_locale_is_chinese() {
                Locale::Chinese
            } else {
                Locale::English
            }
        }
    }
}

/// 当前完整 locale 代码（用于 `.mo` 文件名）。
pub(crate) fn current_code() -> String {
    if let Some(code) = code_slot().lock().unwrap().clone() {
        return code;
    }
    for var in ["LANG", "LANGUAGE"] {
        if let Ok(lang) = std::env::var(var) {
            let code = lang.split(['.', '@']).next().unwrap_or("").trim();
            if !code.is_empty() {
                return code.replace('-', "_");
            }
        }
    }
    system_locale_code()
}

#[cfg(windows)]
fn system_locale_code() -> String {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    if n > 1 {
        let s = String::from_utf16_lossy(&buf[..n as usize - 1]);
        return s.replace('-', "_");
    }
    "en_US".to_string()
}

#[cfg(not(windows))]
fn system_locale_code() -> String {
    "en_US".to_string()
}

/// Windows 语言 ID 的主语言是否为中文（`LANG_CHINESE = 0x04`）。
fn chinese_language_id(lang_id: u16) -> bool {
    lang_id & 0x3ff == 0x04
}

/// 系统 UI 语言是否中文。
///
/// 源码 `lib/i18n.py:90-140` 在 Windows 上读
/// `GetUserDefaultUILanguage()` / `GetSystemDefaultUILanguage()`，因为 Windows
/// 默认不设置 `LANG`。本移植之前只看 `LANG`，所以中文 Windows 上菜单一直是
/// 英文；这里按源码同样的顺序补上。
#[cfg(windows)]
fn system_locale_is_chinese() -> bool {
    use windows_sys::Win32::Globalization::{GetSystemDefaultUILanguage, GetUserDefaultUILanguage};
    // SAFETY: 两个函数都无参数、无副作用，返回 Win32 LANGID。
    let user = unsafe { GetUserDefaultUILanguage() };
    if chinese_language_id(user) {
        return true;
    }
    let system = unsafe { GetSystemDefaultUILanguage() };
    chinese_language_id(system)
}

#[cfg(not(windows))]
fn system_locale_is_chinese() -> bool {
    false
}

/// 当前界面是否中文；用于带占位符的动态句子。
pub(crate) fn is_chinese() -> bool {
    active() == Locale::Chinese
}

/// 替换模板中的占位符（`tr_fmt!` 的实现核心，纯函数便于测试）。
pub(crate) fn fill_placeholders(mut text: String, args: &[(&str, String)]) -> String {
    for (key, value) in args {
        text = text.replace(key, value);
    }
    text
}

/// 翻译带占位符的模板并替换参数。
///
/// Rust 的 `format!` 要求字面量格式串，不能直接吃 `tr()` 的返回值；这个宏
/// 先查表再用 `str::replace` 填 `{}` / `{e}` / `{i}` 等占位符。
#[macro_export]
macro_rules! tr_fmt {
    ($id:expr $(, $key:literal => $val:expr)* $(,)?) => {
        $crate::i18n::fill_placeholders(
            $crate::i18n::tr($id).to_string(),
            &[$(( $key, $val.to_string() )),*],
        )
    };
}

/// 翻译一个 msgid；当前语言表里没有就回退原文。
pub(crate) fn tr(msgid: &str) -> &str {
    match active() {
        Locale::Chinese => zh_local(msgid)
            .or_else(|| mo_translate(&current_code(), msgid))
            .or_else(|| catalog().get(msgid).copied())
            .unwrap_or(msgid),
        Locale::English => msgid,
        Locale::Other => mo_translate(&current_code(), msgid).unwrap_or(msgid),
        Locale::System => translate(Locale::System, msgid),
    }
}

fn zh_local(msgid: &str) -> Option<&'static str> {
    ZH_LOCAL.iter().find(|(k, _)| *k == msgid).map(|(_, v)| *v)
}

fn translate(locale: Locale, msgid: &str) -> &str {
    if locale == Locale::Chinese {
        catalog().get(msgid).copied().unwrap_or(msgid)
    } else {
        msgid
    }
}

// ---------------------------------------------------------------------------
// GNU gettext `.mo`（安装版的 83 个语言目录）
// ---------------------------------------------------------------------------

static MO_CACHE: OnceLock<Mutex<HashMap<String, &'static HashMap<String, &'static str>>>> =
    OnceLock::new();

fn mo_cache() -> &'static Mutex<HashMap<String, &'static HashMap<String, &'static str>>> {
    MO_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 按 locale 代码查安装版 `mypaint.mo`；找不到返回 `None`。
fn mo_translate(code: &str, msgid: &str) -> Option<&'static str> {
    let table = {
        let mut cache = mo_cache().lock().unwrap();
        if !cache.contains_key(code) {
            let loaded = load_mo_for(code)
                .map(|m| Box::leak(Box::new(m)) as &'static HashMap<String, &'static str>);
            cache.insert(
                code.to_string(),
                loaded.unwrap_or_else(|| Box::leak(Box::new(HashMap::new()))),
            );
        }
        cache[code]
    };
    table.get(msgid).copied()
}

fn load_mo_for(code: &str) -> Option<HashMap<String, &'static str>> {
    let mut candidates = Vec::new();
    let base = code.split(['_', '@']).next().unwrap_or(code).to_string();
    let mut codes = vec![code.to_string()];
    if base != code {
        codes.push(base);
    }
    for dir in locale_dirs() {
        for c in &codes {
            candidates.push(dir.join(c).join("LC_MESSAGES").join("mypaint.mo"));
        }
    }
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Some(table) = parse_mo(&bytes) {
                return Some(table);
            }
        }
    }
    None
}

fn locale_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("PIGMENT_LOCALE_DIR") {
        if !dir.trim().is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }
    // 安装版固定路径（本机参考安装）。
    dirs.push(PathBuf::from(r"E:\mypaint-w64\mingw64\share\locale"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("share").join("locale"));
            dirs.push(parent.join("..").join("share").join("locale"));
        }
    }
    dirs
}

/// 解析 GNU MO（`lib/gettext` 的二进制格式），返回 UTF-8 msgid/msgstr。
fn parse_mo(bytes: &[u8]) -> Option<HashMap<String, &'static str>> {
    if bytes.len() < 28 {
        return None;
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let (le, be) = match magic {
        0x9504_12de => (true, false),
        0xde12_0495 => (false, true),
        _ => return None,
    };
    let read = |off: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(off..off + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else if be {
            u32::from_be_bytes(b)
        } else {
            return None;
        })
    };
    let n = read(8)? as usize;
    let orig_off = read(12)? as usize;
    let trans_off = read(16)? as usize;
    let mut out = HashMap::new();
    for i in 0..n {
        let olen = read(orig_off + i * 8)? as usize;
        let ooff = read(orig_off + i * 8 + 4)? as usize;
        let tlen = read(trans_off + i * 8)? as usize;
        let toff = read(trans_off + i * 8 + 4)? as usize;
        let msgid = std::str::from_utf8(bytes.get(ooff..ooff + olen)?).ok()?;
        let msgstr = std::str::from_utf8(bytes.get(toff..toff + tlen)?).ok()?;
        if msgid.is_empty() || msgstr.is_empty() {
            continue;
        }
        let k: &'static str = Box::leak(msgid.to_string().into_boxed_str());
        let v: &'static str = Box::leak(msgstr.to_string().into_boxed_str());
        out.insert(k.to_string(), v);
    }
    Some(out)
}

/// 本项目自有的界面串（不在官方 po 里）的中文译文。
const ZH_LOCAL: &[(&str, &str)] = &[
    ("Revert to Saved", "还原(R)"),
    // 参考机 File 菜单的 F5 条目显示“还原”，不是“恢复”。
    ("Reload", "还原"),
    // 参考机 View 菜单的 GtkToggleAction 显示“对称绘画激活”。
    ("Symmetry Active", "对称绘画激活"),
    ("Global Pressure Mapping", "全局压力映射"),
    ("Brush Pressure", "笔刷压力"),
    ("Brush Pressure 1", "笔刷压力 1"),
    ("Input Pressure", "输入压力"),
    // 官方 zh_CN.po 的现存条目把 Scratchpad 译成“调色板”，但同一文件末尾的
    // obsolete 条目是“暂存区”，实际 MyPaint 界面也用“暂存区”。以界面为准。
    ("Scratchpad", "暂存区"),
    // 自动恢复对话框（autorecover.glade）在本项目是手写 egui 文本；
    // po 里的 msgid 带下划线助记符，补一组与界面完全一致的键。
    (
        "MyPaint crashed with unsaved work, but it makes rolling backups. \
         You can recover a working file from one of these automated backups now. \
         You’ll be asked where to save it before continuing.",
        "MyPaint 崩溃时留下了未保存的工作，但它会做滚动备份。\n\
         你可以从这些自动备份中恢复一个工作文件；继续前会先询问保存位置。",
    ),
    ("Look for backups at startup", "在启动时寻找备份"),
    ("Delete…", "删除…"),
    ("Delete this backup from the disk.", "从磁盘删除此备份。"),
    ("Recover & Save…", "恢复并保存…"),
    (
        "Save this backup to disk, then continue working on it.",
        "将此备份保存到磁盘，然后继续处理它。",
    ),
    ("Decide Later", "稍后决定"),
    ("Export Group…", "导出组…"),
    ("Clone…", "克隆…"),
    ("Delete Brush…", "删除笔刷…"),
    ("Create", "创建"),
    ("Delete Brush", "删除笔刷"),
    ("Delete", "删除"),
    ("Cancel", "取消"),
    ("Close", "关闭"),
    ("Website", "网站"),
    ("Credits", "鸣谢"),
    ("Open Anyway", "仍然打开"),
    ("Save to Scraps first", "先保存到暂存区"),
    ("This file may be incompatible", "此文件可能不兼容"),
    ("Test Input Devices", "测试输入设备"),
    ("Hex:", "十六进制："),
    ("Save as Default", "保存为默认"),
    ("Set current color", "设置当前颜色"),
    ("Import", "导入"),
    ("Line pressures", "线条压力"),
    ("Entrance Pressure:", "起笔压力："),
    ("Midpoint Pressure:", "中点压力："),
    ("Exit Pressure:", "收笔压力："),
    ("Head:", "头："),
    ("Tail:", "尾："),
    ("No options available", "没有可用的选项"),
    ("Pop out into a floating window", "弹出为浮动窗口"),
    ("Dock to Left", "停靠到左侧"),
    ("Dock to Right", "停靠到右侧"),
    ("Action:", "动作："),
    ("Button press:", "按键："),
    ("Reset to Any Task", "重置为任意任务"),
    ("Load…", "载入…"),
    ("Save…", "保存…"),
    ("editing — changes not yet saved", "编辑中 — 修改尚未保存"),
    (
        "editing — paint with any brush or color",
        "编辑中 — 用任意笔刷或颜色绘制",
    ),
    ("Click Edit to make changes to the icon", "点“编辑”修改图标"),
    ("Set Frame to Layer", "画框设为图层"),
    ("Set Frame to Document", "画框设为文档"),
    ("Target", "目标"),
    ("Blend Mode", "混合模式"),
    ("Max Gap Size", "最大间隙"),
    ("Retract Seeps", "回缩渗漏"),
    ("No selected point", "未选点"),
    ("Edit Mask", "编辑遮罩"),
    ("No Recent Files", "没有最近文件"),
    ("Enter = OK, Esc = Cancel", "回车 = 确定，Esc = 取消"),
    ("OK", "确定(O)"),
    ("Canceled", "已取消"),
    ("New document", "新建文档"),
    ("Subwindows restored", "子窗口已恢复"),
    (
        "Subwindows hidden (press again to restore)",
        "子窗口已隐藏（再按一次恢复）",
    ),
    ("Nothing to undo", "没有可撤销的操作"),
    ("Nothing to redo", "没有可重做的操作"),
    ("Canceled current operation", "已取消当前操作"),
    ("Backup deleted", "已删除备份"),
    ("File not found: {}", "文件不存在：{}"),
    ("Failed to delete backup: {e}", "删除备份失败：{e}"),
    ("Failed to restore backup: {e}", "恢复备份失败：{e}"),
    ("Brush pack import failed: {e}", "笔刷包导入失败：{e}"),
    ("New vector layer failed: {e}", "新建矢量层失败：{e}"),
    (
        "Failed to create external-edit directory: {e}",
        "创建外部编辑目录失败：{e}",
    ),
    (
        "Failed to write vector temp file: {e}",
        "写矢量临时文件失败：{e}",
    ),
    (
        "Failed to write layer temp file: {e}",
        "写图层临时文件失败：{e}",
    ),
    ("Layer edit failed: {e}", "编辑图层失败：{e}"),
    ("Rename failed: {e}", "重命名失败：{e}"),
    ("Save failed: {e}", "保存失败：{e}"),
    ("Open failed: {e}", "打开失败：{e}"),
    ("Failed to open scratchpad: {e}", "打开暂存页失败：{e}"),
    ("Failed to save scratchpad: {e}", "暂存页保存失败：{e}"),
    ("Failed to export brush pack: {e}", "导出笔刷包失败：{e}"),
    ("Failed to save mask: {e}", "保存遮罩失败：{e}"),
    ("Failed to parse palette: {e}", "解析调色板失败：{e}"),
    ("Failed to read mask: {e}", "读取遮罩失败：{e}"),
    ("Failed to import palette: {e}", "导入调色板失败：{e}"),
    ("Failed to read palette: {e}", "读取调色板失败：{e}"),
    ("Failed to export palette: {e}", "导出调色板失败：{e}"),
    ("Undo: {name}", "撤销：{name}"),
    ("Redo: {name}", "重做：{name}"),
    ("Slot {i} is empty", "槽位 {i} 为空"),
    ("Brush {i} ← {name}", "笔刷 {i} ← {name}"),
    (
        "Brush {i} ← current settings saved",
        "笔刷 {i} ← 已存入当前设置",
    ),
    ("Failed to save slot {i}: {e}", "槽位 {i} 保存失败：{e}"),
    ("Simplified {n} points", "简化掉 {n} 个点"),
    ("Culled {n} points", "剔除 {n} 个点"),
    ("Symmetry: {state}", "对称绘画：{state}"),
    (
        "Frame: {state} ({x},{y} {w}×{h})",
        "画框：{state}（{x},{y} {w}×{h}）",
    ),
    (
        "Color added to palette ({len} colors)",
        "当前色已加入调色板（共 {len} 色）",
    ),
    ("Scratchpad undo: {name}", "暂存区撤销：{name}"),
    ("Scratchpad redo: {name}", "暂存区重做：{name}"),
    ("Failed to import layers: {e}", "导入图层失败：{e}"),
    (
        "Failed to open brush pack page: {e}",
        "打开笔刷包页面失败：{e}",
    ),
    ("Failed to open online help: {e}", "打开在线帮助失败：{e}"),
    ("Failed to create scratchpad: {e}", "暂存区新建失败：{e}"),
    ("Failed to save scratchpad as: {e}", "暂存区另存失败：{e}"),
    ("Failed to revert scratchpad: {e}", "暂存区回退失败：{e}"),
    (
        "Failed to save default scratchpad: {e}",
        "默认暂存页保存失败：{e}",
    ),
    (
        "Restore brush slot color too: {state}",
        "恢复笔刷槽位时连颜色一起还原：{state}",
    ),
    ("Spectral mixing: {state}", "光谱混色：{state}"),
    (
        "Failed to create scrap directory ({}): {e}",
        "建 scrap 目录失败（{}）：{e}",
    ),
    ("Scrap saved: {}", "已保存 scrap：{}"),
    ("Mask saved: {}", "已保存遮罩：{}"),
    ("{layer_count} layers", "{layer_count} 层"),
    ("{group_count} groups", "{group_count} 组"),
    ("{i} ← empty slot", "{i} ← 空槽位"),
    ("Opened scratchpad: {}", "已打开暂存页：{}"),
    ("Scratchpad saved as: {}", "暂存页已另存为：{}"),
    ("Exported palette: {}", "已导出调色板：{}"),
    (
        "Imported palette {} ({n} colors)",
        "已导入调色板 {}（{n} 色）",
    ),
    ("Scratchpad saved: {}", "暂存区已保存：{}"),
    ("Scratchpad reverted: {}", "暂存区已回退：{}"),
    ("Scratchpad opened: {}", "暂存区已打开：{}"),
    (
        "Scratchpad has not been saved yet: {}",
        "暂存区还没存过盘：{}",
    ),
    ("Saved as default scratchpad: {}", "已存为默认暂存页：{}"),
    (
        "Failed to launch external editor: {e}",
        "启动外部编辑器失败：{e}",
    ),
    ("Autosave failed: {e}", "自动备份失败：{e}"),
    ("Recovered from backup: {}", "已从备份恢复：{}"),
    ("Import brush pack “{name}”?", "导入笔刷包“{name}”？"),
    ("Opened PNG {w}×{h}", "已打开 {w}×{h} 的 PNG"),
    ("Opened JPEG {w}×{h}", "已打开 {w}×{h} 的 JPEG"),
    ("Directory does not exist: {}", "目录不存在：{}"),
    ("Exported {n} brushes to {}", "已导出 {n} 支笔刷到 {}"),
    (
        "Loaded mask ({} shapes / {n} points)",
        "已载入遮罩（{} 个形状 / {n} 点）",
    ),
    ("Failed to write brush: {e}", "写入笔刷失败：{e}"),
    ("Failed to save accel map: {e}", "加速键保存失败：{e}"),
    ("Conflict: {who}", "冲突：{who}"),
    ("Reset {n} overrides", "已重置 {n} 条覆盖"),
    ("Deleted shortcuts for {path}", "已删除 {path} 的按键"),
    (
        "Failed to create background directory {}: {e}",
        "创建背景目录 {} 失败：{e}",
    ),
    ("Failed to write background {}: {e}", "写背景 {} 失败：{e}"),
    (
        "Failed to save preferences ({}): {e}",
        "保存偏好失败（{}）：{e}",
    ),
    ("Color history {i}/{n}", "颜色历史 {i}/{n}"),
    (
        "Cannot delete group: {name} (special groups cannot be deleted)",
        "无法删除分组：{name}（特殊分组不可删除）",
    ),
    ("Save failed: {e}", "保存失败: {e}"),
    (
        "Z: {scale}%\nR: {rotation}°",
        "缩放：{scale}%\n旋转：{rotation}°",
    ),
    (
        ", {} items renamed due to duplicates",
        "，{} 项因重名已改名",
    ),
    (" ({} replaced)", "（替换了 {} 条）"),
    (
        "This panel has not been ported yet. {note}",
        "此面板尚未移植。{note}",
    ),
    (
        "{}\nNot available with current settings",
        "{}\n当前设置下不可用",
    ),
    ("Clear curve", "清零"),
    ("Reset to linear", "重置为线性"),
    ("Delete this curve", "删除该曲线"),
    ("Save", "保存"),
    ("Reload", "重新载入"),
    ("Reset all", "全部重置"),
    (
        "Rescan brush directories (port-only)",
        "重新扫描笔刷目录（本项目新增）",
    ),
    ("Hue", "色相"),
    ("Saturation", "饱和"),
    ("Value", "明度"),
    ("This group has no brushes", "该分组没有笔刷"),
    ("Settings", "设置"),
    (
        "This setting is constant; add an input curve to make it vary with pressure/speed",
        "该设置为常量：没有输入曲线，加一条即可让它随压感/速度变化",
    ),
    ("Input curve", "输入曲线"),
    (
        "Brush icon: bundled 128×128 preview (MyPaint does not redraw it when settings change)",
        "笔刷图标：随附的 128×128 预览图（MyPaint 同上，不随设置改动重画）",
    ),
    ("Reset", "重置"),
    ("(no brushes in this group)", "（这一组没有笔刷）"),
    (
        "OK applies the current color; Cancel leaves it unchanged.",
        "OK 应用当前颜色；Cancel 不改。",
    ),
    ("Most recent last", "最近用的在最后"),
    ("(appears after a stroke)", "（画一笔后出现）"),
    (
        "Recorded after each stroke; only the latest settings per brush are kept",
        "每笔结束后记录；同一支笔只保留最新设置",
    ),
    ("Brush pack import canceled", "已取消笔刷包导入"),
    (
        "New vector layer failed: invalid insert point",
        "新建矢量层失败：插入点无效",
    ),
    ("Groups cannot be edited externally", "组不支持外部编辑"),
    ("No external edit in progress", "没有正在进行的外部编辑"),
    (
        "External edit produced no applicable changes",
        "外部编辑没有产生可应用的改动",
    ),
    (
        "Pick Context: pointer is not over the canvas",
        "Pick Context：指针不在画布上",
    ),
    (
        "Pick Context: no pickable layer here",
        "Pick Context：这里没有可拾取的图层",
    ),
    (
        "Pick Layer: pointer is not over the canvas",
        "Pick Layer：指针不在画布上",
    ),
    (
        "Pick Layer: no pickable layer here",
        "Pick Layer：这里没有可拾取的图层",
    ),
    ("Imported layers", "已导入图层（Imported layers）"),
    ("No layers to import", "没有可导入的图层"),
    (
        "Cannot merge down: both layers must be visible and the lower one unlocked",
        "无法向下合并：两层都须可见、下层须未锁",
    ),
    (
        "Flood Fill: gap detection is on",
        "Flood Fill: 缺口检测已开启",
    ),
    (
        "Color tools: nine independent tools",
        "颜色器：九个独立工具",
    ),
    (
        "Copied the main background color to the scratchpad",
        "已把主文档的背景色复制到暂存区",
    ),
    ("No brush slot selected", "没有选中的笔刷槽位"),
    (
        "Current layer is a group; no surface to export",
        "当前层是组，没有可导出的表面",
    ),
    (
        "Color history is empty (appears after a stroke)",
        "颜色历史还是空的（画一笔后出现）",
    ),
    (
        "Nothing to undo in the scratchpad",
        "暂存区没有可撤销的操作",
    ),
    (
        "Nothing to redo in the scratchpad",
        "暂存区没有可重做的操作",
    ),
    ("Current layer cannot be normalized", "当前层不可归一化"),
    ("No visible layers to merge", "没有可见层可合并"),
    ("Trimmed layer to frame", "已裁剪图层到画框"),
    ("Nothing to trim", "没有需要裁剪的图层内容"),
    ("Save As", "另存为"),
    ("Open", "打开"),
    ("Import Layers", "导入图层"),
    ("Export Image", "导出图像"),
    ("Open Scratchpad", "打开暂存页"),
    ("Export Scratchpad As", "暂存页另存为"),
    ("Import Palette", "导入调色板"),
    ("Export Palette", "导出调色板"),
    ("Import Brush Pack", "导入笔刷包"),
    ("Export Group as Brush Pack", "导出分组为笔刷包"),
    ("Save Mask as GIMP Palette", "保存遮罩为 GIMP 调色板"),
    ("Load Mask from GIMP Palette", "从 GIMP 调色板载入遮罩"),
    (
        "Path to open (.ora / .png):",
        "要打开的文件路径（.ora / .png）：",
    ),
    (
        "Layer files to import (separate with ;):",
        "要导入的图层文件路径（多个用 ; 分隔）：",
    ),
    ("Path to export PNG:", "导出 PNG 的路径："),
    (
        "Path to save (.ora / .png; .ora if no extension):",
        "保存路径（.ora / .png；不带后缀时按 .ora 处理）：",
    ),
    (
        "Path to open in scratchpad (.ora / .png):",
        "要打开到暂存区的文件路径（.ora / .png）：",
    ),
    (
        "Path to save scratchpad as (.ora if no extension):",
        "暂存页另存的路径（不带后缀时按 .ora 处理）：",
    ),
    (
        "Path to GIMP palette (.gpl):",
        "要导入的 GIMP 调色板路径（.gpl）：",
    ),
    (
        "Path to export palette (.gpl):",
        "导出调色板的路径（不带后缀时按 .gpl 处理）：",
    ),
    ("Path to brush pack (.zip):", "要导入的笔刷包路径（.zip）："),
    (
        "Path to export brush pack (.zip):",
        "导出笔刷包的路径（.zip）：",
    ),
    (
        "Path to save mask palette (.gpl):",
        "保存遮罩的 GIMP 调色板路径（.gpl）：",
    ),
    (
        "Path to load mask palette (.gpl):",
        "要载入的 GIMP 调色板路径（.gpl）：",
    ),
    ("Group Properties", "组属性"),
    ("Close Panel", "关闭面板"),
    ("pigment — a Rust/egui port", "pigment — Rust/egui 移植版"),
    (
        "This dialog is the crash-report surface of the port; the real \
         application would show a traceback and recovery options.",
        "这是移植版的崩溃报告界面；真实应用会显示 traceback 与恢复选项。",
    ),
    ("MyPaint crashed (simulated)", "MyPaint 崩溃了（模拟）"),
];

/// 解析后的官方中文表（只包含非空、非 fuzzy 的条目）。
fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = parse_po(include_str!("../assets/i18n/zh_CN.po"));
        // 本地补充串优先级更高（官方 po 里没有这些键）
        for (k, v) in ZH_LOCAL {
            table.insert(k, v);
        }
        table
    })
}

/// 极简 `.po` 解析：`msgid` / `msgstr` + 多行引号续行，跳过 fuzzy。
fn parse_po(text: &str) -> HashMap<&'static str, &'static str> {
    let mut out: HashMap<&'static str, &'static str> = HashMap::new();
    let mut id: Option<String> = None;
    let mut msgstr: Option<String> = None;
    let mut fuzzy = false;
    let mut target = 0u8; // 1 = msgid, 2 = msgstr

    let flush = |id: &mut Option<String>,
                 msgstr: &mut Option<String>,
                 fuzzy: &mut bool,
                 out: &mut HashMap<&'static str, &'static str>| {
        if !*fuzzy {
            if let (Some(i), Some(s)) = (id.take(), msgstr.take()) {
                if !i.is_empty() && !s.is_empty() {
                    let k: &'static str = Box::leak(i.into_boxed_str());
                    let v: &'static str = Box::leak(s.into_boxed_str());
                    out.insert(k, v);
                }
            }
        } else {
            *id = None;
            *msgstr = None;
        }
        *fuzzy = false;
    };

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            flush(&mut id, &mut msgstr, &mut fuzzy, &mut out);
            target = 0;
            continue;
        }
        if line.starts_with("#,") {
            if line.contains("fuzzy") {
                fuzzy = true;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("msgid_plural ") {
            target = 0;
            continue;
        }
        if line.starts_with("msgctxt ") {
            // 上下文不参与 key：同一个英文词在不同上下文里的译文取最后一条
            target = 0;
            continue;
        }
        if line.starts_with("msgstr[") {
            target = 0;
            continue;
        }
        if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut id, &mut msgstr, &mut fuzzy, &mut out);
            id = Some(unquote(rest));
            target = 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("msgstr ") {
            msgstr = Some(unquote(rest));
            target = 2;
            continue;
        }
        if line.starts_with('"') {
            let piece = unquote(line);
            match target {
                1 => {
                    if let Some(s) = id.as_mut() {
                        s.push_str(&piece);
                    }
                }
                2 => {
                    if let Some(s) = msgstr.as_mut() {
                        s.push_str(&piece);
                    }
                }
                _ => {}
            }
        }
    }
    flush(&mut id, &mut msgstr, &mut fuzzy, &mut out);
    out
}

/// 去掉一层引号并还原常见转义。
fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_catalog_parses_and_translates_known_labels() {
        let table = catalog();
        assert!(table.len() > 500, "词条太少: {}", table.len());
        assert_eq!(translate(Locale::Chinese, "Fullscreen"), "全屏");
        assert_eq!(translate(Locale::Chinese, "Brush"), "笔刷");
        // 英文 locale 永远回退原文
        assert_eq!(translate(Locale::English, "Fullscreen"), "Fullscreen");
        // 没有译文的 msgid 回退原文
        let unknown = "definitely not in the catalog";
        assert_eq!(translate(Locale::Chinese, unknown), unknown);
        // 本项目自有串（官方 po 没有）由本地表补上。
        // 参考机 Preferences 窗口的 GTK stock `_Revert to Saved` 显示为
        // “还原(R)”，不是“回退到已保存”。
        assert_eq!(translate(Locale::Chinese, "Revert to Saved"), "还原(R)");
        assert_eq!(
            translate(Locale::Chinese, "Global Pressure Mapping"),
            "全局压力映射"
        );
        assert_eq!(translate(Locale::Chinese, "Scratchpad"), "暂存区");
    }

    #[test]
    fn chinese_language_ids_are_recognized() {
        // zh_CN 0x0804、zh_TW 0x0404：主语言都是 LANG_CHINESE 0x04。
        assert!(chinese_language_id(0x0804));
        assert!(chinese_language_id(0x0404));
        // en_US 0x0409、ja_JP 0x0411 不是中文。
        assert!(!chinese_language_id(0x0409));
        assert!(!chinese_language_id(0x0411));
    }

    #[test]
    fn mo_parser_reads_msgid_msgstr_pairs() {
        // 一个最小 GNU MO：header + 1 组 (msgid, msgstr)。
        let mut b = Vec::new();
        b.extend_from_slice(&0x9504_12deu32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&28u32.to_le_bytes()); // orig table
        b.extend_from_slice(&36u32.to_le_bytes()); // trans table
        b.extend_from_slice(&0u32.to_le_bytes()); // hash size
        b.extend_from_slice(&0u32.to_le_bytes()); // hash offset
        b.extend_from_slice(&5u32.to_le_bytes()); // msgid len
        b.extend_from_slice(&44u32.to_le_bytes()); // msgid offset
        b.extend_from_slice(&7u32.to_le_bytes()); // msgstr len
        b.extend_from_slice(&49u32.to_le_bytes()); // msgstr offset
        b.extend_from_slice(b"Hello");
        b.extend_from_slice(b"Bonjour");
        let table = parse_mo(&b).expect("MO 解析");
        assert_eq!(table.get("Hello"), Some(&"Bonjour"));
    }

    #[test]
    fn other_locales_select_the_other_branch() {
        set_from_pref("fr_FR");
        assert_eq!(active(), Locale::Other);
        assert_eq!(current_code(), "fr_FR");
        assert_eq!(translate(Locale::Other, "Fullscreen"), "Fullscreen");
        set_from_pref("en_US");
        assert_eq!(active(), Locale::English);
    }

    #[test]
    fn placeholders_are_replaced_independently() {
        let text = fill_placeholders(
            "Opened {w}×{h}".to_string(),
            &[("{w}", "640".to_string()), ("{h}", "480".to_string())],
        );
        assert_eq!(text, "Opened 640×480");
        let text = fill_placeholders(
            "Frame: {state} ({x},{y} {w}×{h})".to_string(),
            &[
                ("{state}", "on".to_string()),
                ("{x}", "1".to_string()),
                ("{y}", "2".to_string()),
                ("{w}", "3".to_string()),
                ("{h}", "4".to_string()),
            ],
        );
        assert_eq!(text, "Frame: on (1,2 3×4)");
    }

    #[test]
    fn locale_pref_values_map_to_the_expected_locale() {
        for (value, want) in [
            ("zh_CN", Locale::Chinese),
            ("zh", Locale::Chinese),
            ("en_US", Locale::English),
            ("C", Locale::English),
        ] {
            set_from_pref(value);
            assert_eq!(active(), want, "pref {value:?}");
        }
        set_from_pref("en_US");
        assert_eq!(tr("Fullscreen"), "Fullscreen");
        set_from_pref("zh_CN");
        assert_eq!(tr("Fullscreen"), "全屏");
        set_from_pref("");
    }
}
