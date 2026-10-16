//! pigment 应用主体：MyPaint「画布优先」布局 —— 全窗口画布 + 左右可折叠面板。
//!
//! 交互（**照搬 MyPaint 键位**，此处只列已移植的）：
//! - 左键拖拽绘制；中/右键或 Space+左键平移；滚轮以光标为中心缩放
//! - `[` / `]` 笔刷半径；`E` 橡皮；`P` 光谱混色开关
//! - `1..9,0` 恢复笔刷快捷键槽位；`Ctrl+数字` 存入槽位
//!   （`gui/document.py:_init_context_actions`）
//! - `,` / `.` 上一支 / 下一支笔刷；`Tab` 收起/展开面板；`F11` 全屏
//! - `Ctrl+Z` 撤销、`Ctrl+Shift+Z` 重做（`gui/resources.xml:217-231` 的
//!   动作加速键），另有 MyPaint 老式键位 `z` 撤销 / `y` 重做
//!   （`gui/document.py:527-530`）
//! - `F1` 强制全量重传显存 tile；`F2` 导出 CPU 侧画布 PNG（诊断）
//!
//! 数据流：指针事件 → hokusai 笔画引擎 → PigmentSurface（CPU fix15）
//!   → revision 比对收集脏 tile → convert 为 sRGB8 → GPU 增量上传 → 渲染。
//!
//! 笔刷模型：`base_brush` 是从笔刷库解析出的原始笔刷（编辑器改它），
//! `brush` = base + 颜色/橡皮/光谱/半径覆盖，是真正作画的那支。
//! 对齐 MyPaint：颜色由应用持有并推给笔刷的 `color_h/s/v`
//! （`gui/brushcolor.py:44`），所以改调色不会抹掉编辑器里做的曲线调整。
//!
//! 撤销模型：一次拖拽 = 一条 [`Brushwork`]（`lib/command.py:250`）——
//! 起笔取"画前快照"、落笔取"画后快照"，撤销/重做就是在这两份快照之间
//! 来回切（写时复制，见 `pigment-surface` 的模块文档）。命令栈由
//! [`Document`] 持有，栈深 = MyPaint 的 `DEFAULT_UNDO_STACK_SIZE`(40)。
//! 本应用每笔新建一份 [`BrushState`]（相当于一次干净的笔画开局），
//! 这一点与 MyPaint 的"一支持久笔刷 + `new_stroke` 复位"有个已知差异，
//! 见《MyPaint源码逐项比对报告.md》第九节。

use std::sync::{Arc, Mutex};

use eframe::egui;

use pigment_engine::brushlib::NUM_BRUSHKEYS;
use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::{Brush, BrushSetting, BrushState};
use pigment_engine::{Brushwork, CommandUpdate, Document, StrokeEvent};

use crate::brushes::BrushPanel;
use crate::color::{self, ColorHistory};
use crate::convert;
use crate::editor::BrushEditor;
use crate::gpu::{CanvasPaint, TileRenderer, TileUpload};
use crate::view::View;

/// 顶栏状态文字的存活时长（秒）。
const STATUS_TTL: f64 = 2.5;

/// 笔刷快捷键槽位（对齐 MyPaint `BrushManager.contexts[i]`，即一只 `context%02d` 笔刷）。
///
/// MyPaint 的槽位存的是**整份笔刷设置快照**（`document.py:1714`：
/// `context.brushinfo = self.app.brush.clone()`），并随颜色一起还原
/// （`misc.context_restores_color` 默认 True，`userconfig.py:93`），
/// 所以这里也存 `Brush` + 颜色快照，而不是库里某个下标的引用。
#[derive(Clone)]
struct BrushKeySlot {
    brush: Brush,
    color: [f32; 3],
    /// 来源名（提示用；MyPaint 里是 `context%02d` + `parent_brush_name`）。
    name: String,
}

