//! 调色板模型向量逐位门禁 + 边界测试 —— 拿真 `lib/palette.py` 算出的
//! fixture 与引擎逐点比对（B4-4a）。
//!
//! fixture：`tests/fixtures/palette_vectors.txt`（8695 条向量），由
//! `tests/tools/gen-palette-vectors.py` 生成：
//!
//! ```text
//! python crates/pigment-engine/tests/tools/gen-palette-vectors.py
//! ```
//!
//! 与 B4-2 的 `gen-color-tools-vectors.py` 不同，这一份**不需要 `ast` 抽函数体**：
//! `lib/palette.py` 只 import `re` / `copy` / `logging` / `io` 与几个 `lib.*`，
//! 所以脚本直接 `import lib.palette`（只把拖进 C 扩展的 `lib.helpers` 换成一个
//! 装着**真 `clamp()`** 的替身）。被测的 `Palette` 类、`_outwards_from()`、
//! `_color_distance()` 全是源码本体。浮点一律 Python `repr()`（最短往返），
//! Rust 侧因此可以逐位 `assert_eq!`，**零容差**。
//!
//! 同一个脚本还会写 `crates/pigment-app/tests/fixtures/paletteview_vectors.txt`
//! （50261 条 `gui/colors/paletteview.py` 的布局向量），那份由控件层测试消费
//! （`shell/palette_panel.rs`）—— 那里必须用 `ast` 抽函数体（paletteview 顶部
//! import 了 GTK）。
//!
//! 文件级的边界（对话框、树视图、DnD）不在这里：那些属 `gui/colors/paletteview.py`
//! 的 GTK 装配，见交付记账。

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use pigment_engine::color::{
    color_distance, outwards_from, Color, ColorManager, HcyColor, HsvColor, Palette, RgbColor,
    SimplePaletteDict, UIColor, YcbcrColor, DEFAULT_PALETTE_GPL, EMPTY_SLOT_NAME,
};

// ---------------------------------------------------------------------------
// fixture 解析
// ---------------------------------------------------------------------------

fn fixture_path() -> String {
    format!(
        "{}/tests/fixtures/palette_vectors.txt",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        std::fs::read_to_string(fixture_path()).unwrap_or_else(|e| {
            panic!(
                "读不到 fixture {}：{e}（先跑 tests/tools/gen-palette-vectors.py）",
                fixture_path()
            )
        })
    })
}

/// fixture 的一行 → (行号, 字段)。
fn rows() -> impl Iterator<Item = (usize, Vec<&'static str>)> {
    fixture()
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|(i, l)| (i + 1, l.split('\t').collect()))
}

/// 一个格子（fixture 的 `<empty> <r> <g> <b> <hasname> <name>` 六列）。
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Empty,
    Color { rgb: [f64; 3], name: Option<String> },
}

/// `-` → `None`，其余原样（空串也算"有名字，名字是空串"）。
fn opt_name(s: &str) -> Option<String> {
    if s == "-" {
        None
    } else {
        Some(s.to_string())
    }
}

/// 解析一格的六个字段。
fn entry_from(f: &[&str], ln: usize) -> Entry {
    assert!(f.len() >= 6, "第 {ln} 行的格子字段不足：{f:?}");
    let empty = match f[0] {
        "1" => true,
        "0" => false,
        other => panic!("第 {ln} 行的 empty 标志不是 0/1：{other:?}"),
    };
    let name = opt_name(f[5]);
    if empty {
        return Entry::Empty;
    }
    Entry::Color {
        rgb: [
            parse_f64(f[1], ln),
            parse_f64(f[2], ln),
            parse_f64(f[3], ln),
        ],
        name,
    }
}

fn parse_f64(tok: &str, ln: usize) -> f64 {
    tok.parse::<f64>()
        .unwrap_or_else(|e| panic!("第 {ln} 行不是浮点：{tok:?}（{e}）"))
}

fn parse_usize(tok: &str, ln: usize) -> usize {
    tok.parse::<usize>()
        .unwrap_or_else(|e| panic!("第 {ln} 行不是非负整数：{tok:?}（{e}）"))
}

fn parse_bool(tok: &str, ln: usize) -> bool {
    match tok {
        "1" => true,
        "0" => false,
        other => panic!("第 {ln} 行不是 0/1：{other:?}"),
    }
}

/// `-` → `None`，其余解析成下标。
fn opt_index(tok: &str, ln: usize) -> Option<usize> {
    if tok == "-" {
        None
    } else {
        Some(parse_usize(tok, ln))
    }
}

/// 逐位相等（`+0.0` 与 `-0.0` 视为相等，同 `tests/color_vectors.rs`）。
fn same_bits(actual: f64, expected: f64) -> bool {
    actual.to_bits() == expected.to_bits() || (actual == 0.0 && expected == 0.0)
}

/// 一个调色板在 fixture 里的定义（`pal_meta` + `pal_entry`）。
#[derive(Clone, Debug, Default)]
struct PalDef {
    name: Option<String>,
    columns: i64,
    entries: Vec<Entry>,
}

/// 整个 fixture 的解析结果。
#[derive(Default)]
struct Fixture {
    palettes: HashMap<String, PalDef>,
    /// `pal_dump`：pal_id → 逐行文本
    dumps: HashMap<String, Vec<String>>,
    /// `pal_repr`：pal_id → repr
    reprs: HashMap<String, String>,
    /// `pal_load_ok`：text_id → 是否载入成功
    load_ok: BTreeMap<String, bool>,
    /// `pal_load_text`：text_id → 逐行原文（已反转义）
    load_texts: HashMap<String, Vec<String>>,
    /// `pal_simpledict`：pal_id → 期望的简单字典
    simple: HashMap<String, SimplePaletteDict>,
    /// `pal_simpledict` 声明的条目数（用于交叉核对）
    simple_counts: HashMap<String, usize>,
    /// `pal_distance`
    distances: Vec<([String; 2], [[f64; 3]; 2], f64)>,
    /// `pal_outwards`：(n, i, 序列)
    outwards: Vec<(usize, usize, Vec<usize>)>,
    /// `pal_match`
    matches: Vec<MatchCase>,
    /// `pal_seq` 的序列定义与逐步状态
    seqs: BTreeMap<String, String>,
    steps: BTreeMap<(String, usize), (Vec<String>, usize, Option<usize>, bool, Vec<Entry>)>,
}

/// 一条 `pal_match` 向量。
struct MatchCase {
    line: usize,
    pal_id: String,
    preset: Option<usize>,
    order: Option<Vec<usize>>,
    exact: bool,
    probe: Color,
    ret: bool,
    position: Option<usize>,
    approx: bool,
}

