//! `.myb` 笔刷库 —— 内嵌官方包 + 用户目录扫描 + 按需解析。
//!
//! 设计要点：
//!
//! 1. **列表零成本**：官方包 196 支的元数据（分组/展示名/notes）在 `build.rs`
//!    里解析好，JSON 原文由 `include_str!` 内联。构造笔刷库不读盘、不解析 JSON。
//! 2. **按需解析**：只有用户真正选中某支笔刷时才调用 [`BrushEntry::load`]
//!    解析成 `hokusai::Brush`（20 KiB JSON ≈ 毫秒级），列表滚动零 GC 压力。
//! 3. **用户目录覆盖**：扫描用户笔刷目录时，与内嵌笔刷同名（key 相同）的以
//!    用户版本为准（与 MyPaint 的系统/用户目录语义一致）。
//!
//! 许可：内嵌的官方笔刷包来自 `mypaint-brushes`（CC0 / Public Domain），
//! 原件见 `assets/brushes/COPYING`。

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use hokusai::myb;
use hokusai::{Brush, BrushSetting};

/// 构建期生成的官方笔刷索引（`build.rs`）。
mod generated {
    include!(concat!(env!("OUT_DIR"), "/brush_index.rs"));
}

pub use generated::IndexedBrush;

/// 官方内嵌笔刷索引（顺序 = 官方 `order.conf`）。
pub fn indexed() -> &'static [IndexedBrush] {
    generated::INDEXED
}

/// 兜底笔刷：内嵌官方包里的 `classic/dry_brush`。
///
/// 任何笔刷加载失败时的最后防线。**不要用 `Brush::new()` 当兜底**——它所有
/// 设置都是 0，画出来什么都没有（libmypaint 的笔刷默认值不在 Rust 结构体里，
/// 而在 `.myb` 解析时的 `apply_libmypaint_defaults`）。
pub fn default_brush() -> Brush {
    let lib = BrushLibrary::embedded();
    if let Some(i) = lib.find("classic/dry_brush") {
        if let Ok(b) = lib.entries[i].load() {
            return b;
        }
    }
    fallback_brush()
}

/// 无资产可用时手搓一支能画画的笔刷（内嵌索引理论上不会失败，纯保险）。
fn fallback_brush() -> Brush {
    let mut b = Brush::new();
    b.get_mut(BrushSetting::Opaque).base_value = 1.0;
    b.get_mut(BrushSetting::OpaqueLinearize).base_value = 0.9;
    b.get_mut(BrushSetting::Radius).base_value = 2.0;
    b.get_mut(BrushSetting::Hardness).base_value = 0.5;
    b.get_mut(BrushSetting::AntiAliasing).base_value = 1.0;
    b.get_mut(BrushSetting::DabsPerActualRadius).base_value = 2.0;
    b.get_mut(BrushSetting::DabsPerBasicRadius).base_value = 2.0;
    b
}

/// 笔刷来源。
#[derive(Debug, Clone)]
pub enum Origin {
    /// 编译期内联的 .myb 原文。
    Embedded(&'static str),
    /// 磁盘上的 .myb 文件（用户笔刷目录）。
    File(PathBuf),
}

/// 笔刷库中的一条记录（元数据 + 来源，不含解析后的 `Brush`）。
#[derive(Debug, Clone)]
pub struct BrushEntry {
    /// 分组 id（= 资产目录名，如 `classic`）。
    pub group: String,
    /// 分组展示名（官方 `order.conf` 里的 `Group:` 名，如 `Classic`）。
    pub group_display: String,
    /// 笔刷名（= 文件名去扩展名）。
    pub name: String,
    /// 展示名（优先取 .myb 的 `description`）。
    pub display: String,
    /// .myb 的 `notes`（作者 / 授权 / 说明）。
    pub notes: String,
    /// 唯一 key，形如 `classic/dry_brush`。
    pub key: String,
    /// 来源。
    pub origin: Origin,
}

impl BrushEntry {
    /// 读取 .myb JSON 原文（内嵌则直接返回静态切片，零拷贝）。
    pub fn read_json(&self) -> Result<Cow<'_, str>, BrushLoadError> {
        match &self.origin {
            Origin::Embedded(s) => Ok(Cow::Borrowed(s)),
            Origin::File(p) => fs::read_to_string(p)
                .map(Cow::Owned)
                .map_err(BrushLoadError::Io),
        }
    }

    /// 解析为 `hokusai::Brush`（每次调用都会重新解析，调用方自行缓存）。
    pub fn load(&self) -> Result<Brush, BrushLoadError> {
        let json = self.read_json()?;
        myb::from_str(&json).map_err(|e| BrushLoadError::Parse(e.to_string()))
    }

    /// 是否来自内嵌官方包。
    pub fn is_embedded(&self) -> bool {
        matches!(self.origin, Origin::Embedded(_))
    }
}

