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
use hokusai::{Brush, BrushInput, BrushSetting, InputMapping, SettingValue};

/// 构建期生成的官方笔刷索引（`build.rs`）。
mod generated {
    include!(concat!(env!("OUT_DIR"), "/brush_index.rs"));
}

pub use generated::IndexedBrush;

/// 官方 `order.conf` 原文（构建期随包内嵌；用于恢复**空分组** Favorites 与组顺序）。
const OFFICIAL_ORDER_CONF: &str = include_str!("../assets/brushes/order.conf");

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
    // Match `mypaint_brush_from_defaults`: opaque_multiply starts at 0 with
    // a [(0,0),(1,1)] pressure curve, which is 1.0 at full pressure.
    b.get_mut(BrushSetting::OpaqueMultiply).base_value = 0.0;
    b.get_mut(BrushSetting::OpaqueMultiply).inputs = vec![InputMapping {
        input: BrushInput::Pressure,
        points: vec![(0.0, 0.0), (1.0, 1.0)],
    }];
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
    ///
    /// 补一条 MyPaint **应用层**的默认：`.myb` 没写 `paint_mode` 时，
    /// MyPaint 2.x 用 `pigment_on_by_default=True` → 1.0（光谱 WGM），
    /// 1.x 兼容模式用该设置的最小值 0.0。官方 196 支里 177 支省略
    /// 了该键（显式写出的 19 支全是 1.0），所以这是绝大多数笔刷的默认手感。
    /// 见 `gui/compatconfig.py:36-48` + `gui/brushmanager.py:609-643`。
    pub fn load(&self) -> Result<Brush, BrushLoadError> {
        self.load_with_default(true)
    }

    /// 同 [`BrushEntry::load`]，但可指定省略 `paint_mode` 时的默认 ——
    /// `set_pigment_by_default`（`brushmanager.py:609-643`）。
    pub fn load_with_default(&self, pigment_by_default: bool) -> Result<Brush, BrushLoadError> {
        let json = self.read_json()?;
        parse_brush_json_with_default(&json, pigment_by_default)
            .map_err(|e| BrushLoadError::Parse(e))
    }

    /// `.myb` 是否显式写了 `paint_mode`。
    ///
    /// `BrushManager._reset_pigment_setting`（`brushmanager.py:616-643`）
    /// 只重置 `undefined_settings` 里的设置；runtime 切换兼容模式时
    /// 必须能区分“省略”与“显式 0/1”。
    pub fn has_explicit_paint_mode(&self) -> bool {
        self.read_json()
            .map(|json| json_has_setting(&json, "paint_mode"))
            .unwrap_or(true)
    }

    /// 是否来自内嵌官方包。
    pub fn is_embedded(&self) -> bool {
        matches!(self.origin, Origin::Embedded(_))
    }

    /// 磁盘来源路径（内嵌笔刷为 `None`）。
    pub fn origin_path(&self) -> Option<&Path> {
        match &self.origin {
            Origin::File(p) => Some(p),
            Origin::Embedded(_) => None,
        }
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

    /// 预览图解码后的 RGBA8（供保存/Clone 原样沿用原图标）。
    ///
    /// 源码 `ManagedBrush.save()`（`brushmanager.py:1460-1476`）写的是
    /// `self.preview`（从 `_prev.png` 载入的原图），不是重新渲染的缩略图。
    pub fn preview_rgba(&self) -> Option<(u32, u32, Vec<u8>)> {
        let bytes = self.preview_png()?;
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .ok()?
            .to_rgba8();
        let (w, h) = img.dimensions();
        Some((w, h, img.into_raw()))
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

/// 一个笔刷分组（`BrushManager.groups` 的一项）。
///
/// MyPaint 的组是**有序的笔刷名列表**，同一支笔可以同时出现在多个组里
/// （Favorites 就是"额外挂一份"，不离开原组，`brushselectionwindow.py:267-294`）。
/// 这里用 key 列表表达多对多；`BrushEntry::group` 只保留"主分组"供旧代码使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrushGroup {
    /// 分组 id。官方分组 = 资产目录名（`deevad`）；用户组 / 特殊组 = 组名
    /// （`favorites`、`Favorites` 等）。
    pub id: String,
    /// 展示名（`order.conf` 的 `Group:` 行，或目录名）。
    pub display: String,
    /// 成员 key（`classic/pencil`），顺序 = `order.conf` 顺序。
    pub members: Vec<String>,
}

impl BrushGroup {
    pub fn title(&self) -> &str {
        translate_group_name(&self.display)
    }
}

/// 笔刷库：内嵌官方包 + 可选用户目录。
#[derive(Debug, Default)]
pub struct BrushLibrary {
    pub entries: Vec<BrushEntry>,
    /// 分组与成员顺序（`order.conf` 语义）。UI 的列表顺序取自这里。
    pub groups: Vec<BrushGroup>,
    /// 用户可写的 `order.conf`（第一个用户笔刷目录下；`None` = 不落盘）。
    pub order_conf_path: Option<PathBuf>,
    /// 扫描期失败的文件（路径, 原因）——不阻塞其它笔刷加载。
    pub errors: Vec<(PathBuf, String)>,
}

/// 解析 `order.conf` —— 逐行照 `brushmanager._parse_order_conf`（`:148-185`）：
/// 空行/`#` 注释跳过，`Group: ` 起新组，其余行是笔刷 key（重复项组内忽略）。
///
/// 返回 `[(组名, [key...])]`，保持文件顺序；空组也保留（Favorites）。
pub fn parse_order_conf(text: &str) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<usize> = None;
    for raw in text.replace('\r', "\n").split('\n') {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix("Group: ") {
            let name = name.to_string();
            if !groups.iter().any(|(n, _)| n == &name) {
                groups.push((name.clone(), Vec::new()));
            }
            current = groups.iter().position(|(n, _)| n == &name);
            continue;
        }
        let Some(i) = current else {
            // 文件头没有 Group: 的条目归 Lost & Found（源码 curr_group 初值）
            if !groups.iter().any(|(n, _)| n == FOUND_BRUSHES_GROUP) {
                groups.push((FOUND_BRUSHES_GROUP.to_string(), Vec::new()));
            }
            current = groups.iter().position(|(n, _)| n == FOUND_BRUSHES_GROUP);
            let i = current.expect("刚插入");
            if !groups[i].1.iter().any(|k| k.as_str() == line) {
                groups[i].1.push(line.to_string());
            }
            continue;
        };
        if !groups[i].1.iter().any(|k| k.as_str() == line) {
            groups[i].1.push(line.to_string());
        }
    }
    groups
}

