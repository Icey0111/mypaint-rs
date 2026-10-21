//! 自动备份与崩溃恢复 —— `lib/document.py:56-787` + `gui/autorecover.py`。
//!
//! # 源码结构
//!
//! 源码把备份写进**每文档一个的缓存目录**：
//!
//! ```text
//! <user cache>/mypaint/
//!   doc.XXXXXXXX/           # tempfile.mkdtemp(prefix="doc.")
//!     active                # 每 10s touch 一次的"还在用"金丝雀（:515-523）
//!     autosave/             # CACHE_DOC_AUTOSAVE_SUBDIR
//!       stack.xml           # AutosaveInfo 的元数据与有效性判据（:135-149）
//!       Thumbnails/thumbnail.png
//!       data/*.png          # 各层的增量数据
//! ```
//!
//! 备份本身是**分块、低优先级**的：每个脏图层一个任务，写进
//! `lib.idletask.Processor`（`:601-787`）。恢复对话框遍历缓存根下所有
//! `doc.*/autosave`（`:2186-2208`），跳过 `active` 还新鲜的目录
//! （`AutosaveInfo.cache_in_use`，`:169-176`），双击恢复后立刻弹"另存为"
//! （`autorecover.py:174-184`）。
//!
//! # 本移植的结构
//!
//! 我们的文档 I/O 是单文件 ORA（`Document::save`），没有分层的增量写队列。
//! 因此备份目录写成：
//!
//! ```text
//! <cache>/pigment/mypaint/doc.NN/
//!   active
//!   autosave/
//!     backup.ora          # 完整快照（AUTOSAVE_ORA_NAME）
//!     backup.json         # 尺寸/层数/未保存时长（AutosaveMeta）
//!     thumbnail.png       # 恢复对话框的预览图
//! ```
//!
//! 多出的 `pigment` 这一层是**应用本地隔离**：源码直接用 `<cache>/mypaint`，
//! 与已安装 MyPaint 的备份目录相同；两个程序同时跑时，对方的恢复对话框会把
//! 本程序的 `doc.*` 当成自己的崩溃残留。`PIGMENT_CACHE_DIR` 可覆盖整个根。
//!
//! 与源码的差异**只有写入粒度与缓存根前缀**：源码逐层增量写、可中断；
//! 这里每次全量写一个 ORA。目录结构、`active` 金丝雀、`cache_in_use` 判据、
//! 恢复后要求另存、删除备份的语义都照搬。全量写在大文档上会比源码慢，故只在
//! 用户停止操作 `autosave_interval` 秒后触发一次。
//!
//! `AutosaveInfo::valid` 的判据也照源码：备份存在且能读出元数据才算有效；
//! 读不出时仍列出来（源码 `:221-225` 的 "Incomplete backup" 文案）。

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pigment_surface::LayerStack;
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::fileio::{self, FileError, OraSaveOptions};

// ---- 源码常量（`lib/document.py:61-67`）----

/// `CACHE_APP_SUBDIR_NAME = "mypaint"`（`:63`）。
///
/// 这是 MyPaint 源码里的子目录名；本移植把它放在
/// [`CACHE_ROOT_SUBDIR_NAME`] 之下，避免与已安装的 MyPaint 共用
/// `%LOCALAPPDATA%\mypaint\doc.*`，导致对方的恢复对话框把本程序的
/// 备份当成自己的崩溃残留。
pub const CACHE_APP_SUBDIR_NAME: &str = "mypaint";
/// 本移植的缓存根目录名 —— **应用本地扩展**，源码没有这一层。
///
/// 最终路径：`<user cache>/pigment/mypaint/doc.NN/`。
pub const CACHE_ROOT_SUBDIR_NAME: &str = "pigment";
/// 覆盖缓存根目录 —— **应用本地扩展**（与 `PIGMENT_CONFIG_DIR` 同风格）。
pub const CACHE_DIR_ENV: &str = "PIGMENT_CACHE_DIR";
/// `CACHE_DOC_SUBDIR_PREFIX = "doc."`（`:64`）。
pub const CACHE_DOC_SUBDIR_PREFIX: &str = "doc.";
/// `CACHE_DOC_AUTOSAVE_SUBDIR = "autosave"`（`:65`）。
pub const CACHE_DOC_AUTOSAVE_SUBDIR: &str = "autosave";
/// `CACHE_ACTIVITY_FILE = "active"`（`:66`）。
pub const CACHE_ACTIVITY_FILE: &str = "active";
/// `CACHE_UPDATE_INTERVAL = 10` 秒（`:67`）。
pub const CACHE_UPDATE_INTERVAL: u64 = 10;

