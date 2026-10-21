//! 直线/曲线/椭圆模式 —— `gui/linemode.py` 的移植。
//!
//! 源码结构（`LineModeBase`，`linemode.py:233`）把三种几何图形的逻辑都塞在基类里、
//! 由 `line_mode` 字符串分派；本移植照此保留一个 [`LineMode`] + [`LineKind`]，
//! 这样 `process_line` 的分支顺序与源码一一对应。
//!
//! 覆盖（B3-2）：
//! - `StraightMode`（直线）与 `SequenceMode`（首尾相连的直线段）：
//!   `dynamic_straight_line`（`linemode.py:616`）—— 入笔/中点/中点/收尾四个采样点；
//! - `EllipseMode`：`dynamic_ellipse`（`:634`）+ `ellipse_rotation_angle`（`:597`）
//!   + 椭圆数学（`:937-964`）；
//! - 约束：`Ctrl`/`Shift` 把方向吸到 15° 的整数倍（`constrain_to_angle`）；
//! - 每次重画都是「回滚 + 从头画」：`brush_prep`（`:768`）先 `brushwork_rollback`
//!   再把笔刷以零压力送到起点，所以拖动过程中画布上只会留下**一条**线。
//!
//! 仍未做：
//! - `CurveLine1/CurveLine2`（贝塞尔曲线，靠 `Shift` 对上一笔做"改线"）需要图层的
//!   `get_last_stroke_info()`（笔画身份），我们没有这个信息 → 曲线分支未移植，
//!   见到 `CurveLine*` 时按直线处理。
//!
//! 已接：`LineModeOptionsWidget` / 压力-距离曲线（见
//! [`LineModeSettings::line_settings`]）；`slow_tracking` 在画线期间临时清零、
//! 收笔/离开模式时恢复（`linemode.py:400-405`、`:525-530`）。

use super::{
    Behavior, BrushworkState, DragMode, InteractionMode, Key, ModeContext, Modifiers, PointerEvent,
};

/// 直线工具的五个可调参数（`linemode.py:39-85` 的 `_LINE_MODE_SETTINGS_LIST`）。
///
/// 默认值照搬：入笔 0.3 / 中点 1.0 / 收尾 0.3 / 头部 0.25 / 尾部 0.75。
#[derive(Clone, Copy, Debug)]
pub struct LineModeSettings {
    pub entry_pressure: f32,
    pub midpoint_pressure: f32,
    pub exit_pressure: f32,
    /// 领入段结束位置（占总长的比例）
    pub head: f32,
    /// 尾段开始位置（占总长的比例）
    pub tail: f32,
}

impl Default for LineModeSettings {
    fn default() -> Self {
        Self {
            entry_pressure: 0.3,
            midpoint_pressure: 1.0,
            exit_pressure: 0.3,
            head: 0.25,
            tail: 0.75,
        }
    }
}

impl LineModeSettings {
    /// `line_settings()`（`linemode.py:814`）：返回
    /// `(entry, mid, prange1, prange2, head, tail)`。
    ///
    /// `head == 0.0001` 时入笔压力退化成中点压力（源码特例）。
    pub fn line_settings(&self) -> (f32, f32, f32, f32, f32, f32) {
        let mut p1 = self.entry_pressure;
        let p2 = self.midpoint_pressure;
        let p3 = self.exit_pressure;
        if self.head == 0.0001 {
            p1 = p2;
        }
        (p1, p2, p2 - p1, p3 - p2, self.head, self.tail)
    }
}

/// 压力-距离曲线的 4 个控制点（`linemode.py:146-195` 的映射）。
///
/// 坐标是归一化的 `(x, y)`，`y` 已按源码翻转（`value = 1 - pressure`
/// 供 GTK 的向下坐标系）；本移植在绘制时直接按向下坐标使用。
pub fn pressure_curve_points(s: &LineModeSettings) -> [(f32, f32); 4] {
    [
        (0.0, 1.0 - s.entry_pressure),
        (s.head, 1.0 - s.midpoint_pressure),
        (s.tail, 1.0 - s.midpoint_pressure),
        (1.0, 1.0 - s.exit_pressure),
    ]
}

/// 拖动曲线第 `idx` 个控制点后写回设置（`_changed_cb`，同坐标映射）。
pub fn apply_curve_point(s: &mut LineModeSettings, idx: usize, x: f32, y: f32) {
    let pressure = (1.0 - y).clamp(0.0001, 1.0);
    match idx {
        0 => s.entry_pressure = pressure,
        1 => {
            let tail = s.tail.max(0.0001);
            s.head = x.clamp(0.0001, (tail - 0.0001).max(0.0001));
            s.midpoint_pressure = pressure;
        }
        2 => {
            let head = s.head.max(0.0001);
            s.tail = x.clamp((head + 0.0001).min(0.9999), 0.9999);
        }
        3 => s.exit_pressure = pressure,
        _ => {}
    }
}

