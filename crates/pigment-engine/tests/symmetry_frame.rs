//! 对称（B3-5）与画框在**文档层**的状态语义。
//!
//! 这些测试盯的是"状态与传播"，不是 dab 展开（那是
//! `pigment-surface/tests/symmetry.rs` 的事）：
//!
//! | 断言 | 源码出处 |
//! |---|---|
//! | 中央状态的初值 | `lib/layer/tree.py:130-138` |
//! | `set_symmetry_state` 传播给当前层 | `tree.py:913-915` / `:924-934` |
//! | 切层时重新传播 | `tree.py:973-983`（`set_current_path`） |
//! | 新层继承中央状态 | `tree.py:1687`（`deepinsert`） |
//! | 组层吞掉对称（上游行为） | `lib/layer/core.py:851-870`（空实现） |
//! | 画框初值与开关 | `lib/document.py:338-339` / `:864-878` |
//! | `set_frame` 不钳位 | `lib/document.py:846-849`（`int(var)`） |
//! | `clear()` 一并复位 | `lib/document.py:944-961` |
//! | 画框启用时 `get_user_bbox` 直接用它 | `lib/document.py:1229-1241` |

use pigment_engine::document::SymmetrySetup;
use pigment_engine::hokusai::{Brush, BrushSetting, BrushState};
use pigment_engine::stroke::StrokeEvent;
use pigment_engine::Brushwork;
use pigment_engine::{hokusai, Document};
use pigment_surface::symmetry::SymmetryType;

const TS: i32 = hokusai::tile::TILE_SIZE as i32;

fn test_brush() -> Brush {
    let mut b = pigment_engine::brushlib::default_brush();
    b.get_mut(BrushSetting::Radius).base_value = 1.5; // ~4.5px
    b
}

/// 在当前可画层上画一条横线。
///
/// 走的是 app 同一条路径：`Brushwork::stroke_to`（每个事件套一次原子窗口，
/// 对称矩阵就是在 `begin_atomic` 里重算的）→ 收笔 → `do_brushwork` 入栈。
fn paint_segment(doc: &mut Document, x0: f32, x1: f32, y: f32) {
    let brush = test_brush();
    let mut state = BrushState::default();
    let mut bw = Brushwork::new();
    let steps = 30;
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * i as f32 / steps as f32;
        let ev = StrokeEvent::new(0.016, x, y, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        let surface = doc.paintable_surface_mut().expect("当前层应可画");
        bw.stroke_to(surface, &brush, "Test Brush", &mut state, ev);
    }
    let surface = doc.paintable_surface_mut().expect("当前层应可画");
    if bw.stop_recording(surface, &state, false) {
        doc.do_brushwork(bw);
    }
}

/// `(10, 32) → (100, 32)` 的一条横线（bbox 测试用）。
fn paint_line(doc: &mut Document) {
    paint_segment(doc, 10.0, 100.0, 32.0);
}

/// 当前层表面某个模型坐标上的 alpha。
fn ink_alpha(doc: &Document, x: i32, y: i32) -> f32 {
    let Some(surface) = doc.paintable_surface() else {
        return 0.0;
    };
    let Some(tile) = surface.tile(x.div_euclid(TS), y.div_euclid(TS)) else {
        return 0.0;
    };
    let p = tile[y.rem_euclid(TS) as usize][x.rem_euclid(TS) as usize];
    hokusai::fix15::to_f32(p[3])
}

/// 当前层表面的对称参数（"待生效"那一份，等价 C 的 `state_pending`）。
fn surface_symmetry(doc: &Document) -> Option<pigment_surface::symmetry::SymmetryState> {
    doc.paintable_surface().map(|s| s.symmetry_pending())
}

// ---- 中央状态 ----

/// 初值照 `tree.py:130-138`：未设置中心、未开启、垂直镜像、原点、0°、2 条线。
#[test]
fn symmetry_defaults_match_root_layer_stack() {
    let doc = Document::new();
    assert_eq!(
        doc.symmetry(),
        SymmetrySetup {
            active: false,
            unset: true,
            center: (0, 0),
            type_: SymmetryType::Vertical,
            lines: 2,
            angle: 0.0,
        }
    );
    assert!(!doc.symmetry_active());
    assert!(doc.symmetry_unset());
}

