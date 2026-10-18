//! 调色板模型 —— `lib/palette.py` 的全量移植（B4-4a）。
//!
//! ## 源码里这块是什么
//!
//! `Palette`（`lib/palette.py:36-859`）是一个**扁平的颜色格子列表**，每个格子
//! 要么是一支带名字的颜色（内部一律折成 `RGBColor`），要么是**空槽**。
//! 它还记住"当前色落在哪个格子上"（`_match_position`）以及这次命中是精确还是
//! 近似（`_match_is_approx`），供 GUI 高亮用。
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `palette.py:54-56` `_EMPTY_SLOT_ITEM` / `_EMPTY_SLOT_NAME` | [`EMPTY_SLOT_RGB`] / [`EMPTY_SLOT_NAME`] |
//! | `palette.py:60-104` `__init__` | [`Palette::new`] / [`Palette::from_colors`] / [`Palette::load_str`] |
//! | `palette.py:106-130` `clear()` | [`Palette::clear`] |
//! | `palette.py:132-196` `load()`（GIMP `.gpl` 解析） | [`Palette::load_str`] |
//! | `palette.py:198-222` `save()` / `__unicode__()` | [`Palette::to_gimp_palette_string`] |
//! | `palette.py:224-238` `update()` | [`Palette::update`] |
//! | `palette.py:242-274` 尺寸与元数据 | [`Palette::columns`] / [`Palette::set_columns`] / [`Palette::len`] |
//! | `palette.py:282-309` 命中标记 | [`Palette::match_position`] / [`Palette::set_match_position`] |
//! | `palette.py:311-399` `match_color()` | [`Palette::match_color`] |
//! | `palette.py:401-488` `move_match_position()` | [`Palette::move_match_position`] |
//! | `palette.py:499-519` `_copy_color_in/out()` | [`Palette::copy_color_in`] / [`Palette::copy_color_out`] |
//! | `palette.py:521-548` `append()` | [`Palette::append`] |
//! | `palette.py:550-577` `insert()` | [`Palette::insert`] |
//! | `palette.py:579-671` `reposition()` | [`Palette::reposition`] |
//! | `palette.py:673-690` `pop()` | [`Palette::pop`] |
//! | `palette.py:692-707` `get_color()` / `__getitem__` / `__setitem__` | [`Palette::get`] / [`Palette::set`] |
//! | `palette.py:711-743` 名字存取 | [`Palette::color_name`] / [`Palette::set_color_name`] / [`Palette::color_by_name`] |
//! | `palette.py:745-754` `iter_colors()` | [`Palette::iter_colors`] |
//! | `palette.py:776-827` 序列化 / 克隆 / `__repr__` | [`Palette::to_gimp_palette_string`] / `Clone` / [`Palette::repr`] |
//! | `palette.py:831-859` 简单字典（偏好用） | [`SimplePaletteDict`] |
//! | `palette.py:865-882` `_outwards_from()` | [`outwards_from`] |
//! | `palette.py:884-897` `_color_distance()` | [`color_distance`] |
//!
//! ## 没有对应物的三样东西（记账）
//!
//! 1. **`@event` 通知**（`palette.py:756-772` 的 `info_changed` /
//!    `match_changed` / `sequence_changed` / `color_changed`）：`lib/observable.py`
//!    的事件订阅是给 GTK 控件的 `queue_draw` 用的。egui 是立即模式，控件每帧
//!    直接读模型（与 B4-1 的 `ColorManager` 同一处置），所以这里只有**数据**
//!    变化，没有回调。
//! 2. **文件 IO**：`load()` 的 `filehandle` / `filename` 两条入口
//!    （`palette.py:100-104`）与 `save(filehandle)`（`:198-222`）收在这里是
//!    **字符串进、字符串出**（[`Palette::load_str`] /
//!    [`Palette::to_gimp_palette_string`]）；打开/保存对话框与路径解析留在
//!    `pigment-app`（`gui/colors/paletteview.py:1288-1406` 的
//!    `palette_load_via_dialog` / `palette_save_via_dialog` 本身就是 GTK 的
//!    `FileChooserDialog`，本块未移植，见交付记账）。
//! 3. **`None` 作索引**：源码的 `get_color(None)` / `insert(None, …)` /
//!    `reposition(src, None)` 用 `None` 表示"末尾"，这里用 `Option<usize>` 表达。
//!
//! ## 源码里几处会抛异常的分支（本移植不照抄崩溃）
//!
//! - `pop()`（`palette.py:685-688`）：`self.match_position > i` 在
//!   `match_position is None` 时是 `None > int`，Python 3 抛 `TypeError`。
//!   这里当作"没有命中就不必调整"跳过（见 [`Palette::pop`]）。
//! - `get_color_by_name()`（`palette.py:741-743`）：`for col in self` 迭代的是
//!   `iter_colors()`，空槽会 `yield None`，随后 `col.__name` 抛
//!   `AttributeError`。这里跳过空槽（见 [`Palette::color_by_name`]）。
//! - `reposition()` 里 `self._colors[targ_i]` 越界会抛 `IndexError`
//!   （`palette.py:623`，不在 `try` 内）；这里按无操作处理。
//! - `__setitem__` / `__getitem__` 的负下标（Python 从末尾数）：本移植一律用
//!   `usize`，负下标在应用层不会出现（`get_index_at_pos()` 只给非负值或 `None`）。
//!
//! ## 门槛
//!
//! `tests/color_palette.rs` 拿 `tests/fixtures/palette_vectors.txt` 逐位比对。
//! fixture 由 `tests/tools/gen-palette-vectors.py` 生成 —— 那个脚本
//! **真的 `import lib/palette.py`**（只把拖进 C 扩展的 `lib.helpers` 换成一个
//! 装真 `clamp()` 的替身），执行的是源码本体，不是照抄的公式。
//!
//! ## 默认调色板
//!
//! 源码没有 `DEFAULT_PALETTE` 常量：`ColorManager.__init__` 从
//! `prefs["colors.palette"]` 取，取不到就**读文件**
//! `<datapath>/palettes/MyPaint_Default.gpl`（`gui/colors/adjbases.py:139-147`，
//! 文件名常量在 `:48-50`）。这里把那个文件**逐字节**随库带上
//! （`assets/palettes/MyPaint_Default.gpl`，源文件头写明 CC0），启动时用同一个
//! [`Palette::load_str`] 解析 —— 于是解析器在启动路径上就被走了一遍。

