//! pigment 应用主体 —— 状态与行为。
//!
//! **窗口结构已按 MyPaint 的三段骨架拆分**（`gui/mypaint.glade`）：
//! 顶栏 / 工作区 / 底栏分别在 [`crate::shell`] 的 `topbar` / `workspace` / `footer`
//! 子模块里绘制，本文件只留状态、笔画生命周期、撤销与文档 I/O。
//!
//! 交互（**键位全部查表，不在这里手写**）：
//!
//! 所有键位来自 [`crate::shell::actions`]，它由构建期解析 `gui/resources.xml`
//! 生成（219 个动作 / 105 条加速键），再叠上源码里代码注册的 49 条
//! （`document.py:_init_extra_keys` 的 29 条 + `_init_context_actions` 的 20 条）。
//! 本文件只负责"动作名 → 行为"的映射（[`PigmentApp::run_action`]）。
//!
//! - 画布：左键拖拽绘制；中/右键或 Space+左键平移；滚轮以光标为中心缩放
//! - 笔刷尺寸 `f` / `d`（`Bigger`/`Smaller`，`resources.xml:275,283`）
//! - 橡皮 `e`、普通模式 `n`（`BlendModeEraser`/`BlendModeNormal`）
//! - 缩放 `,` / `.`、平移 `←↑↓→`（`ZoomIn/Out`、`Pan{Left,Right,Up,Down}`）
//! - 撤销 `Ctrl+Z`（老式键位 `z`）、重做 `Ctrl+Shift+Z` / `Ctrl+Y` / `y`
//! - 笔刷槽位 `0`-`9` 恢复、`Ctrl+0`-`9` 存入
//!   （`gui/document.py:_init_context_actions`，运行期动态注册的动作）
//! - `Ctrl+N/O/S`、`Ctrl+Shift+S/E` 文档 I/O；`Ctrl+Q` 退出
//! - `F11` 全屏、`F12` 重置视图、`F10` 适配视图、`Tab` 全屏自动隐藏
//!   （后三个依赖尚未移植的能力，动作命中但**不消费**，见 `run_action`）
//! - **应用本地扩展键**（源码里没有，用空闲键位）：`x` 光谱混色开关、
//!   `Ctrl+,` / `Ctrl+.` 上一支 / 下一支笔刷、`Ctrl+F1` 重传显存、
//!   `Ctrl+F2` 导出 CPU 画布（诊断）。理由逐条写在 `actions::APP_KEYS` 里。
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
use pigment_engine::hokusai::{Brush, BrushSetting, BrushState};
use pigment_engine::{Brushwork, CommandUpdate, Document, StrokeEvent};

