//! 颜色工具模型的行为测试 —— `ColorTool` / `ComponentSlider` / `ColorChanger`
//! 的状态转移、通知与夹取规则（B4-2）。
//!
//! 逐位对照源码数值的向量门禁在 `tests/color_tools_vectors.rs`（fixture 由
//! `tests/tools/gen-color-tools-vectors.py` 用 `ast` 抽真源码算出）。这里补的
//! 是**抽不出向量**的部分：依赖 GTK 事件模型/控件状态的按下-移动-抬起状态
//! 转移、向 adjuster 的通知时机、以及跨模块的组合。每条断言都注了源码出处；
//! 这些期望值是按源码公式手工推导的（不是从引擎里读出来的）。

use std::cell::Cell;
use std::rc::Rc;

use pigment_engine::color::{
    bend_hcy, point_to_amount, scroll_delta, Color, ColorAdjuster, ColorChanger, ColorTool,
    ComponentSlider, DragUpdate, HcyColor, HsvColor, Modifiers, PressOutcome, RgbColor,
    ScrollDirection, SliderKind, UIColor, DEFAULT_MANAGED_COLOR, SCROLL_DELTA,
};

/// 造一个 RGB 颜色。
fn rgb(r: f64, g: f64, b: f64) -> Color {
    Color::Rgb(RgbColor::new(r, g, b))
}

/// 造一个 HSV 颜色。
fn hsv(h: f64, s: f64, v: f64) -> Color {
    Color::Hsv(HsvColor::new(h, s, v))
}

/// 造一个 HCY 颜色。
fn hcy(h: f64, c: f64, y: f64) -> Color {
    Color::Hcy(HcyColor::new(h, c, y))
}

/// 近似相等（只用于手工推导的连续量）。
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

// ---------------------------------------------------------------------------
// 测试替身：一个会数通知次数的 adjuster
// ---------------------------------------------------------------------------

/// 数 `color_updated()` / `color_history_updated()` 被调了几次。
///
/// 源码里这两个是 `ColorAdjuster` 基类的空钩子（`adjbases.py:421-429`），
/// 由管理器逐个调用（`:216-219`、`:264-266`）。
struct SpyAdjuster {
    color_updates: Rc<Cell<u32>>,
    history_updates: Rc<Cell<u32>>,
}

impl ColorAdjuster for SpyAdjuster {
    fn color_updated(&mut self) {
        self.color_updates.set(self.color_updates.get() + 1);
    }

    fn color_history_updated(&mut self) {
        self.history_updates.set(self.history_updates.get() + 1);
    }
}

/// 注册一个 spy，返回两个计数器。
fn add_spy(tool: &mut ColorTool) -> (Rc<Cell<u32>>, Rc<Cell<u32>>) {
    let color_updates = Rc::new(Cell::new(0));
    let history_updates = Rc::new(Cell::new(0));
    tool.add_adjuster(Box::new(SpyAdjuster {
        color_updates: Rc::clone(&color_updates),
        history_updates: Rc::clone(&history_updates),
    }));
    (color_updates, history_updates)
}

// ---------------------------------------------------------------------------
// ColorTool：颜色变化向所有 adjuster 传播（adjbases.py:204-220）
// ---------------------------------------------------------------------------

/// `set_color()` 会通知**每一个**已注册的 adjuster（`adjbases.py:216-219`）。
#[test]
fn set_color_notifies_every_registered_adjuster() {
    let mut tool = ColorTool::new();
    let (c1, h1) = add_spy(&mut tool);
    let (c2, h2) = add_spy(&mut tool);
    assert_eq!(tool.adjuster_count(), 2);

    assert!(tool.set_color(rgb(0.5, 0.25, 0.125)), "换颜色应奏效");
    assert_eq!(c1.get(), 1, "第一个 adjuster 应收到一次 color_updated");
    assert_eq!(c2.get(), 1, "第二个 adjuster 应收到一次 color_updated");
    assert_eq!(h1.get(), 0, "换颜色不该碰历史通知");
    assert_eq!(h2.get(), 0);
    assert_eq!(tool.get_color(), rgb(0.5, 0.25, 0.125));
}

/// 颜色没变时 `set_color()` 直接返回，**不发通知**（`adjbases.py:213-214`
/// 的 `if color == self._color: return`）。
#[test]
fn set_color_skips_notification_when_unchanged() {
    let mut tool = ColorTool::new();
    let (counter, _) = add_spy(&mut tool);
    let current = tool.get_color();
    assert!(!tool.set_color(current), "同一颜色应判为未变");
    assert_eq!(counter.get(), 0, "未变就不该通知（源码先短路再通知）");
}

