//! 颜色工具模型向量逐位门禁 —— 拿真 `adjbases.py` / `sliders.py` 算出的
//! fixture 与引擎逐点比对（B4-2）。
//!
//! fixture：`tests/fixtures/color_tools_vectors.txt`（1788 条向量），由
//! `tests/tools/gen-color-tools-vectors.py` 生成：
//!
//! ```text
//! python crates/pigment-engine/tests/tools/gen-color-tools-vectors.py
//! ```
//!
//! 脚本 `import` 的是**真** `lib/color.py`（B4-1 的同一套做法），并用 `ast`
//! 把 `gui/colors/adjbases.py` / `sliders.py` / `util.py` 里**纯数学**的函数体
//! 抽出来 `exec` —— 执行的是源码那几行，不是照抄的公式。抽出来的函数与行号
//! 写在 fixture 头部的注释里。浮点一律 Python `repr()`（最短往返），Rust 侧
//! 因此可以逐位 `assert_eq!`，**零容差**。
//!
//! 依赖 GTK 控件状态的部分（按钮按下 / 抬起的状态转移、拖动阈值、滚轮方向
//! 之外的控件行为）不在这份 fixture 里：它们在 `tests/color_tools.rs` 里用
//! 源码行号手工推导的期望值断言。

use std::collections::BTreeSet;
use std::sync::OnceLock;

use pigment_engine::color::{
    bend_hcy, point_to_amount, Color, ColorChanger, ColorTool, ComponentSlider, HcyColor, HsvColor,
    Modifiers, RgbColor, ScrollDirection, SliderKind, UIColor, YcbcrColor,
};

/// fixture 的绝对路径（编译期由 `CARGO_MANIFEST_DIR` 定位）。
fn fixture_path() -> String {
    format!(
        "{}/tests/fixtures/color_tools_vectors.txt",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// 整个 fixture 只读一次。
fn fixture() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        std::fs::read_to_string(fixture_path()).unwrap_or_else(|e| {
            panic!(
                "读不到 fixture {}：{e}（先跑 tests/tools/gen-color-tools-vectors.py）",
                fixture_path()
            )
        })
    })
}

/// fixture 的数据行。
fn data_lines() -> impl Iterator<Item = (usize, &'static str)> {
    fixture()
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
}

/// 一行的 tab 分隔字段。
fn fields(line: &str) -> Vec<&str> {
    line.split('\t').collect()
}

/// 解析一个浮点字段。
fn parse_f64(tok: &str, ln: usize) -> f64 {
    tok.parse::<f64>()
        .unwrap_or_else(|e| panic!("第 {ln} 行不是浮点：{tok:?}（{e}）"))
}

/// 解析 `"0"` / `"1"` 布尔字段。
fn parse_bool(tok: &str, ln: usize) -> bool {
    match tok {
        "0" => false,
        "1" => true,
        other => panic!("第 {ln} 行不是 0/1：{other:?}"),
    }
}

/// 解析三个浮点字段。
fn triple(toks: &[&str], ln: usize) -> [f64; 3] {
    assert_eq!(toks.len(), 3, "第 {ln} 行应有 3 个通道：{toks:?}");
    [
        parse_f64(toks[0], ln),
        parse_f64(toks[1], ln),
        parse_f64(toks[2], ln),
    ]
}

/// 逐位相等（`+0.0` 与 `-0.0` 视为相等，同 `tests/color_vectors.rs`）。
fn same_bits(actual: f64, expected: f64) -> bool {
    actual.to_bits() == expected.to_bits() || (actual == 0.0 && expected == 0.0)
}

/// 断言一个三元组逐位相等。
fn assert_triple(actual: [f64; 3], expected: [f64; 3], ctx: &str) {
    for i in 0..3 {
        assert!(
            same_bits(actual[i], expected[i]),
            "{ctx} 分量 {i} 不一致：实际 {:?}（bits {:#x}）期望 {:?}（bits {:#x}）",
            actual[i],
            actual[i].to_bits(),
            expected[i],
            expected[i].to_bits(),
        );
    }
}

/// fixture 的类型串 → `UIColor` 实现类名。
fn class_of_kind(kind: &str) -> &'static str {
    match kind {
        "rgb" => "RGBColor",
        "hsv" => "HSVColor",
        "hcy" => "HCYColor",
        "ycbcr" => "YCbCrColor",
        other => panic!("未知颜色类型 {other:?}"),
    }
}

/// 按类型串造颜色。
fn color_of(kind: &str, v: [f64; 3]) -> Color {
    match kind {
        "rgb" => Color::Rgb(RgbColor::from_rgb(v)),
        "hsv" => Color::Hsv(HsvColor::from_hsv(v)),
        "hcy" => Color::Hcy(HcyColor::from_hcy(v)),
        "ycbcr" => Color::Ycbcr(YcbcrColor::from_ycbcr(v)),
        other => panic!("未知颜色类型 {other:?}"),
    }
}

