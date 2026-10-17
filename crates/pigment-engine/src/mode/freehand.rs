//! 自由手绘模式 —— `gui/freehand.py` 的移植。
//!
//! 结构（源码 → 本文件）：
//! - `FreehandMode._DrawingState`（`:98`）→ [`DrawingState`]：原始事件队列
//!   （含同时间戳事件的清理）、压力/倾角插值器、按键状态、上次处理时间；
//! - `motion_notify_cb`（`:365`）→ [`FreehandMode::motion_with_fake`]：压力/倾角
//!   的合法性检查、evdev 零点补偿、桶旋转的偏移与"减去倾角方位角"；
//! - `_process_queued_event`（`:550`）→ [`FreehandMode::process_pending`]：
//!   算 `dtime`、再喂给 brushwork（`BrushworkModeMixin.stroke_to`）；
//! - `PressureAndTiltInterpolator`（`:720`）→ [`PressureAndTiltInterpolator`]：
//!   逐行照搬（含 `_TEST_DATA` 的 doctest 断言，见本文件测试）。
//!
//! 记账差异：
//! - `GLib.idle_add` 的排队 → 由 GUI 每帧调用一次 [`FreehandMode::process_pending`]；
//! - 设备切换（`device_monitor`/`brush.reset()`）、压力映射曲线
//!   （`input.global_pressure_mapping`）、`tdw.is_sensitive` 没有对应物：
//!   前两者留到 B6 偏好窗口，后者由 GUI 的模态判断代替；
//! - `_hide_drawing_cursor` 只置标志，真正隐藏光标由 GUI 读
//!   [`FreehandMode::hiding_cursor`] 决定。

use super::{
    BrushworkState, InteractionMode, Key, ModeContext, PointerEvent,
};
use std::collections::VecDeque;

/// 自由手绘的可配置项（`gui/userconfig.py` 的默认值 + `app.fakepressure`）。
///
/// 源码每个事件都从 `tdw.app.preferences` **现读**这几个键
/// （`freehand.py:461-478` 的 `input.barrel_rotation_offset` /
/// `input.barrel_rotation_subtract_ascension` / `input.use_barrel_rotation`、
/// `:280` 的 `ui.hide_cursor_while_painting`）以及 `app.fakepressure` /
/// `app.fakerotation`（`application.py:299-300`）。Rust 侧把这些缓存成配置，
/// 由应用在启动/偏好变化时用 [`FreehandConfig::from_prefs`] 重推
/// （[`super::ModeStack::set_freehand_config`]）。
#[derive(Clone, Copy, Debug)]
pub struct FreehandConfig {
    /// `app.fakepressure`（`application.py:299`）。
    ///
    /// **不是偏好项**：源码在 `Application.__init__` 里写死 0.5，之后由
    /// `FakeInputMode` 的滑条改（`freehand.py:697-706`），不进 `settings.json`。
    pub fakepressure: f32,
    /// `app.fakerotation`（`application.py:300`），同上，不是偏好项。
    pub fakerotation: f32,
    /// `input.barrel_rotation_offset`（默认 0.5）。
    pub barrel_rotation_offset: f32,
    /// `input.use_barrel_rotation`（默认 true）。
    pub use_barrel_rotation: bool,
    /// `input.barrel_rotation_subtract_ascension`（默认 true）。
    pub barrel_rotation_subtract_ascension: bool,
    /// `ui.hide_cursor_while_painting`（软键，默认 false）。
    pub hide_cursor_while_painting: bool,
}

impl Default for FreehandConfig {
    fn default() -> Self {
        Self {
            fakepressure: 0.5,
            fakerotation: 0.5,
            barrel_rotation_offset: 0.5,
            use_barrel_rotation: true,
            barrel_rotation_subtract_ascension: true,
            hide_cursor_while_painting: false,
        }
    }
}

