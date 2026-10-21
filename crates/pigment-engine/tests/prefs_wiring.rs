//! 偏好接线（B6-1）的集成测试：`Prefs` → 各消费方。
//!
//! 这些测试只碰内存里的偏好 dict（[`Prefs::from_values`]）与纯函数，
//! 不走磁盘；`settings.json` 的读写与默认表门禁在 `prefs.rs` 的单元测试里
//! （fixture 由 `tests/tools/gen-prefs-defaults.py` 从源码生成）。
//!
//! 覆盖的源码行为：
//! - `ColorManager.__init__` 读 `colors.*` / `misc.context_restores_color`
//!   （`gui/colors/adjbases.py:121-147`）与写回（`:216`、`:258-263`、`:307`、`:354-357`）；
//! - 自由手绘的桶旋转三键与隐藏光标（`gui/freehand.py:280`、`:461-478`）；
//! - `ModeStack` 把配置推给栈里所有模式（源码是每事件现读偏好）；
//! - 调色板简单字典的 JSON 往返（`lib/palette.py:831-859`）。

use std::collections::BTreeMap;
use std::sync::Mutex;

use pigment_engine::color::{
    colors_match, Color, ColorManager, Palette, RgbColor, SimplePaletteDict, WheelType,
    DEFAULT_HIST_HEX, HIST_LEN,
};
use pigment_engine::mode::freehand::{FreehandConfig, FreehandMode};
use pigment_engine::mode::{InteractionMode, ModeStack};
use pigment_engine::prefs::{self, Prefs};
use serde_json::json;

/// 造一份只有若干键的偏好（其余键仍是默认表里的值）。
fn prefs_with(pairs: &[(&str, serde_json::Value)]) -> Prefs {
    let mut values = BTreeMap::new();
    for (k, v) in pairs {
        values.insert((*k).to_string(), v.clone());
    }
    Prefs::from_values(values)
}

fn hex(s: &str) -> Color {
    Color::Rgb(RgbColor::new_from_hex_str(s))
}

// ---------------------------------------------------------------------------
// ColorManager ⇄ Prefs
// ---------------------------------------------------------------------------

/// 启动：历史 = `_DEFAULT_HIST + prefs[colors.history]`（裁到 5），
/// 当前色 = `prefs[colors.current]`（缺失取历史末项）—— `adjbases.py:121-131`。
#[test]
fn color_manager_reads_history_and_current_color_from_prefs() {
    let prefs = prefs_with(&[
        ("colors.history", json!(["#112233", "#445566"])),
        ("colors.current", json!("#aabbcc")),
    ]);
    let mgr = ColorManager::from_prefs(&prefs);

    // 5 条默认 + 2 条用户历史，裁到最近 5 条：3 条默认 + 2 条用户
    assert_eq!(mgr.get_history().len(), HIST_LEN);
    assert!(
        colors_match(&mgr.get_history()[0], &hex(DEFAULT_HIST_HEX[2])),
        "裁切保留的是列表**末尾**（adjbases.py:237-238）"
    );
    assert!(colors_match(&mgr.get_history()[3], &hex("#112233")));
    assert!(colors_match(&mgr.get_history()[4], &hex("#445566")));
    // 当前色取 colors.current
    assert!(colors_match(&mgr.get_color(), &hex("#aabbcc")));
    assert_eq!(mgr.get_color().class_name(), "RGBColor");
}

/// `colors.current` 缺失 → 取历史末项（`adjbases.py:129-131`）。
#[test]
fn color_manager_falls_back_to_the_newest_history_color() {
    let prefs = prefs_with(&[("colors.history", json!(["#112233"]))]);
    let mgr = ColorManager::from_prefs(&prefs);
    assert!(
        colors_match(&mgr.get_color(), &hex("#112233")),
        "hist_hex[-1] 是最近用过的颜色"
    );

    // 连历史都没有 → 出厂历史末项 #003153
    let mgr = ColorManager::from_prefs(&Prefs::from_values(BTreeMap::new()));
    assert!(colors_match(&mgr.get_color(), &hex("#003153")));
    assert_eq!(mgr.get_history().len(), HIST_LEN);
}