/// 三方合并 `order_default.conf` / 用户 `order.conf` / 官方 `order.conf`。
///
/// 逐行照 `_init_ordered_groups`（`brushmanager.py:315-367`）：
///
/// 1. 上游新增的笔刷按官方顺序插到我们列表里（已存在则只更新插入点）；
/// 2. 基线里有、官方已删除的笔刷从我们的列表移除；
/// 3. 空组删除（`Favorites` 除外）。
///
/// 输入/输出都是 `(组展示名, keys)`，保持首次出现的组顺序。
pub fn merge_order_groups(
    base: &[(String, Vec<String>)],
    our: &[(String, Vec<String>)],
    their: &[(String, Vec<String>)],
) -> Vec<(String, Vec<String>)> {
    let mut order: Vec<String> = Vec::new();
    for list in [base, our, their] {
        for (name, _) in list {
            if !order.contains(name) {
                order.push(name.clone());
            }
        }
    }
    let members = |list: &[(String, Vec<String>)], name: &str| -> Vec<String> {
        list.iter()
            .find(|(n, _)| n == name)
            .map(|(_, keys)| keys.clone())
            .unwrap_or_default()
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for name in order {
        let base_b = members(base, &name);
        let their_b = members(their, &name);
        let mut our_b = members(our, &name);

        // 上游新增
        let mut insert_index = 0usize;
        for b in &their_b {
            if let Some(pos) = our_b.iter().position(|k| k == b) {
                insert_index = pos + 1;
            } else if !base_b.contains(b) {
                let pos = insert_index.min(our_b.len());
                our_b.insert(pos, b.clone());
                insert_index = pos + 1;
            }
        }
        // 上游删除（基线里有、官方没了）
        our_b.retain(|b| !(base_b.contains(b) && !their_b.contains(b)));
        // 空组删除，Favorites 保留
        if !our_b.is_empty() || name.eq_ignore_ascii_case(FAVORITES_BRUSH_GROUP) {
            out.push((name, our_b));
        }
    }
    out
}

/// 从 `order.conf` 形式的 `(组名, key 列表)` 建组：
/// 官方组沿用资产目录 id（`Deevad` → `deevad`），特殊/用户组用组名。
fn groups_from_order(entries: &[BrushEntry], order: &[(String, Vec<String>)]) -> Vec<BrushGroup> {
    let key_to_entry: std::collections::HashMap<&str, &BrushEntry> =
        entries.iter().map(|e| (e.key.as_str(), e)).collect();
    let mut groups: Vec<BrushGroup> = Vec::new();
    for (display, keys) in order {
        let id = keys
            .iter()
            .find_map(|k| key_to_entry.get(k.as_str()).map(|e| e.group.clone()))
            .unwrap_or_else(|| match display.as_str() {
                "Favorites" => FAVORITES_BRUSH_GROUP.to_string(),
                "Lost & Found" => FOUND_BRUSHES_GROUP.to_string(),
                "Deleted" => DELETED_BRUSH_GROUP.to_string(),
                "New" => NEW_BRUSH_GROUP.to_string(),
                other => other.to_string(),
            });
        let members: Vec<String> = keys
            .iter()
            .filter(|k| key_to_entry.contains_key(k.as_str()))
            .cloned()
            .collect();
        // 同一 id 出现两次（用户文件里两个组同名）时合并成员
        if let Some(g) = groups.iter_mut().find(|g| g.id == id) {
            for k in members {
                if !g.members.contains(&k) {
                    g.members.push(k);
                }
            }
            continue;
        }
        groups.push(BrushGroup {
            id,
            display: display.clone(),
            members,
        });
    }
    // 未列入任何组的笔刷 → Lost & Found（`_init_unordered_groups`）
    let mut orphaned: Vec<String> = entries
        .iter()
        .filter(|e| !groups.iter().any(|g| g.members.contains(&e.key)))
        .map(|e| e.key.clone())
        .collect();
    if !orphaned.is_empty() {
        if let Some(g) = groups.iter_mut().find(|g| g.id == FOUND_BRUSHES_GROUP) {
            g.members.append(&mut orphaned);
        } else {
            groups.push(BrushGroup {
                id: FOUND_BRUSHES_GROUP.to_string(),
                display: "Lost & Found".to_string(),
                members: orphaned,
            });
        }
    }
    groups
}

impl BrushLibrary {
    /// 仅内嵌官方包（无 IO，构造立即返回）。
    pub fn embedded() -> Self {
        let entries: Vec<BrushEntry> = indexed()
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
        let groups = groups_from_order(&entries, &parse_order_conf(OFFICIAL_ORDER_CONF));
        Self {
            entries,
            groups,
            order_conf_path: None,
            errors: Vec::new(),
        }
    }

    /// 内嵌官方包 + 扫描用户笔刷目录（同名以用户版本覆盖）。
    pub fn load() -> Self {
        let mut lib = Self::embedded();
        let dirs = default_brush_dirs();
        for dir in &dirs {
            lib.scan_dir(dir);
        }
        // 用户 order.conf（`<user_brush>/order.conf`）覆盖组顺序与成员；
        // 上游 `order.conf` 变化时按 `order_default.conf` 做三方合并
        // （`brushmanager.py:315-367`）。
        if let Some(dir) = dirs.first() {
            let path = dir.join("order.conf");
            let base_path = dir.join("order_default.conf");
            let our_text = fs::read_to_string(&path).ok();
            let base_text = fs::read_to_string(&base_path).unwrap_or_default();
            let base = parse_order_conf(&base_text);
            let our = our_text
                .as_deref()
                .map(parse_order_conf)
                .unwrap_or_default();
            let their = parse_order_conf(OFFICIAL_ORDER_CONF);
            lib.order_conf_path = Some(path.clone());
            if base == their {
                // 上游没变：用户文件就是最终顺序
                if let Some(text) = &our_text {
                    lib.apply_order_conf(text);
                }
            } else {
                let merged = merge_order_groups(&base, &our, &their);
                lib.apply_order_pairs(&merged);
                // 把合并结果写回用户文件，并把新基线留作下次比较
                let _ = lib.save_order_conf();
                let _ = fs::write(&base_path, OFFICIAL_ORDER_CONF);
            }
        }
        lib
    }

    /// 扫描一个用户笔刷目录（递归最多 3 层，group 取相对目录名）。
    pub fn scan_dir(&mut self, dir: &Path) {
        if !dir.is_dir() {
            return;
        }
        let mut found: Vec<PathBuf> = Vec::new();
        collect_myb(dir, 0, &mut found);
        found.sort();
        for path in found {
            let Some(name) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
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
            let (display, notes) = read_disk_meta(&path).unwrap_or_default();
            if let Some(existing_entry) = self.entries.iter_mut().find(|e| e.key == key) {
                // 用户目录的 `.myb` 覆盖内嵌官方版本（`brushmanager.py:1436-1441`
                // 先装用户、再补官方；同名只保留一份，用户那份胜出）。
                // 旧实现直接 `continue`，导致「改官方笔刷 → Save Brush → 重启」
                // 静默回到官方设置。
                existing_entry.origin = Origin::File(path);
                if !display.trim().is_empty() {
                    existing_entry.display = display.trim().to_string();
                }
                if !notes.trim().is_empty() {
                    existing_entry.notes = notes.trim().to_string();
                }
                continue;
            }
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
        // 新扫描到的笔刷挂进组：同 id 的组（目录名）追加，否则建新组。
        let mut pending: Vec<(String, String, String)> = Vec::new(); // (group_id, display, key)
        for e in &self.entries {
            if self.groups.iter().any(|g| g.members.contains(&e.key)) {
                continue;
            }
            pending.push((e.group.clone(), e.group_display.clone(), e.key.clone()));
        }
        for (id, display, key) in pending {
            if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
                g.members.push(key);
            } else {
                self.groups.push(BrushGroup {
                    id,
                    display,
                    members: vec![key],
                });
            }
        }
    }

    /// 查找分组下标：接受 id / display / 翻译后的标题。
    fn group_index(&self, name: &str) -> Option<usize> {
        self.groups
            .iter()
            .position(|g| g.id == name || g.display == name || g.title() == name)
    }

    /// 重命名分组（`brushmanager.rename_group`，`brushmanager.py:1143-1153`）。
    ///
    /// 源码是 `create_group(new) + move old + delete_group(old)`；这里在
    /// 原位置改 id/display，保持 UI 顺序稳定。重名/空名拒绝。
    pub fn rename_group(&mut self, old: &str, new: &str) -> Result<(), String> {
        let new = new.trim();
        if new.is_empty() {
            return Err("分组名不能为空".to_string());
        }
        let Some(idx) = self.group_index(old) else {
            return Err(format!("分组不存在: {old}"));
        };
        if self
            .groups
            .iter()
            .enumerate()
            .any(|(i, g)| i != idx && (g.id == new || g.display == new || g.title() == new))
        {
            return Err("A group with this name already exists!".to_string());
        }
        // 官方包目录组（id != display）只改展示名；用户组 id 跟着改。
        let g = &mut self.groups[idx];
        if g.id == g.display {
            g.id = new.to_string();
        }
        g.display = new.to_string();
        let _ = self.save_order_conf();
        Ok(())
    }

    /// 删除分组（`brushmanager.delete_group`，`brushmanager.py:1155-1171`）。
    ///
    /// 特殊组（Favorites / Deleted / Lost & Found）不可删；其余组的成员移到
    /// Deleted 组头部。
    pub fn delete_group(&mut self, name: &str) -> bool {
        let Some(idx) = self.group_index(name) else {
            return false;
        };
        if matches!(
            self.groups[idx].id.as_str(),
            FAVORITES_BRUSH_GROUP | DELETED_BRUSH_GROUP | FOUND_BRUSHES_GROUP
        ) {
            return false;
        }
        let group = self.groups.remove(idx);
        if !group.members.is_empty() {
            let dst = self
                .groups
                .iter()
                .position(|g| g.id == DELETED_BRUSH_GROUP)
                .unwrap_or_else(|| {
                    self.groups.push(BrushGroup {
                        id: DELETED_BRUSH_GROUP.to_string(),
                        display: DELETED_BRUSH_GROUP.to_string(),
                        members: Vec::new(),
                    });
                    self.groups.len() - 1
                });
            for key in group.members.into_iter().rev() {
                if !self.groups[dst].members.iter().any(|k| *k == key) {
                    self.groups[dst].members.insert(0, key);
                }
            }
        }
        let _ = self.save_order_conf();
        true
    }

    /// 按 `order.conf` 顺序返回分组摘要（含空组 Favorites）。
    pub fn groups(&self) -> Vec<GroupInfo> {
        self.groups
            .iter()
            .map(|g| GroupInfo {
                id: g.id.clone(),
                display: g.display.clone(),
                count: g.members.len(),
            })
            .collect()
    }

    /// 某组的成员在 `entries` 里的下标（顺序 = `order.conf`）。
    pub fn group_members(&self, group_id: &str) -> Vec<usize> {
        let Some(g) = self.groups.iter().find(|g| g.id == group_id) else {
            return Vec::new();
        };
        g.members.iter().filter_map(|k| self.find(k)).collect()
    }

    /// 用户 order.conf 覆盖组顺序/成员（`_load_ordered_groups` 的简化版：
    /// 用户文件为准；未列出的旧组保留在尾部）。
    pub fn apply_order_conf(&mut self, text: &str) {
        self.apply_order_pairs(&parse_order_conf(text));
    }

    /// 同 [`Self::apply_order_conf`]，但直接吃解析后的 `(组名, keys)`。
    pub fn apply_order_pairs(&mut self, parsed: &[(String, Vec<String>)]) {
        let mut new_groups: Vec<BrushGroup> = Vec::new();
        for (display, keys) in parsed {
            // 官方组按展示名匹配（Deevad ↔ deevad），用户组按 id/展示名匹配
            let old = self
                .groups
                .iter()
                .find(|g| g.display == *display || g.id == *display || g.title() == display);
            let id = old
                .map(|g| g.id.clone())
                .unwrap_or_else(|| match display.as_str() {
                    "Favorites" => FAVORITES_BRUSH_GROUP.to_string(),
                    "Lost & Found" => FOUND_BRUSHES_GROUP.to_string(),
                    "Deleted" => DELETED_BRUSH_GROUP.to_string(),
                    "New" => NEW_BRUSH_GROUP.to_string(),
                    other => other.to_string(),
                });
            let members: Vec<String> = keys
                .iter()
                .filter(|k| self.find(k).is_some())
                .cloned()
                .collect();
            if let Some(g) = new_groups.iter_mut().find(|g| g.id == id) {
                for k in members {
                    if !g.members.contains(&k) {
                        g.members.push(k);
                    }
                }
            } else {
                new_groups.push(BrushGroup {
                    id,
                    display: display.clone(),
                    members,
                });
            }
        }
        // 用户文件没提到的旧组保留（例如用户只写了 Favorites 一个组）
        for g in &self.groups {
            if !new_groups.iter().any(|n| n.id == g.id) {
                new_groups.push(g.clone());
            }
        }
        // 仍未入组的笔刷 → Lost & Found
        let orphaned: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !new_groups.iter().any(|g| g.members.contains(&e.key)))
            .map(|e| e.key.clone())
            .collect();
        if !orphaned.is_empty() {
            if let Some(g) = new_groups.iter_mut().find(|g| g.id == FOUND_BRUSHES_GROUP) {
                g.members.extend(orphaned);
            } else {
                new_groups.push(BrushGroup {
                    id: FOUND_BRUSHES_GROUP.to_string(),
                    display: "Lost & Found".to_string(),
                    members: orphaned,
                });
            }
        }
        if !new_groups.iter().any(|g| g.id == FAVORITES_BRUSH_GROUP) {
            // 官方包/源码都保留空 Favorites 组
            new_groups.push(BrushGroup {
                id: FAVORITES_BRUSH_GROUP.to_string(),
                display: "Favorites".to_string(),
                members: Vec::new(),
            });
        }
        self.groups = new_groups;
    }

    /// 写回 `order.conf` —— 逐字照 `save_brushorder`（`brushmanager.py:922-935`）：
    /// 头注释 + 每组建 `Group: 展示名`，随后按顺序写成员 key。
    ///
    /// 返回写入路径；`order_conf_path` 为 `None`（纯内嵌库/测试）时写当前目录
    /// 的 `order.conf`，由调用方决定是否保留。
    pub fn save_order_conf(&self) -> io::Result<PathBuf> {
        let path = self
            .order_conf_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("order.conf"));
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)?;
            }
        }
        let mut out = String::from("# this file saves brush groups and order\n");
        for g in &self.groups {
            out.push_str(&format!("Group: {}\n", g.display));
            for key in &g.members {
                out.push_str(key);
                out.push('\n');
            }
        }
        fs::write(&path, out)?;
        Ok(path)
    }

    /// 把某支笔刷加入/移出 Favorites（`BrushPopupMenu._favorite_cb`，`:267-294`）。
    /// 不离开原组。返回写入后是否在 Favorites 里。
    pub fn toggle_favorite(&mut self, key: &str) -> bool {
        let faves = self
            .groups
            .iter_mut()
            .find(|g| g.id == FAVORITES_BRUSH_GROUP);
        let Some(faves) = faves else {
            self.groups.push(BrushGroup {
                id: FAVORITES_BRUSH_GROUP.to_string(),
                display: "Favorites".to_string(),
                members: vec![key.to_string()],
            });
            return true;
        };
        if let Some(pos) = faves.members.iter().position(|k| k == key) {
            faves.members.remove(pos);
            false
        } else {
            faves.members.push(key.to_string());
            true
        }
    }

    pub fn is_favorite(&self, key: &str) -> bool {
        self.groups
            .iter()
            .find(|g| g.id == FAVORITES_BRUSH_GROUP)
            .is_some_and(|g| g.members.iter().any(|k| k == key))
    }

    /// 从某组移除（不删笔刷）—— `BrushList.remove_brush`（`:143-145`）。
    pub fn remove_from_group(&mut self, group_id: &str, key: &str) -> bool {
        let Some(g) = self.groups.iter_mut().find(|g| g.id == group_id) else {
            return false;
        };
        let before = g.members.len();
        g.members.retain(|k| k != key);
        g.members.len() != before
    }

    /// 新建空组（`create_group`，`:1131-1141`）。返回组 id；同名已存在则直接返回。
    pub fn create_group(&mut self, name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if let Some(g) = self
            .groups
            .iter()
            .find(|g| g.display == name || g.id == name)
        {
            return Some(g.id.clone());
        }
        let id = match name {
            "Favorites" => FAVORITES_BRUSH_GROUP.to_string(),
            "New" => NEW_BRUSH_GROUP.to_string(),
            "Deleted" => DELETED_BRUSH_GROUP.to_string(),
            _ => name.to_string(),
        };
        self.groups.push(BrushGroup {
            id: id.clone(),
            display: name.to_string(),
            members: Vec::new(),
        });
        Some(id)
    }

    /// 组内重排（`PixbufList` 的 DnD 落点语义）：把 `key` 移到 `target` 下标。
    pub fn move_in_group(&mut self, group_id: &str, key: &str, target: usize) -> bool {
        let Some(g) = self.groups.iter_mut().find(|g| g.id == group_id) else {
            return false;
        };
        let Some(from) = g.members.iter().position(|k| k == key) else {
            return false;
        };
        let item = g.members.remove(from);
        let target = target.min(g.members.len());
        g.members.insert(target, item);
        true
    }

    // ---- 落盘：`.myb` + `_prev.png`（`ManagedBrush.save`，brushmanager.py:1460-1484）----

    /// 用户可写笔刷目录（`user_brushpath`）：优先取 order.conf 所在目录。
    pub fn user_brush_dir(&self) -> PathBuf {
        self.order_conf_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(PathBuf::from)
            .or_else(|| default_brush_dirs().into_iter().next())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 写 `.myb` + `_prev.png`，返回 `.myb` 路径。预览按源码合成到白底（去 alpha）。
    pub fn save_brush_files(
        &self,
        key: &str,
        brush: &Brush,
        preview: Option<(u32, u32, &[u8])>,
    ) -> io::Result<PathBuf> {
        let meta = self.find(key).map(|i| &self.entries[i]);
        self.save_brush_files_with_meta(key, brush, preview, meta)
    }

    /// 同 [`Self::save_brush_files`]，但可显式指定元数据来源（Clone 用源笔刷的
    /// description/notes）。
    fn save_brush_files_with_meta(
        &self,
        key: &str,
        brush: &Brush,
        preview: Option<(u32, u32, &[u8])>,
        meta: Option<&BrushEntry>,
    ) -> io::Result<PathBuf> {
        let dir = self.user_brush_dir();
        let path = dir.join(format!("{key}.myb"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // `raw_meta()` 读的是**磁盘上的旧 JSON**；编辑器改过 description/notes
        // 后必须以库条目为准（源码 `ManagedBrush.save()` 写的是内存里的
        // `brushinfo` 字符串属性，不是旧文件）。
        let (description, notes) = meta
            .map(|e| (e.display.clone(), e.notes.clone()))
            .unwrap_or_default();
        let json = serialize_brush(brush, &description, &notes)
            .map_err(|e| io::Error::other(format!("序列化 .myb 失败: {e}")))?;
        fs::write(&path, json)?;
        if let Some((w, h, rgba)) = preview {
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for px in rgba.chunks_exact(4) {
                let a = px[3] as u32;
                for c in 0..3 {
                    // 直通 alpha 合成到白底：`c*a + 255*(1-a)`
                    let v = (px[c] as u32 * a + 255 * (255 - a)) / 255;
                    rgb.push(v as u8);
                }
            }
            let png = path.with_file_name(format!(
                "{}_prev.png",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("brush")
            ));
            image::save_buffer(&png, &rgb, w, h, image::ColorType::Rgb8)
                .map_err(|e| io::Error::other(format!("写 _prev.png 失败: {e}")))?;
        }
        Ok(path)
    }

    /// 重命名笔刷（`gui/brusheditor.py:574-636` 的 `rename_button_clicked_cb`）。
    ///
    /// 把当前编辑设置写到用户目录的新 key（保留原 `_prev.png`），并把所有
    /// 分组里的成员 key 换掉；原 key 从库中移除（官方笔刷会在下次 rescan 回来）。
    pub fn rename_brush(
        &mut self,
        key: &str,
        new_name: &str,
        brush: &Brush,
        preview: Option<(u32, u32, &[u8])>,
    ) -> Result<String, String> {
        let new_name = new_name.trim().replace(' ', "_");
        if new_name.is_empty() {
            return Err("No brush selected!".to_string());
        }
        let Some(idx) = self.find(key) else {
            return Err(format!("笔刷不存在: {key}"));
        };
        if self
            .entries
            .iter()
            .any(|e| e.key != key && e.name == new_name)
        {
            return Err("A brush with this name already exists!".to_string());
        }
        let new_key = match key.rsplit_once('/') {
            Some((group, _)) => format!("{group}/{new_name}"),
            None => new_name.clone(),
        };
        if self.entries.iter().any(|e| e.key == new_key) {
            return Err("A brush with this name already exists!".to_string());
        }
        let path = self
            .save_brush_files_with_meta(&new_key, brush, preview, Some(&self.entries[idx]))
            .map_err(|e| format!("保存笔刷失败: {e}"))?;
        let mut entry = self.entries[idx].clone();
        entry.key = new_key.clone();
        entry.name = new_name.clone();
        entry.display = new_name.replace('_', " ");
        entry.origin = Origin::File(path);
        entry.embedded_preview = None;
        for group in &mut self.groups {
            for member in &mut group.members {
                if member == key {
                    *member = new_key.clone();
                }
            }
        }
        self.entries[idx] = entry;
        let _ = self.save_order_conf();
        Ok(new_key)
    }

    /// 唯一笔刷 key：若已存在则追加 ` 1`、` 2`……
    fn unique_key(&self, base: &str) -> String {
        if self.find(base).is_none() {
            return base.to_string();
        }
        let mut n = 1u32;
        loop {
            let candidate = format!("{base} {n}");
            if self.find(&candidate).is_none() {
                return candidate;
            }
            n += 1;
        }
    }

    /// 克隆一支笔刷到当前组中（`ManagedBrush.clone` + `BrushList.insert_brush`）。
    ///
    /// 返回新 `entries` 下标。新 key = `"{src_key} copy"`（重名再追加序号），
    /// 落在源笔刷所在组、紧跟源笔刷之后；父笔刷名写回 `.myb` 的
    /// `parent_brush_name`（`clone_into`，brushmanager.py:1372-1382）。
    pub fn clone_brush(
        &mut self,
        src_key: &str,
        brush: &mut Brush,
        preview: Option<(u32, u32, &[u8])>,
    ) -> io::Result<usize> {
        let Some(src_idx) = self.find(src_key) else {
            return Err(io::Error::other(format!("源笔刷不存在: {src_key}")));
        };
        let src = self.entries[src_idx].clone();
        let new_key = self.unique_key(&format!("{src_key} copy"));
        brush.parent_brush_name = Some(src_key.to_string());
        let path = self.save_brush_files_with_meta(&new_key, brush, preview, Some(&src))?;
        let name = new_key.rsplit('/').next().unwrap_or(&new_key).to_string();
        let idx = self.entries.len();
        self.entries.push(BrushEntry {
            group: src.group.clone(),
            group_display: src.group_display.clone(),
            display: name.clone(),
            notes: String::new(),
            name,
            key: new_key.clone(),
            origin: Origin::File(path),
            embedded_preview: None,
        });
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == src.group) {
            let pos = g
                .members
                .iter()
                .position(|k| k == src_key)
                .map(|p| p + 1)
                .unwrap_or(g.members.len());
            g.members.insert(pos, new_key);
        } else {
            self.groups.push(BrushGroup {
                id: src.group.clone(),
                display: src.group_display.clone(),
                members: vec![new_key],
            });
        }
        Ok(idx)
    }

    /// 保存**已有**笔刷（原地写回用户目录）：更新 entry 的来源与预览缓存。
    pub fn save_brush_entry(
        &mut self,
        key: &str,
        brush: &Brush,
        preview: Option<(u32, u32, &[u8])>,
    ) -> io::Result<usize> {
        let Some(idx) = self.find(key) else {
            return Err(io::Error::other(format!("笔刷不存在: {key}")));
        };
        let path = self.save_brush_files(key, brush, preview)?;
        let e = &mut self.entries[idx];
        e.origin = Origin::File(path);
        e.embedded_preview = None;
        Ok(idx)
    }

    /// 只写 `_prev.png`（BrushIconEditor 的 Save）：仍按 MyPaint 的
    /// `ManagedBrush.save()` 语义把当前 settings 一起写回用户目录。
    pub fn save_brush_preview(
        &mut self,
        key: &str,
        preview: (u32, u32, &[u8]),
    ) -> io::Result<usize> {
        let Some(idx) = self.find(key) else {
            return Err(io::Error::other(format!("笔刷不存在: {key}")));
        };
        let brush = self.entries[idx]
            .load()
            .map_err(|e| io::Error::other(format!("重新读取笔刷失败: {e}")))?;
        self.save_brush_entry(key, &brush, Some(preview))
    }

    /// 删除笔刷（`ManagedBrush.delete_from_disk` + BrushEditor `_delete_brush`）：
    ///
    /// - 用户目录笔刷：删 `.myb`/`_prev.png`，从所有组移除，并从库中删除；
    /// - 官方内嵌笔刷（删不掉）：从所有组移除后放进 `Deleted` 组**最前**
    ///   （`brusheditor.py:1104-1115`）。
    ///
    /// 返回是否从库中彻底删掉（官方笔刷为 `false`）。
    pub fn delete_brush(&mut self, key: &str) -> io::Result<bool> {
        let Some(idx) = self.find(key) else {
            return Err(io::Error::other(format!("笔刷不存在: {key}")));
        };
        let entry = self.entries[idx].clone();
        for g in &mut self.groups {
            g.members.retain(|k| k != key);
        }
        if let Origin::File(path) = &entry.origin {
            let _ = fs::remove_file(path);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("brush");
            let _ = fs::remove_file(path.with_file_name(format!("{stem}_prev.png")));
            self.entries.remove(idx);
            return Ok(true);
        }
        // 官方笔刷删不掉 → Deleted 组最前
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == DELETED_BRUSH_GROUP) {
            g.members.insert(0, key.to_string());
        } else {
            self.groups.push(BrushGroup {
                id: DELETED_BRUSH_GROUP.to_string(),
                display: "Deleted".to_string(),
                members: vec![key.to_string()],
            });
        }
        Ok(false)
    }

    /// 用当前笔刷设置新建一支笔刷（BrushEditor 的 clone 按钮，
    /// `brusheditor.py:664-690`）：写到用户目录、放进 `New` 组**最前**。
    ///
    /// 源码 `ManagedBrush.clone(name=None)` 走 `_get_fileprefix` 的
    /// `"b%03d"` 命名；这里同样从 `b000` 开始找空位。
    pub fn new_brush(
        &mut self,
        brush: &mut Brush,
        preview: Option<(u32, u32, &[u8])>,
    ) -> io::Result<usize> {
        let mut n = 0u32;
        let key = loop {
            let candidate = format!("b{n:03}");
            if self.find(&candidate).is_none() {
                break candidate;
            }
            n += 1;
        };
        // 源码在 clone 按钮里把 parent_brush_name 清空
        brush.parent_brush_name = None;
        let path = self.save_brush_files(&key, brush, preview)?;
        let idx = self.entries.len();
        self.entries.push(BrushEntry {
            group: NEW_BRUSH_GROUP.to_string(),
            group_display: "New".to_string(),
            name: key.clone(),
            display: key.clone(),
            notes: String::new(),
            key: key.clone(),
            origin: Origin::File(path),
            embedded_preview: None,
        });
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == NEW_BRUSH_GROUP) {
            g.members.insert(0, key);
        } else {
            self.groups.push(BrushGroup {
                id: NEW_BRUSH_GROUP.to_string(),
                display: "New".to_string(),
                members: vec![key],
            });
        }
        Ok(idx)
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
        if self.groups.iter().any(|g| g.id == DEFAULT_STARTUP_GROUP) {
            return Some(DEFAULT_STARTUP_GROUP.to_string());
        }
        self.groups
            .iter()
            .map(|g| (g.members.len(), g.id.clone()))
            .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, id)| id)
    }

    /// 保存一个笔刷快捷键槽位 —— `context.save()` 写
    /// `<user_brush>/contextNN.myb`（`gui/document.py:1710-1713`）。
    pub fn save_context(&self, slot: usize, brush: &Brush) -> io::Result<PathBuf> {
        let dir = self.user_brush_dir();
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("context{slot:02}.myb"));
        let json = serialize_brush(brush, "", "")
            .map_err(|e| io::Error::other(format!("序列化 context 笔刷失败: {e}")))?;
        fs::write(&path, json)?;
        Ok(path)
    }

    /// 启动时读回 `contextNN.myb`（`brushmanager.py:414-416`）。
    ///
    /// 文件名是 `context00` … `context09`；缺失的槽位为 `None`。
    pub fn load_contexts(&self) -> [Option<Brush>; NUM_BRUSHKEYS] {
        let dir = self.user_brush_dir();
        std::array::from_fn(|i| {
            let path = dir.join(format!("context{i:02}.myb"));
            let text = fs::read_to_string(path).ok()?;
            parse_brush_json(&text).ok()
        })
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
        let members = self.group_members(&group);
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

/// `.myb` JSON 的 `settings` 对象里是否显式出现某个键。
fn json_has_setting(json: &str, name: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("settings")?.get(name).map(|_| true))
        .unwrap_or(false)
}

