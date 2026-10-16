//! pigment 应用主体：极简工具条 + 全窗口画布（MyPaint「画布优先」哲学）。
//!
//! 交互（对齐 MyPaint 键盘流）：
//! - 左键拖拽：绘制；中/右键或 Space+左键：平移；滚轮：以光标为中心缩放
//! - `[` / `]`：笔刷半径；`1..=5`：颜色预设；`E`：橡皮；`P`：光谱混色模式
//!
//! 数据流：指针事件 → hokusai 笔画引擎 → PigmentSurface（CPU fix15）
//!   → changed_since 收集脏 tile → convert 为 sRGB8 → GPU 增量上传 → 渲染。

use std::sync::{Arc, Mutex};

use eframe::egui;

use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::{Brush, BrushInput, BrushSetting, BrushState, InputMapping};
use pigment_surface::PigmentSurface;

use crate::convert;
use crate::gpu::{CanvasPaint, TileRenderer, TileUpload};
use crate::view::View;

/// 颜色预设（真实颜料取向：光谱模式下表现正确）
const COLORS: &[(&str, f32, f32, f32)] = &[
    ("焦茶", 0.07, 0.75, 0.25),
    ("镉黄", 0.144, 0.90, 1.00),
    ("群青", 0.647, 0.73, 0.75),
    ("钛白", 0.0, 0.0, 1.0),
    ("镉红", 0.0, 0.85, 0.70),
];

pub struct PigmentApp {
    surface: PigmentSurface,
    brush: Brush,
    stroke: Option<BrushState>,
    view: View, // 逻辑点坐标域（points）
    renderer: Arc<Mutex<TileRenderer>>,
    last_sync: u64,
    // 笔刷参数
    color_idx: usize,
    radius_log: f32,
    paint_mode: bool,
    eraser: bool,
    stroke_dirty: bool,
}

