//! 平滑墨线模式 —— `gui/inktool.py` 的 `InkingMode`（核心捕获 + 样条重绘）。
//!
//! 源码分两阶段：**CAPTURE** 按距离/时间阈值采控制点，再用 Catmull-Rom
//! 样条插值成密集的笔刷事件；**ADJUST** 让用户在画布上拖节点、增删/简化。
//!
//! 本移植范围：
//! - ✅ 捕获阈值（`MAX_INTERNODE_DISTANCE_MIDDLE/ENDS`、`MAX_INTERNODE_TIME`）；
//! - ✅ `spline_iter` + `_draw_curve_segment`（步数上限、`t` 单调、压力/倾角夹取）；
//! - ✅ evdev 零压力/零倾角回退、镜像倾角、假压力 0.5；
//! - ✅ 每次追加节点后**整条曲线重绘**（源码 `_queue_redraw_curve` 的语义），
//!   收笔时提交为一条 Brushwork；
//! - ⏳ 未移植：ADJUST 阶段的节点命中/拖动、接受/拒绝按钮、节点覆盖层、
//!   `simplify_nodes`/`cull_nodes`、恢复默认点距的选项面板。这些记账在
//!   比对报告附录 A。
//!
//! 因此本模式当前等价于「画完立即固定」的 Inking：曲线形状/压力与源码的
//! 捕获+插值一致，只是不能在画布上继续调节点。

use super::freehand::spline_4p;
use super::{
    Behavior, BrushworkState, InteractionMode, Key, ModeContext, PointerEvent,
};

/// 中间节点的最大显示像素间距（`MAX_INTERNODE_DISTANCE_MIDDLE`）。
pub const MAX_INTERNODE_DISTANCE_MIDDLE: f32 = 30.0;
/// 首/次节点的最大显示像素间距（`MAX_INTERNODE_DISTANCE_ENDS`）。
pub const MAX_INTERNODE_DISTANCE_ENDS: f32 = 10.0;
/// 节点之间的最短时间（秒，`MAX_INTERNODE_TIME = 1/100`）。
pub const MAX_INTERNODE_TIME: f32 = 1.0 / 100.0;
/// 插值切片的最长时间（`INTERPOLATION_MAX_SLICE_TIME = 1/200`）。
pub const INTERPOLATION_MAX_SLICE_TIME: f32 = 1.0 / 200.0;
/// 插值切片的最大模型距离（`INTERPOLATION_MAX_SLICE_DISTANCE = 20`）。
pub const INTERPOLATION_MAX_SLICE_DISTANCE: f32 = 20.0;
/// 每段最大切片数（`MAX_INTERNODE_DISTANCE_MIDDLE * 5`）。
pub const INTERPOLATION_MAX_SLICES: f32 = MAX_INTERNODE_DISTANCE_MIDDLE * 5.0;

/// 一个捕获到的控制点（`inktool.py:61` 的 `_Node`，9 个字段同源码）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkNode {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub xtilt: f32,
    pub ytilt: f32,
    /// 绝对时间（秒）。
    pub time: f32,
    pub viewzoom: f32,
    pub viewrotation: f32,
    pub barrel_rotation: f32,
}

impl InkNode {
    fn row(&self) -> [f32; 9] {
        [
            self.x,
            self.y,
            self.pressure,
            self.xtilt,
            self.ytilt,
            self.time,
            self.viewzoom,
            self.viewrotation,
            self.barrel_rotation,
        ]
    }

    fn from_row(r: [f32; 9]) -> Self {
        Self {
            x: r[0],
            y: r[1],
            pressure: r[2],
            xtilt: r[3],
            ytilt: r[4],
            time: r[5],
            viewzoom: r[6],
            viewrotation: r[7],
            barrel_rotation: r[8],
        }
    }
}