/// `push_history()` 通知的是 `color_history_updated()`，不是
/// `color_updated()`（`adjbases.py:264-266`）。
#[test]
fn push_history_notifies_history_not_color() {
    let mut tool = ColorTool::new();
    let (color_counter, history_counter) = add_spy(&mut tool);
    tool.push_history(rgb(0.1, 0.2, 0.3));
    assert_eq!(color_counter.get(), 0);
    assert_eq!(history_counter.get(), 1);
    assert_eq!(
        tool.get_previous_color(),
        rgb(0.1, 0.2, 0.3),
        "历史末项 = get_previous_color()（adjbases.py:279-281）"
    );
    assert_eq!(
        tool.get_history().last().copied(),
        Some(rgb(0.1, 0.2, 0.3)),
        "get_history() 的最新一项在末尾（adjbases.py:275-277）"
    );
}

/// `get_color_manager()` 只是只读视图：读到的当前色与工具的当前色一致
/// （对应 `ColorAdjuster.get_color_manager()`，`adjbases.py:387-393`）。
#[test]
fn color_manager_view_matches_tool_color() {
    let mut tool = ColorTool::new();
    tool.set_color(hsv(0.2, 0.4, 0.6));
    assert_eq!(tool.get_color_manager().get_color(), tool.get_color());
    assert_eq!(
        tool.get_color_manager().get_previous_color(),
        tool.get_previous_color()
    );
}

/// 注册表：`add_adjuster()` 返回句柄，`remove_adjuster(handle)` 之后不再收到
/// 通知（`adjbases.py:190-196`）。
#[test]
fn adjuster_registry_add_and_remove() {
    let mut tool = ColorTool::new();
    let (counter, _) = add_spy(&mut tool);
    let handle = tool.add_adjuster(Box::new(ComponentSlider::new(SliderKind::HsvH)));
    assert_eq!(tool.adjuster_count(), 2);

    tool.remove_adjuster(handle);
    assert_eq!(tool.adjuster_count(), 1);
    tool.set_color(rgb(0.2, 0.3, 0.4));
    assert_eq!(counter.get(), 1, "留下的那个仍应收到通知");
}

/// 滑条也能注册成 adjuster（它实现了 `ColorAdjuster`；源码里
/// `SliderColorAdjuster` 就是 `ColorAdjusterWidget` 的子类，
/// `adjbases.py:933-957`）。
#[test]
fn slider_can_be_registered_as_an_adjuster() {
    let mut tool = ColorTool::new();
    for kind in SliderKind::ALL {
        tool.add_adjuster(Box::new(ComponentSlider::new(kind)));
    }
    assert_eq!(tool.adjuster_count(), 9);
    assert!(tool.set_color(rgb(0.5, 0.5, 0.5)));
}

// ---------------------------------------------------------------------------
// 分量滑条：写回的颜色类型与分量（sliders.py:163-305）
// ---------------------------------------------------------------------------

/// 滑条写回的是**该滑条所在空间**的颜色对象 —— 于是中央色的类型跟着变
/// （`lib/color.py:13-18`；`sliders.py:171-174` 的 `RGBColor(color=...)`）。
#[test]
fn slider_writeback_switches_color_type() {
    let managed = hsv(0.5, 0.6, 0.8);
    let [r, g, b] = managed.get_rgb();

    let out = ComponentSlider::new(SliderKind::RgbR).get_color_for_bar_amount(&managed, 0.25);
    assert_eq!(out.class_name(), "RGBColor", "RGB 滑条写回 RGBColor");
    let [or, og, ob] = out.get_rgb();
    assert!(near(or, 0.25));
    assert!(near(og, g) && near(ob, b), "另外两个通道原样带过来");
    assert!(r != 0.25, "前提：原红通道不是 0.25");

    let out =
        ComponentSlider::new(SliderKind::HcyC).get_color_for_bar_amount(&rgb(0.3, 0.6, 0.7), 0.4);
    assert_eq!(out.class_name(), "HCYColor", "HCY 滑条写回 HCYColor");

    // HSV 滑条写回 HSVColor，且读回的分量就是条值
    let out =
        ComponentSlider::new(SliderKind::HsvV).get_color_for_bar_amount(&rgb(0.3, 0.6, 0.7), 0.9);
    assert_eq!(out.class_name(), "HSVColor");
    assert!(near(out.get_hsv()[2], 0.9));
}