/// 完整快照的文件名（本移植的写入粒度，见模块文档）。
pub const AUTOSAVE_ORA_NAME: &str = "backup.ora";
/// 元数据文件名（替代源码 `stack.xml` 的属性读取）。
pub const AUTOSAVE_META_NAME: &str = "backup.json";
/// 恢复对话框的缩略图。
pub const AUTOSAVE_THUMB_NAME: &str = "thumbnail.png";

// ---- 路径 ----

/// 用户的缓存根 —— `GLib.get_user_cache_dir()`。
///
/// - Windows：`%LOCALAPPDATA%`（GLib 的 `g_get_user_cache_dir` 在 Windows
///   返回 `CSIDL_LOCAL_APPDATA`）；
/// - 其他：`$XDG_CACHE_HOME`，缺省 `~/.cache`。
pub fn user_cache_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("XDG_CACHE_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".cache");
        }
    }
    std::env::temp_dir()
}

/// 应用缓存根（`get_app_cache_root`，`:2168-2183`），**会建目录**。
///
/// 路径是 `<user cache>/pigment/mypaint`；`PIGMENT_CACHE_DIR` 可覆盖
/// 整个根目录。多出 `pigment` 这一层是本移植的隔离措施 —— 源码直接
/// 用 `<user cache>/mypaint`，与真实 MyPaint 的自动备份目录相同，
/// 两个程序同时跑时对方的恢复对话框会列出本程序的备份。
pub fn app_cache_root() -> io::Result<PathBuf> {
    let root = if let Some(dir) = std::env::var_os(CACHE_DIR_ENV).filter(|v| !v.is_empty()) {
        PathBuf::from(dir)
    } else {
        user_cache_dir()
            .join(CACHE_ROOT_SUBDIR_NAME)
            .join(CACHE_APP_SUBDIR_NAME)
    };
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

/// 建一个工作文档的缓存目录（`tempfile.mkdtemp(prefix="doc.")`，`:386-410`）。
///
/// 同时写 `active` 金丝雀（源码在创建后立刻写一次，`:404-409`）。
pub fn create_doc_cache_dir() -> io::Result<PathBuf> {
    let root = app_cache_root()?;
    for i in 0..10_000u32 {
        let dir = root.join(format!(
            "{CACHE_DOC_SUBDIR_PREFIX}{}-{i:04}",
            std::process::id()
        ));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                touch_activity(&dir)?;
                return Ok(dir);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "缓存目录名用尽（doc.<pid>-NNNN）",
    ))
}

/// 更新 `active` 金丝雀 —— `:519-520` 的 `os.utime(activity_file_path, None)`。
pub fn touch_activity(doc_cache_dir: &Path) -> io::Result<()> {
    let path = doc_cache_dir.join(CACHE_ACTIVITY_FILE);
    // 已存在时只更新时间戳；不存在时写一句说明（同源码 `:405-409`）
    if path.exists() {
        let now = SystemTime::now();
        std::fs::OpenOptions::new().write(true).open(&path)?;
        filetime_set(&path, now)
    } else {
        std::fs::write(
            &path,
            "A recent timestamp on this file indicates that\n\
             its containing cache subfolder is active.\n",
        )
    }
}