/// `gui/drawutils.py:96` 的 `spline_iter`：首尾各复制一次，产出
/// `(p[-1], p0, p1, p2)` 四元组；`t=0` 在 p0、`t=1` 在 p1。
pub fn spline_iter(nodes: &[InkNode]) -> Vec<[InkNode; 4]> {
    let mut out = Vec::new();
    if nodes.is_empty() {
        return out;
    }
    let mut cint: [Option<InkNode>; 4] = [None; 4];
    // double_first: 把第一个点放进 cint[3]，其余仍是 None，循环第一次
    // 移位后形成 p[-1]=p0。
    cint[3] = Some(nodes[0]);
    for n in nodes {
        cint[0] = cint[1];
        cint[1] = cint[2];
        cint[2] = cint[3];
        cint[3] = Some(*n);
        if let [Some(a), Some(b), Some(c), Some(d)] = cint {
            out.push([a, b, c, d]);
        }
    }
    // double_last
    cint[0] = cint[1];
    cint[1] = cint[2];
    cint[2] = cint[3];
    cint[3] = Some(*nodes.last().unwrap());
    if let [Some(a), Some(b), Some(c), Some(d)] = cint {
        out.push([a, b, c, d]);
    }
    out
}

/// 逐轴 Catmull-Rom（`spline_4p`）。
fn spline_node(t: f32, p: [InkNode; 4]) -> InkNode {
    let a = p[0].row();
    let b = p[1].row();
    let c = p[2].row();
    let d = p[3].row();
    let mut out = [0.0f32; 9];
    for i in 0..9 {
        out[i] = spline_4p(t, a[i], b[i], c[i], d[i]);
    }
    InkNode::from_row(out)
}

/// 平滑墨线模式。
pub struct InkingMode {
    nodes: Vec<InkNode>,
    brushwork: BrushworkState,
    dragging: bool,
    /// 按下时记录，用于 evdev 零值回退（源码 `_button_down`）。
    button_down: Option<u32>,
    last_good_pressure: f32,
    last_good_xtilt: f32,
    last_good_ytilt: f32,
    /// 上一个事件节点的 `(显示 x, 显示 y, 时间秒)`。
    last_node_evdata: Option<(f32, f32, f32)>,
    last_event_node: Option<InkNode>,
    start: (f32, f32),
}

impl Default for InkingMode {
    fn default() -> Self {
        Self::new()
    }
}

