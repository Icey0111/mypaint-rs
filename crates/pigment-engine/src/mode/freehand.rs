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

use super::{BrushworkState, InteractionMode, Key, ModeContext, PointerEvent};
use crate::hokusai::{Brush, BrushSetting, BrushState, TiledSurface};
use crate::stroke::StrokeEvent;
use std::collections::VecDeque;
use std::time::Instant;

/// 低缩放大笔刷的即时预览（Krita Instant Preview 的最小实现）。
///
/// 输入事件先画到**当前显示 mip 级别**的预览面上，显示立即更新；原始事件
/// 排队，随后按全分辨率重放进真实表面。重放完成后撤掉预览、提交撤销命令。
/// 真实像素与完全不启用预览时逐字节一致（事件/状态都按原样重放）。
struct PreviewStroke {
    level: u32,
    scale: f32,
    brush: Brush,
    state: BrushState,
    real_events: VecDeque<StrokeEvent>,
    last_pos: (f32, f32),
    started: Instant,
    events: u64,
}

fn preview_log() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PIGMENT_PREVIEW_DEBUG").is_some())
}

fn preview_min_radius() -> f32 {
    use std::sync::OnceLock;
    static R: OnceLock<f32> = OnceLock::new();
    *R.get_or_init(|| {
        std::env::var("PIGMENT_PREVIEW_MIN_RADIUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4.0)
    })
}

/// 与 `pigment_render::mip_level_for_zoom` 同一公式（引擎不依赖渲染层）。
fn preview_level_for_zoom(zoom: f32) -> u32 {
    if zoom >= 1.0 {
        return 0;
    }
    let inv = 1.0 / zoom.max(1.0 / 1024.0);
    (inv.log2().floor().max(0.0) as u32).min(pigment_surface::mipmap::MAX_MIPMAP_LEVEL)
}

/// 把跨笔刷状态里的“长度”换算到预览级别坐标。
///
/// 位置/速度/半径都除以 `scale`；`norm_speed*` 已经按半径归一化，保持不变。
fn scale_state_to_preview(state: &BrushState, scale: f32) -> BrushState {
    let mut s = state.clone();
    let inv = 1.0 / scale;
    s.actual_x *= inv;
    s.actual_y *= inv;
    s.actual_dab_x *= inv;
    s.actual_dab_y *= inv;
    s.last_event_x *= inv;
    s.last_event_y *= inv;
    s.actual_radius *= inv;
    s.norm_dx_slow *= inv;
    s.norm_dy_slow *= inv;
    s.direction_dx *= inv;
    s.direction_dy *= inv;
    s.direction_angle_dx *= inv;
    s.direction_angle_dy *= inv;
    s.skip_distance *= inv;
    s.skip_last_x *= inv;
    s.skip_last_y *= inv;
    s.dist_past_dab *= inv;
    s.last_dab_x *= inv;
    s.last_dab_y *= inv;
    s.gridmap_x *= inv;
    s.gridmap_y *= inv;
    s
}

/// Per-frame GPU dab budget. The flush cost is paid after event processing,
/// so stopping at a fixed queue depth keeps frame time stable; remaining
/// events are processed in later frames without splitting any event.
fn gpu_dab_budget() -> usize {
    use std::sync::OnceLock;
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("PIGMENT_GPU_DAB_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v| v > 0)
            .unwrap_or(8192)
    })
}

