//! 颜色管理器与调整动作的单元测试 —— `ColorManager` / `ColorAction` /
//! `BrushStep` / `ContextRestoreColor` 的边界行为。
//!
//! 逐位对照源码数值的向量门禁在 `tests/color_vectors.rs`；这里补的是
//! **状态机与夹取规则**（历史容量/去重、动作单调性、开关往返）。

use pigment_engine::color::{
    colors_match, BrushStep, Color, ColorAction, ColorManager, HcyColor, HsvColor, RgbColor,
    UIColor, DEFAULT_HIST_HEX, HIST_LEN, LEVEL_FLOOR,
};

/// 造一个 RGB 颜色。
fn rgb(r: f64, g: f64, b: f64) -> Color {
    Color::Rgb(RgbColor::new(r, g, b))
}

/// 造一个 HSV 颜色。
fn hsv(h: f64, s: f64, v: f64) -> Color {
    Color::Hsv(HsvColor::new(h, s, v))
}

/// `#rrggbb` → `Color`（走源码的 `new_from_hex_str`）。
fn hex(s: &str) -> Color {
    Color::Rgb(RgbColor::new_from_hex_str(s))
}

/// 近似相等（只用于不能逐位断言的组合，如 `1.8/1.8` 这类）。
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

// ---------------------------------------------------------------------------
// 启动状态
// ---------------------------------------------------------------------------

/// 启动时：历史 = `_DEFAULT_HIST`，当前色 = 历史末项（`adjbases.py:121-131`）。
#[test]
fn startup_seeds_default_history_and_current_color() {
    let mgr = ColorManager::new();
    assert_eq!(mgr.get_history().len(), HIST_LEN);
    for (i, hexstr) in DEFAULT_HIST_HEX.iter().enumerate() {
        assert!(
            colors_match(&mgr.get_history()[i], &hex(hexstr)),
            "历史第 {i} 项应为 {hexstr}，实际 {}",
            mgr.get_history()[i].repr()
        );
    }
    // col_hex = hist_hex[-1]
    assert!(colors_match(&mgr.get_color(), &hex("#003153")));
    assert!(colors_match(&mgr.get_previous_color(), &hex("#003153")));
    // 历史与当前色都是 RGBColor（`new_from_hex_str()` 恒返回 RGBColor）
    assert_eq!(mgr.get_color().class_name(), "RGBColor");
}

