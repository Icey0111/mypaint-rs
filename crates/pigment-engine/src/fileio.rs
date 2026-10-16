//! 文档 I/O —— 照搬 MyPaint `lib/document.py` 的 `save`/`load` 分派与
//! `.ora` / `.png` 读写。
//!
//! # 支持格式（以源码为准）
//!
//! | 扩展名 | MyPaint 方法 | 本文件 |
//! |---|---|---|
//! | `.png` | `save_png` / `load_png`（`:1646` / `:1732`） | [`save_png`] / [`load_png`] |
//! | `.ora` | `save_ora` / `load_ora`（`:1770` / `:1804`） | [`save_ora`] / [`load_ora`] |
//! | `.jpg` | `save_jpg` / `load_from_pixbuf_file`（`:1747` / `:1738`） | 未做（见下） |
//!
//! **报告里写的 `.myp` 是不存在的** —— `MyPaint` master 全仓没有 `save_myp` /
//! `load_myp`，文件对话框只列 `*.ora *.png *.jpg *.jpeg`（`gui/filehandling.py:437`）。
//! 本轮只做 `.ora` 与 `.png`；`.jpg` 有损且 MyPaint 走 GdkPixbuf，
//! 优先级低于前两者，留待需要时补。
//!
//! # `.ora` 的结构（照搬 `_save_layers_to_new_orazip`，`:2014`）
//!
//! 条目顺序与内容：
//!
//! ```text
//! mimetype                      "image/openraster"（**必须第一个且不压缩**）
//! data/layer-00.png             图层像素（`_make_refname("layer", (0,), ".png")`）
//! data/mypaint-settings.json   文档设置 JSON（`json.dumps(dict(settings), indent=2)`）
//! Thumbnails/thumbnail.png     256×256 预览
//! mergedimage.png              摊平后的预览（alpha=False）
//! stack.xml                    图层树描述
//! ```
//!
//! `stack.xml` 的字节格式按**真实样本**对齐（见 `_mypaint_ref/tests/*.ora`
//! 与 `fill_outlines.ora`）：声明用单引号、缩进 2 空格、空元素写成 `<x />`、
//! 属性排序规则是「先无命名空间、再按命名空间」，组内按**局部名**字典序。
//!
//! # 与源码的三处差异（都是"暂无图层栈/画框"造成）
//!
//! 1. **单图层**：文档只有一层，路径固定 `(0,)`，所以图层文件名恒为
//!    `data/layer-00.png`。P1 引入图层栈后按真实路径生成 `layer-00-01.png` 之类。
//! 2. **无 strokemap**：源码会额外写 `data/layer-00-strokemap.dat` 并挂
//!    `mypaint:strokemap` 属性（`lib/layer/data.py:1676`）；我们还没有 strokemap
//!    层，故不写。读侧对缺这一项的文件完全兼容（MyPaint 自己也是
//!    "有就读、没有就跳过"，`data.py:1545`）。
//! 3. **`mypaint:version` 写 `"2.0.0"` 而不是上游源码字面量 `"2.1.0-alpha"`**：
//!    该属性会被对方 `Document._compat_check`（`:1788`）拿去做
//!    `lib.meta.compatibility()` 判定，写"未来版本的预发布号"会让真正的
//!    MyPaint 2.0.x 报 PARTIALLY 并弹兼容警告（见 [`compatibility`] 的移植）。
//!    我们实现的是 2.0 的 ORA 方言，故写 2.0.0。
//! 4. **图层 PNG 保留 alpha**：源码的 `_save_rect_to_ora` 未显式传 `alpha`
//!    （`save_as_png` 的默认是 `False`），但仓库内真实 ORA（如
//!    `smallimage.ora` 的 `data/layer000.png`）里的图层 PNG 都是 RGBA；
//!    单图层文档若摊平会把数码画的透明区变成底色，破坏文档语义。
//!    故图层写 `alpha=true`、`mergedimage.png` 写 `alpha=false`（与源码一致）。

use std::path::Path;

use hokusai::tile::TILE_SIZE;
use pigment_surface::combine::{CombineMode, DEFAULT_MODE};
use pigment_surface::color::{self, CANVAS_BG};
use pigment_surface::{LayerStack, PigmentSurface};

use crate::zip::{ZipArchive, ZipWriter};

// ---- 常量（照搬 lib/xml.py、lib/document.py、lib/meta.py）----

/// `lib/xml.py:22`。
pub const OPENRASTER_MEDIA_TYPE: &str = "image/openraster";
/// `lib/xml.py:23`。
pub const OPENRASTER_VERSION: &str = "0.0.5";
/// `lib/xml.py:24`。
pub const OPENRASTER_MYPAINT_NS: &str = "http://mypaint.org/ns/openraster";
/// 写进 `mypaint:version` 的版本号。见模块文档第 3 条。
pub const MYPAINT_VERSION: &str = "2.0.0";
/// `lib/document.py:94` 的 `_ORA_JSON_SETTINGS_ZIP_PATH`。
pub const ORA_JSON_SETTINGS_ZIP_PATH: &str = "data/mypaint-settings.json";
/// `lib/layer/tree.py:564` 的 `size=256` 默认值。
pub const THUMBNAIL_SIZE: u32 = 256;
/// `mypaintlib.MAX_MIPMAP_LEVEL`（应用层 C++ 常量，本仓库克隆里没有定义；
/// 取值不影响结果 —— 循环另受 `max(w,h) <= 256` 约束，8 足够覆盖任何画幅）。
pub const MAX_MIPMAP_LEVEL: u32 = 8;

/// EOTF 值，写进 `mypaint:eotf`（`str(lib.eotf.eotf())` = `"2.2"`）。
pub const EOTF_ATTR_VALUE: &str = "2.2";

/// 属性名（带 MyPaint 命名空间前缀）。
pub const ATTR_NS_VERSION: &str = "mypaint:version";
pub const ATTR_NS_EOTF: &str = "mypaint:eotf";
pub const ATTR_NS_FRAME_ACTIVE: &str = "mypaint:frame-active";
pub const ATTR_NS_JSON_SETTINGS: &str = "mypaint:json-settings";
/// 背景层单块平铺 tile（`data.py:1047` `ORA_BGTILE_ATTR`）。
pub const ATTR_NS_BG_TILE: &str = "mypaint:background-tile";
/// 背景层的 legacy 属性名（`data.py:1046`，读侧兼容）。
pub const ATTR_BG_TILE_LEGACY: &str = "background_tile";

// ---- 错误 ----

/// 文档 I/O 错误。
///
/// 对应源码的 `lib.error.FileHandlingError`（带用户可读串）。
#[derive(Debug)]
pub enum FileError {
    Io(std::io::Error),
    Zip(crate::zip::ZipError),
    Png(String),
    /// 格式不支持（`Document._unsupported`）。
    Unsupported(String),
    /// 文件不存在（`load` 的前置检查，`:1513`）。
    NotFound(String),
    /// 结构非法（缺条目、XML 不合法等）。
    Malformed(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Zip(e) => write!(f, "{e}"),
            Self::Png(e) => write!(f, "{e}"),
            Self::Unsupported(ext) => write!(
                f,
                "无法保存或打开这种格式的文件（扩展名 {ext:?}）。\n\
                 支持：.ora（OpenRaster）、.png（PNG）"
            ),
            Self::NotFound(p) => write!(f, "文件不存在：{p}"),
            Self::Malformed(m) => write!(f, "文件结构非法：{m}"),
        }
    }
}

impl std::error::Error for FileError {}

impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<crate::zip::ZipError> for FileError {
    fn from(e: crate::zip::ZipError) -> Self {
        Self::Zip(e)
    }
}

// ---- 版本兼容判定（照搬 lib/meta.py:175-253）----