use super::model::{Color, RgbColor, UIColor, YcbcrColor};

/// 空槽的哨兵颜色 —— `lib/palette.py:55`
/// `_EMPTY_SLOT_ITEM = RGBColor(-1, -1, -1)`。
///
/// 源码用 `is` 判等（身份），所以一个**真正的** `(-1,-1,-1)` 颜色不是空槽；
/// 但 `append(unique=True)` 里的 `==` 判等又会让两者相等（见
/// [`Palette::append`] 的注释）。两种比较在这里都如实保留。
pub const EMPTY_SLOT_RGB: RgbColor = RgbColor {
    r: -1.0,
    g: -1.0,
    b: -1.0,
};

/// 空槽在 `.gpl` 里的名字 —— `lib/palette.py:56` `_EMPTY_SLOT_NAME = "__NONE__"`。
pub const EMPTY_SLOT_NAME: &str = "__NONE__";

/// 近似命中的距离阈值 —— `lib/palette.py:376` 的 `d < 0.06`
/// （源码注释 `:383-390` 解释了 0.06 的来历）。
pub const MATCH_APPROX_DISTANCE: f64 = 0.06;

/// 偏好键：调色板的简单字典形式 —— `gui/colors/adjbases.py:48`
/// `PREFS_PALETTE_DICT_KEY = "colors.palette"`。
pub const PREFS_PALETTE_DICT_KEY: &str = "colors.palette";

/// 数据目录下的调色板子目录 —— `adjbases.py:49` `DATAPATH_PALETTES_SUBDIR`。
pub const DATAPATH_PALETTES_SUBDIR: &str = "palettes";

/// 默认调色板文件名 —— `adjbases.py:50` `DEFAULT_PALETTE_FILE`。
pub const DEFAULT_PALETTE_FILE: &str = "MyPaint_Default.gpl";

/// 默认调色板文件的**内容**（`_mypaint_ref/palettes/MyPaint_Default.gpl`
/// 的逐字节副本，CC0）。
///
/// 源码从 `<datapath>/palettes/` 读这个文件（`adjbases.py:143-146`）。本项目还没有
/// 数据目录这个概念（偏好系统属 B6），所以把文件随库带上、编译期内嵌。
pub const DEFAULT_PALETTE_GPL: &str = include_str!("../../assets/palettes/MyPaint_Default.gpl");

// ---------------------------------------------------------------------------
// 格子与错误
// ---------------------------------------------------------------------------

/// 一个非空格子：一支颜色 + 它的名字。
///
/// 源码把名字存在颜色对象的私有属性上（`palette.py:191` 的 `col.__name = …`；
/// 在类体里写 `__name` 会被 Python 改写成 `_Palette__name`，但类内所有读写
/// 都走同一个改写，语义就是"每支颜色带一个名字"）。Rust 里没有挂属性的地方，
/// 于是把"颜色 + 名字"显式做成一个结构体。
#[derive(Clone, Debug, PartialEq)]
pub struct Swatch {
    /// 颜色。**永远是 `RGBColor`** —— `_copy_color_in()` 会把任何 `UIColor`
    /// 折成 `RGBColor`（`palette.py:517` 的 `RGBColor(color=col)`）。
    pub color: RgbColor,
    /// 名字，可以为 `None`（匿名色）。
    pub name: Option<String>,
}

/// 载入 `.gpl` 时的错误 —— `palette.py:153-154` 的 `RuntimeError`
/// 与 `:172` 的 `int(value)` 失败。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteLoadError {
    /// 首行不是 `GIMP Palette` —— `palette.py:153-154`。
    NotAGimpPalette,
    /// `Columns:` 的值不是整数 —— `palette.py:172` 的 `int(value)` 抛
    /// `ValueError`。
    BadColumns(String),
}

impl std::fmt::Display for PaletteLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAGimpPalette => write!(f, "Not a valid GIMP Palette"),
            Self::BadColumns(v) => write!(f, "invalid literal for int() with base 10: {v:?}"),
        }
    }
}

impl std::error::Error for PaletteLoadError {}

/// 偏好的"简单字典"形式 —— `palette.py:831-859` 的 `to_simple_dict()` /
/// `new_from_simple_dict()`。
///
/// 源码是一个普通的 `dict`（键 `name` / `columns` / `entries`），`entries` 里
/// 每项要么是 `None`（空槽），要么是 `(hex 串, 名字或 None)` 二元组。
/// Rust 里用一个具名结构体表达，语义逐项对应；JSON 的编码/解码就是
/// [`SimplePaletteDict::to_json`] / [`SimplePaletteDict::from_json`]
/// （B6-1 接上偏好系统时补的，对应源码 `json.dumps`/`json.loads` 的那层）。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SimplePaletteDict {
    /// `simple["name"]`
    pub name: Option<String>,
    /// `simple["columns"]`
    pub columns: i64,
    /// `simple["entries"]`
    pub entries: Vec<Option<(String, Option<String>)>>,
}