impl FreehandConfig {
    /// 从偏好读这几个键 —— 默认值取 `gui/userconfig.py:65-67` 与
    /// `gui/freehand.py:280`（`ui.hide_cursor_while_painting` 是软键，
    /// 默认 `false`）。
    ///
    /// 假压力/假旋转照源码**不读偏好**（`application.py:299-300` 是写死的
    /// 运行期状态），这里固定 0.5，等 B6-3 的假输入滑条接上后再由
    /// `app.fakepressure`/`fakerotation` 覆盖。
    pub fn from_prefs(prefs: &crate::prefs::Prefs) -> Self {
        use crate::prefs::{
            KEY_BARREL_ROTATION_OFFSET, KEY_BARREL_ROTATION_SUBTRACT_ASCENSION,
            KEY_HIDE_CURSOR_WHILE_PAINTING, KEY_USE_BARREL_ROTATION,
        };
        Self {
            fakepressure: 0.5,
            fakerotation: 0.5,
            // `freehand.py:461`：`prefs.get("input.barrel_rotation_offset")`
            barrel_rotation_offset: prefs
                .f64_or(KEY_BARREL_ROTATION_OFFSET, 0.5) as f32,
            // `freehand.py:475-477`
            use_barrel_rotation: prefs.bool_or(KEY_USE_BARREL_ROTATION, true),
            // `freehand.py:467-469`
            barrel_rotation_subtract_ascension: prefs
                .bool_or(KEY_BARREL_ROTATION_SUBTRACT_ASCENSION, true),
            // `freehand.py:280`
            hide_cursor_while_painting: prefs
                .bool_or(KEY_HIDE_CURSOR_WHILE_PAINTING, false),
        }
    }
}

/// 一个原始事件（`freehand.py:156` 的 9 元组）。
///
/// 六个轴都可为 `None`：`None` 表示设备没给该轴的数据，交给插值器填。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawEvent {
    pub time: f64,
    pub x: f32,
    pub y: f32,
    pub pressure: Option<f32>,
    pub xtilt: Option<f32>,
    pub ytilt: Option<f32>,
    pub viewzoom: Option<f32>,
    pub viewrotation: Option<f32>,
    pub barrel_rotation: Option<f32>,
}

impl RawEvent {
    /// 六个轴是否都有值（`None in (...)` 的取反）。
    #[inline]
    pub fn has_all_axes(&self) -> bool {
        self.pressure.is_some()
            && self.xtilt.is_some()
            && self.ytilt.is_some()
            && self.viewzoom.is_some()
            && self.viewrotation.is_some()
            && self.barrel_rotation.is_some()
    }

    fn with_time(&self, time: f64) -> Self {
        Self { time, ..*self }
    }

    /// 六个轴按序取出（已保证 `has_all_axes`）。
    fn axes(&self) -> [f32; 6] {
        [
            self.pressure.unwrap_or(0.0),
            self.xtilt.unwrap_or(0.0),
            self.ytilt.unwrap_or(0.0),
            self.viewzoom.unwrap_or(1.0),
            self.viewrotation.unwrap_or(0.0),
            self.barrel_rotation.unwrap_or(0.0),
        ]
    }
}

/// Catmull-Rom 样条（`gui/drawutils.py:60` 的 `spline_4p`，逐元素版）。
///
/// `t ∈ [0,1]` 时返回 p0→p1 之间的插值点。
#[inline]
pub fn spline_4p(t: f32, p_m1: f32, p0: f32, p1: f32, p2: f32) -> f32 {
    (t * ((2.0 - t) * t - 1.0) * p_m1
        + (t * t * (3.0 * t - 5.0) + 2.0) * p0
        + t * ((4.0 - 3.0 * t) * t + 1.0) * p1
        + (t - 1.0) * t * t * p2)
        / 2.0
}

/// 把 6 个轴一起插值（源码在 numpy 数组上调 `spline_4p`）。
fn spline_4p_axes(t: f32, pm1: &[f32; 6], p0: &[f32; 6], p1: &[f32; 6], p2: &[f32; 6]) -> [f32; 6] {
    let mut out = [0.0f32; 6];
    for i in 0..6 {
        out[i] = spline_4p(t, pm1[i], p0[i], p1[i], p2[i]);
    }
    out
}

/// 压力/倾角插值器（`freehand.py:720` 的 `PressureAndTiltInterpolator`）。
///
/// 序列语义（照源码注释）：
/// - 有完整轴数据的事件成为**控制点** `pt0/pt1`；
/// - 缺轴事件先攒在 `np` 里，等两头都有控制点时按 Catmull-Rom 插值补齐；
/// - 零压力 → 非零、非零 → 零两个方向都清历史，避免"钩状"痕迹；
/// - 非零 → 零时把零压力事件**翻倍**，给鼠标用户一个漂亮的收尾。
#[derive(Default)]
pub struct PressureAndTiltInterpolator {
    pt0_prev: Option<RawEvent>,
    pt0: Option<RawEvent>,
    pt1: Option<RawEvent>,
    pt1_next: Option<RawEvent>,
    np: Vec<RawEvent>,
    np_next: Vec<RawEvent>,
}