/// 类型串 + 三个分量 → `Color`。
fn color_of(kind: &str, v: [f64; 3]) -> Color {
    match kind {
        "rgb" => Color::Rgb(pigment_engine::color::RgbColor::new(v[0], v[1], v[2])),
        "hsv" => Color::Hsv(HsvColor::new(v[0], v[1], v[2])),
        "hcy" => Color::Hcy(HcyColor::new(v[0], v[1], v[2])),
        "ycbcr" => Color::Ycbcr(YcbcrColor::new(v[0], v[1], v[2])),
        other => panic!("未知颜色类型 {other:?}"),
    }
}

/// fixture 里 `-` 的转义还原。
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn parsed() -> &'static Fixture {
    static FX: OnceLock<Fixture> = OnceLock::new();
    FX.get_or_init(|| {
        let mut fx = Fixture::default();
        for (ln, f) in rows() {
            match f[0] {
                "pal_meta" => {
                    fx.palettes.insert(
                        f[1].to_string(),
                        PalDef {
                            name: opt_name(f[2]),
                            columns: f[3]
                                .parse()
                                .unwrap_or_else(|e| panic!("第 {ln} 行 columns 不是整数：{e}")),
                            entries: Vec::new(),
                        },
                    );
                }
                "pal_entry" => {
                    let def = fx
                        .palettes
                        .get_mut(f[1])
                        .unwrap_or_else(|| panic!("第 {ln} 行：未声明的 pal_id {}", f[1]));
                    assert_eq!(
                        parse_usize(f[2], ln),
                        def.entries.len(),
                        "第 {ln} 行：格子下标不连续"
                    );
                    def.entries.push(entry_from(&f[3..], ln));
                }
                "pal_dump" => {
                    let v = fx.dumps.entry(f[1].to_string()).or_default();
                    assert_eq!(
                        parse_usize(f[2], ln),
                        v.len(),
                        "第 {ln} 行：dump 行号不连续"
                    );
                    // 文本里不含制表符；空行是"末尾空字段"
                    v.push(f.get(3).copied().unwrap_or("").to_string());
                }
                "pal_repr" => {
                    fx.reprs.insert(f[1].to_string(), f[2].to_string());
                }
                "pal_load_ok" => {
                    fx.load_ok.insert(f[1].to_string(), parse_bool(f[2], ln));
                }
                "pal_load_text" => {
                    let v = fx.load_texts.entry(f[1].to_string()).or_default();
                    assert_eq!(parse_usize(f[2], ln), v.len(), "第 {ln} 行：原文行号不连续");
                    v.push(unescape(f.get(3).copied().unwrap_or("")));
                }
                "pal_simpledict" => {
                    // 字段：pal_id / columns / hasname / name / 条目数
                    let d = SimplePaletteDict {
                        name: opt_name(f[4]),
                        columns: f[2].parse().unwrap_or_else(|e| {
                            panic!("第 {ln} 行 simpledict columns 不是整数：{e}")
                        }),
                        entries: Vec::new(),
                    };
                    // 条目数随后逐条追加，这里只做一次交叉核对
                    let expected_entries = parse_usize(f[5], ln);
                    fx.simple.insert(f[1].to_string(), d);
                    fx.simple_counts.insert(f[1].to_string(), expected_entries);
                }
                "pal_simpledict_entry" => {
                    let d = fx
                        .simple
                        .get_mut(f[1])
                        .unwrap_or_else(|| panic!("第 {ln} 行：未声明的 simpledict {}", f[1]));
                    assert_eq!(
                        parse_usize(f[2], ln),
                        d.entries.len(),
                        "第 {ln} 行：simpledict 条目下标不连续"
                    );
                    let e = if parse_bool(f[3], ln) {
                        None
                    } else {
                        Some((f[4].to_string(), opt_name(f[6])))
                    };
                    d.entries.push(e);
                }
                "pal_distance" => {
                    // 存的是**原始分量**（不是 RGB）：HSV/HCY/YCbCr 的分量与
                    // RGB 三元组不是一回事
                    let av = [
                        parse_f64(f[2], ln),
                        parse_f64(f[3], ln),
                        parse_f64(f[4], ln),
                    ];
                    let bv = [
                        parse_f64(f[6], ln),
                        parse_f64(f[7], ln),
                        parse_f64(f[8], ln),
                    ];
                    fx.distances.push((
                        [f[1].to_string(), f[5].to_string()],
                        [av, bv],
                        parse_f64(f[9], ln),
                    ));
                }
                "pal_outwards" => {
                    let seq: Vec<usize> = f[3].split(',').map(|s| parse_usize(s, ln)).collect();
                    fx.outwards
                        .push((parse_usize(f[1], ln), parse_usize(f[2], ln), seq));
                }
                "pal_match" => {
                    let order = if parse_bool(f[3], ln) {
                        Some(f[4].split(',').map(|s| parse_usize(s, ln)).collect())
                    } else {
                        None
                    };
                    fx.matches.push(MatchCase {
                        line: ln,
                        pal_id: f[1].to_string(),
                        preset: opt_index(f[2], ln),
                        order,
                        exact: parse_bool(f[5], ln),
                        probe: color_of(
                            f[6],
                            [
                                parse_f64(f[7], ln),
                                parse_f64(f[8], ln),
                                parse_f64(f[9], ln),
                            ],
                        ),
                        ret: parse_bool(f[10], ln),
                        position: opt_index(f[11], ln),
                        approx: parse_bool(f[12], ln),
                    });
                }
                "pal_seq" => {
                    fx.seqs.insert(f[1].to_string(), f[2].to_string());
                }
                "pal_step" => {
                    let key = (f[1].to_string(), parse_usize(f[2], ln));
                    let args: Vec<String> = f[3..].iter().map(|s| s.to_string()).collect();
                    fx.steps.insert(key, (args, 0, None, false, Vec::new()));
                }
                "pal_state" => {
                    let key = (f[1].to_string(), parse_usize(f[2], ln));
                    let st = fx
                        .steps
                        .get_mut(&key)
                        .unwrap_or_else(|| panic!("第 {ln} 行：没有对应的 pal_step {key:?}"));
                    st.1 = parse_usize(f[3], ln);
                    st.2 = opt_index(f[4], ln);
                    st.3 = parse_bool(f[5], ln);
                }
                "pal_state_entry" => {
                    let key = (f[1].to_string(), parse_usize(f[2], ln));
                    let st = fx
                        .steps
                        .get_mut(&key)
                        .unwrap_or_else(|| panic!("第 {ln} 行：没有对应的 pal_step {key:?}"));
                    assert_eq!(
                        parse_usize(f[3], ln),
                        st.4.len(),
                        "第 {ln} 行：state 格子下标不连续"
                    );
                    st.4.push(entry_from(&f[4..], ln));
                }
                other => panic!("第 {ln} 行：未知类型 {other:?}"),
            }
        }
        fx
    })
}