/// 设置的界面显示名 —— `lib/brushsettings.py` 的 `displayed_name`。
///
/// 数据源是 `_libmypaint_ref/brushsettings.json`；编辑器显示 "Opacity" 而不是
/// 内部名 `opaque`，内部名仍用于 `.myb` 键与调试。
pub fn setting_display_name(cname: &str) -> &str {
    match cname {
        "opaque" => "Opacity",
        "opaque_multiply" => "Opacity multiply",
        "opaque_linearize" => "Opacity linearize",
        "radius_logarithmic" => "Radius",
        "hardness" => "Hardness",
        "softness" => "Softness",
        "anti_aliasing" => "Pixel feather",
        "dabs_per_basic_radius" => "Dabs per basic radius",
        "dabs_per_actual_radius" => "Dabs per actual radius",
        "dabs_per_second" => "Dabs per second",
        "gridmap_scale" => "GridMap Scale",
        "gridmap_scale_x" => "GridMap Scale X",
        "gridmap_scale_y" => "GridMap Scale Y",
        "radius_by_random" => "Radius by random",
        "speed1_slowness" => "Fine speed filter",
        "speed2_slowness" => "Gross speed filter",
        "speed1_gamma" => "Fine speed gamma",
        "speed2_gamma" => "Gross speed gamma",
        "offset_by_random" => "Jitter",
        "offset_y" => "Offset Y",
        "offset_x" => "Offset X",
        "offset_angle" => "Angular Offset: Direction",
        "offset_angle_asc" => "Angular Offset: Ascension",
        "offset_angle_view" => "Angular Offset: View",
        "offset_angle_2" => "Angular Offset Mirrored: Direction",
        "offset_angle_2_asc" => "Angular Offset Mirrored: Ascension",
        "offset_angle_2_view" => "Angular Offset Mirrored: View",
        "offset_angle_adj" => "Angular Offsets Adjustment",
        "offset_multiplier" => "Offsets Multiplier",
        "offset_by_speed" => "Offset by speed",
        "offset_by_speed_slowness" => "Offset by speed filter",
        "slow_tracking" => "Slow position tracking",
        "slow_tracking_per_dab" => "Slow tracking per dab",
        "tracking_noise" => "Tracking noise",
        "color_h" => "Color hue",
        "color_s" => "Color saturation",
        "color_v" => "Color value",
        "restore_color" => "Save color",
        "change_color_h" => "Change color hue",
        "change_color_l" => "Change color lightness (HSL)",
        "change_color_hsl_s" => "Change color satur. (HSL)",
        "change_color_v" => "Change color value (HSV)",
        "change_color_hsv_s" => "Change color satur. (HSV)",
        "smudge" => "Smudge",
        "paint_mode" => "Pigment",
        "smudge_transparency" => "Smudge transparency",
        "smudge_length" => "Smudge length",
        "smudge_length_log" => "Smudge length multiplier",
        "smudge_bucket" => "Smudge bucket",
        "smudge_radius_log" => "Smudge radius",
        "eraser" => "Eraser",
        "stroke_threshold" => "Stroke threshold",
        "stroke_duration_logarithmic" => "Stroke duration",
        "stroke_holdtime" => "Stroke hold time",
        "custom_input" => "Custom input",
        "custom_input_slowness" => "Custom input filter",
        "elliptical_dab_ratio" => "Elliptical dab: ratio",
        "elliptical_dab_angle" => "Elliptical dab: angle",
        "direction_filter" => "Direction filter",
        "lock_alpha" => "Lock alpha",
        "colorize" => "Colorize",
        "posterize" => "Posterize",
        "posterize_num" => "Posterization levels",
        "snap_to_pixel" => "Snap to pixel",
        "pressure_gain_log" => "Pressure gain",
        other => other,
    }
}