use crate::brushes::BrushPanel;
use crate::color::{self, ColorHistory};
use crate::editor::BrushEditor;
use crate::gpu::TileRenderer;
use crate::shell::actions;
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
pub(crate) struct BrushKeySlot {
    pub(crate) brush: Brush,
    pub(crate) color: [f32; 3],
    /// 来源名（提示用；MyPaint 里是 `context%02d` + `parent_brush_name`）。
    pub(crate) name: String,
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
pub(crate) fn color_swatch(
    ui: &mut egui::Ui,
    hsv: [f32; 3],
    selected: bool,
    tip: &str,
) -> egui::Response {
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

/// 字段一律 `pub(crate)`：UI 已经按 `gui/` 的模块边界拆到 [`crate::shell`] 的
/// 子模块里（顶栏 / 工作区 / 底栏），那些模块要读写这些状态。
/// 「谁能改谁」的约束靠方法（`set_color`、`derive_active` 等）而不是字段可见性。
pub struct PigmentApp {
    /// 文档模型（画布表面 + 撤销栈），对应 MyPaint 的 `app.doc.model`。
    pub(crate) doc: Document,
    // ---- 笔刷 ----
    pub(crate) brushes: BrushPanel,
    pub(crate) editor: BrushEditor,
    /// 库笔刷（编辑器在此之上改设置）。
    pub(crate) base_brush: Brush,
    /// 实际作画的笔刷（base + 覆盖）。
    pub(crate) brush: Brush,
    /// 当前笔刷的 MyPaint 展示名（`BrushInfo` 的 `parent_brush_name`）：
    /// 笔画事件流要记它，命令的展示名由它生成（`lib/command.py:331`）。
    pub(crate) brush_name: String,
    /// 半径覆盖（按 `[`/`]` 才产生；换笔刷时清空，回到笔刷自带尺寸）。
    pub(crate) radius_override: Option<f32>,
    /// 橡皮覆盖（`E`）。
    pub(crate) eraser: bool,
    /// 光谱混色覆盖（`P`；`None` = 用笔刷自带值）。
    pub(crate) paint_override: Option<bool>,
    /// 当前颜色 `[h, s, v]`。
    pub(crate) color: [f32; 3],
    /// 颜色历史（`gui/colors/adjbases.py:ColorManager._hist`）。
    pub(crate) color_history: ColorHistory,
    // ---- 笔刷快捷键 ----
    /// 10 个槽位（`BrushManager.contexts`）。
    pub(crate) brushkeys: [Option<BrushKeySlot>; NUM_BRUSHKEYS],
    /// 恢复槽位时是否连颜色一起还原（MyPaint 偏好 `misc.context_restores_color`，默认开）。
    pub(crate) context_restores_color: bool,
    /// 笔刷编辑器「实时更新最近一笔」（`gui/brusheditor.py` 的 live update）：
    /// 开着时改笔刷设置就用新笔重绘上一笔
    /// （`CommandStack::update_last_command`）。
    pub(crate) live_update: bool,
    /// 临时状态文字（带时间戳，`STATUS_TTL` 秒后消失）。
    ///
    /// MyPaint 的瞬时反馈走 `GtkStatusbar` 的独立 context；我们统一收在这里，
    /// 由底栏状态栏显示（S1 之前它寄居在顶栏右侧，属"骨架未移植"的权宜位置）。
    pub(crate) status: Option<(String, f64)>,
    // ---- 文档 I/O ----
    /// 当前文件的绝对路径（`gui/filehandling.py` 的 `self.filename`）。
    ///
    /// `None` = 还没存过盘 → `Ctrl+S` 走「另存为」（照 `save_cb`）。
    pub(crate) current_file: Option<std::path::PathBuf>,
    /// 在 app 内的路径输入框（**技术栈适配**：GTK FileChooser 在 egui 里没有
    /// 对应物，改用内嵌的路径输入弹层；语义仍是"给个文件名再存/开"）。
    pub(crate) file_prompt: Option<FilePrompt>,
    // ---- 交互与渲染 ----
    /// 正在绘制时的引擎状态（每次起笔新建，落笔丢弃）。
    pub(crate) stroke_state: Option<BrushState>,
    /// 正在记录的作画命令（`gui/mode.py` 的 `__active_brushwork`）。
    pub(crate) brushwork: Option<Brushwork>,
    pub(crate) view: View,
    /// 画布在窗口里的矩形（egui 点）。上一帧 `canvas()` 记下的。
    ///
    /// 键盘缩放要有"围绕哪里缩放"的锚点，而 `handle_keys` 在画布绘制**之前**
    /// 跑，拿不到本帧的画布矩形 —— 这与源码同构：MyPaint 用的也是
    /// "最后一次指针事件的位置"（`document.py:1789`），本身就是上一帧的信息。
    pub(crate) canvas_rect: Option<egui::Rect>,
    pub(crate) renderer: Arc<Mutex<TileRenderer>>,
    /// 下一帧强制清空显存缓存并全量重传（诊断键 Ctrl+F1）。
    pub(crate) force_resync: bool,
    /// 本帧计划上传的 tile 数（命中上限说明还有欠账，需继续重绘）。
    pub(crate) pending_uploads: usize,
    pub(crate) stroke_dirty: bool,
    pub(crate) fullscreen: bool,
    // ---- 停靠面板系统（S6）----
    /// 工作区停靠状态（左/右/浮动组）。默认布局 = 左 BrushGroupTool、
    /// 右 BrushEditor（差异记账见 shell/dock.rs 模块文档）。
    pub(crate) dock: crate::shell::dock::DockState,
    // ---- 工具条（S4）----
    /// 图标纹理缓存（`shell::toolbars::IconCache`）。
    pub(crate) icon_cache: crate::shell::toolbars::IconCache,
    /// toolbar1 的 8 个片段开关（偏好 `ui.toolbar_items`，出厂默认见
    /// `userconfig.py:76-85`）。持久化属 B6。
    pub(crate) toolbar_fragments: [bool; 8],
    // ---- 图层面板（B2-5）----
    /// 缩略图缓存：layer id → (失效键, 纹理)。键变了才重渲染
    /// （键含表面 clock + 本层模式/不透明度，见
    /// [`crate::shell::layers_panel::ThumbKey`]）。
    pub(crate) layer_thumbs:
        std::collections::HashMap<u64, (crate::shell::layers_panel::ThumbKey, egui::TextureHandle)>,
    /// 正在行内重命名的层 id（双击名称进入；LayerProperties 对话框的
    /// 技术栈适配）。`rename_buf` 是编辑缓冲。
    pub(crate) layer_rename: Option<u64>,
    pub(crate) rename_buf: String,
    /// 行内重命名编辑框是否**已经拿到过焦点**（焦点要下一帧才到手，
    /// 首帧的 `has_focus() == false` 不是"失焦"）。
    pub(crate) rename_focused: bool,
    /// 正在重命名的层路径（B2-6：层可以嵌在组里，索引不够用）。
    pub(crate) rename_path: Vec<usize>,
    // ---- 图层树（B2-6）----
    /// 折叠的组（按空格不折叠：MyPaint 默认折叠，本实现默认展开）。
    pub(crate) layers_collapsed: std::collections::HashSet<Vec<usize>>,
    /// 正在拖动的层路径（MyPaint treeview 的 DnD 重排）。
    pub(crate) layer_drag: Option<Vec<usize>>,
    /// 当前落点（deepinsert 路径）。
    pub(crate) layer_drop_target: Option<Vec<usize>>,
}

// ---- 文档 I/O ----

/// 在 app 内的路径输入弹层。
///
/// **技术栈适配**：MyPaint 用 GTK 的 `Gtk.FileChooserDialog`
/// （`gui/filehandling.py:1268` 的 `save_as_dialog` / `open_cb` 的对话框）。
/// egui 没有自带文件对话框，本项目也不引入额外依赖，所以用一个内嵌的
/// 文本输入弹层代替 —— 交互语义（"先确定文件名，再执行"）与源码一致，
/// 只是控件形态不同。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum FileAction {
        /// `save_as_cb`（另存为）。`save_cb` 在"还没存过盘"时也走这里 ——
        /// 见 `PigmentApp::save_command`，所以不需要独立的 `Save` 变体。
        SaveAs,
        /// `open_cb`（打开）
        Open,
        /// `Export`（导出 PNG，不动 current_file）
        Export,
    }

    pub(crate) struct FilePrompt {
        pub(crate) action: FileAction,
        path: String,
        /// 上一次的错误（校验失败时显示，不关闭弹层）。
        error: Option<String>,
        /// 是否还没抢过键盘焦点。
        ///
        /// 只在第一帧抢：否则每帧 `request_focus()` 会把焦点一直锁在输入框上，
        /// 「确定 / 取消」按钮永远点不到。
        want_focus: bool,
    }