/// 笔刷加载错误。
#[derive(Debug)]
pub enum BrushLoadError {
    Io(io::Error),
    Parse(String),
}

impl fmt::Display for BrushLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "读取失败: {e}"),
            Self::Parse(e) => write!(f, "解析失败: {e}"),
        }
    }
}

impl std::error::Error for BrushLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse(_) => None,
        }
    }
}

/// 分组摘要（供 UI 过滤条使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub id: String,
    pub display: String,
    pub count: usize,
}

/// 笔刷库：内嵌官方包 + 可选用户目录。
#[derive(Debug, Default)]
pub struct BrushLibrary {
    pub entries: Vec<BrushEntry>,
    /// 扫描期失败的文件（路径, 原因）——不阻塞其它笔刷加载。
    pub errors: Vec<(PathBuf, String)>,
}

impl BrushLibrary {
    /// 仅内嵌官方包（无 IO，构造立即返回）。
    pub fn embedded() -> Self {
        let entries = indexed()
            .iter()
            .map(|b| BrushEntry {
                group: b.group.to_string(),
                group_display: b.group_display.to_string(),
                name: b.name.to_string(),
                display: b.display.to_string(),
                notes: b.notes.to_string(),
                key: b.path.to_string(),
                origin: Origin::Embedded(b.json),
            })
            .collect();
        Self {
            entries,
            errors: Vec::new(),
        }
    }

    /// 内嵌官方包 + 扫描用户笔刷目录（同名以用户版本覆盖）。
    pub fn load() -> Self {
        let mut lib = Self::embedded();
        for dir in default_brush_dirs() {
            lib.scan_dir(&dir);
        }
        lib
    }

    /// 扫描一个用户笔刷目录（递归最多 3 层，group 取相对目录名）。
    pub fn scan_dir(&mut self, dir: &Path) {
        if !dir.is_dir() {
            return;
        }
        let existing: HashSet<String> = self.entries.iter().map(|e| e.key.clone()).collect();
        let mut found: Vec<PathBuf> = Vec::new();
        collect_myb(dir, 0, &mut found);
        found.sort();
        for path in found {
            let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                continue;
            };
            let group = path
                .parent()
                .and_then(|p| p.strip_prefix(dir).ok())
                .filter(|p| !p.as_os_str().is_empty())
                .and_then(|p| p.to_str())
                .map(|s| s.replace('\\', "/"))
                .unwrap_or_else(|| "user".to_string());
            let key = format!("{group}/{name}");
            if existing.contains(&key) {
                continue; // 用户目录优先，但已有同名则跳过（避免列表重复）
            }
            let (display, notes) = read_disk_meta(&path).unwrap_or_default();
            self.entries.push(BrushEntry {
                group_display: group.clone(),
                group,
                display: if display.trim().is_empty() {
                    name.clone()
                } else {
                    display.trim().to_string()
                },
                notes: notes.trim().to_string(),
                name,
                key,
                origin: Origin::File(path),
            });
        }
    }

    /// 按首次出现顺序返回分组摘要（内嵌部分即官方 `order.conf` 顺序）。
    pub fn groups(&self) -> Vec<GroupInfo> {
        let mut out: Vec<GroupInfo> = Vec::new();
        for e in &self.entries {
            match out.iter_mut().find(|g| g.id == e.group) {
                Some(g) => g.count += 1,
                None => out.push(GroupInfo {
                    id: e.group.clone(),
                    display: e.group_display.clone(),
                    count: 1,
                }),
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn find(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.key == key)
    }
}

/// 用户笔刷目录（跨平台，按 MyPaint 惯例）。
///
/// 另支持环境变量 `PIGMENT_BRUSH_PATH` 覆盖（`;` 或 `:` 分隔多个目录），
/// 便于开发期挂载任意笔刷包。
pub fn default_brush_dirs() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("PIGMENT_BRUSH_PATH") {
        let dirs: Vec<PathBuf> = std::env::split_paths(&v).collect();
        if !dirs.is_empty() {
            return dirs;
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(d) = std::env::var_os(var) {
                let d = PathBuf::from(d);
                out.push(d.join("mypaint").join("brushes"));
                out.push(d.join("pigment").join("brushes"));
            }
        }
    } else {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            });
        if let Some(d) = data {
            out.push(d.join("mypaint").join("brushes"));
            out.push(d.join("pigment").join("brushes"));
        }
    }
    // 去重（保序）
    let mut seen: HashSet<PathBuf> = HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

fn collect_myb(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_myb(&path, depth + 1, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("myb") {
            out.push(path);
        }
    }
}

/// 只解析展示需要的字段（不构建完整 `Brush`）。
fn read_disk_meta(path: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let get = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Some((get("description"), get("notes")))
}