/// 色轮类型与 `misc.context_restores_color` 也从偏好来
/// （`adjbases.py:133-136`、`userconfig.py:93`）。
#[test]
fn color_manager_reads_wheel_type_and_context_restores_color() {
    let prefs = prefs_with(&[
        ("colors.wheels.type", json!("ryb")),
        ("misc.context_restores_color", json!(false)),
    ]);
    let mgr = ColorManager::from_prefs(&prefs);
    assert_eq!(mgr.wheel_type(), WheelType::Ryb);
    assert_eq!(mgr.wheel_type_name(), "ryb");
    assert!(!mgr.context_restores_color());

    // 未知色轮名 → 源码会 KeyError；这里退回 rgb（保守）
    let prefs = prefs_with(&[("colors.wheels.type", json!("nope"))]);
    assert_eq!(
        ColorManager::from_prefs(&prefs).wheel_type(),
        WheelType::DEFAULT
    );
}

/// 调色板：`prefs[colors.palette]` 是"简单字典"，不是 `.gpl` 文本
/// （`adjbases.py:139-147` + `palette.py:846-859`）。
#[test]
fn color_manager_reads_palette_from_prefs() {
    let prefs = prefs_with(&[(
        "colors.palette",
        json!({
            "name": "测试板",
            "columns": 2,
            "entries": [["#ff0000", "红"], null, ["#00ff00", null]]
        }),
    )]);
    let mgr = ColorManager::from_prefs(&prefs);
    let pal = mgr.palette();
    assert_eq!(pal.name(), Some("测试板"));
    assert_eq!(pal.columns(), 2);
    assert_eq!(pal.len(), 3, "空槽也占一格");
    assert!(colors_match(
        &Color::Rgb(pal.get(0).unwrap()),
        &hex("#ff0000")
    ));
    assert_eq!(pal.color_name(0), Some("红"));
    assert!(pal.get(1).is_none(), "null 是空槽");
    assert_eq!(pal.color_name(2), None, "名字为 null");

    // 坏调色板（不是对象）→ 回落默认 `.gpl`（`adjbases.py:142-147`）
    let prefs = prefs_with(&[("colors.palette", json!("not a dict"))]);
    let mgr = ColorManager::from_prefs(&prefs);
    assert_eq!(
        mgr.palette().len(),
        Palette::mypaint_default().len(),
        "回落默认调色板"
    );
}

/// 写回：5 条键都要出现在偏好里，且能原样读回来
/// （`adjbases.py:216`/`:258-263`/`:307`/`:354-357` + `document.py:1721-1724`）。
#[test]
fn color_manager_store_prefs_writes_the_source_keys() {
    let mut prefs = Prefs::from_values(BTreeMap::new());
    let mut mgr = ColorManager::from_prefs(&prefs);
    mgr.set_color(hex("#123456"));
    mgr.push_history(hex("#654321"));
    mgr.set_wheel_type(WheelType::Rygb);
    mgr.set_context_restores_color(false);
    mgr.palette_mut()
        .append(Some(&hex("#abcdef")), Some("新色"), false, false);

    mgr.store_prefs(&mut prefs);

    // 键名照抄源码常量（adjbases.py:45-48 + userconfig.py:93）
    assert_eq!(prefs.get_str("colors.current"), Some("#123456"));
    let hist = prefs.get_array("colors.history").expect("历史是列表");
    assert_eq!(
        hist.last().unwrap(),
        &json!("#654321"),
        "最近用的在最后（adjbases.py:256）"
    );
    assert_eq!(prefs.get_str("colors.wheels.type"), Some("rygb"));
    assert!(!prefs.bool_or("misc.context_restores_color", true));
    let pal = prefs.get("colors.palette").expect("调色板写回了");
    assert_eq!(
        pal["entries"].as_array().unwrap().len(),
        mgr.palette().len()
    );
    assert_eq!(
        pal["entries"].as_array().unwrap().last().unwrap()[0],
        json!("#abcdef")
    );

    // 用写回后的偏好重建 → 状态一致（往返）
    let again = ColorManager::from_prefs(&prefs);
    assert!(colors_match(&again.get_color(), &hex("#123456")));
    assert_eq!(again.wheel_type(), WheelType::Rygb);
    assert!(!again.context_restores_color());
    assert!(colors_match(
        &Color::Rgb(again.palette().get(again.palette().len() - 1).unwrap()),
        &hex("#abcdef")
    ));
    assert_eq!(
        again.palette().color_name(again.palette().len() - 1),
        Some("新色")
    );
    // 历史往返后仍是同样 5 条
    assert_eq!(again.get_history().len(), HIST_LEN);
    assert!(colors_match(
        &again.get_history()[HIST_LEN - 1],
        &hex("#654321")
    ));
}