/// 设置的出厂默认值 —— `brushsettings.json` 的 `default`。
///
/// 编辑器"重置该设置"要回到这个值（`brusheditor.py` 的
/// `base_value_reset_button_clicked_cb` 是 `adj.set_value(s.default)`），不是清成 0。
pub fn setting_default(cname: &str) -> Option<f64> {
    match cname {
        "opaque" => Some(1.0),
        "opaque_multiply" => Some(0.0),
        "opaque_linearize" => Some(0.9),
        "radius_logarithmic" => Some(2.0),
        "hardness" => Some(0.8),
        "anti_aliasing" => Some(1.0),
        "dabs_per_basic_radius" => Some(0.0),
        "dabs_per_actual_radius" => Some(2.0),
        "dabs_per_second" => Some(0.0),
        "gridmap_scale" => Some(0.0),
        "gridmap_scale_x" => Some(1.0),
        "gridmap_scale_y" => Some(1.0),
        "radius_by_random" => Some(0.0),
        "speed1_slowness" => Some(0.04),
        "speed2_slowness" => Some(0.8),
        "speed1_gamma" => Some(4.0),
        "speed2_gamma" => Some(4.0),
        "offset_by_random" => Some(0.0),
        "offset_y" => Some(0.0),
        "offset_x" => Some(0.0),
        "offset_angle" => Some(0.0),
        "offset_angle_asc" => Some(0.0),
        "offset_angle_view" => Some(0.0),
        "offset_angle_2" => Some(0.0),
        "offset_angle_2_asc" => Some(0.0),
        "offset_angle_2_view" => Some(0.0),
        "offset_angle_adj" => Some(0.0),
        "offset_multiplier" => Some(0.0),
        "offset_by_speed" => Some(0.0),
        "offset_by_speed_slowness" => Some(1.0),
        "slow_tracking" => Some(0.0),
        "slow_tracking_per_dab" => Some(0.0),
        "tracking_noise" => Some(0.0),
        "color_h" => Some(0.0),
        "color_s" => Some(0.0),
        "color_v" => Some(0.0),
        "restore_color" => Some(0.0),
        "change_color_h" => Some(0.0),
        "change_color_l" => Some(0.0),
        "change_color_hsl_s" => Some(0.0),
        "change_color_v" => Some(0.0),
        "change_color_hsv_s" => Some(0.0),
        "smudge" => Some(0.0),
        "paint_mode" => Some(1.0),
        "smudge_transparency" => Some(0.0),
        "smudge_length" => Some(0.5),
        "smudge_length_log" => Some(0.0),
        "smudge_bucket" => Some(0.0),
        "smudge_radius_log" => Some(0.0),
        "eraser" => Some(0.0),
        "stroke_threshold" => Some(0.0),
        "stroke_duration_logarithmic" => Some(4.0),
        "stroke_holdtime" => Some(0.0),
        "custom_input" => Some(0.0),
        "custom_input_slowness" => Some(0.0),
        "elliptical_dab_ratio" => Some(1.0),
        "elliptical_dab_angle" => Some(90.0),
        "direction_filter" => Some(2.0),
        "lock_alpha" => Some(0.0),
        "colorize" => Some(0.0),
        "posterize" => Some(0.0),
        "posterize_num" => Some(0.05),
        "snap_to_pixel" => Some(0.0),
        "pressure_gain_log" => Some(0.0),
        _ => return None,
    }
}