impl PressureAndTiltInterpolator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 回到初始干净状态（`_clear`）。
    fn clear(&mut self) {
        *self = Self::default();
    }

    /// 控制点/缓冲各前进一步（`_step`）。
    fn step(&mut self) {
        self.pt0_prev = self.pt0;
        self.pt0 = self.pt1;
        self.pt1 = self.pt1_next;
        self.pt1_next = None;
        self.np = std::mem::take(&mut self.np_next);
        self.np_next = Vec::new();
    }

    /// 在 p0→p1 之间插值（`_interpolate_p0_p1`；不前进也不清空）。
    fn interpolate_p0_p1(&self) -> Vec<RawEvent> {
        let mut out = Vec::new();
        let mut can_interp =
            self.pt0.is_some() && self.pt1.is_some() && !self.np.is_empty();
        let mut pt0p = self.pt0_prev;
        let mut pt1n = self.pt1_next;
        let mut t0 = 0.0f64;
        let mut dt = 0.0f64;
        if can_interp {
            let p0 = self.pt0.expect("刚判过 Some");
            if pt0p.is_none() {
                pt0p = Some(p0);
            }
            if pt1n.is_none() {
                pt1n = self.pt1;
            }
            t0 = p0.time;
            dt = self.pt1.expect("刚判过 Some").time - t0;
            can_interp = dt > 0.0;
        }
        if can_interp {
            let pm1 = pt0p.expect("上面补过").axes();
            let p0 = self.pt0.expect("刚判过 Some").axes();
            let p1 = self.pt1.expect("刚判过 Some").axes();
            let p2 = pt1n.expect("上面补过").axes();
            for ev in &self.np {
                let t = ((ev.time - t0) / dt) as f32;
                let a = spline_4p_axes(t, &pm1, &p0, &p1, &p2);
                out.push(RawEvent {
                    time: ev.time,
                    x: ev.x,
                    y: ev.y,
                    pressure: Some(a[0]),
                    xtilt: Some(a[1]),
                    ytilt: Some(a[2]),
                    viewzoom: Some(a[3]),
                    viewrotation: Some(a[4]),
                    barrel_rotation: Some(a[5]),
                });
            }
        }
        if let Some(pt1) = self.pt1 {
            out.push(pt1);
        }
        out
    }

    /// 插值 + 前进（含两个压力方向转折的特殊处理）。
    fn interpolate_and_step(&mut self) -> Vec<RawEvent> {
        let mut out = self.interpolate_p0_p1();
        let next_pressure = self.pt1_next.map(|e| e.pressure.unwrap_or(0.0));
        let cur_pressure = self.pt1.map(|e| e.pressure.unwrap_or(0.0));
        match (next_pressure, cur_pressure) {
            // 零 → 非零：清历史避免残缺的起笔
            (Some(n), Some(c)) if n > 0.0 && c <= 0.0 => {
                self.pt0_prev = None;
                self.pt0 = self.pt1;
                self.pt1 = self.pt1_next;
                self.pt1_next = None;
                self.np.clear();
                self.np_next.clear();
            }
            // 非零 → 零：把零压力事件翻倍收尾，然后彻底清空
            (Some(n), Some(c)) if n <= 0.0 && c > 0.0 => {
                self.step();
                self.pt1_next = self.pt1;
                out.extend(self.interpolate_p0_p1());
                self.clear();
            }
            _ => self.step(),
        }
        out
    }

    /// 喂一个事件，产出 0..n 个"熟"事件（`feed`）。
    pub fn feed(&mut self, ev: RawEvent) -> Vec<RawEvent> {
        if !ev.has_all_axes() {
            self.np_next.push(ev);
            return Vec::new();
        }
        self.pt1_next = Some(ev);
        self.interpolate_and_step()
    }
}

/// 每个画布的作图状态（`FreehandMode._DrawingState`，`freehand.py:98`）。
#[derive(Default)]
pub struct DrawingState {
    /// 上一次事件是否带压力（决定鼠标的假压力要不要补）。
    pub last_event_had_pressure: bool,
    /// 与上一事件时间戳相同的原始事件（Windows 的时钟粒度问题）。
    zero_dtime_motions: Vec<RawEvent>,
    /// 待处理的原始运动队列。
    motion_queue: VecDeque<RawEvent>,
    /// 压力/倾角插值器。
    pub interp: PressureAndTiltInterpolator,
    /// 上一个**已处理**事件的时间（毫秒）。
    last_handled_event_time: f64,
    /// 队列里最后一个事件的时间。
    last_queued_event_time: f64,
    /// 按下中的按键号（evdev 零点补偿用）。
    pub button_down: Option<u32>,
    pub last_good_raw_pressure: f32,
    pub last_good_raw_xtilt: f32,
    pub last_good_raw_ytilt: f32,
}