/// 按 MyPaint 的规则播种 10 个槽位（`_init_default_brushkeys_and_history`）。
fn seed_brushkeys(panel: &BrushPanel) -> [Option<BrushKeySlot>; NUM_BRUSHKEYS] {
    let seed = panel.library.brushkey_seed();
    std::array::from_fn(|i| {
        let entry = panel.library.entries.get(seed[i]?)?;
        let brush = entry.load().ok()?;
        // 槽位建立时复制的是笔刷文件里的 brushinfo，颜色自然来自文件
        let color = [
            brush.get(BrushSetting::ColorH).base_value,
            brush.get(BrushSetting::ColorS).base_value,
            brush.get(BrushSetting::ColorV).base_value,
        ];
        Some(BrushKeySlot {
            brush,
            color,
            name: format!("{} ({})", entry.mypaint_name(), entry.group_display),
        })
    })
}

/// 画一个颜色方块按钮（MyPaint `ColorHistoryView` 的 `ColorPreview` 的极简版）。
fn color_swatch(ui: &mut egui::Ui, hsv: [f32; 3], selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let [r, g, b] = color::hsv_to_rgb(hsv);
    ui.painter()
        .rect_filled(rect, 3.0, egui::Color32::from_rgb(
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8,
        ));
    if selected {
        ui.painter().rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }
    resp.on_hover_text(format!("{tip}\n{}", color::hex_from_hsv(hsv)))
}

pub struct PigmentApp {
    /// 文档模型（画布表面 + 撤销栈），对应 MyPaint 的 `app.doc.model`。
    doc: Document,
    // ---- 笔刷 ----
    brushes: BrushPanel,
    editor: BrushEditor,
    /// 库笔刷（编辑器在此之上改设置）。
    base_brush: Brush,
    /// 实际作画的笔刷（base + 覆盖）。
    brush: Brush,
    /// 当前笔刷的 MyPaint 展示名（`BrushInfo` 的 `parent_brush_name`）：
    /// 笔画事件流要记它，命令的展示名由它生成（`lib/command.py:331`）。
    brush_name: String,
    /// 半径覆盖（按 `[`/`]` 才产生；换笔刷时清空，回到笔刷自带尺寸）。
    radius_override: Option<f32>,
    /// 橡皮覆盖（`E`）。
    eraser: bool,
    /// 光谱混色覆盖（`P`；`None` = 用笔刷自带值）。
    paint_override: Option<bool>,
    /// 当前颜色 `[h, s, v]`。
    color: [f32; 3],
    /// 颜色历史（`gui/colors/adjbases.py:ColorManager._hist`）。
    color_history: ColorHistory,
    // ---- 笔刷快捷键 ----
    /// 10 个槽位（`BrushManager.contexts`）。
    brushkeys: [Option<BrushKeySlot>; NUM_BRUSHKEYS],
    /// 恢复槽位时是否连颜色一起还原（MyPaint 偏好 `misc.context_restores_color`，默认开）。
    context_restores_color: bool,
    /// 笔刷编辑器「实时更新最近一笔」（`gui/brusheditor.py` 的 live update）：
    /// 开着时改笔刷设置就用新笔重绘上一笔
    /// （`CommandStack::update_last_command`）。
    live_update: bool,
    /// 顶栏临时状态文字（带时间戳，2.5 秒后消失）。
    status: Option<(String, f64)>,
    // ---- 交互与渲染 ----
    /// 正在绘制时的引擎状态（每次起笔新建，落笔丢弃）。
    stroke_state: Option<BrushState>,
    /// 正在记录的作画命令（`gui/mode.py` 的 `__active_brushwork`）。
    brushwork: Option<Brushwork>,
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

        let color_history = ColorHistory::new();
        let brushkeys = seed_brushkeys(&brushes);
        let brush_name = brushes
            .selected_entry()
            .map(|e| e.mypaint_name())
            .unwrap_or_else(|| "Unknown Brush".to_string());
        let mut app = Self {
            doc: Document::new(),
            brushes,
            editor: BrushEditor::new(),
            base_brush,
            brush: Brush::new(),
            brush_name,
            radius_override: None,
            eraser: false,
            paint_override: None,
            color: color_history.current(),
            color_history,
            brushkeys,
            context_restores_color: true,
            live_update: false,
            status: None,
            stroke_state: None,
            brushwork: None,
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
        if let Some(e) = self.brushes.selected_entry() {
            self.brush_name = e.mypaint_name();
        }
        self.derive_active();
    }