    impl FilePrompt {
        pub(crate) fn new(action: FileAction, seed: Option<&std::path::Path>) -> Self {
            let path = seed
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| default_document_path(action));
            Self {
                action,
                path,
                error: None,
                want_focus: true,
            }
        }

        fn title(&self) -> &'static str {
            match self.action {
                FileAction::SaveAs => "另存为（Save As）",
                FileAction::Open => "打开（Open）",
                FileAction::Export => "导出 PNG（Export）",
            }
        }
    }

/// 把文件名补成绝对路径（照 `os.path.abspath`，`filehandling.py:996`）。
fn abspath(p: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|d| d.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// 弹层里给的路径要按动作补扩展名。
///
/// MyPaint 靠文件对话框的过滤器自动补扩展名（`save_as_dialog` 的
/// `set_current_name`）；我们没有对话框，就按动作给个默认后缀。
fn with_default_extension(action: FileAction, p: &str) -> String {
    let ext = match action {
        FileAction::Export => "png",
        _ => "ora",
    };
    let path = std::path::Path::new(p);
    match path.extension() {
        Some(_) => p.to_string(),
        None => format!("{p}.{ext}"),
    }
}

/// 弹层的默认文件名（照 `save_as_dialog` 的 `suggested_filename`；
/// 没有当前文件时 MyPaint 用文档名 "Untitled"，我们用 "untitled"）。
fn default_document_path(action: FileAction) -> String {
    let ext = match action {
        FileAction::Export => "png",
        _ => "ora",
    };
    std::env::current_dir()
        .map(|d| d.join(format!("untitled.{ext}")).display().to_string())
        .unwrap_or_else(|_| format!("untitled.{ext}"))
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
            current_file: None,
            file_prompt: None,
            stroke_state: None,
            brushwork: None,
            view: View::default(),
            canvas_rect: None,
            renderer: Arc::new(Mutex::new(renderer)),
            force_resync: false,
            pending_uploads: 0,
            stroke_dirty: false,
            fullscreen: false,
            icon_cache: Default::default(),
            toolbar_fragments: crate::shell::toolbars::DEFAULT_FRAGMENTS,
            layer_thumbs: std::collections::HashMap::new(),
            layer_rename: None,
            rename_buf: String::new(),
            rename_focused: false,
            rename_path: Vec::new(),
            layers_collapsed: std::collections::HashSet::new(),
            layer_drag: None,
            layer_drop_target: None,
            dock: crate::shell::dock::DockState {
                left: vec![crate::shell::dock::Group {
                    panels: vec![crate::shell::dock::PanelId::BrushGroups],
                    active: 0,
                }],
                right: vec![crate::shell::dock::Group {
                    panels: vec![crate::shell::dock::PanelId::BrushEditor],
                    active: 0,
                }],
                ..Default::default()
            },
        };
        app.derive_active();
        app
    }

    /// 由 `base_brush` + 覆盖项推导出实际作画笔刷。
    ///
    /// 颜色只覆盖 `color_*` 的 base_value 而**保留笔刷自带的颜色动态**，
    /// 与 libmypaint 的 `mypaint_brush_set_base_value(COLOR_*)` 语义一致。
    pub(crate) fn derive_active(&mut self) {
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
    pub(crate) fn reload_selected_brush(&mut self) {
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
    pub(crate) fn paint_active(&self) -> bool {
        self.paint_override
            .unwrap_or_else(|| self.base_brush.get(BrushSetting::Paint).base_value >= 0.5)
    }

    /// 设置当前颜色并推给笔刷 —— 对齐 `BrushColorManager.set_color()`
    /// （`gui/brushcolor.py:44`：颜色由应用持有，再写给笔刷的 `color_h/s/v`）。
    pub(crate) fn set_color(&mut self, color: [f32; 3]) {
        self.color = color;
        self.derive_active();
    }

    pub(crate) fn set_status(&mut self, msg: String, now: f64) {
        self.status = Some((msg, now + STATUS_TTL));
    }

    /// `Ctrl+N` → `DocNew`：新建空白文档（`gui/filehandling.py:1088` 的
    /// `open_cb` 之前那段 `app.doc.model.clear()` 语义 —— 新文档把
    /// 内容、撤销栈、作画时长、当前文件名全部重置）。
    pub(crate) fn new_document(&mut self, now: f64) {
        self.flush_pending_stroke();
        self.doc.clear();
        self.doc.set_frame_enabled(false);
        self.doc.set_frame(0, 0, 0, 0);
        self.doc.xres = None;
        self.doc.yres = None;
        self.doc.settings.clear();
        self.current_file = None;
        self.force_resync = true; // 表面换了 → 显存全丢重传
        self.view = View::default();
        self.set_status("新建文档".to_string(), now);
    }

    /// 按扩展名分派保存并更新状态（`gui/filehandling.py:978` 的 `save_file`）。
    fn save_to_path(&mut self, path: &std::path::Path, export: bool, now: f64) {
        // 导出走 PNG 且不记进 current_file
        let alpha = if export { Some(false) } else { None };
        match self.doc.save(path, alpha) {
            Ok(_) => {
                if !export {
                    self.current_file = Some(path.to_path_buf());
                }
                let verb = if export { "已导出" } else { "已保存" };
                self.set_status(format!("{verb}：{}", path.display()), now);
            }
            Err(e) => self.set_status(format!("保存失败：{e}"), now),
        }
    }

    /// 打开文件（`gui/filehandling.py:890` 的 `open_file`）。
    fn open_from_path(&mut self, path: &std::path::Path, now: f64) {
        self.flush_pending_stroke();
        // 先记下旧状态：载入失败时不该把用户的画弄丢（源码会弹错并保留文档）。
        // B2-3 起文档是图层栈，整栈 COW 克隆（tile 共享 Arc，代价低）。
        let snapshot = self.doc.layers().clone();
        match self.doc.load(path) {
            Ok(outcome) => {
                self.current_file = Some(path.to_path_buf());
                self.force_resync = true;
                self.view = View::default();
                let msg = match outcome {
                    pigment_engine::LoadOutcome::Ora {
                        bbox,
                        layer_count,
                        mypaint_version,
                        compatibility,
                    } => {
                        let compat = match compatibility {
                            Some(pigment_engine::Compatibility::Partially) => {
                                "（该文件来自更新的 MyPaint，可能只部分兼容）"
                            }
                            Some(pigment_engine::Compatibility::Incompatible) => {
                                "（该文件来自不兼容的版本）"
                            }
                            _ => "",
                        };
                        let ver = mypaint_version.unwrap_or_else(|| "未知".to_string());
                        let layers = if layer_count == 1 {
                            "1 层".to_string()
                        } else {
                            format!("{layer_count} 层")
                        };
                        format!(
                            "已打开 {}×{}、{layers} 的 OpenRaster（mypaint {}）{compat}",
                            bbox.2, bbox.3, ver
                        )
                    }
                    pigment_engine::LoadOutcome::Png { bbox } => {
                        format!("已打开 {}×{} 的 PNG", bbox.2, bbox.3)
                    }
                };
                self.set_status(msg, now);
            }
            Err(e) => {
                // 回滚：`Document::load` 会先 clear()，所以失败时栈已经空了
                *self.doc.layers_mut() = snapshot;
                self.force_resync = true;
                self.set_status(format!("打开失败：{e}"), now);
            }
        }
    }

    /// 处理弹层提交。
    fn apply_file_prompt(&mut self, now: f64) {
        let Some(prompt) = self.file_prompt.take() else {
            return;
        };
        let raw = prompt.path.trim();
        if raw.is_empty() {
            self.file_prompt = Some(FilePrompt {
                error: Some("请填写文件路径".to_string()),
                ..prompt
            });
            return;
        }
        let action = prompt.action;
        let candidate = if action == FileAction::Open {
            raw.to_string()
        } else {
            with_default_extension(action, raw)
        };
        let path = abspath(&candidate);
        match action {
            FileAction::Open => {
                if !path.is_file() {
                    self.file_prompt = Some(FilePrompt {
                        error: Some(format!("文件不存在：{}", path.display())),
                        ..prompt
                    });
                    return;
                }
                self.open_from_path(&path, now);
            }
            FileAction::Export => self.save_to_path(&path, true, now),
            FileAction::SaveAs => {
                // 目录必须存在，否则给个明确提示（MyPaint 由对话框保证）
                if let Some(dir) = path.parent() {
                    if !dir.is_dir() {
                        self.file_prompt = Some(FilePrompt {
                            error: Some(format!("目录不存在：{}", dir.display())),
                            ..prompt
                        });
                        return;
                    }
                }
                self.save_to_path(&path, false, now);
            }
        }
    }

    /// `Ctrl+S`：有当前文件就直接存，否则走另存为（照 `save_cb`，`:1230`）。
    pub(crate) fn save_command(&mut self, now: f64) {
        match self.current_file.clone() {
            Some(p) => self.save_to_path(&p, false, now),
            None => {
                self.file_prompt = Some(FilePrompt::new(FileAction::SaveAs, None));
            }
        }
    }

    /// 画路径输入弹层。
    ///
    /// 用 `egui::Modal`（带遮罩、挡住背后一切输入）—— 正好对应 GTK 的模态
    /// 文件对话框，于是画布交互天然被屏蔽。
    fn show_file_prompt(&mut self, ctx: &egui::Context, now: f64) {
        let Some(mut prompt) = self.file_prompt.take() else {
            return;
        };
        // Enter 必须在画 TextEdit **之前**取：egui 的单行 TextEdit 会
        // `consume_key(Enter)`（"回车即失焦"就靠它），之后再查 `key_pressed`
        // 恒为 false —— 早先就是这么写的，结果回车提交不了。
        let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let mut press_ok = false;
        let mut cancel = false;
        // `Id::new` 在 egui 0.37 已废弃；`Id::unique(源)` 是它的等价替换
        // （同一个源每帧得到同一个 Id，正是模态需要的稳定标识）。
        egui::Modal::new(egui::Id::unique("pigment_file_prompt")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(prompt.title());
            ui.label(match prompt.action {
                FileAction::Open => "要打开的文件路径（.ora / .png）：",
                FileAction::Export => "导出 PNG 的路径：",
                FileAction::SaveAs => "保存路径（.ora / .png；不带后缀时按 .ora 处理）：",
            });
            let resp = ui.add(
                egui::TextEdit::singleline(&mut prompt.path)
                    .desired_width(540.0)
                    .hint_text(r"C://path//to//picture.ora"),
            );
            // 只在第一帧抢焦点：否则每帧 `request_focus()` 会把焦点锁在输入框上，
            // 「确定 / 取消」按钮永远点不到。
            if prompt.want_focus {
                resp.request_focus();
                prompt.want_focus = false;
            }
            if let Some(err) = &prompt.error {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("确定").clicked() {
                    press_ok = true;
                }
                if ui.button("取消").clicked() {
                    cancel = true;
                }
                ui.label(
                    egui::RichText::new("回车 = 确定，Esc = 取消")
                        .weak()
                        .small(),
                );
            });
        });
        if cancel {
            self.set_status("已取消".to_string(), now);
            return;
        }
        self.file_prompt = Some(prompt);
        if enter || press_ok {
            self.apply_file_prompt(now);
        }
    }

    // ---- 笔画生命周期 ----

    /// 组装一个笔画事件。
    ///
    /// 参数表与 `gui/freehand.py:601-611` 一致；压力/倾角的钳位对齐它在
    /// `stroke_to` 之前的 `clamp()`。我们还没有倾角、视图旋转与笔杆旋转
    /// 输入（数位板属 M6），这三项填 0。
    pub(crate) fn stroke_event(&self, x: f32, y: f32, pressure: f32, dtime: f64) -> StrokeEvent {
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
    ///
    /// 当前层不可画（组 / 锁 / 祖先隐藏）时**什么都不画**——照
    /// `Brushwork._check_recording_started` 对非可画层 `_stroke_target_layer =
    /// None`（`command.py:403-408`）的语义，事件流也不记录。
    pub(crate) fn paint_event(&mut self, ev: StrokeEvent) {
        let (Some(mut bw), Some(mut st)) = (self.brushwork.take(), self.stroke_state.take()) else {
            return;
        };
        match self.doc.paintable_surface_mut() {
            Some(surface) => {
                bw.stroke_to(surface, &self.brush, &self.brush_name, &mut st, ev);
            }
            None => {
                // 不可画：整段作画作废（MyPaint 同样不产生命令）
                self.brushwork = None;
                self.stroke_state = None;
                return;
            }
        }
        self.brushwork = Some(bw);
        self.stroke_state = Some(st);
    }

    /// 落笔：收尾并把这段作画提交给撤销栈
    /// （`gui/mode.py:766-768` 的 `brushwork_commit`）。
    pub(crate) fn commit_stroke(&mut self) {
        let (Some(mut bw), Some(st)) = (self.brushwork.take(), self.stroke_state.take()) else {
            self.brushwork = None;
            self.stroke_state = None;
            return;
        };
        // 没画到东西的命令直接丢弃（`if changed: model.do(cmd)`）
        let Some(surface) = self.doc.paintable_surface_mut() else {
            return;
        };
        if bw.stop_recording(surface, &st, false) {
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
    pub(crate) fn undo(&mut self, now: f64) {
        self.flush_pending_stroke();
        match self.doc.undo() {
            // 文案对齐 `gui/document.py:192` 的 `UNDO_CMD = _("Undo %s")`
            Some(name) => self.set_status(format!("撤销：{name}"), now),
            None => self.set_status("没有可撤销的操作".to_string(), now),
        }
    }

    /// 重做（`gui/document.py:558` 的 `redo_cb`）。
    pub(crate) fn redo(&mut self, now: f64) {
        self.flush_pending_stroke();
        match self.doc.redo() {
            Some(name) => self.set_status(format!("重做：{name}"), now),
            None => self.set_status("没有可重做的操作".to_string(), now),
        }
    }

    /// 笔刷编辑器的 Live Update：用当前笔刷重绘上一笔
    /// （`gui/brusheditor.py:1135` 的 `_live_update_idle_cb`）。
    pub(crate) fn apply_live_update(&mut self) {
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
    pub(crate) fn restore_brushkey(&mut self, i: usize, now: f64) {
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
    pub(crate) fn store_brushkey(&mut self, i: usize, now: f64) {
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

    /// 按键分发 —— 全部键位判断都交给 [`crate::shell::actions`] 的表。
    ///
    /// 结构对应 MyPaint 的 `KeyboardManager._key_press_cb`（`gui/keyboard.py:93-157`）：
    /// 1. 输入框有焦点时让位 —— 源码靠 `propagate_key_event()`（`keyboard.py:97`），
    ///    egui 的 `key_pressed()` 不看焦点，所以这里显式早退。
    /// 2. 按**事件流顺序**取本帧的按下事件。用事件而不是 `key_pressed()`：
    ///    事件自带按下瞬间的修饰键快照，也不用为每个绑定再问一次输入状态。
    /// 3. 修饰键**精确匹配**（源码是元组查表，不是"包含"）。
    /// 4. 命中但**未实现**的动作不消费按键（返回 false），因此不会挡住后面的绑定。
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.text_edit_focused() {
            return;
        }
        let events: Vec<(egui::Key, actions::Mods)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, actions::Mods::from_egui(modifiers))),
                    _ => None,
                })
                .collect()
        });
        if events.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time);
        for (key, mods) in events {
            self.dispatch_key(ctx, key, mods, now);
        }
    }

    /// 一个按键事件 → 一个动作。
    fn dispatch_key(&mut self, ctx: &egui::Context, key: egui::Key, mods: actions::Mods, now: f64) {
        // 应用本地扩展键优先（它们占的是源码没碰过的键位，见 `actions::APP_KEYS`）
        if let Some(k) = actions::lookup_app(key, mods) {
            if self.run_app_key(k.id, now) {
                return;
            }
        }
        match actions::lookup(key, mods) {
            Some(actions::Bound::Action(id)) | Some(actions::Bound::ExtraAction(id)) => {
                self.run_action(id, ctx, now);
            }
            Some(actions::Bound::Behavior(b)) => {
                self.run_behavior(b, ctx);
            }
            None => {}
        }
    }

    /// 工具条按钮的**按下态**（toggle 语义的动作，对应 GtkToggleAction
    /// 的 active）。未列出的动作（含全部未实现的）不亮。
    pub(crate) fn toolbar_pressed(&self, id: &str) -> bool {
        match id {
            "BlendModeEraser" => self.eraser,
            "Fullscreen" => self.fullscreen,
            // 面板开关（toggle_dockpanel_cb 的动作 active 状态 = 已添加）
            "ToggleModeOptionsTool" => {
                self.dock.is_added(crate::shell::dock::PanelId::ModeOptionsTool)
            }
            "ToggleLayersTool" => self.dock.is_added(crate::shell::dock::PanelId::LayersTool),
            "ToggleScratchpadTool" => {
                self.dock.is_added(crate::shell::dock::PanelId::ScratchpadTool)
            }
            "TogglePreviewTool" => self.dock.is_added(crate::shell::dock::PanelId::PreviewTool),
            "ToggleHistoryPanel" => {
                self.dock.is_added(crate::shell::dock::PanelId::HistoryPanel)
            }
            "BrushGroups" => self.dock.is_added(crate::shell::dock::PanelId::BrushGroups),
            _ => false,
        }
    }

    /// 菜单/工具条条目是否可用（决定置灰，S3/S4 共用）。
    ///
    /// 与 [`PigmentApp::run_action`] 的 match 臂**一一对应**：
    /// 这里 true 的 id，`run_action` 必定返回 true。有状态的两条
    /// （Undo/Redo 的 sensitivity）照搬 `document.py:582-591` 的
    /// `_update_undo_redo`：栈里有没有命令。
    pub(crate) fn action_available(&self, id: &str) -> bool {
        match id {
            // ---- 文档 I/O ----
            "New" | "Open" | "Save" | "SaveAs" | "Export" => true,
            // ---- 撤销栈（动态 sensitivity）----
            "Undo" => self.doc.command_stack.undo_len() > 0,
            "Redo" => self.doc.command_stack.redo_len() > 0,
            // ---- 笔刷 ----
            "Bigger" | "Smaller" | "BlendModeNormal" | "BlendModeEraser" => true,
            // ---- 视图 ----
            "ZoomIn" | "ZoomOut" | "ZoomInCentered" | "ZoomOutCentered" => true,
            "PanLeft" | "PanRight" | "PanUp" | "PanDown" => true,
            // ---- 停靠面板（S6）----
            "ToggleModeOptionsTool" | "ToggleLayersTool" | "ToggleScratchpadTool"
            | "TogglePreviewTool" | "ToggleHistoryPanel" | "BrushGroups" => true,
            // ---- 图层（B2-5/B2-6；动态 sensitivity 照源码 enable 语义）----
            "NewPaintingLayerAbove" | "NewLayerGroupAbove" | "DuplicateLayer" | "RemoveLayer" => {
                true
            }
            "RaiseLayerInStack" | "LowerLayerInStack" => {
                // 同层内的兄弟间换位（`_bubble_layer` 的跨组分支未移植）
                let ls = self.doc.layers();
                let path = ls.current_path().to_vec();
                let (idx, parent) = match path.split_last() {
                    Some((last, rest)) => (*last, rest.to_vec()),
                    None => return false,
                };
                let count = if parent.is_empty() {
                    ls.len()
                } else {
                    ls.layer_at(&parent).map(|l| l.child_count()).unwrap_or(0)
                };
                if id == "RaiseLayerInStack" {
                    idx > 0
                } else {
                    idx + 1 < count
                }
            }
            "MergeLayerDown" => {
                // `get_merge_down_target` 的判据（tree.py:2003）：下一个兄弟存在、
                // 未锁、可归一化（`normalizable` 与命令共用）
                let ls = self.doc.layers();
                let path = ls.current_path().to_vec();
                let Some((idx, parent)) = path.split_last() else {
                    return false;
                };
                let mut target = parent.to_vec();
                target.push(idx + 1);
                match (ls.layer_at(&path), ls.layer_at(&target)) {
                    (Some(upper), Some(lower)) => {
                        !ls.branch_locked(&path)
                            && !ls.branch_locked(&target)
                            && pigment_engine::command::normalizable(upper)
                            && pigment_engine::command::normalizable(lower)
                    }
                    _ => false,
                }
            }
            "ShowBackgroundToggle" | "BackgroundWindow" => true,
            // ---- 窗口 ----
            "Fullscreen" | "Quit" => true,
            // ---- 笔刷槽位（`ContextRestoreColor` 与 20 个动态槽位动作）----
            "ContextRestoreColor" => true,
            _ => actions::parse_context_action(id).is_some(),
        }
    }

    /// 可勾选条目（`GtkToggleAction`）的当前状态；`None` = 不画复选槽。
    ///
    /// 只为**已实现**的 toggle 返回值。源码里 Normal/Eraser/LockAlpha/Colorize
    /// 是一组互斥的 toggle（`gui/brushmodifier.py`），本应用目前只区分
    /// "普通 / 橡皮"两种（差异记账在 B2）。
    pub(crate) fn toggle_state(&self, id: &str) -> Option<bool> {
        match id {
            "ShowBackgroundToggle" => Some(self.doc.layers().background_visible()),
            "Fullscreen" => Some(self.fullscreen),
            "BlendModeNormal" => Some(!self.eraser),
            "BlendModeEraser" => Some(self.eraser),
            _ => None,
        }
    }

    /// 执行一个 MyPaint 动作（动作名来自 `resources.xml` 或源码里的额外键）。
    ///
    /// **没实现的动作返回 `false`** —— 这是"差异记账、不阻塞推进"的落点：
    /// S3 菜单与 S4 工具条要按这个返回值把没实现的项置灰。
    pub(crate) fn run_action(&mut self, id: &str, ctx: &egui::Context, now: f64) -> bool {
        match id {
            // ---- 文档 I/O（`gui/filehandling.py` 的 *_cb）----
            "New" => {
                self.new_document(now);
                ctx.request_repaint();
            }
            "Open" => self.file_prompt = Some(FilePrompt::new(FileAction::Open, None)),
            "Save" => {
                self.save_command(now);
                ctx.request_repaint();
            }
            "SaveAs" => {
                self.file_prompt = Some(FilePrompt::new(
                    FileAction::SaveAs,
                    self.current_file.as_deref(),
                ));
            }
            "Export" => {
                // 建议名 = 当前文件名换 .png（照 `save_as_dialog` 的 suggested_filename）
                let seed = self
                    .current_file
                    .as_ref()
                    .map(|p| p.with_extension("png"));
                self.file_prompt = Some(FilePrompt::new(FileAction::Export, seed.as_deref()));
            }
            // ---- 撤销栈（`gui/document.py:552-558`）----
            "Undo" => {
                self.undo(now);
                ctx.request_repaint();
            }
            "Redo" => {
                self.redo(now);
                ctx.request_repaint();
            }
            // ---- 笔刷尺寸（`brush_bigger_cb` / `brush_smaller_cb`）----
            "Bigger" | "Smaller" => {
                let step = if id == "Bigger" { 0.15 } else { -0.15 };
                let cur = self.brush.get(BrushSetting::Radius).base_value;
                self.radius_override = Some((cur + step).clamp(-1.0, 6.0));
                self.derive_active();
            }
            // ---- 笔刷混色模式（`gui/brushmodifier.py`）----
            // 源码里 Normal/Eraser/LockAlpha/Colorize 是一组互斥的 GtkToggleAction，
            // 本应用目前只区分"普通 / 橡皮"两种。
            "BlendModeNormal" => {
                self.eraser = false;
                self.derive_active();
            }
            "BlendModeEraser" => {
                self.eraser = true;
                self.derive_active();
            }
            // ---- 视图：缩放（`gui/document.py:1776-1816` 的 zoom()）----
            // `ZoomIn/Out` 以**指针**为中心（CENTER_ON_POINTER），带 Centered 后缀的
            // 那几个是工具条上的版本，以**视口中心**为中心（CENTER_ON_VIEWPORT）。
            "ZoomIn" | "ZoomOut" | "ZoomInCentered" | "ZoomOutCentered" => {
                let zoom_in = id.starts_with("ZoomIn");
                let viewport_centered = id.ends_with("Centered");
                let anchor = self
                    .canvas_anchor(ctx, !viewport_centered)
                    .unwrap_or([0.0, 0.0]);
                let z = self.view.stepped_zoom(zoom_in);
                self.view.set_zoom_at(z, anchor[0], anchor[1]);
                ctx.request_repaint();
            }
            // ---- 视图：平移（`gui/document.py:1756-1774`）----
            // 步长 = min(视口宽, 视口高) * PAN_STEP，方向与 `tdw.scroll()` 同号
            "PanLeft" | "PanRight" | "PanUp" | "PanDown" => {
                let step = self.pan_step();
                match id {
                    "PanLeft" => self.view.pan(step, 0.0),
                    "PanRight" => self.view.pan(-step, 0.0),
                    "PanUp" => self.view.pan(0.0, step),
                    _ => self.view.pan(0.0, -step),
                }
                ctx.request_repaint();
            }
            // ---- 图层（`layerswindow` 的动作回调；B2-5）----
            "NewPaintingLayerAbove" => {
                self.doc.add_layer();
                ctx.request_repaint();
            }
            // `NewLayerGroupAbove` → `AddLayer(layer_class=LayerStack)`（layerswindow）
            "NewLayerGroupAbove" => {
                self.doc.add_group();
                ctx.request_repaint();
            }
            "RemoveLayer" => {
                self.doc.remove_layer();
                ctx.request_repaint();
            }
            "RaiseLayerInStack" => {
                self.doc.bubble_layer_up();
                ctx.request_repaint();
            }
            "LowerLayerInStack" => {
                self.doc.bubble_layer_down();
                ctx.request_repaint();
            }
            "DuplicateLayer" => {
                self.doc.duplicate_layer();
                ctx.request_repaint();
            }
            "MergeLayerDown" => {
                if !self.doc.merge_layer_down() {
                    // 源码 `_merge_layer_down_cb` 失败时弹 info bar（document.py）
                    self.set_status("无法向下合并：两层都须可见、下层须未锁".to_string(), now);
                }
                ctx.request_repaint();
            }
            "ShowBackgroundToggle" => {
                let v = !self.doc.layers().background_visible();
                self.doc.layers_mut().set_background_visible(v);
                ctx.request_repaint();
            }
            "BackgroundWindow" => {
                // 技术栈适配：背景色对话框未移植，指向面板里的取色钮
                self.set_status("背景色请用图层面板底部的取色钮（技术栈适配）".to_string(), now);
            }
            // ---- 停靠面板（`toggle_dockpanel_cb`，drawwindow.py:352-367）----
            // Toggle*Tool → MyPaint<GType 名>（去 Toggle 前缀）；BrushGroups 是
            // 工厂动作（MyPaintFactoryAction，reveal BrushGroupTool，drawwindow.py:340-353）
            "ToggleModeOptionsTool" => self.dock.toggle(crate::shell::dock::PanelId::ModeOptionsTool),
            "ToggleLayersTool" => self.dock.toggle(crate::shell::dock::PanelId::LayersTool),
            "ToggleScratchpadTool" => self.dock.toggle(crate::shell::dock::PanelId::ScratchpadTool),
            "TogglePreviewTool" => self.dock.toggle(crate::shell::dock::PanelId::PreviewTool),
            "ToggleHistoryPanel" => self.dock.toggle(crate::shell::dock::PanelId::HistoryPanel),
            "BrushGroups" => self.dock.toggle(crate::shell::dock::PanelId::BrushGroups),
            // ---- 窗口 ----
            "Fullscreen" => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            "Quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            // ---- 笔刷快捷键是否连颜色一起还原（`ContextRestoreColor`，按键 `）----
            "ContextRestoreColor" => {
                self.context_restores_color = !self.context_restores_color;
                let state = if self.context_restores_color { "开" } else { "关" };
                self.set_status(format!("恢复笔刷槽位时连颜色一起还原：{state}"), now);
            }
            // ---- 笔刷槽位（`gui/document.py:467-492` 动态注册的 Context0N/Context0Ns）----
            _ => {
                if let Some((slot, store)) = actions::parse_context_action(id) {
                    if store {
                        self.store_brushkey(slot, now);
                    } else {
                        self.restore_brushkey(slot, now);
                    }
                } else {
                    return false;
                }
            }
        }
        true
    }

    /// 执行源码里以 lambda 注册的硬编码行为（不是动作）。
    fn run_behavior(&mut self, behavior: &str, ctx: &egui::Context) -> bool {
        match behavior {
            // `document.py:530`：`k("<control>w", lambda action: ...quit_cb())`
            "quit" => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                true
            }
            // `pan-left/right/up/down`：源码里的 lambda 版光标键平移。
            // 实际到不了这里 —— `resources.xml` 的 PanLeft/PanRight/PanUp/PanDown
            // 占着同样的键且优先级更高（`keyboard.py:139-142`），此处保留以如实反映源码。
            _ => false,
        }
    }

    /// 执行一条**应用本地扩展键**（`actions::APP_KEYS`），返回是否处理了。
    fn run_app_key(&mut self, id: &str, now: f64) -> bool {
        match id {
            "pigment:spectral" => {
                self.paint_override = Some(!self.paint_active());
                self.derive_active();
                let state = if self.paint_active() { "开" } else { "关" };
                self.set_status(format!("光谱混色：{state}"), now);
            }
            "pigment:resync-gpu" => {
                // 丢弃全部显存 tile 并重传。若画面恢复正常 → 属显存同步问题
                self.force_resync = true;
            }
            "pigment:dump-canvas" => {
                // 导出 CPU 侧画布为 PNG，与屏幕对照可区分「CPU 无内容」与「显存没同步」
                // 组没有表面 → 只导出当前可画层
                match self.doc.paintable_surface() {
                    Some(surface) => match crate::dump::dump_surface_png(surface) {
                        Ok(p) => println!("已导出 CPU 画布快照: {}", p.display()),
                        Err(e) => eprintln!("导出失败: {e}"),
                    },
                    None => self.set_status("当前层是组，没有可导出的表面".to_string(), now),
                }
            }
            "pigment:prev-brush" | "pigment:next-brush" => {
                let next = id == "pigment:next-brush";
                if self.brushes.step(if next { 1 } else { -1 }) {
                    self.reload_selected_brush();
                }
            }
            _ => return false,
        }
        true
    }

    /// 画布内的锚点（画布局部坐标，单位 = egui 点），用于键盘缩放。
    ///
    /// 对应源码的 `Document.get_last_event_info(tdw)`（`document.py:1789`）：
    /// 取"最后一次指针事件的位置"。这里用**当前**指针位置，且只认它落在画布内
    /// 的情况 —— 落在画布外（或指针不在窗口里）就退回视口中心，避免把视点甩飞。
    fn canvas_anchor(&self, ctx: &egui::Context, prefer_pointer: bool) -> Option<[f32; 2]> {
        let rect = self.canvas_rect?;
        let p = if prefer_pointer {
            ctx.input(|i| i.pointer.latest_pos())
                .filter(|p| rect.contains(*p))
        } else {
            None
        };
        let p = p.unwrap_or_else(|| rect.center());
        let local = p - rect.min;
        Some([local.x, local.y])
    }

    /// 光标键平移的步长 —— `document.py:1763`：
    /// `step = min(视口宽, 视口高) * PAN_STEP`。
    fn pan_step(&self) -> f32 {
        let r = self.canvas_rect.unwrap_or(egui::Rect::NOTHING);
        r.width().min(r.height()) * crate::view::PAN_STEP
    }
}

impl eframe::App for PigmentApp {
    /// egui 0.37 的 App 入口是 Ui 形态：`ui` 覆盖整个 `content_rect`。
    ///
    /// 面板的声明顺序 = 源码的三段结构（`gui/mypaint.glade`）：
    /// 顶栏切走上边、底栏切走下边，剩下的矩形归工作区（内部再切左右侧栏，
    /// 中央留给画布）。**顺序不能换** —— egui 的面板是从当前剩余矩形上"压边"
    /// 出来的，先声明的先占。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);

        // ── 顶栏 ── `MyPaintTopBar`：菜单栏 + toolbar1 + toolbar2
        self.topbar(ui);
        // ── 底栏 ── `footer_box`：12 个部件
        self.footer(ui);
        // ── 工作区 ── `MyPaintWorkspace`：侧栏 + 中央画布
        self.workspace(ui);

        // 文件路径弹层放最后画（浮在最上层）。它打开时输入框会拿到键盘焦点，
        // 于是 `handle_keys` 的 `text_edit_focused()` 早退会顺带挡住所有快捷键。
        let now = ui.input(|i| i.time);
        self.show_file_prompt(&ctx, now);
        if self.file_prompt.is_some() {
            ctx.request_repaint();
        }
    }
}