impl PigmentApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("pigment 需要 wgpu renderer（NativeOptions.renderer = Wgpu）");
        let renderer = TileRenderer::new(&rs.device, rs.target_format);

        let mut app = Self {
            surface: PigmentSurface::new(),
            brush: Brush::new(),
            stroke: None,
            view: View::default(),
            renderer: Arc::new(Mutex::new(renderer)),
            last_sync: 0,
            color_idx: 0,
            radius_log: 2.2, // ≈ 9px
            paint_mode: true,
            eraser: false,
            stroke_dirty: false,
        };
        app.rebuild_brush();
        app
    }

    fn rebuild_brush(&mut self) {
        let mut b = Brush::new();
        let mut set = |s: BrushSetting, v: f32| b.get_mut(s).base_value = v;
        let (_, h, s, v) = COLORS[self.color_idx];
        set(BrushSetting::Radius, self.radius_log);
        set(BrushSetting::Hardness, 0.7);
        set(BrushSetting::DabsPerActualRadius, 1.5);
        set(BrushSetting::DabsPerBasicRadius, 1.5);
        set(BrushSetting::DabsPerSecond, 0.0);
        set(BrushSetting::ColorH, h);
        set(BrushSetting::ColorS, s);
        set(BrushSetting::ColorV, v);
        set(BrushSetting::Paint, if self.paint_mode { 1.0 } else { 0.0 });
        set(BrushSetting::Eraser, if self.eraser { 1.0 } else { 0.0 });
        // 压感 → 不透明度（数位板在 M6 接入，鼠标恒为 1.0）
        set(BrushSetting::Opaque, 0.35);
        b.get_mut(BrushSetting::Opaque).inputs = vec![InputMapping {
            input: BrushInput::Pressure,
            points: vec![(0.0, 0.0), (1.0, 0.65)],
        }];
        self.brush = b;
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (smaller, larger, eraser, paint, color_key) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::OpenBracket),
                i.key_pressed(egui::Key::CloseBracket),
                i.key_pressed(egui::Key::E),
                i.key_pressed(egui::Key::P),
                [
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                ]
                .iter()
                .position(|&k| i.key_pressed(k)),
            )
        });
        let mut dirty = false;
        if smaller {
            self.radius_log = (self.radius_log - 0.15).max(-1.0);
            dirty = true;
        }
        if larger {
            self.radius_log = (self.radius_log + 0.15).min(5.0);
            dirty = true;
        }
        if eraser {
            self.eraser = !self.eraser;
            dirty = true;
        }
        if paint {
            self.paint_mode = !self.paint_mode;
            dirty = true;
        }
        if let Some(idx) = color_key {
            self.color_idx = idx.min(COLORS.len() - 1);
            self.eraser = false;
            dirty = true;
        }
        if dirty {
            self.rebuild_brush();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, (name, _, _, _)) in COLORS.iter().enumerate() {
                    let selected = self.color_idx == i && !self.eraser;
                    if ui.selectable_label(selected, format!("{} {}", i + 1, name)).clicked() {
                        self.color_idx = i;
                        self.eraser = false;
                        self.rebuild_brush();
                    }
                }
                ui.separator();
                if ui
                    .selectable_label(self.eraser, "E 橡皮")
                    .clicked()
                {
                    self.eraser = !self.eraser;
                    self.rebuild_brush();
                }
                let mut paint = self.paint_mode;
                if ui.checkbox(&mut paint, "P 光谱混色").changed() {
                    self.paint_mode = paint;
                    self.rebuild_brush();
                }
                ui.separator();
                ui.label(format!("半径 {:.0}px", self.radius_log.exp()));
                ui.separator();
                ui.label(format!("缩放 {:.0}%", self.view.zoom * 100.0));
                ui.label(format!(
                    "tiles {} ({:.1} MiB)",
                    self.surface.tile_count(),
                    self.surface.memory_bytes() as f64 / 1048576.0
                ));
                let resident = self.renderer.lock().unwrap().resident();
                ui.label(format!("显存驻留 {resident}"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("左键画 · 右键/中键/Space 平移 · 滚轮缩放 · [ ] 半径");
                });
            });
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ppp = ui.ctx().pixels_per_point();
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        // ---- 平移 ----
        let pan_delta = if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
        {
            response.drag_delta()
        } else {
            let space = ui.input(|i| i.key_down(egui::Key::Space));
            if space && response.dragged_by(egui::PointerButton::Primary) {
                response.drag_delta()
            } else {
                egui::Vec2::ZERO
            }
        };
        if pan_delta != egui::Vec2::ZERO {
            self.view.pan(pan_delta.x, pan_delta.y);
        }

        // ---- 缩放（以光标为锚点）----
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && response.hovered() {
            if let Some(pos) = response.hover_pos() {
                let local = pos - rect.min;
                let factor = (scroll * 0.002).exp();
                self.view.zoom_at(local.x, local.y, factor);
            }
        }

        // ---- 绘制 ----
        let space_down = ui.input(|i| i.key_down(egui::Key::Space));
        if !space_down && response.drag_started_by(egui::PointerButton::Primary) {
            self.stroke = Some(BrushState::default());
            if let Some(pos) = response.interact_pointer_pos() {
                let local = pos - rect.min;
                let [wx, wy] = self.view.screen_to_world(local.x, local.y);
                let mut st = self.stroke.take().unwrap();
                self.brush
                    .stroke_to(&mut st, &mut self.surface, wx, wy, 1.0, 0.0, 0.0, 0.01);
                self.stroke = Some(st);
                self.stroke_dirty = true;
            }
        }
        if !space_down && response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(pos), Some(mut st)) =
                (response.interact_pointer_pos(), self.stroke.take())
            {
                let local = pos - rect.min;
                let [wx, wy] = self.view.screen_to_world(local.x, local.y);
                let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05) as f64;
                self.brush
                    .stroke_to(&mut st, &mut self.surface, wx, wy, 1.0, 0.0, 0.0, dt);
                self.stroke = Some(st);
                self.stroke_dirty = true;
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.stroke = None;
        }

        // ---- 笔刷光标预览 ----
        if let Some(pos) = response.hover_pos() {
            let r = self.radius_log.exp() * self.view.zoom;
            ui.painter().circle_stroke(
                pos,
                r.max(1.0),
                egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
            );
        }

        // ---- 收集脏 tile（增量上传包）----
        let clock = self.surface.clock();
        let mut uploads: Vec<TileUpload> = if clock > self.last_sync {
            let mut changed = self.surface.changed_since(self.last_sync);
            changed.sort();
            changed
                .into_iter()
                .filter_map(|c| {
                    let t = self.surface.tile_entry(c.0, c.1)?;
                    Some(TileUpload {
                        coord: c,
                        revision: t.revision(),
                        pixels: convert::tile_to_rgba8_on_white(t.pixels()),
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        self.last_sync = clock;

        // ---- 可见 tile 列表（物理像素域）----
        let vp_w = rect.width() * ppp;
        let vp_h = rect.height() * ppp;
        let view_phys = View {
            zoom: self.view.zoom * ppp,
            offset: [self.view.offset[0] * ppp, self.view.offset[1] * ppp],
        };
        let (mut tx0, mut ty0, mut tx1, mut ty1) =
            view_phys.visible_tile_range(vp_w, vp_h, TILE_SIZE as i32);
        // 保护：极端缩小时可见范围可达数万 tile，夹到视口中心附近的上限内
        // （空区域由面板白底兜底，不影响观感；M5 再上 atlas/mip 方案）
        const MAX_SPAN: i32 = 96;
        if tx1 - tx0 + 1 > MAX_SPAN {
            let c = (tx0 + tx1) / 2;
            tx0 = c - MAX_SPAN / 2;
            tx1 = tx0 + MAX_SPAN - 1;
        }
        if ty1 - ty0 + 1 > MAX_SPAN {
            let c = (ty0 + ty1) / 2;
            ty0 = c - MAX_SPAN / 2;
            ty1 = ty0 + MAX_SPAN - 1;
        }
        let mut visible = Vec::with_capacity(
            ((tx1 - tx0 + 1).max(0) * (ty1 - ty0 + 1).max(0)) as usize,
        );
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                visible.push((tx, ty));
            }
        }

        // ---- 显存补传：可见但被 LRU 淘汰（或从未上传）的 tile ----
        // 面板白底可兜底空白，但「有内容却被淘汰」必须重传，否则画布漏底。
        {
            let renderer = self.renderer.lock().unwrap();
            let mut refill = 0usize;
            for &coord in &visible {
                if refill >= 64 {
                    break; // 每帧限量，避免淘汰风暴时卡帧
                }
                if renderer.has(coord) {
                    continue;
                }
                if let Some(t) = self.surface.tile_entry(coord.0, coord.1) {
                    uploads.push(TileUpload {
                        coord,
                        revision: t.revision(),
                        pixels: convert::tile_to_rgba8_on_white(t.pixels()),
                    });
                    refill += 1;
                }
            }
        }

        // ---- 视图矩阵：world px → 全窗口裁剪空间 ----
        let win = ui.ctx().content_rect();
        let win_w = win.width() * ppp;
        let win_h = win.height() * ppp;
        let view_window = View {
            zoom: view_phys.zoom,
            offset: [
                (self.view.offset[0] + rect.min.x) * ppp,
                (self.view.offset[1] + rect.min.y) * ppp,
            ],
        };
        let view_matrix = view_window.clip_matrix(win_w, win_h);

        ui.painter().add(eframe::egui_wgpu::Callback::new_paint_callback(
            rect,
            CanvasPaint {
                renderer: Arc::clone(&self.renderer),
                view_matrix,
                uploads,
                visible,
            },
        ));

        // 绘制中需要持续重绘（dab 沿笔画插值）
        if self.stroke.is_some() || self.stroke_dirty {
            self.stroke_dirty = false;
            ui.ctx().request_repaint();
        }
    }
}

impl eframe::App for PigmentApp {
    /// egui 0.37 的 App 入口是 Ui 形态：`ui` 覆盖整个 `content_rect`。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);
        self.toolbar(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::WHITE))
            .show(ui, |ui| self.canvas(ui));
    }
}