    /// 当前生效的光谱混色开关状态。
    fn paint_active(&self) -> bool {
        self.paint_override
            .unwrap_or_else(|| self.base_brush.get(BrushSetting::Paint).base_value >= 0.5)
    }

    /// 设置当前颜色并推给笔刷 —— 对齐 `BrushColorManager.set_color()`
    /// （`gui/brushcolor.py:44`：颜色由应用持有，再写给笔刷的 `color_h/s/v`）。
    fn set_color(&mut self, color: [f32; 3]) {
        self.color = color;
        self.derive_active();
    }

    fn set_status(&mut self, msg: String, now: f64) {
        self.status = Some((msg, now + STATUS_TTL));
    }

    // ---- 笔画生命周期 ----

    /// 组装一个笔画事件。
    ///
    /// 参数表与 `gui/freehand.py:601-611` 一致；压力/倾角的钳位对齐它在
    /// `stroke_to` 之前的 `clamp()`。我们还没有倾角、视图旋转与笔杆旋转
    /// 输入（数位板属 M6），这三项填 0。
    fn stroke_event(&self, x: f32, y: f32, pressure: f32, dtime: f64) -> StrokeEvent {
        StrokeEvent::new(
            dtime,
            x,
            y,
            pressure.clamp(0.0, 1.0),
            0.0,
            0.0,
            self.view.zoom, // viewzoom ← MyPaint 的 tdw.scale
            0.0,
            0.0,
        )
    }

    /// 把一个事件喂给当前作画命令：记录事件流 + 落到画布。
    fn paint_event(&mut self, ev: StrokeEvent) {
        let (Some(mut bw), Some(mut st)) = (self.brushwork.take(), self.stroke_state.take()) else {
            return;
        };
        bw.stroke_to(
            self.doc.surface_mut(),
            &self.brush,
            &self.brush_name,
            &mut st,
            ev,
        );
        self.brushwork = Some(bw);
        self.stroke_state = Some(st);
    }

    /// 落笔：收尾并把这段作画提交给撤销栈
    /// （`gui/mode.py:766-768` 的 `brushwork_commit`）。
    fn commit_stroke(&mut self) {
        let (Some(mut bw), Some(st)) = (self.brushwork.take(), self.stroke_state.take()) else {
            self.brushwork = None;
            self.stroke_state = None;
            return;
        };
        // 没画到东西的命令直接丢弃（`if changed: model.do(cmd)`）
        if bw.stop_recording(self.doc.surface_mut(), &st, false) {
            self.doc.do_brushwork(bw);
        }
    }

    /// 把在途作画收尾（撤销/重做前的清账，对应 `Document.sync_pending_changes()`）。
    fn flush_pending_stroke(&mut self) {
        if self.brushwork.is_some() {
            self.commit_stroke();
        }
    }

    // ---- 撤销 / 重做 ----

    /// 撤销（`gui/document.py:552` 的 `undo_cb`）。
    fn undo(&mut self, now: f64) {
        self.flush_pending_stroke();
        match self.doc.undo() {
            // 文案对齐 `gui/document.py:192` 的 `UNDO_CMD = _("Undo %s")`
            Some(name) => self.set_status(format!("撤销：{name}"), now),
            None => self.set_status("没有可撤销的操作".to_string(), now),
        }
    }

    /// 重做（`gui/document.py:558` 的 `redo_cb`）。
    fn redo(&mut self, now: f64) {
        self.flush_pending_stroke();
        match self.doc.redo() {
            Some(name) => self.set_status(format!("重做：{name}"), now),
            None => self.set_status("没有可重做的操作".to_string(), now),
        }
    }