/// `lib/meta.py:175` 的 `Compatibility`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compatibility {
    /// 应用主版本 < 文件主版本（`:179`）。
    Incompatible,
    /// 主版本相同、应用次版本 < 文件次版本（`:181`）。
    Partially,
    /// 其余（`:183`）。
    Fully,
}

/// 解析版本串为 `(major, minor, patch, prerel)`（`lib/meta.py:134`）。
///
/// 非法输入返回 `None`（源码 doctest 里的四个反例都要被拒）。
pub fn parse_version_string(s: &str) -> Option<(u32, u32, u32, String)> {
    let (base, prerel) = match s.find('-') {
        Some(i) => {
            let p = &s[i..];
            // 去掉预发布号：`-alpha.3` → `-alpha`
            let p = match p.find('.') {
                Some(j) => &p[..j],
                None => p,
            };
            (&s[..i], p.to_string())
        }
        None => (s, String::new()),
    };
    if !matches!(prerel.as_str(), "" | "-alpha" | "-beta" | "-rc") {
        return None;
    }
    let parts: Vec<&str> = base.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut nums = [0u32; 3];
    for (i, p) in parts.iter().enumerate() {
        // 源码用 int()，负号会被拒；这里同样拒掉非纯数字
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        nums[i] = p.parse().ok()?;
    }
    Some((nums[0], nums[1], nums[2], prerel))
}

/// `lib/meta.py:194` 的 `compatibility()`：当前版本（[`MYPAINT_VERSION`]）
/// 对目标版本的兼容度。
///
/// 目标版本串非法时返回 `None`（源码返回 `None`，调用方 `_compat_check`
/// 据此放行）。
///
/// 分支刻意与源码 `_compatibility`（`lib/meta.py:226-238`）逐行对应 ——
/// 其中两条分支给出同一个 `Fully`，clippy 的 `if_same_then_else` 会报，
/// 但合并它们就丢掉了"这条规则来自源码哪一行"的线索，所以保留并豁免。
#[allow(clippy::if_same_then_else)]
pub fn compatibility(target: &str) -> Option<Compatibility> {
    let (t_major, t_minor, _t_patch, _t_pre) = parse_version_string(target)?;
    let (c_major, c_minor, _c_patch, _c_pre) = parse_version_string(MYPAINT_VERSION)?;
    Some(if t_major > c_major {
        // 应用主版本 < 文件主版本 → 不兼容（:229）
        Compatibility::Incompatible
    } else if t_major < c_major {
        // 文件主版本更低 → 完全兼容（:231）
        Compatibility::Fully
    } else if t_minor < c_minor {
        // 次版本更低 → 完全兼容（:233）
        Compatibility::Fully
    } else if t_minor > c_minor {
        // 次版本更高 → 只能部分兼容（:235）
        Compatibility::Partially
    } else {
        Compatibility::Fully
    })
}

// ---- ORA 文件名规则 ----

/// `lib/layer/data.py:462` 的 `_make_refname(prefix, path, suffix, sep="-"`。
///
/// - 路径元素按 `%02d` 补齐后以 `-` 连接
/// - `suffix` 不以 `.` 开头时补一个 `-`
///
/// 实例：`("layer", (0,), ".png")` → `"layer-00.png"`；
/// `("layer", (0, 1), "strokemap.dat")` → `"layer-00-01-strokemap.dat"`。
pub fn make_refname(prefix: &str, path: &[usize], suffix: &str) -> String {
    let path_ref = path
        .iter()
        .map(|n| format!("{n:02}"))
        .collect::<Vec<_>>()
        .join("-");
    let suffix = if suffix.starts_with('.') {
        suffix.to_string()
    } else {
        format!("-{suffix}")
    };
    format!("{prefix}-{path_ref}{suffix}")
}

// ---- XML 写出 ----

/// 按真实 ORA 样本的属性排序规则输出属性。
///
/// 规则（由 `smallimage.ora` 与 `fill_outlines.ora` 双向验证）：
/// **先无命名空间属性，再带命名空间属性；组内按局部名（`:` 之后）字典序。**
fn sorted_attrs(attrs: &[(&str, String)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = attrs
        .iter()
        .map(|(k, val)| (k.to_string(), val.clone()))
        .collect();
    v.sort_by(|a, b| {
        let key = |s: &str| {
            let (ns, local) = match s.find(':') {
                Some(i) => (1u8, s[i + 1..].to_string()),
                None => (0u8, s.to_string()),
            };
            (ns, local)
        };
        key(&a.0).cmp(&key(&b.0))
    });
    v
}

/// XML 属性值的转义（对应 `xml.sax.saxutils.quoteattr` 的最小集）。
fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

/// 按 CPython `str(float)` 的习惯输出浮点数属性值。
///
/// 源码里 `attrs["opacity"] = str(self.opacity)`，而 `self.opacity` 是 float 1.0，
/// 所以写出的是 **`"1.0"`** 而不是 `"1"`。Rust 的 `{}` 对 `1.0f32` 会给出 `"1"`，
/// 与真实文件对不上，故这里显式补出小数部分。
pub fn fmt_float_py(v: f32) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1e15 {
        // 整数值：Python 会保留一位小数（1.0、0.0、2.0 …）
        format!("{v:.1}")
    } else {
        // 其余交给 Rust 的最短往返表示，与 Python 的 repr 在常见取值上一致
        format!("{v}")
    }
}

/// 写一个**自闭合**元素（对应 ElementTree 的 `<x ... />`）。
fn write_empty_element(out: &mut String, indent: usize, tag: &str, attrs: &[(&str, String)]) {
    write_attrs(out, indent, tag, attrs);
    out.push_str(" />\n");
}

/// 写一个**容器**元素的开始标签（对应 ElementTree 的 `<x ...>`）。
fn write_open_element(out: &mut String, indent: usize, tag: &str, attrs: &[(&str, String)]) {
    write_attrs(out, indent, tag, attrs);
    out.push_str(">\n");
}

fn write_attrs(out: &mut String, indent: usize, tag: &str, attrs: &[(&str, String)]) {
    out.push_str(&"  ".repeat(indent));
    out.push('<');
    out.push_str(tag);
    for (k, v) in sorted_attrs(attrs) {
        out.push(' ');
        out.push_str(&k);
        out.push_str("=\"");
        out.push_str(&escape_attr(&v));
        out.push('"');
    }
}