/// 按 fixture 的定义构造一个 `Palette`。
fn build_palette(def: &PalDef) -> Palette {
    let mut p = Palette::new();
    for e in &def.entries {
        match e {
            Entry::Empty => p.append(None, None, false, false),
            Entry::Color { rgb, name } => {
                let c = Color::Rgb(pigment_engine::color::RgbColor::new(rgb[0], rgb[1], rgb[2]));
                p.append(Some(&c), name.as_deref(), false, false);
            }
        }
    }
    p.set_name(def.name.as_deref());
    p.set_columns(def.columns);
    p
}

/// 当前状态（与 fixture 的 `pal_state` 对照）。
fn state_of(p: &Palette) -> (usize, Option<usize>, bool, Vec<Entry>) {
    let entries = (0..p.len())
        .map(|i| match p.get(i) {
            None => Entry::Empty,
            Some(c) => Entry::Color {
                rgb: [c.r, c.g, c.b],
                name: p.color_name(i).map(str::to_owned),
            },
        })
        .collect();
    (p.len(), p.match_position(), p.match_is_approx(), entries)
}

// ---------------------------------------------------------------------------
// 1. 真 .gpl 文件：载入结果逐格比对
// ---------------------------------------------------------------------------

/// 19 个源码 `.gpl` 文件 + 5 个手工调色板的载入结果：名字、列数、逐格颜色
/// 与名字全部逐位相等。
#[test]
fn gpl_loads_match_source_entry_by_entry() {
    let fx = parsed();
    assert!(
        fx.palettes.len() >= 24,
        "fixture 里的调色板太少：{}",
        fx.palettes.len()
    );
    // 必须包含源码那几个有代表性的文件
    for id in [
        "MyPaint_Default",
        "GIMP_Default",
        "Greys16",
        "Tango_Palette",
        "greys16",
        "red2blue5",
        "empty_mix",
        "dups",
        "black_only",
    ] {
        assert!(fx.palettes.contains_key(id), "fixture 缺调色板 {id}");
    }
    let mut checked = 0usize;
    for (id, def) in &fx.palettes {
        let pal = build_palette(def);
        assert_eq!(pal.len(), def.entries.len(), "{id} 的长度");
        assert_eq!(pal.name(), def.name.as_deref(), "{id} 的名字");
        assert_eq!(pal.columns(), def.columns, "{id} 的列数");
        for (i, e) in def.entries.iter().enumerate() {
            match e {
                Entry::Empty => {
                    assert!(pal.get(i).is_none(), "{id} 第 {i} 格应为空槽");
                    assert!(pal.color_name(i).is_none(), "{id} 第 {i} 格空槽没有名字");
                }
                Entry::Color { rgb, name } => {
                    let got = pal
                        .get(i)
                        .unwrap_or_else(|| panic!("{id} 第 {i} 格不该是空槽"));
                    for c in 0..3 {
                        assert!(
                            same_bits([got.r, got.g, got.b][c], rgb[c]),
                            "{id} 第 {i} 格分量 {c}：{} != {}",
                            [got.r, got.g, got.b][c],
                            rgb[c]
                        );
                    }
                    assert_eq!(
                        pal.color_name(i).map(str::to_owned),
                        *name,
                        "{id} 第 {i} 格的名字"
                    );
                }
            }
            checked += 1;
        }
    }
    assert!(checked > 1900, "实际只比对了 {checked} 格");
}

/// 默认调色板就是 `palettes/MyPaint_Default.gpl`：77 格、7 列、名字、
/// 首尾两格（源码文件里是 Black 与 CMYK Yellow）。
#[test]
fn builtin_default_palette_is_the_source_file() {
    let pal = Palette::mypaint_default();
    assert_eq!(pal.name(), Some("MyPaint Default"));
    assert_eq!(pal.columns(), 7);
    assert_eq!(pal.len(), 77, "11 行 × 7 列");
    // 第 0 格：0 0 0    Black
    let first = pal.get(0).expect("第 0 格有颜色");
    assert_eq!((first.r, first.g, first.b), (0.0, 0.0, 0.0));
    assert_eq!(pal.color_name(0), Some("Black"));
    // 最后一格：255 255 0    CMYK Yellow
    let last = pal.get(76).expect("最后一格有颜色");
    assert_eq!((last.r, last.g, last.b), (1.0, 1.0, 0.0));
    assert_eq!(pal.color_name(76), Some("CMYK Yellow"));
    // 没有空槽
    assert!(pal.iter_colors().all(|c| c.is_some()));
    // 内嵌的原文就是源码目录那一份（前两行；注意源文件是 CRLF 换行，
    // `load_str` 靠 `str::lines()` + `trim()` 吃掉了 `\r`）
    let mut head = DEFAULT_PALETTE_GPL.lines();
    assert_eq!(head.next(), Some("GIMP Palette"));
    assert_eq!(head.next(), Some("Name: MyPaint Default"));
    assert_eq!(head.next(), Some("Columns: 7"));
    // 与 fixture 里同一个文件载入的结果一致（真源码走的是同一个解析器）
    let fx = parsed();
    let def = &fx.palettes["MyPaint_Default"];
    assert_eq!(pal.len(), def.entries.len());
    assert_eq!(pal.name(), def.name.as_deref());
    assert_eq!(pal.columns(), def.columns);
}

/// 序列化回 `.gpl` 的**每一行**都与源码 `__unicode__()` 相同
/// （`palette.py:776-799`）。
#[test]
fn gpl_dump_matches_source_line_by_line() {
    let fx = parsed();
    let mut checked = 0usize;
    for (id, lines) in &fx.dumps {
        let Some(def) = fx.palettes.get(id) else {
            panic!("dump 里的 {id} 没有对应的调色板定义");
        };
        let pal = build_palette(def);
        let text = pal.to_gimp_palette_string();
        let got: Vec<&str> = text.split('\n').collect();
        assert_eq!(got.len(), lines.len(), "{id} 的 dump 行数");
        for (i, want) in lines.iter().enumerate() {
            assert_eq!(&got[i], want, "{id} dump 第 {i} 行");
            checked += 1;
        }
    }
    assert!(checked > 2000, "实际只比对了 {checked} 行");
}