impl DrawingState {
    /// 入队（`queue_motion`）：清理同时间戳事件、插值它们的时刻。
    pub fn queue_motion(&mut self, ev: RawEvent) {
        let mut ev = ev;
        if ev.time < self.last_queued_event_time {
            // 时间倒退：夹住（源码打 warning）
            ev.time = self.last_queued_event_time;
        }
        if ev.time == self.last_queued_event_time {
            // 同一时间戳（Windows 上 GTK 约 15ms 粒度，数位板 8ms 一个事件）
            self.zero_dtime_motions.push(ev);
            return;
        }
        if !self.zero_dtime_motions.is_empty() {
            let dtime = ev.time - self.last_queued_event_time;
            let (mut zt, interval) = if dtime > 100.0 {
                // 太老了，别跟新事件绑在一起
                (ev.time - 100.0, 100.0)
            } else {
                (self.last_queued_event_time, dtime)
            };
            let step = interval / (self.zero_dtime_motions.len() as f64 + 1.0);
            for z in std::mem::take(&mut self.zero_dtime_motions) {
                zt += step;
                self.motion_queue.push_back(z.with_time(zt));
            }
        }
        self.motion_queue.push_back(ev);
        self.last_queued_event_time = ev.time;
    }

    /// 取出一个待处理事件并过一遍插值器（`next_processing_events`）。
    pub fn next_processing_events(&mut self) -> Vec<RawEvent> {
        match self.motion_queue.pop_front() {
            Some(ev) => self.interp.feed(ev),
            None => Vec::new(),
        }
    }

    /// 队列是否空了。
    #[inline]
    pub fn queue_is_empty(&self) -> bool {
        self.motion_queue.is_empty()
    }
}

/// 自由手绘模式（`freehand.py:36` 的 `FreehandMode`）。
pub struct FreehandMode {
    /// brushwork 基础设施（`BrushworkModeMixin`）。
    pub brushwork: BrushworkState,
    /// 本画布的作图状态。
    pub draw: DrawingState,
    /// 配置（假压力、桶旋转等）。
    pub config: FreehandConfig,
    /// 光标是否该隐藏（GUI 读它决定画不画自绘光标）。
    hiding_cursor: bool,
}

impl Default for FreehandMode {
    fn default() -> Self {
        Self::new()
    }
}

impl FreehandMode {
    pub fn new() -> Self {
        Self {
            brushwork: BrushworkState::default(),
            draw: DrawingState::default(),
            config: FreehandConfig::default(),
            hiding_cursor: false,
        }
    }

    /// 是否该隐藏系统光标（`hide_cursor_while_painting` 且 давления > 0）。
    #[inline]
    pub fn hiding_cursor(&self) -> bool {
        self.hiding_cursor
    }

    /// 换配置（偏好热更新）—— 见 [`FreehandConfig`] 的说明。
    ///
    /// 源码每个事件都现读 `tdw.app.preferences`，所以"改偏好立刻生效"；
    /// 本移植把配置缓存在模式里，由
    /// [`super::ModeStack::set_freehand_config`] 在偏好变化时重推。
    pub fn set_freehand_config(&mut self, config: FreehandConfig) {
        self.config = config;
        // 光标标志也要按新配置重算（源码 `:280` 是每次 motion 现算的）。
        if !config.hide_cursor_while_painting {
            self.hiding_cursor = false;
        }
    }

    /// 图形光标位置（`tdw.set_last_painting_pos` 的等价物）：最后一次
    /// 有效压力的模型坐标。
    #[inline]
    pub fn last_painting_pos(&self) -> Option<(f32, f32)> {
        self.brushwork.last_pos().map(|p| (p.0, p.1))
    }

