//! 工作区 —— `MyPaintWorkspace`（`gui/workspace.py:32`）。
//!
//! 源码里这块是**可停靠面板系统**：`Gtk.VBox`，中央是画布
//! （`TiledDrawWidget`，glade 里 `expand=True`），四周挂 `ToolStack`
//! （`gui/toolstack.py`），每个工具面板都是一个 `Gtk.Notebook` 的一页，
//! 可以拖到另一侧、也可以拖出来变成独立窗口，布局本身可序列化
//! （`get_layout()` / `build_from_layout()`）。
//!
//! S6 的停靠语义（添加/移除/浮动/标签切换）由 [`crate::shell::dock`]
//! 承接（技术栈适配：egui_dock 0.21 只支持 egui ^0.36，与本项目锁定的
//! egui 0.37 不兼容，改为 egui 原生实现同样的模型）。本文件保留：
//! 中央画布（M2 以来的渲染关键路径）与笔刷编辑器面板的内容。
//!
//! 默认停靠（差异记账）：MyPaint 出厂布局里侧栏是空的
//! （`userconfig.py:104`，`workspace.layout` 只有 position/autohide），
//! 面板靠动作按需 reveal；本项目此前一直默认显示笔刷面板与编辑器，
//! 保留该初始状态：左 = BrushGroupTool，右 = BrushEditor。

use eframe::egui;
use pigment_engine::color::UIColor;

use pigment_engine::hokusai::BrushSetting;
use pigment_render::{mip_level_for_zoom, prepare_canvas, TileKey};

#[cfg(test)]
use pigment_engine::hokusai::tile::TILE_SIZE;
#[cfg(test)]
use pigment_render::MAX_VISIBLE_TILES_PER_AXIS;

use crate::app::{PigmentApp, ViewTool};
use crate::gpu::CanvasPaint;
use crate::view::View;

/// `VisualizeRendering`：给本帧上传的 tile 画边框。
/// `PIGMENT_FRAME_PERF=1` 时每 30 帧打一条画布帧分解。
///
/// 分段与 bench 的拆法一致：处理输入（引擎侧，大部分已在 GPU 队列里）／
/// `flush_gpu_dabs`（一次 submit + 回读）／`prepare_canvas`（合成 + 上传）。
/// 剩下的是 egui 布局、覆盖层与 present —— 用它判断"慢在画布还是慢在 UI"。
fn frame_perf() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PIGMENT_FRAME_PERF").is_some())
}

fn report_frame_perf(
    total_ns: u128,
    events_ns: u128,
    flush_ns: u128,
    prepare_ns: u128,
    visible: usize,
    uploads: usize,
    pending_left: usize,
    more: bool,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    static TOTAL: AtomicU64 = AtomicU64::new(0);
    static EVENTS: AtomicU64 = AtomicU64::new(0);
    static FLUSH: AtomicU64 = AtomicU64::new(0);
    static PREPARE: AtomicU64 = AtomicU64::new(0);
    static MAX_TOTAL: AtomicU64 = AtomicU64::new(0);
    TOTAL.fetch_add(total_ns as u64, Ordering::Relaxed);
    EVENTS.fetch_add(events_ns as u64, Ordering::Relaxed);
    FLUSH.fetch_add(flush_ns as u64, Ordering::Relaxed);
    PREPARE.fetch_add(prepare_ns as u64, Ordering::Relaxed);
    MAX_TOTAL.fetch_max(total_ns as u64, Ordering::Relaxed);
    // 表面侧计数器（跨线程累积）：回答「这一笔到底走 GPU 还是 CPU」。
    static P_GPU: AtomicU64 = AtomicU64::new(0);
    static P_CPU: AtomicU64 = AtomicU64::new(0);
    static P_FLUSH_NS: AtomicU64 = AtomicU64::new(0);
    static P_FLUSH_CALLS: AtomicU64 = AtomicU64::new(0);
    static P_FLUSH_TILES: AtomicU64 = AtomicU64::new(0);
    let cur_gpu = pigment_surface::perf::get(&pigment_surface::perf::DABS_GPU);
    let cur_cpu = pigment_surface::perf::get(&pigment_surface::perf::DABS_CPU);
    let cur_fns = pigment_surface::perf::get(&pigment_surface::perf::FLUSH_NS);
    let cur_fc = pigment_surface::perf::get(&pigment_surface::perf::FLUSH_CALLS);
    let cur_ft = pigment_surface::perf::get(&pigment_surface::perf::FLUSH_TILES);
    let d_gpu = cur_gpu.saturating_sub(P_GPU.swap(cur_gpu, Ordering::Relaxed));
    let d_cpu = cur_cpu.saturating_sub(P_CPU.swap(cur_cpu, Ordering::Relaxed));
    let d_fns = cur_fns.saturating_sub(P_FLUSH_NS.swap(cur_fns, Ordering::Relaxed));
    let d_fc = cur_fc.saturating_sub(P_FLUSH_CALLS.swap(cur_fc, Ordering::Relaxed));
    let d_ft = cur_ft.saturating_sub(P_FLUSH_TILES.swap(cur_ft, Ordering::Relaxed));

    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    // 作画帧一条一条打。用「本帧有 dab」而不是「帧时间超阈值」判定：
    // 事件预算把单帧压到 6–12ms，用 16ms 阈值永远抓不到真正作画的帧，
    // 而均值又被几千个空转帧稀释成 1ms 出头。
    if d_gpu + d_cpu > 0 || events_ns > 2_000_000 {
        eprintln!(
            "paint n={n} total={:.2}ms events={:.2} (sync {:.2}) flush={:.2} prepare={:.2} rest={:.2} | dabs gpu={d_gpu} cpu={d_cpu} flushes={d_fc} tiles={d_ft} | pending_left={pending_left} more={more}",
            total_ns as f64 / 1e6,
            events_ns as f64 / 1e6,
            d_fns as f64 / 1e6,
            flush_ns as f64 / 1e6,
            prepare_ns as f64 / 1e6,
            (total_ns as f64 - events_ns as f64 - flush_ns as f64 - prepare_ns as f64) / 1e6,
        );
    }
    if n % 30 == 0 {
        let f = n as f64;
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / f / 1e6;
        eprintln!(
            "frame n={n} total={:.2}ms (max {:.2}) events={:.2} flush={:.2} prepare={:.2} rest={:.2} | visible={visible} uploads={uploads} | gpu_dabs={d_gpu} cpu_dabs={d_cpu} flushes={d_fc} sync={:.2}ms tiles={d_ft}",
            get(&TOTAL),
            MAX_TOTAL.load(Ordering::Relaxed) as f64 / 1e6,
            get(&EVENTS),
            get(&FLUSH),
            get(&PREPARE),
            get(&TOTAL) - get(&EVENTS) - get(&FLUSH) - get(&PREPARE),
            d_fns as f64 / 1e6,
        );
    }
}

fn paint_redraw_debug(painter: &egui::Painter, canvas: egui::Rect, view: &View, tiles: &[TileKey]) {
    for k in tiles {
        let (wx, wy) = k.world_origin();
        let size = k.world_size() as f32;
        let [x0, y0] = view.world_to_screen(wx as f32, wy as f32);
        let [x1, y1] = view.world_to_screen(wx as f32 + size, wy as f32 + size);
        let r = egui::Rect::from_two_pos(
            canvas.min + egui::vec2(x0, y0),
            canvas.min + egui::vec2(x1, y1),
        );
        painter.rect_filled(
            r,
            0.0,
            egui::Color32::from_rgba_unmultiplied(255, 40, 40, 24),
        );
        painter.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 60, 60)),
            egui::StrokeKind::Middle,
        );
    }
}