    /// 笔刷编辑器的 Live Update：用当前笔刷重绘上一笔
    /// （`gui/brusheditor.py:1135` 的 `_live_update_idle_cb`）。
    fn apply_live_update(&mut self) {
        if self.brushwork.is_some() {
            // 正在画的一笔不是"上一笔"，且它还没收尾（`sshot_after` 未取）
            // → 交给下一次编辑动作。照搬源码的 `get_last_command()` 语义。
            return;
        }
        let brush = self.brush.clone();
        let name = self.brush_name.clone();
        let updated = self.doc.update_last_command(&CommandUpdate::Brush {
            brush: &brush,
            name: &name,
        });
        if updated.is_none() {
            // 没有可更新的作画命令（或笔刷设置无法序列化）
            return;
        }
        // 重绘会改写 surface 内容，显存按 revision 自愈式重传
        self.stroke_dirty = true;
    }

    /// 恢复笔刷快捷键槽位 —— 对齐 `document.context_cb` 的非 store 分支：
    /// `bm.select_brush(context)`，若 `misc.context_restores_color` 再还原颜色。
    fn restore_brushkey(&mut self, i: usize, now: f64) {
        let Some(slot) = self.brushkeys[i].clone() else {
            self.set_status(format!("槽位 {i} 为空"), now);
            return;
        };
        self.base_brush = slot.brush;
        self.radius_override = None;
        self.eraser = false;
        self.paint_override = None;
        if self.context_restores_color {
            self.color = slot.color;
        }
        self.derive_active();
        self.set_status(format!("笔刷 {i} ← {}", slot.name), now);
    }