/// 要画哪种几何图形（源码的 `line_mode` 字符串）。/// 要画哪种几何图形（源码的 `line_mode` 字符串）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    /// 两点之间的直线（`StraightMode`）
    Straight,
    /// 接在上一笔末端的直线（`SequenceMode`）
    Sequence,
    /// 椭圆/圆（`EllipseMode`）
    Ellipse,
}

impl LineKind {
    pub fn action_name(self) -> &'static str {
        match self {
            LineKind::Straight => "StraightMode",
            LineKind::Sequence => "SequenceMode",
            LineKind::Ellipse => "EllipseMode",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            LineKind::Straight => "Lines and Curves",
            LineKind::Sequence => "Connected Lines",
            LineKind::Ellipse => "Ellipses and Circles",
        }
    }

    pub fn usage(self) -> &'static str {
        match self {
            LineKind::Straight => "Draw straight or curved lines",
            LineKind::Sequence => "Draw sequences of straight lines",
            LineKind::Ellipse => "Draw ellipses; Shift rotates, Ctrl constrains ratio/angle",
        }
    }
}

// ---- 向量/几何工具（`linemode.py:913-1076`，逐函数照搬）----

/// 两点差 `(x2-x1, y2-y1)`。
#[inline]
pub fn difference(x1: f32, y1: f32, x2: f32, y2: f32) -> (f32, f32) {
    (x2 - x1, y2 - y1)
}

/// 向量长度。
#[inline]
pub fn vector_length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

/// 两点距离。
#[inline]
pub fn distance(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let (x, y) = difference(x1, y1, x2, y2);
    vector_length(x, y)
}

/// 向量数乘。
#[inline]
pub fn multiply(x: f32, y: f32, d: f32) -> (f32, f32) {
    (x * d, y * d)
}

/// 向量相加。
#[inline]
pub fn add(x1: f32, y1: f32, x2: f32, y2: f32) -> (f32, f32) {
    (x1 + x2, y1 + y2)
}

/// `p1 + p2 * d`。
#[inline]
pub fn multiply_add(x1: f32, y1: f32, x2: f32, y2: f32, d: f32) -> (f32, f32) {
    let (x3, y3) = multiply(x2, y2, d);
    add(x1, y1, x3, y3)
}

/// 两点中点。
#[inline]
pub fn midpoint(x1: f32, y1: f32, x2: f32, y2: f32) -> (f32, f32) {
    ((x1 + x2) / 2.0, (y1 + y2) / 2.0)
}

/// 点积。
#[inline]
pub fn dot_product(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    x1 * x2 + y1 * y2
}

/// 垂直向量（交换分量并翻一个符号）。
#[inline]
pub fn perpendicular(x1: f32, y1: f32) -> (f32, f32) {
    (-y1, x1)
}

/// 长度 + 单位向量（长度为 0 时单位向量给 (0,0)）。
pub fn length_and_normal(x1: f32, y1: f32, x2: f32, y2: f32) -> (f32, f32, f32) {
    let (mut x, mut y) = difference(x1, y1, x2, y2);
    let length = vector_length(x, y);
    if length == 0.0 {
        x = 0.0;
        y = 0.0;
    } else {
        x /= length;
        y /= length;
    }
    (length, x, y)
}

/// 单位向量。
#[inline]
pub fn normal(x1: f32, y1: f32, x2: f32, y2: f32) -> (f32, f32) {
    let (_, x, y) = length_and_normal(x1, y1, x2, y2);
    (x, y)
}

/// 把一个角度吸到 15° 的整数倍（就近取整；`constraint_angle`）。
pub fn constraint_angle(angle: f32) -> f32 {
    let n = (angle / 15.0).floor();
    let rem = angle - n * 15.0;
    if rem < 7.5 {
        n * 15.0
    } else {
        (n + 1.0) * 15.0
    }
}

/// 由单位向量的 y 分量与角度还原单位向量（`angle_normal`）。
pub fn angle_normal(ny: f32, angle: f32) -> (f32, f32) {
    let angle = if ny < 0.0 { 360.0 - angle } else { angle };
    let rad = angle.to_radians();
    (rad.cos(), rad.sin())
}

/// 把 `(x, y)` 相对起点的方向吸附到 15° 的整数倍，长度不变。
pub fn constrain_to_angle(x: f32, y: f32, sx: f32, sy: f32) -> (f32, f32) {
    let (length, nx, ny) = length_and_normal(sx, sy, x, y);
    let angle = nx.clamp(-1.0, 1.0).acos().to_degrees();
    let angle = constraint_angle(angle);
    let (ax, ay) = angle_normal(ny, angle);
    (sx + ax * length, sy + ay * length)
}

/// 两向量的夹角（度；点积异常时给 0）。
pub fn get_angle(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dot = dot_product(x1, y1, x2, y2);
    if dot.abs() < 1.0 {
        dot.clamp(-1.0, 1.0).acos().to_degrees()
    } else {
        0.0
    }
}