/// 写一个容器元素的结束标签（`indent_etree` 的缩进规则）。
fn write_close_element(out: &mut String, indent: usize, tag: &str) {
    out.push_str(&"  ".repeat(indent));
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// 生成 `stack.xml`（照搬 `_save_layers_to_new_orazip` 拼装 `image` 的部分，
/// B2-4 起多图层 + 背景层）。
///
/// - `layer_elems`：自顶向下各 `<layer>` 的属性（`group.py:503-508` 按
///   `_layers` 序写出 —— index 0 = 最顶层在文档最前）
/// - `bg_elem`：背景层元素，总是根 stack 的**最后一个**子元素
///   （`tree.py:2513-2526`，`bg_path = (len(self),)`）
/// - 根 `<stack>`：`RootLayerStack.INITIAL_MODE = CombineNormal`
///   （`tree.py:107`）→ `composite-op="svg:src-over"`、`isolation="isolate"`、
///   `name="Root"`（`RootLayerStack.DEFAULT_NAME`）
fn build_stack_xml(
    image_w: i32,
    image_h: i32,
    xres: Option<u32>,
    yres: Option<u32>,
    frame_active: bool,
    json_settings_entry: Option<&str>,
    layer_elems: &[Vec<(&'static str, String)>],
    bg_elem: Option<&[(&'static str, String)]>,
) -> String {
    let mut s = String::with_capacity(1024);
    // ElementTree.tostring 的声明用单引号（与真实样本一致）
    s.push_str("<?xml version='1.0' encoding='UTF-8'?>\n");

    // ---- image ----
    s.push_str("<image xmlns:mypaint=\"");
    s.push_str(OPENRASTER_MYPAINT_NS);
    s.push('"');
    let mut image_attrs: Vec<(&str, String)> = vec![
        ("w", image_w.to_string()),
        ("h", image_h.to_string()),
        ("version", OPENRASTER_VERSION.to_string()),
        (ATTR_NS_EOTF, EOTF_ATTR_VALUE.to_string()),
        (
            ATTR_NS_FRAME_ACTIVE,
            if frame_active { "true" } else { "false" }.to_string(),
        ),
        (ATTR_NS_VERSION, MYPAINT_VERSION.to_string()),
    ];
    if let Some(j) = json_settings_entry {
        image_attrs.push((ATTR_NS_JSON_SETTINGS, j.to_string()));
    }
    // 源码只在 xres/yres 都为真时才写（`:2124`）
    if let (Some(x), Some(y)) = (xres, yres) {
        if x > 0 && y > 0 {
            image_attrs.push(("xres", x.to_string()));
            image_attrs.push(("yres", y.to_string()));
        }
    }
    for (k, v) in sorted_attrs(&image_attrs) {
        s.push(' ');
        s.push_str(&k);
        s.push_str("=\"");
        s.push_str(&escape_attr(&v));
        s.push('"');
    }
    s.push_str(">\n");

    // ---- 根 stack（容器元素；照 RootLayerStack 语义）----
    let stack_attrs: Vec<(&str, String)> = vec![
        ("opacity", fmt_float_py(1.0)),
        ("visibility", "visible".to_string()),
        ("composite-op", "svg:src-over".to_string()),
        ("isolation", "isolate".to_string()),
        ("name", "Root".to_string()),
    ];
    write_open_element(&mut s, 1, "stack", &stack_attrs);

    // ---- 图层（自顶向下）----
    for attrs in layer_elems {
        write_empty_element(&mut s, 2, "layer", attrs);
    }
    // ---- 背景层：根 stack 的最后一个子元素 ----
    if let Some(bg) = bg_elem {
        write_empty_element(&mut s, 2, "layer", bg);
    }

    write_close_element(&mut s, 1, "stack");
    write_close_element(&mut s, 0, "image");
    s
}

// ---- 缩略图（照搬 lib/layer/tree.py:564 render_layer_preview）----

/// 生成 256×256 上限的缩略图，返回 `(pixels_rgba8, w, h)`。
///
/// 算法照搬 `render_layer_preview`：
/// 1. mipmap 逐级减半，直到 `max(w,h) <= size`（每级 `x/=2; y/=2; w/=2; h/=2`）
/// 2. `w = max(1,w)`、`h = max(1,h)`
/// 3. 若 `w` 与 `h` 都不等于 `size` → `scale_proportionally(pixbuf, size, size)`，
///    而该函数是 `shrink_only=True`，**缩放比 ≥ 1 时原样返回** ——
///    所以第 1 步减半之后这一步实际是空操作。
///
/// 实证：`fill_outlines.ora`（画布 3456×3008）的缩略图是 **216×188**，
/// 正是 4 次减半的结果，第 3 步没有生效 —— 与本实现一致。
///
/// B2-4 起作用于整个图层栈（源码 `root_stack.render_thumbnail(bbox)`），
/// 合成含背景层。
pub fn render_thumbnail(layers: &LayerStack, bbox: (i32, i32, i32, i32)) -> (Vec<u8>, u32, u32) {
    let (mut x, mut y, mut w, mut h) = bbox;
    // 空画布：照 `_validate_layer_bbox_arg` 的下限「至少一个 tile」
    if w <= 0 || h <= 0 {
        return (vec![0u8; 4], 1, 1);
    }

    let mut level = 0;
    while level < MAX_MIPMAP_LEVEL && w.max(h) > THUMBNAIL_SIZE as i32 {
        level += 1;
        x /= 2;
        y /= 2;
        w /= 2;
        h /= 2;
    }
    let w = w.max(1);
    let h = h.max(1);

    // 按减半后的尺寸做区域平均（mipmap 本身就是逐级 box filter，
    // 对 2 的幂次减半而言等价）
    let factor = 1i32 << level;
    let mut full =
        vec![0u8; ((w * factor) * (h * factor) * 4) as usize];
    layers.render_rect_rgba8(
        (x * factor, y * factor, (w * factor) as u32, (h * factor) as u32),
        false,
        &mut full,
    );
    let src_w = (w * factor) as usize;
    let (tw, th) = (w as usize, h as usize);
    let mut out = vec![0u8; tw * th * 4];
    for ty in 0..th {
        for tx in 0..tw {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..factor as usize {
                for dx in 0..factor as usize {
                    let sx = tx * factor as usize + dx;
                    let sy = ty * factor as usize + dy;
                    let o = (sy * src_w + sx) * 4;
                    if o + 3 < full.len() {
                        r += full[o] as u32;
                        g += full[o + 1] as u32;
                        b += full[o + 2] as u32;
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            let o = (ty * tw + tx) * 4;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
            out[o + 3] = 255;
        }
    }
    (out, tw as u32, th as u32)
}

// ---- 保存 ----

/// `save_png` 的结果。
pub struct PngSaved {
    /// 实际写出的矩形（空画布会被退化成 1×1）。
    pub bbox: (i32, i32, i32, i32),
}

/// 导出 PNG（照搬 `lib/document.py:1646` `save_png` + `_save_single_file_png`）。
///
/// `bbox` 传 [`crate::document::Document::get_user_bbox`] 的结果。
/// `alpha = false` 时合成画布底色，`true` 时保留透明 ——
/// 源码的默认值是 `alpha = not self.layer_stack.background_visible`；
/// B2-3 起文档有背景层，调用方按背景可见性显式传参。
pub fn save_png(
    surface: &PigmentSurface,
    filename: impl AsRef<Path>,
    bbox: (i32, i32, i32, i32),
    alpha: bool,
) -> Result<PngSaved, FileError> {
    let (x, y, w, h) = surface
        .save_as_png(filename.as_ref(), bbox, alpha, CANVAS_BG)
        .map_err(FileError::Png)?;
    Ok(PngSaved { bbox: (x, y, w, h) })
}

/// ORA 保存的可选项（对应 `save_ora(filename, options=None, **kwargs)` 的 kwargs）。
#[derive(Debug, Clone)]
pub struct OraSaveOptions {
    /// 画幅；`None` 时用图层数据外接框（源码 `bbox=None` 分支）。
    pub bbox: Option<(i32, i32, i32, i32)>,
    pub xres: Option<u32>,
    pub yres: Option<u32>,
    /// 画框是否启用（写 `mypaint:frame-active`）。
    pub frame_active: bool,
    /// 文档设置 JSON（`None` 则整个属性与条目都不写）。
    pub settings_json: Option<String>,
}

impl Default for OraSaveOptions {
    fn default() -> Self {
        Self {
            bbox: None,
            xres: None,
            yres: None,
            frame_active: false,
            // 源码里 save_ora 传 `settings=dict(self._settings)`，
            // 默认文档是空 dict，序列化成 `{}`
            settings_json: Some("{}".to_string()),
        }
    }
}

/// `save_ora` 的结果。
pub struct OraSaved {
    /// 写进 `stack.xml` 的画幅。
    pub bbox: (i32, i32, i32, i32),
    /// 缩略图 `(pixels, w, h)`（源码 `save_ora` 也把 thumbnail 返回给 UI）。
    pub thumbnail: (Vec<u8>, u32, u32),
}

/// 保存为 OpenRaster（照搬 `lib/document.py:1770` `save_ora`
/// → `_save_layers_to_new_orazip`，`:2014`；图层栈部分照
/// `tree.py:2494` + `group.py:487` + `data.py:411`/`:1068`）。
pub fn save_ora(
    layers: &LayerStack,
    filename: impl AsRef<Path>,
    opts: &OraSaveOptions,
    data_bbox: (i32, i32, i32, i32),
) -> Result<OraSaved, FileError> {
    // 源码：`if bbox is None: bbox = data_bbox`
    let bbox = opts.bbox.unwrap_or(data_bbox);
    let (bx, by, bw, bh) = bbox;
    let bg_rgb = layers.background_color();

    let mut zip = ZipWriter::create(filename.as_ref())?;

    // 1. mimetype —— 必须第一个且 STORED
    zip.add_stored("mimetype", OPENRASTER_MEDIA_TYPE.as_bytes())?;

    // 2. 图层数据：自顶向下（`_layers` 序），每层写**自己的**数据外接框
    //    （`data.py:411` 用 `self.get_bbox()`，不是画幅），坐标相对画幅左上角。
    let mut layer_elems: Vec<Vec<(&'static str, String)>> = Vec::new();
    for (i, layer) in layers.iter_top_down() {
        let rect = surface_tile_bbox(&layer.surface);
        let entry = format!("data/{}", make_refname("layer", &[i], ".png"));
        let tmp_png = temp_sibling(filename.as_ref(), &format!("layer-{i:02}.png"));
        layer
            .surface
            .save_as_png(&tmp_png, rect, true, bg_rgb)
            .map_err(FileError::Png)?;
        let bytes = std::fs::read(&tmp_png)?;
        let _ = std::fs::remove_file(&tmp_png);
        zip.add_stored(&entry, &bytes)?;
        let (lx, ly, _, _) = rect;
        // `_get_stackxml_element("layer", x, y)` + `_save_rect_to_ora` 的 src
        let mut attrs: Vec<(&'static str, String)> = vec![
            ("x", (lx - bx).to_string()),
            ("y", (ly - by).to_string()),
            ("opacity", fmt_float_py(layer.opacity)),
            (
                "visibility",
                if layer.visible { "visible" } else { "hidden" }.to_string(),
            ),
            ("composite-op", layer.mode.ora_name().to_string()),
            ("src", entry),
        ];
        if let Some(n) = &layer.name {
            if !n.is_empty() {
                attrs.push(("name", n.clone()));
            }
        }
        if layer.locked {
            attrs.push(("edit-locked", "true".to_string()));
        }
        // 「Update the initially-selected flag on all layers」（document.py:2077-2079）
        if i == layers.current_index() {
            attrs.push(("selected", "true".to_string()));
        }
        layer_elems.push(attrs);
    }

    // 3. 背景层（`tree.py:2513-2526` 总是写；`data.py:1068`）：
    //    - 整幅铺满画幅的不透明 PNG（背景表面循环平铺 → 纯色即等价）
    //    - 单块 64×64 tile，挂 `mypaint:background-tile`
    let n = layers.len();
    let bg_full_entry = format!("data/{}", make_refname("background", &[n], ".png"));
    let bg_tile_entry = format!("data/{}", make_refname("background", &[n], "tile.png"));
    let fw = bw.max(1) as usize;
    let fh = bh.max(1) as usize;
    let mut bg_full = vec![0u8; fw * fh * 4];
    for px in bg_full.chunks_exact_mut(4) {
        px[..3].copy_from_slice(&bg_rgb);
        px[3] = 255;
    }
    let tmp_bg = temp_sibling(filename.as_ref(), "background.png");
    pigment_surface::png::write_rgba8_png(&tmp_bg, bg_full, fw as u32, fh as u32)
        .map_err(FileError::Png)?;
    let bytes = std::fs::read(&tmp_bg)?;
    let _ = std::fs::remove_file(&tmp_bg);
    zip.add_stored(&bg_full_entry, &bytes)?;

    let ts = TILE_SIZE as u32;
    let mut bg_tile = vec![0u8; (ts * ts * 4) as usize];
    for px in bg_tile.chunks_exact_mut(4) {
        px[..3].copy_from_slice(&bg_rgb);
        px[3] = 255;
    }
    let tmp_tile = temp_sibling(filename.as_ref(), "background-tile.png");
    pigment_surface::png::write_rgba8_png(&tmp_tile, bg_tile, ts, ts).map_err(FileError::Png)?;
    let bytes = std::fs::read(&tmp_tile)?;
    let _ = std::fs::remove_file(&tmp_tile);
    zip.add_stored(&bg_tile_entry, &bytes)?;

    // BackgroundLayer 的 mode = LayerBase 默认 = SpectralWGM（真实 MyPaint
    // 文件的背景元素就是 `composite-op="mypaint:spectral-wgm"`）。
    let bg_elem: Vec<(&'static str, String)> = vec![
        ("x", "0".to_string()),
        ("y", "0".to_string()),
        ("opacity", fmt_float_py(1.0)),
        ("visibility", "visible".to_string()),
        ("composite-op", "mypaint:spectral-wgm".to_string()),
        ("name", "background".to_string()),
        ("src", bg_full_entry),
        (ATTR_NS_BG_TILE, bg_tile_entry),
    ];

    // 4. 文档设置 JSON
    let settings_entry = if let Some(json) = &opts.settings_json {
        zip.add_stored(ORA_JSON_SETTINGS_ZIP_PATH, json.as_bytes())?;
        Some(ORA_JSON_SETTINGS_ZIP_PATH)
    } else {
        None
    };

    // 5. 缩略图（256×256 上限；整栈合成）
    let thumb = render_thumbnail(layers, bbox);
    let tmp_thumb = temp_sibling(filename.as_ref(), "thumb.png");
    pigment_surface::png::write_rgba8_png(&tmp_thumb, thumb.0.clone(), thumb.1, thumb.2)
        .map_err(FileError::Png)?;
    let bytes = std::fs::read(&tmp_thumb)?;
    let _ = std::fs::remove_file(&tmp_thumb);
    zip.add_stored("Thumbnails/thumbnail.png", &bytes)?;

    // 6. mergedimage.png —— 整栈摊平、无 alpha（源码 `alpha=False, background=True`）
    let mut merged = vec![0u8; fw * fh * 4];
    layers.render_rect_rgba8((bx, by, fw as u32, fh as u32), false, &mut merged);
    let tmp_merged = temp_sibling(filename.as_ref(), "merged.png");
    pigment_surface::png::write_rgba8_png(&tmp_merged, merged, fw as u32, fh as u32)
        .map_err(FileError::Png)?;
    let bytes = std::fs::read(&tmp_merged)?;
    let _ = std::fs::remove_file(&tmp_merged);
    zip.add_stored("mergedimage.png", &bytes)?;

    // 7. stack.xml
    let xml = build_stack_xml(
        bw,
        bh,
        opts.xres,
        opts.yres,
        opts.frame_active,
        settings_entry,
        &layer_elems,
        Some(&bg_elem),
    );
    zip.add_stored("stack.xml", xml.as_bytes())?;

    zip.finish()?;
    Ok(OraSaved { bbox, thumbnail: thumb })
}

/// 图层自身的数据外接框（tile 对齐；`SurfaceBackedLayer.get_bbox` 的平栈版）。
fn surface_tile_bbox(s: &PigmentSurface) -> (i32, i32, i32, i32) {
    let ts = TILE_SIZE as i32;
    let mut it = s.tiles().map(|(c, _)| c);
    let Some(first) = it.next() else {
        return (0, 0, 0, 0);
    };
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for (tx, ty) in it {
        x0 = x0.min(tx);
        y0 = y0.min(ty);
        x1 = x1.max(tx);
        y1 = y1.max(ty);
    }
    (x0 * ts, y0 * ts, (x1 - x0 + 1) * ts, (y1 - y0 + 1) * ts)
}

/// 临时文件路径：与目标同目录（保证同一文件系统），名字带唯一后缀。
fn temp_sibling(target: &Path, tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ora".to_string());
    dir.join(format!(".{}-{}-{}-{}", stem, tag, std::process::id(), n))
}

// ---- 载入 ----

/// `load_png` 的结果（对应 `load_layer_from_png` 的返回值 + `set_frame`）。
pub struct PngLoaded {
    /// 载入内容的矩形（`load_png` 会把它设成画框，`:1736`）。
    pub bbox: (i32, i32, i32, i32),
}

/// 载入 PNG（照搬 `lib/document.py:1732` `load_png`）。
///
/// 与源码一致：**先 `clear()`**，再从原点载入单层内容，最后把内容外接框设为画框。
/// 调用方负责 `clear()`（本函数接收已清空的表面）。
pub fn load_png(
    surface: &mut PigmentSurface,
    filename: impl AsRef<Path>,
) -> Result<PngLoaded, FileError> {
    if !filename.as_ref().is_file() {
        return Err(FileError::NotFound(filename.as_ref().display().to_string()));
    }
    let bbox = surface
        .load_from_png(filename.as_ref(), 0, 0)
        .map_err(FileError::Png)?;
    Ok(PngLoaded { bbox })
}

/// 单个载入图层的数据（`_layer_new_from_orazip` 的产物 + 通用 flags，
/// flags 解析照 `core.py:167-178` `_load_common_flags_from_ora_elem`）。
pub struct OraLayerData {
    pub surface: PigmentSurface,
    pub name: Option<String>,
    pub mode: CombineMode,
    pub opacity: f32,
    pub visible: bool,
    pub locked: bool,
    pub initially_selected: bool,
}

impl std::fmt::Debug for OraLayerData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OraLayerData")
            .field("name", &self.name)
            .field("mode", &self.mode)
            .field("opacity", &self.opacity)
            .field("visible", &self.visible)
            .field("locked", &self.locked)
            .field("initially_selected", &self.initially_selected)
            .field("tiles", &self.surface.tile_count())
            .finish()
    }
}

/// `load_ora` 的结果（源码里副作用落在 `layer_stack` 与文档属性上）。
#[derive(Debug)]
#[allow(missing_docs)]
pub struct OraLoaded {
    pub bbox: (i32, i32, i32, i32),
    pub xres: Option<u32>,
    pub yres: Option<u32>,
    pub frame_enabled: bool,
    pub settings_json: Option<String>,
    pub mypaint_version: Option<String>,
    /// 目标版本对当前实现的兼容度（对应 `_compat_check`，`:1788`）。
    pub compatibility: Option<Compatibility>,
    /// 自顶向下的图层（index 0 = 最顶层；XML 文档序 = `_layers` 序，
    /// `group.py:124-131` 按文档序 append 回栈顶）。
    pub layers: Vec<OraLayerData>,
    /// 背景色（文件里没有可用背景 tile 时保持默认白）。
    pub background_color: [u8; 3],
    /// 文件里是否读到了可用的背景 tile（`tree.py:2395-2420`）。
    pub background_found: bool,
    /// 载入后应选中的层（`_set_current_path_after_ora_load`：先找
    /// `selected` 标记的最上层，否则第 0 层）。
    pub current_index: usize,
}

/// 载入 OpenRaster（照搬 `lib/document.py:1804` `load_ora`）。
///
/// 调用方负责 `clear()`。与源码一致：
/// - 读 `stack.xml`，取 `image` 的 `w/h/xres/yres` 与 MyPaint 命名空间属性
/// - 子元素按文档序解析（第一子元素 = 最顶层，`group.py` 的 append 语义）
/// - `mypaint:background-tile` / legacy `background_tile` → 背景色；
///   tile 非纯色时按普通图层处理（`BackgroundError` 的 fallthrough）
/// - 缺 strokemap 不报错（有就读、没有就跳过）
pub fn load_ora(filename: impl AsRef<Path>) -> Result<OraLoaded, FileError> {
    if !filename.as_ref().is_file() {
        return Err(FileError::NotFound(filename.as_ref().display().to_string()));
    }
    let z = ZipArchive::open(filename.as_ref())?;

    // `logger.debug("mimetype: %r", orazip.read("mimetype").strip())`（`:1811`）
    // 源码不去校验它，但缺 mimetype 就不是 ORA；我们只做存在性提示，不拒绝
    // （与源码的宽容态度一致 —— 它的读取完全靠 stack.xml）。
    let xml_bytes = z
        .read("stack.xml")
        .map_err(|_| FileError::Malformed("ORA 缺少 stack.xml".to_string()))?;
    let xml = String::from_utf8_lossy(&xml_bytes).into_owned();

    let img = parse_stack_xml(&xml)?;

    let image_w = img.width.max(0);
    let image_h = img.height.max(0);

    // 分辨率：源码 `if image_xres and image_yres`（`:1845`）
    let xres = if img.xres > 0 { Some(img.xres as u32) } else { None };
    let yres = if img.yres > 0 { Some(img.yres as u32) } else { None };

    // 设置 JSON（`:1869`）：它可能指向一个 zip 条目
    let settings_json = img
        .json_settings
        .as_ref()
        .and_then(|entry| z.read(entry).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned());

    // 子元素按文档序 → index 0 = 最顶层（见 OraLoaded::layers 注释）
    let mut out_layers: Vec<OraLayerData> = Vec::new();
    let mut background_color = [255u8, 255, 255];
    let mut background_found = false;
    for l in &img.layers {
        // 背景判定（`tree.py:2395-2420`）：只允许一个背景
        if let Some(bg_src) = &l.bg_tile {
            if let Ok(png) = z.read(bg_src) {
                match uniform_tile_color(&png) {
                    Some(rgb) => {
                        if !background_found {
                            background_color = rgb;
                            background_found = true;
                            continue;
                        }
                    }
                    // `BackgroundError` → 警告并按普通图层继续（源码同）
                    None => {}
                }
            }
        }
        let Some(src) = &l.src else { continue };
        let png = z
            .read(src)
            .map_err(|e| FileError::Malformed(format!("读图层数据 {src:?} 失败：{e}")))?;
        let (px, w, h) = pigment_surface::png::decode_rgba8_png(&png).map_err(FileError::Png)?;
        let mut surface = PigmentSurface::new();
        surface.load_from_rgba8(&px, l.x, l.y, w, h);
        let mode = l
            .composite_op
            .as_deref()
            .and_then(CombineMode::from_ora_name)
            .unwrap_or(DEFAULT_MODE);
        out_layers.push(OraLayerData {
            surface,
            name: l.name.clone().filter(|s| !s.is_empty()),
            mode,
            opacity: l.opacity.unwrap_or(1.0).clamp(0.0, 1.0),
            visible: l.visible.unwrap_or(true),
            locked: l.locked,
            initially_selected: l.selected,
        });
    }
    if out_layers.is_empty() {
        // `_set_current_path_after_ora_load` 的 num_loaded==0 分支：
        // 「Adding an empty painting layer」（`CREATE_PAINTING_LAYER_IF_EMPTY`）
        out_layers.push(OraLayerData {
            surface: PigmentSurface::new(),
            name: None,
            mode: DEFAULT_MODE,
            opacity: 1.0,
            visible: true,
            locked: false,
            initially_selected: false,
        });
    }
    let current_index = out_layers
        .iter()
        .position(|l| l.initially_selected)
        .unwrap_or(0);

    Ok(OraLoaded {
        bbox: (0, 0, image_w, image_h),
        xres,
        yres,
        frame_enabled: img.frame_active,
        settings_json,
        mypaint_version: img.mypaint_version.clone(),
        compatibility: img.mypaint_version.as_deref().and_then(compatibility),
        layers: out_layers,
        background_color,
        background_found,
        current_index,
    })
}

/// 判断一块 PNG 是否是**纯色不透明** tile（`tiledsurface.BackgroundError`
/// 的语义：背景 tile 必须能用单色表示），返回 `(r, g, b)`。
fn uniform_tile_color(png: &[u8]) -> Option<[u8; 3]> {
    let (px, w, h) = pigment_surface::png::decode_rgba8_png(png).ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    let first = [px[0], px[1], px[2]];
    if px[3] != 255 {
        return None;
    }
    for p in px.chunks_exact(4) {
        if p[3] != 255 || p[0] != first[0] || p[1] != first[1] || p[2] != first[2] {
            return None;
        }
    }
    Some(first)
}

// ---- stack.xml 解析 ----

/// 解析出的 `layer` 元素（含全部通用 flags；`<stack>` 元素的 layer 型子元素
/// 也按文档序收集进来 —— 组嵌套已记账为后续小项，B2 平栈处理）。
#[derive(Debug, Clone, Default)]
pub struct XmlLayer {
    pub src: Option<String>,
    pub name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub opacity: Option<f32>,
    pub visible: Option<bool>,
    pub locked: bool,
    pub selected: bool,
    pub composite_op: Option<String>,
    /// `mypaint:background-tile` 或 legacy `background_tile`。
    pub bg_tile: Option<String>,
}

/// 解析出的 `image` 元素（只取本实现用得到的字段）。
#[derive(Debug, Clone, Default)]
pub struct XmlImage {
    pub width: i32,
    pub height: i32,
    pub xres: i32,
    pub yres: i32,
    pub frame_active: bool,
    pub mypaint_version: Option<String>,
    pub json_settings: Option<String>,
    /// 文档序（第一个 = 最顶层）。
    pub layers: Vec<XmlLayer>,
}

/// 解析 `stack.xml`。
///
/// 只用 `quick-xml` 的事件流做浅层遍历：`image` 的属性 + 全部 `layer` 元素
/// （含嵌套在 `stack` 里的，按文档序 —— 即自顶向下）。
///
/// 注意 `quick-xml` **不会**在 EOF 处对未闭合的元素报错（实测
/// `<image><unclosed>` 会安静地以 `Eof` 收尾），所以这里自己维护开放元素
/// 深度，顺便要求必须真出现过 `image` —— 否则一个 `stack.xml` 被截断
/// 也能"解析成功"，随后报出的会是让人费解的「没有可载入的图层」。
pub fn parse_stack_xml(xml: &str) -> Result<XmlImage, FileError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut img = XmlImage::default();
    let mut saw_image = false;
    let mut depth = 0i32;
    let mut open_tags: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let tag = String::from_utf8_lossy(name.as_ref()).into_owned();
                let attrs = collect_attrs(&e);
                apply_element(&mut img, &tag, &attrs, &mut saw_image);
                depth += 1;
                open_tags.push(tag);
            }
            Ok(Event::Empty(e)) => {
                let name = e.name();
                let tag = String::from_utf8_lossy(name.as_ref()).into_owned();
                let attrs = collect_attrs(&e);
                apply_element(&mut img, &tag, &attrs, &mut saw_image);
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                open_tags.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(FileError::Malformed(format!("stack.xml 解析失败：{e}")));
            }
            _ => {}
        }
        buf.clear();
    }
    if depth != 0 {
        return Err(FileError::Malformed(format!(
            "stack.xml 结构不完整（有未闭合的元素：{}）",
            open_tags.join(",")
        )));
    }
    if !saw_image {
        return Err(FileError::Malformed(
            "stack.xml 里没有 <image> 元素".to_string(),
        ));
    }
    Ok(img)
}

/// 取出一个元素的所有属性（名字原样保留命名空间前缀）。
fn collect_attrs(e: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    for a in e.attributes().flatten() {
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let v = a
            .unescape_value()
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
        attrs.push((k, v));
    }
    attrs
}

/// 把 `<image>` / `<layer>` 的属性并进结果。
fn apply_element(
    img: &mut XmlImage,
    tag: &str,
    attrs: &[(String, String)],
    saw_image: &mut bool,
) {
    let get = |k: &str| attrs.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    let get_i32 = |k: &str| {
        get(k)
            .and_then(|v| v.trim().parse::<i32>().ok())
            .unwrap_or(0)
    };
    match tag {
        "image" => {
            *saw_image = true;
            img.width = get_i32("w");
            img.height = get_i32("h");
            img.xres = get_i32("xres");
            img.yres = get_i32("yres");
            img.frame_active = get(ATTR_NS_FRAME_ACTIVE)
                .or_else(|| get("frame-active"))
                .map(|v| xsd2bool(&v))
                .unwrap_or(false);
            img.mypaint_version = get(ATTR_NS_VERSION).or_else(|| get("version"));
            img.json_settings = get(ATTR_NS_JSON_SETTINGS).or_else(|| get("json-settings"));
        }
        "layer" => {
            let opacity = get("opacity").and_then(|v| v.trim().parse::<f32>().ok());
            let visible = get("visibility").map(|v| v.to_ascii_lowercase() != "hidden");
            let locked = get("edit-locked").map(|v| xsd2bool(&v)).unwrap_or(false);
            let selected = get("selected").map(|v| xsd2bool(&v)).unwrap_or(false);
            let bg_tile = get(ATTR_NS_BG_TILE).or_else(|| get(ATTR_BG_TILE_LEGACY));
            img.layers.push(XmlLayer {
                src: get("src"),
                name: get("name"),
                x: get_i32("x"),
                y: get_i32("y"),
                opacity,
                visible,
                locked,
                selected,
                composite_op: get("composite-op"),
                bg_tile,
            });
        }
        _ => {} // stack / 其他元素本轮忽略（组嵌套已记账）
    }
}

/// `lib/xml.py:58` 的 `xsd2bool`。
pub fn xsd2bool(arg: &str) -> bool {
    matches!(arg.to_ascii_lowercase().as_str(), "true" | "1")
}

// ---- 按扩展名分派 ----

/// 识别出的格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ora,
    Png,
    Jpg,
}

/// 按扩展名判断格式（照 `os.path.splitext` + `getattr(self, "save_"+ext)`）。
///
/// 返回 `None` 表示不支持的扩展名（对应 `Document._unsupported`）。
pub fn format_for_path(path: impl AsRef<Path>) -> Option<Format> {
    let ext = path
        .as_ref()
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())?;
    match ext.as_str() {
        "ora" => Some(Format::Ora),
        "png" => Some(Format::Png),
        "jpg" | "jpeg" => Some(Format::Jpg),
        _ => None,
    }
}

/// 未实现格式的提示（目前只有 jpg 会走到）。
pub fn unsupported_message(path: impl AsRef<Path>) -> FileError {
    let ext = path
        .as_ref()
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
        FileError::Unsupported(format!(
            "{ext}（有损格式，MyPaint 走 GdkPixbuf，本实现尚未移植）"
        ))
    } else {
        FileError::Unsupported(ext)
    }
}