/// `history_hex()` 就是源码写回偏好的那个格式（`adjbases.py:258-263`）。
#[test]
fn history_hex_matches_prefs_format() {
    let mgr = ColorManager::new();
    assert_eq!(
        mgr.history_hex(),
        vec![
            "#ee3333".to_string(),
            "#336699".to_string(),
            "#44aa66".to_string(),
            "#aa6633".to_string(),
            "#003153".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// 当前色
// ---------------------------------------------------------------------------

/// `set_color()` 的颜色没变就不动（`adjbases.py:213-214`），返回值告诉我们变没变。
#[test]
fn set_color_ignores_unchanged_color() {
    let mut mgr = ColorManager::new();
    let current = mgr.get_color();
    assert!(!mgr.set_color(current), "同一颜色应判为未变");
    assert!(mgr.set_color(hsv(0.5, 0.5, 0.5)), "换颜色应判为变了");
    assert_eq!(mgr.get_color().class_name(), "HSVColor");
}

/// `set_color()` 存的是**拷贝**，调用方之后改不了管理器里的颜色。
/// （源码靠 `copy(color)`，`adjbases.py:215`；`Color` 是 `Copy`，语义相同。）
#[test]
fn set_color_stores_a_copy() {
    let mut mgr = ColorManager::new();
    let mut incoming = RgbColor::new(0.1, 0.2, 0.3);
    mgr.set_color(Color::Rgb(incoming));
    incoming.r = 0.9;
    assert!(near(mgr.get_color().get_rgb()[0], 0.1));
    assert!(near(incoming.r, 0.9), "改的是那份副本");
}

// ---------------------------------------------------------------------------
// 颜色历史：容量、去重、MRU
// ---------------------------------------------------------------------------

/// 压入历史里已有的颜色 → 它被移到末尾、长度不变（`adjbases.py:249-257`）。
#[test]
fn push_history_dedupes_and_moves_to_end() {
    let mut mgr = ColorManager::new();
    let first = mgr.get_history()[0];
    mgr.push_history(first);
    assert_eq!(mgr.get_history().len(), HIST_LEN);
    assert!(colors_match(&mgr.get_previous_color(), &first));
    let hits = mgr
        .get_history()
        .iter()
        .filter(|c| colors_match(c, &first))
        .count();
    assert_eq!(hits, 1, "同一个颜色在历史里只能出现一次");
}

/// 压入新颜色 → 末尾是它，最旧的被挤掉，长度仍是 `_HIST_LEN`（`_trim_hist()`）。
#[test]
fn push_history_trims_to_capacity() {
    let mut mgr = ColorManager::new();
    let oldest = mgr.get_history()[0];
    let newest = rgb(0.5, 0.5, 0.5);
    mgr.push_history(newest);
    assert_eq!(mgr.get_history().len(), HIST_LEN);
    assert!(colors_match(&mgr.get_previous_color(), &newest));
    assert!(
        !mgr.get_history().iter().any(|c| colors_match(c, &oldest)),
        "最旧的应被挤出"
    );
}

/// 去重是**跨颜色类**的：历史里存的是 RGBColor，压进来的 HSVColor 只要相等
/// 也会被去掉（判据 `c == color or color == c`，两个方向都试）。
#[test]
fn push_history_dedupes_across_color_classes() {
    let mut mgr = ColorManager::new();
    let same_as_last = HsvColor::from_color(&hex("#003153"));
    mgr.push_history(Color::Hsv(same_as_last));
    assert_eq!(
        mgr.get_history().len(),
        HIST_LEN,
        "等价的 HSVColor 不该新增一项"
    );
    assert_eq!(
        mgr.get_history().last().map(|c| c.class_name()),
        Some("HSVColor"),
        "被压入的那份（HSVColor）应在末尾"
    );
    let hits = mgr
        .get_history()
        .iter()
        .filter(|c| colors_match(c, &hex("#003153")))
        .count();
    assert_eq!(hits, 1);
}

/// `push_current_history()` 压的是当前色 —— 应用层每笔结束调它
/// （`brushcolor.py:54-57`）。
#[test]
fn push_current_history_uses_current_color() {
    let mut mgr = ColorManager::new();
    let target = hsv(0.25, 0.75, 0.5);
    mgr.set_color(target);
    mgr.push_current_history();
    assert!(colors_match(&mgr.get_previous_color(), &target));
}

/// 连续压入超过容量的不同颜色，长度恒为 `_HIST_LEN`。
#[test]
fn history_capacity_is_fixed() {
    let mut mgr = ColorManager::new();
    for i in 0..20 {
        let t = i as f64 / 20.0;
        mgr.push_history(rgb(t, 1.0 - t, 0.5));
        assert_eq!(
            mgr.get_history().len(),
            HIST_LEN,
            "第 {i} 次压入后长度应恒为 5"
        );
    }
}

// ---------------------------------------------------------------------------
// 颜色调整动作：边界与单调性
// ---------------------------------------------------------------------------

/// 明度调整的夹取不对称：`Brighter` 只顶上界、`Darker` 只顶下界 0.005
/// （`document.py:1592-1609`）。
#[test]
fn brighter_and_darker_clamp_asymmetrically() {
    // Brighter：v += 0.08，上限 1.0
    assert!(near(ColorAction::Brighter.apply([0.5, 0.5, 1.0])[2], 1.0));
    assert!(near(ColorAction::Brighter.apply([0.5, 0.5, 0.95])[2], 1.0));
    assert!(near(ColorAction::Brighter.apply([0.5, 0.5, 0.5])[2], 0.58));
    // Darker：v -= 0.08，下限 0.005（不是 0）
    assert!(near(
        ColorAction::Darker.apply([0.5, 0.5, 0.0])[2],
        LEVEL_FLOOR
    ));
    assert!(near(
        ColorAction::Darker.apply([0.5, 0.5, 0.01])[2],
        LEVEL_FLOOR
    ));
    assert!(near(ColorAction::Darker.apply([0.5, 0.5, 0.5])[2], 0.42));
    // 两者都不碰 h/s
    let out = ColorAction::Brighter.apply([0.3, 0.7, 0.5]);
    assert!(near(out[0], 0.3) && near(out[1], 0.7));
}

/// 饱和度调整同样是单边夹取（`document.py:1627-1644`）。
#[test]
fn purer_and_grayer_clamp_asymmetrically() {
    assert!(near(ColorAction::Purer.apply([0.5, 1.0, 0.5])[1], 1.0));
    assert!(near(ColorAction::Purer.apply([0.5, 0.5, 0.5])[1], 0.58));
    assert!(near(
        ColorAction::Grayer.apply([0.5, 0.0, 0.5])[1],
        LEVEL_FLOOR
    ));
    assert!(near(ColorAction::Grayer.apply([0.5, 0.5, 0.5])[1], 0.42));
    // 灰色彩度 0 加一档还是 0.08（从 0 起步不会被下限拦住）
    assert!(near(ColorAction::Purer.apply([0.5, 0.0, 0.5])[1], 0.08));
}

/// 色相调整是环形的（`% 1.0`，`document.py:1611-1625`）。
#[test]
fn hue_actions_wrap_around() {
    let up = ColorAction::IncreaseHue.apply([0.99, 0.5, 0.5]);
    assert!(near(up[0], (0.99 + 0.015) % 1.0), "实际 {}", up[0]);
    assert!(up[0] >= 0.0 && up[0] < 1.0);
    let down = ColorAction::DecreaseHue.apply([0.0, 0.5, 0.5]);
    assert!(near(down[0], 0.985), "实际 {}", down[0]);
    // 一圈回到原处
    let mut h = 0.0;
    for _ in 0..(1.0 / 0.015_f64).round() as usize {
        h = ColorAction::IncreaseHue.apply([h, 0.5, 0.5])[0];
    }
    assert!(h >= 0.0 && h < 1.0, "色相应一直在 [0,1) 内，实际 {h}");
    // 色相动作不碰 s/v
    let out = ColorAction::IncreaseHue.apply([0.5, 0.25, 0.75]);
    assert!(near(out[1], 0.25) && near(out[2], 0.75));
}

/// 六个动作的 id 与 `resources.xml` 一一对应，且能反查。
#[test]
fn color_action_names_round_trip() {
    for act in ColorAction::ALL {
        assert_eq!(
            ColorAction::from_action_name(act.action_name()),
            Some(act),
            "{} 反查失败",
            act.action_name()
        );
    }
    assert_eq!(ColorAction::from_action_name("Bigger"), None);
    assert_eq!(ColorAction::from_action_name("Unknown"), None);
}

/// 调整动作经管理器执行后，当前色的**类型变成 HSVColor**
/// （`lib/color.py:13-18` + `brushcolor.py:39-48` 的类型切换）。
#[test]
fn adjust_switches_current_color_to_hsv() {
    let mut mgr = ColorManager::new();
    assert_eq!(mgr.get_color().class_name(), "RGBColor");
    assert!(mgr.adjust(ColorAction::Brighter));
    assert_eq!(mgr.get_color().class_name(), "HSVColor");
    // 明度确实上去了，色相/饱和度保持（相对上一色）
    let before = HsvColor::from_color(&hex("#003153"));
    let after = mgr.get_color().get_hsv();
    assert!(near(after[2], (before.v + 0.08).min(1.0)));
    assert!(near(after[0], before.h) && near(after[1], before.s));
}

/// 对已经是上限的明度再 `Brighter`：颜色不变 → `set_color` 判未变。
#[test]
fn adjust_at_ceiling_is_a_no_op() {
    let mut mgr = ColorManager::new();
    mgr.set_color(hsv(0.5, 0.5, 1.0));
    assert!(!mgr.adjust(ColorAction::Brighter), "1.0 再提亮应无变化");
    mgr.set_color(hsv(0.5, 0.5, LEVEL_FLOOR));
    assert!(!mgr.adjust(ColorAction::Darker), "已在下限再压暗应无变化");
}

// ---------------------------------------------------------------------------
// 笔刷尺寸 / 不透明度（Bigger/Smaller/MoreOpaque/LessOpaque）
// ---------------------------------------------------------------------------

/// 四个动作各自作用在哪个笔刷设置上（内部名 = `BrushSetting::cname()`）。
#[test]
fn brush_step_targets_match_source() {
    assert_eq!(BrushStep::Bigger.cname(), "radius_logarithmic");
    assert_eq!(BrushStep::Smaller.cname(), "radius_logarithmic");
    assert_eq!(BrushStep::MoreOpaque.cname(), "opaque");
    assert_eq!(BrushStep::LessOpaque.cname(), "opaque");
    for step in BrushStep::ALL {
        assert_eq!(BrushStep::from_action_name(step.action_name()), Some(step));
    }
    assert_eq!(BrushStep::from_action_name("Brighter"), None);
}

/// 四个动作的 `cname` 都能被 libmypaint 的设置表反查 —— 应用层靠它找到
/// 要写哪个笔刷设置（`pigment-app` 的 `run_action` 里 `from_cname(...)`）。
#[test]
fn brush_step_cnames_resolve_to_settings() {
    use pigment_engine::hokusai::BrushSetting;
    for step in BrushStep::ALL {
        assert!(
            BrushSetting::from_cname(step.cname()).is_some(),
            "{} 的 cname {:?} 反查不到设置",
            step.action_name(),
            step.cname()
        );
    }
    assert_eq!(
        BrushSetting::from_cname("radius_logarithmic"),
        Some(BrushSetting::Radius)
    );
    assert_eq!(
        BrushSetting::from_cname("opaque"),
        Some(BrushSetting::Opaque)
    );
}

/// 尺寸动作：`±0.3`，夹到 `radius_logarithmic` 的 `[-2.0, 6.0]`。
#[test]
fn brush_size_steps_clamp_to_setting_bounds() {
    assert!(near(BrushStep::Bigger.apply(0.0), 0.3));
    assert!(near(BrushStep::Bigger.apply(-1.9), -1.6));
    assert!(near(BrushStep::Bigger.apply(5.9), 6.0), "顶到上界");
    assert!(near(BrushStep::Bigger.apply(6.0), 6.0), "已在上界不动");
    assert!(near(BrushStep::Bigger.apply(8.0), 6.0), "越界值被夹回");
    assert!(near(BrushStep::Smaller.apply(0.0), -0.3));
    assert!(near(BrushStep::Smaller.apply(-1.9), -2.0), "顶到下界");
    assert!(near(BrushStep::Smaller.apply(-2.0), -2.0), "已在下界不动");
    assert!(near(BrushStep::Smaller.apply(-5.0), -2.0), "越界值被夹回");
}

/// 不透明度动作：`*1.8` / `/1.8`，夹到 `opaque` 的 `[0.0, 2.0]`
/// —— 注意上界是 **2.0** 不是 1.0（`document.py:1585` 的 `* 1.8` 能超过 1）。
#[test]
fn brush_opacity_steps_clamp_to_setting_bounds() {
    assert!(near(BrushStep::MoreOpaque.apply(1.0), 1.8), "1.0 乘 1.8");
    assert!(
        near(BrushStep::MoreOpaque.apply(1.2), 2.0),
        "1.2*1.8=2.16 夹到 2.0"
    );
    assert!(near(BrushStep::MoreOpaque.apply(2.0), 2.0), "已在上界不动");
    assert!(near(BrushStep::MoreOpaque.apply(0.0), 0.0));
    assert!(near(BrushStep::LessOpaque.apply(1.8), 1.0), "1.8 除 1.8");
    assert!(near(BrushStep::LessOpaque.apply(0.0), 0.0), "0 除不出去");
    assert!(near(BrushStep::LessOpaque.apply(2.0), 2.0 / 1.8));
}

// ---------------------------------------------------------------------------
// ContextRestoreColor（`misc.context_restores_color`）
// ---------------------------------------------------------------------------

/// 默认开着（`gui/userconfig.py:93`）。
#[test]
fn context_restore_color_defaults_to_on() {
    let mgr = ColorManager::new();
    assert!(mgr.context_restores_color());
}

/// 开着时恢复槽位会连颜色一起还原（`document.py:1717-1719` +
/// `brushmodifier.restore_context_of_selected_brush()`）。
#[test]
fn restore_brush_context_restores_color_when_enabled() {
    let mut mgr = ColorManager::new();
    let saved = [0.5, 0.25, 0.75];
    assert!(mgr.restore_brush_context(saved));
    assert_eq!(mgr.get_color().class_name(), "HSVColor");
    let got = mgr.get_color().get_hsv();
    assert!(near(got[0], saved[0]) && near(got[1], saved[1]) && near(got[2], saved[2]));
}

/// 关掉之后恢复槽位**不动**当前色（颜色只随笔刷设置走）。
#[test]
fn restore_brush_context_keeps_color_when_disabled() {
    let mut mgr = ColorManager::new();
    mgr.set_color(rgb(0.1, 0.2, 0.3));
    let before = mgr.get_color();
    mgr.set_context_restores_color(false);
    assert!(!mgr.restore_brush_context([0.5, 0.25, 0.75]));
    assert!(
        colors_match(&mgr.get_color(), &before),
        "关掉开关后颜色不该被槽位覆盖"
    );
}

/// 开关可以来回拨（`document.py:1721-1724` 写偏好；`resources.xml:381` 的按键 `）。
#[test]
fn context_restore_color_toggles_both_ways() {
    let mut mgr = ColorManager::new();
    assert!(!mgr.toggle_context_restores_color());
    assert!(!mgr.context_restores_color());
    assert!(mgr.toggle_context_restores_color());
    assert!(mgr.context_restores_color());
    mgr.set_context_restores_color(false);
    assert!(!mgr.context_restores_color());
}

/// 存入槽位时记下的颜色 = `app.brush.get_color_hsv()`
/// （`document.py:1711` 存的是整份 brushinfo，颜色就是这三个基值）。
#[test]
fn brush_slot_color_is_current_hsv() {
    let mut mgr = ColorManager::new();
    mgr.set_color(hsv(0.4, 0.6, 0.8));
    let slot = mgr.brush_slot_color();
    assert!(near(slot[0], 0.4) && near(slot[1], 0.6) && near(slot[2], 0.8));
    // 存取往返
    mgr.set_color(rgb(0.0, 0.0, 0.0));
    assert!(mgr.restore_brush_context(slot));
    assert!(colors_match(&mgr.get_color(), &hsv(0.4, 0.6, 0.8)));
}

// ---------------------------------------------------------------------------
// 相等语义的非对称性（源码如此，见 model 模块文档）
// ---------------------------------------------------------------------------

/// `HCYColor == RGBColor` 与 `RGBColor == HCYColor` 可以给出**不同**结果：
/// 前者因 `other.c` 抛 `AttributeError` 退回 8bpc 比较，后者走 `round(3)`。
/// 0.5001 与 0.5009 的 8bpc 都是 127，但 `round(3)` 是 0.5 与 0.501。
#[test]
fn color_equality_is_asymmetric_like_source() {
    let a = Color::Hcy(HcyColor::new(0.3, 0.0, 0.5001));
    let b = rgb(0.5009, 0.5009, 0.5009);
    assert!(a.eq_src(&b), "HCY 在左：8bpc 都是 127 → 相等");
    assert!(!b.eq_src(&a), "RGB 在左：round(3) 是 0.5 vs 0.501 → 不等");
    // 而 push_history 的判据是两个方向取或，所以这种颜色**算**重复
    assert!(colors_match(&a, &b));
}