    /// 存入笔刷快捷键槽位 —— 对齐 `document.context_cb` 的 store 分支：
    /// `context.brushinfo = app.brush.clone()`（整份快照，不是库引用）。
    fn store_brushkey(&mut self, i: usize, now: f64) {
        let name = self
            .brushes
            .selected_entry()
            .map(|e| e.mypaint_name())
            .unwrap_or_else(|| "当前笔刷".to_string());
        self.brushkeys[i] = Some(BrushKeySlot {
            brush: self.base_brush.clone(),
            color: self.color,
            name,
        });
        self.set_status(format!("笔刷 {i} ← 已存入当前设置"), now);
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // GTK 的 accelerator 在文本输入框获得焦点时不会触发（MyPaint 的笔刷搜索框同理），
        // egui 的 `key_pressed` 不看焦点，所以这里自己补上等价判断。
        if ctx.text_edit_focused() {
            return;
        }
        // MyPaint `_init_context_actions`（gui/document.py:468）：
        //   数字键    → Context0N，accel "N"            → 恢复槽位 N
        //   Ctrl+数字 → Context0Ns，accel "<control>N"  → 存入槽位 N
        const DIGITS: [egui::Key; NUM_BRUSHKEYS] = [
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        let (
            restore,
            store,
            smaller,
            larger,
            eraser,
            paint,
            resync,
            dump,
            prev,
            next,
            tab,
            f11,
            undo,
            redo,
            now,
        ) = ctx.input(|i| {
            let (mut restore, mut store) = (None, None);
            for (digit, key) in DIGITS.iter().enumerate() {
                if !i.key_pressed(*key) {
                    continue;
                }
                if i.modifiers.ctrl {
                    store = Some(digit);
                } else if !i.modifiers.alt && !i.modifiers.command {
                    restore = Some(digit);
                }
            }
            let now = i.time;
            // 撤销/重做键位（`gui/resources.xml:217-231` + `gui/document.py:527-530`）：
            //   Ctrl+Z           → 撤销
            //   Ctrl+Shift+Z     → 重做；Ctrl+Y → 重做
            //   z / y（无修饰）  → MyPaint 老式键位
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
            let shift = i.modifiers.shift;
            let no_mod = !ctrl && !shift && !i.modifiers.alt;
            let undo = (ctrl && !shift && i.key_pressed(egui::Key::Z))
                || (no_mod && i.key_pressed(egui::Key::Z));
            let redo = (ctrl && i.key_pressed(egui::Key::Y))
                || (ctrl && shift && i.key_pressed(egui::Key::Z))
                || (no_mod && i.key_pressed(egui::Key::Y));
            (
                restore.map(|d| (d, now)),
                store.map(|d| (d, now)),
                i.key_pressed(egui::Key::OpenBracket),
                i.key_pressed(egui::Key::CloseBracket),
                i.key_pressed(egui::Key::E),
                i.key_pressed(egui::Key::P),
                i.key_pressed(egui::Key::F1),
                i.key_pressed(egui::Key::F2),
                i.key_pressed(egui::Key::Comma),
                i.key_pressed(egui::Key::Period),
                i.key_pressed(egui::Key::Tab),
                i.key_pressed(egui::Key::F11),
                undo,
                redo,
                now,
            )
        });

        if undo {
            self.undo(now);
            ctx.request_repaint();
        }
        if redo {
            self.redo(now);
            ctx.request_repaint();
        }
        if resync {
            // 诊断：丢弃全部显存 tile 并重传。若画面恢复正常 → 属显存同步问题
            self.force_resync = true;
        }
        if dump {
            // 诊断：导出 CPU 侧画布为 PNG，与屏幕画面对照可区分「CPU 无内容」与「显存没同步」
            match crate::dump::dump_surface_png(self.doc.surface()) {
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
        if let Some((digit, now)) = store {
            self.store_brushkey(digit, now);
        }
        if let Some((digit, now)) = restore {
            self.restore_brushkey(digit, now);
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
                // ---- 撤销 / 重做（MyPaint 工具条的 toolbar-edit.xml）----
                // 文案与置灰规则照搬 `gui/document.py:582-591` 的
                // `_update_undo_redo`：有命令时显示 "Undo <命令名>"，否则 "Undo"。
                let now_tb = ui.input(|i| i.time);
                let undo_label = match self.doc.get_last_command() {
                    Some(cmd) => format!("↶ 撤销 {}", cmd.display_name()),
                    None => "↶ 撤销".to_string(),
                };
                let redo_label = match self.doc.command_stack.next_redo_name() {
                    Some(name) => format!("↷ 重做 {name}"),
                    None => "↷ 重做".to_string(),
                };
                let can_undo = self.doc.command_stack.undo_len() > 0;
                let can_redo = self.doc.command_stack.redo_len() > 0;
                if ui
                    .add_enabled(can_undo, egui::Button::new(undo_label))
                    .on_hover_text("撤销（Ctrl+Z / z）")
                    .clicked()
                {
                    self.undo(now_tb);
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new(redo_label))
                    .on_hover_text("重做（Ctrl+Shift+Z / Ctrl+Y / y）")
                    .clicked()
                {
                    self.redo(now_tb);
                }
                ui.separator();
                // ---- 颜色历史（`ColorManager._hist`：最旧 → 最新）----
                let history: Vec<[f32; 3]> = self.color_history.colors().to_vec();
                let n = history.len();
                let cur_rgb = color::hsv_to_rgb(self.color);
                for (i, hsv) in history.into_iter().enumerate() {
                    let rgb = color::hsv_to_rgb(hsv);
                    let selected = (0..3).all(|k| (rgb[k] - cur_rgb[k]).abs() < 1e-3);
                    let tip = format!(
                        "颜色历史 {}/{}{}\n点击设为当前色（每笔结束后自动记录）",
                        i + 1,
                        n,
                        if i + 1 == n { "（最近）" } else { "" }
                    );
                    if color_swatch(ui, hsv, selected, &tip).clicked() {
                        self.set_color(hsv);
                        self.eraser = false;
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
                    self.doc.surface().tile_count(),
                    self.doc.surface().memory_bytes() as f64 / 1048576.0
                ));
                // MyPaint 的底栏有 128px 笔刷指示按钮；我们先把「笔刷名 + 描述」
                // 与快捷键槽位反馈放在同一处（GTK 底栏未移植前的权宜位置）
                let now = ui.input(|i| i.time);
                let status = self
                    .status
                    .as_ref()
                    .filter(|(_, until)| now < *until)
                    .map(|(msg, _)| msg.clone());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let entry = self.brushes.selected_entry().cloned();
                    if let Some(e) = entry {
                        ui.label(egui::RichText::new(e.mypaint_name()).strong())
                            .on_hover_text(if e.display.is_empty() {
                                e.key.clone()
                            } else {
                                format!("{}\n{}", e.mypaint_name(), e.display)
                            });
                    }
                    if let Some(msg) = status {
                        ui.separator();
                        ui.label(egui::RichText::new(msg).weak().small());
                    }
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
                                // 主标题用 MyPaint 的展示名（笔刷名），description 退居 tooltip
                                ui.label(egui::RichText::new(e.mypaint_name()).heading())
                                    .on_hover_text(if e.display.is_empty() {
                                        e.key.clone()
                                    } else {
                                        format!("{}\n{}", e.mypaint_name(), e.display)
                                    });
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

                        // ---- 颜色历史 + HSV 调整 ----
                        egui::CollapsingHeader::new("颜色")
                            .id_salt("color")
                            .default_open(true)
                            .show(ui, |ui| {
                                let history: Vec<[f32; 3]> = self.color_history.colors().to_vec();
                                let n = history.len();
                                ui.horizontal_wrapped(|ui| {
                                    for (i, hsv) in history.into_iter().enumerate() {
                                        let tip = format!("颜色历史 {}/{}", i + 1, n);
                                        if color_swatch(ui, hsv, false, &tip).clicked() {
                                            self.set_color(hsv);
                                        }
                                    }
                                    ui.label(
                                        egui::RichText::new("最近用的在最后")
                                            .weak()
                                            .small(),
                                    );
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
                                    self.set_color(color);
                                }
                            });

                        // ---- 笔刷快捷键（MyPaint `BrushManager.contexts`）----
                        egui::CollapsingHeader::new("笔刷快捷键 1–0")
                            .id_salt("brushkeys")
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(
                                        "数字键恢复槽位；Ctrl+数字把当前设置存进槽位。\n\
                                         槽位内容 = 整份笔刷设置（含颜色），与 MyPaint 一致。",
                                    )
                                    .weak()
                                    .small(),
                                );
                                let now = ui.input(|i| i.time);
                                ui.horizontal_wrapped(|ui| {
                                    for i in 0..NUM_BRUSHKEYS {
                                        let label = i.to_string();
                                        let slot = self.brushkeys[i].as_ref();
                                        let tip = match slot {
                                            Some(s) => {
                                                format!("{} ← {}", i, s.name)
                                            }
                                            None => format!("{i} ← 空槽位"),
                                        };
                                        let resp = ui
                                            .add(egui::Button::new(label).min_size(egui::vec2(
                                                22.0, 22.0,
                                            )))
                                            .on_hover_text(format!(
                                                "{tip}\n左键恢复 / Ctrl+左键存入"
                                            ));
                                        if !resp.clicked() {
                                            continue;
                                        }
                                        if ui.input(|i| i.modifiers.ctrl) {
                                            self.store_brushkey(i, now);
                                        } else {
                                            self.restore_brushkey(i, now);
                                        }
                                    }
                                });
                                ui.checkbox(
                                    &mut self.context_restores_color,
                                    "恢复槽位时连颜色一起还原",
                                )
                                .on_hover_text(
                                    "MyPaint 偏好 misc.context_restores_color（默认开）\n\
                                     userconfig.py:93",
                                );
                            });

                        // ---- 笔刷编辑器 ----
                        egui::CollapsingHeader::new("笔刷编辑器")
                            .id_salt("editor")
                            .default_open(true)
                            .show(ui, |ui| {
                                // 「实时更新最近一笔」= MyPaint 笔刷编辑器的 live_update
                                // （`gui/brusheditor.py:1119-1139`）：改设置就用新笔
                                // 重绘上一笔，靠的是 `Brushwork.update()`
                                ui.checkbox(&mut self.live_update, "实时更新最近一笔")
                                    .on_hover_text(
                                        "开着时改笔刷设置会用新笔重绘上一笔\n\
                                         （MyPaint 笔刷编辑器的 Live Update）",
                                    );
                                // 编辑器顶部图标 = 当前笔刷的预览图（MyPaint `brusheditor.py:786`）
                                let sel = self.brushes.selected;
                                let icon = self.brushes.icon(ui.ctx(), sel);
                                let edited =
                                    self.editor.show(ui, &mut self.base_brush, Some(&icon));
                                if edited {
                                    self.derive_active();
                                    if self.live_update {
                                        self.apply_live_update();
                                    }
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
        // 一次拖拽 = 一条 Brushwork（`gui/mode.py:722` 的 brushwork_begin/commit）：
        // 起笔取画前快照，落笔取画后快照并提交给撤销栈。
        let space_down = ui.input(|i| i.key_down(egui::Key::Space));
        if !space_down && response.drag_started_by(egui::PointerButton::Primary) {
            // 本应用的笔画开局：新建一份引擎状态（见文件头「撤销模型」注释）
            self.stroke_state = Some(BrushState::default());
            self.brushwork = Some(Brushwork::new());
            if let Some(pos) = response.interact_pointer_pos() {
                let local = pos - rect.min;
                let [wx, wy] = self.view.screen_to_world(local.x, local.y);
                let ev = self.stroke_event(wx, wy, 1.0, 0.01);
                self.paint_event(ev);
            }
            self.stroke_dirty = true;
        }
        if !space_down && response.dragged_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = pos - rect.min;
                let [wx, wy] = self.view.screen_to_world(local.x, local.y);
                let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05) as f64;
                let ev = self.stroke_event(wx, wy, 1.0, dt);
                self.paint_event(ev);
            }
            self.stroke_dirty = true;
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.commit_stroke();
            // 颜色历史入栈时机对齐 MyPaint：`__input_stroke_ended_cb`
            // （gui/brushcolor.py:54）——每笔结束后记录，且橡皮不记录。
            if !self.eraser {
                self.color_history.push(self.color);
            }
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
        let evict: Vec<(i32, i32)> = {
            let mut renderer = self.renderer.lock().unwrap();
            if self.force_resync {
                renderer.invalidate_all();
                self.force_resync = false;
            }
            let plan = crate::gpu::plan_uploads(
                self.doc.surface(),
                |c| renderer.uploaded_rev(c),
                &visible,
                crate::gpu::MAX_UPLOADS_PER_FRAME,
            );
            for coord in plan {
                let Some(t) = self.doc.surface().tile_entry(coord.0, coord.1) else {
                    continue;
                };
                uploads.push(TileUpload {
                    coord,
                    revision: t.revision(),
                    pixels: convert::tile_to_rgba8(t.pixels(), convert::CANVAS_BG),
                });
            }
            // 撤销会把画布退回较早状态：那时"多出来"的 tile 已从 CPU 消失，
            // 但显存里还留着旧内容，必须剔除，否则画面残留撤销不掉的笔迹。
            crate::gpu::plan_evictions(
                self.doc.surface(),
                |c| renderer.uploaded_rev(c).is_some(),
                &visible,
            )
        };
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
                    evict,
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
        if self.stroke_state.is_some() || self.stroke_dirty || backlog {
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
            // 左：笔刷分组面板（MyPaint 的 `BrushGroupTool`）
            let mut pick = false;
            egui::Panel::left("brushes")
                .resizable(true)
                .default_size(240.0)
                // 下界 = `TOOL_WIDGET_MIN_WIDTH`（`gui/toolstack.py:40`）
                .size_range(crate::brushes::TOOL_WIDGET_MIN_WIDTH..=520.0)
                .show(ui, |ui| {
                    pick = self.brushes.show(ui);
                });
            if pick {
                self.reload_selected_brush();
            }
            // 右：编辑器
            self.side_panel(ui);
        }

        egui::CentralPanel::default()
            // 画布底色 = MyPaint 默认背景（米白纸色），不是纯白
            .frame(egui::Frame::NONE.fill(convert::canvas_bg_color()))
            .show(ui, |ui| self.canvas(ui));
    }
}