/// 椭圆起点（按旋转角旋转后的向量 + 该角的 sin/cos）。
pub fn starting_point_for_ellipse(x: f32, y: f32, rotate: f32) -> (f32, f32, f32, f32) {
    let r = rotate.to_radians();
    let sin = r.sin();
    let cos = r.cos();
    let (x, y) = rotate_ellipse(x, y, cos, sin);
    (x, y, sin, cos)
}

/// 椭圆上的点（`degree` 是参数角，0..360；`r_sin/r_cos` 是旋转角的正余弦）。
pub fn point_in_ellipse(x: f32, y: f32, r_sin: f32, r_cos: f32, degree: f32) -> (f32, f32) {
    let r2 = degree.to_radians();
    let cos = r2.cos();
    let sin = r2.sin();
    rotate_ellipse(y * sin, x * cos, r_sin, r_cos)
}

/// 旋转（`rotate_ellipse`，逐行照搬）。
#[inline]
pub fn rotate_ellipse(x: f32, y: f32, sin: f32, cos: f32) -> (f32, f32) {
    let (x1, y1) = multiply(x, y, sin);
    let (x2, y2) = multiply(x, y, cos);
    (x2 - y1, y2 + x1)
}

/// 直线模式（`LineModeBase`）。
pub struct LineMode {
    pub kind: LineKind,
    pub settings: LineModeSettings,
    /// 拖拽基础设施（`DragMode`：弹簧加载 + 修饰键记录）
    pub drag: DragMode,
    /// brushwork 基础设施
    pub brushwork: BrushworkState,
    /// 起笔点
    sx: f32,
    sy: f32,
    /// 最近一次更新到的点
    lx: f32,
    ly: f32,
    /// 终点（曲线/椭圆用）
    ex: f32,
    ey: f32,
    /// 椭圆旋转角（度，初始 90）
    angle: f32,
    /// 旋转测量基准向量
    ellipse_vec: Option<(f32, f32)>,
    last_angle: f32,
    /// 起手时的修饰键（从本笔的修饰键里异或掉，避免它又触发别的分支）
    invert_modifiers: Modifiers,
    /// 当前层可画（`_line_possible`）
    line_possible: bool,
    /// 视图缩放/旋转快照（画笔事件要用）
    viewzoom: f32,
    viewrotation: f32,
}

impl LineMode {
    pub fn new(kind: LineKind) -> Self {
        Self {
            kind,
            settings: LineModeSettings::default(),
            drag: DragMode {
                // 键盘快捷键起手时忽略修饰键（`LineModeBase.__init__` 传下来的）
                ignore_modifiers: false,
                ..DragMode::default()
            },
            brushwork: BrushworkState::default(),
            sx: 0.0,
            sy: 0.0,
            lx: 0.0,
            ly: 0.0,
            ex: 0.0,
            ey: 0.0,
            angle: 90.0,
            ellipse_vec: None,
            last_angle: 90.0,
            invert_modifiers: Modifiers::NONE,
            line_possible: false,
            viewzoom: 1.0,
            viewrotation: 0.0,
        }
    }

    /// 更新「当前层是否可画」（`_update_cursors`）。
    fn refresh_possible(&mut self, ctx: &ModeContext<'_>) {
        self.line_possible = ctx.current_is_paintable();
    }

    /// 开一条线（`start_command`，`linemode.py:371`）。
    fn start_command(
        &mut self,
        ctx: &mut ModeContext<'_>,
        ev: &PointerEvent,
        modifiers: Modifiers,
    ) {
        self.viewzoom = ev.viewzoom;
        self.viewrotation = ev.viewrotation;
        self.brushwork
            .begin(ctx, false, Some(self.kind.display_name().to_string()));
        // 起手时按住的修饰键不再参与判断（源码 `kbmods ^= invert_kbmods`）
        self.invert_modifiers = modifiers;
        self.sx = ev.x;
        self.sy = ev.y;
        // SequenceMode（Connected Lines）：起点接上一笔终点
        // （`linemode.py:447-451`）。首笔没有 last_painting_pos 时从按下点起。
        if self.kind == LineKind::Sequence {
            if let Some((px, py)) = ctx.doc.last_painting_pos {
                self.sx = px;
                self.sy = py;
            }
        }
        self.lx = ev.x;
        self.ly = ev.y;
        self.ex = ev.x;
        self.ey = ev.y;
        if self.kind == LineKind::Ellipse {
            self.angle = 90.0;
            self.ellipse_vec = None;
            self.last_angle = 90.0;
        }
    }

    /// 更新位置（`update_position`）：只记最后点，真正的绘制在 `process_line`。
    fn update_position(&mut self, ev: &PointerEvent) {
        self.lx = ev.x;
        self.ly = ev.y;
    }