    /// 运动事件（`motion_notify_cb`：`freehand.py:365`）。
    ///
    /// `fakepressure`：鼠标的按键/松开事件模拟出来的假压力
    /// （`button_press_cb`/`button_release_cb` 传，普通移动传 `None`）。
    pub fn motion_with_fake(
        &mut self,
        ctx: &mut ModeContext<'_>,
        ev: &PointerEvent,
        fakepressure: Option<f32>,
    ) -> bool {
        if !ctx.current_is_paintable() {
            return false;
        }

        // 原始读数
        let mut pressure = ev.pressure;
        let xtilt = ev.xtilt;
        let ytilt = ev.ytilt;
        let viewzoom = ev.viewzoom;
        let viewrotation = ev.viewrotation;
        let barrel_rotation = ev.barrel_rotation;

        // evdev 补偿：按下期间读到 0 压力 → 用上一个好值
        // （https://github.com/mypaint/mypaint/issues/29）
        if self.draw.button_down.is_some() {
            match pressure {
                Some(p) if p == 0.0 => pressure = Some(self.draw.last_good_raw_pressure),
                Some(p) if p.is_finite() => self.draw.last_good_raw_pressure = p,
                _ => {}
            }
        }

        // 保证压力有定义
        if let Some(p) = pressure {
            if !p.is_finite() {
                // 无穷/NaN：别夹（brush.hpp 会处理），当作没给
                pressure = None;
            } else {
                pressure = Some(p.clamp(0.0, 1.0));
            }
            self.draw.last_event_had_pressure = true;
        }

        // 没压力就造假压力
        let pressure = match pressure {
            Some(p) => p,
            None => {
                let p = match fakepressure {
                    Some(f) => f.clamp(0.0, 1.0),
                    None => {
                        if ev.button == 1 || self.draw.button_down.is_some() {
                            self.config.fakepressure
                        } else {
                            0.0
                        }
                    }
                };
                self.draw.last_event_had_pressure = false;
                p
            }
        };

        // 倾角：NaN（无倾角设备）→ 0；范围夹住
        let (xt, yt) = match (xtilt, ytilt) {
            (Some(x), Some(y)) if (x + y).is_finite() => (x, y),
            _ => (0.0, 0.0),
        };
        let mut xtilt = xt.clamp(-1.0, 1.0);
        let mut ytilt = yt.clamp(-1.0, 1.0);

        // evdev 补偿：倾角也有同样的问题
        if self.draw.button_down.is_some() {
            if xtilt == 0.0 {
                xtilt = self.draw.last_good_raw_xtilt;
            } else {
                self.draw.last_good_raw_xtilt = xtilt;
            }
            if ytilt == 0.0 {
                ytilt = self.draw.last_good_raw_ytilt;
            } else {
                self.draw.last_good_raw_ytilt = ytilt;
            }
        }

        let tilt_ascension = 0.5 * (-xtilt).atan2(ytilt) / std::f32::consts::PI;

        // 桶旋转：加偏移、可减去倾角方位角
        let b_offset = self.config.barrel_rotation_offset;
        let mut barrel = barrel_rotation.map(|b| (b + b_offset).rem_euclid(1.0));
        if let Some(b) = barrel {
            if self.config.barrel_rotation_subtract_ascension {
                barrel = Some((b - tilt_ascension).rem_euclid(1.0));
            }
        }
        // 没有桶旋转轴（或关了）→ 用键盘可调的假旋转
        let barrel_rotation = match barrel {
            Some(b) if self.config.use_barrel_rotation => b,
            _ => (self.config.fakerotation + b_offset).rem_euclid(1.0),
        };

        // 画的时候隐藏光标
        self.hiding_cursor = self.config.hide_cursor_while_painting && pressure > 0.0;

        // 入队
        let pressure = pressure.max(0.0);
        self.draw.queue_motion(RawEvent {
            time: ev.time_ms,
            x: ev.x,
            y: ev.y,
            pressure: Some(pressure),
            xtilt: Some(xtilt),
            ytilt: Some(ytilt),
            viewzoom: Some(viewzoom),
            viewrotation: Some(viewrotation),
            barrel_rotation: Some(barrel_rotation),
        });
        true
    }

    /// 处理队列里的全部事件（`_motion_queue_idle_cb` + `_process_queued_event`）。
    pub fn process_pending(&mut self, ctx: &mut ModeContext<'_>) {
        loop {
            let events = self.draw.next_processing_events();
            if events.is_empty() && self.draw.queue_is_empty() {
                break;
            }
            for ev in events {
                self.process_queued_event(ctx, &ev);
            }
            if self.draw.queue_is_empty() {
                break;
            }
        }
        // 队列空了 → 收尾光标状态
        if self.draw.queue_is_empty() && self.draw.button_down.is_none() {
            self.hiding_cursor = false;
        }
    }