/// `set_symmetry_state` 把中央状态复制给当前层的表面（`tree.py:913-915`）。
#[test]
fn set_symmetry_state_propagates_to_current_layer() {
    let mut doc = Document::new();
    doc.set_symmetry_state(
        Some(true),
        Some((100, 50)),
        Some(SymmetryType::Rotational),
        Some(5),
        Some(30.0),
    );
    assert!(doc.symmetry_active());
    assert_eq!(doc.symmetry_center(), (100, 50));
    assert_eq!(doc.symmetry_type(), SymmetryType::Rotational);
    assert_eq!(doc.symmetry_lines(), 5);
    assert_eq!(doc.symmetry_angle(), 30.0);

    let s = surface_symmetry(&doc).expect("当前层应可画");
    assert_eq!(s.type_, SymmetryType::Rotational);
    assert_eq!(s.center_x, 100.0);
    assert_eq!(s.center_y, 50.0);
    assert_eq!(s.num_lines, 5.0);
    assert_eq!(s.angle, 30.0);
    assert!(doc.paintable_surface().unwrap().symmetry_active());

    // `None` = 这一项没变（`tree.py:940-949`）
    doc.set_symmetry_state(Some(false), None, None, None, None);
    assert!(!doc.symmetry_active());
    assert_eq!(doc.symmetry_center(), (100, 50), "没给的项不该被清掉");
    let s = surface_symmetry(&doc).unwrap();
    assert_eq!(s.num_lines, 5.0);
}

/// 切层时中央状态要重新复制给新当前层（`tree.py:973-983`）。
#[test]
fn switching_layers_repropagates_symmetry() {
    let mut doc = Document::new();
    doc.set_symmetry_state(Some(true), Some((7, 8)), Some(SymmetryType::Horizontal), Some(3), Some(15.0));
    // 第二层（新层会继承当前状态），然后切回去
    assert!(doc.add_layer());
    assert_eq!(doc.layers().len(), 2);
    doc.select_layer(&[1]);
    let s = surface_symmetry(&doc).expect("底层可画");
    assert_eq!(s.center_x, 7.0);
    assert_eq!(s.center_y, 8.0);
    doc.select_layer(&[0]);
    let s = surface_symmetry(&doc).expect("新层可画");
    assert_eq!(s.type_, SymmetryType::Horizontal, "切回来的层也该有中央状态");
    assert_eq!(s.num_lines, 3.0);
}

/// 新层插入时吃一份中央状态（`tree.py:1687` 的 `deepinsert`）。
#[test]
fn new_layer_inherits_central_symmetry() {
    let mut doc = Document::new();
    doc.set_symmetry_state(Some(true), Some((40, 40)), Some(SymmetryType::Snowflake), Some(6), Some(10.0));
    assert!(doc.add_layer());
    let s = surface_symmetry(&doc).expect("新当前层可画");
    assert!(doc.paintable_surface().unwrap().symmetry_active());
    assert_eq!(s.type_, SymmetryType::Snowflake);
    assert_eq!(s.num_lines, 6.0);
    assert_eq!(s.center_x, 40.0);
}

/// 组层吞掉对称参数（`core.py:851-870` 的空实现），但不该 panic。
#[test]
fn group_layer_swallows_symmetry_without_panicking() {
    let mut doc = Document::new();
    assert!(doc.add_group());
    assert_eq!(doc.layers().len(), 2);
    doc.set_symmetry_state(Some(true), Some((1, 2)), Some(SymmetryType::Vertical), Some(2), Some(0.0));
    // 中央状态照记（根栈上的数据与当前层类型无关）
    assert!(doc.symmetry_active());
    assert_eq!(doc.symmetry_center(), (1, 2));
    // 当前层是组 → 没有可画的表面，传播是 no-op
    assert!(doc.paintable_surface().is_none());
}

// ---- 绘制路径上的对称（关闭 = 逐像素不变 / 开启 = 两侧都有） ----

/// 对称关闭时，整条文档级绘制路径（文档 → 当前层 → 表面）的像素与
/// "从没碰过对称 API"的文档**逐像素一致**。
///
/// 这是"关闭态零影响"的文档级门禁；表面级的 `draw_dab` 与参考后端
/// `MemSurface` 的全等断言在 `pigment-surface/tests/symmetry.rs`。
#[test]
fn symmetry_off_paints_identical_pixels() {
    let mut pristine = Document::new();
    paint_line(&mut pristine);

    // 走 app 的开关路径：先开、改参数、再关（矩阵真的按 Rotational 算过）
    let mut toggled = Document::new();
    toggled.set_symmetry_state(Some(true), Some((55, 32)), Some(SymmetryType::Rotational), Some(6), Some(25.0));
    toggled.set_symmetry_state(Some(false), None, None, None, None);
    paint_line(&mut toggled);

    let coords = |d: &Document| {
        let mut v: Vec<(i32, i32)> = d
            .paintable_surface()
            .unwrap()
            .tiles()
            .map(|(c, _)| c)
            .collect();
        v.sort_unstable();
        v
    };
    let (a, b) = (coords(&pristine), coords(&toggled));
    assert_eq!(a, b, "已分配 tile 集合不同");
    assert!(!a.is_empty(), "没有画出任何 tile，测试无效");
    for (tx, ty) in a {
        assert_eq!(
            pristine.paintable_surface().unwrap().tile(tx, ty),
            toggled.paintable_surface().unwrap().tile(tx, ty),
            "tile ({tx},{ty}) 像素不一致"
        );
    }
}