    /// 收笔（`stop_command`）。
    fn stop_command(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent, modifiers: Modifiers) {
        let (x, y) = self.process_line(ctx, ev.x, ev.y, modifiers);
        self.brushwork.commit(ctx, false);
        // `record_last_stroke` 里除了"记住上一笔以便 Shift 改线"以外还有：
        // 把光标位置记到末点、恢复 slow_tracking、重置笔刷。
        // `linemode.py:525-530`：直线/序列记终点，椭圆记起点。
        ctx.doc.last_painting_pos = if self.kind == LineKind::Ellipse {
            Some((self.sx, self.sy))
        } else {
            Some((x, y))
        };
        self.ex = x;
        self.ey = y;
        // `linemode.py` calls `brush.reset()`, which is libmypaint's
        // brush_reset: states and smudge buckets are cleared but the PRNG
        // stream survives. `BrushState::reset` mirrors that; assigning a
        // fresh default would restart the RNG at seed 1000.
        ctx.brush_state.reset();
    }

    /// 算并画出当前这条线（`process_line`，`linemode.py:550`）。
    ///
    /// 返回最终用的终点（源码的返回值）。
    pub fn process_line(
        &mut self,
        ctx: &mut ModeContext<'_>,
        x: f32,
        y: f32,
        modifiers: Modifiers,
    ) -> (f32, f32) {
        let (sx, sy) = (self.sx, self.sy);
        // 起手时的修饰键被异或掉
        let kb = Modifiers {
            shift: modifiers.shift ^ self.invert_modifiers.shift,
            ctrl: modifiers.ctrl ^ self.invert_modifiers.ctrl,
            alt: modifiers.alt ^ self.invert_modifiers.alt,
        };
        let mut x = x;
        let mut y = y;
        match self.kind {
            LineKind::Ellipse => {
                let mut constrain = false;
                if kb.ctrl {
                    let (cx, cy) = constrain_to_angle(x, y, sx, sy);
                    x = cx;
                    y = cy;
                    constrain = true;
                }
                if kb.shift {
                    self.ellipse_rotation_angle(x, y, sx, sy, constrain);
                } else {
                    self.ellipse_vec = None;
                }
                self.dynamic_ellipse(ctx, x, y, sx, sy);
            }
            LineKind::Straight | LineKind::Sequence => {
                if kb.ctrl || kb.shift {
                    let (cx, cy) = constrain_to_angle(x, y, sx, sy);
                    x = cx;
                    y = cy;
                }
                self.dynamic_straight_line(ctx, x, y, sx, sy);
            }
        }
        self.ex = x;
        self.ey = y;
        (x, y)
    }

    /// 椭圆旋转角（`ellipse_rotation_angle`，`linemode.py:597`）。
    fn ellipse_rotation_angle(&mut self, x: f32, y: f32, sx: f32, sy: f32, constrain: bool) {
        let (x1, y1) = normal(sx, sy, x, y);
        if self.ellipse_vec.is_none() {
            self.ellipse_vec = Some((x1, y1));
            self.last_angle = self.angle;
        }
        let (x2, y2) = self.ellipse_vec.unwrap_or((x1, y1));
        let (px, py) = perpendicular(x2, y2);
        let pangle = get_angle(x1, y1, px, py);
        let mut angle = get_angle(x1, y1, x2, y2);
        if pangle > 90.0 {
            angle = 360.0 - angle;
        }
        angle += self.last_angle;
        if constrain {
            angle = constraint_angle(angle);
        }
        self.angle = angle;
    }

    /// 直线（`dynamic_straight_line`，`linemode.py:616`）：四个采样点。
    fn dynamic_straight_line(
        &mut self,
        ctx: &mut ModeContext<'_>,
        x: f32,
        y: f32,
        sx: f32,
        sy: f32,
    ) {
        self.brush_prep(ctx, sx, sy);
        let (entry_p, midpoint_p, _prange1, _prange2, head, tail) = self.settings.line_settings();
        let (length, nx, ny) = length_and_normal(sx, sy, x, y);
        // 起笔（1/4 处，入笔压力）
        let (mx, my) = multiply_add(sx, sy, nx, ny, 0.25);
        self.stroke_point(ctx, mx, my, entry_p);
        // 领入段结束（中点压力）
        let (mx, my) = multiply_add(sx, sy, nx, ny, head * length);
        self.stroke_point(ctx, mx, my, midpoint_p);
        // 尾段开始（中点压力）
        let (mx, my) = multiply_add(sx, sy, nx, ny, tail * length);
        self.stroke_point(ctx, mx, my, midpoint_p);
        // 终点（收尾压力）
        self.stroke_point(ctx, x, y, self.settings.exit_pressure);
    }

