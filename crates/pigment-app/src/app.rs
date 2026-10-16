//! pigment 应用主体：MyPaint「画布优先」布局 —— 全窗口画布 + 左右可折叠面板。
//!
//! 交互（对齐 MyPaint 键盘流）：
//! - 左键拖拽绘制；中/右键或 Space+左键平移；滚轮以光标为中心缩放
//! - `[` / `]` 笔刷半径；`1..=5` 颜料色预设；`E` 橡皮；`P` 光谱混色开关
//! - `,` / `.` 上一支 / 下一支笔刷；`Tab` 收起/展开面板；`F11` 全屏
//! - `F1` 强制全量重传显存 tile；`F2` 导出 CPU 侧画布 PNG（诊断）
//!
//! 数据流：指针事件 → hokusai 笔画引擎 → PigmentSurface（CPU fix15）
//!   → revision 比对收集脏 tile → convert 为 sRGB8 → GPU 增量上传 → 渲染。
//!
//! 笔刷模型：`base_brush` 是从笔刷库解析出的原始笔刷（编辑器改它），
//! `brush` = base + 颜色/橡皮/光谱/半径覆盖，是真正作画的那支。
//! 这样「改调色板颜色」不会抹掉你在编辑器里做的曲线调整。

use std::sync::{Arc, Mutex};

use eframe::egui;

use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::{Brush, BrushSetting, BrushState};
use pigment_surface::PigmentSurface;

use crate::brushes::BrushPanel;
use crate::convert;
use crate::editor::BrushEditor;
use crate::gpu::{CanvasPaint, TileRenderer, TileUpload};
use crate::view::View;

/// 颜色预设（真实颜料取向：光谱模式下表现正确）。
const COLORS: &[(&str, [f32; 3])] = &[
    ("焦茶", [0.07, 0.75, 0.25]),
    ("镉黄", [0.144, 0.90, 1.00]),
    ("群青", [0.647, 0.73, 0.75]),
    ("钛白", [0.0, 0.0, 1.0]),
    ("镉红", [0.0, 0.85, 0.70]),
];

pub struct PigmentApp {
    surface: PigmentSurface,
    // ---- 笔刷 ----
    brushes: BrushPanel,
    editor: BrushEditor,
    /// 库笔刷（编辑器在此之上改设置）。
    base_brush: Brush,
    /// 实际作画的笔刷（base + 覆盖）。
    brush: Brush,
    /// 半径覆盖（按 `[`/`]` 才产生；换笔刷时清空，回到笔刷自带尺寸）。
    radius_override: Option<f32>,
    /// 橡皮覆盖（`E`）。
    eraser: bool,
    /// 光谱混色覆盖（`P`；`None` = 用笔刷自带值）。
    paint_override: Option<bool>,
    /// 当前颜色 `[h, s, v]`。
    color: [f32; 3],
    // ---- 交互与渲染 ----
    stroke: Option<BrushState>,
    view: View,
    renderer: Arc<Mutex<TileRenderer>>,
    /// 下一帧强制清空显存缓存并全量重传（诊断键 F1）。
    force_resync: bool,
    /// 本帧计划上传的 tile 数（命中上限说明还有欠账，需继续重绘）。
    pending_uploads: usize,
    stroke_dirty: bool,
    show_panels: bool,
    fullscreen: bool,
}

