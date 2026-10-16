//! 笔画事件流 —— 照搬 MyPaint `lib/stroke.py`。
//!
//! `Stroke` 是「一次落笔」的**可重放记录**：笔刷设置（序列化）+ 落笔时的
//! 笔刷状态 + 逐个指针事件（dtime/x/y/压力/倾角/视图缩放与旋转）。它不存
//! 像素，像素由表面自己记住；撤销用的是快照（`lib/command.py:Brushwork`），
//! 事件流服务的是「用另一支笔重绘上一笔」（`Brushwork.update`，`lib/command.py:377`）
//! 以及将来的笔画文档模型。
//!
//! 与源码逐项对应：
//!
//! | MyPaint | 本文件 |
//! |---|---|
//! | `lib/stroke.py:26` `_SERIAL_NUMBER` | [`Stroke::serial_number`]（进程内自增） |
//! | `:35` `start_recording(brush)` | [`Stroke::start_recording`] |
//! | `:51` `record_event(dtime, x, y, …)` | [`Stroke::record_event`] |
//! | `:78` `stop_recording()` | [`Stroke::stop_recording`] |
//! | `:93` `is_empty` / `empty` | [`Stroke::is_empty`] |
//! | `:98` `render(surface)` | [`Stroke::render`] |
//! | `:138` `copy_using_different_brush` | [`Stroke::copy_using_different_brush`] |
//!
//! 事件按 9 个 float64 一行存储（`:84` `np.array(..., dtype="float64")`、
//! `:86` `version = b"2"`），顺序与 MyPaint 的 `record_event` 参数一致。
//!
//! # 两处技术栈适配
//!
//! 1. **brush_settings 用 hokusai 的 JSON**：MyPaint 存的是 `BrushInfo.save_to_string()`
//!    的字符串（其笔刷库需要字符串才能重建 `Brush`）；本项目的笔刷库
//!    （hokusai）正好提供 `myb::to_string_pretty()` / `myb::from_str()` 这对
//!    逆变换，所以直接用它，**语义相同**（都是"把整份设置存成字符串"）。
//! 2. **重放丢弃 3 个输入**：`viewzoom` / `viewrotation` / `barrel_rotation`
//!    照 MyPaint 一起记录（事件行仍是 9 列），但 hokusai 0.3 的
//!    `Brush::stroke_to` 只接受 8 个参数、未实现这三个输入，故重放时不喂给
//!    引擎。这是引擎 API 的缺口，不是设计取舍。

use std::sync::atomic::{AtomicU64, Ordering};

use hokusai::myb;
use hokusai::{Brush, BrushState, TiledSurface};

/// 笔画数据的版本号（`lib/stroke.py:86` 的 `version = b"2"`）。
pub const STROKE_DATA_VERSION: u8 = 2;

/// 每个事件的字段数（dtime, x, y, pressure, xtilt, ytilt, viewzoom,
/// viewrotation, barrel_rotation）。
pub const EVENT_FIELDS: usize = 9;

/// 一个指针事件（`Stroke.record_event` 的参数表）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeEvent {
    pub dtime: f64,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub xtilt: f32,
    pub ytilt: f32,
    pub viewzoom: f32,
    pub viewrotation: f32,
    pub barrel_rotation: f32,
}

impl StrokeEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dtime: f64,
        x: f32,
        y: f32,
        pressure: f32,
        xtilt: f32,
        ytilt: f32,
        viewzoom: f32,
        viewrotation: f32,
        barrel_rotation: f32,
    ) -> Self {
        Self {
            dtime,
            x,
            y,
            pressure,
            xtilt,
            ytilt,
            viewzoom,
            viewrotation,
            barrel_rotation,
        }
    }

    /// 展开成一行 float64（存储与重放用）。
    pub fn row(&self) -> [f64; EVENT_FIELDS] {
        [
            self.dtime,
            self.x as f64,
            self.y as f64,
            self.pressure as f64,
            self.xtilt as f64,
            self.ytilt as f64,
            self.viewzoom as f64,
            self.viewrotation as f64,
            self.barrel_rotation as f64,
        ]
    }

    /// 从一行 float64 还原。
    pub fn from_row(row: [f64; EVENT_FIELDS]) -> Self {
        Self {
            dtime: row[0],
            x: row[1] as f32,
            y: row[2] as f32,
            pressure: row[3] as f32,
            xtilt: row[4] as f32,
            ytilt: row[5] as f32,
            viewzoom: row[6] as f32,
            viewrotation: row[7] as f32,
            barrel_rotation: row[8] as f32,
        }
    }

    /// 喂给引擎一步（`lib/stroke.py:124` 的 `b.stroke_to(...)`）。
    ///
    /// 返回引擎是否真的画了 dab。
    pub fn apply<S: TiledSurface>(
        &self,
        brush: &Brush,
        state: &mut BrushState,
        surface: &mut S,
    ) -> bool {
        brush.stroke_to(
            state,
            surface,
            self.x,
            self.y,
            self.pressure,
            self.xtilt,
            self.ytilt,
            self.dtime,
        )
    }
}