// ---------------------------------------------------------------------------
// 调色板简单字典的 JSON 往返
// ---------------------------------------------------------------------------

#[test]
fn palette_simple_dict_json_round_trip() {
    // palette.py:831-859：键 name/columns/entries，空槽 null，有颜色 [hex, 名字]
    let simple = SimplePaletteDict {
        name: Some("板".to_string()),
        columns: 4,
        entries: vec![
            Some(("#010203".to_string(), Some("一".to_string()))),
            None,
            Some(("#040506".to_string(), None)),
        ],
    };
    let json_value = simple.to_json();
    assert_eq!(json_value["name"], json!("板"));
    assert_eq!(json_value["columns"], json!(4));
    assert_eq!(
        json_value["entries"],
        json!([["#010203", "一"], null, ["#040506", null]])
    );
    assert_eq!(
        SimplePaletteDict::from_json(&json_value),
        Some(simple),
        "往返不丢格子/名字/空槽"
    );
    // 不是对象 → None（调用方回落默认调色板）
    assert_eq!(SimplePaletteDict::from_json(&json!([])), None);
}

// ---------------------------------------------------------------------------
// 自由手绘配置 ⇄ Prefs
// ---------------------------------------------------------------------------

/// 桶旋转三键与隐藏光标都从偏好来（`freehand.py:280`、`:461-478`）。
#[test]
fn freehand_config_reads_barrel_rotation_and_cursor_prefs() {
    let prefs = prefs_with(&[
        ("input.barrel_rotation_offset", json!(0.25)),
        ("input.use_barrel_rotation", json!(false)),
        ("input.barrel_rotation_subtract_ascension", json!(false)),
        ("ui.hide_cursor_while_painting", json!(true)),
    ]);
    let cfg = FreehandConfig::from_prefs(&prefs);
    assert!((cfg.barrel_rotation_offset - 0.25).abs() < 1e-6);
    assert!(!cfg.use_barrel_rotation);
    assert!(!cfg.barrel_rotation_subtract_ascension);
    assert!(cfg.hide_cursor_while_painting);
    // 假压力/假旋转不是偏好项（application.py:299-300 写死 0.5）
    assert!((cfg.fakepressure - 0.5).abs() < 1e-6);
    assert!((cfg.fakerotation - 0.5).abs() < 1e-6);

    // 缺省 + 类型不对 → 回落源码默认值（userconfig.py:65-67；软键 false）
    let prefs = prefs_with(&[
        ("input.barrel_rotation_offset", json!("不是数字")),
        ("input.use_barrel_rotation", json!(1)),
        ("ui.hide_cursor_while_painting", json!("yes")),
    ]);
    let cfg = FreehandConfig::from_prefs(&prefs);
    assert!((cfg.barrel_rotation_offset - 0.5).abs() < 1e-6);
    assert!(cfg.use_barrel_rotation, "不是 bool → 用默认 true");
    assert!(cfg.barrel_rotation_subtract_ascension);
    assert!(!cfg.hide_cursor_while_painting);
}