fn filetime_set(path: &Path, _now: SystemTime) -> io::Result<()> {
    // 标准库没有 utime；重新写一次文件内容即可更新 mtime（内容是固定说明）。
    let body = "A recent timestamp on this file indicates that\n\
                its containing cache subfolder is active.\n";
    std::fs::write(path, body)
}

/// 某个文档缓存目录下的备份目录（`<cache_dir>/autosave`）。
pub fn autosave_dir(doc_cache_dir: &Path) -> PathBuf {
    doc_cache_dir.join(CACHE_DOC_AUTOSAVE_SUBDIR)
}

/// 递归删除工作文档的缓存目录（`_cleanup_cache_dir`，`:412-430`）。
///
/// 源码在文档关闭/应用退出时调用；`ignore_errors` 的语义照搬（删不掉就
/// 算了，下次恢复对话框会把它列出来）。
pub fn cleanup_doc_cache_dir(doc_cache_dir: &Path) {
    let _ = std::fs::remove_dir_all(doc_cache_dir);
}

// ---- 元数据 ----

/// 备份的元数据（本移植替代 `stack.xml` 的属性读取）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AutosaveMeta {
    pub width: u32,
    pub height: u32,
    pub num_layers: usize,
    /// 写备份时文档累计的"未保存作画时长"（`unsaved_painting_time`）。
    pub unsaved_painting_time: f64,
    /// Unix 秒；恢复对话框的排序/描述用。
    pub saved_at: u64,
}