impl InkingMode {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            brushwork: BrushworkState::default(),
            dragging: false,
            button_down: None,
            last_good_pressure: 0.0,
            last_good_xtilt: 0.0,
            last_good_ytilt: 0.0,
            last_node_evdata: None,
            last_event_node: None,
            start: (0.0, 0.0),
        }
    }

    /// 已捕获的节点（测试/诊断）。
    pub fn nodes(&self) -> &[InkNode] {
        &self.nodes
    }

    /// 从指针事件构造节点（`_get_event_data`）。
    fn event_node(&mut self, ev: &PointerEvent) -> InkNode {
        let pressure = self.event_pressure(ev);
        let (xtilt, ytilt) = self.event_tilt(ev);
        InkNode {
            x: ev.x,
            y: ev.y,
            pressure,
            xtilt,
            ytilt,
            time: (ev.time_ms / 1000.0) as f32,
            viewzoom: ev.viewzoom,
            viewrotation: ev.viewrotation,
            barrel_rotation: ev.barrel_rotation.unwrap_or(0.0),
        }
    }

    /// `_get_event_pressure`：缺压力轴时鼠标用 0.5；按着键时禁止零压力，
    /// 用上一个好值（evdev 补丁，`inktool.py:699-725`）。
    fn event_pressure(&mut self, ev: &PointerEvent) -> f32 {
        let mut pressure = ev.pressure.unwrap_or(0.0);
        if ev.pressure.is_none() && self.button_down.is_some() {
            pressure = 0.5;
        }
        if self.button_down.is_some() {
            if pressure == 0.0 {
                pressure = self.last_good_pressure;
            } else {
                self.last_good_pressure = pressure;
            }
        }
        pressure.clamp(0.0, 1.0)
    }

    /// `_get_event_tilt`：夹取 + evdev 零值回退（镜像由画布层处理）。
    fn event_tilt(&mut self, ev: &PointerEvent) -> (f32, f32) {
        let mut xtilt = ev.xtilt.unwrap_or(0.0).clamp(-1.0, 1.0);
        let mut ytilt = ev.ytilt.unwrap_or(0.0).clamp(-1.0, 1.0);
        if self.button_down.is_some() {
            if xtilt == 0.0 {
                xtilt = self.last_good_xtilt;
            } else {
                self.last_good_xtilt = xtilt;
            }
            if ytilt == 0.0 {
                ytilt = self.last_good_ytilt;
            } else {
                self.last_good_ytilt = ytilt;
            }
        }
        (xtilt, ytilt)
    }

    /// 整条曲线重绘（`_queue_redraw_curve`）：先回滚在录的一段，再 begin，
    /// 然后逐段样条喂给笔刷。每次追加节点都调用 —— 与源码的 O(n²) 重绘一致。
    fn redraw_curve(&mut self, ctx: &mut ModeContext<'_>) {
        self.brushwork.rollback(ctx);
        if self.nodes.len() < 2 {
            return;
        }
        self.brushwork
            .begin(ctx, true, Some("Inking".to_string()));
        let mut t_abs = self.nodes[0].time;
        for p in spline_iter(&self.nodes) {
            self.draw_segment(ctx, p, &mut t_abs);
        }
    }

    /// `_draw_curve_segment`：按时间/距离决定切片数，`t` 单调不减。
    fn draw_segment(&mut self, ctx: &mut ModeContext<'_>, p: [InkNode; 4], t_abs: &mut f32) {
        let dtime_p0_p1 = p[2].time - p[1].time;
        let steps_t = dtime_p0_p1 / INTERPOLATION_MAX_SLICE_TIME;
        let dist_p1_p2 = (p[2].x - p[3].x).hypot(p[2].y - p[3].y);
        let steps_d = dist_p1_p2 / INTERPOLATION_MAX_SLICE_DISTANCE;
        let steps = INTERPOLATION_MAX_SLICES
            .min(steps_t.max(steps_d).max(2.0))
            .ceil();
        let mut last_t_abs = *t_abs;
        for i in 0..=(steps as i32) {
            let t = i as f32 / steps;
            let mut point = spline_node(t, p);
            point.pressure = point.pressure.clamp(0.0, 1.0);
            point.xtilt = point.xtilt.clamp(-1.0, 1.0);
            point.ytilt = point.ytilt.clamp(-1.0, 1.0);
            let t_now = point.time.max(last_t_abs);
            let dtime = t_now - last_t_abs;
            last_t_abs = t_now;
            self.brushwork.stroke_to(
                ctx,
                dtime as f64,
                point.x,
                point.y,
                point.pressure,
                point.xtilt,
                point.ytilt,
                point.viewzoom,
                point.viewrotation,
                point.barrel_rotation,
                false,
            );
        }
        *t_abs = last_t_abs;
    }
}