/// `__repr__` 逐字相同（`palette.py:822-827`）。
#[test]
fn repr_matches_source() {
    let fx = parsed();
    for (id, want) in &fx.reprs {
        let pal = build_palette(&fx.palettes[id]);
        assert_eq!(&pal.repr(), want, "{id} 的 repr");
    }
    // 空调色板的字面形式
    assert_eq!(
        Palette::new().repr(),
        "<Palette colors=0, columns=0, name=None>"
    );
}

/// 偏好的简单字典：`to_simple_dict()` 逐项相同，且
/// `from_simple_dict(to_simple_dict(p))` 还原出同一份调色板
/// （`palette.py:831-859`）。
#[test]
fn simple_dict_round_trips_like_source() {
    let fx = parsed();
    for (id, want) in &fx.simple {
        let pal = build_palette(&fx.palettes[id]);
        let got = pal.to_simple_dict();
        assert_eq!(got.columns, want.columns, "{id} 的 columns");
        assert_eq!(got.name, want.name, "{id} 的 name");
        assert_eq!(got.entries.len(), want.entries.len(), "{id} 的条目数");
        for (i, w) in want.entries.iter().enumerate() {
            assert_eq!(&got.entries[i], w, "{id} 第 {i} 项");
        }
        // 往返：hex 往返只保留 8bpc，所以比较的是**同一份**调色板
        let back = Palette::from_simple_dict(&want.clone());
        assert_eq!(back.len(), pal.len(), "{id} 往返后长度");
        assert_eq!(back.name(), pal.name(), "{id} 往返后名字");
        assert_eq!(back.columns(), pal.columns(), "{id} 往返后列数");
    }
    // 空槽在简单字典里是 None，还原回空槽
    let pal = build_palette(&fx.palettes["empty_mix"]);
    let back = Palette::from_simple_dict(&pal.to_simple_dict());
    assert_eq!(back.len(), pal.len());
    assert!(
        back.get(1).is_none() && back.get(4).is_none(),
        "空槽位置保持"
    );
    assert!(back.get(0).is_some() && back.get(2).is_some());
}

/// `.gpl` 解析的容错分支：坏表头 / 坏 Columns / 杂乱分隔 / 表头结束后的
/// 键值行 / 未知键 / 只有表头 / 空槽（`palette.py:147-196`）。
#[test]
fn gpl_parse_edge_cases_match_source() {
    let fx = parsed();
    assert!(
        fx.load_ok.len() >= 7,
        "fixture 里的 load 用例太少：{}",
        fx.load_ok.len()
    );
    for (id, want_ok) in &fx.load_ok {
        let lines = fx
            .load_texts
            .get(id)
            .unwrap_or_else(|| panic!("{id} 缺 pal_load_text"));
        // 逐行原文按 '\n' 拼回去（生成端就是这么切的，末尾空行保留了换行）
        let text = lines.join("\n");
        let mut pal = Palette::new();
        let got = pal.load_str(&text);
        assert_eq!(
            got.is_ok(),
            *want_ok,
            "{id}：载入是否成功不一致（实际 {got:?}）"
        );
        if !want_ok {
            continue;
        }
        let def = &fx.palettes[id];
        assert_eq!(pal.len(), def.entries.len(), "{id} 的格子数");
        assert_eq!(pal.name(), def.name.as_deref(), "{id} 的名字");
        assert_eq!(pal.columns(), def.columns, "{id} 的列数");
        for (i, e) in def.entries.iter().enumerate() {
            match e {
                Entry::Empty => assert!(pal.get(i).is_none(), "{id} 第 {i} 格应为空槽"),
                Entry::Color { rgb, name } => {
                    let got = pal
                        .get(i)
                        .unwrap_or_else(|| panic!("{id} 第 {i} 格不该是空槽"));
                    assert!(
                        same_bits(got.r, rgb[0])
                            && same_bits(got.g, rgb[1])
                            && same_bits(got.b, rgb[2]),
                        "{id} 第 {i} 格颜色"
                    );
                    assert_eq!(
                        pal.color_name(i).map(str::to_owned),
                        *name,
                        "{id} 第 {i} 格名字"
                    );
                }
            }
        }
        // 再序列化一遍也要一样
        if let Some(want_lines) = fx.dumps.get(id) {
            let text = pal.to_gimp_palette_string();
            let got_lines: Vec<&str> = text.split('\n').collect();
            assert_eq!(got_lines.len(), want_lines.len(), "{id} 重新序列化的行数");
            for (i, want) in want_lines.iter().enumerate() {
                assert_eq!(&got_lines[i], want, "{id} 重新序列化第 {i} 行");
            }
        }
    }
    // 两个错误分支具体是什么错
    let mut pal = Palette::new();
    assert_eq!(
        pal.load_str("nope\n"),
        Err(pigment_engine::color::PaletteLoadError::NotAGimpPalette)
    );
    assert!(matches!(
        pal.load_str("GIMP Palette\nColumns: seven\n"),
        Err(pigment_engine::color::PaletteLoadError::BadColumns(_))
    ));
}

// ---------------------------------------------------------------------------
// 2. 距离 / 搜索顺序
// ---------------------------------------------------------------------------

/// `_color_distance()` 逐位相同（`palette.py:884-897`，注意是**立方根**）。
#[test]
fn color_distance_is_bit_exact() {
    let fx = parsed();
    assert!(fx.distances.len() >= 100);
    for (kinds, rgbs, want) in &fx.distances {
        let a = color_of(&kinds[0], rgbs[0]);
        let b = color_of(&kinds[1], rgbs[1]);
        let got = color_distance(&a, &b);
        assert!(
            same_bits(got, *want),
            "distance({}, {}) = {got:?}（bits {:#x}）期望 {want:?}（bits {:#x}）",
            kinds[0],
            kinds[1],
            got.to_bits(),
            want.to_bits()
        );
    }
    // 自身到自身的距离是 0（YCbCr 往返后仍严格为 0）
    let c = Color::Rgb(pigment_engine::color::RgbColor::new(0.3, 0.6, 0.9));
    assert_eq!(color_distance(&c, &c), 0.0);
}

/// `_outwards_from()` 逐项相同，**包括源码里重复输出的那几次**
/// （`palette.py:865-882`）。
#[test]
fn outwards_from_reproduces_source_duplicates() {
    let fx = parsed();
    for (n, i, want) in &fx.outwards {
        assert_eq!(outwards_from(*n, *i), *want, "outwards_from({n}, {i})");
    }
    // 源码的重复：i 先输出一次，j = 0 那一轮又输出两次
    assert_eq!(outwards_from(1, 0), vec![0, 0, 0]);
    assert_eq!(outwards_from(3, 1), vec![1, 1, 1, 0, 2]);
}

// ---------------------------------------------------------------------------
// 3. match_color
// ---------------------------------------------------------------------------