/// 按类型串取该色彩空间的三个代表分量。
fn space_triple(kind: &str, c: Color) -> [f64; 3] {
    match kind {
        "rgb" => c.get_rgb(),
        "hsv" => c.get_hsv(),
        "hcy" => {
            let Color::Hcy(h) = c else {
                panic!("期望 HCYColor，实际 {}", c.class_name())
            };
            [h.h, h.c, h.y]
        }
        "ycbcr" => {
            let Color::Ycbcr(y) = c else {
                panic!("期望 YCbCrColor，实际 {}", c.class_name())
            };
            [y.y, y.cb, y.cr]
        }
        other => panic!("未知颜色类型 {other:?}"),
    }
}

/// 按类型串造滑条。
fn slider_of(name: &str, ln: usize) -> ComponentSlider {
    let kind = SliderKind::from_class_name(name)
        .unwrap_or_else(|| panic!("第 {ln} 行未知滑条类 {name:?}"));
    ComponentSlider::new(kind)
}

// ---------------------------------------------------------------------------
// 滑条：条值 ↔ 颜色分量（sliders.py:171-305 的原文）
// ---------------------------------------------------------------------------

/// `get_bar_amount_for_color()`：从被编辑颜色读条值。
///
/// 源码出处：九个滑条的 `get_bar_amount_for_color()`（`sliders.py:176-301`）。
/// 跨空间的情形也在这里：managed 是 HSV 时 RGB 滑条读到的是
/// `get_rgb()` 的分量（`UIColor.r` 的默认实现），HCY 滑条读到的是
/// `HCYColor(color=...)` 之后的 `h/c/y`。
#[test]
fn slider_bar_amounts_match_source() {
    let mut n = 0;
    for (ln, line) in data_lines() {
        let f = fields(line);
        if f[0] != "slider_amt" {
            continue;
        }
        assert_eq!(f.len(), 7, "第 {ln} 行字段数不对：{line:?}");
        let slider = slider_of(f[1], ln);
        let managed = color_of(f[2], triple(&f[3..6], ln));
        let expected = parse_f64(f[6], ln);
        let actual = slider.get_bar_amount_for_color(&managed);
        assert!(
            same_bits(actual, expected),
            "第 {ln} 行 {} 读条值：实际 {actual:?}（bits {:#x}）期望 {expected:?}（bits {:#x}）",
            f[1],
            actual.to_bits(),
            expected.to_bits()
        );
        n += 1;
    }
    assert_eq!(n, 54, "slider_amt 向量条数");
}

/// `get_color_for_bar_amount()`：条值写回颜色分量。
///
/// 源码出处：九个滑条的 `get_color_for_bar_amount()`（`sliders.py:171-301`）。
/// 两点必须照抄：写回的颜色是**该滑条所在空间**的子类（于是中央色的类型跟着
/// 变）；`amt` 不夹取（fixture 里就有 1.5 与 -0.25）。
#[test]
fn slider_component_mapping_matches_source() {
    let mut n = 0;
    for (ln, line) in data_lines() {
        let f = fields(line);
        if f[0] != "slider_set" {
            continue;
        }
        assert_eq!(f.len(), 11, "第 {ln} 行字段数不对：{line:?}");
        let slider = slider_of(f[1], ln);
        let managed = color_of(f[2], triple(&f[3..6], ln));
        let amt = parse_f64(f[6], ln);
        let out_kind = f[7];
        let expected = triple(&f[8..11], ln);
        let actual = slider.get_color_for_bar_amount(&managed, amt);
        assert_eq!(
            actual.class_name(),
            class_of_kind(out_kind),
            "第 {ln} 行 {} 的结果类型不对（条值 {amt}）",
            f[1]
        );
        assert_triple(
            space_triple(out_kind, actual),
            expected,
            &format!("第 {ln} 行 {} 条值 {amt} 写分量", f[1]),
        );
        n += 1;
    }
    assert_eq!(n, 324, "slider_set 向量条数");
}

// ---------------------------------------------------------------------------
// 滑条坐标 ↔ 条值（adjbases.py:1055-1067 的原文）
// ---------------------------------------------------------------------------