impl SimplePaletteDict {
    /// 编码成 JSON —— `palette.py:831-844` 的 `to_simple_dict()` 结果
    /// 经 `json.dumps` 之后的形态。
    ///
    /// 空槽写 `null`，有颜色的格写 `[hex, 名字或 null]`（源码是 Python
    /// 二元组，JSON 里就是两元素数组）。
    pub fn to_json(&self) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|e| match e {
                None => serde_json::Value::Null,
                Some((hex, name)) => serde_json::json!([hex, name]),
            })
            .collect();
        serde_json::json!({
            "name": self.name,
            "columns": self.columns,
            "entries": entries,
        })
    }

    /// 从 JSON 解码 —— `palette.py:846-859` 的 `new_from_simple_dict()`。
    ///
    /// 源码逐项 `simple.get(key, 默认)`：缺 `name` → `None`、缺 `columns` →
    /// `set_columns(None)`（**会抛 TypeError**）、缺 `entries` → 空列表。
    /// 这里把"缺/类型不对"统一按保守值处理（`columns` 缺省 0 = 未指定），
    /// 不是对象则返回 `None`（调用方回落到默认调色板）。
    pub fn from_json(simple: &serde_json::Value) -> Option<Self> {
        let obj = simple.as_object()?;
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let columns = obj.get("columns").and_then(|v| v.as_i64()).unwrap_or(0);
        let mut entries = Vec::new();
        for entry in obj
            .get("entries")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            match entry {
                serde_json::Value::Null => entries.push(None),
                serde_json::Value::Array(pair) => {
                    let hex = pair.first().and_then(|v| v.as_str())?.to_owned();
                    let name = pair.get(1).and_then(|v| v.as_str()).map(str::to_owned);
                    entries.push(Some((hex, name)));
                }
                // 源码对非 None 的条目不检查形状，会在解包时抛异常；
                // 这里跳过坏条目（保守，见上）。
                _ => {}
            }
        }
        Some(Self {
            name,
            columns,
            entries,
        })
    }
}

// ---------------------------------------------------------------------------
// 辅助函数（`palette.py:862-897`）
// ---------------------------------------------------------------------------

/// 调色板内的搜索顺序，从给定下标向外扩散 —— `palette.py:865-882`
/// 的 `_outwards_from(n, i)`。
///
/// 逐行照抄，**包括它先 `yield i` 再在 `j = 0` 那一轮又 `yield i` 两次**的
/// 重复（源码的循环体没有跳过 `j = 0`）。重复项不影响 `match_color()` 的结果
/// （命中即刻 `break`），但顺序本身是源码行为，所以如实保留、由 fixture 固化。
///
/// `n` 是序列长度，`i` 必须满足 `0 <= i < n`（源码 `:870` 的 `assert`）。
pub fn outwards_from(n: usize, i: usize) -> Vec<usize> {
    assert!(i < n, "outwards_from() 要求 0 <= i < n（palette.py:870）");
    let mut out = Vec::with_capacity(2 * n + 1);
    out.push(i);
    for j in 0..n {
        let mut exhausted = true;
        if i >= j {
            out.push(i - j);
            exhausted = false;
        }
        if i + j < n {
            out.push(i + j);
            exhausted = false;
        }
        if exhausted {
            break;
        }
    }
    out
}

/// Python 的浮点幂 `x ** y`（`float_pow`：非特殊的指数直接落到 C 库的 `pow`）。
///
/// 为什么要这个而不是 `x * x` / `x.powi(2)`：CPython 的 `d**2` 走的是
/// `pow(d, 2.0)`，而 Rust 里写 `x * x` 得到的是**正确舍入的平方**，两者在某些
/// 输入上差 1 ulp（例：`0.6685491467074105**2` = `0.44695796156320666`，
/// 而 `x*x` = `0.4469579615632067`）。`_color_distance()` 的每一项都是 `**2`，
/// 逐位门禁要求一致。
///
/// `black_box` 的作用是**阻止 LLVM 把 `pow(x, 2.0)` 折成 `x*x`**
/// （那正是 1 ulp 差异的来源）。指数仍是那个常量、只是编译器看不见它。
#[inline]
fn py_pow(x: f64, y: f64) -> f64 {
    x.powf(std::hint::black_box(y))
}