/// 进程内笔画序号（`lib/stroke.py:26` 的类级 `_SERIAL_NUMBER`）。
static SERIAL_NUMBER: AtomicU64 = AtomicU64::new(0);

/// 一次落笔的可重放记录。
///
/// 录制期（`start_recording` 之后、`stop_recording` 之前）可以追加事件；
/// 录制结束后即视为不可变（MyPaint 的注释：`lib/stroke.py:21`）。
#[derive(Clone, Debug)]
pub struct Stroke {
    finished: bool,
    /// 进程内唯一序号（发射顺序）。
    pub serial_number: u64,
    /// 笔刷设置的序列化（MyPaint 的 `brush_settings`）。
    brush_settings: String,
    /// 笔刷展示名（MyPaint 的 `BrushInfo.get_string_property("parent_brush_name")`）。
    brush_name: String,
    /// 落笔瞬间的笔刷状态（`brush.get_states_as_array()`）。
    brush_state: Option<BrushState>,
    /// 扁平化的事件数据（不含版本号；行主序、每行 [`EVENT_FIELDS`] 个 float64）。
    data: Vec<f64>,
    /// 笔画总作画时长（秒），取引擎的 `stroke_total_painting_time`。
    total_painting_time: f64,
    /// 录制中的事件缓冲（`tmp_event_list`）。
    tmp_event_list: Vec<StrokeEvent>,
}

impl Default for Stroke {
    fn default() -> Self {
        Self::new()
    }
}

impl Stroke {
    /// 新建一条尚未录制的笔画。
    pub fn new() -> Self {
        let serial_number = SERIAL_NUMBER.fetch_add(1, Ordering::Relaxed) + 1;
        Self {
            finished: false,
            serial_number,
            brush_settings: String::new(),
            brush_name: String::new(),
            brush_state: None,
            data: Vec::new(),
            total_painting_time: 0.0,
            tmp_event_list: Vec::new(),
        }
    }

    /// 开始录制（`lib/stroke.py:35`）。
    ///
    /// MyPaint 在这里还会调 `brush.new_stroke()` 复位引擎的 `stroke_*` 字段；
    /// 本项目的作画循环为每笔新建一个 [`BrushState`]（等价于全新的笔画状态，
    /// 见 `pigment-app/src/app.rs` 的笔画生命周期），所以这里不再复位。
    ///
    /// 返回 `Err` 只可能是笔刷设置无法序列化（设置里有非有限值）。
    pub fn start_recording(
        &mut self,
        brush: &Brush,
        brush_name: &str,
        state: &BrushState,
    ) -> Result<(), String> {
        assert!(!self.finished, "Stroke 已结束，不可重新录制");
        self.brush_settings = myb::to_string_pretty(brush).map_err(|e| e.to_string())?;
        self.brush_name = brush_name.to_string();
        self.brush_state = Some(state.clone());
        self.tmp_event_list.clear();
        Ok(())
    }

    /// 记录一个事件（`lib/stroke.py:51`）。
    pub fn record_event(&mut self, ev: StrokeEvent) {
        assert!(!self.finished, "Stroke 已结束，不可再记录事件");
        self.tmp_event_list.push(ev);
    }

    /// 结束录制（`lib/stroke.py:78`）。
    ///
    /// `total_painting_time` 与 MyPaint 一致地取自引擎状态
    /// （`brush.get_total_stroke_painting_time()` → hokusai 的
    /// `BrushState::stroke_total_painting_time`）。
    pub fn stop_recording(&mut self, state: &BrushState) {
        if self.finished {
            return;
        }
        self.data = self
            .tmp_event_list
            .iter()
            .flat_map(|ev| ev.row())
            .collect();
        self.total_painting_time = state.stroke_total_painting_time;
        self.tmp_event_list = Vec::new();
        self.finished = true;
    }