/// `match_color()` 的返回值、命中位置、近似标志全部相同
/// （`palette.py:311-399`）：覆盖 exact 两态、显式 `order`、预置命中位置
/// 后的向外搜索、空槽跳过。
#[test]
fn match_color_matches_source() {
    let fx = parsed();
    assert!(
        fx.matches.len() >= 1000,
        "match 向量太少：{}",
        fx.matches.len()
    );
    for case in &fx.matches {
        let def = &fx.palettes[&case.pal_id];
        let mut pal = build_palette(def);
        if let Some(p) = case.preset {
            pal.set_match_position(Some(p));
            assert_eq!(
                pal.match_position(),
                Some(p),
                "第 {} 行：预置命中位置",
                case.line
            );
        }
        let ret = pal.match_color(&case.probe, case.exact, case.order.as_deref());
        assert_eq!(ret, case.ret, "第 {} 行：返回值", case.line);
        assert_eq!(
            pal.match_position(),
            case.position,
            "第 {} 行：命中位置",
            case.line
        );
        assert_eq!(
            pal.match_is_approx(),
            case.approx,
            "第 {} 行：近似标志",
            case.line
        );
    }
}

/// 源码 doctest 里的那几条（`palette.py:331-348`）：这是唯一能看到
/// "近似命中 → 精确命中 → exact 失败"整条语义的地方。
#[test]
fn match_color_follows_source_doctest() {
    // red2blue = RGBColor(1,0,0).interpolate(RGBColor(0,1,1), 5)
    let red = Color::Rgb(pigment_engine::color::RgbColor::new(1.0, 0.0, 0.0));
    let cyan = Color::Rgb(pigment_engine::color::RgbColor::new(0.0, 1.0, 1.0));
    let cols: Vec<Color> = red.interpolate(&cyan, 5);
    let mut p = Palette::from_colors(&cols);
    assert_eq!(p.len(), 5);

    let grey45 = Color::Rgb(pigment_engine::color::RgbColor::new(0.45, 0.45, 0.45));
    assert!(p.match_color(&grey45, false, None));
    assert_eq!(p.match_position(), Some(2));
    assert!(p.match_is_approx(), "0.45 的灰只是近似命中");
    let matched = p.get(p.match_position().unwrap()).unwrap();
    assert_eq!((matched.r, matched.g, matched.b), (0.5, 0.5, 0.5));

    let grey50 = Color::Rgb(pigment_engine::color::RgbColor::new(0.5, 0.5, 0.5));
    assert!(p.match_color(&grey50, false, None));
    assert!(!p.match_is_approx(), "0.5 的灰是精确命中");

    // exact=True 时近似色不再算命中
    assert!(!p.match_color(&grey45, true, None));
    assert!(p.match_color(&grey50, true, None));
    // 失败时不动命中标记
    assert_eq!(p.match_position(), Some(2));
}

// ---------------------------------------------------------------------------
// 4. 有状态的编辑操作
// ---------------------------------------------------------------------------

/// 八个操作序列（append/insert/pop/reposition/set/setname/setmatch/clear/
/// update/move）逐步比对：每步之后长度、命中位置、近似标志、以及**每一格**
/// 的颜色与名字都要与源码相同。
#[test]
fn edit_operations_match_source_step_by_step() {
    let fx = parsed();
    assert_eq!(fx.seqs.len(), 8, "序列数");
    let mut compared_steps = 0usize;
    for (seq_id, base_id) in &fx.seqs {
        let mut pal = build_palette(&fx.palettes[base_id]);
        let mut step = 0usize;
        loop {
            let Some((args, len, pos, approx, entries)) = fx.steps.get(&(seq_id.clone(), step))
            else {
                break;
            };
            apply_op(&mut pal, args, &fx, seq_id, step);
            let got = state_of(&pal);
            assert_eq!(got.0, *len, "{seq_id} 第 {step} 步（{}）的长度", args[0]);
            assert_eq!(
                got.1, *pos,
                "{seq_id} 第 {step} 步（{}）的命中位置",
                args[0]
            );
            assert_eq!(
                got.2, *approx,
                "{seq_id} 第 {step} 步（{}）的近似标志",
                args[0]
            );
            assert_eq!(got.3.len(), entries.len(), "{seq_id} 第 {step} 步的格子数");
            for (i, (a, b)) in got.3.iter().zip(entries.iter()).enumerate() {
                assert_eq!(a, b, "{seq_id} 第 {step} 步（{}）的第 {i} 格", args[0]);
            }
            compared_steps += 1;
            step += 1;
        }
        assert!(step > 0, "{seq_id} 一步都没有");
    }
    assert_eq!(compared_steps, fx.steps.len(), "所有 step 都该被比对");
}

/// 回放 fixture 的一步操作。参数编码与生成脚本的 `payload` 一一对应。
///
/// 名字字段用 `-` 表示 `None`、空串表示"有名字但是空的"，所以 `<hasname>`
/// 那一列是冗余的（照着 fixture 保留，便于人工核对）。
fn apply_op(pal: &mut Palette, args: &[String], fx: &Fixture, seq_id: &str, step: usize) {
    let ctx = || format!("{seq_id} 第 {step} 步");
    match args[0].as_str() {
        "append" => {
            // op,种类,c0,c1,c2,hasname,name,unique,match
            let col = op_color(&args[1..]);
            pal.append(
                col.as_ref(),
                opt_name(&args[6]).as_deref(),
                args[7] == "1",
                args[8] == "1",
            );
        }
        "insert" => {
            // op,i,种类,c0,c1,c2,hasname,name
            let i = opt_index(&args[1], 0);
            let col = op_color(&args[2..]);
            pal.insert(i, col.as_ref(), opt_name(&args[7]).as_deref());
        }
        "pop" => {
            pal.pop(
                args[1]
                    .parse()
                    .unwrap_or_else(|e| panic!("{}：pop 下标 {e}", ctx())),
            );
        }
        "reposition" => {
            let src: usize = args[1].parse().unwrap_or_else(|e| panic!("{}：{e}", ctx()));
            let targ = opt_index(&args[2], 0);
            pal.reposition(src, targ);
        }
        "set" => {
            let i: usize = args[1].parse().unwrap_or_else(|e| panic!("{}：{e}", ctx()));
            let col = op_color(&args[2..]);
            let ok = pal.set(i, col.as_ref());
            assert!(ok, "{}：set 越界", ctx());
        }
        "setname" => {
            let i: usize = args[1].parse().unwrap_or_else(|e| panic!("{}：{e}", ctx()));
            let ok = pal.set_color_name(i, opt_name(&args[3]).as_deref());
            assert!(ok, "{}：set_color_name 越界", ctx());
        }
        "setmatch" => {
            let p = opt_index(&args[1], 0);
            pal.set_match_position(p);
        }
        "move" => {
            // 参考色 = 当前命中格的颜色；空槽/没命中时用生成脚本里那个灰
            let refcol = pal
                .match_position()
                .and_then(|p| pal.get(p))
                .map(Color::Rgb)
                .unwrap_or(Color::Rgb(pigment_engine::color::RgbColor::new(
                    0.5, 0.55, 0.45,
                )));
            let dir: i32 = args[1].parse().unwrap_or_else(|e| panic!("{}：{e}", ctx()));
            pal.move_match_position(dir, &refcol);
        }
        "clear" => pal.clear(),
        "update" => {
            let other = build_palette(&fx.palettes[&args[1]]);
            pal.update(&other);
        }
        other => panic!("{}：未知操作 {other:?}", ctx()),
    }
}