/// `point_to_amount()`：像素 → 条值，含两侧边框扣除与竖条反向。
#[test]
fn slider_point_to_amount_matches_source() {
    let mut n = 0;
    for (ln, line) in data_lines() {
        let f = fields(line);
        if f[0] != "slider_point" {
            continue;
        }
        assert_eq!(f.len(), 6, "第 {ln} 行字段数不对：{line:?}");
        let pos = parse_f64(f[1], ln);
        let length = parse_f64(f[2], ln);
        let border = parse_f64(f[3], ln);
        let vertical = parse_bool(f[4], ln);
        let expected = parse_f64(f[5], ln);
        let actual = point_to_amount(pos, length, border, vertical);
        assert!(
            same_bits(actual, expected),
            "第 {ln} 行 point_to_amount({pos}, {length}, {border}, {vertical})：\
             实际 {actual:?}（bits {:#x}）期望 {expected:?}（bits {:#x}）",
            actual.to_bits(),
            expected.to_bits()
        );
        n += 1;
    }
    assert_eq!(n, 132, "slider_point 向量条数");
}

// ---------------------------------------------------------------------------
// 滚轮（adjbases.py:1069-1083 的原文）
// ---------------------------------------------------------------------------

/// `ComponentSlider::scroll()`：方向符号 → 夹到 0..1 → 写回分量。
///
/// 源码出处：`SliderColorAdjuster.__scroll_cb()`（`adjbases.py:1069-1083`），
/// 内部调的是九个滑条的真 `get_bar/get_color_for_bar_amount()`，所以这条
/// 向量同时约束了"读条值 → 加增量 → 夹取 → 写分量"整条链。
#[test]
fn slider_scroll_matches_source() {
    let mut n = 0;
    for (ln, line) in data_lines() {
        let f = fields(line);
        if f[0] != "slider_scroll" {
            continue;
        }
        assert_eq!(f.len(), 12, "第 {ln} 行字段数不对：{line:?}");
        let slider = slider_of(f[1], ln);
        let managed = color_of(f[2], triple(&f[3..6], ln));
        let vertical = parse_bool(f[6], ln);
        let direction = match f[7] {
            "UP" => ScrollDirection::Up,
            "DOWN" => ScrollDirection::Down,
            "LEFT" => ScrollDirection::Left,
            "RIGHT" => ScrollDirection::Right,
            "SMOOTH" => ScrollDirection::Smooth,
            other => panic!("第 {ln} 行未知滚轮方向 {other:?}"),
        };
        let out_kind = f[8];
        let expected = triple(&f[9..12], ln);
        let actual = slider.scroll(&managed, vertical, direction);
        assert_eq!(
            actual.class_name(),
            class_of_kind(out_kind),
            "第 {ln} 行 {} 滚轮结果类型不对",
            f[1]
        );
        assert_triple(
            space_triple(out_kind, actual),
            expected,
            &format!("第 {ln} 行 {} 滚轮 {:?}", f[1], direction),
        );
        n += 1;
    }
    assert_eq!(n, 540, "slider_scroll 向量条数");
}

// ---------------------------------------------------------------------------
// 按钮 2/3 的 HCY 弯曲（adjbases.py:739-788 的原文）
// ---------------------------------------------------------------------------

/// `bend_hcy()`：位移 → HCY 分量的映射（按钮 2/3 拖动）。
///
/// 源码出处：`ColorAdjusterWidget.__motion_notify_cb()` 的
/// `elif ... __button_down > 1` 分支（`adjbases.py:739-788`）。覆盖了：
/// 三个弯曲轴（shift=彩度 / ctrl=色相 / 其余=亮度）、"取位移大的轴"、
/// 文本方向 RTL 的 `dx` 镜像、以及彩度/亮度夹到 `0..1`、色相环形回绕。
#[test]
fn bend_vectors_match_source() {
    let mut n = 0;
    for (ln, line) in data_lines() {
        let f = fields(line);
        if f[0] != "bend" {
            continue;
        }
        assert_eq!(f.len(), 19, "第 {ln} 行字段数不对：{line:?}");
        let start = color_of(f[1], triple(&f[2..5], ln));
        let button: u8 = f[5].parse().expect("按钮号");
        assert!(button > 1, "第 {ln} 行弯曲只覆盖按钮 > 1");
        let w = parse_f64(f[6], ln);
        let h = parse_f64(f[7], ln);
        let sx = parse_f64(f[8], ln);
        let sy = parse_f64(f[9], ln);
        let ex = parse_f64(f[10], ln);
        let ey = parse_f64(f[11], ln);
        let modifiers = Modifiers {
            shift: parse_bool(f[12], ln),
            control: parse_bool(f[13], ln),
        };
        let rtl = parse_bool(f[14], ln);
        let expected = triple(&f[16..19], ln);

        let actual = bend_hcy(start, (sx, sy), (ex, ey), (w, h), modifiers, rtl);
        assert_triple(
            [actual.h, actual.c, actual.y],
            expected,
            &format!(
                "第 {ln} 行 起点 {} 按钮 {button} 位移 ({}, {}) mods {modifiers:?} rtl {rtl}",
                f[1],
                ex - sx,
                ey - sy
            ),
        );
        n += 1;
    }
    assert_eq!(n, 738, "bend 向量条数");
}