/// 读备份元数据；缺失/损坏返回 `None`（此时 `AutosaveInfo.valid = false`）。
pub fn read_meta(autosave_dir: &Path) -> Option<AutosaveMeta> {
    let text = std::fs::read_to_string(autosave_dir.join(AUTOSAVE_META_NAME)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 一份可恢复的备份 —— `AutosaveInfo`（`lib/document.py:112-187`）。
#[derive(Clone, Debug)]
pub struct AutosaveInfo {
    /// 备份目录本身（`.../doc.NN/autosave`）。
    pub path: PathBuf,
    /// 完整快照文件。
    pub ora_path: PathBuf,
    /// 缩略图文件（可能不存在）。
    pub thumbnail_path: PathBuf,
    /// `stack.xml`（这里是 `backup.ora`）的 mtime。
    pub last_modified: SystemTime,
    pub width: u32,
    pub height: u32,
    pub num_layers: usize,
    pub unsaved_painting_time: f64,
    /// 目录结构看起来完整（有快照 + 元数据能读）。
    pub valid: bool,
    /// 缓存目录可能仍被另一个实例使用（`active` 新鲜）。
    pub cache_in_use: bool,
}

impl AutosaveInfo {
    /// 从备份目录读一份信息 —— `AutosaveInfo.new_for_path`（`:127-187`）。
    pub fn new_for_path(path: &Path) -> Option<Self> {
        if !path.is_dir() {
            return None;
        }
        let ora_path = path.join(AUTOSAVE_ORA_NAME);
        let meta = read_meta(path);
        let valid = meta.is_some() && ora_path.is_file();
        let last_modified = std::fs::metadata(&ora_path)
            .and_then(|m| m.modified())
            .or_else(|_| std::fs::metadata(path).and_then(|m| m.modified()))
            .unwrap_or(UNIX_EPOCH);
        let cache_dir = path.parent()?;
        let cache_in_use = cache_activity_fresh(cache_dir);
        let meta = meta.unwrap_or(AutosaveMeta {
            width: 0,
            height: 0,
            num_layers: 0,
            unsaved_painting_time: 0.0,
            saved_at: 0,
        });
        Some(Self {
            path: path.to_path_buf(),
            ora_path,
            thumbnail_path: path.join(AUTOSAVE_THUMB_NAME),
            last_modified,
            width: meta.width,
            height: meta.height,
            num_layers: meta.num_layers,
            unsaved_painting_time: meta.unsaved_painting_time.max(0.0),
            valid,
            cache_in_use,
        })
    }

    /// 人类可读的描述 —— `get_description()`（`:189-244`）。
    ///
    /// 文案与源码同结构：三行（更新时间 / 尺寸与层数 / 未保存作画时长）；
    /// 无效备份用 "Incomplete backup" 一行。
    pub fn description(&self, now: SystemTime) -> String {
        let ago = if now >= self.last_modified {
            human_time_period_abbr(now.duration_since(self.last_modified).unwrap_or_default())
        } else {
            human_time_period_abbr(self.last_modified.duration_since(now).unwrap_or_default())
        };
        let ago_word = if now >= self.last_modified {
            "ago"
        } else {
            "from now"
        };
        if self.cache_in_use {
            return format!(
                "Cache folder still may be in use.\n\
                 Are you running more than once instance of MyPaint?\n\
                 Close app and wait {CACHE_UPDATE_INTERVAL}s to retry."
            );
        }
        if !self.valid {
            return format!("Incomplete backup updated {ago} {ago_word}");
        }
        format!(
            "Backup updated {ago} {ago_word}\n\
             Size: {}×{} pixels, Layers: {}\n\
             Contains {} of unsaved painting.",
            self.width,
            self.height,
            self.num_layers,
            human_time_period_abbr(Duration::from_secs_f64(self.unsaved_painting_time.max(0.0))),
        )
    }
}

/// `lib/helpers.py` 的 `fmt_time_period_abbr`：`8s` / `3h42m` / `2d3h`。
pub fn human_time_period_abbr(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        let s = secs % 60;
        if s == 0 {
            return format!("{mins}m");
        }
        return format!("{mins}m{s}s");
    }
    let hours = mins / 60;
    let m = mins % 60;
    if hours < 24 {
        if m == 0 {
            return format!("{hours}h");
        }
        return format!("{hours}h{m}m");
    }
    let days = hours / 24;
    let h = hours % 24;
    if h == 0 {
        format!("{days}d")
    } else {
        format!("{days}d{h}h")
    }
}

/// `active` 文件是否新鲜（`CACHE_UPDATE_INTERVAL + 3` 秒内）——
/// `AutosaveInfo.new_for_path` 的 `cache_in_use` 判据（`:169-176`）。
pub fn cache_activity_fresh(doc_cache_dir: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(doc_cache_dir.join(CACHE_ACTIVITY_FILE)) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    match SystemTime::now().duration_since(modified) {
        Ok(d) => d.as_secs() <= CACHE_UPDATE_INTERVAL + 3,
        Err(_) => false,
    }
}

// ---- 写备份 ----

/// 交给后台线程的一份自动备份快照。
///
/// `LayerStack::clone` 只复制 tile 的 `Arc`（像素写时复制），所以拍快照
/// 很便宜，后台线程读到的也是一份**不会随继续作画而改变**的稳定视图。
pub struct AutosaveSnapshot {
    pub layers: LayerStack,
    pub bbox: (i32, i32, i32, i32),
    pub options: OraSaveOptions,
    pub num_layers: usize,
    pub unsaved_painting_time: f64,
}

/// 把文档写进备份目录 —— `_queue_autosave_writes`（`:601-787`）的单文件版。
///
/// 走 [`Document::autosave_snapshot`]；**不清零** `unsaved_painting_time`
/// （源码的自动备份也不清零，只有显式 `Document.save` 才清）。
pub fn write_backup(doc: &Document, doc_cache_dir: &Path) -> Result<AutosaveInfo, FileError> {
    write_backup_snapshot(doc.autosave_snapshot(), doc_cache_dir)
}

/// 后台线程入口：把快照写进 `doc_cache_dir/autosave/`。
///
/// 与手动 `save_ora` 的区别只在于省掉 `mergedimage.png` 和整幅
/// `background.png`（恢复不读它们），大幅降低超大画幅备份的内存与时间；
/// 图层 PNG / stack.xml / 缩略图仍然完整。
pub fn write_backup_snapshot(
    snapshot: AutosaveSnapshot,
    doc_cache_dir: &Path,
) -> Result<AutosaveInfo, FileError> {
    let dir = autosave_dir(doc_cache_dir);
    std::fs::create_dir_all(&dir)?;
    let ora_path = dir.join(AUTOSAVE_ORA_NAME);
    // 先写临时文件再改名：崩溃时不会留下半个 ORA（源码的 chunked 写天然
    // 有这样的效果 —— 任务队列只在完整层数据写完后才更新 manifest）。
    let tmp = dir.join(format!("{AUTOSAVE_ORA_NAME}.tmp.ora"));
    let _ = std::fs::remove_file(&tmp);
    let saved = fileio::save_ora(&snapshot.layers, &tmp, &snapshot.options, snapshot.bbox)?;
    std::fs::rename(&tmp, &ora_path)?;

    let (_, _, w, h) = saved.bbox;
    let meta = AutosaveMeta {
        width: w.max(0) as u32,
        height: h.max(0) as u32,
        num_layers: snapshot.num_layers,
        unsaved_painting_time: snapshot.unsaved_painting_time,
        saved_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let text = serde_json::to_string_pretty(&meta).unwrap_or_else(|_| "{}".into());
    std::fs::write(dir.join(AUTOSAVE_META_NAME), text)?;
    // 缩略图：`save_ora` 已经渲染过同一份 `Thumbnails/thumbnail.png`
    // （上限 256×256），直接用返回值，别再对整栈跑第二遍。
    let (px, tw, th) = saved.thumbnail;
    if let Some(img) = image::RgbaImage::from_raw(tw, th, px) {
        let _ = img.save_with_format(dir.join(AUTOSAVE_THUMB_NAME), image::ImageFormat::Png);
    }
    touch_activity(doc_cache_dir)?;
    AutosaveInfo::new_for_path(&dir)
        .ok_or_else(|| FileError::Malformed(format!("备份目录不完整：{}", dir.display())))
}

/// 列出所有可恢复的备份 —— `get_available_autosaves()`（`:2186-2208`）。
///
/// 按 `last_modified` **升序**返回（源码恢复对话框再 `reversed()` 显示）。
pub fn available_autosaves() -> Vec<AutosaveInfo> {
    let Ok(root) = app_cache_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<AutosaveInfo> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(CACHE_DOC_SUBDIR_PREFIX)
        })
        .filter_map(|e| AutosaveInfo::new_for_path(&autosave_dir(&e.path())))
        .collect();
    out.sort_by(|a, b| a.last_modified.cmp(&b.last_modified));
    out
}