/// 操作参数里的颜色（`<种类> <c0> <c1> <c2>`，种类 `none` 表示 `None`）。
fn op_color(args: &[String]) -> Option<Color> {
    if args[0] == "none" {
        return None;
    }
    let v = |i: usize| -> f64 {
        args[i]
            .parse()
            .unwrap_or_else(|e| panic!("颜色分量 {i} 不是浮点：{e}"))
    };
    Some(color_of(&args[0], [v(1), v(2), v(3)]))
}

// ---------------------------------------------------------------------------
// 5. 模型边界（不依赖 fixture）
// ---------------------------------------------------------------------------

/// 空调色板上的所有操作都不炸：源码里能走到这些路径的地方都是"没匹配就返回"。
#[test]
fn empty_palette_boundaries() {
    let mut p = Palette::new();
    assert_eq!(p.len(), 0);
    assert!(p.is_empty());
    // `__bool__` 恒真（`palette.py:262-270`）
    assert!(p.is_truthy());
    assert!(p.get(0).is_none());
    assert!(p.color_name(0).is_none());
    assert!(p.color_by_name("nope").is_none());
    assert!(p.iter_colors().next().is_none());
    assert!(p.pop(0).is_none());
    assert!(!p.set(0, None));
    assert!(!p.set_color_name(0, Some("x")));
    // match_color：没有格子 → 返回 false、命中位置不变
    let grey = Color::Rgb(pigment_engine::color::RgbColor::new(0.5, 0.5, 0.5));
    assert!(!p.match_color(&grey, false, None));
    assert_eq!(p.match_position(), None);
    assert!(!p.match_color(&grey, true, None));
    // move_match_position：没命中位置 → 只挑最近的（没有），返回 None
    assert!(p.move_match_position(1, &grey).is_none());
    assert!(p.move_match_position(-1, &grey).is_none());
    assert!(p.move_match_position(0, &grey).is_none());
    // reposition 的空 src / 越界目标都不炸
    p.reposition(0, None);
    p.reposition(3, Some(0));
    p.insert(Some(99), None, None); // 越界插入 → 追加
    assert_eq!(p.len(), 1);
    // 序列化/反序列化空调色板
    assert_eq!(
        p.to_gimp_palette_string(),
        format!("GIMP Palette\n#\n0 0 0    {EMPTY_SLOT_NAME}\n")
    );
    let mut q = Palette::new();
    q.load_str(&p.to_gimp_palette_string()).expect("空槽往返");
    assert_eq!(q.len(), 1);
    assert!(q.get(0).is_none(), "空槽往返后仍是空槽");
    assert_eq!(q.color_name(0), None);
    // 简单字典往返
    let d = p.to_simple_dict();
    assert_eq!(d.entries, vec![None]);
    assert_eq!(Palette::from_simple_dict(&d).len(), 1);
}

/// 超长的颜色名字：不截断、不丢，序列化时用四个空格与 RGB 分开
/// （`palette.py:798` 的 `"%d %d %d    %s\n"`）。
///
/// 注意 `.gpl` 的读取会 `strip()` 名字（`palette.py:183`），所以带首尾空白的
/// 名字往返一次后会被削掉 —— 这是源码行为，单独钉住。
#[test]
fn long_color_names_survive_round_trip() {
    let mut p = Palette::new();
    // 中间有空格、**不带**首尾空白（load 会 strip 首尾）
    let long: String = "很长的名字".repeat(200);
    let col = Color::Rgb(pigment_engine::color::RgbColor::new(0.25, 0.5, 0.75));
    p.append(Some(&col), Some(&long), false, false);
    assert_eq!(p.color_name(0), Some(long.as_str()));
    assert_eq!(
        p.color_name(0).unwrap().chars().count(),
        long.chars().count()
    );

    let text = p.to_gimp_palette_string();
    assert!(
        text.contains(&format!("63 127 191    {long}")),
        "名字前是四个空格"
    );

    let mut q = Palette::new();
    q.load_str(&text).expect("往返");
    assert_eq!(q.len(), 1);
    assert_eq!(q.color_name(0), Some(long.as_str()));
    assert_eq!(q.get(0).unwrap().r, 63.0 / 255.0);

    // 首尾空白在载入时被 strip 掉（源码 `:183` 的 `col_name.strip()`）
    let mut padded = Palette::new();
    padded.append(Some(&col), Some("  spaced  "), false, false);
    assert!(padded
        .to_gimp_palette_string()
        .contains("63 127 191      spaced  \n"));
    let mut back = Palette::new();
    back.load_str(&padded.to_gimp_palette_string())
        .expect("往返");
    assert_eq!(back.color_name(0), Some("spaced"), "首尾空白被削掉");

    // 空名字（不是 None）与没有名字是两回事：dump 里前者带四个空格
    let mut r = Palette::new();
    r.append(Some(&col), Some(""), false, false);
    assert_eq!(r.color_name(0), Some(""));
    assert!(r.to_gimp_palette_string().contains("63 127 191    \n"));
    let mut s = Palette::new();
    s.append(Some(&col), None, false, false);
    assert_eq!(s.color_name(0), None);
    assert!(s.to_gimp_palette_string().contains("63 127 191\n"));
}