/// 输入轴的显示名 —— `brushsettings.json` 的 `displayed_name`。
pub fn input_display_name(cname: &str) -> &str {
    match cname {
        "pressure" => "Pressure",
        "random" => "Random",
        "stroke" => "Stroke",
        "direction" => "Direction",
        "tilt_declination" => "Declination/Tilt",
        "tilt_ascension" => "Ascension",
        "speed1" => "Fine speed",
        "speed2" => "Gross speed",
        "custom" => "Custom",
        "direction_angle" => "Direction 360",
        "attack_angle" => "Attack Angle",
        "tilt_declinationx" => "Declination/Tilt X",
        "tilt_declinationy" => "Declination/Tilt Y",
        "gridmap_x" => "GridMap X",
        "gridmap_y" => "GridMap Y",
        "viewzoom" => "Zoom Level",
        "brush_radius" => "Base Brush Radius",
        "barrel_rotation" => "Barrel Rotation",
        other => other,
    }
}
/// 解析 `.myb` JSON 并恢复**显式写出**的设置（不把显式 0 当未定义）。
pub fn parse_brush_json(json: &str) -> Result<Brush, String> {
    parse_brush_json_with_default(json, true)
}

/// 同 [`parse_brush_json`]，但可指定"省略 `paint_mode` 时是否开颜料" ——
/// `BrushManager.set_pigment_by_default`（`brushmanager.py:609-643`）：
/// 1.x 兼容模式传 `false`，2.x 传 `true`。
pub fn parse_brush_json_with_default(
    json: &str,
    pigment_by_default: bool,
) -> Result<Brush, String> {
    let mut brush = myb::from_str(json).map_err(|e| e.to_string())?;
    apply_raw_settings(json, &mut brush);
    if !json_has_setting(json, "paint_mode") {
        brush.set(
            BrushSetting::Paint,
            SettingValue::constant(if pigment_by_default { 1.0 } else { 0.0 }),
        );
    }
    Ok(brush)
}