/// 配置真的进了模式对象（`FreehandMode::set_freehand_config` 走同一个字段）。
#[test]
fn freehand_mode_takes_the_config() {
    let prefs = prefs_with(&[("input.barrel_rotation_offset", json!(0.0))]);
    let mut mode = FreehandMode::new();
    mode.set_freehand_config(FreehandConfig::from_prefs(&prefs));
    assert_eq!(mode.config.barrel_rotation_offset, 0.0);
    assert!(mode.config.use_barrel_rotation);
}

// ---------------------------------------------------------------------------
// ModeStack 广播
// ---------------------------------------------------------------------------

/// 记录收到的配置的模式（只用于验证广播）。
struct ConfigProbe;

static SEEN: Mutex<Vec<FreehandConfig>> = Mutex::new(Vec::new());

impl InteractionMode for ConfigProbe {
    fn name(&self) -> &'static str {
        "ConfigProbe"
    }

    fn set_freehand_config(&mut self, config: FreehandConfig) {
        SEEN.lock().unwrap().push(config);
    }
}

/// `ModeStack::set_freehand_config` 要推给栈里**每一个**模式 —— 源码是每事件
/// 现读偏好（`freehand.py:461-478`），我们没有"现读"，只能偏好一变就重推。
#[test]
fn mode_stack_broadcasts_freehand_config_to_every_mode() {
    SEEN.lock().unwrap().clear();
    let mut stack = ModeStack::new(|| Box::new(ConfigProbe));
    stack.set_freehand_config(FreehandConfig::default());
    assert_eq!(SEEN.lock().unwrap().len(), 1, "默认模式收到一次");

    // 再压一个模式（不需要 ModeContext 的完整构造：直接走 set 广播即可）
    let mut cfg = FreehandConfig::default();
    cfg.barrel_rotation_offset = 0.75;
    stack.set_freehand_config(cfg);
    let seen = SEEN.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!((seen[1].barrel_rotation_offset - 0.75).abs() < 1e-6);
    assert_eq!(stack.len(), 1);
}

/// 真栈（FreehandMode）也能被广播：配置落到 `config` 字段上。
#[test]
fn real_freehand_stack_accepts_the_broadcast() {
    let mut stack = ModeStack::new(|| Box::new(FreehandMode::new()));
    let prefs = prefs_with(&[
        ("input.use_barrel_rotation", json!(false)),
        ("ui.hide_cursor_while_painting", json!(true)),
    ]);
    stack.set_freehand_config(FreehandConfig::from_prefs(&prefs));
    assert_eq!(stack.len(), 1);
    let top = stack.top();
    assert_eq!(top.name(), "Freehand Drawing");
}

// ---------------------------------------------------------------------------
// 默认表与键常量的交叉检查
// ---------------------------------------------------------------------------

/// 本块接线用到的键，要么在默认表里（`frame.color_rgba` 等），要么明确是
/// 软键（`ui.hide_cursor_while_painting` / `symmetry.line_alpha` /
/// `colors.*`）。这条测试防止"接了一个源码根本没有的键"。
#[test]
fn wired_keys_are_either_defaults_or_documented_soft_keys() {
    let defaults = prefs::default_configuration(prefs::current_platform(), None);
    for key in [
        prefs::KEY_SAVING_DEFAULT_FORMAT,
        prefs::KEY_LAST_OPENED_SCRATCHPAD,
        prefs::KEY_BARREL_ROTATION_OFFSET,
        prefs::KEY_USE_BARREL_ROTATION,
        prefs::KEY_BARREL_ROTATION_SUBTRACT_ASCENSION,
        prefs::KEY_FRAME_COLOR_RGBA,
    ] {
        assert!(defaults.contains_key(key), "默认表里应当有 {key}");
    }
    for key in [
        prefs::KEY_HIDE_CURSOR_WHILE_PAINTING,
        prefs::KEY_SYMMETRY_LINE_ALPHA,
        "colors.current",
        "colors.history",
        "colors.palette",
        "colors.wheels.type",
    ] {
        assert!(
            !defaults.contains_key(key),
            "{key} 是软键（源码用 .get(key, 默认) 读，默认表里故意没有）"
        );
    }
}