/// 重复色：`append(unique=True)` 从**末尾**找第一个 `==` 相等的格子
/// （`palette.py:532-540`），命中位置指向的是那个旧格子。
#[test]
fn duplicate_colors_and_unique_append() {
    let red = Color::Rgb(pigment_engine::color::RgbColor::new(1.0, 0.0, 0.0));
    let blue = Color::Rgb(pigment_engine::color::RgbColor::new(0.0, 0.0, 1.0));
    let mut p = Palette::new();
    p.append(Some(&red), Some("Red"), false, false);
    p.append(Some(&blue), Some("Blue"), false, false);
    p.append(Some(&red), Some("Red again"), false, false);
    assert_eq!(p.len(), 3);

    // unique 追加已有色 → 不增长度，命中位置指到**最后**一个同色格子
    p.append(Some(&red), Some("Red"), true, true);
    assert_eq!(p.len(), 3, "unique 去重");
    assert_eq!(p.match_position(), Some(2));
    assert!(!p.match_is_approx());
    assert_eq!(
        p.color_name(2),
        Some("Red again"),
        "命中的是旧格子，名字不改"
    );

    // 新色照常追加并命中
    let green = Color::Rgb(pigment_engine::color::RgbColor::new(0.0, 1.0, 0.0));
    p.append(Some(&green), None, true, true);
    assert_eq!(p.len(), 4);
    assert_eq!(p.match_position(), Some(3));

    // `==` 是 round(3) 比较：四舍五入后相同的颜色也算重复
    let almost = Color::Rgb(pigment_engine::color::RgbColor::new(1.0, 0.0004, 0.0));
    p.append(Some(&almost), None, true, true);
    assert_eq!(p.len(), 4, "round(3) 后与 Red 相同 → 视为重复");
    assert_eq!(p.match_position(), Some(2));

    // 空槽哨兵 RGBColor(-1,-1,-1) 与真正的 (-1,-1,-1) 在 unique 判等里相等
    let mut q = Palette::new();
    q.append(Some(&red), None, false, false);
    q.append(None, None, false, false); // 空槽
    assert_eq!(q.len(), 2);
    let sentinel = Color::Rgb(pigment_engine::color::RgbColor::new(-1.0, -1.0, -1.0));
    q.append(Some(&sentinel), None, true, true);
    assert_eq!(q.len(), 2, "哨兵与真 (-1,-1,-1) 在 `==` 下相等（源码如此）");
    assert_eq!(q.match_position(), Some(1), "命中到那个空槽");
    // 但 `pop`/`get` 仍然按身份把空槽当空槽
    assert!(q.get(1).is_none());
}

/// `PaletteNext` / `PalettePrev` 的模型语义（`palette.py:401-488`）：
/// 未命中 → 先挑最近的不换色；近似命中 → 落实；精确命中 → 步进；
/// 走到头就停住（源码的 `while` 不环绕）；空槽被跳过。
#[test]
fn move_match_position_follows_source_semantics() {
    let mk = |r: f64, g: f64, b: f64| Color::Rgb(pigment_engine::color::RgbColor::new(r, g, b));
    let mut p = Palette::new();
    for c in [
        mk(1.0, 0.0, 0.0),
        mk(0.0, 1.0, 0.0),
        mk(0.0, 0.0, 1.0),
        mk(1.0, 1.0, 0.0),
    ] {
        p.append(Some(&c), None, false, false);
    }
    // 没命中位置 → 挑最近的，返回 None（不换色）
    let refcol = mk(0.0, 1.0, 0.0);
    assert!(p.move_match_position(1, &refcol).is_none());
    assert_eq!(p.match_position(), Some(1), "挑了绿色");
    assert!(!p.match_is_approx(), "绿色就在板上 → 精确命中");

    // 精确命中 → 步进，返回新颜色
    let next = p.move_match_position(1, &refcol).expect("步进到蓝色");
    assert_eq!((next.r, next.g, next.b), (0.0, 0.0, 1.0));
    assert_eq!(p.match_position(), Some(2));
    let next = p.move_match_position(1, &refcol).expect("步进到黄色");
    assert_eq!((next.r, next.g, next.b), (1.0, 1.0, 0.0));
    assert_eq!(p.match_position(), Some(3));

    // 走到末尾之后再往前 → 停在末尾（源码的 while 不环绕）
    assert!(
        p.move_match_position(1, &refcol).is_none(),
        "末尾没有下一格"
    );
    assert_eq!(p.match_position(), Some(3), "位置不变");

    // 往回走
    let prev = p.move_match_position(-1, &refcol).expect("退到蓝色");
    assert_eq!((prev.r, prev.g, prev.b), (0.0, 0.0, 1.0));
    assert_eq!(p.match_position(), Some(2));
    // 走到 0 之后再往回 → 停在 0
    assert!(p.move_match_position(-1, &refcol).is_some());
    assert!(p.move_match_position(-1, &refcol).is_some());
    assert_eq!(p.match_position(), Some(0));
    assert!(p.move_match_position(-1, &refcol).is_none());
    assert_eq!(p.match_position(), Some(0));

    // 方向 0 → 什么都不做
    assert!(p.move_match_position(0, &refcol).is_none());
    assert_eq!(p.match_position(), Some(0));

    // 近似命中 → 落实（位置不变、approx 变 false、返回该颜色）
    let mut q = Palette::new();
    q.append(Some(&mk(0.5, 0.5, 0.5)), None, false, false);
    q.append(Some(&mk(1.0, 1.0, 1.0)), None, false, false);
    let near = mk(0.45, 0.45, 0.45);
    assert!(q.match_color(&near, false, None));
    assert_eq!(q.match_position(), Some(0));
    assert!(q.match_is_approx(), "0.45 只是近似");
    let concrete = q.move_match_position(1, &near).expect("落实成精确命中");
    assert_eq!((concrete.r, concrete.g, concrete.b), (0.5, 0.5, 0.5));
    assert_eq!(q.match_position(), Some(0), "落实不改位置");
    assert!(!q.match_is_approx());

    // 空槽被跳过
    let mut r = Palette::new();
    r.append(Some(&mk(1.0, 0.0, 0.0)), None, false, false);
    r.append(None, None, false, false);
    r.append(None, None, false, false);
    r.append(Some(&mk(0.0, 0.0, 1.0)), None, false, false);
    r.set_match_position(Some(0));
    r.set_match_is_approx(false);
    let jumped = r
        .move_match_position(1, &mk(1.0, 0.0, 0.0))
        .expect("跳过两个空槽");
    assert_eq!((jumped.r, jumped.g, jumped.b), (0.0, 0.0, 1.0));
    assert_eq!(r.match_position(), Some(3));
}