/// 把原始 JSON 里**显式出现**的设置覆盖回 `Brush`（保住显式 0 与输入曲线）。
fn apply_raw_settings(json: &str, brush: &mut Brush) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let Some(settings) = v.get("settings").and_then(|s| s.as_object()) else {
        return;
    };
    for (name, obj) in settings {
        let Some(setting) = BrushSetting::from_cname(name) else {
            continue;
        };
        let base_value = obj
            .get("base_value")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0) as f32;
        let mut inputs: Vec<InputMapping> = Vec::new();
        let mut unknown_inputs: std::collections::BTreeMap<String, Vec<(f32, f32)>> =
            std::collections::BTreeMap::new();
        if let Some(map) = obj.get("inputs").and_then(|x| x.as_object()) {
            for (iname, pts) in map {
                let points: Vec<(f32, f32)> = pts
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| {
                                let a = p.as_array()?;
                                Some((a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(input) = BrushInput::from_cname(iname) {
                    inputs.push(InputMapping { input, points });
                } else {
                    unknown_inputs.insert(iname.clone(), points);
                }
            }
        }
        brush.set(
            setting,
            SettingValue {
                base_value,
                inputs,
                unknown_inputs,
            },
        );
    }
}

/// 把 `Brush` 序列化成 MyPaint v3 `.myb`（`BrushInfo.to_json`，`lib/brush.py:251-277`）。
///
/// 与 hokusai 0.3 的 `to_string_pretty` 的三处差异（都是源语义）：
/// 1. **写全部设置**，包括 `base_value == 0.0` 的显式零；hokusai 会把它们略掉，
///    重读时又被 libmypaint 默认值回填（`anti_aliasing` 0→1 等）；
/// 2. 写 `description` / `notes`（hokusai 的 Raw 没有这两个字段，保存即丢）；
/// 3. 逐项写 `inputs`（升序输入名 + `[[x,y],…]`）。
///
/// 未知设置（`smudge_bucket` 等）按 hokusai 解析出的原文补回，保证不丢键。
fn serialize_brush(brush: &Brush, description: &str, notes: &str) -> Result<String, String> {
    use serde_json::{json, Map, Value};

    let mut settings: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for (i, sv) in brush.settings().iter().enumerate() {
        let name = BrushSetting::ALL[i].cname();
        let mut inputs: Map<String, Value> = Map::new();
        for m in &sv.inputs {
            let pts: Vec<Value> = m.points.iter().map(|(x, y)| json!([x, y])).collect();
            inputs.insert(m.input.cname().to_string(), Value::Array(pts));
        }
        for (iname, pts) in &sv.unknown_inputs {
            let pts: Vec<Value> = pts.iter().map(|(x, y)| json!([x, y])).collect();
            inputs.insert(iname.clone(), Value::Array(pts));
        }
        settings.insert(
            name.to_string(),
            json!({ "base_value": sv.base_value, "inputs": inputs }),
        );
    }
    for (name, u) in &brush.unknown_settings {
        let mut inputs: Map<String, Value> = Map::new();
        for (iname, pts) in &u.inputs {
            let pts: Vec<Value> = pts.iter().map(|(x, y)| json!([x, y])).collect();
            inputs.insert(iname.clone(), Value::Array(pts));
        }
        settings.insert(
            name.clone(),
            json!({ "base_value": u.base_value, "inputs": inputs }),
        );
    }

    let doc = json!({
        "version": 3,
        "comment": brush.comment.clone().unwrap_or_else(|| "MyPaint brush file".to_string()),
        "parent_brush_name": brush.parent_brush_name.clone().unwrap_or_default(),
        "settings": settings,
        "group": brush.group.clone().unwrap_or_default(),
        "notes": notes,
        "description": description,
    });
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

/// 从原始 `.myb` 取 `description` / `notes`（保存时原样保留）；解析失败退回 entry 元数据。
#[cfg(test)]
fn raw_meta(entry: &BrushEntry) -> (String, String) {
    let Ok(raw) = entry.read_json() else {
        return (entry.display.clone(), entry.notes.clone());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (entry.display.clone(), entry.notes.clone());
    };
    (
        v.get("description")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        v.get("notes")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_metadata_matches_brushsettings_json() {
        // 编辑器显示名 / 出厂默认值 / 输入轴名都来自 brushsettings.json
        assert_eq!(setting_display_name("opaque"), "Opacity");
        assert_eq!(setting_display_name("radius_logarithmic"), "Radius");
        assert_eq!(setting_display_name("not_a_setting"), "not_a_setting");
        assert!((setting_default("radius_logarithmic").unwrap() - 2.0).abs() < 1e-9);
        assert!((setting_default("opaque").unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(setting_default("not_a_setting"), None);
        assert_eq!(input_display_name("pressure"), "Pressure");
        assert_eq!(input_display_name("tilt_declination"), "Declination/Tilt");
    }

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
        assert_eq!(seed[1].map(|e| lib.entries[e].key.as_str()), Some(group[0]));
        assert_eq!(seed[0].map(|e| lib.entries[e].key.as_str()), Some(group[9]));
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

    #[test]
    fn groups_rename_and_delete_move_orphans_to_deleted() {
        let mut lib = BrushLibrary::default();
        let dir = std::env::temp_dir().join(format!("pigment-group-rename-{}", std::process::id()));
        lib.order_conf_path = Some(dir.join("order.conf"));
        lib.groups = vec![
            BrushGroup {
                id: "mine".into(),
                display: "Mine".into(),
                members: vec!["classic/pencil".into()],
            },
            BrushGroup {
                id: FAVORITES_BRUSH_GROUP.into(),
                display: FAVORITES_BRUSH_GROUP.into(),
                members: Vec::new(),
            },
        ];
        assert!(lib.rename_group("Mine", "Renamed").is_ok());
        assert_eq!(lib.groups[0].display, "Renamed");
        assert!(
            lib.rename_group("Renamed", "Favorites").is_err(),
            "重名拒绝"
        );
        assert!(lib.delete_group("Renamed"));
        assert!(lib.delete_group(FAVORITES_BRUSH_GROUP) == false);
        let deleted = lib
            .groups
            .iter()
            .find(|g| g.id == DELETED_BRUSH_GROUP)
            .expect("Deleted 组自动创建");
        assert_eq!(deleted.members, vec!["classic/pencil".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_brush_writes_new_key_and_updates_groups() {
        let mut lib = BrushLibrary::embedded();
        let dir = std::env::temp_dir().join(format!("pigment-brush-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        lib.order_conf_path = Some(dir.join("order.conf"));
        let key = lib.entries[0].key.clone();
        let brush = lib.entries[0].load().expect("加载首支笔刷");
        let old_in_group = lib
            .groups
            .iter()
            .any(|g| g.members.iter().any(|m| *m == key));
        let new_key = lib
            .rename_brush(&key, "Renamed Brush", &brush, None)
            .expect("重命名成功");
        assert_ne!(new_key, key);
        assert!(new_key.ends_with("Renamed_Brush"));
        let idx = lib.find(&new_key).expect("新 key 在库里");
        assert_eq!(lib.entries[idx].name, "Renamed_Brush");
        assert!(lib.entries[idx].origin_path().is_some_and(|p| p.is_file()));
        assert!(
            lib.groups
                .iter()
                .any(|g| g.members.iter().any(|m| *m == new_key))
                == old_in_group
        );
        assert!(lib.find(&key).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn order_conf_parser_keeps_empty_groups_and_comments() {
        // 逐行照 _parse_order_conf：空行/# 注释跳过、Group 起组、组内去重
        let parsed =
            parse_order_conf("# header\nGroup: Favorites\nGroup: A\na/one\n# note\nA/two\na/one\n");
        assert_eq!(
            parsed,
            vec![
                ("Favorites".to_string(), vec![]),
                (
                    "A".to_string(),
                    vec!["a/one".to_string(), "A/two".to_string()]
                ),
            ]
        );
    }

    #[test]
    fn user_dir_myb_overrides_embedded_brush() {
        let dir =
            std::env::temp_dir().join(format!("pigment-brush-override-{}", std::process::id()));
        let group = dir.join("classic");
        fs::create_dir_all(&group).unwrap();
        // 最小 .myb：只改半径；其余设置由 hokusai 的默认值补齐。
        fs::write(
            group.join("dry_brush.myb"),
            r#"{"version":3,"group":"classic","parent_brush_name":"","settings":{"radius_logarithmic":{"base_value":5.0}}}"#,
        )
        .unwrap();

        let mut lib = BrushLibrary::embedded();
        let i0 = lib.find("classic/dry_brush").expect("内嵌包应有 dry_brush");
        assert!(matches!(lib.entries[i0].origin, Origin::Embedded(_)));
        let n0 = lib.entries.len();

        lib.scan_dir(&dir);
        assert_eq!(lib.entries.len(), n0, "同名用户笔刷不能追加成第二份");
        let i = lib.find("classic/dry_brush").unwrap();
        assert!(matches!(lib.entries[i].origin, Origin::File(_)));
        let b = lib.entries[i].load().unwrap();
        assert!(
            (b.get(BrushSetting::Radius).base_value - 5.0).abs() < 1e-6,
            "应载入用户目录里的版本"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn omitted_paint_mode_defaults_to_pigment_one() {
        // MyPaint 2.x 应用层默认 `pigment_on_by_default=True`（compatconfig.py:44-46）
        let lib = BrushLibrary::embedded();
        let i = lib.find("classic/dry_brush").unwrap();
        assert!(!json_has_setting(
            &lib.entries[i].read_json().unwrap(),
            "paint_mode"
        ));
        assert!(!lib.entries[i].has_explicit_paint_mode());
        let b = lib.entries[i].load().unwrap();
        assert_eq!(
            b.get(BrushSetting::Paint).base_value,
            1.0,
            "省略 paint_mode 的官方笔刷应以光谱 WGM 起手"
        );
        // 1.x 兼容模式：同一个省略键的笔刷默认 Normal
        let b1x = lib.entries[i].load_with_default(false).expect("解析失败");
        assert_eq!(
            b1x.get(BrushSetting::Paint).base_value,
            0.0,
            "1.x 兼容模式应把省略 paint_mode 的笔刷默认成 Normal"
        );

        // 显式写 0.0 的笔刷必须保留 Normal，不能被默认覆盖
        let dir =
            std::env::temp_dir().join(format!("pigment-brush-paintmode-{}", std::process::id()));
        let group = dir.join("classic");
        fs::create_dir_all(&group).unwrap();
        fs::write(
            group.join("dry_brush.myb"),
            r#"{"version":3,"group":"classic","settings":{"paint_mode":{"base_value":0.0}}}"#,
        )
        .unwrap();
        let mut lib2 = BrushLibrary::embedded();
        lib2.scan_dir(&dir);
        let j = lib2.find("classic/dry_brush").unwrap();
        assert!(lib2.entries[j].has_explicit_paint_mode());
        assert_eq!(
            lib2.entries[j]
                .load()
                .unwrap()
                .get(BrushSetting::Paint)
                .base_value,
            0.0
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_zero_settings_survive_save_and_load() {
        // Classic/Pencil 显式写 `anti_aliasing: 0.0`；hokusai 的 from_str 会把它
        // 当成未定义并回填 1.0（源码用“键是否存在”区分，`brush.py:300-305`）。
        let lib = BrushLibrary::embedded();
        let i = lib.find("classic/pencil").unwrap();
        assert!(!json_has_setting(
            &lib.entries[i].read_json().unwrap(),
            "paint_mode"
        ));
        let b = lib.entries[i].load().unwrap();
        assert_eq!(b.get(BrushSetting::AntiAliasing).base_value, 0.0);

        let json = serialize_brush(&b, "", "").unwrap();
        assert!(json_has_setting(&json, "anti_aliasing"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["settings"]["anti_aliasing"]["base_value"].as_f64(),
            Some(0.0),
            "显式 0 必须写进文件"
        );
        // 重新走我们的 load 路径仍应是 0
        let mut b2 = myb::from_str(&json).unwrap();
        apply_raw_settings(&json, &mut b2);
        assert_eq!(b2.get(BrushSetting::AntiAliasing).base_value, 0.0);
    }

    #[test]
    fn save_preserves_description_and_notes() {
        let lib = BrushLibrary::embedded();
        let i = lib.find("deevad/2B_pencil").unwrap();
        let (desc, notes) = raw_meta(&lib.entries[i]);
        assert!(desc.contains("digital pencil"), "desc={desc}");
        assert!(notes.contains("David Revoy"), "notes={notes}");

        let b = lib.entries[i].load().unwrap();
        let json = serialize_brush(&b, &desc, &notes).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["description"].as_str(), Some(desc.as_str()));
        assert_eq!(v["notes"].as_str(), Some(notes.as_str()));
        assert_eq!(v["comment"].as_str(), Some("MyPaint brush file"));
        // 全部设置都写出（含显式零），而不是 hokusai 的“非零才写”
        assert!(
            v["settings"].as_object().unwrap().len() >= BrushSetting::ALL.len(),
            "应写全部设置"
        );
    }

    #[test]
    fn context_slots_roundtrip_through_files() {
        let dir =
            std::env::temp_dir().join(format!("pigment-brush-context-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut lib = BrushLibrary::embedded();
        lib.order_conf_path = Some(dir.join("order.conf"));

        let i = lib.find("classic/pencil").unwrap();
        let mut b = lib.entries[i].load().unwrap();
        b.set(BrushSetting::ColorH, SettingValue::constant(0.42));
        b.set(BrushSetting::ColorS, SettingValue::constant(0.8));
        b.set(BrushSetting::ColorV, SettingValue::constant(0.6));
        lib.save_context(3, &b).unwrap();
        assert!(dir.join("context03.myb").is_file());

        let loaded = lib.load_contexts();
        assert!(loaded[0].is_none());
        let s = loaded[3].as_ref().expect("槽位 3 应读回");
        assert_eq!(s.get(BrushSetting::AntiAliasing).base_value, 0.0);
        assert!((s.get(BrushSetting::ColorH).base_value - 0.42).abs() < 1e-6);
        assert!((s.get(BrushSetting::ColorS).base_value - 0.8).abs() < 1e-6);
        assert!((s.get(BrushSetting::ColorV).base_value - 0.6).abs() < 1e-6);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn official_groups_include_empty_favorites() {
        let lib = BrushLibrary::embedded();
        let faves = lib
            .groups
            .iter()
            .find(|g| g.id == FAVORITES_BRUSH_GROUP)
            .expect("官方 order.conf 有空 Favorites 组");
        assert!(faves.members.is_empty());
        assert_eq!(faves.title(), "Favorites");
        // 官方 order.conf 覆盖 196 支；不应出现 Lost & Found
        assert!(!lib.groups.iter().any(|g| g.id == FOUND_BRUSHES_GROUP));
    }

    #[test]
    fn favorites_are_multi_group_and_roundtrip_order_conf() {
        let mut lib = BrushLibrary::embedded();
        let key = "classic/pencil";
        assert!(!lib.is_favorite(key));
        assert!(lib.toggle_favorite(key));
        assert!(lib.is_favorite(key));
        // 仍留在原组（MyPaint 的 Favorites 是"额外挂一份"）
        assert!(lib
            .groups
            .iter()
            .find(|g| g.id == "classic")
            .unwrap()
            .members
            .iter()
            .any(|k| k == key));

        let dir = std::env::temp_dir().join(format!("pigment-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        lib.order_conf_path = Some(dir.join("order.conf"));
        let path = lib.save_order_conf().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# this file saves brush groups and order\n"));
        let parsed = parse_order_conf(&text);
        let faves = parsed.iter().find(|(n, _)| n == "Favorites").unwrap();
        assert_eq!(faves.1, vec![key.to_string()]);

        // 重新载入用户 order.conf：Favorites 仍能看到成员
        let mut reloaded = BrushLibrary::embedded();
        reloaded.apply_order_conf(&text);
        assert!(reloaded.is_favorite(key));
        assert_eq!(reloaded.group_members(FAVORITES_BRUSH_GROUP).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_group_and_reorder_are_persisted_in_memory() {
        let mut lib = BrushLibrary::embedded();
        let id = lib.create_group("My Group").unwrap();
        assert!(lib.group_members(&id).is_empty());
        assert_eq!(lib.create_group("My Group").unwrap(), id, "同名返回原组");

        let members = lib.group_members("classic");
        let second = lib.entries[members[1]].key.clone();
        assert!(lib.move_in_group("classic", &second, 0));
        let after = lib.group_members("classic");
        assert_eq!(after[0], members[1], "移到组首");
    }

    #[test]
    fn new_brush_goes_to_front_of_new_group() {
        let mut lib = BrushLibrary::embedded();
        let dir = std::env::temp_dir().join(format!("pigment-newbrush-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        lib.order_conf_path = Some(dir.join("order.conf"));
        let mut brush = lib.entries[0].load().unwrap();
        let preview = vec![10u8, 20, 30, 255];
        let idx = lib.new_brush(&mut brush, Some((1, 1, &preview))).unwrap();
        assert_eq!(lib.entries[idx].key, "b000");
        assert_eq!(lib.entries[idx].group, NEW_BRUSH_GROUP);
        assert_eq!(lib.group_members(NEW_BRUSH_GROUP), vec![idx]);
        assert!(dir.join("b000.myb").is_file());
        assert!(dir.join("b000_prev.png").is_file());
        // 第二支 b001，插到组首
        let idx2 = lib.new_brush(&mut brush, Some((1, 1, &preview))).unwrap();
        assert_eq!(lib.entries[idx2].key, "b001");
        assert_eq!(lib.group_members(NEW_BRUSH_GROUP), vec![idx2, idx]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_user_brush_removes_files_and_stock_goes_to_deleted_group() {
        let mut lib = BrushLibrary::embedded();
        let dir = std::env::temp_dir().join(format!("pigment-del-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        lib.order_conf_path = Some(dir.join("order.conf"));
        // 先造一支用户笔刷
        let mut brush = lib.entries[0].load().unwrap();
        let preview = vec![0u8, 0, 0, 255];
        let idx = lib.new_brush(&mut brush, Some((1, 1, &preview))).unwrap();
        let key = lib.entries[idx].key.clone();
        assert!(dir.join(format!("{key}.myb")).is_file());
        assert!(lib.delete_brush(&key).unwrap(), "用户笔刷应彻底删除");
        assert!(lib.find(&key).is_none());
        assert!(!dir.join(format!("{key}.myb")).is_file());

        // 官方笔刷删不掉 → Deleted 组最前，原组移除
        let stock = "classic/pencil";
        assert!(!lib.delete_brush(stock).unwrap());
        assert!(lib.find(stock).is_some());
        assert_eq!(
            lib.group_members(DELETED_BRUSH_GROUP)[0],
            lib.find(stock).unwrap()
        );
        assert!(!lib
            .group_members("classic")
            .iter()
            .any(|&i| lib.entries[i].key == stock));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clone_writes_myb_and_preview_and_inserts_after_source() {
        let mut lib = BrushLibrary::embedded();
        let dir = std::env::temp_dir().join(format!("pigment-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        lib.order_conf_path = Some(dir.join("order.conf"));

        let src_key = "classic/pencil";
        let src_idx = lib.find(src_key).unwrap();
        let mut brush = lib.entries[src_idx].load().unwrap();
        let preview = vec![255u8, 0, 0, 128, 0, 0, 255, 255]; // 2×1 RGBA
        let idx = lib
            .clone_brush(src_key, &mut brush, Some((2, 1, &preview)))
            .unwrap();
        let new_key = lib.entries[idx].key.clone();
        assert_eq!(new_key, "classic/pencil copy");
        assert_eq!(
            lib.entries[idx].origin_path(),
            Some(dir.join("classic/pencil copy.myb")).as_deref()
        );
        let json = std::fs::read_to_string(dir.join("classic/pencil copy.myb")).unwrap();
        let parsed = hokusai::myb::from_str(&json).unwrap();
        assert_eq!(parsed.parent_brush_name.as_deref(), Some(src_key));
        assert!(dir.join("classic/pencil copy_prev.png").is_file());

        // 插在源笔刷之后，仍在 Classic 组
        let members = lib.group_members("classic");
        let pos = members.iter().position(|&i| i == idx).unwrap();
        assert_eq!(members[pos - 1], src_idx);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_rgba_decodes_the_embedded_icon() {
        let lib = BrushLibrary::embedded();
        let e = &lib.entries[lib.find("classic/pencil").unwrap()];
        let (w, h, px) = e.preview_rgba().expect("官方笔刷应有内嵌预览");
        assert_eq!((w, h), (128, 128), "MyPaint 预览固定 128×128");
        assert_eq!(px.len(), (w * h * 4) as usize);
        assert!(px.chunks_exact(4).any(|p| p[3] > 0));
    }

    /// `save_brush_entry(key, ..., None)` 不能重写 `_prev.png`
    /// （源码 `ManagedBrush.save` 只在有 `self.preview` 时写）。
    #[test]
    fn saving_without_preview_keeps_the_existing_icon() {
        let mut lib = BrushLibrary::embedded();
        let dir = std::env::temp_dir().join(format!("pigment-prev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        lib.order_conf_path = Some(dir.join("order.conf"));
        let key = "classic/pencil";
        let brush = lib.entries[lib.find(key).unwrap()].load().unwrap();

        let one = [1u8, 2, 3, 255];
        lib.save_brush_entry(key, &brush, Some((1, 1, &one)))
            .unwrap();
        let prev = dir.join("classic/pencil_prev.png");
        let sentinel = std::fs::read(&prev).unwrap();
        // 再保存一次、不传预览：磁盘上的旧图标必须原样保留
        lib.save_brush_entry(key, &brush, None).unwrap();
        assert_eq!(std::fs::read(&prev).unwrap(), sentinel);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 新建/Clone 传入的预览必须原样写盘（不是重新渲染的缩略图）。
    #[test]
    fn new_brush_copies_the_source_preview_pixels() {
        let mut lib = BrushLibrary::embedded();
        let dir = std::env::temp_dir().join(format!("pigment-prevcopy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        lib.order_conf_path = Some(dir.join("order.conf"));
        let src = &lib.entries[lib.find("classic/pencil").unwrap()];
        let (w, h, px) = src.preview_rgba().unwrap();
        let mut brush = src.load().unwrap();
        lib.new_brush(&mut brush, Some((w, h, &px))).unwrap();

        let out = image::open(dir.join("b000_prev.png")).unwrap().to_rgb8();
        assert_eq!(out.dimensions(), (w, h));
        // 抽样：RGBA 按源码规则合成到白底
        for (x, y) in [(0, 0), (w / 3, h / 2), (w - 1, h - 1), (10, 20)] {
            let i = ((y * w + x) * 4) as usize;
            let p = out.get_pixel(x, y);
            for c in 0..3 {
                let a = px[i + 3] as u32;
                let want = (px[i + c] as u32 * a + 255 * (255 - a)) / 255;
                assert!(
                    (p[c] as i32 - want as i32).abs() <= 1,
                    "({x},{y}) 通道 {c} 应保留原图标"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn three_way_merge_adds_upstream_brushes_and_keeps_our_order() {
        let pair = |s: &str, v: &[&str]| (s.to_string(), v.iter().map(|x| x.to_string()).collect());
        let base = vec![pair("A", &["a1", "a2"]), pair("B", &["b1"])];
        let our = vec![pair("A", &["a2", "a1"]), pair("D", &["d1"])];
        let their = vec![pair("A", &["a1", "new", "a2"]), pair("C", &["c1"])];
        let merged = merge_order_groups(&base, &our, &their);
        let get = |name: &str| {
            merged
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("A"), vec!["a2", "a1", "new"]);
        assert_eq!(get("C"), vec!["c1"]);
        assert_eq!(get("D"), vec!["d1"]);
        assert!(!merged.iter().any(|(n, _)| n == "B"), "空组应被删掉");
        // 组顺序按 base→our→their 首次出现（源码用 set，顺序本就不确定；
        // 这里取确定性顺序）：A、B、D、C，去掉空 B 后是 A、D、C。
        let names: Vec<&str> = merged.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["A", "D", "C"]);
    }

    #[test]
    fn three_way_merge_removes_brushes_deleted_upstream() {
        let pair = |s: &str, v: &[&str]| (s.to_string(), v.iter().map(|x| x.to_string()).collect());
        let base = vec![pair("A", &["a1", "gone"])];
        let our = vec![pair("A", &["a1", "gone", "mine"])];
        let their = vec![pair("A", &["a1"])];
        let merged = merge_order_groups(&base, &our, &their);
        assert_eq!(merged[0].1, vec!["a1", "mine"]);
    }

    #[test]
    fn three_way_merge_keeps_favorites_when_empty() {
        let pair = |s: &str, v: &[&str]| (s.to_string(), v.iter().map(|x| x.to_string()).collect());
        let base = vec![pair("Favorites", &["f1"])];
        let our = vec![pair("Favorites", &["f1"])];
        let their = vec![pair("Favorites", &[])];
        let merged = merge_order_groups(&base, &our, &their);
        assert_eq!(merged, vec![pair("Favorites", &[])]);
    }
}