    /// 处理一个队列事件（`freehand.py:550`）。
    fn process_queued_event(&mut self, ctx: &mut ModeContext<'_>, ev: &RawEvent) {
        let last = self.draw.last_handled_event_time;
        self.draw.last_handled_event_time = ev.time;
        if last == 0.0 {
            // 第一个事件只用来定时间基准（源码同）
            return;
        }
        let dtime = ((ev.time - last) / 1000.0).max(0.0);
        if !ctx.current_is_paintable() {
            return;
        }
        // 插值可能产出越界的压力/倾角（源码在 feed 后再夹一次）
        let pressure = ev.pressure.unwrap_or(0.0).clamp(0.0, 1.0);
        let xtilt = ev.xtilt.unwrap_or(0.0).clamp(-1.0, 1.0);
        let ytilt = ev.ytilt.unwrap_or(0.0).clamp(-1.0, 1.0);
        self.brushwork.stroke_to(
            ctx,
            dtime,
            ev.x,
            ev.y,
            pressure,
            xtilt,
            ytilt,
            ev.viewzoom.unwrap_or(1.0),
            ev.viewrotation.unwrap_or(0.0),
            ev.barrel_rotation.unwrap_or(0.0),
            true,
        );
        // `freehand.py:614-617`：压力非零时更新 TDW 的最后作画位置
        if pressure != 0.0 {
            ctx.doc.last_painting_pos = Some((ev.x, ev.y));
        }
    }
}

impl InteractionMode for FreehandMode {
    fn name(&self) -> &'static str {
        "Freehand Drawing"
    }

    fn usage(&self) -> &str {
        "Paint free-form brush strokes"
    }

    fn action_name(&self) -> Option<&'static str> {
        Some("FreehandMode")
    }

    fn pointer_behavior(&self) -> super::Behavior {
        super::Behavior::PAINT_FREEHAND
    }

    fn scroll_behavior(&self) -> super::Behavior {
        super::Behavior::CHANGE_VIEW
    }

    fn is_live_updateable(&self) -> bool {
        true
    }

    fn is_painting(&self) -> bool {
        self.brushwork.is_active()
    }

    /// 收下偏好推来的配置（`ModeStack::set_freehand_config`）。
    fn set_freehand_config(&mut self, config: FreehandConfig) {
        FreehandMode::set_freehand_config(self, config);
    }

    fn enter(&mut self, _ctx: &mut ModeContext<'_>) {
        self.draw = DrawingState::default();
        self.hiding_cursor = false;
    }

    fn leave(&mut self, ctx: &mut ModeContext<'_>) {
        self.draw = DrawingState::default();
        self.hiding_cursor = false;
        self.brushwork.commit(ctx, true);
    }

    fn checkpoint(&mut self, ctx: &mut ModeContext<'_>) {
        // 有未提交的改动就落账（不发零压力收尾，避免笔迹断裂）
        self.brushwork.commit(ctx, false);
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !ctx.current_is_paintable() || ev.button != 1 {
            return false;
        }
        // 鼠标没有压力轴：按下时用假压力补一个运动事件
        if !self.draw.last_event_had_pressure {
            let fake = self.config.fakepressure;
            self.motion_with_fake(ctx, ev, Some(fake));
        }
        self.draw.button_down = Some(ev.button);
        self.draw.last_good_raw_pressure = 0.0;
        self.draw.last_good_raw_xtilt = 0.0;
        self.draw.last_good_raw_ytilt = 0.0;
        true
    }

    fn button_release(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !ctx.current_is_paintable() || ev.button != 1 {
            return false;
        }
        // 松开时补一个零压力事件 → 笔迹有收尾
        if !self.draw.last_event_had_pressure {
            self.motion_with_fake(ctx, ev, Some(0.0));
        }
        self.draw.button_down = None;
        self.draw.last_good_raw_pressure = 0.0;
        self.draw.last_good_raw_xtilt = 0.0;
        self.draw.last_good_raw_ytilt = 0.0;
        self.hiding_cursor = false;
        // 先把队列画完（含刚补的收尾事件），再把这一段作画落账。
        //
        // 记账：源码是**惰性**提交的（`lib/document.py:1160-1186` 的
        // `sync_pending_changes()` 在 do/undo/redo 之前才 flush），所以鼠标
        // 松开的那一刻命令还没入栈。本实现在这里显式 `commit`（等价于源码
        // `leave()`/`checkpoint()` 的收尾），因为：
        // 1. 用户观感一致（松开即"这一笔完成"）；
        // 2. 菜单/快捷键的 Undo 可用性直接看撤销栈长度，惰性提交会让
        //    第一笔之后 Undo 仍然灰着。
        self.process_pending(ctx);
        self.brushwork.commit(ctx, false);
        true
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        self.motion_with_fake(ctx, ev, None)
    }

    fn key_press(&mut self, _ctx: &mut ModeContext<'_>, key: Key) -> bool {
        // Escape：丢弃排队中的笔迹（`freehand.py` 文档里提到"按 Escape 可中止"）
        if key == Key::Escape {
            self.draw = DrawingState::default();
            return true;
        }
        false
    }

    fn process_pending(&mut self, ctx: &mut ModeContext<'_>) -> bool {
        self.process_pending(ctx);
        !self.draw.queue_is_empty()
    }
}