    /// 椭圆（`dynamic_ellipse`，`linemode.py:634`）：360 个采样点，
    /// 头部与尾部各按压力渐变。
    fn dynamic_ellipse(&mut self, ctx: &mut ModeContext<'_>, x: f32, y: f32, sx: f32, sy: f32) {
        const POINTS: f32 = 360.0;
        let (x1, y1) = difference(sx, sy, x, y);
        let (x1, y1, sin, cos) = starting_point_for_ellipse(x1, y1, self.angle);
        let (rx, ry) = point_in_ellipse(x1, y1, sin, cos, 0.0);
        self.brush_prep(ctx, sx + rx, sy + ry);
        let (entry_p, midpoint_p, prange1, prange2, h, t) = self.settings.line_settings();
        let head = POINTS * h;
        let head_range = head as i32 + 1;
        let tail = POINTS * t;
        let tail_range = tail as i32 + 1;
        let tail_length = POINTS - tail;
        // 起点
        let (px, py) = point_in_ellipse(x1, y1, sin, cos, 1.0);
        let (_, nx, ny) = length_and_normal(rx, ry, px, py);
        let (mx, my) = multiply_add(rx, ry, nx, ny, 0.25);
        self.stroke_point(ctx, sx + mx, sy + my, entry_p);
        let pressure = (1.0 / head * prange1 + entry_p).abs();
        self.stroke_point(ctx, sx + px, sy + py, pressure);
        for degree in 2..head_range {
            let d = degree as f32;
            let (px, py) = point_in_ellipse(x1, y1, sin, cos, d);
            let pressure = (d / head * prange1 + entry_p).abs();
            self.stroke_point(ctx, sx + px, sy + py, pressure);
        }
        for degree in head_range..tail_range {
            let (px, py) = point_in_ellipse(x1, y1, sin, cos, degree as f32);
            self.stroke_point(ctx, sx + px, sy + py, midpoint_p);
        }
        for degree in tail_range..=(POINTS as i32) {
            let d = degree as f32;
            let (px, py) = point_in_ellipse(x1, y1, sin, cos, d);
            let pressure = ((d - tail) / tail_length * prange2 + midpoint_p).abs();
            self.stroke_point(ctx, sx + px, sy + py, pressure);
        }
    }

    /// 把笔刷送到起点并把画面回滚到这一笔之前（`brush_prep`，`linemode.py:768`）。
    fn brush_prep(&mut self, ctx: &mut ModeContext<'_>, sx: f32, sy: f32) {
        // `self.model.brush.reset()` keeps the PRNG stream; see stop_command.
        ctx.brush_state.reset();
        self.brushwork.rollback(ctx);
        // 零压力送到起点（dtime=10.0，源码如此）
        self.brushwork.stroke_to(
            ctx,
            10.0,
            sx,
            sy,
            0.0,
            0.0,
            0.0,
            self.viewzoom,
            self.viewrotation,
            0.0,
            false,
        );
    }

    /// 画一个采样点（`_stroke_to`，`linemode.py:750`：dtime 固定 1.0、不切段）。
    fn stroke_point(&mut self, ctx: &mut ModeContext<'_>, x: f32, y: f32, pressure: f32) {
        self.brushwork.stroke_to(
            ctx,
            1.0,
            x,
            y,
            pressure.clamp(0.0, 1.0),
            0.0,
            0.0,
            self.viewzoom,
            self.viewrotation,
            0.0,
            false,
        );
    }
}