/// `ScaleOverlay`（`overlays.py:141-193`）：右上角 "Zoom: xx.x%"。
fn paint_scale_feedback(painter: &egui::Painter, rect: egui::Rect, zoom: f32) {
    let text = format!("Zoom: {:.01}%", zoom * 100.0);
    let font = egui::FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(text, font, egui::Color32::from_rgb(238, 238, 238));
    let pad = 6.0;
    let size = galley.size() + egui::vec2(pad * 2.0, pad * 2.0);
    let pos = egui::pos2(rect.right() - size.x - 12.0, rect.top() + 12.0);
    let bg = egui::Rect::from_min_size(pos, size);
    painter.rect_filled(
        bg,
        4.0,
        egui::Color32::from_rgba_unmultiplied(18, 18, 18, 210),
    );
    painter.galley(pos + egui::vec2(pad, pad), galley, egui::Color32::WHITE);
}

/// `FadingOverlay` 的动画参数（`overlays.py:42-49,74-89`）。
const LAST_POS_FADE_FPS: f64 = 20.0;
const LAST_POS_FADE_DURATION: f64 = 1.5;

/// 当前淡出 alpha：每 1/20 秒减 `1/(20*1.5)`，1.5 秒整归零。
fn last_pos_alpha(started_at: f64, now: f64) -> f32 {
    let ticks = ((now - started_at).max(0.0) * LAST_POS_FADE_FPS).floor();
    let alpha = 1.0 - ticks / (LAST_POS_FADE_FPS * LAST_POS_FADE_DURATION);
    alpha.clamp(0.0, 1.0) as f32
}

/// `LastPaintPosOverlay`（`overlays.py:196-278`）：上一笔终点的 X 标记。
/// 填充拖拽路径：黑白交替虚线（`FloodFillOverlay.paint`，`fill.py:414-430`）。
fn paint_fill_drag(painter: &egui::Painter, path: &[egui::Pos2], ppp: f32) {
    if path.len() < 2 {
        return;
    }
    let dash = 9.0 / ppp;
    let width = 2.0 / ppp;
    let colors = [
        egui::Color32::from_black_alpha(204),
        egui::Color32::from_white_alpha(204),
    ];
    let mut color_idx = 0usize;
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = a.distance(b);
        if len <= f32::EPSILON {
            continue;
        }
        let dir = (b - a) / len;
        let mut t = 0.0;
        while t < len {
            let end = (t + dash).min(len);
            painter.line_segment(
                [a + dir * t, a + dir * end],
                egui::Stroke::new(width, colors[color_idx % 2]),
            );
            color_idx += 1;
            t = end;
        }
    }
}

fn paint_last_pos_feedback(
    painter: &egui::Painter,
    canvas: egui::Rect,
    view: &View,
    x: f32,
    y: f32,
    alpha: f32,
) {
    let [sx, sy] = view.world_to_screen(x, y);
    let c = canvas.min + egui::vec2(sx, sy);
    let r = 4.0;
    let a = |v: f32| (v * alpha).round().clamp(0.0, 255.0) as u8;
    let outer = egui::Stroke::new(
        8.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, a(200.0)),
    );
    let inner = egui::Stroke::new(
        6.0,
        egui::Color32::from_rgba_unmultiplied(236, 240, 241, a(255.0)),
    );
    for stroke in [outer, inner] {
        painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
        painter.line_segment([c + egui::vec2(r, -r), c + egui::vec2(-r, r)], stroke);
    }
}

impl PigmentApp {
    /// 执行一条 `input.button_mapping` 动作。
    ///
    /// 源码走 `get_handler_object()` 分三类（`gui/buttonmap.py:98-125`）：
    /// 模式类押栈、popup state、普通 GtkAction。这里：
    /// - 三个视图操纵模式要记住"是哪个鼠标键起手"（拖拽期间持续按它）；
    /// - `FrameEditMode` / `BrushResizeMode` 需要在按下点立刻转发一次 press；
    /// - 其余（含 `Undo`/`Bigger`/`FreehandMode`/`ColorPickMode` 等）直接走
    ///   [`PigmentApp::run_action`]，与菜单/加速键同一条路径。
    ///
    /// 返回是否真的消费了这次按钮（未实现的动作返回 `false`）。
    #[allow(clippy::too_many_arguments)]
    fn dispatch_mapped_button(
        &mut self,
        action: &str,
        btn: egui::PointerButton,
        _gdk_button: u32,
        pos: egui::Pos2,
        rect: egui::Rect,
        now_ms: f64,
        mods: pigment_engine::mode::Modifiers,
        ctx: &egui::Context,
    ) -> bool {
        match action {
            "PanViewMode" | "ZoomViewMode" | "RotateViewMode" => {
                let tool = match action {
                    "PanViewMode" => ViewTool::Pan,
                    "ZoomViewMode" => ViewTool::Zoom,
                    _ => ViewTool::Rotate,
                };
                self.view_tool = Some(tool);
                self.view_drag_button = btn;
                self.view_drag_prev = None;
                true
            }
            "ShowPopupMenu" => {
                self.popup_menu = Some(pos);
                self.popup_opened = now_ms / 1000.0;
                true
            }
            "FrameEditMode" => {
                let mode: Box<dyn pigment_engine::mode::InteractionMode> =
                    Box::new(pigment_engine::mode::frameedit::FrameEditMode::new());
                self.with_mode(|modes, mctx| modes.push(mode, mctx));
                self.mapped_mode_button = Some(btn);
                let ev = self.pointer_event(pos, rect, now_ms, 1, mods);
                self.mode_button_press(ev);
                true
            }
            "BrushResizeMode" => {
                self.begin_brush_resize(now_ms / 1000.0);
                true
            }
            _ => self.run_action(action, ctx, now_ms / 1000.0),
        }
    }

    /// 把一个帧间位移拆成若干小步喂给绘画模式。
    ///
    /// 以前每帧只喂"当前指针位置"这一个事件；高速笔画时一步就是几十个
    /// 屏幕像素（低缩放下是几百个画布像素），引擎会同步生成上百个 dab，
    /// 单事件就能卡住整帧。拆成 ~3px 的小步后，处理预算可以在事件之间
    /// 停下来，把余下的交给后续帧（笔迹会短暂落后笔尖，但界面不卡）。
    fn dispatch_paint_motion(
        &mut self,
        pos: egui::Pos2,
        rect: egui::Rect,
        now_ms: f64,
        mods: pigment_engine::mode::Modifiers,
    ) {
        const STEP_PX: f32 = 2.0;
        const MAX_STEPS: usize = 128;
        let prev = self.paint_prev_pos.unwrap_or(pos);
        let prev_time = if self.paint_prev_time > 0.0 && self.paint_prev_time <= now_ms {
            self.paint_prev_time
        } else {
            now_ms
        };
        let delta = pos - prev;
        let steps = ((delta.length() / STEP_PX).ceil() as usize).clamp(1, MAX_STEPS);
        for k in 1..=steps {
            let f = k as f32 / steps as f32;
            let p = prev + delta * f;
            let t = prev_time + (now_ms - prev_time) * f as f64;
            let ev = self.pointer_event(p, rect, t, 0, mods);
            self.mode_motion_enqueue(ev);
        }
        self.paint_prev_pos = Some(pos);
        self.paint_prev_time = now_ms;
    }

    /// 把主视图中心挪到模型坐标 `(wx, wy)` —— `recenter_on_model_coords`。
    pub(crate) fn preview_recenter(&mut self, wx: f32, wy: f32, canvas: egui::Rect) {
        let c = canvas.center();
        self.view.offset = [c.x - wx * self.view.zoom, c.y - wy * self.view.zoom];
    }