/// 修一个常量名（`std::f32::consts` 在 2021 edition 里是 `consts`）。
mod consts_shim {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 源码 doctest 的硬门禁（`freehand.py:730-750`）。
    ///
    /// 断言逐条照抄：
    /// 1. 所有原始事件都是 9 元组；
    /// 2. 原始数据里确实有缺轴的；
    /// 3. 输出的"熟"事件里**没有**缺轴的；
    /// 4. 输出事件数 ≤ 输入事件数；
    /// 5. 所有"完整"的原始事件都原样出现在输出里；
    /// 6. 没有 `70 < t < 110` 的事件（零压力后的空档被跳过）；
    /// 7. `t ∈ {70, 110}` 的事件恰好各一个。
    #[test]
    fn interpolator_matches_source_doctest() {
        let raw: Vec<RawEvent> = TEST_DATA.to_vec();
        let mut interp = PressureAndTiltInterpolator::new();
        let mut cooked: Vec<RawEvent> = Vec::new();
        for ev in &raw {
            cooked.extend(interp.feed(*ev));
        }
        assert!(
            cooked.iter().all(|e| e.has_all_axes()),
            "输出的熟事件不该缺轴"
        );
        assert!(cooked.len() <= raw.len(), "输出不该比输入多");
        for ev in raw.iter().filter(|e| e.has_all_axes()) {
            assert!(cooked.contains(ev), "带轴原始事件应原样出现在输出里: {ev:?}");
        }
        assert!(
            !cooked.iter().any(|e| e.time > 70.0 && e.time < 110.0),
            "零压力之后的空档要被跳过"
        );
        let at = |t: f64| cooked.iter().filter(|e| e.time == t).count();
        assert_eq!(at(70.0), 1, "t=70 的零压力事件要保留");
        assert_eq!(at(110.0), 1, "t=110 的事件要保留");
    }

    /// 源码 `_TEST_DATA`（`freehand.py:756-799`）。
    const TEST_DATA: [RawEvent; 33] = [
        ev(3.0, 0.3, None),
        ev(7.0, 0.7, None),
        ev(10.0, 1.0, Some(0.33)),
        ev(13.0, 1.3, None),
        ev(15.0, 1.5, None),
        ev(17.0, 1.7, None),
        ev(20.0, 2.0, Some(0.45)),
        ev(23.0, 2.3, None),
        ev(27.0, 2.7, None),
        ev(30.0, 3.0, Some(0.50)),
        ev(33.0, 3.3, None),
        ev(37.0, 3.7, None),
        ev(40.0, 4.0, Some(0.40)),
        ev(44.0, 4.4, None),
        ev(47.0, 4.7, None),
        ev(50.0, 5.0, Some(0.30)),
        ev(53.0, 5.3, None),
        ev(57.0, 5.7, None),
        ev(60.0, 6.0, Some(0.11)),
        ev(63.0, 6.3, None),
        ev(67.0, 6.7, None),
        ev(70.0, 7.0, Some(0.0)),
        ev(73.0, 7.0, None),
        ev(78.0, 50.0, None),
        ev(83.0, 110.0, None),
        ev(88.0, 120.0, None),
        ev(93.0, 130.0, None),
        ev(98.0, 140.0, None),
        ev(103.0, 150.0, None),
        ev(108.0, 160.0, None),
        ev(110.0, 170.0, Some(0.11)),
        ev(120.0, 171.0, Some(0.33)),
        ev(130.0, 172.0, Some(0.0)),
    ];

    /// 构造测试数据：x=y=value；有压力时其余轴也给齐（源码如此）。
    const fn ev(time: f64, value: f32, pressure: Option<f32>) -> RawEvent {
        RawEvent {
            time,
            x: value,
            y: value,
            pressure,
            xtilt: if pressure.is_some() { Some(0.0) } else { None },
            ytilt: if pressure.is_some() { Some(0.0) } else { None },
            viewzoom: if pressure.is_some() { Some(1.0) } else { None },
            viewrotation: if pressure.is_some() { Some(0.0) } else { None },
            barrel_rotation: if pressure.is_some() { Some(0.0) } else { None },
        }
    }