impl InteractionMode for LineMode {
    fn name(&self) -> &'static str {
        self.kind.display_name()
    }

    fn usage(&self) -> &str {
        self.kind.usage()
    }

    /// 直线/序列/椭圆三种模式也必须报出自己的 `resources.xml` 动作 id。
    ///
    /// 缺了它，工具条的选中态、底栏“工具”标签、`action_checked` 和外部
    /// 自动化都只能看到一个匿名的 `LineMode`（`action_name() == None`）。
    fn action_name(&self) -> Option<&'static str> {
        Some(self.kind.action_name())
    }

    fn set_line_settings(&mut self, settings: LineModeSettings) {
        self.settings = settings;
    }

    fn pointer_behavior(&self) -> Behavior {
        Behavior::PAINT_CONSTRAINED
    }

    fn scroll_behavior(&self) -> Behavior {
        Behavior::CHANGE_VIEW
    }

    fn stackable_on(&self, _mode: &dyn InteractionMode) -> bool {
        // 源码 `permitted_switch_actions` 是一个白名单（视图/笔刷类的临时模式），
        // 但没有覆盖 `stackable_on`，所以走默认实现。
        false
    }

    fn is_painting(&self) -> bool {
        self.brushwork.is_active()
    }

    fn enter(&mut self, ctx: &mut ModeContext<'_>) {
        self.brushwork = BrushworkState::default();
        self.drag = DragMode::default();
        self.refresh_possible(ctx);
    }

    fn leave(&mut self, ctx: &mut ModeContext<'_>) {
        self.drag.drag.stop();
        self.brushwork.commit(ctx, true);
        self.line_possible = false;
    }

    fn checkpoint(&mut self, ctx: &mut ModeContext<'_>) {
        self.brushwork.commit(ctx, false);
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 {
            return false;
        }
        self.refresh_possible(ctx);
        if !self.line_possible {
            return false;
        }
        // 弹簧加载：把进入时的修饰键记下来（`DragMode.enter`）
        self.drag.enter(ev.modifiers);
        self.drag.drag.start(ev.x, ev.y);
        self.drag.drag.start_button = Some(ev.button);
        let initial = self.drag.drag.initial_modifiers.unwrap_or(Modifiers::NONE);
        self.start_command(ctx, ev, initial);
        // 按下即画出"零长度"的线（源码在 drag_start 后由 idle 回调做一次）
        self.process_line(ctx, ev.x, ev.y, ev.modifiers);
        true
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.drag.drag.in_drag {
            return false;
        }
        self.update_position(ev);
        // 拖动中每帧重画（源码用 idle 回调 `_drag_idle_cb`，本移植同步做）
        self.process_line(ctx, ev.x, ev.y, ev.modifiers);
        true
    }

    fn button_release(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.drag.drag.in_drag || ev.button != self.drag.drag.start_button.unwrap_or(1) {
            return false;
        }
        self.drag.drag.stop();
        if self.line_possible {
            self.stop_command(ctx, ev, ev.modifiers);
        }
        self.refresh_possible(ctx);
        true
    }

    fn key_press(&mut self, ctx: &mut ModeContext<'_>, key: Key) -> bool {
        // 空格起手（`DragMode.key_press_cb`）：本移植里空格是抓手平移，
        // 所以这里只吃掉 Escape（取消当前这条线）。
        if key == Key::Escape && self.drag.drag.in_drag {
            self.cancel(ctx);
            return true;
        }
        false
    }

    fn process_pending(&mut self, _ctx: &mut ModeContext<'_>) -> bool {
        // 拖动中的重画已在 motion 里同步做完（源码靠 idle 回调节流）
        false
    }
}