/// 自由手绘的可配置项（`gui/userconfig.py` 的默认值 + `app.fakepressure`）。
///
/// 源码每个事件都从 `tdw.app.preferences` **现读**这几个键
/// （`freehand.py:461-478` 的 `input.barrel_rotation_offset` /
/// `input.barrel_rotation_subtract_ascension` / `input.use_barrel_rotation`、
/// `:280` 的 `ui.hide_cursor_while_painting`）以及 `app.fakepressure` /
/// `app.fakerotation`（`application.py:299-300`）。Rust 侧把这些缓存成配置，
/// 由应用在启动/偏好变化时用 [`FreehandConfig::from_prefs`] 重推
/// （[`super::ModeStack::set_freehand_config`]）。
#[derive(Clone, Debug)]
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
    /// 低缩放大笔刷的即时预览（Krita Instant Preview 的移植）。
    ///
    /// 软键 `ui.instant_preview`，`PIGMENT_INSTANT_PREVIEW=1` 可强制打开
    /// （测试/对照用）。
    pub instant_preview: bool,
    /// `input.global_pressure_mapping` 的点列 `(x, y)`。
    ///
    /// 源码 `_apply_pressure_mapping_settings`（`application.py:594-610`）
    /// 在两点 `(0,1),(1,0)` 时视为 1:1 关闭；其余情况用 `y' = 1 - y`
    /// 喂给 `MappingWrapper`。这里保存原始点列，应用时按同一规则做
    /// 分段线性映射。
    pub pressure_mapping: Vec<(f32, f32)>,
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
            instant_preview: false,
            pressure_mapping: vec![(0.0, 1.0), (1.0, 0.0)],
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
            barrel_rotation_offset: prefs.f64_or(KEY_BARREL_ROTATION_OFFSET, 0.5) as f32,
            // `freehand.py:475-477`
            use_barrel_rotation: prefs.bool_or(KEY_USE_BARREL_ROTATION, true),
            // `freehand.py:467-469`
            barrel_rotation_subtract_ascension: prefs
                .bool_or(KEY_BARREL_ROTATION_SUBTRACT_ASCENSION, true),
            // `freehand.py:280`
            hide_cursor_while_painting: prefs.bool_or(KEY_HIDE_CURSOR_WHILE_PAINTING, false),
            // 默认开（Krita 的全局开关也是默认开）；`PIGMENT_INSTANT_PREVIEW=0`
            // 或偏好 `ui.instant_preview=false` 可关闭。
            instant_preview: match std::env::var("PIGMENT_INSTANT_PREVIEW") {
                Ok(v) => !(v == "0" || v.eq_ignore_ascii_case("false")),
                Err(_) => prefs.bool_or("ui.instant_preview", true),
            },
            pressure_mapping: read_pressure_mapping(prefs),
        }
    }

    /// 应用全局压力曲线（`application.py:594-610`）。
    ///
    /// 两点 `(0,1),(1,0)` = 1:1 关闭；其余点按 x 升序分段线性，
    /// y 先按源码的 `1.0 - y` 翻转再插值。
    pub fn map_pressure(&self, pressure: f32) -> f32 {
        if self.pressure_mapping.len() < 2 {
            return pressure;
        }
        // 1:1（关闭）——源码的容差 1e-4
        if self.pressure_mapping.len() == 2
            && (self.pressure_mapping[0].1 - 1.0).abs() + (self.pressure_mapping[1].1 - 0.0).abs()
                < 0.0001
        {
            return pressure;
        }
        let p = pressure.clamp(0.0, 1.0);
        let mut pts: Vec<(f32, f32)> = self
            .pressure_mapping
            .iter()
            .map(|(x, y)| (x.clamp(0.0, 1.0), (1.0 - y).clamp(0.0, 1.0)))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if p <= pts[0].0 {
            return pts[0].1;
        }
        for w in pts.windows(2) {
            let (x0, y0) = w[0];
            let (x1, y1) = w[1];
            if p <= x1 {
                let t = if (x1 - x0).abs() < f32::EPSILON {
                    0.0
                } else {
                    (p - x0) / (x1 - x0)
                };
                return (y0 + (y1 - y0) * t).clamp(0.0, 1.0);
            }
        }
        pts.last().map(|p| p.1).unwrap_or(pressure)
    }
}