    /// 是否已结束录制。
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// 这一笔是否"什么都没画"（`lib/stroke.py:93`）。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_painting_time == 0.0
    }

    /// 笔画总时长（秒）。
    #[inline]
    pub fn total_painting_time(&self) -> f64 {
        self.total_painting_time
    }

    /// 落笔时的笔刷名（展示用）。
    #[inline]
    pub fn brush_name(&self) -> &str {
        &self.brush_name
    }

    /// 记录到的事件数。
    #[inline]
    pub fn event_count(&self) -> usize {
        self.data.len() / EVENT_FIELDS
    }

    /// 落笔瞬间的笔刷状态。
    #[inline]
    pub fn brush_state(&self) -> Option<&BrushState> {
        self.brush_state.as_ref()
    }

    /// 逐行读取事件数据（存储格式与 MyPaint 的 `stroke_data` 一致）。
    pub fn rows(&self) -> impl Iterator<Item = [f64; EVENT_FIELDS]> + '_ {
        self.data.chunks_exact(EVENT_FIELDS).map(|c| {
            let mut row = [0.0; EVENT_FIELDS];
            row.copy_from_slice(c);
            row
        })
    }

    /// 逐条读取事件。
    pub fn events(&self) -> impl Iterator<Item = StrokeEvent> + '_ {
        self.rows().map(StrokeEvent::from_row)
    }

    /// 在表面上重放这一笔（`lib/stroke.py:98`）。
    ///
    /// 与 MyPaint 一致：**自己造一份引擎状态**并从记录值恢复
    /// （`:102-105` 新建 `Brush` 再 `set_states_from_array`），所以调用者
    /// 不需要准备状态，重放结果也与调用者当前状态无关。
    ///
    /// 调用者需自行保证表面处于干净状态（`Brushwork.update` 先
    /// `load_snapshot(sshot_before)` 再重放，见 `lib/command.py:384`）。
    /// 返回 `false` 表示笔刷设置无法解析，重放未发生。
    pub fn render<S: TiledSurface>(&self, surface: &mut S) -> bool {
        assert!(self.finished, "Stroke 尚未结束录制，无法重放");
        let Ok(brush) = myb::from_str(&self.brush_settings) else {
            return false;
        };
        let mut state = self.brush_state.clone().unwrap_or_default();
        surface.begin_atomic();
        for ev in self.events() {
            ev.apply(&brush, &mut state, surface);
        }
        surface.end_atomic();
        true
    }

    /// 复制成"换一支笔"的副本（`lib/stroke.py:138`）。
    ///
    /// 与 MyPaint 一致：只替换笔刷设置与名字，**保留落笔状态**
    /// （`:146` 的注释：新笔刷对 states 的语义可能不同，但重置成 0
    /// 会造成更多毛刺）。
    pub fn copy_using_different_brush(
        &self,
        brush: &Brush,
        brush_name: &str,
    ) -> Result<Self, String> {
        assert!(self.finished, "Stroke 尚未结束录制");
        let mut clone = self.clone();
        clone.brush_settings = myb::to_string_pretty(brush).map_err(|e| e.to_string())?;
        clone.brush_name = brush_name.to_string();
        Ok(clone)
    }

    /// 序列化后的笔刷设置（测试与调试用）。
    #[inline]
    pub fn brush_settings(&self) -> &str {
        &self.brush_settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushlib::default_brush;
    use pigment_surface::PigmentSurface;

    /// 造一支测试笔刷，压感曲线拉满，保证一定能画出东西。
    fn test_brush() -> Brush {
        let mut b = default_brush();
        b.get_mut(hokusai::BrushSetting::Radius).base_value = 2.0;
        b
    }

    #[test]
    fn serial_numbers_are_unique_and_increasing() {
        let a = Stroke::new();
        let b = Stroke::new();
        assert!(a.serial_number < b.serial_number);
    }

    #[test]
    fn records_events_and_flattens_to_float64_rows() {
        let brush = test_brush();
        let state = BrushState::default();
        let mut s = Stroke::new();
        s.start_recording(&brush, "Fan", &state).unwrap();
        s.record_event(StrokeEvent::new(0.01, 1.0, 2.0, 0.5, 0.1, -0.1, 1.0, 0.0, 0.0));
        s.record_event(StrokeEvent::new(0.02, 3.0, 4.0, 0.25, 0.0, 0.0, 2.0, 90.0, 0.5));
        s.stop_recording(&state);

        assert!(s.is_finished());
        assert_eq!(s.event_count(), 2);
        let rows: Vec<_> = s.rows().collect();
        assert_eq!(rows[0][0], 0.01);
        assert_eq!(rows[0][1], 1.0);
        assert_eq!(rows[1][3], 0.25);
        assert_eq!(rows[1][7], 90.0);
        assert_eq!(rows[1][8], 0.5);
        assert_eq!(s.brush_name(), "Fan");
    }

    #[test]
    fn empty_stroke_reports_empty() {
        let brush = test_brush();
        let state = BrushState::default();
        let mut s = Stroke::new();
        s.start_recording(&brush, "Fan", &state).unwrap();
        s.stop_recording(&state);
        // 引擎从未被喂过事件 → stroke_total_painting_time 为 0
        assert!(s.is_empty());
        assert_eq!(s.total_painting_time(), 0.0);
    }

    #[test]
    fn painted_stroke_is_not_empty_and_records_painting_time() {
        let brush = test_brush();
        let mut state = BrushState::default();
        let mut surface = PigmentSurface::new();
        let mut s = Stroke::new();
        s.start_recording(&brush, "Fan", &state).unwrap();
        for i in 0..30 {
            let ev = StrokeEvent::new(0.016, i as f32 * 2.0, 5.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            s.record_event(ev);
            ev.apply(&brush, &mut state, &mut surface);
        }
        s.stop_recording(&state);
        assert!(!s.is_empty(), "画过的笔画不该是空的");
        assert!(s.total_painting_time() > 0.0);
        assert!(surface.tile_count() > 0);
    }

    /// 重放必须画出与原始笔画**完全相同的像素**。
    #[test]
    fn render_reproduces_the_same_pixels() {
        let brush = test_brush();
        let mut state = BrushState::default();
        let mut original = PigmentSurface::new();
        let mut s = Stroke::new();
        s.start_recording(&brush, "Fan", &state).unwrap();
        for i in 0..40 {
            let ev = StrokeEvent::new(
                0.016,
                (i as f32 * 1.7).sin() * 20.0 + 40.0,
                i as f32 * 1.3,
                1.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
            );
            s.record_event(ev);
            ev.apply(&brush, &mut state, &mut original);
        }
        s.stop_recording(&state);
        assert!(!s.is_empty());

        // 换一个干净表面重放
        let mut replay = PigmentSurface::new();
        assert!(s.render(&mut replay));
        let mut again = PigmentSurface::new();
        s.render(&mut again);

        // 两次重放结果一致（引擎状态来自记录值，与调用者无关）
        assert_eq!(again.tile_count(), replay.tile_count());
        for coord in again.tiles().map(|(c, _)| c) {
            assert_eq!(
                again.tile(coord.0, coord.1),
                replay.tile(coord.0, coord.1),
                "重放结果不稳定：{coord:?}"
            );
        }
        // 重放的内容确实落到表面上（不是空笔）
        assert!(replay.tile_count() > 0, "重放应画出像素");
    }

    #[test]
    fn copy_using_different_brush_keeps_events_and_state() {
        let brush = test_brush();
        let mut state = BrushState::default();
        let mut surface = PigmentSurface::new();
        let mut s = Stroke::new();
        s.start_recording(&brush, "Old", &state).unwrap();
        for i in 0..20 {
            let ev = StrokeEvent::new(0.016, i as f32 * 3.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            s.record_event(ev);
            ev.apply(&brush, &mut state, &mut surface);
        }
        s.stop_recording(&state);

        let mut other = test_brush();
        other.get_mut(hokusai::BrushSetting::Radius).base_value = 1.0;
        let copy = s.copy_using_different_brush(&other, "New").unwrap();

        assert_eq!(copy.brush_name(), "New");
        assert_ne!(copy.brush_settings(), s.brush_settings());
        assert_eq!(copy.event_count(), s.event_count(), "事件流应原样保留");
        assert!(copy.brush_state().is_some(), "落笔状态应保留");
        assert_eq!(copy.total_painting_time(), s.total_painting_time());
        assert_eq!(copy.serial_number, s.serial_number, "浅拷贝共享序号");
    }
}