    /// 本帧画布 pass 的清屏色（画布底色，sRGB 目标要换成线性值）。
    fn canvas_clear_color(&self) -> eframe::wgpu::Color {
        let [r, g, b] = crate::shell::chrome::canvas_backdrop_rgb(self.doc.layers());
        let ch = |v: u8| {
            let x = v as f32 / 255.0;
            let lin = if self.target_format.is_srgb() {
                pigment_surface::color::eotf_decode(x)
            } else {
                x
            };
            f64::from(lin)
        };
        eframe::wgpu::Color {
            r: ch(r),
            g: ch(g),
            b: ch(b),
            a: 1.0,
        }
    }

    /// 画布（`TiledDrawWidget`）—— 工作区的中央，`expand=True`。
    ///
    /// 这一段是 M2 打通的渲染关键路径，S1 只是把它从"整窗口中央"
    /// 挪进工作区中央：视图矩阵用的是 `content_rect`（全窗口）而非本面板的
    /// `rect`，所以压边之后**偏移量必须跟着面板原点走** —— 见下面
    /// `view_window.offset` 里那个 `rect.min`。
    /// 诊断用：`PIGMENT_SELFTEST_STROKE=1` 时按 240Hz 合成一笔。
    ///
    /// 数位板每秒报 133–240 个点，而事件预算只有 6–12ms/帧。大笔刷单点
    /// 成本一旦超过「帧时间 ÷ 每帧到达点数」，队列就会持续积压 —— 表现
    /// 就是笔迹追不上光标、松笔后还要排空很久。这条自检用**真实输入
    /// 路径**（mode_button_press / motion_enqueue / release）复现同一压力，
    /// 不依赖人手。
    fn selftest_stroke(&mut self, rect: egui::Rect) {
        struct St {
            t0: std::time::Instant,
            idx: u32,
            done: bool,
        }
        static ST: std::sync::Mutex<Option<St>> = std::sync::Mutex::new(None);
        const N: u32 = 200;
        const HZ: f64 = 240.0;

        let mut guard = match ST.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let st = guard.get_or_insert_with(|| {
            // 默认强制到 6.25%：低缩放下大笔刷才是"一格 tile 都装不下"的场景。
            // `PIGMENT_SELFTEST_ZOOM` / `PIGMENT_SELFTEST_RADIUS` 可在压测
            // 缩放级别切换时改成高缩放、大半径。
            let zoom: f32 = std::env::var("PIGMENT_SELFTEST_ZOOM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0625);
            let radius: f32 = std::env::var("PIGMENT_SELFTEST_RADIUS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(400.0);
            self.view.set_zoom_at(zoom, rect.center().x, rect.center().y);
            // 半径是 `base_value + 压感映射`，满压再 +2.0，所以这里预留掉。
            let log_r = radius.ln() - 2.0;
            self.brush
                .get_mut(pigment_engine::hokusai::BrushSetting::Radius)
                .base_value = log_r;
            self.base_brush
                .get_mut(pigment_engine::hokusai::BrushSetting::Radius)
                .base_value = log_r;
            eprintln!("selftest: start (zoom={:.4})", self.view.zoom);
            St {
                t0: std::time::Instant::now(),
                idx: 0,
                done: false,
            }
        });

        let now_ms = st.t0.elapsed().as_secs_f64() * 1000.0;
        let due = ((now_ms / 1000.0 * HZ) as u32).min(N + 1);
        while st.idx < due {
            let i = st.idx;
            st.idx += 1;
            let f = i as f32 / N as f32;
            // 屏幕上 800px 的快速横向笔画，带一点起伏。
            // `PIGMENT_SELFTEST_SPEED` 放大位移，复现真实快速笔画下
            // 每事件 16–32px 的输入压力。
            let speed: f32 = std::env::var("PIGMENT_SELFTEST_SPEED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0);
            let pos = egui::pos2(
                rect.min.x + 220.0 + f * 800.0 * speed,
                rect.min.y + 320.0 + (f * 9.0).sin() * 90.0,
            );
            let time_ms = i as f64 / HZ * 1000.0;
            if i == 0 {
                let ev = self.pointer_event(pos, rect, time_ms, 1, Default::default());
                self.mode_button_press(ev);
            } else if i < N {
                let ev = self.pointer_event(pos, rect, time_ms, 1, Default::default());
                self.mode_motion_enqueue(ev);
            } else {
                // 释放事件的 button 仍是 1（与真实鼠标/数位板一致）；
                // 早期这里传 0，FreehandMode::button_release 会直接忽略，
                // 合成笔画永远等不到抬笔。
                let ev = self.pointer_event(pos, rect, time_ms, 1, Default::default());
                self.mode_button_release(ev);
            }
        }
        if st.idx > N && !st.done && self.modes.top().pending_inputs() == 0 {
            st.done = true;
            eprintln!(
                "selftest: drained at {:.0}ms",
                st.t0.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    pub(crate) fn canvas(&mut self, ui: &mut egui::Ui) {
        let t_frame = frame_perf().then(std::time::Instant::now);
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        // 记下画布矩形：键盘缩放要用它算锚点（`handle_keys` 在画布之前跑，
        // 只能拿到上一帧的值 —— 与源码用"最后一次指针事件"同构）
        self.canvas_rect = Some(rect);

        // ---- 滚轮（`ScrollableModeMixin.scroll_cb`）----
        // 默认缩放、Ctrl/Alt 反转平移；Shift 走阶梯/离散步进。
        if response.hovered() {
            // `ui.support_smooth_scrolling`（`preferenceswindow.py:400-402`）：
            // 关掉时源码按"离散步进"处理滚轮（`tileddrawwidget` 的事件分类），
            // egui 侧对应 `raw_scroll_delta`（一格 50 点）而不是平滑增量。
            let smooth = self.prefs.bool_or("ui.support_smooth_scrolling", true);
            let (delta, imods) = ui.input(|i| {
                (
                    if smooth {
                        i.smooth_scroll_delta
                    } else {
                        stepwise_wheel(&i.raw.events, i.content_rect().height())
                    },
                    i.modifiers,
                )
            });
            if delta != egui::Vec2::ZERO {
                if let Some(pos) = response.hover_pos() {
                    self.apply_canvas_wheel(rect, pos, imods, delta);
                }
            }
        }

        // ---- 绘制 ----
        // B3-1：输入交给交互模式栈（`gui/mode.py`）。栈顶（默认 FreehandMode）
        // 自己排队、做压力/倾角插值，再由 `BrushworkModeMixin` 原子提交
        // （一次拖拽 = 一条 Brushwork，`lib/command.py:250`）。
        //
        // 文件弹层开着时禁掉画布交互（GTK 的模态对话框也是这个效果）；
        // 按住空格是"抓手"平移，不算绘画（`DragMode.key_press_cb` 的 space 分支
        // 在本移植里由 view 平移直接处理）。
        let space_down = ui.input(|i| i.key_down(egui::Key::Space));
        let modal = self.modal_active();
        // 指针时间戳：用应用启动以来的毫秒（没有 GTK 事件时间可用）
        let now_ms = ui.input(|i| i.time) * 1000.0;
        let mods = self.pointer_modifiers(ui.ctx());

        // ---- 指针按钮映射（`input.button_mapping`）----
        // 默认 Windows：中键弹菜单、右键平移；Shift 旋转、Ctrl/Alt 缩放、
        // Ctrl/Alt+Shift 进 FrameEditMode（`userconfig.py:127-139`）。
        let imods = ui.input(|i| i.modifiers);
        for (btn, gdk_button) in [
            (egui::PointerButton::Middle, 2u32),
            (egui::PointerButton::Secondary, 3u32),
        ] {
            let started = response.drag_started_by(btn) || response.clicked_by(btn);
            if !started {
                continue;
            }
            let Some(action) =
                self.mapped_button_action(gdk_button, imods.ctrl, imods.shift, imods.alt)
            else {
                continue;
            };
            if let Some(pos) = response.interact_pointer_pos() {
                self.dispatch_mapped_button(
                    &action,
                    btn,
                    gdk_button,
                    pos,
                    rect,
                    now_ms,
                    mods,
                    ui.ctx(),
                );
            }
        }

        // ---- 主键 + 修饰键（源码允许 `<Control>Button1` 这种绑定）----
        // 无修饰键的 Button1 固定给绘画，不查表（`buttonmap.py:601-614` 的
        // 编辑器同样拒绝它）。命中后整段按住的时期都不喂绘画模式。
        let primary_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
        let primary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
        if primary_pressed && response.hovered() && (imods.ctrl || imods.shift || imods.alt) {
            if let Some(action) = self.mapped_button_action(1, imods.ctrl, imods.shift, imods.alt) {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.primary_action_active = true;
                    self.dispatch_mapped_button(
                        &action,
                        egui::PointerButton::Primary,
                        1,
                        pos,
                        rect,
                        now_ms,
                        mods,
                        ui.ctx(),
                    );
                }
            }
        }

        // 空格 + 左键 = 假 Button2：源码把 space 注入 `input.button_mapping`
        // 的 Button2（`document.py:698-715`），Windows 默认是 ShowPopupMenu，
        // 不是硬编码平移。
        if space_down && primary_pressed && response.hovered() {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some(action) =
                    self.mapped_button_action(2, imods.ctrl, imods.shift, imods.alt)
                {
                    self.primary_action_active = true;
                    self.dispatch_mapped_button(
                        &action,
                        egui::PointerButton::Primary,
                        2,
                        pos,
                        rect,
                        now_ms,
                        mods,
                        ui.ctx(),
                    );
                }
            }
        }

        // 底栏上下文取色：按钮按下后进入"下一次画布点击执行 PickContext"。
        if self.pick_context_pending && primary_pressed && response.hovered() {
            self.pick_context_pending = false;
            self.primary_action_active = true;
            self.pick_context(ui.ctx(), now_ms / 1000.0);
        }

        // ---- 视图操纵工具（`gui/viewmanip.py` 的三个 Oneshot）----
        // 源码是压栈模式，拖拽结束后 pop 回原工具；这里用一次性 view_tool 状态。
        let view_gesture = self.view_tool.is_some();
        if view_gesture {
            let btn = self.view_drag_button;
            if response.drag_started_by(btn) {
                self.view_drag_prev = response
                    .interact_pointer_pos()
                    .map(|p| egui::pos2(p.x - rect.min.x, p.y - rect.min.y));
            }
            if response.dragged_by(btn) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let local = egui::pos2(pos.x - rect.min.x, pos.y - rect.min.y);
                    if let Some(prev) = self.view_drag_prev {
                        let c = egui::vec2(rect.width() * 0.5, rect.height() * 0.5);
                        if let Some(tool) = self.view_tool {
                            PigmentApp::apply_view_drag(&mut self.view, tool, prev, local, c);
                        }
                    }
                    self.view_drag_prev = Some(local);
                }
            }
            if response.drag_stopped_by(btn) || response.clicked_by(btn) {
                self.view_tool = None;
                self.view_drag_prev = None;
                self.view_drag_button = egui::PointerButton::Primary;
                // 源码不在画布点击后写瞬时状态：底栏每帧都显示
                // `mode.get_usage()`，再追加“模式名: 用法”属于重复内容。
            }
        }

        // ---- 按钮映射压栈的临时模式（FrameEditMode）----
        let mut mapped_mode_active = false;
        if let Some(btn) = self.mapped_mode_button {
            mapped_mode_active = true;
            let down = ui.input(|i| i.pointer.button_down(btn));
            if down {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let ev = self.pointer_event(pos, rect, now_ms, 0, mods);
                    self.mode_motion(ev);
                }
            } else {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let ev = self.pointer_event(pos, rect, now_ms, 1, mods);
                    self.mode_button_release(ev);
                }
                self.mapped_mode_button = None;
                self.finish_oneshot(mods, now_ms / 1000.0);
                ui.ctx().request_repaint();
            }
        }

        if !modal
            && !space_down
            && !view_gesture
            && !mapped_mode_active
            && !self.primary_action_active
            && self.tablet_allows_painting()
        {
            // 本帧是否已经按"拖拽"语义喂过事件（见下面的 click 分支）
            let mut dispatched_drag = false;
            if response.drag_started_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                // 起笔不重置 BrushState：源码每笔只调 `brush.new_stroke()`
                // 复位计时（`lib/stroke.py:47`），位置、RNG、smudge bucket
                // 都跨笔画保留；重置只发生在换设备时（`freehand.py:394`）。
                // 计时由 `BrushworkState::begin` 复位。
                // 下一笔开始 → 清掉上一笔的 X 标记并停止淡出
                // （`overlays.py:216-226`）。
                self.last_pos_marker_at = None;
                if let Some(pos) = response.interact_pointer_pos() {
                    // egui 的 `drag_started` 在指针越过阈值后才置位，此时
                    // `interact_pointer_pos()` 已经离开按下点几像素。画框的
                    // 角柄、对称轴这类"按在 5px 命中区上才开始拖"的模式会
                    // 因此漏掉按下事件。用 press_origin 还原真正的按下位置。
                    let start = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                    // 填充拖拽路径（`FloodFillOverlay`）：按下时清空并记第一点。
                    if self.modes.top().action_name() == Some("FloodFillMode") {
                        self.fill_drag_path.clear();
                        self.fill_drag_path.push(start);
                    }
                    let ev = self.pointer_event(start, rect, now_ms, 1, mods);
                    self.mode_button_press(ev);
                    self.paint_prev_pos = Some(start);
                    self.paint_prev_time = now_ms;
                }
                self.stroke_dirty = true;
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                if let Some(pos) = response.interact_pointer_pos() {
                    if self.modes.top().action_name() == Some("FloodFillMode") {
                        // 只记隔开至少 2px 的采样点，避免每帧堆积。
                        if self
                            .fill_drag_path
                            .last()
                            .is_none_or(|p| p.distance(pos) >= 2.0)
                        {
                            self.fill_drag_path.push(pos);
                        }
                    }
                    self.dispatch_paint_motion(pos, rect, now_ms, mods);
                }
                self.stroke_dirty = true;
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                dispatched_drag = true;
                if let Some(pos) = response.interact_pointer_pos() {
                    self.paint_prev_pos = None;
                    self.paint_prev_time = 0.0;
                    let ev = self.pointer_event(pos, rect, now_ms, 1, mods);
                    self.mode_button_release(ev);
                    // 填充松开即执行填充，叠加层同时移除（`fill.py:173-178`）。
                    if self.modes.top().action_name() == Some("FloodFillMode") {
                        self.fill_drag_path.clear();
                    }
                    // Oneshot（取色）拖完按修饰键规则回原工具（mode.py:1302-1317）
                    self.finish_oneshot(mods, now_ms / 1000.0);
                    // 出栈可能让覆盖层（笔刷尺寸圆圈）消失；请求一帧重绘，
                    // 否则窗口会停在"还带着圆圈"的上一帧，直到下次输入。
                    ui.ctx().request_repaint();
                }
                // 颜色历史入栈时机对齐 MyPaint：`__input_stroke_ended_cb`
                // （gui/brushcolor.py:54）——每笔结束后记录，且橡皮不记录。
                if !self.eraser {
                    self.color_manager.push_current_history();
                }
                // 笔刷历史同理（`brushmanager.py:1064`），由自由手绘模式发出。
                self.record_brush_history();
                // `input_stroke_ended` → 记录 last_painting_pos 并开始 1.5s 淡出。
                self.note_last_paint_pos(now_ms / 1000.0);
            }
            // 单击（按下即松开、指针几乎没动）：
            //
            // egui 的 `Sense::click_and_drag` 会先观望"这是点击还是拖拽"
            // （`Response::drag_started` 的文档：同时感知 click 时只有指针移动
            // 一小段之后才算 drag），所以一次干脆的点击**不会**走上面的
            // drag_started/drag_stopped。但 MyPaint 的 GTK 事件流里
            // ButtonPress/ButtonRelease 一定成对到达每一个模式
            // （`gui/mode.py` 的 `button_press_cb`/`button_release_cb`），
            // 填充工具的单击就是一粒种子 + 一次填充（`gui/fill.py:161-178`）。
            // 因此这里把"被判定为点击"的那一次补成一对 press/release，
            // 否则单击填充永远不会触发。
            //
            // 上面的拖拽分支与 `clicked_by` 互斥（egui 在判定为拖拽后不置
            // CLICKED），`dispatched_drag` 只是防御性双保险。
            if !dispatched_drag && response.clicked_by(egui::PointerButton::Primary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    // 对称/画框编辑模式的垃圾桶按钮（`symmetry.py:736-740`、
                    // `framewindow.py:182-187`）优先于绘画。
                    let mut consumed = false;
                    match self.modes.top().action_name() {
                        Some("SymmetryEditMode") => {
                            let d = crate::shell::overlays::symmetry_disable_pos(
                                rect,
                                &self.view,
                                self.doc.symmetry_center(),
                            );
                            if pos.distance(d) <= 16.0 {
                                self.doc
                                    .set_symmetry_state(Some(false), None, None, None, None);
                                consumed = true;
                            }
                        }
                        Some("FrameEditMode") => {
                            let d = crate::shell::overlays::frame_remove_pos(rect);
                            if pos.distance(d) <= 16.0 {
                                self.doc.set_frame_enabled(false);
                                consumed = true;
                            }
                        }
                        _ => {}
                    }
                    // Inking 画布 OK/垃圾桶优先于绘画（`inktool.py:317-356`）
                    if let Some(accept) = self.ink_button_at(rect, pos) {
                        consumed = self.with_mode(|modes, ctx| {
                            if accept {
                                modes.top_mut().ink_accept(ctx)
                            } else {
                                modes.top_mut().ink_reject(ctx)
                            }
                        });
                        if consumed {
                            self.stroke_dirty = true;
                        }
                    }
                    if !consumed {
                        // 同上：单击也是一笔，但不重置引擎状态（bucket 连续）。
                        self.last_pos_marker_at = None;
                        self.paint_prev_pos = None;
                        self.paint_prev_time = 0.0;
                        let press = self.pointer_event(pos, rect, now_ms, 1, mods);
                        self.mode_button_press(press);
                        let release = self.pointer_event(pos, rect, now_ms, 1, mods);
                        self.mode_button_release(release);
                        self.finish_oneshot(mods, now_ms / 1000.0);
                        self.stroke_dirty = true;
                        if !self.eraser {
                            self.color_manager.push_current_history();
                        }
                        self.record_brush_history();
                        self.note_last_paint_pos(now_ms / 1000.0);
                    }
                }
            }
            // 无按键悬停：Inking ADJUST 的节点预亮需要它
            // （`inktool.py:356-362` 的 `motion_notify_cb`）。
            // 正在作画（包括合成输入的 selftest）时不再补 hover 事件：
            // egui 只知道真实指针按键，合成笔画期间会把每帧的悬停都塞进
            // 事件队列，最终把抬笔事件淹掉。
            if response.hovered()
                && !ui.input(|i| i.pointer.any_down())
                && !self.modes.top().is_painting()
            {
                if let Some(pos) = response.hover_pos() {
                    let ev = self.pointer_event(pos, rect, now_ms, 0, mods);
                    self.mode_motion(ev);
                }
            }
        }

        // 主键映射手势在**松开这一帧的输入处理之后**才解锁：否则
        // `drag_stopped_by`/`clicked_by` 会把同一次按键再当绘画喂一遍。
        if !primary_down {
            self.primary_action_active = false;
        }
        if std::env::var_os("PIGMENT_SELFTEST_STROKE").is_some() {
            self.selftest_stroke(rect);
        }

        // 本帧的绘画预算用完了：请求下一帧继续把队列画完。
        let t_events = std::time::Instant::now();
        let more_pending = self.mode_process_pending_budgeted();
        let events_ns = t_events.elapsed();
        if more_pending {
            ui.ctx().request_repaint();
        }

        // ---- 画布：屏幕 tile 循环（远景/近景同一条路径）----
        // 目标网格是「级别空间的 64×64 tile」：`level` 由缩放决定
        // （`tileddrawwidget.py:1296`），一格覆盖 `64<<level` 世界像素。
        // 所以缩得再小，一帧枚举的格数也始终是视口量级（≈ 视口/32..64 每轴），
        // 不存在"远景 tile 数爆显存"的问题 —— 旧的整幅缩略图分支因此删掉了。
        // GPU dab 一帧同步一次：把本帧所有事件排队的 dab 合并成一个
        // submit + 一次回读。渲染/合成从这里开始读像素。
        let t_flush = std::time::Instant::now();
        if let Some(surface) = self.doc.paintable_surface_mut() {
            // 渲染帧限量推进 GPU 队列：剩余 dab 下一帧继续，避免单帧
            // 积压成几百个 dab 的 submit 尖峰。快照/采样仍走完整 flush。
            surface.flush_gpu_dabs_budgeted();
        }
        let flush_ns = t_flush.elapsed();
        let ppp = ui.ctx().pixels_per_point();
        let viewport = [
            rect.min.x * ppp,
            rect.min.y * ppp,
            rect.width() * ppp,
            rect.height() * ppp,
        ];
        let t_prepare = std::time::Instant::now();
        // Instant Preview 期间把显示级别锁在预览级别：缩放时先显示低分辨率
        // 笔迹，全分辨率同步完再回到目标级别。
        let preview_level = self.modes.top().instant_preview_state().map(|(l, _)| l);
        let level_now = preview_level.unwrap_or_else(|| mip_level_for_zoom(self.view.zoom));
        // 级别刚变、上一帧还有没补完的格子，或显存被整批作废：用宽松预算
        // 尽快追平，避免整屏在低预算下逐帧“长出来”。
        let level_change = level_now != self.mip_level
            || self.pending_uploads > 0
            || self.force_resync
            || self.no_double_buffering;
        let mut tiles = prepare_canvas(
            &self.renderer,
            self.doc.layers(),
            &self.view,
            viewport,
            // `NoDoubleBuffereing`（`document.py:2091-2093`）：GTK 关双缓冲
            // 在本移植里等价于“每帧丢弃显存缓存、直接重建可见 tile”。
            self.force_resync || self.no_double_buffering,
            level_change,
            preview_level,
            self.display_filter,
        );
        let prepare_ns = t_prepare.elapsed();
        if let Some(t) = t_frame {
            let pending_left = self.modes.top().pending_inputs();
            report_frame_perf(
                t.elapsed().as_nanos(),
                events_ns.as_nanos(),
                flush_ns.as_nanos(),
                prepare_ns.as_nanos(),
                tiles.draw.len(),
                tiles.uploads.len(),
                pending_left,
                more_pending,
            );
        }
        self.force_resync = false;
        if self.no_double_buffering {
            ui.ctx().request_repaint();
        }
        self.pending_uploads = tiles.pending_uploads;
        self.last_prep_ms = prepare_ns.as_secs_f32() * 1e3;
        self.last_uploads = tiles.uploads.len();
        self.last_uncovered = tiles.uncovered;
        self.window_prep_max_ms = self.window_prep_max_ms.max(self.last_prep_ms);
        self.window_uploads_max = self.window_uploads_max.max(self.last_uploads);
        self.window_uncovered_max = self.window_uncovered_max.max(self.last_uncovered);
        if self.pending_uploads > 0 {
            // 分帧同步还没补齐：保持重绘，直到所有可见格都上传完成。
            ui.ctx().request_repaint();
        }
        // E2E 诊断：当前级别与可见格数（语义见 `PIGMENT_E2E_STATUS`）。
        self.mip_level = level_now;
        self.mip_tiles = tiles.draw.len();
        // 可见的 Instant Preview 状态：否则用户无法判断当前笔刷/缩放是否
        // 真的走了低分辨率预览路径。
        if let Some((level, pending)) = self.modes.top().instant_preview_state() {
            let text = if pending > 0 {
                format!("即时预览 L{level} · 后台同步 {pending}")
            } else {
                format!("即时预览 L{level}")
            };
            ui.painter().text(
                egui::pos2(rect.left() + 10.0, rect.top() + 10.0),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(12.0),
                egui::Color32::from_black_alpha(170),
            );
        }
        // 调试覆盖层要在 uploads 被移进 GPU callback 之前抄下坐标。
        let debug_tiles: Vec<TileKey> = tiles.uploads.iter().map(|u| u.key).collect();

        tiles.checks = !self.doc.layers().render_is_opaque()
            && self.prefs.bool_or("view.real_alpha_checks", true);
        tiles.clear = self.canvas_clear_color();
        if self.direct_canvas {
            // Plan A：画布由 shell 在 egui pass 之前直接渲染（独立 wgpu pass）。
            self.canvas_frame = Some(tiles);
        } else {
            // eframe 回退：渲染器挂进 egui 的 PaintCallback。
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                CanvasPaint { frame: tiles },
            ));
        }

        // ---- 覆盖层（B3-5）：画框 + 对称辅助线 ----
        // 源码在 `TiledDrawWidget` 里先画画布、再逐个 `overlay.paint(cr)`
        // （`tileddrawwidget.py:1150-1156`，挂载顺序见 `drawwindow.py:157-158`）。
        // 这里的 wgpu callback 就是"画布"，覆盖层排在它之后加进同一帧的
        // 绘制列表，于是自然压在 tile 之上。
        //
        // 裁剪到画布矩形：画框/轴线在视口外还有几何，不裁会画到侧栏上
        // （源码的 cairo 上下文本来就被裁剪到 TDW 的分配区域）。
        let mut overlay_style = crate::shell::overlays::OverlayStyle::from_prefs(&self.prefs);
        overlay_style.active_symmetry_axis = self.modes.top().active_symmetry_axis();
        crate::shell::overlays::paint_canvas_overlays(
            &ui.painter().with_clip_rect(rect),
            rect,
            &self.view,
            &self.doc,
            overlay_style,
        );

        // ---- 画框编辑手柄（`framewindow.py:837-899`）----
        if self.modes.top().action_name() == Some("FrameEditMode") {
            let (fx, fy, fw, fh) = self.doc.get_frame();
            if fw > 0 && fh > 0 {
                let fr =
                    crate::shell::overlays::frame_display_rect(rect, &self.view, (fx, fy, fw, fh));
                let painter = ui.painter().with_clip_rect(rect);
                let normal = egui::Color32::from_rgb(0xEC, 0xF0, 0xF1);
                let active = egui::Color32::from_rgb(0xFC, 0xE9, 0x4F);
                let hp = response.hover_pos();
                let near = |a: egui::Pos2, b: egui::Pos2, d: f32| {
                    hp.is_some_and(|p| p.distance(a) <= d || p.distance(b) <= d)
                };
                let edges = [
                    (
                        egui::pos2(fr.left(), fr.top()),
                        egui::pos2(fr.right(), fr.top()),
                    ),
                    (
                        egui::pos2(fr.right(), fr.top()),
                        egui::pos2(fr.right(), fr.bottom()),
                    ),
                    (
                        egui::pos2(fr.right(), fr.bottom()),
                        egui::pos2(fr.left(), fr.bottom()),
                    ),
                    (
                        egui::pos2(fr.left(), fr.bottom()),
                        egui::pos2(fr.left(), fr.top()),
                    ),
                ];
                for (a, b) in edges {
                    painter.line_segment(
                        [a, b],
                        egui::Stroke::new(6.0, egui::Color32::from_black_alpha(120)),
                    );
                    painter.line_segment(
                        [a, b],
                        egui::Stroke::new(2.0, if near(a, b, 6.0) { active } else { normal }),
                    );
                }
                let corners = [
                    fr.left_top(),
                    fr.right_top(),
                    fr.right_bottom(),
                    fr.left_bottom(),
                ];
                for c in corners {
                    painter.circle_filled(c, 8.0, egui::Color32::from_black_alpha(120));
                    painter.circle_filled(
                        c,
                        6.0,
                        if hp.is_some_and(|p| p.distance(c) <= 10.0) {
                            active
                        } else {
                            normal
                        },
                    );
                }
            }
        }

        // `VisualizeRendering`（`document.py:2087-2089`）：把本帧重绘的 tile
        // 区域框出来。源码用 renderer 的调试标记；这里直接标记上传集合。
        if self.visualize_rendering {
            let now = ui.input(|i| i.time);
            if !debug_tiles.is_empty() {
                self.debug_uploads = debug_tiles.clone();
                self.debug_uploads_until = now + 0.8;
            }
            let tiles = if now < self.debug_uploads_until {
                &self.debug_uploads
            } else {
                &debug_tiles
            };
            paint_redraw_debug(&ui.painter().with_clip_rect(rect), rect, &self.view, tiles);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(900));
        } else {
            self.debug_uploads.clear();
        }
        // `ToggleScaleFeedback`（`overlays.py:141-193`）：右上角显示缩放。
        if self.scale_feedback {
            paint_scale_feedback(&ui.painter().with_clip_rect(rect), rect, self.view.zoom);
        }
        // `ToggleLastPosFeedback`（`overlays.py:196-278`）：上一笔终点画 X，
        // 结束后 1.5 秒淡出；下一笔开始立即清除。
        if self.last_pos_feedback {
            if let (Some((x, y)), Some(started_at)) =
                (self.doc.last_painting_pos, self.last_pos_marker_at)
            {
                let now = ui.input(|i| i.time);
                let alpha = last_pos_alpha(started_at, now);
                if alpha > 0.0 {
                    paint_last_pos_feedback(
                        &ui.painter().with_clip_rect(rect),
                        rect,
                        &self.view,
                        x,
                        y,
                        alpha,
                    );
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                } else {
                    self.last_pos_marker_at = None;
                }
            }
        }

        // ---- 笔刷尺寸圆圈（`gui/brushmanip.py` 的 `BrushSizeOverlay`）----
        // 只在 `BrushResizeMode` 拖拽期间存在；源码把它 append 到
        // `display_overlays`（`:70-71`），所以画在画框/对称轴之上。
        if let Some(state) = self.modes.top().brush_size_overlay() {
            crate::shell::overlays::paint_brush_size(
                &ui.painter().with_clip_rect(rect),
                rect,
                &self.view,
                state,
                overlay_style,
            );
        }

        // ---- 取色预览方块（`ColorPickPreviewOverlay`，`colorpicker.py:253-364`）----
        //
        // 只在取色模式的按键/快捷键按住期间显示（源码刻意避免闪烁）。
        let pick_mode = self
            .modes
            .top()
            .action_name()
            .map(|n| n.starts_with("ColorPickMode") || n.starts_with("FlipColorPickMode"))
            .unwrap_or(false);
        if pick_mode && ui.input(|i| i.pointer.primary_down()) {
            // 拖拽时 `response.hover_pos()` 可能为 None（指针被模式 grab），
            // 用输入层的最近位置；仍限定在画布内。
            let pos = ui.input(|i| i.pointer.interact_pos());
            if let Some(pos) = pos.filter(|p| rect.contains(*p)) {
                let rgb = self.color_manager.get_color().get_rgb();
                let rgb32 = egui::Color32::from_rgb(
                    (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                );
                crate::shell::overlays::paint_pick_preview(
                    &ui.painter().with_clip_rect(rect),
                    rect,
                    pos,
                    rgb32,
                );
            }
        }

        // ---- 填充种子路径（`FloodFillOverlay`，`fill.py:361-430`）----
        if !ui.input(|i| i.pointer.primary_down()) {
            self.fill_drag_path.clear();
        }
        if !self.fill_drag_path.is_empty() {
            paint_fill_drag(
                &ui.painter().with_clip_rect(rect),
                &self.fill_drag_path,
                ui.ctx().pixels_per_point(),
            );
        }

        // ---- 笔刷光标（`gui/cursor.py`）----
        //
        // 源码给每个绘画模式设置一张自绘 pixbuf 光标；egui 侧改为在画布上
        // 直接画内外圈 + 动态十字线，并隐藏系统光标（`CursorIcon::None`）。
        // `ui.hide_cursor_while_painting` 只控制"作画时是否还显示这个自绘光标"。
        let paints = self
            .modes
            .top()
            .pointer_behavior()
            .intersects(pigment_engine::mode::Behavior::PAINT_BRUSH);
        // 注意顺序：`cursor_action_name()` 对 FreehandMode 也返回动作名，
        // 不能把它当成“非笔刷模式”的先决条件，否则会走“只隐藏系统光标、
        // 不画圆环”的分支，表现就是完全没有光标。笔刷模式必须先走自绘圆环。
        if paints && response.hovered() && !space_down && !view_gesture {
            let painting = ui.input(|i| i.pointer.any_down());
            let hide_now = painting && self.mode_prefs.hide_cursor_while_painting;
            if !hide_now {
                if let Some(pos) = response.hover_pos() {
                    // `tileddrawwidget.py:586-588`：crosshair 预设用系统十字光标，
                    // 不画圆环；其余三个预设都是自绘圆环（宽度/颜色由偏好定）。
                    if crate::shell::overlays::cursor_is_crosshair(&self.prefs) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    } else {
                        let base_r = self.brush.get(BrushSetting::Radius).base_value;
                        let by_random = self.brush.get(BrushSetting::OffsetByRandom).base_value;
                        let visual = base_r.exp() * (1.0 + 2.0 * by_random);
                        // `tileddrawwidget.py:606`：`r = visual_radius * scale + 0.5`
                        let cursor_style = crate::shell::overlays::CursorStyle::from_brush(
                            &self.brush,
                            self.eraser,
                        );
                        crate::shell::overlays::paint_brush_cursor(
                            &ui.painter().with_clip_rect(rect),
                            rect,
                            pos,
                            visual * self.view.zoom + 0.5,
                            &overlay_style,
                            cursor_style,
                            &self.prefs,
                        );
                        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    }
                }
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            }
        } else if response.hovered() && !space_down && !view_gesture {
            // 非笔刷模式：源码用 action 图标光标（填充/直线/取色/画框等）。
            // 图标光标资源尚未接回新 chrome 时，用系统十字兜底，绝不隐身为空。
            let action = self.modes.top().action_name().unwrap_or("");
            if matches!(
                action,
                "FloodFillMode"
                    | "FloodFillGCMode"
                    | "StraightMode"
                    | "SequenceMode"
                    | "EllipseMode"
                    | "InkingMode"
                    | "FrameEditMode"
                    | "SymmetryEditMode"
                    | "ColorPickMode"
                    | "ColorPickModeH"
                    | "ColorPickModeC"
                    | "ColorPickModeY"
                    | "FlipColorPickMode"
                    | "FlipColorPickModeH"
                    | "FlipColorPickModeC"
                    | "FlipColorPickModeY"
            ) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            } else if action == "LayerMoveMode" {
                // 源码用专门的 layer-move 光标；系统移动光标是最接近的等价物。
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            }
        }

        // ---- Inking ADJUST 节点（`inktool.py:1074-1116`）----
        if self.modes.top().ink_editing() {
            if let Some(nodes) = self.modes.top().ink_nodes() {
                let painter = ui.painter().with_clip_rect(rect);
                let target = self.modes.top().ink_target_node();
                for (i, n) in nodes.iter().enumerate() {
                    let s = self.view.world_to_screen(n.x, n.y);
                    let p = egui::pos2(rect.min.x + s[0], rect.min.y + s[1]);
                    let color = if target == Some(i) {
                        egui::Color32::from_rgb(0xF1, 0xC4, 0x0F)
                    } else {
                        egui::Color32::from_rgb(0xEC, 0xF0, 0xF1)
                    };
                    painter.circle_filled(p, 4.0, color);
                }
            }
            // 画布 OK / 垃圾桶浮动按钮（`inktool.py:1090-1116`）
            if let Some((accept, reject)) = self.ink_button_positions(rect) {
                let painter = ui.painter().with_clip_rect(rect);
                for (local, glyph) in [(accept, "✓"), (reject, "×")] {
                    let p = egui::pos2(rect.min.x + local.x, rect.min.y + local.y);
                    painter.circle_filled(p, 16.0, egui::Color32::from_rgb(0xEC, 0xF0, 0xF1));
                    painter.text(
                        p,
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        egui::FontId::proportional(18.0),
                        egui::Color32::BLACK,
                    );
                }
            }
        }

        // ---- 对称/画框编辑模式的浮动按钮（`symmetry.py:955-975`、
        // `framewindow.py:888-899`）----
        match self.modes.top().action_name() {
            Some("SymmetryEditMode") => {
                let painter = ui.painter().with_clip_rect(rect);
                let center = self.doc.symmetry_center();
                let d = crate::shell::overlays::symmetry_disable_pos(rect, &self.view, center);
                painter.circle_filled(d, 16.0, egui::Color32::from_rgb(0xEC, 0xF0, 0xF1));
                painter.text(
                    d,
                    egui::Align2::CENTER_CENTER,
                    "×",
                    egui::FontId::proportional(18.0),
                    egui::Color32::BLACK,
                );
                let [sx, sy] = self.view.world_to_screen(center.0 as f32, center.1 as f32);
                let c = rect.min + egui::vec2(sx, sy);
                if rect.contains(c) {
                    painter.circle_filled(c, 8.0, egui::Color32::from_rgb(0xF1, 0xC4, 0x0F));
                }
            }
            Some("FrameEditMode") => {
                let painter = ui.painter().with_clip_rect(rect);
                let d = crate::shell::overlays::frame_remove_pos(rect);
                painter.circle_filled(d, 16.0, egui::Color32::from_rgb(0xEC, 0xF0, 0xF1));
                painter.text(
                    d,
                    egui::Align2::CENTER_CENTER,
                    "🗑",
                    egui::FontId::proportional(14.0),
                    egui::Color32::BLACK,
                );
            }
            _ => {}
        }

        // 对称中心拖拽（`symmetry.py:972-975` 的中心色块；引擎的
        // SymmetryEditMode 只处理轴/垃圾桶，中心点由 UI 直接移动）。
        if self.modes.top().action_name() == Some("SymmetryEditMode") {
            let center = self.doc.symmetry_center();
            let [sx, sy] = self.view.world_to_screen(center.0 as f32, center.1 as f32);
            let c = rect.min + egui::vec2(sx, sy);
            // 用**按下点**判命中，而不是当前 hover：egui 的 `drag_started()`
            // 在指针越过拖拽阈值的那一帧才为真，此时 `hover_pos()` 已经离开
            // 中心点了。快速拖拽（一帧走几十像素）会因 `near == false`
            // 永远起不了拖拽 —— 中心点看起来"拖不动"。
            let press = ui.input(|i| i.pointer.press_origin());
            let near = press
                .or_else(|| response.hover_pos())
                .is_some_and(|p| p.distance(c) <= 12.0);
            if near && response.drag_started() {
                self.symmetry_center_drag = true;
            }
            if self.symmetry_center_drag && response.dragged() {
                if let Some(p) = response.interact_pointer_pos() {
                    let [wx, wy] = self
                        .view
                        .screen_to_world(p.x - rect.left(), p.y - rect.top());
                    self.doc.set_symmetry_state(
                        None,
                        Some((wx.round() as i32, wy.round() as i32)),
                        None,
                        None,
                        None,
                    );
                }
            }
            if response.drag_stopped() {
                self.symmetry_center_drag = false;
            }
        }
        if self.stroke_dirty {
            self.stroke_dirty = false;
            ui.ctx().request_repaint();
        }
    }
}