/// `LineMode` 需要几个只在 trait 方法之间传的字段，单独写 impl 块。
impl LineMode {
    /// 取消当前这条线（Escape）：回滚画面。
    pub fn cancel(&mut self, ctx: &mut ModeContext<'_>) {
        self.brushwork.rollback(ctx);
        self.drag.drag.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::hokusai::BrushState;

    #[test]
    fn every_line_kind_reports_its_action_id() {
        let cases = [
            (LineKind::Straight, "StraightMode", "Lines and Curves"),
            (LineKind::Sequence, "SequenceMode", "Connected Lines"),
            (LineKind::Ellipse, "EllipseMode", "Ellipses and Circles"),
        ];
        for (kind, action, name) in cases {
            let m = LineMode::new(kind);
            assert_eq!(
                m.action_name(),
                Some(action),
                "{kind:?} 必须报出 resources.xml 的动作 id"
            );
            assert_eq!(m.name(), name);
        }
    }

    #[test]
    fn pressure_curve_points_and_drag_write_back() {
        let mut s = LineModeSettings::default();
        let p = pressure_curve_points(&s);
        assert_eq!(p[0].0, 0.0);
        assert!((p[0].1 - 0.7).abs() < 1e-6); // 1 - 0.3
        assert!((p[1].0 - 0.25).abs() < 1e-6);
        assert!((p[2].0 - 0.75).abs() < 1e-6);
        assert!((p[3].1 - 0.7).abs() < 1e-6);

        apply_curve_point(&mut s, 1, 0.4, 0.2); // 中点压力 0.8，head=0.4
        assert!((s.head - 0.4).abs() < 1e-6);
        assert!((s.midpoint_pressure - 0.8).abs() < 1e-6);
        apply_curve_point(&mut s, 2, 0.6, 0.0); // tail=0.6
        assert!((s.tail - 0.6).abs() < 1e-6);
        apply_curve_point(&mut s, 0, 0.0, 0.9); // entry=0.1
        assert!((s.entry_pressure - 0.1).abs() < 1e-6);
        apply_curve_point(&mut s, 3, 1.0, 0.4); // exit=0.6
        assert!((s.exit_pressure - 0.6).abs() < 1e-6);
        // head 不能越过 tail，tail 也不能越过 head
        apply_curve_point(&mut s, 1, 0.99, 0.5);
        assert!(s.head < s.tail, "head={} tail={}", s.head, s.tail);
        apply_curve_point(&mut s, 2, 0.0, 0.5);
        assert!(s.tail > s.head, "head={} tail={}", s.head, s.tail);
    }
    #[test]
    fn line_settings_can_be_pushed_from_the_app() {
        let mut m = LineMode::new(LineKind::Straight);
        let s = LineModeSettings {
            entry_pressure: 0.1,
            midpoint_pressure: 0.9,
            exit_pressure: 0.2,
            head: 0.3,
            tail: 0.7,
        };
        m.set_line_settings(s);
        let (p1, p2, p2_minus_p1, p3_minus_p2, head, tail) = m.settings.line_settings();
        assert!((p1 - 0.1).abs() < 1e-6);
        assert!((p2 - 0.9).abs() < 1e-6);
        assert!((p2_minus_p1 - 0.8).abs() < 1e-6);
        assert!((p3_minus_p2 + 0.7).abs() < 1e-6);
        assert!((head - 0.3).abs() < 1e-6);
        assert!((tail - 0.7).abs() < 1e-6);
    }

    fn ctx_and_mode<'a>(
        doc: &'a mut Document,
        brush: &'a crate::hokusai::Brush,
        state: &'a mut BrushState,
    ) -> (ModeContext<'a>, LineMode) {
        let ctx = ModeContext {
            doc,
            brush,
            brush_name: "test",
            brush_state: state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            // 取色通道（B3-4）：直线模式不读不写，给中性值/空出口
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        (ctx, LineMode::new(LineKind::Straight))
    }

    #[test]
    fn constraint_snaps_to_fifteen_degrees() {
        // 沿 +x 方向：角度 0
        let (x, y) = constrain_to_angle(100.0, 0.0, 0.0, 0.0);
        assert!((x - 100.0).abs() < 1e-3 && y.abs() < 1e-3, "got {x},{y}");
        // 10° 方向 → 吸附到 15°
        let a = 10f32.to_radians();
        let (x, y) = constrain_to_angle(100.0 * a.cos(), 100.0 * a.sin(), 0.0, 0.0);
        let got = y.atan2(x).to_degrees();
        assert!((got - 15.0).abs() < 1e-2, "10° 应吸到 15°，实得 {got}");
        // 刚好 30°
        let a = 30f32.to_radians();
        let (x, y) = constrain_to_angle(50.0 * a.cos(), 50.0 * a.sin(), 0.0, 0.0);
        let got = y.atan2(x).to_degrees();
        assert!((got - 30.0).abs() < 1e-2, "30° 应保持 30°，实得 {got}");
        assert_eq!(constraint_angle(0.0), 0.0);
        assert_eq!(constraint_angle(7.4), 0.0);
        assert_eq!(constraint_angle(7.6), 15.0);
    }

    #[test]
    fn vector_helpers_match_source() {
        assert_eq!(difference(1.0, 2.0, 4.0, 6.0), (3.0, 4.0));
        assert!((distance(0.0, 0.0, 3.0, 4.0) - 5.0).abs() < 1e-6);
        let (len, nx, ny) = length_and_normal(0.0, 0.0, 3.0, 4.0);
        assert!((len - 5.0).abs() < 1e-6);
        assert!((nx - 0.6).abs() < 1e-6 && (ny - 0.8).abs() < 1e-6);
        // 零长度：单位向量给 (0,0)
        let (len, nx, ny) = length_and_normal(5.0, 5.0, 5.0, 5.0);
        assert_eq!((len, nx, ny), (0.0, 0.0, 0.0));
        assert_eq!(multiply_add(0.0, 0.0, 1.0, 0.0, 3.0), (3.0, 0.0));
        assert_eq!(midpoint(0.0, 0.0, 4.0, 2.0), (2.0, 1.0));
        assert_eq!(perpendicular(1.0, 0.0), (0.0, 1.0));
        assert!((dot_product(1.0, 2.0, 3.0, 4.0) - 11.0).abs() < 1e-6);
    }

    #[test]
    fn ellipse_points_match_source_parameterisation() {
        // 源码 `point_in_ellipse`（linemode.py:947）把 `(y, x)` 反过来传给
        // `rotate_ellipse`，所以参数角 0° 落在 **+y 轴**上（而椭圆的 `angle`
        // 也正好从 90° 起算）。照搬后：0° → (0, rx)，90° → (ry, 0)。
        let (x, y) = point_in_ellipse(10.0, 5.0, 0.0, 1.0, 0.0);
        assert!(x.abs() < 1e-4, "0° → x={x}");
        assert!((y - 10.0).abs() < 1e-4, "0° → y={y}");
        let (x, y) = point_in_ellipse(10.0, 5.0, 0.0, 1.0, 90.0);
        assert!((x - 5.0).abs() < 1e-4, "90° → x={x}");
        assert!(y.abs() < 1e-4, "90° → y={y}");
        // 椭圆上所有点到中心满足 (x/ry)² + (y/rx)² = 1
        for degree in [0.0, 30.0, 47.0, 120.0, 250.0, 359.0] {
            let (x, y) = point_in_ellipse(10.0, 5.0, 0.0, 1.0, degree);
            let v = (x / 5.0).powi(2) + (y / 10.0).powi(2);
            assert!((v - 1.0).abs() < 1e-3, "degree {degree}: 不在椭圆上（{v}）");
        }
    }

    #[test]
    fn line_settings_special_case() {
        let s = LineModeSettings::default();
        let (p1, p2, r1, r2, head, tail) = s.line_settings();
        assert_eq!((p1, p2, head, tail), (0.3, 1.0, 0.25, 0.75));
        assert!((r1 - 0.7).abs() < 1e-6 && (r2 - (-0.7)).abs() < 1e-6);
        // head == 0.0001 → 入笔压力退化成中点压力
        let s = LineModeSettings {
            head: 0.0001,
            ..LineModeSettings::default()
        };
        assert_eq!(s.line_settings().0, 1.0);
    }

    /// 拖一条直线 = 一条 Brushwork，四个压力采样点，撤销可整条回滚。
    #[test]
    fn straight_line_draws_one_undoable_brushwork() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let before = doc.command_stack.undo_len();
        let (mut ctx, mut mode) = ctx_and_mode(&mut doc, &brush, &mut state);

        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
        let motion = PointerEvent::motion(110.0, 60.0, 16.0);
        InteractionMode::motion(&mut mode, &mut ctx, &motion);
        let release = PointerEvent::button(110.0, 60.0, 32.0, 1);
        assert!(InteractionMode::button_release(
            &mut mode, &mut ctx, &release
        ));

        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before + 1,
            "一条直线只该入栈一条 Brushwork"
        );
        let painted = ctx
            .doc
            .layers()
            .current()
            .surface()
            .map(|s| s.tile_count())
            .unwrap_or(0);
        assert!(painted > 0, "直线应真的画在层上");
        // 撤销 → 回滚干净
        ctx.doc.undo();
        assert_eq!(
            ctx.doc
                .layers()
                .current()
                .surface()
                .map(|s| s.tile_count())
                .unwrap_or(1),
            0
        );
    }

    /// Ctrl 约束：终点落在 15° 的整数倍方向上。
    #[test]
    fn ctrl_constrains_the_line_direction() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let (mut ctx, mut mode) = ctx_and_mode(&mut doc, &brush, &mut state);
        let press = PointerEvent::button(0.0, 0.0, 0.0, 1);
        InteractionMode::button_press(&mut mode, &mut ctx, &press);
        // 目标方向约 10°，按住 Ctrl → 应吸到 15°
        let a = 10f32.to_radians();
        let (tx, ty) = (100.0 * a.cos(), 100.0 * a.sin());
        let motion = PointerEvent {
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
            ..PointerEvent::motion(tx, ty, 16.0)
        };
        InteractionMode::motion(&mut mode, &mut ctx, &motion);
        let (ex, ey) = (mode.ex, mode.ey);
        let got = ey.atan2(ex).to_degrees();
        assert!(
            (got - 15.0).abs() < 0.5,
            "Ctrl 应把 10° 吸到 15°，实得 {got}"
        );
    }

    /// SequenceMode（Connected Lines）的起点接**上一笔终点**
    /// （`linemode.py:447-451` + `:525-530`）。
    #[test]
    fn sequence_mode_starts_at_previous_endpoint() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let (mut ctx, mut mode) = ctx_and_mode(&mut doc, &brush, &mut state);
        mode.kind = LineKind::Sequence;

        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
        InteractionMode::motion(
            &mut mode,
            &mut ctx,
            &PointerEvent::motion(110.0, 40.0, 16.0),
        );
        InteractionMode::button_release(
            &mut mode,
            &mut ctx,
            &PointerEvent::button(110.0, 40.0, 32.0, 1),
        );
        assert_eq!(
            ctx.doc.last_painting_pos,
            Some((110.0, 40.0)),
            "收笔应记住终点"
        );

        // 第二笔按在远处，但起点必须接上一笔终点
        let press = PointerEvent::button(300.0, 200.0, 0.0, 1);
        assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
        assert_eq!((mode.sx, mode.sy), (110.0, 40.0), "第二笔应从上一笔终点起");
    }

    /// 组上不能画线。
    #[test]
    fn line_on_group_does_nothing() {
        let mut doc = Document::new();
        let mut group = pigment_surface::layer::Layer::new_group();
        group.name = Some("G".into());
        doc.layers_mut().insert(0, group);
        doc.select_layer(&[0]);
        let before = doc.command_stack.undo_len();
        let brush = crate::brushlib::default_brush();
        let mut state = BrushState::default();
        let (mut ctx, mut mode) = ctx_and_mode(&mut doc, &brush, &mut state);
        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        assert!(!InteractionMode::button_press(&mut mode, &mut ctx, &press));
        let motion = PointerEvent::motion(50.0, 50.0, 16.0);
        assert!(!InteractionMode::motion(&mut mode, &mut ctx, &motion));
        let release = PointerEvent::button(50.0, 50.0, 32.0, 1);
        assert!(!InteractionMode::button_release(
            &mut mode, &mut ctx, &release
        ));
        assert_eq!(ctx.doc.command_stack.undo_len(), before);
    }
}