/// 便捷：让 `Document` 用的画布底色（测试里也要用）。
pub fn canvas_bg() -> [u8; 3] {
    CANVAS_BG
}

/// 便捷：把 `Color32` 之外的地方也要的 EOTF 常量转出来。
pub fn eotf_value() -> f32 {
    color::EOTF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_refname_matches_mypaint() {
        // path=(0,) 的单图层
        assert_eq!(make_refname("layer", &[0], ".png"), "layer-00.png");
        // path=(0,0) 的组内子层（fill_outlines.ora 里就是这个）
        assert_eq!(make_refname("layer", &[0, 0], ".png"), "layer-00-00.png");
        // suffix 不带点时要补 '-'
        assert_eq!(
            make_refname("layer", &[0, 0], "strokemap.dat"),
            "layer-00-00-strokemap.dat"
        );
        // 背景层
        assert_eq!(make_refname("background", &[1], ".png"), "background-01.png");
        // 空路径（源码里的边界情形：`"layer" + "-" + "" + ".png"`）
        assert_eq!(make_refname("layer", &[], ".png"), "layer-.png");
        // 两位以上要大写补零，不能截断
        assert_eq!(make_refname("layer", &[123], ".png"), "layer-123.png");
    }

    #[test]
    fn version_parsing_matches_source_doctests() {
        // 非法输入（源码 doctest 的四个反例）
        assert_eq!(parse_version_string("1.0.2.3-alpha"), None);
        assert_eq!(parse_version_string("2.0-beta"), None);
        assert_eq!(parse_version_string("2.0.3-gamma"), None);
        assert_eq!(parse_version_string("1.-2.3"), None);
        // 合法输入
        assert_eq!(
            parse_version_string("1.0.2-alpha"),
            Some((1, 0, 2, "-alpha".to_string()))
        );
        assert_eq!(parse_version_string("3.1.5"), Some((3, 1, 5, String::new())));
        assert_eq!(
            parse_version_string("2.0.1-alpha"),
            Some((2, 0, 1, "-alpha".to_string()))
        );
        // 预发布号被剥掉
        assert_eq!(
            parse_version_string("2.2.1-rc.7"),
            Some((2, 2, 1, "-rc".to_string()))
        );
    }

    #[test]
    fn compatibility_matches_source_doctests() {
        use Compatibility::*;
        assert_eq!(compatibility("1.0.0"), Some(Fully));
        assert_eq!(compatibility("1.0.0-alpha"), Some(Fully));
        // 目标主版本更高 → 不兼容
        assert_eq!(compatibility("3.0.0"), Some(Incompatible));
        // 目标次版本更高（当前实现是 2.0.0）→ 部分兼容
        assert_eq!(compatibility("2.5.0"), Some(Partially));
        // 目标次版本更低 → 完全兼容
        assert_eq!(compatibility("2.0.0"), Some(Fully));
        // 非法串 → None（调用方据此放行）
        assert_eq!(compatibility("not-a-version"), None);
    }

    /// 我们写的版本号不能被自己判成"部分兼容"（否则会向用户弹警告）。
    #[test]
    fn our_own_version_is_fully_compatible() {
        assert_eq!(compatibility(MYPAINT_VERSION), Some(Compatibility::Fully));
    }

    /// 浮点属性值要像 CPython 的 `str(float)`：1.0 写成 `"1.0"` 而不是 `"1"`。
    #[test]
    fn float_attrs_match_python_str_float() {
        assert_eq!(fmt_float_py(1.0), "1.0");
        assert_eq!(fmt_float_py(0.0), "0.0");
        assert_eq!(fmt_float_py(2.0), "2.0");
        assert_eq!(fmt_float_py(0.517), "0.517");
        assert_eq!(fmt_float_py(0.5), "0.5");
    }

    /// 测试辅助：构造一个 `<layer>` 的属性向量（按 save_ora 的写法）。
    fn test_layer_attrs(
        src: &str,
        x: i32,
        y: i32,
        opacity: f32,
        selected: bool,
        visible: bool,
        compop: &str,
        name: Option<&str>,
    ) -> Vec<(&'static str, String)> {
        let mut attrs: Vec<(&'static str, String)> = vec![
            ("x", x.to_string()),
            ("y", y.to_string()),
            ("opacity", fmt_float_py(opacity)),
            (
                "visibility",
                if visible { "visible" } else { "hidden" }.to_string(),
            ),
            ("composite-op", compop.to_string()),
            ("src", src.to_string()),
        ];
        if let Some(n) = name {
            if !n.is_empty() {
                attrs.push(("name", n.to_string()));
            }
        }
        if selected {
            attrs.push(("selected", "true".to_string()));
        }
        attrs
    }

    fn test_bg_attrs() -> Vec<(&'static str, String)> {
        vec![
            ("x", "0".to_string()),
            ("y", "0".to_string()),
            ("opacity", fmt_float_py(1.0)),
            ("visibility", "visible".to_string()),
            ("composite-op", "mypaint:spectral-wgm".to_string()),
            ("name", "background".to_string()),
            ("src", "data/background-01.png".to_string()),
            (ATTR_NS_BG_TILE, "data/background-01-tile.png".to_string()),
        ]
    }

    /// 写出的 layer/stack opacity 必须是 `"1.0"`（与真实 MyPaint 文件一致）。
    #[test]
    fn stack_xml_writes_opacity_as_python_float() {
        let layer = test_layer_attrs("s", 0, 0, 1.0, true, true, "svg:src-over", None);
        let xml = build_stack_xml(1, 1, None, None, false, None, &[layer], None);
        assert!(xml.contains("opacity=\"1.0\""), "opacity 应写成 1.0：{xml}");
        assert!(!xml.contains("opacity=\"1\""), "不该出现 Python 不会写的 \"1\"：{xml}");
    }

    #[test]
    fn xsd2bool_matches_source() {
        assert!(xsd2bool("true"));
        assert!(xsd2bool("True"));
        assert!(xsd2bool("1"));
        assert!(!xsd2bool("false"));
        assert!(!xsd2bool("0"));
        assert!(!xsd2bool(""));
    }

    #[test]
    fn format_detection_is_case_insensitive() {
        assert_eq!(format_for_path("a.ora"), Some(Format::Ora));
        assert_eq!(format_for_path("a.ORA"), Some(Format::Ora));
        assert_eq!(format_for_path("a.PNG"), Some(Format::Png));
        assert_eq!(format_for_path("a.jpeg"), Some(Format::Jpg));
        assert_eq!(format_for_path("a.txt"), None);
        assert_eq!(format_for_path("noext"), None);
    }

    /// stack.xml 的字节格式必须与真实样本一致：单引号声明、2 空格缩进、
    /// 空元素 `<x />`、无命名空间属性在前、组内按局部名排序。
    #[test]
    fn stack_xml_matches_real_sample_layout() {
        let layer = test_layer_attrs("data/layer-00.png", 0, 0, 1.0, true, true, "svg:src-over", None);
        let xml = build_stack_xml(
            128,
            128,
            None,
            None,
            false,
            Some(ORA_JSON_SETTINGS_ZIP_PATH),
            &[layer],
            Some(&test_bg_attrs()),
        );
        assert!(
            xml.starts_with("<?xml version='1.0' encoding='UTF-8'?>\n"),
            "声明必须用单引号且紧跟换行：{xml:?}"
        );
        assert!(xml.ends_with("</image>\n"), "必须以 </image> 加换行收尾");
        assert!(xml.contains("\n  <stack "), "stack 缩进 2 空格");
        assert!(xml.contains("\n    <layer "), "layer 缩进 4 空格");
        assert!(xml.contains(" />\n"), "空元素写成 <x />");
        assert!(xml.contains("xmlns:mypaint=\"http://mypaint.org/ns/openraster\""));

        // image 属性顺序：无命名空间的 h/version/w 先（按局部名字典序），
        // 再是 mypaint:* —— 与 fill_outlines.ora 实测顺序一致
        let line = xml.lines().nth(1).unwrap();
        let h = line.find("h=\"").unwrap();
        let ver = line.find("version=\"").unwrap();
        let w = line.find("w=\"").unwrap();
        let fa = line.find(ATTR_NS_FRAME_ACTIVE).unwrap();
        assert!(h < ver && ver < w && w < fa, "属性顺序不符：{line}");

        // layer 属性：composite-op < opacity < selected < src=" < visibility < x= < y=
        // （必须用 ` key="` 带前导空格匹配：`y="` 会命中 `visibility="` 的子串）
        let ll = xml.lines().find(|l| l.contains("<layer ")).unwrap();
        let at = |k: &str| ll.find(&format!(" {k}=\"")).unwrap();
        for (a, b) in [
            ("composite-op", "opacity"),
            ("opacity", "selected"),
            ("selected", "src"),
            ("src", "visibility"),
            ("visibility", "x"),
            ("x", "y"),
        ] {
            assert!(at(a) < at(b), "{a} 应排在 {b} 之前：{ll}");
        }
    }

    #[test]
    fn stack_xml_escapes_attribute_values() {
        let layer = test_layer_attrs(
            "data/a&b.png",
            0,
            0,
            1.0,
            false,
            true,
            "svg:src-over",
            Some("a <b> \"c\""),
        );
        let xml = build_stack_xml(1, 1, None, None, false, None, &[layer], None);
        assert!(xml.contains("data/a&amp;b.png"));
        assert!(xml.contains("name=\"a &lt;b&gt; &quot;c&quot;\""));
    }

    #[test]
    fn stack_xml_omits_xres_when_zero() {
        let mk = |x: Option<u32>, y: Option<u32>| {
            let layer = test_layer_attrs("s", 0, 0, 1.0, true, true, "svg:src-over", None);
            build_stack_xml(1, 1, x, y, false, None, &[layer], None)
        };
        let xml = mk(Some(0), Some(0));
        assert!(!xml.contains("xres"), "xres=0 时不该写出（源码 `if xres and yres`）");
        let xml2 = mk(Some(72), Some(72));
        assert!(xml2.contains("xres=\"72\""));
        assert!(xml2.contains("yres=\"72\""));
    }

    #[test]
    fn parse_round_trips_our_own_xml() {
        let layer = test_layer_attrs(
            "data/layer-00.png",
            64,
            128,
            1.0,
            true,
            true,
            "svg:src-over",
            Some("Layer 1"),
        );
        let xml = build_stack_xml(
            640,
            480,
            Some(300),
            Some(300),
            true,
            Some(ORA_JSON_SETTINGS_ZIP_PATH),
            &[layer],
            Some(&test_bg_attrs()),
        );
        let img = parse_stack_xml(&xml).unwrap();
        assert_eq!((img.width, img.height), (640, 480));
        assert_eq!((img.xres, img.yres), (300, 300));
        assert!(img.frame_active);
        assert_eq!(img.mypaint_version.as_deref(), Some(MYPAINT_VERSION));
        assert_eq!(img.json_settings.as_deref(), Some(ORA_JSON_SETTINGS_ZIP_PATH));
        // 图层 + 背景层
        assert_eq!(img.layers.len(), 2);
        assert_eq!(img.layers[0].src.as_deref(), Some("data/layer-00.png"));
        assert_eq!((img.layers[0].x, img.layers[0].y), (64, 128));
        assert_eq!(img.layers[0].name.as_deref(), Some("Layer 1"));
        assert!(img.layers[0].selected);
        // 背景元素在最后，带背景 tile 属性
        assert_eq!(
            img.layers[1].bg_tile.as_deref(),
            Some("data/background-01-tile.png")
        );
        assert_eq!(img.layers[1].name.as_deref(), Some("background"));
    }

    /// 解析真实的 MyPaint 样本（`smallimage.ora` 的 stack.xml）。
    #[test]
    fn parses_real_mypaint_stack_xml() {
        let xml = r#"<?xml version='1.0' encoding='UTF-8'?>
<image h="128" w="128">
  <stack>
    <layer mypaint_strokemap="data/layer000_strokemap.dat" opacity="0.517" src="data/layer000.png" x="0" y="0" />
    <layer mypaint_strokemap="data/layer001_strokemap.dat" opacity="1.0" src="data/layer001.png" x="0" y="0" />
    <layer background_tile="data/background_tile.png" name="background" opacity="1.0" src="data/background.png" x="0" y="0" />
  </stack>
</image>
"#;
        let img = parse_stack_xml(xml).unwrap();
        assert_eq!((img.width, img.height), (128, 128));
        assert_eq!(img.layers.len(), 3);
        assert_eq!(img.layers[0].src.as_deref(), Some("data/layer000.png"));
        assert_eq!(img.layers[2].name.as_deref(), Some("background"));
        // 老文件没有 eotf/version 属性
        assert!(img.mypaint_version.is_none());
        assert!(!img.frame_active);
    }

    /// 解析 `fill_outlines.ora` 风格：带命名空间属性与分组。
    #[test]
    fn parses_namespaced_mypaint_attrs() {
        let xml = r#"<?xml version='1.0' encoding='UTF-8'?>
<image xmlns:mypaint="http://mypaint.org/ns/openraster" h="3008" version="0.0.5" w="3456" mypaint:frame-active="false" mypaint:json-settings="data/mypaint-settings.json">
  <stack composite-op="svg:src-over" isolation="isolate" name="Root" opacity="1.0" visibility="visible">
    <stack composite-op="svg:src-over" isolation="isolate" name="G" opacity="1.0" visibility="hidden">
      <layer composite-op="svg:src-over" name="L" opacity="1.0" src="data/layer-00-00.png" visibility="visible" x="1664" y="1088" />
    </stack>
  </stack>
</image>
"#;
        let img = parse_stack_xml(xml).unwrap();
        assert_eq!((img.width, img.height), (3456, 3008));
        assert!(!img.frame_active);
        assert_eq!(
            img.json_settings.as_deref(),
            Some("data/mypaint-settings.json")
        );
        assert_eq!(img.layers.len(), 1, "嵌套在 stack 里的 layer 也要收到");
        assert_eq!((img.layers[0].x, img.layers[0].y), (1664, 1088));
    }

    #[test]
    fn malformed_xml_reports_error() {
        assert!(parse_stack_xml("<image><unclosed>").is_err());
    }

    /// 属性排序：先无命名空间（按局部名），再带命名空间（按局部名）。
    #[test]
    fn attr_sort_puts_unprefixed_first() {
        let v = sorted_attrs(&[
            (ATTR_NS_VERSION, "2.0.0".into()),
            ("w", "10".into()),
            (ATTR_NS_EOTF, "2.2".into()),
            ("h", "5".into()),
        ]);
        let names: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, vec!["h", "w", ATTR_NS_EOTF, ATTR_NS_VERSION]);
    }

    /// 空表面的缩略图退化成 1×1，不 panic。
    #[test]
    fn thumbnail_of_empty_surface_is_one_pixel() {
        let s = PigmentSurface::new();
        let st = LayerStack::new();
        let (px, w, h) = render_thumbnail(&st, (0, 0, 0, 0));
        assert_eq!((w, h), (1, 1));
        assert_eq!(px.len(), 4);
    }

    /// 大画幅的缩略图按 2 的幂次减半 —— 3456×3008 → 216×188
    /// （与 `fill_outlines.ora` 实测一致）。
    #[test]
    fn thumbnail_halves_like_mypaint() {
        let s = PigmentSurface::new();
        let st = LayerStack::new();
        let (_, w, h) = render_thumbnail(&st, (0, 0, 3456, 3008));
        assert_eq!((w, h), (216, 188), "应与 fill_outlines.ora 的缩略图尺寸一致");
    }

    /// 小画幅不缩放：64×64 → 64×64（`scale_proportionally` 是 shrink_only）。
    #[test]
    fn small_thumbnail_is_not_upscaled() {
        let s = PigmentSurface::new();
        let st = LayerStack::new();
        let (_, w, h) = render_thumbnail(&st, (0, 0, 64, 64));
        assert_eq!((w, h), (64, 64), "只缩不放");
    }
}