/// 调色板匹配用的颜色距离 —— `palette.py:884-897` 的 `_color_distance()`。
///
/// 源码注释说要"几何 YCbCr 距离"，但表达式写的是
/// `((d_cb**2) + (d_cr**2) + (d_y)**2) ** (1.0 / 3)` —— **立方根**，不是平方根。
/// 照抄（距离只用于比较大小与 0.06 阈值，尺度不影响语义，但会影响门禁数值）。
pub fn color_distance(c1: &Color, c2: &Color) -> f64 {
    let a = YcbcrColor::from_color(c1);
    let b = YcbcrColor::from_color(c2);
    let d_cb = a.cb - b.cb;
    let d_cr = a.cr - b.cr;
    let d_y = a.y - b.y;
    // 注意结合顺序：Python 的左结合 = ((d_cb² + d_cr²) + d_y²)
    py_pow(
        py_pow(d_cb, 2.0) + py_pow(d_cr, 2.0) + py_pow(d_y, 2.0),
        1.0 / 3.0,
    )
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// 一个扁平的、可序列化成 GIMP 调色板的颜色格子列表
/// （`lib/palette.py:36-859` 的 `Palette`）。
///
/// 与 `Vec<RgbColor>` 的区别都在源码的语义里：
/// - 允许**空槽**（`Some`/`None`），GIMP 格式里写成 `0 0 0    __NONE__`；
/// - 每个格子带名字；
/// - 记住"当前色命中哪个格子"，并区分精确/近似。
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// 列数，0 = 自然流动 —— `palette.py:84` `self._columns = 0`。
    ///
    /// 用 `i64` 而不是 `usize`：源码的 `Columns:` 值经 `int()` 直接存下，
    /// 负数也存得下（下游一律按 `< 1` 当自然流动处理）。
    columns: i64,
    /// 格子列表 —— `palette.py:86` `self._colors = []`。
    entries: Vec<Option<Swatch>>,
    /// 调色板名字 —— `palette.py:88` `self._name = None`。
    name: Option<String>,
    /// 当前命中位置 —— `palette.py:90`，`None` = 没命中。
    match_position: Option<usize>,
    /// 当前命中是否只是近似 —— `palette.py:92`。
    match_is_approx: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

impl Palette {
    /// 空调色板 —— `Palette()`（`palette.py:60-104`，无参分支）。
    pub fn new() -> Self {
        Self {
            columns: 0,
            entries: Vec::new(),
            name: None,
            match_position: None,
            match_is_approx: false,
        }
    }

    /// 由一串颜色构造 —— `Palette(colors=…)`（`palette.py:96-99`）。
    ///
    /// 逐个走 `_copy_color_in()`，所以 HSV/HCY/YCbCr 都会被折成 `RGBColor`。
    pub fn from_colors<'a, I>(colors: I) -> Self
    where
        I: IntoIterator<Item = &'a Color>,
    {
        let mut p = Self::new();
        for col in colors {
            p.entries.push(Self::copy_color_in(Some(col), None));
        }
        p
    }

    /// 源码启动路径上的那一份调色板 —— `adjbases.py:139-147` 的
    /// "没有偏好就读 `<datapath>/palettes/MyPaint_Default.gpl`"。
    ///
    /// 这里解析的是内嵌的同一份文件内容（[`DEFAULT_PALETTE_GPL`]）。
    /// 文件是编译期常量、且有 `tests/color_palette.rs` 的真源码向量把守，
    /// 解析失败属于构建错误，所以 `expect`。
    pub fn mypaint_default() -> Self {
        let mut p = Self::new();
        p.load_str(DEFAULT_PALETTE_GPL)
            .expect("内置的 MyPaint_Default.gpl 必须能解析");
        p
    }

    // ---- 构造、载入、保存 ----

    /// 重置为初始状态 —— `palette.py:106-130` `clear()`。
    ///
    /// 源码默认会发三个事件（`silent=False`）；事件本移植不承接（见模块文档）。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.columns = 0;
        self.name = None;
        self.match_position = None;
        self.match_is_approx = false;
    }

    /// 从 GIMP 调色板文本载入 —— `palette.py:132-196` `load()`。
    ///
    /// 逐行对应源码的三条正则（`:147-149`）与四步流程：
    ///
    /// ```text
    /// comment_line_re = re.compile(r"^#")                                    # :147
    /// field_line_re   = re.compile(r"^(\w+)\s*:\s*(.*)$")                    # :148
    /// color_line_re   = re.compile(r"^(\d+)\s+(\d+)\s+(\d+)\s*(?:\b(.*))$")  # :149
    /// ```
    ///
    /// 与源码一致的几处细节：
    /// - 首行 `.strip()` 后必须正好是 `GIMP Palette`（`:153-154`）；
    /// - 每行先 `.strip()`，空行与 `#` 开头跳过（`:158-163`）；
    /// - **表头只认 `键:值`，且只到第一行不是键值对为止**（`:164-177`），之后
    ///   的行一律按颜色行解析；未知键只警告（`:174`）；
    /// - `Columns:` 的值用 `int()`，失败**抛出**（`:172`）；
    /// - 颜色行的 RGB 经 `clamp(int(x), 0, 0xFF) / 0xFF`（`:184-186`）；
    /// - `0 0 0    __NONE__` 是空槽（`:187-188`），其余都是带名字的格子
    ///   （`:190-192`，名字 `strip()` 过，可能是空串）。
    pub fn load_str(&mut self, text: &str) -> Result<(), PaletteLoadError> {
        self.clear();
        let mut lines = text.lines();
        let first = lines.next().unwrap_or("");
        if first.trim() != "GIMP Palette" {
            return Err(PaletteLoadError::NotAGimpPalette);
        }
        let mut header_done = false;
        for raw in lines {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            if !header_done {
                if let Some((key, value)) = parse_field_line(line) {
                    // 源码 `key.lower()`；`.gpl` 的键都是 ASCII
                    match key.to_ascii_lowercase().as_str() {
                        "name" => self.name = Some(value.trim().to_string()),
                        "columns" => {
                            self.columns = value
                                .trim()
                                .parse::<i64>()
                                .map_err(|_| PaletteLoadError::BadColumns(value.to_string()))?;
                        }
                        _ => {}
                    }
                    continue;
                }
                header_done = true;
            }
            let Some((r, g, b, col_name)) = parse_color_line(line) else {
                // `palette.py:179-181`：不匹配只警告，继续下一行
                continue;
            };
            let r = clamp_i64(r, 0, 0xFF) as f64 / 0xFF as f64;
            let g = clamp_i64(g, 0, 0xFF) as f64 / 0xFF as f64;
            let b = clamp_i64(b, 0, 0xFF) as f64 / 0xFF as f64;
            if r == g && g == b && r == 0.0 && col_name == EMPTY_SLOT_NAME {
                // `palette.py:187-188`：`self.append(None)`
                self.append(None, None, false, false);
            } else {
                // `palette.py:190-192`：直接 append 到内部列表（名字可能是空串）
                self.entries.push(Some(Swatch {
                    color: RgbColor::new(r, g, b),
                    name: Some(col_name),
                }));
            }
        }
        Ok(())
    }

    /// 只读访问格子列表（遮罩 ↔ GIMP 调色板互操作需要按名字分组）。
    pub fn swatches(&self) -> &[Option<Swatch>] {
        &self.entries
    }

    /// 序列化成 GIMP 调色板文本 —— `palette.py:776-799` 的 `__unicode__()`
    /// （`save()` 只是把它写进文件，`palette.py:198-222`）。
    ///
    /// 逐行照抄：表头两行按需、固定一行 `#`、每格一行
    /// `"%d %d %d    %s\n"`（名字前**四个空格**）或 `"%d %d %d\n"`（匿名字）。
    pub fn to_gimp_palette_string(&self) -> String {        let mut s = String::from("GIMP Palette\n");
        if let Some(name) = &self.name {
            s.push_str(&format!("Name: {name}\n"));
        }
        if self.columns > 0 {
            s.push_str(&format!("Columns: {}\n", self.columns));
        }
        s.push_str("#\n");
        for entry in &self.entries {
            let (r, g, b, col_name) = match entry {
                None => (0, 0, 0, Some(EMPTY_SLOT_NAME.to_string())),
                Some(sw) => {
                    // `clamp(int(c * 0xFF), 0, 0xFF)`（`palette.py:794`）
                    let q = |c: f64| clamp_i64((c * 0xFF as f64) as i64, 0, 0xFF);
                    (q(sw.color.r), q(sw.color.g), q(sw.color.b), sw.name.clone())
                }
            };
            match col_name {
                None => s.push_str(&format!("{r} {g} {b}\n")),
                Some(name) => s.push_str(&format!("{r} {g} {b}    {name}\n")),
            }
        }
        s
    }

    /// 用另一份调色板整体覆盖 —— `palette.py:224-238` `update()`。
    ///
    /// 名字与列数一起搬（`:234-235`），命中位置**不**搬（源码里 `clear()` 已把
    /// 它清空，之后没有再设，所以更新完没命中）。
    pub fn update(&mut self, other: &Palette) {
        self.clear();
        for entry in &other.entries {
            self.entries.push(entry.clone());
        }
        self.name = other.name.clone();
        self.columns = other.columns;
    }

    // ---- 尺寸与元数据（`palette.py:240-274`）----

    /// `get_columns()`：列数，0 表示未指定。
    pub fn columns(&self) -> i64 {
        self.columns
    }

    /// `set_columns(n)`：设列数（源码 `int(n)`，负数也存得下）。
    pub fn set_columns(&mut self, n: i64) {
        self.columns = n;
    }

    /// `get_name()`。
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// `set_name(name)`：`None` 表示没有名字（源码 `:257-259`）。
    pub fn set_name(&mut self, name: Option<&str>) {
        self.name = name.map(str::to_owned);
    }

    /// `__bool__`：调色板**永远**为真，与长度无关（`palette.py:262-270`）。
    ///
    /// Rust 没有隐式布尔转换，判断"有没有调色板"用的是 `Option<Palette>`；
    /// 保留这个方法是让"空调色板也算真"这条源码事实有落点、也可被测试引用。
    pub fn is_truthy(&self) -> bool {
        true
    }

    // ---- 命中标记（`palette.py:280-309`）----

    /// `get_match_position()`。
    pub fn match_position(&self) -> Option<usize> {
        self.match_position
    }

    /// `set_match_position(i)`：越界（含负数）一律置 `None`（`palette.py:290-293`）。
    pub fn set_match_position(&mut self, i: Option<usize>) {
        let i = match i {
            Some(i) if i < self.entries.len() => Some(i),
            _ => None,
        };
        self.match_position = i;
    }

    /// `get_match_is_approx()`。
    pub fn match_is_approx(&self) -> bool {
        self.match_is_approx
    }

    /// `set_match_is_approx(approx)`（源码 `bool(approx)`，`:306`）。
    pub fn set_match_is_approx(&mut self, approx: bool) {
        self.match_is_approx = approx;
    }

    /// 把命中位置挪到最接近 `col` 的格子上 —— `palette.py:311-399`
    /// `match_color(col, exact=False, order=None)`。
    ///
    /// 搜索顺序：给了 `order` 就用它；否则有命中位置就从那里向外扩散
    /// （[`outwards_from`]），没有就按 0..len。
    ///
    /// 两条分支（`palette.py:361-390`）：
    /// - `exact`：只认 `c == col`；
    /// - 否则：`c == col` 或距离 `< 0.06` 就算**精确命中**并立刻停下
    ///   （源码注释 `:365-368` 说这是为了让 PaletteNext/Prev 手感正常）；
    ///   一路都不满足时取距离最小的那个，标记为**近似**。
    ///
    /// 空槽永远跳过（`:363-364`）。返回是否命中（`:394-399`）。
    pub fn match_color(&mut self, col: &Color, exact: bool, order: Option<&[usize]>) -> bool {
        let search_order: Vec<usize> = match order {
            Some(o) => o.to_vec(),
            None => match self.match_position {
                Some(p) => outwards_from(self.entries.len(), p),
                None => (0..self.entries.len()).collect(),
            },
        };
        let mut bestmatch_i: Option<usize> = None;
        let mut bestmatch_d: Option<f64> = None;
        let mut is_approx = true;
        for i in search_order {
            // 源码越界会抛 IndexError；`order` 由调用方给，越界项跳过
            let Some(Some(swatch)) = self.entries.get(i) else {
                continue;
            };
            if exact {
                // `c == col`：左操作数是 RGBColor（`lib/color.py:329-353`）
                if swatch.color.eq_src(col) {
                    bestmatch_i = Some(i);
                    is_approx = false;
                    break;
                }
            } else {
                let d = color_distance(col, &Color::Rgb(swatch.color));
                if swatch.color.eq_src(col) || d < MATCH_APPROX_DISTANCE {
                    bestmatch_i = Some(i);
                    is_approx = false;
                    break;
                }
                if bestmatch_d.is_none() || d < bestmatch_d.unwrap_or(f64::MAX) {
                    bestmatch_i = Some(i);
                    bestmatch_d = Some(d);
                }
            }
        }
        match bestmatch_i {
            Some(i) => {
                self.match_position = Some(i);
                self.match_is_approx = is_approx;
                true
            }
            None => false,
        }
    }

    /// 按方向步进命中位置 —— `palette.py:401-488` `move_match_position()`。
    ///
    /// 三段语义（源码 docstring `:411-447` 逐条对应）：
    /// 1. 还没有命中位置 → 先 `match_color(refcol)`（**不改当前色**），返回 `None`；
    /// 2. 当前是近似命中 → 把它"落实"成精确命中，位置不变，返回该颜色；
    /// 3. 当前是精确命中 → 沿 `direction` 步进、跳过空槽，返回新颜色。
    ///
    /// `direction` 先规整成 `-1`/`0`/`1`（`:450-457`），`0` 直接返回 `None`。
    /// 返回值是"新命中的颜色"，仅在产生了新的精确命中时是 `Some`。
    pub fn move_match_position(&mut self, direction: i32, refcol: &Color) -> Option<RgbColor> {
        let direction = direction.signum();
        if direction == 0 {
            return None;
        }
        let Some(old_pos) = self.match_position else {
            // `palette.py:461-463`：只挑最近的，不动当前色
            self.match_color(refcol, false, None);
            return None;
        };
        let new_pos = if self.match_is_approx {
            // `palette.py:466-469`：把近似命中落实
            Some(old_pos)
        } else {
            // `palette.py:470-479`：沿方向找第一个非空格子
            let n = self.entries.len() as i64;
            let mut pos = old_pos as i64 + i64::from(direction);
            let mut found = None;
            while pos < n && pos >= 0 {
                if self.entries[pos as usize].is_some() {
                    found = Some(pos as usize);
                    break;
                }
                pos += i64::from(direction);
            }
            found
        };
        let Some(new_pos) = new_pos else {
            return None;
        };
        // 顺序照 `palette.py:482-487`：先取颜色、再落位置与 approx
        let result = Self::copy_color_out(&self.entries[new_pos]);
        self.set_match_position(Some(new_pos));
        self.set_match_is_approx(false);
        result
    }

    // ---- 颜色存取（`palette.py:497-707`）----

    /// `_copy_color_in(col, name=None)`：把任意 `UIColor` 折成内部格子。
    ///
    /// 源码在 `name is None` 时会去读颜色对象上的 `__name` 属性
    /// （`palette.py:510-514`，`try/except AttributeError`）；本移植的
    /// `Color` 没有挂属性的地方，所以"带名字的复制"由调用方显式传 `name`
    /// （`reposition()` 的复制到空槽分支就是这么做的）。
    fn copy_color_in(col: Option<&Color>, name: Option<&str>) -> Option<Swatch> {
        let col = col?;
        Some(Swatch {
            color: RgbColor::from_color(col),
            name: name.map(str::to_owned),
        })
    }

    /// `_copy_color_out(col)`：空槽 → `None`，否则给一份拷贝（`:499-504`）。
    fn copy_color_out(entry: &Option<Swatch>) -> Option<RgbColor> {
        entry.as_ref().map(|sw| sw.color)
    }

    /// 取某格子参与 `==` 比较时的颜色 —— 空槽是哨兵 `RGBColor(-1,-1,-1)`
    /// （`palette.py:55`）。
    fn compared_color(entry: &Option<Swatch>) -> RgbColor {
        match entry {
            Some(sw) => sw.color,
            None => EMPTY_SLOT_RGB,
        }
    }

    /// 追加一个颜色 —— `palette.py:521-548` `append(col, name=None,
    /// unique=False, match=False)`。
    ///
    /// `unique=True` 时从**末尾往前**找一个 `==` 相等的格子并直接返回
    /// （`palette.py:532-540`）：注意这里用的是 `==`（`round(3)` 比较），
    /// 不是身份比较，所以 (`a`) 空槽与真正的 `RGBColor(-1,-1,-1)` 会被认成同一个，
    /// (`b`) "红色"与"四舍五入后相同的红"也算重复。照抄。
    ///
    /// `set_match` 即源码的 `match`（Rust 的关键字），语义不变：命中位置指向
    /// 新格子（或 `unique` 找到的那个旧格子），并标记为精确。
    pub fn append(
        &mut self,
        col: Option<&Color>,
        name: Option<&str>,
        unique: bool,
        set_match: bool,
    ) {
        let col = Self::copy_color_in(col, name);
        if unique {
            let col_cmp = Self::compared_color(&col);
            for i in (0..self.entries.len()).rev() {
                // `col == self._colors[i]`：左操作数是新格子
                if col_cmp.eq_src(&Color::Rgb(Self::compared_color(&self.entries[i]))) {
                    if set_match {
                        self.match_position = Some(i);
                        self.match_is_approx = false;
                    }
                    return;
                }
            }
        }
        let end_i = self.entries.len();
        self.entries.push(col);
        if set_match {
            self.match_position = Some(end_i);
            self.match_is_approx = false;
        }
    }

    /// 插入一格 —— `palette.py:550-577` `insert(i, col, name=None)`。
    ///
    /// `i = None` 表示追加到末尾。插入点在命中位置之前或正好等于命中位置时，
    /// 命中位置跟着 +1（`palette.py:574-576`，经属性 setter 的越界检查）。
    /// 越界插入照 Python `list.insert()` 的语义夹到末尾。
    pub fn insert(&mut self, i: Option<usize>, col: Option<&Color>, name: Option<&str>) {
        let col = Self::copy_color_in(col, name);
        match i {
            None => self.entries.push(col),
            Some(i) => {
                let i = i.min(self.entries.len());
                self.entries.insert(i, col);
                if let Some(pos) = self.match_position {
                    if pos >= i {
                        self.set_match_position(Some(pos + 1));
                    }
                }
            }
        }
    }

    /// 移动/复制一格 —— `palette.py:579-671` `reposition(src_i, targ_i)`。
    ///
    /// 两条路径：
    /// - 目标是**空槽** → 复制进去（源格保留），长度不变（`:620-633`）；
    /// - 否则 → 先 `pop(src_i)` 再 `insert(targ_i)`，长度不变、顺序变
    ///   （`:635-671`）。命中位置在这两种情形下各有细规则，逐行照抄。
    ///
    /// 源码的三处边界：`src_i == targ_i` 直接返回（`:612-613`）、
    /// `src_i` 越界直接返回（`:617-618`）、`targ_i` 越界会抛 `IndexError`
    /// （`:623`）—— 最后这一条这里按无操作处理（见模块文档）。
    pub fn reposition(&mut self, src_i: usize, targ_i: Option<usize>) {
        if Some(src_i) == targ_i {
            return;
        }
        if src_i >= self.entries.len() {
            return;
        }
        // 源码 `col = self._colors[src_i]`（可能是空槽哨兵）
        let src = self.entries[src_i].clone();
        let match_pos = self.match_position;

        // ---- 特例：目标是空槽 → 只复制（`palette.py:620-633`）----
        if let Some(ti) = targ_i {
            let Some(targ) = self.entries.get(ti) else {
                // 源码此处 IndexError；按无操作处理
                return;
            };
            if targ.is_none() {
                // `_copy_color_in(col)`：源已经是 RGBColor，等价于原样带名字复制
                self.entries[ti] = src.clone();
                // 复制**自**命中格子 → 命中跟着走；复制**到**命中格子 → 清空命中
                if match_pos == Some(src_i) {
                    self.set_match_position(Some(ti));
                } else if match_pos == Some(ti) {
                    self.set_match_position(None);
                }
                return;
            }
        }

        // ---- 常规：先移除，再插入（`palette.py:635-671`）----
        self.entries.remove(src_i);
        let mut moving_match = false;
        let mut updated_match = false;
        let mut match_pos = match_pos;
        let mut targ_i = targ_i;
        if match_pos.is_some() {
            // 向右移动时要补偿 pop() 造成的下标偏移（`palette.py:641-642`）。
            // 注意源码把这一步放在 `if match_pos is not None` **里面**，
            // 所以没有命中位置时不做补偿（照抄）。
            if let Some(ti) = targ_i {
                if ti > src_i {
                    targ_i = Some(ti - 1);
                }
            }
            if match_pos == Some(src_i) {
                match_pos = None;
                moving_match = true;
                updated_match = true;
            } else if match_pos.map(|p| p > src_i).unwrap_or(false) {
                match_pos = match_pos.map(|p| p - 1);
                updated_match = true;
            }
        }
        match targ_i {
            None => {
                self.entries.push(src);
                if moving_match {
                    match_pos = Some(self.entries.len() - 1);
                    updated_match = true;
                }
            }
            Some(ti) => {
                let ti = ti.min(self.entries.len());
                self.entries.insert(ti, src);
                if let Some(p) = match_pos {
                    if moving_match {
                        match_pos = Some(ti);
                        updated_match = true;
                    } else if p >= ti {
                        match_pos = Some(p + 1);
                        updated_match = true;
                    }
                }
            }
        }
        if updated_match {
            self.set_match_position(match_pos);
        }
    }

    /// 移除一格并返回它 —— `palette.py:673-690` `pop(i)`。
    ///
    /// 命中位置等于被删下标 → 清空；大于 → 减一（`:685-688`）。
    /// 源码在 `match_position is None` 时那句 `None > i` 会抛 `TypeError`
    /// （见模块文档）；这里当作"没有命中就不用调整"。
    ///
    /// 返回被移除的颜色（空槽 → `None`），越界 → `None`。
    pub fn pop(&mut self, i: usize) -> Option<RgbColor> {
        if i >= self.entries.len() {
            return None;
        }
        let removed = self.entries.remove(i);
        if let Some(pos) = self.match_position {
            if pos == i {
                self.set_match_position(None);
            } else if pos > i {
                self.set_match_position(Some(pos - 1));
            }
        }
        Self::copy_color_out(&removed)
    }

    /// 按下标取颜色 —— `palette.py:692-707` `get_color(i)` / `__getitem__`。
    ///
    /// 空槽与越界都是 `None`。源码的 `get_color(None)` 也返回 `None`，因此
    /// "下标未知"在 Rust 里直接就是 `Option` 的 `None`。
    pub fn get(&self, i: usize) -> Option<RgbColor> {
        Self::copy_color_out(self.entries.get(i)?)
    }

    /// 按下标覆盖一格 —— `palette.py:705-707` `__setitem__`。
    ///
    /// 源码越界会抛 `IndexError`；这里返回 `false`（无操作）。
    pub fn set(&mut self, i: usize, col: Option<&Color>) -> bool {
        if i >= self.entries.len() {
            return false;
        }
        self.entries[i] = Self::copy_color_in(col, None);
        true
    }

    // ---- 名字存取（`palette.py:709-743`）----

    /// `get_color_name(i)`：空槽与越界都是 `None`（`:711-719`）。
    pub fn color_name(&self, i: usize) -> Option<&str> {
        self.entries.get(i)?.as_ref()?.name.as_deref()
    }

    /// `set_color_name(i, name)`：空槽与越界都是无操作（`:721-730`）。
    pub fn set_color_name(&mut self, i: usize, name: Option<&str>) -> bool {
        let Some(Some(sw)) = self.entries.get_mut(i) else {
            return false;
        };
        sw.name = name.map(str::to_owned);
        true
    }

    /// `get_color_by_name(name)`：找**第一个**同名的颜色（`:732-743`）。
    ///
    /// 源码遇到空槽会抛 `AttributeError`（见模块文档），这里跳过空槽。
    pub fn color_by_name(&self, name: &str) -> Option<RgbColor> {
        self.entries
            .iter()
            .flatten()
            .find(|sw| sw.name.as_deref() == Some(name))
            .map(|sw| sw.color)
    }

    // ---- 迭代与克隆 ----

    /// `iter_colors()`：逐格给出颜色，空槽 `None`（`:745-754`）。
    pub fn iter_colors(&self) -> impl Iterator<Item = Option<RgbColor>> + '_ {
        self.entries.iter().map(Self::copy_color_out)
    }

    /// 格子数 —— `palette.py:272-274` `__len__`。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否一格都没有（注意 `__bool__` 恒真，见 [`Palette::is_truthy`]）。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 源码 `__repr__`（`palette.py:822-827`）。
    ///
    /// `%r` 作用于 `None` 是 `None`、作用于 str 是带单引号的字面量。
    pub fn repr(&self) -> String {
        let name = match &self.name {
            None => "None".to_string(),
            Some(n) => format!("'{}'", n.replace('\\', "\\\\").replace('\'', "\\'")),
        };
        format!(
            "<Palette colors={}, columns={}, name={}>",
            self.entries.len(),
            self.columns,
            name
        )
    }

    /// 偏好的简单字典形式 —— `palette.py:831-844` `to_simple_dict()`。
    ///
    /// `entries` 里空槽是 `None`，其余是 `(to_hex_str(), 名字)`。
    pub fn to_simple_dict(&self) -> SimplePaletteDict {
        SimplePaletteDict {
            name: self.name.clone(),
            columns: self.columns,
            entries: self
                .entries
                .iter()
                .map(|e| match e {
                    None => None,
                    Some(sw) => Some((sw.color.to_hex_str(), sw.name.clone())),
                })
                .collect(),
        }
    }

    /// 由简单字典构造 —— `palette.py:846-859` `new_from_simple_dict()`。
    ///
    /// 逐项 `append()`（于是每格都过一遍 `_copy_color_in`）。
    /// 源码对每个条目先 `RGBColor.new_from_hex_str(s)`（`:857`）：解析失败
    /// **不报错**、退回 `#808080` 那一档（`lib/color.py:188-203`）。
    pub fn from_simple_dict(simple: &SimplePaletteDict) -> Self {
        let mut pal = Self::new();
        pal.set_name(simple.name.as_deref());
        pal.set_columns(simple.columns);
        for entry in &simple.entries {
            match entry {
                None => pal.append(None, None, false, false),
                Some((hex, name)) => {
                    let col = Color::Rgb(RgbColor::new_from_hex_str(hex));
                    pal.append(Some(&col), name.as_deref(), false, false);
                }
            }
        }
        pal
    }
}