/// 对称开启后，笔画在对称轴两侧都留下笔迹；关闭时只有一侧。
#[test]
fn symmetry_on_paints_both_sides() {
    // 笔画只画在 x∈[10,40]，垂直轴在 x=55 → 镜像落在 x∈[70,100]。
    // 取一对关于 x=55 对称的点：25 与 85（模型坐标差 0.5，见表面的像素中心约定）。
    let mut doc = Document::new();
    doc.set_symmetry_state(
        Some(true),
        Some((55, 32)),
        Some(SymmetryType::Vertical),
        Some(2),
        Some(0.0),
    );
    paint_segment(&mut doc, 10.0, 40.0, 32.0);
    let original = ink_alpha(&doc, 25, 32);
    let mirrored = ink_alpha(&doc, 85, 32);
    assert!(original > 0.05, "原始笔画在 (25,32) 应有笔迹，实得 {original}");
    assert!(mirrored > 0.05, "镜像笔画在 (85,32) 应有笔迹，实得 {mirrored}");

    // 关闭对称：右侧没有镜像笔迹
    let mut plain = Document::new();
    plain.set_symmetry_state(
        Some(false),
        Some((55, 32)),
        Some(SymmetryType::Vertical),
        Some(2),
        Some(0.0),
    );
    paint_segment(&mut plain, 10.0, 40.0, 32.0);
    assert!(ink_alpha(&plain, 25, 32) > 0.05);
    assert!(
        ink_alpha(&plain, 85, 32) < 0.01,
        "关闭对称后不该有镜像笔迹：{}",
        ink_alpha(&plain, 85, 32)
    );
}

// ---- 画框 ----

/// 画框初值与开关（`document.py:338-339` / `:864-878`）。
#[test]
fn frame_defaults_and_toggle_state() {
    let mut doc = Document::new();
    assert_eq!(doc.get_frame(), (0, 0, 0, 0), ":338 的 `self._frame`");
    assert!(!doc.get_frame_enabled(), ":339 的 `self._frame_enabled = False`");

    // FrameToggle 的落地端（`gui/document.py:1066-1072`）
    doc.set_frame_enabled(!doc.get_frame_enabled());
    assert!(doc.get_frame_enabled());
    doc.set_frame_enabled(!doc.get_frame_enabled());
    assert!(!doc.get_frame_enabled());
}

/// `set_frame` 不钳位（`document.py:846-849` 的 `int(var)`），
/// 并且画框启用时 `get_user_bbox` 原样返回它（`:1229-1241`）。
#[test]
fn frame_is_stored_verbatim_and_drives_user_bbox() {
    let mut doc = Document::new();
    doc.set_frame(5, -7, 120, 80);
    assert_eq!(doc.get_frame(), (5, -7, 120, 80));

    // 源码只做 int() 转换，没有 max(0, …)；编辑模式的 1px 兜底在
    // framewindow.py:376-386（未移植），所以负值如实保存。
    doc.set_frame(-3, -4, -5, -6);
    assert_eq!(doc.get_frame(), (-3, -4, -5, -6));

    // 关闭画框 → 导出外接框走内容（这里先画一笔，得到非空 data_bbox）
    doc.set_frame(0, 0, 100, 100);
    assert!(!doc.get_frame_enabled());
    paint_line(&mut doc);
    let data = doc.data_bbox();
    assert_eq!(doc.get_user_bbox(), data, "画框关闭时 bbox 不受画框影响");
    assert_ne!(data, (0, 0, 100, 100), "内容 bbox 应与画框不同，测试才有意义");

    // 开启画框 → 导出外接框就是画框本身（内容与画框无关）
    doc.set_frame_enabled(true);
    assert_eq!(doc.get_user_bbox(), (0, 0, 100, 100));
}

/// `clear()` 一并复位画框、分辨率、设置与对称（`document.py:944-961`）。
#[test]
fn clear_resets_frame_resolution_settings_and_symmetry() {
    let mut doc = Document::new();
    doc.set_frame(10, 20, 30, 40);
    doc.set_frame_enabled(true);
    doc.xres = Some(300);
    doc.yres = Some(300);
    doc.settings.insert("k".to_string(), serde_json::json!("v"));
    doc.set_symmetry_state(Some(true), Some((3, 4)), Some(SymmetryType::Rotational), Some(8), Some(45.0));

    doc.clear();

    assert_eq!(doc.get_frame(), (0, 0, 0, 0));
    assert!(!doc.get_frame_enabled());
    assert_eq!(doc.xres, None);
    assert_eq!(doc.yres, None);
    assert!(doc.settings.is_empty());
    assert_eq!(doc.symmetry(), SymmetrySetup::default());
}