/// 彩度滑条的条值上限是 1，而 HCY 的 `c` 合法域是 0..=2 —— 源码就是只能拖到
/// 一半（`sliders.py:271-274` 直接把条值赋给 `c`；`adjbases.py:1080` 把条值
/// 夹到 0..1）。滚轮也一样：`c = 2.0` 滚一格会被夹到 `c = 1.0`。
#[test]
fn hcy_chroma_slider_cannot_reach_full_range() {
    let slider = ComponentSlider::new(SliderKind::HcyC);
    let managed = hcy(0.5, 2.0, 0.5);
    // 读条值：c = 2.0 原样读出来（sliders.py:276-278）
    assert!(near(slider.get_bar_amount_for_color(&managed), 2.0));
    // 条满格只能写到 1.0
    let out = slider.get_color_for_bar_amount(&managed, 1.0);
    let Color::Hcy(out) = out else {
        panic!("HcyC 应写回 HCYColor")
    };
    assert!(near(out.c, 1.0), "条值 1.0 → c = 1.0，到不了 2.0");
    // 滚一格：clamp(2.0 - 0.015, 0, 1) = 1.0
    let out = slider.scroll(&managed, false, ScrollDirection::Up);
    let Color::Hcy(out) = out else {
        panic!("HcyC 应写回 HCYColor")
    };
    assert!(near(out.c, 1.0), "滚轮先夹条值再写分量");
}

// ---------------------------------------------------------------------------
// 滚轮方向（adjbases.py:1069-1077）
// ---------------------------------------------------------------------------

/// 滚轮的四个符号：横条整体取反；`DOWN`/`LEFT` 再取反一次。
///
/// 源码（`adjbases.py:1069-1077`）：
///
/// ```python
/// d = self.SCROLL_DELTA
/// if not self.vertical:
///     d *= -1
/// if event.direction in (Gdk.ScrollDirection.DOWN, Gdk.ScrollDirection.LEFT):
///     d *= -1
/// ```
#[test]
fn scroll_direction_signs_match_source() {
    assert!(near(SCROLL_DELTA, 0.015), "adjbases.py:449");
    // 竖直条：UP/RIGHT/SMOOTH 正向
    for d in [
        ScrollDirection::Up,
        ScrollDirection::Right,
        ScrollDirection::Smooth,
    ] {
        assert!(near(scroll_delta(true, d), 0.015), "竖直 {d:?} 应为 +0.015");
    }
    // 竖直条：DOWN/LEFT 反向
    for d in [ScrollDirection::Down, ScrollDirection::Left] {
        assert!(
            near(scroll_delta(true, d), -0.015),
            "竖直 {d:?} 应为 -0.015"
        );
    }
    // 横向条：先整体取反
    for d in [
        ScrollDirection::Up,
        ScrollDirection::Right,
        ScrollDirection::Smooth,
    ] {
        assert!(
            near(scroll_delta(false, d), -0.015),
            "横向 {d:?} 应为 -0.015"
        );
    }
    // 横向条：DOWN/LEFT 再取反回来
    for d in [ScrollDirection::Down, ScrollDirection::Left] {
        assert!(
            near(scroll_delta(false, d), 0.015),
            "横向 {d:?} 应为 +0.015"
        );
    }
}

/// 滚一格的具体落色：横向 R 通道 UP 减一格，逐位是 `0.3 - 0.015`
/// （`adjbases.py:1069-1083` + `sliders.py:171-174`）。
#[test]
fn one_scroll_step_maps_to_one_delta() {
    let slider = ComponentSlider::new(SliderKind::RgbR);
    let managed = rgb(0.3, 0.6, 0.7);
    let out = slider.scroll(&managed, false, ScrollDirection::Up);
    assert!(
        near(out.get_rgb()[0], 0.3 - 0.015),
        "实际 {}",
        out.get_rgb()[0]
    );
    // 到头后不再越界：条值夹到 0
    let out = slider.scroll(&rgb(0.0, 0.6, 0.7), false, ScrollDirection::Up);
    assert!(near(out.get_rgb()[0], 0.0));
    let out = slider.scroll(&rgb(1.0, 0.6, 0.7), false, ScrollDirection::Down);
    assert!(near(out.get_rgb()[0], 1.0));
}

// ---------------------------------------------------------------------------
// 坐标 → 条值（adjbases.py:1055-1067）
// ---------------------------------------------------------------------------