// ---------------------------------------------------------------------------
// 行解析（等价于 `palette.py:147-149` 的三条正则）
// ---------------------------------------------------------------------------

/// `clamp(x, lo, hi)` —— `lib/helpers.py:244-249` 的整数版
/// （只服务 `.gpl` 的通道值与序列化）。
fn clamp_i64(x: i64, lo: i64, hi: i64) -> i64 {
    if x < lo {
        return lo;
    }
    if x > hi {
        return hi;
    }
    x
}

/// Python 正则里的 `\w`：字母、数字、下划线。
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `^(\w+)\s*:\s*(.*)$`（`palette.py:148`）的一行匹配。
///
/// 手写而不是引第三方正则（本块不新增依赖），逐条对应正则语义：
/// - `\w+` 至少一个词字符（ASCII 字母/数字/下划线；Python 的 `\w` 还认
///   Unicode 字母，`.gpl` 的键只有 `Name` / `Columns`，不影响）；
/// - 之后允许任意空白、一个 `:`、再任意空白；
/// - `(.*)` 吃到行尾（行已 `strip()`，没有换行）。
fn parse_field_line(line: &str) -> Option<(String, String)> {
    let mut key_end = None;
    for (idx, ch) in line.char_indices() {
        if is_word_char(ch) {
            key_end = Some(idx + ch.len_utf8());
        } else {
            break;
        }
    }
    let key_end = key_end?;
    let rest = line[key_end..].trim_start();
    let rest = rest.strip_prefix(':')?;
    Some((line[..key_end].to_string(), rest.trim_start().to_string()))
}