impl PigmentApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("pigment 需要 wgpu renderer（NativeOptions.renderer = Wgpu）");
        let renderer = TileRenderer::new(&rs.device, rs.target_format);

        let brushes = BrushPanel::new();
        let base_brush = brushes.load_selected().unwrap_or_else(|e| {
            eprintln!("笔刷加载失败，退回内嵌默认笔刷: {e}");
            pigment_engine::brushlib::default_brush()
        });

        let mut app = Self {
            surface: PigmentSurface::new(),
            brushes,
            editor: BrushEditor::new(),
            base_brush,
            brush: Brush::new(),
            radius_override: None,
            eraser: false,
            paint_override: None,
            color: COLORS[0].1,
            stroke: None,
            view: View::default(),
            renderer: Arc::new(Mutex::new(renderer)),
            force_resync: false,
            pending_uploads: 0,
            stroke_dirty: false,
            show_panels: true,
            fullscreen: false,
        };
        app.derive_active();
        app
    }

    /// 由 `base_brush` + 覆盖项推导出实际作画笔刷。
    ///
    /// 颜色只覆盖 `color_*` 的 base_value 而**保留笔刷自带的颜色动态**，
    /// 与 libmypaint 的 `mypaint_brush_set_base_value(COLOR_*)` 语义一致。
    fn derive_active(&mut self) {
        let mut b = self.base_brush.clone();
        let [h, s, v] = self.color;
        b.get_mut(BrushSetting::ColorH).base_value = h;
        b.get_mut(BrushSetting::ColorS).base_value = s;
        b.get_mut(BrushSetting::ColorV).base_value = v;
        if self.eraser {
            b.get_mut(BrushSetting::Eraser).base_value = 1.0;
        }
        if let Some(p) = self.paint_override {
            b.get_mut(BrushSetting::Paint).base_value = if p { 1.0 } else { 0.0 };
        }
        if let Some(r) = self.radius_override {
            b.get_mut(BrushSetting::Radius).base_value = r;
        }
        self.brush = b;
        self.editor.mark_dirty();
    }

    /// 从笔刷库重新载入当前选中笔刷（清空半径/光谱覆盖，恢复笔刷自带性格）。
    fn reload_selected_brush(&mut self) {
        match self.brushes.load_selected() {
            Ok(b) => {
                self.base_brush = b;
                self.radius_override = None;
                self.eraser = false;
                self.paint_override = None;
            }
            Err(e) => eprintln!("笔刷加载失败: {e}"),
        }
        self.derive_active();
    }

    /// 当前生效的光谱混色开关状态。
    fn paint_active(&self) -> bool {
        self.paint_override
            .unwrap_or_else(|| self.base_brush.get(BrushSetting::Paint).base_value >= 0.5)
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (smaller, larger, eraser, paint, color_key, resync, dump, prev, next, tab, f11) =
            ctx.input(|i| {
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
                    i.key_pressed(egui::Key::F1),
                    i.key_pressed(egui::Key::F2),
                    i.key_pressed(egui::Key::Comma),
                    i.key_pressed(egui::Key::Period),
                    i.key_pressed(egui::Key::Tab),
                    i.key_pressed(egui::Key::F11),
                )
            });

        if resync {
            // 诊断：丢弃全部显存 tile 并重传。若画面恢复正常 → 属显存同步问题
            self.force_resync = true;
        }
        if dump {
            // 诊断：导出 CPU 侧画布为 PNG，与屏幕画面对照可区分「CPU 无内容」与「显存没同步」
            match crate::dump::dump_surface_png(&self.surface) {
                Ok(p) => println!("已导出 CPU 画布快照: {}", p.display()),
                Err(e) => eprintln!("导出失败: {e}"),
            }
        }
        if tab {
            self.show_panels = !self.show_panels;
        }
        if f11 {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if prev || next {
            if self.brushes.step(if next { 1 } else { -1 }) {
                self.reload_selected_brush();
            }
        }

        let mut dirty = false;
        if smaller || larger {
            let cur = self.brush.get(BrushSetting::Radius).base_value;
            let step = if larger { 0.15 } else { -0.15 };
            self.radius_override = Some((cur + step).clamp(-1.0, 6.0));
            dirty = true;
        }
        if eraser {
            self.eraser = !self.eraser;
            dirty = true;
        }
        if paint {
            self.paint_override = Some(!self.paint_active());
            dirty = true;
        }
        if let Some(idx) = color_key {
            self.color = COLORS[idx.min(COLORS.len() - 1)].1;
            dirty = true;
        }
        if dirty {
            self.derive_active();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.show_panels, "☰")
                    .on_hover_text("显示/隐藏面板（Tab）")
                    .clicked()
                {
                    self.show_panels = !self.show_panels;
                }
                ui.separator();
                for (i, (name, hsv)) in COLORS.iter().enumerate() {
                    let selected = self.color == *hsv && !self.eraser;
                    if ui
                        .selectable_label(selected, format!("{} {name}", i + 1))
                        .clicked()
                    {
                        self.color = *hsv;
                        self.eraser = false;
                        self.derive_active();
                    }
                }
                ui.separator();
                if ui.selectable_label(self.eraser, "E 橡皮").clicked() {
                    self.eraser = !self.eraser;
                    self.derive_active();
                }
                let mut paint = self.paint_active();
                if ui
                    .checkbox(&mut paint, "P 光谱混色")
                    .on_hover_text("关掉即退回 RGB 插值，便于并排对比")
                    .changed()
                {
                    self.paint_override = Some(paint);
                    self.derive_active();
                }
                ui.separator();
                ui.label(format!("半径 {:.0}px", self.brush.get(BrushSetting::Radius).base_value.exp()));
                ui.label(format!("缩放 {:.0}%", self.view.zoom * 100.0));
                ui.separator();
                let resident = self.renderer.lock().unwrap().resident();
                ui.label(format!(
                    "tiles {} · {:.1} MiB · 显存 {resident}",
                    self.surface.tile_count(),
                    self.surface.memory_bytes() as f64 / 1048576.0
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let name = self
                        .brushes
                        .selected_entry()
                        .map(|e| e.display.clone())
                        .unwrap_or_default();
                    ui.label(egui::RichText::new(name).strong());
                });
            });
        });
    }

    /// 右侧面板：笔刷信息 + 颜色 + 笔刷编辑器。
    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::right("editor")
            .resizable(true)
            .default_size(310.0)
            .size_range(250.0..=520.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // ---- 笔刷信息 ----
                        if let Some(e) = self.brushes.selected_entry() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&e.display).heading());
                                ui.label(egui::RichText::new(&e.group_display).weak().small());
                            });
                            ui.label(
                                egui::RichText::new(&e.key)
                                    .weak()
                                    .small()
                                    .monospace(),
                            );
                            if !e.notes.is_empty() {
                                egui::CollapsingHeader::new("笔刷说明 / 授权")
                                    .id_salt("brush-notes")
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new(&e.notes).small());
                                    });
                            }
                        }
                        ui.separator();

                        // ---- 颜色 ----
                        egui::CollapsingHeader::new("颜色")
                            .id_salt("color")
                            .default_open(true)
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    for (name, hsv) in COLORS {
                                        if ui
                                            .selectable_label(self.color == *hsv, *name)
                                            .clicked()
                                        {
                                            self.color = *hsv;
                                            self.derive_active();
                                        }
                                    }
                                });
                                let mut color = self.color;
                                let mut changed = false;
                                ui.horizontal(|ui| {
                                    ui.label("色相");
                                    changed |= ui
                                        .add(egui::Slider::new(&mut color[0], 0.0..=1.0).show_value(true))
                                        .changed();
                                });
                                ui.horizontal(|ui| {
                                    ui.label("饱和");
                                    changed |= ui
                                        .add(egui::Slider::new(&mut color[1], 0.0..=1.0).show_value(true))
                                        .changed();
                                });
                                ui.horizontal(|ui| {
                                    ui.label("明度");
                                    changed |= ui
                                        .add(egui::Slider::new(&mut color[2], 0.0..=1.0).show_value(true))
                                        .changed();
                                });
                                if changed {
                                    self.color = color;
                                    self.derive_active();
                                }
                            });

                        // ---- 笔刷编辑器 ----
                        egui::CollapsingHeader::new("笔刷编辑器")
                            .id_salt("editor")
                            .default_open(true)
                            .show(ui, |ui| {
                                let edited = self.editor.show(
                                    ui,
                                    &mut self.base_brush,
                                    &self.brush,
                                    self.color,
                                );
                                if edited {
                                    self.derive_active();
                                }
                            });

                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "编辑只作用于当前会话的这支笔刷（尚未回写 .myb）\n\
                                 用户笔刷目录：%APPDATA%/mypaint/brushes\n\
                                 或用 PIGMENT_BRUSH_PATH 指定",
                            )
                            .weak()
                            .small(),
                        );
                    });
            });
        let _ = ctx;
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
            if let (Some(pos), Some(mut st)) = (response.interact_pointer_pos(), self.stroke.take())
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
        let mut visible =
            Vec::with_capacity(((tx1 - tx0 + 1).max(0) * (ty1 - ty0 + 1).max(0)) as usize);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                visible.push((tx, ty));
            }
        }

        // ---- 自愈式增量同步：比对 CPU revision 与显存已上传 revision ----
        // 判据是状态而非事件流水，所以本帧的决定可被安全丢弃后重算，不会丢上传。
        let mut uploads: Vec<TileUpload> = Vec::new();
        {
            let mut renderer = self.renderer.lock().unwrap();
            if self.force_resync {
                renderer.invalidate_all();
                self.force_resync = false;
            }
            let plan = crate::gpu::plan_uploads(
                &self.surface,
                |c| renderer.uploaded_rev(c),
                &visible,
                crate::gpu::MAX_UPLOADS_PER_FRAME,
            );
            for coord in plan {
                let Some(t) = self.surface.tile_entry(coord.0, coord.1) else {
                    continue;
                };
                uploads.push(TileUpload {
                    coord,
                    revision: t.revision(),
                    pixels: convert::tile_to_rgba8_on_white(t.pixels()),
                });
            }
        }
        self.pending_uploads = uploads.len();

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

        ui.painter()
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                CanvasPaint {
                    renderer: Arc::clone(&self.renderer),
                    view_matrix,
                    uploads,
                    visible,
                },
            ));

        // ---- 笔刷光标预览：放在 callback 之后，保证压在 tile 之上可见 ----
        if let Some(pos) = response.hover_pos() {
            let r = self
                .brush
                .get(BrushSetting::Radius)
                .base_value
                .exp()
                .max(0.5)
                * self.view.zoom;
            let color = if self.eraser {
                egui::Color32::from_rgb(200, 80, 80)
            } else {
                egui::Color32::from_gray(110)
            };
            ui.painter()
                .circle_stroke(pos, r.max(1.0), egui::Stroke::new(1.0, color));
        }

        // 绘制中持续重绘（dab 沿笔画插值）；上传有欠账（命中每帧上限）时也要继续，
        // 否则剩下的 tile 会一直停在旧状态
        let backlog = self.pending_uploads >= crate::gpu::MAX_UPLOADS_PER_FRAME;
        if self.stroke.is_some() || self.stroke_dirty || backlog {
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

        if self.show_panels {
            // 左：笔刷库
            let color = self.color;
            let mut pick = false;
            egui::Panel::left("brushes")
                .resizable(true)
                .default_size(240.0)
                .size_range(170.0..=420.0)
                .show(ui, |ui| {
                    pick = self.brushes.show(ui, color);
                });
            if pick {
                self.reload_selected_brush();
            }
            // 右：编辑器
            self.side_panel(ui);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::WHITE))
            .show(ui, |ui| self.canvas(ui));
    }
}