/// 边框内的像素夹到 0，边框外的夹到 1；竖直条反向。
///
/// 源码：`p = clamp(p - BORDER_WIDTH, 0, len)`，`len = 宽度 - 2*BORDER_WIDTH`，
/// 竖条 `amt = 1 - amt`（`adjbases.py:1057-1067`）。`BORDER_WIDTH = 2`
/// （`:455`）。
#[test]
fn point_to_amount_clamps_to_the_bar() {
    // 横条：长 100，边框 2 → 有效长度 96
    assert!(
        near(point_to_amount(0.0, 100.0, 2.0, false), 0.0),
        "边框里侧"
    );
    assert!(
        near(point_to_amount(2.0, 100.0, 2.0, false), 0.0),
        "边框右沿"
    );
    assert!(near(point_to_amount(50.0, 100.0, 2.0, false), 48.0 / 96.0));
    assert!(
        near(point_to_amount(98.0, 100.0, 2.0, false), 1.0),
        "另一边框"
    );
    assert!(
        near(point_to_amount(200.0, 100.0, 2.0, false), 1.0),
        "越界夹住"
    );
    // 竖条：同一位置给出镜像值
    assert!(near(point_to_amount(98.0, 100.0, 2.0, true), 0.0));
    assert!(near(point_to_amount(2.0, 100.0, 2.0, true), 1.0));
    // 没有边框时就是满宽
    assert!(near(point_to_amount(0.0, 100.0, 0.0, false), 0.0));
    assert!(near(point_to_amount(100.0, 100.0, 0.0, false), 1.0));
    // 滑条控件默认边框 = 2（adjbases.py:455），这里与常量对齐
    assert!(near(pigment_engine::color::BORDER_WIDTH, 2.0));

    // get_color_at_position() 就是"坐标 → 条值 → 颜色"两步（adjbases.py:1019-1022）
    let slider = ComponentSlider::new(SliderKind::RgbR);
    let out = slider.color_at_position(&rgb(0.3, 0.6, 0.7), 98.0, 100.0, 2.0, false);
    assert!(near(out.get_rgb()[0], 1.0), "右端边框 → 条值 1");
    let out = slider.color_at_position(&rgb(0.3, 0.6, 0.7), 2.0, 100.0, 2.0, true);
    assert!(near(out.get_rgb()[0], 1.0), "竖条顶端 → 条值 1（y 轴反向）");
}

// ---------------------------------------------------------------------------
// 拖动阈值（adjbases.py:528-532）
// ---------------------------------------------------------------------------

/// 阈值 = `2 * min(4, gtk-dnd-drag-threshold)`，落在 0..=8。
#[test]
fn drag_threshold_doubles_and_caps_at_eight() {
    assert!(near(ColorChanger::drag_threshold_from_gtk(3), 6.0));
    assert!(
        near(ColorChanger::drag_threshold_from_gtk(10), 8.0),
        "min(4, 10)"
    );
    assert!(near(ColorChanger::drag_threshold_from_gtk(4), 8.0));
    assert!(near(ColorChanger::drag_threshold_from_gtk(1), 2.0));
    assert!(
        near(ColorChanger::drag_threshold_from_gtk(0), 0.0),
        "GTK 拿不到设置时给 0"
    );
    assert!(near(ColorChanger::new(6.0).drag_threshold(), 6.0));
}

// ---------------------------------------------------------------------------
// 按下（adjbases.py:660-702）
// ---------------------------------------------------------------------------

/// 按钮 1 单击：立刻把位置色设为当前色，然后只是"武装"状态（不是拖动）
/// —— 源码在非 drag source 时走到 `:701-702` 的 `return False`。
#[test]
fn press_button1_sets_color_and_arms_state() {
    let mut tool = ColorTool::new();
    let before = tool.get_color();
    let mut changer = ColorChanger::new(8.0);
    let picked = rgb(0.9, 0.1, 0.2);

    let outcome = changer.start_drag(&mut tool, 1, false, (12.0, 34.0), Some(picked));
    assert_eq!(outcome, PressOutcome::ColorSet);
    assert_eq!(tool.get_color(), picked, "单击就落色（:671-672）");
    assert_ne!(tool.get_color(), before);
    assert_eq!(changer.button_down(), Some(1));
    assert_eq!(changer.drag_start_pos(), Some((12.0, 34.0)), ":667");
    assert_eq!(
        changer.drag_start_color(),
        None,
        "非 drag source 不记起点色（:673-676）"
    );
}

/// 按钮 1 单击 + `IS_DRAG_SOURCE`：颜色照设，同时记下起点色，可能起 DnD
/// （`adjbases.py:673-676`，源码这里 `return True`）。
#[test]
fn press_button1_on_drag_source_allows_drag() {
    let mut tool = ColorTool::new();
    let mut changer = ColorChanger::new(8.0);
    changer.is_drag_source = true;
    let picked = rgb(0.1, 0.2, 0.3);
    let outcome = changer.start_drag(&mut tool, 1, false, (0.0, 0.0), Some(picked));
    assert_eq!(outcome, PressOutcome::DragPossible);
    assert_eq!(tool.get_color(), picked, "颜色照样先设上");
    assert_eq!(changer.drag_start_color(), Some(picked));
}

