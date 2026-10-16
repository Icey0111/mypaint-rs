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

/// MyPaint 的笔刷快捷键数量（`gui/brushmanager.py:_NUM_BRUSHKEYS`）。
pub const NUM_BRUSHKEYS: usize = 10;

/// MyPaint 的默认启动分组（`gui/brushmanager.py:_DEFAULT_STARTUP_GROUP`）。
///
/// 源码自己都标注了 `# Suggestion only (FIXME: no effect?)`：现代官方笔刷包里
/// 并不存在名为 `set#2` 的分组，于是 `_init_default_brushkeys_and_history`
/// 会回退到**成员最多的分组**（并列时取名字靠前者）。
pub const DEFAULT_STARTUP_GROUP: &str = "set#2";

/// 用户收藏分组 —— `brushmanager.FAVORITES_BRUSH_GROUP`（`gui/brushmanager.py:56`）。
pub const FAVORITES_BRUSH_GROUP: &str = "favorites";
/// 孤儿笔刷的归宿（分组被删时）—— `DELETED_BRUSH_GROUP`（`:55`）。
pub const DELETED_BRUSH_GROUP: &str = "deleted";
/// 新建笔刷的归属 —— `NEW_BRUSH_GROUP`（`:57`）。
pub const NEW_BRUSH_GROUP: &str = "new";
/// 未列入 `order.conf` 的笔刷归宿 —— `FOUND_BRUSHES_GROUP`（`:54`）。
pub const FOUND_BRUSHES_GROUP: &str = "lost&found";

/// 分组名 → 展示名 —— 照搬 `brushmanager.translate_group_name()`
/// （`gui/brushmanager.py:129`）。
///
/// 表里的 key 全是**目录名风格**（小写、`set#N`、`lost&found`）：它们对应那些
/// 没有 `order.conf` 条目的分组（用户目录、以及 `_init_unordered_groups`
/// 兜底出来的 `Lost & Found`）。
///
/// 有 `order.conf` 条目的分组，其展示名直接取 `Group:` 行 —— 本项目就是
/// `BrushEntry::group_display` —— 再经这里兜底（官方包里 `Group: Deevad`
/// 与目录名 `deevad` 只差大小写，两张表结果一致）。
pub fn translate_group_name(name: &str) -> &str {
    match name {
        FOUND_BRUSHES_GROUP => "Lost & Found",
        DELETED_BRUSH_GROUP => "Deleted",
        FAVORITES_BRUSH_GROUP => "Favorites",
        "ink" => "Ink",
        "classic" => "Classic",
        "set#1" => "Set#1",
        "set#2" => "Set#2",
        "set#3" => "Set#3",
        "set#4" => "Set#4",
        "set#5" => "Set#5",
        "experimental" => "Experimental",
        NEW_BRUSH_GROUP => "New",
        other => other,
    }
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
    /// 编译期内嵌的 `{name}_prev.png` 原文（官方包随附的 128×128 预览图）。
    ///
    /// MyPaint 的 `ManagedBrush.get_preview()`（`gui/brushmanager.py:1242`）首选
    /// 读盘上这份图；内嵌模式没有盘可读，因此在构建期内联。`None` = 需回退实时渲染。
    pub embedded_preview: Option<&'static [u8]>,
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

    /// 笔刷的 128×128 预览图 PNG 原文。
    ///
    /// 对齐 MyPaint `ManagedBrush.get_preview()`（`gui/brushmanager.py:1242`）：
    /// **优先取随附的 `{name}_prev.png`**，只有取不到时才回退到
    /// [`crate`] 之外的实时渲染（`drawutils.render_brush_preview_pixbuf`）。
    ///
    /// - 内嵌笔刷：编译期内联的字节。
    /// - 磁盘笔刷：与 `.myb` 同目录同名的 `{stem}_prev.png`（MyPaint 的
    ///   `_get_fileprefix()`，`brushmanager.py:1397`）。
    pub fn preview_png(&self) -> Option<Cow<'_, [u8]>> {
        if let Some(bytes) = self.embedded_preview {
            return Some(Cow::Borrowed(bytes));
        }
        if let Origin::File(p) = &self.origin {
            let stem = p.file_stem()?.to_str()?;
            let sibling = p.with_file_name(format!("{stem}_prev.png"));
            if let Ok(bytes) = fs::read(&sibling) {
                return Some(Cow::Owned(bytes));
            }
        }
        None
    }

    /// MyPaint 的笔刷展示名 —— 对齐 `ManagedBrush.get_display_name()`
    /// （`gui/brushmanager.py:1349`）：**笔刷名**（而非 `.myb` 的 `description`），
    /// 下划线替换成空格。
    ///
    /// MyPaint 的列表是图标网格 + tooltip 显示此名；`description` 只作为
    /// tooltip 的第二行（`brushselectionwindow.py:46` 的 `managedbrush_namefunc`）。
    pub fn mypaint_name(&self) -> String {
        self.name.replace('_', " ")
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

/// 分组摘要（供 `BrushGroupTool` 面板与分组菜单使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    /// 分组 id —— 本项目取资产目录名（MyPaint 取 `order.conf` 的 `Group:` 名，
    /// 见 `docs`/比对报告里记录的差异）。
    pub id: String,
    /// 分组展示名（官方 `order.conf` 里的 `Group:` 名，如 `Deevad`；
    /// 用户目录分组则退化为目录名）。
    pub display: String,
    pub count: usize,
}