/// `^(\d+)\s+(\d+)\s+(\d+)\s*(?:\b(.*))$`（`palette.py:149`）的一行匹配。
///
/// 手写等价物。必须照抄的几个细节（都由源码 fixture 固化）：
/// - 三个 `\d+` 之间至少一个空白（`\s+`）；
/// - 第三个数字之后的 `\s*` 是**贪婪**的，随后是可选的 `\b(.*)`；
/// - `\b` 是词边界（"前一个是词字符" XOR "后一个是词字符"）。第三个数字的
///   末位一定是词字符，所以：
///   - 若 `\s*` 吃到行尾或停在非词字符前 → 边界在数字后立刻成立，名字是
///     其后全部内容（可能全是空白，`strip()` 成空串）；
///   - 若 `\s*` 停在**词字符**前（例如制表符后紧跟 `Black`）→ 边界在那里成立，
///     名字从该词字符起；
///   - 若第三个数字后**没有**空白且紧跟词字符（例如 `0 0 0Black`）→ 既没有
///     边界、`$` 也对不上 → 整行**不匹配**（源码只警告并跳过）。
fn parse_color_line(line: &str) -> Option<(i64, i64, i64, String)> {
    let mut rest = line;
    let mut nums = [0i64; 3];
    for i in 0..3 {
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits_end == 0 {
            return None;
        }
        // 超长数字串按 `i64` 饱和（随后还要 clamp 到 0..=0xFF）
        nums[i] = rest[..digits_end].parse::<i64>().unwrap_or(i64::MAX);
        rest = &rest[digits_end..];
        if i < 2 {
            // 数字之间的 `\s+`：至少要吃掉一个空白
            let trimmed = rest.trim_start();
            if trimmed.len() == rest.len() {
                return None;
            }
            rest = trimmed;
        }
    }
    // 第三个数字之后：`\s*` + 可选 `\b(.*)`
    let ws = rest.len() - rest.trim_start().len();
    let word = rest[ws..].chars().next().is_some_and(is_word_char);
    if word && ws == 0 {
        // `0 0 0Black`：没有词边界可用，不匹配
        return None;
    }
    let name_start = if word { ws } else { 0 };
    Some((
        nums[0],
        nums[1],
        nums[2],
        rest[name_start..].trim().to_string(),
    ))
}