impl InteractionMode for InkingMode {
    fn name(&self) -> &'static str {
        "Inking"
    }

    fn usage(&self) -> &str {
        "Draw, and then adjust smooth lines"
    }

    fn action_name(&self) -> Option<&'static str> {
        Some("InkingMode")
    }

    fn pointer_behavior(&self) -> Behavior {
        Behavior::PAINT_FREEHAND
    }

    fn scroll_behavior(&self) -> Behavior {
        Behavior::CHANGE_VIEW
    }

    fn is_painting(&self) -> bool {
        self.brushwork.is_active()
    }

    fn enter(&mut self, _ctx: &mut ModeContext<'_>) {
        self.nodes.clear();
        self.dragging = false;
        self.button_down = None;
        self.last_node_evdata = None;
        self.last_event_node = None;
    }

    fn leave(&mut self, ctx: &mut ModeContext<'_>) {
        self.brushwork.commit(ctx, true);
        self.dragging = false;
        self.button_down = None;
    }

    fn checkpoint(&mut self, ctx: &mut ModeContext<'_>) {
        self.brushwork.commit(ctx, false);
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != 1 || !ctx.current_is_paintable() {
            return false;
        }
        self.nodes.clear();
        self.last_node_evdata = None;
        self.last_event_node = None;
        self.dragging = true;
        self.button_down = Some(ev.button);
        self.last_good_pressure = 0.0;
        self.last_good_xtilt = 0.0;
        self.last_good_ytilt = 0.0;
        self.start = (ev.x, ev.y);
        let node = self.event_node(ev);
        self.nodes.push(node);
        self.last_node_evdata = Some((ev.x * ev.viewzoom, ev.y * ev.viewzoom, node.time));
        self.last_event_node = Some(node);
        true
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.dragging {
            return false;
        }
        let node = self.event_node(ev);
        let disp = (ev.x * ev.viewzoom, ev.y * ev.viewzoom);
        let append = match self.last_node_evdata {
            None => true,
            Some((lx, ly, lt)) => {
                if (disp.0, disp.1, node.time) == (lx, ly, lt) {
                    false
                } else {
                    let dist = (disp.0 - lx).hypot(disp.1 - ly);
                    let dt = node.time - lt;
                    let max_dist = if self.nodes.len() < 2 {
                        MAX_INTERNODE_DISTANCE_ENDS
                    } else {
                        MAX_INTERNODE_DISTANCE_MIDDLE
                    };
                    dist > max_dist && dt > MAX_INTERNODE_TIME
                }
            }
        };
        if append {
            self.nodes.push(node);
            self.last_node_evdata = Some((disp.0, disp.1, node.time));
            self.redraw_curve(ctx);
        }
        self.last_event_node = Some(node);
        true
    }

    fn button_release(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if ev.button != self.button_down.unwrap_or(1) || !self.dragging {
            return false;
        }
        self.dragging = false;
        self.button_down = None;
        if let Some(last) = self.last_event_node {
            if self.nodes.last().copied() != Some(last) {
                self.nodes.push(last);
            }
        }
        self.redraw_curve(ctx);
        self.brushwork.commit(ctx, true);
        true
    }

    fn key_press(&mut self, ctx: &mut ModeContext<'_>, key: Key) -> bool {
        if key == Key::Escape && (self.dragging || !self.nodes.is_empty()) {
            self.brushwork.rollback(ctx);
            self.nodes.clear();
            self.dragging = false;
            self.button_down = None;
            self.last_node_evdata = None;
            self.last_event_node = None;
            return true;
        }
        false
    }

    fn process_pending(&mut self, _ctx: &mut ModeContext<'_>) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    fn node(x: f32, y: f32, t: f32) -> InkNode {
        InkNode {
            x,
            y,
            pressure: 0.5,
            xtilt: 0.0,
            ytilt: 0.0,
            time: t,
            viewzoom: 1.0,
            viewrotation: 0.0,
            barrel_rotation: 0.0,
        }
    }

    #[test]
    fn spline_iter_doubles_first_and_last() {
        let nodes = vec![node(0.0, 0.0, 0.0), node(1.0, 0.0, 0.1), node(2.0, 0.0, 0.2)];
        let segs = spline_iter(&nodes);
        // n=3 → 2 段（首段重复首点、末段重复末点；中间段共用 n0/n1/n2）
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0][1].x, 0.0);
        assert_eq!(segs[0][2].x, 1.0);
        assert_eq!(segs[1][2].x, 2.0, "末段 p1 是最后一点");
        assert_eq!(segs[1][3].x, 2.0, "末段重复最后一点");
    }

    #[test]
    fn usage_matches_source() {
        let mode = InkingMode::new();
        assert_eq!(mode.name(), "Inking");
        assert_eq!(mode.usage(), "Draw, and then adjust smooth lines");
        assert_eq!(mode.action_name(), Some("InkingMode"));
    }

    #[test]
    fn capture_thresholds_and_one_undo_entry() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut picked = None;
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            color: [0.0, 0.0, 0.0],
            picked_color: Some(&mut picked),
        };
        let mut mode = InkingMode::new();
        let before = ctx.doc.command_stack.undo_len();
        let press = PointerEvent::button(0.0, 0.0, 0.0, 1);
        assert!(mode.button_press(&mut ctx, &press));
        // 每 40ms 走 40 模型 px（>30 display px 且 >10ms）→ 追加
        for i in 1..=10 {
            let ev = PointerEvent::motion(i as f32 * 40.0, 0.0, i as f64 * 40.0);
            assert!(mode.motion(&mut ctx, &ev));
        }
        let release = PointerEvent::button(400.0, 0.0, 440.0, 1);
        assert!(mode.button_release(&mut ctx, &release));
        assert!(mode.nodes().len() > 2, "应采到多个控制点");
        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before + 1,
            "一次 Inking 只入栈一条 Brushwork"
        );
        assert!(
            ctx.doc
                .layers()
                .current()
                .surface()
                .map(|s| s.tile_count())
                .unwrap_or(0)
                > 0
        );
    }
}