/// 按钮 3 按下：**不动**当前色，只记住弯曲起点色（`adjbases.py:697-699`）。
#[test]
fn press_button3_arms_hcy_bend_without_changing_color() {
    let mut tool = ColorTool::new();
    let before = tool.get_color();
    let mut changer = ColorChanger::new(8.0);
    let picked = hcy(0.25, 0.5, 0.5);

    let outcome = changer.start_drag(&mut tool, 3, false, (5.0, 5.0), Some(picked));
    assert_eq!(outcome, PressOutcome::Armed);
    assert_eq!(tool.get_color(), before, "按钮 3 不改色");
    assert_eq!(changer.drag_start_color(), Some(picked));
}

/// 双击 + `HAS_DETAILS_DIALOG`：清掉按钮状态与弯曲起点，请 GUI 开对话框
/// （`adjbases.py:679-695`）。注意源码**不清** `__drag_start_pos`（`:682-684`
/// 只把 `__button_down` 与 `__drag_start_color` 置 None）。
#[test]
fn press_double_click_requests_details_dialog() {
    let mut tool = ColorTool::new();
    let before = tool.get_color();
    let mut changer = ColorChanger::new(8.0);
    changer.has_details_dialog = true;

    let outcome = changer.start_drag(&mut tool, 1, true, (7.0, 9.0), Some(rgb(1.0, 1.0, 1.0)));
    assert_eq!(outcome, PressOutcome::DetailsDialog);
    assert_eq!(changer.button_down(), None, ":684");
    assert_eq!(changer.drag_start_color(), None, ":684");
    assert_eq!(
        changer.drag_start_pos(),
        Some((7.0, 9.0)),
        "源码不清按下位置"
    );
    assert_eq!(tool.get_color(), before, "对话框确认前不动颜色");
}

/// 双击但控件**没有**详情对话框：不是按钮 1 单击那一支，也不是按钮 ≠ 1
/// 那一支，于是什么都不发生（`adjbases.py:671-699` 三条支路都不进）。
#[test]
fn press_double_click_without_dialog_does_nothing() {
    let mut tool = ColorTool::new();
    let before = tool.get_color();
    let mut changer = ColorChanger::new(8.0);
    let outcome = changer.start_drag(&mut tool, 1, true, (1.0, 2.0), Some(rgb(1.0, 0.0, 0.0)));
    assert_eq!(outcome, PressOutcome::Armed);
    assert_eq!(tool.get_color(), before);
    assert_eq!(changer.button_down(), Some(1), "按钮状态仍被记下（:662）");
    assert_eq!(changer.drag_start_color(), None);
}

// ---------------------------------------------------------------------------
// 移动（adjbases.py:704-791）
// ---------------------------------------------------------------------------