/// 删除一份备份 —— `autorecover.py:215-230` 的 `shutil.rmtree`。
pub fn delete_autosave(info: &AutosaveInfo) -> io::Result<()> {
    std::fs::remove_dir_all(&info.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-autosave-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn time_period_abbr_matches_helpers_py() {
        assert_eq!(human_time_period_abbr(Duration::from_secs(8)), "8s");
        assert_eq!(human_time_period_abbr(Duration::from_secs(60)), "1m");
        assert_eq!(human_time_period_abbr(Duration::from_secs(125)), "2m5s");
        assert_eq!(human_time_period_abbr(Duration::from_secs(3600)), "1h");
        assert_eq!(
            human_time_period_abbr(Duration::from_secs(3 * 3600 + 42 * 60)),
            "3h42m"
        );
        assert_eq!(human_time_period_abbr(Duration::from_secs(86400)), "1d");
        assert_eq!(
            human_time_period_abbr(Duration::from_secs(2 * 86400 + 3 * 3600)),
            "2d3h"
        );
    }

    #[test]
    fn backup_roundtrip_lists_and_describes() {
        let root = temp_cache("roundtrip");
        let doc_dir = root.join("doc.test");
        std::fs::create_dir_all(autosave_dir(&doc_dir)).unwrap();
        touch_activity(&doc_dir).unwrap();
        assert!(cache_activity_fresh(&doc_dir), "刚写的 active 应算在用");

        let mut doc = Document::new();
        doc.add_layer();
        doc.unsaved_painting_time = 42.0;
        let info = write_backup(&doc, &doc_dir).unwrap();
        assert!(info.valid);
        // 写备份的那一刻本进程还活着，`active` 是新鲜的 → 源码同样把它算作
        // "可能仍在使用"（恢复对话框据此跳过当前文档的缓存目录）。
        assert!(info.cache_in_use);
        assert_eq!(info.num_layers, doc.layers().len());
        assert!(info.ora_path.is_file());
        assert!(info.thumbnail_path.is_file());
        assert_eq!(info.unsaved_painting_time, 42.0);

        // 模拟"另一个实例已退出"：删掉金丝雀后再读，描述才是可恢复文案
        std::fs::remove_file(doc_dir.join(CACHE_ACTIVITY_FILE)).unwrap();
        let info = AutosaveInfo::new_for_path(&autosave_dir(&doc_dir)).unwrap();
        assert!(!info.cache_in_use);
        let desc = info.description(SystemTime::now());
        assert!(desc.contains("Layers:"), "{desc}");
        assert!(desc.contains("42s of unsaved painting"), "{desc}");

        // 自动备份省掉两份画幅级预览：merged 和整幅 background。
        // 图层 PNG / tile / stack.xml 仍必须齐全，且能被载入。
        let zip = crate::zip::ZipArchive::open(&info.ora_path).unwrap();
        let names = zip.names();
        assert!(
            !names.iter().any(|n| n == "mergedimage.png"),
            "自动备份不应写 mergedimage.png：{names:?}"
        );
        assert!(
            !names.iter().any(|n| {
                n.starts_with("data/background-") && n.ends_with(".png") && !n.contains("tile")
            }),
            "自动备份不应写整幅 background.png：{names:?}"
        );

        // 快照是能真正载入的 ORA
        let mut restored = Document::new();
        restored.load(&info.ora_path).unwrap();
        assert_eq!(restored.layers().len(), doc.layers().len());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_meta_is_listed_as_incomplete() {
        let root = temp_cache("incomplete");
        let dir = root.join("autosave");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(AUTOSAVE_ORA_NAME), b"not a zip").unwrap();
        let info = AutosaveInfo::new_for_path(&dir).unwrap();
        assert!(!info.valid);
        assert!(info
            .description(SystemTime::now())
            .contains("Incomplete backup"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fresh_activity_marks_the_cache_in_use() {
        let root = temp_cache("activity");
        let doc_dir = root.join("doc.x");
        std::fs::create_dir_all(&doc_dir).unwrap();
        std::fs::write(doc_dir.join(CACHE_ACTIVITY_FILE), b"x").unwrap();
        assert!(cache_activity_fresh(&doc_dir));
        assert!(AutosaveInfo::new_for_path(&autosave_dir(&doc_dir)).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn doc_cache_dir_names_use_the_source_prefix() {
        let dir = create_doc_cache_dir().unwrap();
        assert!(dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(CACHE_DOC_SUBDIR_PREFIX));
        // 缓存根必须带 `pigment/mypaint` 命名空间，不能直接落到真实
        // MyPaint 的 `%LOCALAPPDATA%\mypaint`（否则对方恢复对话框会
        // 把本程序的备份列出来）。
        assert!(dir
            .parent()
            .unwrap()
            .ends_with(Path::new(CACHE_ROOT_SUBDIR_NAME).join(CACHE_APP_SUBDIR_NAME)));
        assert!(dir.join(CACHE_ACTIVITY_FILE).is_file());
        cleanup_doc_cache_dir(&dir);
        assert!(!dir.exists());
    }
}