impl GroupInfo {
    /// 面板标题 / 菜单项文字 —— `BrushGroupTool.tool_widget_title`
    /// （`gui/brushselectionwindow.py:384`）与 `BrushGroupsMenu._update`（`:603`）。
    pub fn title(&self) -> &str {
        translate_group_name(&self.display)
    }
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
                embedded_preview: b.prev,
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
                embedded_preview: None,
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

    /// 启动分组：`DEFAULT_STARTUP_GROUP` 若存在则用它，否则取**成员最多**的分组
    /// （并列时分组名靠后者胜出）—— 对齐 `_init_default_brushkeys_and_history`。
    pub fn startup_group(&self) -> Option<String> {
        if self.entries.iter().any(|e| e.group == DEFAULT_STARTUP_GROUP) {
            return Some(DEFAULT_STARTUP_GROUP.to_string());
        }
        self.groups()
            .into_iter()
            .map(|g| (g.count, g.id))
            .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, id)| id)
    }

    /// 笔刷快捷键槽位的默认内容（返回 `entries` 下标）。
    ///
    /// 对齐 `gui/brushmanager.py:_init_default_brushkeys_and_history`：
    ///
    /// ```python
    /// idx = (i + 9) % 10  # keyboard order
    /// group_idx = idx % len(default_group)
    /// ```
    ///
    /// 即**槽位 i（数字键 i）取启动分组第 `(i+9)%10` 支**——这解释了为什么
    /// MyPaint 里按 `1` 拿到的不是分组第一支，而是第 10 支。
    pub fn brushkey_seed(&self) -> [Option<usize>; NUM_BRUSHKEYS] {
        let Some(group) = self.startup_group() else {
            return [None; NUM_BRUSHKEYS];
        };
        let members: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.group == group)
            .map(|(i, _)| i)
            .collect();
        if members.is_empty() {
            return [None; NUM_BRUSHKEYS];
        }
        std::array::from_fn(|i| Some(members[((i + 9) % NUM_BRUSHKEYS) % members.len()]))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mypaint_name_replaces_underscores() {
        // 对齐 ManagedBrush.get_display_name()：笔刷名 + 下划线换空格
        let lib = BrushLibrary::embedded();
        let i = lib
            .find("Dieterle/Pencil-_Left_Handed")
            .expect("内嵌官方包应含此笔刷");
        assert_eq!(lib.entries[i].name, "Pencil-_Left_Handed");
        assert_eq!(lib.entries[i].mypaint_name(), "Pencil- Left Handed");
    }

    #[test]
    fn startup_group_falls_back_to_largest_group() {
        // `set#2` 在现代官方包里不存在 → 回退成员最多的分组（实测 deevad 36 支）
        let lib = BrushLibrary::embedded();
        assert_eq!(lib.startup_group().as_deref(), Some("deevad"));
        let n = lib.entries.iter().filter(|e| e.group == "deevad").count();
        assert_eq!(n, 36, "deevad 应为最大分组，否则播种顺序的断言要跟着改");
    }

    #[test]
    fn brushkey_seed_uses_mypaint_keyboard_order() {
        let lib = BrushLibrary::embedded();
        let group: Vec<&str> = lib
            .entries
            .iter()
            .filter(|e| e.group == "deevad")
            .map(|e| e.key.as_str())
            .collect();
        let seed = lib.brushkey_seed();

        // 槽位 i ← 分组第 (i+9)%10 支
        for (i, s) in seed.iter().enumerate() {
            let idx = (i + 9) % NUM_BRUSHKEYS;
            let got = s.map(|e| lib.entries[e].key.as_str());
            assert_eq!(got, Some(group[idx]), "槽位 {i} 应为分组第 {idx} 支");
        }
        // 键盘序的直接体现：数字键 1 → 第 1 支；数字键 0 → 第 10 支
        assert_eq!(
            seed[1].map(|e| lib.entries[e].key.as_str()),
            Some(group[0])
        );
        assert_eq!(
            seed[0].map(|e| lib.entries[e].key.as_str()),
            Some(group[9])
        );
    }

    #[test]
    fn translate_group_name_matches_mypaint_table() {
        // 逐项对照 `gui/brushmanager.py:131-144` 的字面表
        for (raw, want) in [
            ("lost&found", "Lost & Found"),
            ("deleted", "Deleted"),
            ("favorites", "Favorites"),
            ("ink", "Ink"),
            ("classic", "Classic"),
            ("set#1", "Set#1"),
            ("set#2", "Set#2"),
            ("set#3", "Set#3"),
            ("set#4", "Set#4"),
            ("set#5", "Set#5"),
            ("experimental", "Experimental"),
            ("new", "New"),
        ] {
            assert_eq!(translate_group_name(raw), want, "{raw} 的展示名不对");
        }
        // 表外原样返回（官方包的 `Group: Dieterle` / `Group: Deevad` 都走这条路）
        assert_eq!(translate_group_name("Dieterle"), "Dieterle");
        assert_eq!(translate_group_name("Deevad"), "Deevad");
    }

    #[test]
    fn group_titles_use_order_conf_names() {
        // `GroupInfo.title()` = `translate_group_name(group_display)`：
        // 官方包里 `Deevad` 目录名是小写，但 order.conf 的 `Group: Deevad`
        // 才是 MyPaint 的面板标题（`brushselectionwindow.py:384`）。
        let lib = BrushLibrary::embedded();
        let groups = lib.groups();
        let deevad = groups.iter().find(|g| g.id == "deevad").unwrap();
        assert_eq!(deevad.title(), "Deevad");
        let kaerhon = groups.iter().find(|g| g.id == "kaerhon_v1").unwrap();
        assert_eq!(kaerhon.title(), "Kaerhon");
    }
}