/// 非 drag source 的按钮 1 移动：只给"按位置取色"信号，不自己设色 ——
/// 位置→颜色是控件自己的 `get_color_at_position()`（`:733-737`）。
#[test]
fn motion_button1_samples_position() {
    let mut tool = ColorTool::new();
    let before = tool.get_color();
    let mut changer = ColorChanger::new(8.0);
    changer.start_drag(&mut tool, 1, false, (10.0, 10.0), Some(rgb(0.1, 0.1, 0.1)));
    let after_press = tool.get_color();

    let update = changer.update(
        &mut tool,
        (90.0, 10.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert_eq!(update, DragUpdate::SamplePosition);
    assert_eq!(tool.get_color(), after_press, "模型不猜位置色，交给控件");
    assert_ne!(after_press, before);
}

/// drag source 的按钮 1 移动：超过阈值才起 DnD，且起点色被清掉表示"拖已开始"
/// （`adjbases.py:708-732`）。
#[test]
fn motion_button1_drag_source_begins_drag_past_threshold() {
    let mut tool = ColorTool::new();
    let mut changer = ColorChanger::new(8.0);
    changer.is_drag_source = true;
    let picked = rgb(0.4, 0.5, 0.6);
    changer.start_drag(&mut tool, 1, false, (10.0, 10.0), Some(picked));

    // 阈值是 8：差 6.0 < 8，还不算拖动
    let before = changer.update(
        &mut tool,
        (16.0, 10.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert_eq!(before, DragUpdate::None);
    assert_eq!(changer.drag_start_color(), Some(picked), "还没开始拖");

    // 差 9.0 > 8 → 起 DnD
    let now = changer.update(
        &mut tool,
        (19.0, 10.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert_eq!(now, DragUpdate::BeginDrag);
    assert_eq!(changer.drag_start_color(), None, ":721 起拖前清起点色");

    // 已经在拖了：再移动不再重复发起（`:709-710`）
    let again = changer.update(
        &mut tool,
        (99.0, 10.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert_eq!(again, DragUpdate::None);
}

/// 没有按钮按着时移动什么都不做（`adjbases.py:707`）。
#[test]
fn motion_without_button_does_nothing() {
    let mut tool = ColorTool::new();
    let before = tool.get_color();
    let mut changer = ColorChanger::new(8.0);
    assert_eq!(
        changer.update(
            &mut tool,
            (50.0, 50.0),
            (100.0, 100.0),
            Modifiers::SHIFT,
            false
        ),
        DragUpdate::None
    );
    assert_eq!(tool.get_color(), before);
}

// ---------------------------------------------------------------------------
// 抬起（adjbases.py:793-808）
// ---------------------------------------------------------------------------

/// 抬起点离按下点小于阈值 → 是一次"点击"，位置用**按下点**
/// （`adjbases.py:806-807` 的 `clicked(event, (xa, ya))`）。
#[test]
fn release_within_threshold_is_a_click_on_press_position() {
    let mut tool = ColorTool::new();
    let mut changer = ColorChanger::new(8.0);
    changer.start_drag(&mut tool, 1, false, (10.0, 20.0), Some(rgb(0.1, 0.2, 0.3)));

    let clicked = changer.stop_drag(1, (13.0, 24.0)); // 差 5 < 8
    assert_eq!(clicked, Some((10.0, 20.0)), "用的是按下点");
    assert_eq!(changer.button_down(), None, "状态清空（:796-801）");
    assert_eq!(changer.drag_start_pos(), None);
    assert_eq!(changer.drag_start_color(), None);
}

/// 抬起点离按下点超过阈值 → 不是点击（拖动结束），但状态照样清空。
#[test]
fn release_after_a_real_drag_is_not_a_click() {
    let mut tool = ColorTool::new();
    let mut changer = ColorChanger::new(8.0);
    changer.start_drag(&mut tool, 1, false, (10.0, 10.0), Some(rgb(0.1, 0.2, 0.3)));
    let clicked = changer.stop_drag(1, (40.0, 40.0)); // 差 ≈42 > 8
    assert_eq!(clicked, None);
    assert_eq!(changer.button_down(), None);
}

/// 抬起的是**别的**按钮：整段跳过，状态原样保留（`adjbases.py:795`）。
#[test]
fn release_with_another_button_is_ignored() {
    let mut tool = ColorTool::new();
    let mut changer = ColorChanger::new(8.0);
    changer.start_drag(&mut tool, 1, false, (10.0, 10.0), Some(rgb(0.1, 0.2, 0.3)));
    let clicked = changer.stop_drag(3, (10.0, 10.0));
    assert_eq!(clicked, None);
    assert_eq!(changer.button_down(), Some(1), "按钮 1 还按着");
    assert_eq!(changer.drag_start_pos(), Some((10.0, 10.0)));
}

// ---------------------------------------------------------------------------
// HCY 弯曲：轴选择与夹取（adjbases.py:739-788）
// ---------------------------------------------------------------------------

/// 轴选择：Shift = 彩度，Ctrl = 色相，其余 = 亮度；两个都按时 **Shift 优先**
/// （`adjbases.py:756-763` 是 `if` / `elif` / `else`）。
#[test]
fn bend_axis_selection_prefers_shift() {
    use pigment_engine::color::{bend_axis, BendAxis};
    assert_eq!(bend_axis(Modifiers::SHIFT), BendAxis::Chroma);
    assert_eq!(bend_axis(Modifiers::CONTROL), BendAxis::Hue);
    assert_eq!(bend_axis(Modifiers::NONE), BendAxis::Luma);
    assert_eq!(
        bend_axis(Modifiers {
            shift: true,
            control: true
        }),
        BendAxis::Chroma,
        "源码 if/elif：Shift 赢"
    );
}

/// 色相弯曲向左拖会绕回 0..1（`adjbases.py:777-783` 的 `while h < 0: h += 1`
/// 之后还有 `h % 1.0`）。
#[test]
fn bend_hue_wraps_into_range() {
    // 起点 h=0，Ctrl（色相），向右拖 10 px：dx = sx - ex = -10 → h = 1.1 → 0.1
    let out = bend_hcy(
        hcy(0.0, 0.5, 0.5),
        (50.0, 50.0),
        (60.0, 50.0),
        (100.0, 100.0),
        Modifiers::CONTROL,
        false,
    );
    assert!(near(out.h, 0.1), "实际 {}", out.h);
    // 向左拖 10 px：dx = 10 → p = -10 → h = -0.1 → while 加回 0.9
    let out = bend_hcy(
        hcy(0.0, 0.5, 0.5),
        (50.0, 50.0),
        (40.0, 50.0),
        (100.0, 100.0),
        Modifiers::CONTROL,
        false,
    );
    assert!(near(out.h, 0.9), "实际 {}", out.h);
    // 色相弯曲只动 h
    assert!(near(out.c, 0.5) && near(out.y, 0.5));
}

/// 彩度/亮度弯曲夹到 0..1；色相不夹（环形）。
/// `adjbases.py:773-776`（chroma 的 `clamp(p / size, 0.0, 1.0)`）与
/// `:784-787`（luma 同理）。
#[test]
fn bend_clamps_chroma_and_luma_but_not_hue() {
    // 彩度：满位移 200px、尺寸 100 → 直接顶到上下界
    // 向右拖：dx = sx - ex = -200（源码 `dd = sx - ex`，见 :753）→ p 变大 → 夹到 1
    let out = bend_hcy(
        hcy(0.5, 0.5, 0.5),
        (50.0, 50.0),
        (250.0, 50.0),
        (100.0, 100.0),
        Modifiers::SHIFT,
        false,
    );
    assert!(near(out.c, 1.0), "向右拖把彩度顶到 1，实际 {}", out.c);
    // 向左拖：dx = +200 → p = 50 - 200 = -150 → 夹到 0
    let out = bend_hcy(
        hcy(0.5, 0.5, 0.5),
        (50.0, 50.0),
        (-150.0, 50.0),
        (100.0, 100.0),
        Modifiers::SHIFT,
        false,
    );
    assert!(
        near(out.c, 0.0),
        "向左拖把彩度压到 0（不是 HCY 的 2），实际 {}",
        out.c
    );

    // 亮度：同一位移方向对应 y 的 1 / 0
    let out = bend_hcy(
        hcy(0.5, 0.5, 0.5),
        (50.0, 50.0),
        (250.0, 50.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert!(near(out.y, 1.0), "实际 {}", out.y);
    let out = bend_hcy(
        hcy(0.5, 0.5, 0.5),
        (50.0, 50.0),
        (-150.0, 50.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert!(near(out.y, 0.0), "实际 {}", out.y);
}

/// 位移取绝对值大的那个轴，相等时取 `dy`（`adjbases.py:769-771`：
/// `dd = dx if abs(dx) > abs(dy) else dy` 是**严格**大于）。
#[test]
fn bend_picks_the_larger_displacement() {
    // dy 更大：dx=5, dy=-20。亮度分支先 dy = -dy = 20。
    // y0=0.5、size=100：p = 50 - 20 = 30 → y = 0.3（若错取 dx 会是 0.45）
    let out = bend_hcy(
        hcy(0.5, 0.5, 0.5),
        (50.0, 50.0),
        (55.0, 70.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert!(near(out.y, 0.3), "实际 {}（说明取的是 dy）", out.y);
    // 相等时取 dy：dx=10, dy=10（向右下拖）→ 亮度分支把 dy 取反成 -10，
    // 绝对值与 dx 相等，`abs(dx) > abs(dy)` 不成立 → dd = dy = -10
    // p = 50 + 10 = 60 → y = 0.6（若错取 dx 会是 0.4）
    let out = bend_hcy(
        hcy(0.5, 0.5, 0.5),
        (50.0, 50.0),
        (40.0, 40.0),
        (100.0, 100.0),
        Modifiers::NONE,
        false,
    );
    assert!(near(out.y, 0.6), "实际 {}（相等时应取 dy）", out.y);
}

/// RTL 文本方向把 `dx` 镜像（`adjbases.py:766-767`）。
#[test]
fn bend_mirrors_dx_in_rtl() {
    let args = (
        (50.0, 50.0),
        (60.0, 50.0),
        (100.0, 100.0),
        Modifiers::CONTROL,
    );
    let ltr = bend_hcy(hcy(0.5, 0.5, 0.5), args.0, args.1, args.2, args.3, false);
    let rtl = bend_hcy(hcy(0.5, 0.5, 0.5), args.0, args.1, args.2, args.3, true);
    // dx = 50-60 = -10：LTR → p = 50+10 = 60 → h = 0.6
    // RTL 把 dx 变 +10 → p = 50-10 = 40 → h = 0.4
    assert!(near(ltr.h, 0.6), "LTR 实际 {}", ltr.h);
    assert!(near(rtl.h, 0.4), "RTL 实际 {}", rtl.h);
}

/// 弯曲结果**总是** `HCYColor`，且用的是控件长边（`max(w, h)`）当比例尺
/// （`adjbases.py:747-750`）。
#[test]
fn bend_uses_the_longer_widget_side() {
    // 尺寸 120x80 → 比例尺 120。y0=0.5，向右拖 12px：dd = 50-72 = -12
    // p = 0.5*120 + 12 = 72 → y = 72/120 = 0.6
    // （若错用 80 当比例尺会是 p = 40+12 = 52 → y = 0.65）
    let out = bend_hcy(
        hcy(0.5, 0.5, 0.5),
        (60.0, 40.0),
        (72.0, 40.0),
        (120.0, 80.0),
        Modifiers::NONE,
        false,
    );
    assert!(
        near(out.y, 0.6),
        "实际 {}（应拿 120 而不是 80 当比例尺）",
        out.y
    );
    // 起点色是 RGB 时先转成 HCY 再弯（`:747` 的 `HCYColor(color=...)`）
    let start = rgb(0.2, 0.4, 0.9);
    let expected = HcyColor::from_color(&start);
    let out = bend_hcy(
        start,
        (60.0, 40.0),
        (48.0, 40.0), // 向左 12px：dd = +12 → 分量从基准值往下走
        (120.0, 80.0),
        Modifiers::NONE,
        false,
    );
    assert!(
        near(out.y, (expected.y * 120.0 - 12.0) / 120.0),
        "实际 {}，起点 y={}",
        out.y,
        expected.y
    );
    assert!(
        near(out.h, expected.h) && near(out.c, expected.c),
        "只动被弯的那个分量"
    );
}

/// 弯曲里"彩度先夹到 0..1 再当比例尺"这条（`adjbases.py:773-776`）：
/// c=2.0 的起点实际按 c0=1.0 起算，所以往左拖会立刻从 1.0 往下走。
#[test]
fn bend_chroma_scales_from_the_clamped_value() {
    let out = bend_hcy(
        hcy(0.5, 2.0, 0.5),
        (50.0, 50.0),
        (60.0, 50.0), // 向右 10px：dx=-10 → dd=-10
        (100.0, 100.0),
        Modifiers::SHIFT,
        false,
    );
    // c0 = clamp(2.0, 0, 1) = 1.0；p = 100 + 10 = 110 → c = 1.0（夹住）
    assert!(near(out.c, 1.0), "实际 {}", out.c);
    let out = bend_hcy(
        hcy(0.5, 2.0, 0.5),
        (50.0, 50.0),
        (40.0, 50.0), // 向左 10px：dx=+10 → p = 90 → c = 0.9
        (100.0, 100.0),
        Modifiers::SHIFT,
        false,
    );
    assert!(near(out.c, 0.9), "实际 {}（从 1.0 起算而不是 2.0）", out.c);
}

// ---------------------------------------------------------------------------
// 常量与源码兜底色
// ---------------------------------------------------------------------------

/// 没有管理器时的兜底色 —— `adjbases.py:371`
/// `_DEFAULT_COLOR = RGBColor(0.0, 0.19, 0.33)`。
#[test]
fn default_managed_color_matches_source() {
    assert!(near(DEFAULT_MANAGED_COLOR.r, 0.0));
    assert!(near(DEFAULT_MANAGED_COLOR.g, 0.19));
    assert!(near(DEFAULT_MANAGED_COLOR.b, 0.33));
}

/// 滑条种类的源码类名能往返（fixture 就是按类名索引的）。
#[test]
fn slider_kind_names_round_trip() {
    for kind in SliderKind::ALL {
        assert_eq!(
            SliderKind::from_class_name(kind.class_name()),
            Some(kind),
            "{} 反查失败",
            kind.class_name()
        );
    }
    assert_eq!(
        SliderKind::from_class_name("YCbCrLumaSlider"),
        None,
        "源码没有 YCbCr 滑条"
    );
    // 每个滑条写回的颜色类型（sliders.py 里 `RGBColor(color=...)` 等）
    for kind in SliderKind::ALL {
        assert_eq!(ComponentSlider::new(kind).kind(), kind);
        let out = ComponentSlider::new(kind).get_color_for_bar_amount(&rgb(0.5, 0.5, 0.5), 0.5);
        assert_eq!(out.class_name(), kind.color_class_name());
    }
    assert_eq!(SliderKind::RgbB.color_class_name(), "RGBColor");
    assert_eq!(SliderKind::HsvS.color_class_name(), "HSVColor");
    assert_eq!(SliderKind::HcyY.color_class_name(), "HCYColor");
}