// ---------------------------------------------------------------------------
// 覆盖度：防止生成脚本悄悄丢了一类用例
// ---------------------------------------------------------------------------

/// fixture 必须覆盖全部九种滑条、五个滚轮方向、三种修饰键组合、
/// 两个文本方向与 2/3 两个按钮。
#[test]
fn fixture_covers_every_slider_and_bend_branch() {
    let mut sliders_seen = BTreeSet::new();
    let mut directions_seen = BTreeSet::new();
    let mut mods_seen = BTreeSet::new();
    let mut rtl_seen = BTreeSet::new();
    let mut buttons_seen = BTreeSet::new();
    let mut kinds = BTreeSet::new();

    for (ln, line) in data_lines() {
        let f = fields(line);
        kinds.insert(f[0].to_string());
        match f[0] {
            "slider_amt" | "slider_set" => {
                sliders_seen.insert(f[1].to_string());
            }
            "slider_scroll" => {
                sliders_seen.insert(f[1].to_string());
                directions_seen.insert(f[7].to_string());
            }
            "slider_point" => {}
            "bend" => {
                buttons_seen.insert(f[5].to_string());
                mods_seen.insert((f[12].to_string(), f[13].to_string()));
                rtl_seen.insert(f[14].to_string());
            }
            other => panic!("第 {ln} 行未知向量类型 {other:?}"),
        }
    }

    let expected_sliders: BTreeSet<String> = SliderKind::ALL
        .iter()
        .map(|k| k.class_name().to_string())
        .collect();
    assert_eq!(sliders_seen, expected_sliders, "九个滑条必须都被覆盖");
    assert_eq!(
        directions_seen.len(),
        5,
        "五个滚轮方向都要有：{directions_seen:?}"
    );
    assert_eq!(
        mods_seen.len(),
        3,
        "三种修饰键组合（无/shift/ctrl）都要有：{mods_seen:?}"
    );
    assert_eq!(rtl_seen.len(), 2, "LTR 与 RTL 都要有：{rtl_seen:?}");
    assert_eq!(
        buttons_seen,
        BTreeSet::from(["2".to_string(), "3".to_string()]),
        "按钮 2 与 3 都要有"
    );
    assert_eq!(
        kinds,
        BTreeSet::from([
            "bend".to_string(),
            "slider_amt".to_string(),
            "slider_point".to_string(),
            "slider_scroll".to_string(),
            "slider_set".to_string(),
        ]),
        "fixture 的五种行类型都应出现"
    );
}

// ---------------------------------------------------------------------------
// 状态机与向量的连接点：update() 必须用同一个弯曲映射
// ---------------------------------------------------------------------------

/// `ColorChanger::update()` 的按钮 3 弯曲结果必须与 [`bend_hcy`] 一致，
/// 且颜色确实落进了 [`ColorTool`]。
///
/// 源码出处：`__motion_notify_cb()` 里算完就 `self.set_managed_color(col)`
/// （`adjbases.py:788`），没有第二套公式。
#[test]
fn drag_update_uses_the_same_bend_mapping() {
    let mut tool = ColorTool::new();
    tool.set_color(color_of("rgb", [0.2, 0.4, 0.9]));
    let start = tool.get_color();

    let mut changer = ColorChanger::new(8.0);
    let pos = (60.0, 40.0);
    let outcome = changer.start_drag(&mut tool, 3, false, pos, Some(start));
    assert_eq!(
        outcome,
        pigment_engine::color::PressOutcome::Armed,
        "按钮 3 按下只记起点色（adjbases.py:697-699）"
    );

    let modifiers = Modifiers::SHIFT;
    let moved = (40.0, 55.0);
    let update = changer.update(&mut tool, moved, (100.0, 100.0), modifiers, true);
    let expected = bend_hcy(start, pos, moved, (100.0, 100.0), modifiers, true);
    assert_eq!(
        update,
        pigment_engine::color::DragUpdate::Bended(Color::Hcy(expected)),
        "update 必须就是用 bend_hcy 算的"
    );
    assert_eq!(
        tool.get_color().class_name(),
        "HCYColor",
        "弯曲后中央色变成 HCYColor（adjbases.py:747、:788）"
    );
    let [h, c, y] = space_triple("hcy", tool.get_color());
    assert_triple(
        [h, c, y],
        [expected.h, expected.c, expected.y],
        "落进工具的颜色",
    );
}