/// `reposition()` 的复制/移动语义（`palette.py:579-671`）：
/// 目标是空槽 → 复制（源保留）；否则 → 移动（源消失）。
#[test]
fn reposition_copies_into_empty_slots_and_moves_otherwise() {
    let mk = |r: f64, g: f64, b: f64| Color::Rgb(pigment_engine::color::RgbColor::new(r, g, b));
    let red = mk(1.0, 0.0, 0.0);
    let blue = mk(0.0, 0.0, 1.0);
    let mut p = Palette::new();
    p.append(Some(&red), Some("Red"), false, false);
    p.append(None, None, false, false);
    p.append(Some(&blue), Some("Blue"), false, false);
    assert_eq!(p.len(), 3);

    // 复制进空槽：长度不变、源还在、名字也带过去
    p.set_match_position(Some(0));
    p.reposition(0, Some(1));
    assert_eq!(p.len(), 3, "复制不改变长度");
    assert!(p.get(0).is_some(), "源格保留");
    assert_eq!(p.get(1).expect("空槽被填上").r, 1.0);
    assert_eq!(p.color_name(1), Some("Red"), "名字也复制过去");
    assert_eq!(p.match_position(), Some(1), "复制自命中格 → 命中跟着走");

    // 复制**到**命中格所在的空槽 → 命中被清空（`palette.py:631-632`）
    let mut p2 = Palette::new();
    p2.append(Some(&red), Some("Red"), false, false);
    p2.append(None, None, false, false);
    p2.set_match_position(Some(1));
    p2.reposition(0, Some(1));
    assert_eq!(p2.match_position(), None, "复制到命中格 → 清空命中");

    // 移动到一个非空槽：源消失、顺序变、长度不变
    let green = mk(0.0, 1.0, 0.0);
    let mut q = Palette::new();
    for (col, name) in [(&red, "A"), (&green, "B"), (&blue, "C")] {
        q.append(Some(col), Some(name), false, false);
    }
    // 没有命中位置 → 源码不补偿 pop() 的下标偏移（`palette.py:639-642`）
    q.reposition(0, Some(2));
    assert_eq!(q.len(), 3);
    assert_eq!(q.color_name(0), Some("B"));
    assert_eq!(q.color_name(1), Some("C"));
    assert_eq!(q.color_name(2), Some("A"), "插到目标下标处（未补偿）");

    // 移到末尾
    q.reposition(0, None);
    assert_eq!(q.len(), 3);
    assert_eq!(q.color_name(0), Some("C"));
    assert_eq!(q.color_name(2), Some("B"));

    // 源下标越界 / 目标等于源 → 无操作
    let before: Vec<Option<RgbColor>> = q.iter_colors().collect();
    q.reposition(9, Some(0));
    q.reposition(1, Some(1));
    let after: Vec<Option<RgbColor>> = q.iter_colors().collect();
    assert_eq!(before, after);
}

// ---------------------------------------------------------------------------
// 6. 与 ColorManager 的接线（`adjbases.py:219`）
// ---------------------------------------------------------------------------

/// `ColorManager::set_color()` 之后调色板的命中位置自动更新 —— 源码
/// `set_color()` 的最后一步就是 `self._palette.match_color(color)`。
#[test]
fn color_manager_keeps_palette_match_in_sync() {
    let mut mgr = ColorManager::new();
    // 启动时调色板是默认的那一份，还没换过颜色 → 没有命中
    assert_eq!(mgr.palette().len(), 77);
    assert_eq!(mgr.palette().match_position(), None);

    // 换成一个板上有的颜色（Black，第 0 格）→ 精确命中
    let black = Color::Rgb(pigment_engine::color::RgbColor::new(0.0, 0.0, 0.0));
    assert!(mgr.set_color(black));
    assert_eq!(mgr.palette().match_position(), Some(0));
    assert!(!mgr.palette().match_is_approx());
    assert_eq!(mgr.palette().color_name(0), Some("Black"));

    // 换成一个板上没有的颜色 → 近似命中最近的格子（White 在第 6 格）
    let near_white = Color::Rgb(pigment_engine::color::RgbColor::new(1.0, 1.0, 0.97));
    assert!(mgr.set_color(near_white));
    assert!(mgr.palette().match_position().is_some());
    assert!(mgr.palette().match_is_approx(), "近似命中");

    // 颜色没变 → set_color 短路，调色板也不动
    let before = mgr.palette().match_position();
    assert!(!mgr.set_color(near_white));
    assert_eq!(mgr.palette().match_position(), before);

    // `PaletteAddCurrentColor` 的效果：unique 追加 + 命中
    let cur = mgr.get_color();
    let before_len = mgr.palette().len();
    mgr.palette_mut().append(Some(&cur), None, true, true);
    assert_eq!(mgr.palette().len(), before_len + 1);
    assert_eq!(mgr.palette().match_position(), Some(before_len));
    // 再加一次同样的颜色 → unique 去重，长度不变
    let cur = mgr.get_color();
    mgr.palette_mut().append(Some(&cur), None, true, true);
    assert_eq!(mgr.palette().len(), before_len + 1);
}

/// `PaletteNext` / `PalettePrev` 动作的整条链（`drawwindow.py:659-684`）：
/// 取当前色 → `move_match_position` → 有色就 `set_color`。
#[test]
fn palette_next_and_prev_actions_follow_drawwindow() {
    let mut mgr = ColorManager::new();
    // 把调色板换成一小串灰，方便看步进
    let mut pal = Palette::new();
    for i in 0..4 {
        let v = i as f64 / 3.0;
        pal.append(
            Some(&Color::Rgb(pigment_engine::color::RgbColor::new(v, v, v))),
            None,
            false,
            false,
        );
    }
    mgr.palette_mut().update(&pal);

    // 当前色 #003153 不在板上 → 第一次按 Next 只挑最近的、不换色
    let cur = mgr.get_color();
    let before = cur.to_hex_str();
    assert!(mgr.palette_mut().move_match_position(1, &cur).is_none());
    assert_eq!(mgr.get_color().to_hex_str(), before, "只挑最近的不换色");
    assert!(mgr.palette().match_position().is_some());
    assert!(mgr.palette().match_is_approx(), "#003153 只是近似");

    // 第二次按 Next → 落实到最近的灰并换色
    let cur = mgr.get_color();
    let new = mgr
        .palette_mut()
        .move_match_position(1, &cur)
        .expect("落实成精确命中");
    assert!(mgr.set_color(Color::Rgb(new)));
    assert!(!mgr.palette().match_is_approx());

    // 之后每次 Next 都能步进（直到末尾），Prev 能退回来
    let mut seen = 0;
    loop {
        let cur = mgr.get_color();
        let Some(new) = mgr.palette_mut().move_match_position(1, &cur) else {
            break;
        };
        assert!(mgr.set_color(Color::Rgb(new)));
        seen += 1;
        assert!(seen < 10, "步进必须终止");
    }
    assert!(seen >= 1, "至少能往前走一格");
    let cur = mgr.get_color();
    assert!(
        mgr.palette_mut().move_match_position(-1, &cur).is_some(),
        "Prev 能退回来"
    );
}