/// 读 `input.global_pressure_mapping`（`[[x,y], ...]`）。
fn read_pressure_mapping(prefs: &crate::prefs::Prefs) -> Vec<(f32, f32)> {
    let default = vec![(0.0, 1.0), (1.0, 0.0)];
    let Some(arr) = prefs.get_array("input.global_pressure_mapping") else {
        return default;
    };
    let mut out = Vec::new();
    for pair in arr {
        if let Some(pair) = pair.as_array() {
            if pair.len() >= 2 {
                if let (Some(x), Some(y)) = (pair[0].as_f64(), pair[1].as_f64()) {
                    out.push((x as f32, y as f32));
                }
            }
        }
    }
    if out.len() < 2 {
        default
    } else {
        out
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
        let mut can_interp = self.pt0.is_some() && self.pt1.is_some() && !self.np.is_empty();
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

    /// 直接取一个原始事件（预算处理按事件切片时用；不再整批 feed）。
    #[inline]
    pub fn pop_raw(&mut self) -> Option<RawEvent> {
        self.motion_queue.pop_front()
    }

    /// 队列是否空了。
    #[inline]
    pub fn queue_is_empty(&self) -> bool {
        self.motion_queue.is_empty()
    }

    #[inline]
    pub fn queue_len(&self) -> usize {
        self.motion_queue.len()
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
    /// 指针已抬起、但排队中的笔迹还没画完：收尾/入栈要等队列排空。
    release_pending: bool,
    /// 插值器输出、尚未喂给 brushwork 的“熟”事件。
    pending_events: VecDeque<RawEvent>,
    /// 低缩放大笔刷的即时预览；真实笔画在后面排队重放。
    preview: Option<PreviewStroke>,
    /// Real-stroke diagnostics: start time + perf counter snapshot.
    stroke_perf: Option<(std::time::Instant, [u64; 6])>,
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
            release_pending: false,
            pending_events: VecDeque::new(),
            preview: None,
            stroke_perf: None,
        }
    }

    /// 原始队列 + 插值器输出队列是否都空了。
    #[inline]
    fn has_pending_events(&self) -> bool {
        !self.pending_events.is_empty() || !self.draw.queue_is_empty()
    }

    fn log_stroke_perf(&mut self) {
        if let Some((t, base)) = self.stroke_perf.take() {
            let now = Self::perf_snapshot();
            eprintln!(
                "stroke: {:.0}ms dabs_gpu={} dabs_cpu={} flushes={} flush={:.1}ms samples={} sample={:.1}ms",
                t.elapsed().as_secs_f64() * 1e3,
                now[0].saturating_sub(base[0]),
                now[1].saturating_sub(base[1]),
                now[3].saturating_sub(base[3]),
                now[2].saturating_sub(base[2]) as f64 / 1e6,
                now[5].saturating_sub(base[5]),
                now[4].saturating_sub(base[4]) as f64 / 1e6,
            );
        }
    }

    fn perf_snapshot() -> [u64; 6] {
        use pigment_surface::perf;
        [
            perf::get(&perf::DABS_GPU),
            perf::get(&perf::DABS_CPU),
            perf::get(&perf::FLUSH_NS),
            perf::get(&perf::FLUSH_CALLS),
            perf::get(&perf::SAMPLE_NS),
            perf::get(&perf::SAMPLE_CALLS),
        ]
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
        self.config = config.clone();
        // 光标标志也要按新配置重算（源码 `:280` 是每次 motion 现算的）。
        if !config.hide_cursor_while_painting {
            self.hiding_cursor = false;
        }
    }

    /// 图形光标位置（`tdw.set_last_painting_pos` 的等价物）：最后一次
    /// 有效压力的模型坐标。
    #[inline]
    pub fn last_painting_pos(&self) -> Option<(f32, f32)> {
        if let Some(p) = &self.preview {
            return Some(p.last_pos);
        }
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

        // 镜像视图：x 倾角与桶旋转取反（`freehand.py:492-494`）。
        // 注意源码是在算出 tilt_ascension / barrel 之后再翻转这两个**输出**值。
        let (xtilt, barrel_rotation) = if ev.viewmirrored {
            (-xtilt, -barrel_rotation)
        } else {
            (xtilt, barrel_rotation)
        };

        // 画的时候隐藏光标
        self.hiding_cursor = self.config.hide_cursor_while_painting && pressure > 0.0;

        // 入队（先过全局压力曲线，`freehand.py:496-499`）
        let pressure = self.config.map_pressure(pressure.max(0.0));
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
        self.process_pending_until(ctx, None);
        self.finish_release_if_drained(ctx);
    }

    /// 在 `deadline` 前尽量多处理一些排队事件；剩余留给后续帧。
    ///
    /// 每个事件至少处理一次（即使预算已经用完），保证有界的工作量下
    /// 队列一定能推进，不会活锁。
    pub fn process_pending_budgeted(
        &mut self,
        ctx: &mut ModeContext<'_>,
        deadline: std::time::Instant,
    ) -> bool {
        self.process_pending_until(ctx, Some(deadline));
        self.finish_release_if_drained(ctx);
        let gpu_pending = ctx
            .doc
            .paintable_surface()
            .is_some_and(|s| s.gpu_pending_ops() > 0);
        self.has_pending_events() || self.has_preview_pending() || self.release_pending || gpu_pending
    }

    fn process_pending_until(
        &mut self,
        ctx: &mut ModeContext<'_>,
        deadline: Option<std::time::Instant>,
    ) {
        let mut processed_any = false;
        loop {
            // 积压未清空时不再喂新事件：GPU 队列按帧限量推进，帧时间
            // 保持稳定；整个事件始终完整处理，不会被拆开。
            let pending = ctx.doc.paintable_surface().map_or(0, |s| s.gpu_pending_ops());
            if deadline.is_some() && pending >= gpu_dab_budget() {
                if std::env::var_os("PIGMENT_TRACE_BUDGET").is_some() {
                    eprintln!("budget hold pending={pending}");
                }
                break;
            }
            if processed_any {
                if let Some(deadline) = deadline {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                }
            }
            let Some(ev) = self.next_processed_event() else {
                break;
            };
            // 一个事件完整交给引擎：libmypaint 自己在 `stroke_to` 内按
            // 时间与 dab 间距做仿真步进，GUI 再切一次会改变 slow_tracking、
            // 压力插值和 RNG 消耗，破坏与上游的一致性。
            self.process_queued_event(ctx, &ev);
            processed_any = true;
            // GPU dab queue costs are paid at the frame flush, outside the
            // deadline above. Bound the number of queued dabs so one frame
            // cannot turn into a multi-hundred-dab submit (visible stutter).
            // Whole events are always kept intact; only the frame at which
            // the next event is processed moves.
            let pending = ctx.doc.paintable_surface().map_or(0, |s| s.gpu_pending_ops());
            if deadline.is_some() && pending >= gpu_dab_budget() {
                if std::env::var_os("PIGMENT_TRACE_BUDGET").is_some() {
                    eprintln!("budget break pending={pending}");
                }
                break;
            }
        }
        // 只有抬笔后（或根本没在画）才推进全分辨率重放：笔还按着时
        // 即使本帧输入队列暂时为空，也不能让后台重放抢掉预览的预算。
        if !self.has_pending_events() && (self.release_pending || self.draw.button_down.is_none()) {
            self.drain_preview_real(ctx, deadline);
        }
        // 队列空了 → 收尾光标状态
        if !self.has_pending_events() && self.draw.button_down.is_none() && !self.release_pending {
            self.hiding_cursor = false;
        }
    }

    /// 取下一个“熟”事件：优先消费已插值事件；否则从原始队列取一个喂插值器。
    fn next_processed_event(&mut self) -> Option<RawEvent> {
        if let Some(ev) = self.pending_events.pop_front() {
            return Some(ev);
        }
        loop {
            let raw = self.draw.pop_raw()?;
            let out = self.draw.interp.feed(raw);
            if !out.is_empty() {
                self.pending_events.extend(out);
                return self.pending_events.pop_front();
            }
        }
    }

    /// 延迟提交：抬笔事件已入队，等输入队列、预览重放和 GPU 全部排空后再记账。
    fn finish_release_if_drained(&mut self, ctx: &mut ModeContext<'_>) {
        if !self.release_pending || self.has_pending_events() {
            return;
        }
        if self.preview.is_some() {
            if !self.preview_ready_to_clear(ctx) {
                return;
            }
            self.clear_preview(ctx);
        }
        self.release_pending = false;
        self.log_stroke_perf();
        self.brushwork.commit(ctx, false);
        self.hiding_cursor = false;
    }

    /// 是否还有等待全分辨率重放的输入事件。
    #[inline]
    fn has_preview_pending(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|p| !p.real_events.is_empty())
    }

    /// 启动预览：播种当前 mip 的预览面，真实命令先 begin 但暂不落笔。
    fn maybe_start_preview(&mut self, ctx: &mut ModeContext<'_>, ev: &StrokeEvent) -> bool {
        if self.preview.is_some() || !self.config.instant_preview {
            return false;
        }
        // 只有真正按住笔的笔画才开预览；悬停/收笔后的零散事件不开，
        // 否则没有抬笔事件来收尾，预览会永远挂着。
        if self.draw.button_down.is_none() {
            return false;
        }
        let level = preview_level_for_zoom(ev.viewzoom);
        if level == 0 {
            return false;
        }
        let base_radius = ctx.brush.get(BrushSetting::Radius).base_value.exp();
        if base_radius < preview_min_radius() {
            return false;
        }
        if ctx.doc.paintable_surface().is_none() {
            return false;
        }
        let scale = (1u32 << level) as f32;
        let mut preview_brush = ctx.brush.clone();
        let radius_log = ctx.brush.get(BrushSetting::Radius).base_value;
        preview_brush.set_base_value(BrushSetting::Radius, radius_log - scale.ln());
        let preview_state = scale_state_to_preview(ctx.brush_state, scale);

        let Some(surface) = ctx.doc.paintable_surface_mut() else {
            return false;
        };
        if surface.has_preview_overlay() {
            return false;
        }
        let preview_surface = surface.seed_preview_at(level);
        surface.set_preview_overlay(level, preview_surface);
        // 真实命令的 before 快照在第一个重放事件时才拍；此处 begin 只负责
        // 目标层/计时/分段。预览笔画完全没有碰真实表面。
        self.brushwork.begin(ctx, false, None);
        self.preview = Some(PreviewStroke {
            level,
            scale,
            brush: preview_brush,
            state: preview_state,
            real_events: VecDeque::new(),
            last_pos: (ev.x, ev.y),
            started: Instant::now(),
            events: 0,
        });
        if preview_log() {
            eprintln!(
                "preview: start level={level} zoom={:.4} radius={base_radius:.1} scale={scale:.0}",
                ev.viewzoom
            );
        }
        true
    }

    /// 把事件画到预览面并把原始事件排进全分辨率队列。
    fn apply_preview_event(&mut self, ctx: &mut ModeContext<'_>, ev: StrokeEvent) -> bool {
        let Some(p) = self.preview.as_ref() else {
            return false;
        };
        let scale = p.scale;
        let preview_ev = StrokeEvent::new(
            ev.dtime,
            ev.x / scale,
            ev.y / scale,
            ev.pressure,
            ev.xtilt,
            ev.ytilt,
            1.0,
            ev.viewrotation,
            ev.barrel_rotation,
        );
        let Some(surface) = ctx.doc.paintable_surface_mut() else {
            return false;
        };
        let Some(preview_surface) = surface.preview_surface_mut() else {
            return false;
        };
        let p = self.preview.as_mut().expect("刚检查过");
        preview_surface.begin_atomic();
        preview_ev.apply(&p.brush, &mut p.state, preview_surface);
        preview_surface.end_atomic();
        p.real_events.push_back(ev);
        p.last_pos = (ev.x, ev.y);
        p.events += 1;
        true
    }

    /// 按时间预算把排队的输入重放到全分辨率表面。
    fn drain_preview_real(&mut self, ctx: &mut ModeContext<'_>, deadline: Option<Instant>) {
        let force_all = deadline.is_none();
        let started = Instant::now();
        let mut done = 0usize;
        loop {
            let Some(ev) = self
                .preview
                .as_ref()
                .and_then(|p| p.real_events.front().copied())
            else {
                break;
            };
            if let Some(d) = deadline {
                if Instant::now() >= d {
                    break;
                }
            }
            let pending = ctx.doc.paintable_surface().map_or(0, |s| s.gpu_pending_ops());
            if deadline.is_some() && pending >= gpu_dab_budget() {
                break;
            }
            if let Some(p) = self.preview.as_mut() {
                p.real_events.pop_front();
            }
            self.brushwork.stroke_to(
                ctx,
                ev.dtime,
                ev.x,
                ev.y,
                ev.pressure,
                ev.xtilt,
                ev.ytilt,
                ev.viewzoom,
                ev.viewrotation,
                ev.barrel_rotation,
                true,
            );
            done += 1;
            // 全分辨率事件每个都要做 smudge 采样（~8ms），一帧多做几个就会
            // 掉帧。预览在顶着画面，后台重放慢慢来：有预算时一帧只放一个。
            if deadline.is_some() {
                break;
            }
        }
        if done > 0 && force_all && preview_log() {
            eprintln!(
                "preview: full drain {done} events {:.0}ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
    }

    /// 重放全部排空且 GPU/回读都落地后，才能撤掉预览。
    fn preview_ready_to_clear(&self, ctx: &ModeContext<'_>) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|p| p.real_events.is_empty())
            && ctx
                .doc
                .paintable_surface()
                .is_none_or(|s| s.gpu_pending_ops() == 0)
    }

    fn clear_preview(&mut self, ctx: &mut ModeContext<'_>) {
        let Some(p) = self.preview.take() else {
            return;
        };
        if let Some(surface) = ctx.doc.paintable_surface_mut() {
            surface.clear_preview_overlay();
        }
        if preview_log() {
            eprintln!(
                "preview: done level={} events={} wall={:.0}ms",
                p.level,
                p.events,
                p.started.elapsed().as_secs_f64() * 1e3
            );
        }
    }

    /// 同步收尾（切笔刷/撤销/离开模式）：强制 flush，丢掉预览。
    fn force_finish_preview(&mut self, ctx: &mut ModeContext<'_>) {
        if self.preview.is_none() {
            return;
        }
        self.drain_preview_real(ctx, None);
        if let Some(surface) = ctx.doc.paintable_surface_mut() {
            surface.flush_gpu_dabs();
            surface.clear_preview_overlay();
        }
        if let Some(p) = self.preview.take() {
            if preview_log() {
                eprintln!(
                    "preview: forced level={} events={} wall={:.0}ms",
                    p.level,
                    p.events,
                    p.started.elapsed().as_secs_f64() * 1e3
                );
            }
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
        let viewzoom = ev.viewzoom.unwrap_or(1.0);
        let viewrotation = ev.viewrotation.unwrap_or(0.0);
        let barrel_rotation = ev.barrel_rotation.unwrap_or(0.0);
        let event = StrokeEvent::new(
            dtime,
            ev.x,
            ev.y,
            pressure,
            xtilt,
            ytilt,
            viewzoom,
            viewrotation,
            barrel_rotation,
        );
        if self.preview.is_none() {
            self.maybe_start_preview(ctx, &event);
        }
        if self.preview.is_some() {
            if !self.apply_preview_event(ctx, event) {
                // 预览面被外部撤掉（切层/快照）：退回真实路径，事件不能丢。
                self.preview = None;
                self.brushwork.stroke_to(
                    ctx,
                    dtime,
                    ev.x,
                    ev.y,
                    pressure,
                    xtilt,
                    ytilt,
                    viewzoom,
                    viewrotation,
                    barrel_rotation,
                    true,
                );
            }
        } else {
            self.brushwork.stroke_to(
                ctx,
                dtime,
                ev.x,
                ev.y,
                pressure,
                xtilt,
                ytilt,
                viewzoom,
                viewrotation,
                barrel_rotation,
                true,
            );
        }
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

    fn pending_inputs(&self) -> usize {
        self.pending_events.len()
            + self.draw.queue_len()
            + self
                .preview
                .as_ref()
                .map_or(0, |p| p.real_events.len())
    }

    fn instant_preview_state(&self) -> Option<(u32, usize)> {
        self.preview
            .as_ref()
            .map(|p| (p.level, p.real_events.len()))
    }

    fn last_painted_pos(&self) -> Option<(f32, f32)> {
        if let Some(p) = &self.preview {
            return Some(p.last_pos);
        }
        self.brushwork.last_pos().map(|p| (p.0, p.1))
    }

    /// 收下偏好推来的配置（`ModeStack::set_freehand_config`）。
    fn set_freehand_config(&mut self, config: FreehandConfig) {
        FreehandMode::set_freehand_config(self, config);
    }

    fn enter(&mut self, _ctx: &mut ModeContext<'_>) {
        self.draw = DrawingState::default();
        self.pending_events.clear();
        self.hiding_cursor = false;
    }

    fn leave(&mut self, ctx: &mut ModeContext<'_>) {
        self.process_pending_until(ctx, None);
        self.force_finish_preview(ctx);
        self.draw = DrawingState::default();
        self.pending_events.clear();
        self.hiding_cursor = false;
        self.release_pending = false;
        self.brushwork.commit(ctx, true);
    }

    fn checkpoint(&mut self, ctx: &mut ModeContext<'_>) {
        // 有未提交的改动就落账（不发零压力收尾，避免笔迹断裂）。
        // 预算延后的队列必须先画完，否则撤销/切笔刷会丢掉后半段笔迹。
        self.process_pending_until(ctx, None);
        self.force_finish_preview(ctx);
        self.finish_release_if_drained(ctx);
        self.brushwork.commit(ctx, false);
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !ctx.current_is_paintable() || ev.button != 1 {
            return false;
        }
        // 上一笔的收尾可能还在队列里（预算不足而延后）。预览路径下，
        // 全分辨率重放可能要几秒；如果用户在同一级别马上画下一笔，不能
        // 让他等重放结束——继续复用同一个预览面和 Brushwork，把两笔
        // 合并成同一条 undo 命令，重放在后台按顺序继续。
        let same_preview_level = self
            .preview
            .as_ref()
            .is_some_and(|p| p.level == preview_level_for_zoom(ev.viewzoom));
        if same_preview_level {
            self.release_pending = false;
        } else if self.release_pending || self.has_pending_events() || self.preview.is_some() {
            self.process_pending_until(ctx, None);
            self.force_finish_preview(ctx);
            self.finish_release_if_drained(ctx);
        }
        self.stroke_perf = Some((std::time::Instant::now(), Self::perf_snapshot()));
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
        // 松开的那一刻命令还没入栈。这里先给一小段时间预算：短笔画立即
        // 提交（观感与源码一致），高速/低缩放留下的大队列则交给后续帧，
        // 避免抬笔瞬间卡死。`finish_release_if_drained` 会在队列排空时提交。
        self.release_pending = true;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(4);
        self.process_pending_budgeted(ctx, deadline);
        true
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        self.motion_with_fake(ctx, ev, None)
    }

    fn key_press(&mut self, ctx: &mut ModeContext<'_>, key: Key) -> bool {
        // Escape：丢弃排队中的笔迹（`freehand.py` 文档里提到"按 Escape 可中止"）。
        // 预览笔画也要撤掉；已经重放进真实表面的部分用 brushwork rollback 复原。
        if key == Key::Escape {
            if self.preview.is_some() {
                if let Some(surface) = ctx.doc.paintable_surface_mut() {
                    surface.clear_preview_overlay();
                }
                self.preview = None;
                self.brushwork.rollback(ctx);
            }
            self.draw = DrawingState::default();
            self.pending_events.clear();
            self.release_pending = false;
            return true;
        }
        false
    }

    fn process_pending(&mut self, ctx: &mut ModeContext<'_>) -> bool {
        FreehandMode::process_pending(self, ctx);
        self.has_pending_events() || self.release_pending
    }

    fn process_pending_budgeted(
        &mut self,
        ctx: &mut ModeContext<'_>,
        deadline: std::time::Instant,
    ) -> bool {
        FreehandMode::process_pending_budgeted(self, ctx, deadline)
    }
}

/// 修一个常量名（`std::f32::consts` 在 2021 edition 里是 `consts`）。
mod consts_shim {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_mapping_is_piecewise_linear_with_inverted_y() {
        // 默认两点 (0,1),(1,0) = 1:1 关闭
        let cfg = FreehandConfig::default();
        assert!((cfg.map_pressure(0.37) - 0.37).abs() < 1e-6);
        // 自定义：x=0→y=0, x=1→y=0 在源码里表示 y'=1 常压
        let mut cfg = FreehandConfig::default();
        cfg.pressure_mapping = vec![(0.0, 0.0), (1.0, 0.0)];
        assert!((cfg.map_pressure(0.0) - 1.0).abs() < 1e-6);
        assert!((cfg.map_pressure(0.5) - 1.0).abs() < 1e-6);
        // 折线：(0,1) -> y'=0，(0.5,0.5) -> y'=0.5，(1,0) -> y'=1
        let mut cfg = FreehandConfig::default();
        cfg.pressure_mapping = vec![(0.0, 1.0), (0.5, 0.5), (1.0, 0.0)];
        assert!((cfg.map_pressure(0.25) - 0.25).abs() < 1e-6);
        assert!((cfg.map_pressure(0.5) - 0.5).abs() < 1e-6);
        assert!((cfg.map_pressure(0.75) - 0.75).abs() < 1e-6);
        // 端点夹取
        assert_eq!(cfg.map_pressure(-1.0), 0.0);
        assert_eq!(cfg.map_pressure(2.0), 1.0);
    }

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
            assert!(
                cooked.contains(ev),
                "带轴原始事件应原样出现在输出里: {ev:?}"
            );
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
        assert!(
            times.windows(2).all(|w| w[0] <= w[1]),
            "时刻必须单调: {times:?}"
        );
    }

    /// 预算处理：一次调用只推进队列的一部分，剩余留待后续帧；
    /// 抬笔后由 `finish_release_if_drained` 在队列排空时提交一条 Brushwork。
    #[test]
    fn budgeted_processing_drains_over_frames_and_commits_once() {
        use crate::document::Document;
        use std::time::{Duration, Instant};

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
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FreehandMode::new();
        let before = ctx.doc.command_stack.undo_len();

        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
        // 队列里堆 200 个事件，但先不处理。
        for i in 1..=200 {
            let ev = PointerEvent::motion(10.0 + i as f32, 10.0 + (i as f32) * 0.25, i as f64);
            assert!(InteractionMode::motion(&mut mode, &mut ctx, &ev));
        }
        assert!(!mode.draw.queue_is_empty());
        // 立即到期的预算：必须仍有剩余，且不能提前提交。
        assert!(InteractionMode::process_pending_budgeted(
            &mut mode,
            &mut ctx,
            Instant::now()
        ));
        assert_eq!(ctx.doc.command_stack.undo_len(), before);

        // 逐帧排空。
        let mut guard = 0;
        while InteractionMode::process_pending_budgeted(
            &mut mode,
            &mut ctx,
            Instant::now() + Duration::from_millis(2),
        ) {
            guard += 1;
            assert!(guard < 10_000, "预算没有推进队列");
        }
        // 松开：提交这一笔。
        let release = PointerEvent::button(210.0, 60.0, 201.0, 1);
        assert!(InteractionMode::button_release(
            &mut mode, &mut ctx, &release
        ));
        let mut guard = 0;
        while InteractionMode::process_pending_budgeted(
            &mut mode,
            &mut ctx,
            Instant::now() + Duration::from_millis(2),
        ) {
            guard += 1;
            assert!(guard < 10_000);
        }
        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before + 1,
            "排空后应恰好提交一条 Brushwork"
        );
    }

    /// Instant Preview：低缩放大笔刷先显示预览，真实笔画延后重放；
    /// 重放结果必须与不启用预览时**逐字节一致**。
    #[test]
    fn instant_preview_matches_full_res_stroke() {
        use crate::document::Document;
        use crate::hokusai::BrushSetting;
        use pigment_surface::tile_bytes;
        use std::collections::BTreeMap;

        fn tiles(doc: &Document) -> BTreeMap<(i32, i32), Vec<u8>> {
            doc.paintable_surface()
                .unwrap()
                .tiles()
                .map(|(c, t)| (c, tile_bytes(t.pixels()).to_vec()))
                .collect()
        }

        fn run(preview: bool) -> Document {
            let mut doc = Document::new();
            let mut brush = crate::brushlib::default_brush();
            brush
                .get_mut(BrushSetting::Radius)
                .base_value = 400f32.ln();
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
                color: [0.0, 0.0, 0.0],
                picked_color: None,
            };
            let mut mode = FreehandMode::new();
            mode.config.instant_preview = preview;
            let zoom = 0.0625;
            let ev = |x: f32, y: f32, t: f64, button: bool| {
                let mut e = if button {
                    PointerEvent::button(x, y, t, 1)
                } else {
                    PointerEvent::motion(x, y, t)
                };
                e.viewzoom = zoom;
                e
            };
            let press = ev(100.0, 100.0, 0.0, true);
            assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
            let motion = |mode: &mut FreehandMode,
                              ctx: &mut ModeContext<'_>,
                              i: i32| {
                let e = ev(100.0 + i as f32 * 20.0, 100.0 + i as f32 * 10.0, i as f64 * 8.0, false);
                assert!(InteractionMode::motion(mode, ctx, &e));
            };
            for i in 1..=20 {
                motion(&mut mode, &mut ctx, i);
            }
            let release = ev(100.0 + 20.0 * 20.0, 100.0 + 20.0 * 10.0, 168.0, true);
            assert!(InteractionMode::button_release(
                &mut mode, &mut ctx, &release
            ));
            mode.process_pending(&mut ctx);
            assert!(mode.preview.is_none(), "排空后必须撤掉预览");
            assert!(
                !ctx.doc
                    .paintable_surface()
                    .unwrap()
                    .has_preview_overlay(),
                "真实表面不能残留预览覆盖"
            );
            assert_eq!(ctx.doc.command_stack.undo_len(), 1);
            drop(ctx);
            doc
        }

        // 先单独验证预览确实挂上了：只喂前两个事件并立即到期，预览重放
        // 不会在本帧跑完。
        {
            let mut doc = Document::new();
            let mut brush = crate::brushlib::default_brush();
            brush
                .get_mut(BrushSetting::Radius)
                .base_value = 400f32.ln();
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
                color: [0.0, 0.0, 0.0],
                picked_color: None,
            };
            let mut mode = FreehandMode::new();
            mode.config.instant_preview = true;
            let mut e = PointerEvent::button(100.0, 100.0, 0.0, 1);
            e.viewzoom = 0.0625;
            assert!(InteractionMode::button_press(&mut mode, &mut ctx, &e));
            let mut m = PointerEvent::motion(120.0, 110.0, 8.0);
            m.viewzoom = 0.0625;
            assert!(InteractionMode::motion(&mut mode, &mut ctx, &m));
            mode.process_pending_until(&mut ctx, Some(Instant::now()));
            let mut m2 = PointerEvent::motion(140.0, 120.0, 16.0);
            m2.viewzoom = 0.0625;
            assert!(InteractionMode::motion(&mut mode, &mut ctx, &m2));
            mode.process_pending_until(&mut ctx, Some(Instant::now()));
            let mut m3 = PointerEvent::motion(160.0, 130.0, 24.0);
            m3.viewzoom = 0.0625;
            assert!(InteractionMode::motion(&mut mode, &mut ctx, &m3));
            mode.process_pending_until(&mut ctx, Some(Instant::now()));
            assert!(mode.preview.is_some(), "第三个事件后应进入预览");
            assert_eq!(mode.preview.as_ref().unwrap().level, 4);
            assert!(ctx
                .doc
                .paintable_surface()
                .unwrap()
                .has_preview_overlay());
            let _ = InteractionMode::key_press(&mut mode, &mut ctx, Key::Escape);
            assert!(mode.preview.is_none());
        }

        let preview_doc = run(true);
        let control_doc = run(false);
        assert_eq!(
            tiles(&preview_doc),
            tiles(&control_doc),
            "预览重放必须与实时全分辨率笔画逐字节一致"
        );
    }

    /// 回归：零位移事件不能因为“时间切分”被反复放回队首。
    ///
    /// 早期实现把切分比例无条件 `clamp(.., 0.5)`，于是 dist=0、dt>0 的
    /// 事件每轮都切一半时间却没有任何位移进展，队列永远排不空。
    #[test]
    fn zero_distance_motion_is_consumed_once() {
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
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FreehandMode::new();
        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
        mode.process_pending(&mut ctx);

        // 第一个带时间的事件只定基准；第二个同位置事件才会走切分路径。
        let first = PointerEvent::motion(10.0, 10.0, 8.0);
        assert!(InteractionMode::motion(&mut mode, &mut ctx, &first));
        mode.process_pending(&mut ctx);
        let second = PointerEvent::motion(10.0, 10.0, 16.0);
        assert!(InteractionMode::motion(&mut mode, &mut ctx, &second));
        mode.process_pending(&mut ctx);
        assert!(
            !mode.has_pending_events(),
            "零位移事件必须一次消费完，不能无限切分"
        );
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
            let ev = PointerEvent::motion(
                10.0 + i as f32 * 8.0,
                10.0 + i as f32 * 4.0,
                i as f64 * 16.0,
            );
            assert!(InteractionMode::motion(&mut mode, &mut ctx, &ev));
            mode.process_pending(&mut ctx);
        }
        // 松开
        let release = PointerEvent::button(58.0, 34.0, 7.0 * 16.0, 1);
        assert!(InteractionMode::button_release(
            &mut mode, &mut ctx, &release
        ));
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