    #[test]
    fn spline_passes_through_control_points() {
        // t=0 → p0，t=1 → p1
        assert!((spline_4p(0.0, 9.0, 1.0, 2.0, 9.0) - 1.0).abs() < 1e-6);
        assert!((spline_4p(1.0, 9.0, 1.0, 2.0, 9.0) - 2.0).abs() < 1e-6);
        // 线性控制点 → 线性插值
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let want = 1.0 + t;
            assert!((spline_4p(t, 0.0, 1.0, 2.0, 3.0) - want).abs() < 1e-5);
        }
    }

    #[test]
    fn zero_dtime_events_get_spread_over_the_gap() {
        let mut st = DrawingState::default();
        let mut out = Vec::new();
        for (i, t) in [0.0, 10.0, 10.0, 10.0, 20.0].into_iter().enumerate() {
            st.queue_motion(RawEvent {
                time: t,
                x: i as f32,
                y: 0.0,
                pressure: Some(0.5),
                xtilt: Some(0.0),
                ytilt: Some(0.0),
                viewzoom: Some(1.0),
                viewrotation: Some(0.0),
                barrel_rotation: Some(0.0),
            });
            out.extend(st.next_processing_events());
        }
        // 三个同时间戳事件被摊到 (10,20) 之间，插值器会给它们各自的时刻
        let times: Vec<f64> = out.iter().map(|e| e.time).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "时刻必须单调: {times:?}");
    }

    /// 一次完整的按压-移动-松开 = **一条** Brushwork，且撤销能干净回退。
    ///
    /// 这是 B3-1 的主干行为：GUI 不再手搓事件循环，模式负责排队+插值+
    /// 原子提交（`mode.py:686-873` 的 `brushwork_begin/commit`）。
    #[test]
    fn freehand_stroke_makes_one_undoable_brushwork() {
        use crate::document::Document;

        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut status = Vec::new();
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: Some(&mut status),
            // 取色通道（B3-4）：自由手绘不读不写，给中性值/空出口
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FreehandMode::new();
        let before = ctx.doc.command_stack.undo_len();

        // 按下（鼠标无压感 → 模式自己补假压力）
        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
        // 移动若干次，时间间隔 16ms
        for i in 1..=6 {
            let ev = PointerEvent::motion(10.0 + i as f32 * 8.0, 10.0 + i as f32 * 4.0, i as f64 * 16.0);
            assert!(InteractionMode::motion(&mut mode, &mut ctx, &ev));
            mode.process_pending(&mut ctx);
        }
        // 松开
        let release = PointerEvent::button(58.0, 34.0, 7.0 * 16.0, 1);
        assert!(InteractionMode::button_release(&mut mode, &mut ctx, &release));
        mode.process_pending(&mut ctx);

        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before + 1,
            "整条笔迹只该入栈一条 Brushwork"
        );
        let painted = ctx
            .doc
            .layers()
            .current()
            .surface()
            .map(|s| s.tile_count())
            .unwrap_or(0);
        assert!(painted > 0, "笔迹应真的落在当前层上");
        // 撤销 → 像素回滚，且撤销栈回到基线
        ctx.doc.undo();
        assert_eq!(
            ctx.doc
                .layers()
                .current()
                .surface()
                .map(|s| s.tile_count())
                .unwrap_or(1),
            0,
            "撤销应把这一笔的 tile 全清掉"
        );
    }

    /// 组上作画不该产生命令（`get_paintable` 语义在模式层的体现）。
    #[test]
    fn freehand_on_group_paints_nothing() {
        use crate::document::Document;
        use crate::mode::Behavior;
        use pigment_surface::layer::Layer;

        let mut doc = Document::new();
        // 造一个组并选中它
        let mut group = Layer::new_group();
        group.name = Some("G".into());
        doc.layers_mut().insert(0, group);
        doc.select_layer(&[0]);
        assert!(doc.layers().paintable_at(&[0]).is_none());
        // `select_layer` 自己会入一条自动撤销命令，先记下基线
        let before = doc.command_stack.undo_len();

        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut status = Vec::new();
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: Some(&mut status),
            // 取色通道（B3-4）：自由手绘不读不写，给中性值/空出口
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FreehandMode::new();
        let press = PointerEvent {
            pressure: None,
            ..PointerEvent::button(10.0, 10.0, 0.0, 1)
        };
        assert!(!InteractionMode::button_press(&mut mode, &mut ctx, &press));
        let motion = PointerEvent {
            pressure: None,
            ..PointerEvent::motion(20.0, 20.0, 16.0)
        };
        assert!(!InteractionMode::motion(&mut mode, &mut ctx, &motion));
        mode.process_pending(&mut ctx);
        InteractionMode::button_release(&mut mode, &mut ctx, &press);
        mode.process_pending(&mut ctx);
        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before,
            "组上作画不能产生命令"
        );
        assert_eq!(interpolator_behavior(), Behavior::PAINT_FREEHAND);
    }

    fn interpolator_behavior() -> super::super::Behavior {
        FreehandMode::new().pointer_behavior()
    }
}