/// 离散步进滚轮增量 —— `ui.support_smooth_scrolling` 关闭时的取值。
///
/// egui 0.37 的 `InputState` 不再暴露 `raw_scroll_delta`，但 `raw.events` 里
/// 仍有后端发来的 `MouseWheel`（单位 Point/Line/Page）。这里按 egui 自己的
/// 换算规则折成"点"：1 行 = 50 点（`InputOptions::line_scroll_speed` 的出厂值），
/// 1 页 = 视口高度。关闭平滑滚动时用它，滚轮就回到整格步进。
fn stepwise_wheel(events: &[egui::Event], page_height: f32) -> egui::Vec2 {
    let mut out = egui::Vec2::ZERO;
    for event in events {
        if let egui::Event::MouseWheel { unit, delta, .. } = event {
            let scale = match unit {
                egui::MouseWheelUnit::Point => 1.0,
                egui::MouseWheelUnit::Line => 50.0,
                egui::MouseWheelUnit::Page => page_height,
            };
            out += *delta * scale;
        }
    }
    out
}

/// 对称类型在选项面板里的显示名（`symmetry.py:368` 的 `Symmetry Type` 条目）。
fn symmetry_label(t: pigment_surface::symmetry::SymmetryType) -> &'static str {
    use pigment_surface::symmetry::SymmetryType as T;
    match t {
        T::Vertical => "Vertical",
        T::Horizontal => "Horizontal",
        T::VertHorz => "Vertical + Horizontal",
        T::Rotational => "Rotational",
        T::Snowflake => "Snowflake",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(unit: egui::MouseWheelUnit, dx: f32, dy: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(dx, dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn last_pos_fades_out_over_one_and_a_half_seconds() {
        // FadingOverlay：20fps，每 tick 减 1/30，1.5s 后为 0
        // （overlays.py:48-49,80-89）。
        assert_eq!(last_pos_alpha(10.0, 10.0), 1.0);
        assert!((last_pos_alpha(10.0, 10.05) - (1.0 - 1.0 / 30.0)).abs() < 1e-6);
        assert!(last_pos_alpha(10.0, 10.75) > 0.0);
        assert_eq!(last_pos_alpha(10.0, 11.5), 0.0);
        assert_eq!(last_pos_alpha(10.0, 99.0), 0.0);
    }

    #[test]
    fn stepwise_wheel_converts_units_to_points() {
        // Point 原样；Line 按 50 点/行；Page 按视口高度
        let events = [
            wheel(egui::MouseWheelUnit::Point, 10.0, -20.0),
            wheel(egui::MouseWheelUnit::Line, 1.0, 2.0),
            wheel(egui::MouseWheelUnit::Page, 0.0, 1.0),
        ];
        let d = stepwise_wheel(&events, 600.0);
        assert_eq!(d, egui::vec2(10.0 + 50.0, -20.0 + 100.0 + 600.0));
        // 没有滚轮事件 → 零（关闭平滑滚动不会凭空产生位移）
        assert_eq!(stepwise_wheel(&[], 600.0), egui::Vec2::ZERO);
        // 非滚轮事件被忽略
        assert_eq!(
            stepwise_wheel(
                &[egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                600.0
            ),
            egui::Vec2::ZERO
        );
    }
}

/// 选级公式照 `tileddrawwidget.py:1296`：`floor(log2(1/scale))`，钳到
/// `MAX_MIPMAP_LEVEL`。**与内容大小无关**（旧的实现还会按 bbox 边长提级，
/// 那是整幅缩略图架构的产物，源码没有这段）。
#[test]
fn mip_level_follows_scale_only() {
    assert_eq!(mip_level_for_zoom(1.0), 0);
    assert_eq!(mip_level_for_zoom(2.0), 0, "放大不降级");
    assert_eq!(mip_level_for_zoom(0.75), 0, "floor(log2(1/0.75)) = 0");
    assert_eq!(mip_level_for_zoom(0.5), 1);
    assert_eq!(mip_level_for_zoom(0.25), 2);
    assert_eq!(mip_level_for_zoom(0.125), 3);
    assert_eq!(mip_level_for_zoom(0.0625), 4, "1/16 → level 4");
    assert_eq!(
        mip_level_for_zoom(0.01),
        pigment_surface::mipmap::MAX_MIPMAP_LEVEL,
        "更远钳到上限（剩下的缩小交给纹理插值）"
    );
    assert_eq!(
        mip_level_for_zoom(0.0),
        pigment_surface::mipmap::MAX_MIPMAP_LEVEL
    );
}

/// 屏幕 tile 循环的枚举量：`level` 跟着缩放走，所以每格在屏幕上始终 ≳ 32
/// 物理像素 —— 一帧枚举的格数与缩放**无关**（1080p 约 30×17，最差约 60×34）。
///
/// 只有缩到 1/16 以下时（level 被钳到 4）每格才越来越小、格数才开始按 1/zoom
/// 涨；源码也在这里付代价（它的 pixbuf 边长同样按 1/zoom 涨，`:1301-1330`），
/// 我们的代价是 draw call 数，最后靠 [`MAX_VISIBLE_TILES_PER_AXIS`] 兜底。
#[test]
fn screen_tile_count_stays_bounded_above_the_level_cap() {
    for zoom in [1.0f32, 0.5, 0.25, 0.125, 0.0625] {
        let level = mip_level_for_zoom(zoom);
        let tile_world = (TILE_SIZE as f32) * (1u32 << level) as f32;
        // 1920 物理像素宽 / (一格的世界边长 × 缩放) = 每轴格数
        let per_axis = (1920.0 / zoom / tile_world).ceil();
        assert!(
            per_axis <= 64.0,
            "zoom={zoom} level={level} 每轴 {per_axis} 格，超出预期"
        );
    }
    // 极限缩小时（level 已封顶）格数会涨，但被安全网上限挡住
    for zoom in [0.02f32, 0.005] {
        let level = mip_level_for_zoom(zoom);
        let tile_world = (TILE_SIZE as f32) * (1u32 << level) as f32;
        let raw = (1920.0 / zoom / tile_world).ceil() as i32;
        assert!(
            raw.min(MAX_VISIBLE_TILES_PER_AXIS) <= MAX_VISIBLE_TILES_PER_AXIS,
            "枚举量必须被钳住"
        );
    }
}
