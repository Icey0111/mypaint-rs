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
//! - 笔刷不透明度 `s` / `a`（`MoreOpaque`/`LessOpaque`，`resources.xml:292,300`）
//! - 颜色调整 `Shift+F`/`Shift+D`（明度）、`Shift+W`/`Shift+Q`（色相）、
//!   `Shift+S`/`Shift+A`（饱和度）—— `resources.xml:309-354` 的六个
//!   Menu→Color→Adjust Color 动作，B4-1 接到引擎的颜色管理器上
//! - 橡皮 `e`、普通模式 `n`（`BlendModeEraser`/`BlendModeNormal`）
//! - 取色 `r`；`Ctrl+r` / `Shift+r` / `Alt+r` 分别只取色相 / 彩度 / 亮度
//!   （`FlipColorPickMode*`，`resources.xml:1896-1930`）
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
use pigment_engine::color::{BrushStep, Color, ColorAction, ColorManager};
use pigment_engine::hokusai::{Brush, BrushSetting, BrushState};
use pigment_engine::{CommandUpdate, Document};

use crate::brushes::BrushPanel;
use crate::color;
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
    /// 当前色 + 颜色历史 + `ContextRestoreColor` 开关 —— **颜色的唯一事实来源**。
    ///
    /// 对应 `gui/brushcolor.py` 的 `BrushColorManager`：颜色由应用持有，笔刷的
    /// `color_h/s/v` 只是它的投影（[`PigmentApp::derive_active`] 负责投影）。
    /// 类型、比较、历史去重、调整动作的语义都在引擎
    /// （`pigment_engine::color`，逐行对照 `lib/color.py` /
    /// `gui/colors/adjbases.py` / `gui/brushcolor.py`）。
    pub(crate) color_manager: ColorManager,
    // ---- 笔刷快捷键 ----
    /// 10 个槽位（`BrushManager.contexts`）。
    pub(crate) brushkeys: [Option<BrushKeySlot>; NUM_BRUSHKEYS],
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
    /// 笔刷引擎的跨事件状态（`hokusai::BrushState`）：每次起笔重置
    /// （源码 `brush.reset()` 的等价物）。
    pub(crate) stroke_state: BrushState,
    /// 交互模式栈（`gui/mode.py:1327` 的 `ModeStack`；B3-1 起接管作画路径）。
    pub(crate) modes: pigment_engine::mode::ModeStack,
    /// 指针时间戳的单调基准（毫秒）。egui 的输入时间没有 MyPaint 那样的
    /// 事件时间戳，这里用"应用启动以来的毫秒"并把倒退夹住
    /// （`DrawingState::queue_motion` 里还有一层夹逼）。
    pub(crate) pending_pointer_time: f64,
    /// 模式层的可配置项（假压力/假旋转/桶旋转/隐藏光标；
    /// `gui/userconfig.py` 的默认值，B6 起接偏好系统）。
    pub(crate) mode_prefs: ModePrefs,
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

/// 模式层的可配置项（`gui/userconfig.py` + `app.fakepressure/fakerotation`）。
///
/// B6 引入偏好系统前先放这几个默认值（源码默认值见 `userconfig.py:61-67`
/// 与 `application.py:299-300`）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModePrefs {
    /// 鼠标/无压感设备的假压力（默认 0.5）。
    pub fakepressure: f32,
    /// 假桶旋转（默认 0.5）。
    pub fakerotation: f32,
    /// 画的时候隐藏系统光标（默认 false）。
    pub hide_cursor_while_painting: bool,
}

impl Default for ModePrefs {
    fn default() -> Self {
        Self {
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
        }
    }
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

        let color_manager = ColorManager::new();
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
            color_manager,
            brushkeys,
            live_update: false,
            status: None,
            current_file: None,
            file_prompt: None,
            stroke_state: BrushState::default(),
            modes: pigment_engine::mode::ModeStack::new(|| {
                Box::new(pigment_engine::mode::freehand::FreehandMode::new())
            }),
            pending_pointer_time: 0.0,
            mode_prefs: ModePrefs::default(),
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
        let [h, s, v] = self.color_hsv();
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

    /// 当前色的 HSV —— 笔刷通道与 egui 控件都吃 `[f32; 3]`。
    ///
    /// 等价于源码的 `app.brush.get_color_hsv()`：真正的颜色存在
    /// [`PigmentApp::color_manager`] 里（f64），这里只是投影到 f32 通道
    /// （libmypaint 的 brush setting 本身就是 C `float`）。
    pub(crate) fn color_hsv(&self) -> [f32; 3] {
        color::hsv_of(&self.color_manager.get_color())
    }

    /// 设置当前颜色并推给笔刷 —— 对齐 `BrushColorManager.set_color()`
    /// （`gui/brushcolor.py:33-37`：颜色由应用持有，再写给笔刷的
    /// `color_h/s/v`）。
    pub(crate) fn set_color(&mut self, color: Color) {
        self.color_manager.set_color(color);
        self.derive_active();
    }

    /// 用 HSV 三元组设置当前色 —— 笔刷通道、取色器、滑块都从这条进来。
    ///
    /// 落成 `HSVColor` 与源码一致：`BrushColorManager.__settings_changed_cb`
    /// 读到 `color_h/s/v` 变化后就是 `HSVColor(*brush.get_color_hsv())`
    /// （`gui/brushcolor.py:43`）。
    pub(crate) fn set_color_hsv(&mut self, hsv: [f32; 3]) {
        self.set_color(color::hsv_color(hsv));
    }

    pub(crate) fn set_status(&mut self, msg: String, now: f64) {
        self.status = Some((msg, now + STATUS_TTL));
    }

    /// `Ctrl+N` → `DocNew`：新建空白文档（`gui/filehandling.py:1088` 的
    /// `open_cb` 之前那段 `app.doc.model.clear()` 语义 —— 新文档把
    /// 内容、撤销栈、作画时长、当前文件名全部重置）。
    pub(crate) fn new_document(&mut self, now: f64) {
        self.flush_pending_stroke();
        // `Document::clear` 已经把画框/分辨率/文档设置/对称状态一并复位
        // （`lib/document.py:944-961`），这里不再重复设一遍。
        self.doc.clear();
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

    /// 当前修饰键（`mode.py:403` 的 `current_modifiers`）。
    pub(crate) fn pointer_modifiers(&self, ctx: &egui::Context) -> pigment_engine::mode::Modifiers {
        ctx.input(|i| pigment_engine::mode::Modifiers {
            shift: i.modifiers.shift,
            ctrl: i.modifiers.command,
            alt: i.modifiers.alt,
        })
    }

    /// 把一个 egui 指针位置投影成模式层事件（模型坐标 + 毫秒时间戳）。
    ///
    /// 压力/倾角/桶旋转都是 `None`：本应用目前只接鼠标（数位板输入属 M6），
    /// 自由手绘模式会给它们补假压力（`freehand.py:433-442`）。
    pub(crate) fn pointer_event(
        &self,
        pos: egui::Pos2,
        rect: egui::Rect,
        time_ms: f64,
        button: u32,
        modifiers: pigment_engine::mode::Modifiers,
    ) -> pigment_engine::mode::PointerEvent {
        let local = pos - rect.min;
        let [x, y] = self.view.screen_to_world(local.x, local.y);
        pigment_engine::mode::PointerEvent {
            time_ms,
            x,
            y,
            pressure: None,
            xtilt: None,
            ytilt: None,
            viewzoom: self.view.zoom,
            viewrotation: 0.0,
            barrel_rotation: None,
            button,
            modifiers,
        }
    }

    /// 把一个指针事件交给当前交互模式（`gui/mode.py` 的 `button_press_cb` 等）。
    ///
    /// B3-1：作画路径不再由 GUI 手写，而是走 [`ModeStack`] —— 栈顶模式
    /// （默认 [`FreehandMode`]）负责排队、插值、`brushwork` 的原子提交。
    /// 当前层不可画时模式自己不画（`get_paintable`），所以这里无需特判。
    pub(crate) fn mode_button_press(&mut self, ev: pigment_engine::mode::PointerEvent) -> bool {
        self.pending_pointer_time = self.pending_pointer_time.max(ev.time_ms);
        let mut handled = false;
        self.with_mode(|modes, ctx| {
            handled = modes.top_mut().button_press(ctx, &ev);
        });
        self.mode_process_pending();
        handled
    }

    /// 指针移动（`motion_notify_cb`）。
    pub(crate) fn mode_motion(&mut self, ev: pigment_engine::mode::PointerEvent) -> bool {
        self.pending_pointer_time = self.pending_pointer_time.max(ev.time_ms);
        let mut handled = false;
        self.with_mode(|modes, ctx| {
            handled = modes.top_mut().motion(ctx, &ev);
        });
        self.mode_process_pending();
        handled
    }

    /// 指针松开（`button_release_cb`）。
    pub(crate) fn mode_button_release(&mut self, ev: pigment_engine::mode::PointerEvent) -> bool {
        self.pending_pointer_time = self.pending_pointer_time.max(ev.time_ms);
        let mut handled = false;
        self.with_mode(|modes, ctx| {
            handled = modes.top_mut().button_release(ctx, &ev);
        });
        self.mode_process_pending();
        handled
    }

    /// 处理模式排队的输入（`GLib.idle_add` 的等价物：每帧一次）。
    pub(crate) fn mode_process_pending(&mut self) {
        self.with_mode(|modes, ctx| modes.process_pending(ctx));
    }

    /// 把在途作画落账（撤销/重做前、切笔刷前的清账；
    /// 对应 `Document.sync_pending_changes()` → `mode.checkpoint()`）。
    fn flush_pending_stroke(&mut self) {
        self.with_mode(|modes, ctx| modes.checkpoint(ctx));
    }

    /// 切换当前工具模式（`gui/document.py` 的 `mode_flip_action_activated_cb`
    /// 与 Edit→Current Mode 那组无线电动作）。
    ///
    /// 记账：源码用 `context_push`（叠在兼容模式之上）或 `pop`，本移植直接
    /// `reset` —— 我们还没有需要"叠在下面"的临时视图模式（平移/缩放仍由 GUI
    /// 直接处理），reset 的观感一致。
    pub(crate) fn switch_mode(&mut self, action: &str, now: f64) {
        use pigment_engine::mode::fill::FillMode;
        use pigment_engine::mode::freehand::FreehandMode;
        use pigment_engine::mode::line::{LineKind, LineMode};
        let new_mode: Box<dyn pigment_engine::mode::InteractionMode> = match action {
            "FreehandMode" => Box::new(FreehandMode::new()),
            "StraightMode" => Box::new(LineMode::new(LineKind::Straight)),
            "SequenceMode" => Box::new(LineMode::new(LineKind::Sequence)),
            "EllipseMode" => Box::new(LineMode::new(LineKind::Ellipse)),
            // B3-3：填充（`gui/fill.py:40` 的 FloodFillMode）
            "FloodFillMode" => Box::new(FillMode::new()),
            _ => return,
        };
        let label = new_mode.name().to_string();
        let usage = new_mode.usage().to_string();
        self.with_mode(|modes, ctx| modes.reset(Some(new_mode), ctx));
        self.set_status(format!("{label}: {usage}"), now);
    }

    /// 切到取色模式（`ColorPickMode` 与 `H`/`C`/`Y` 三个变体）。
    ///
    /// 两条入口的构造参数不同，照 `document.py:2163-2201`：
    ///
    /// - **单选动作/菜单**（工具条、Edit→Current Mode）：用类默认值
    ///   （`ignore_modifiers=False, temporary_activation=True`），
    ///   **不立即取色** —— 得先在画布上按一下（`colorpicker.py:34-39`）；
    /// - **`Flip*` 动作**（键盘）：`ignore_modifiers=True,
    ///   temporary_activation=False`（`document.py:2169-2173`），
    ///   `enter()` 时就在 `enter_pick`（最后一次指针事件的位置）取一次色。
    ///
    /// 压栈方式与 [`PigmentApp::switch_mode`] 一样用 `reset`：源码的
    /// `context_push` 会保留下面的模式（"用完回到原来那个"），但我们还没有
    /// 需要叠放的临时视图模式，因此按 reset 处理（既有记账）。
    pub(crate) fn switch_pick_mode(
        &mut self,
        variant: pigment_engine::mode::pick::PickVariant,
        keyboard: bool,
        enter_pick: Option<(f32, f32)>,
        now: f64,
    ) {
        use pigment_engine::mode::pick::ColorPickMode;
        let new_mode: Box<dyn pigment_engine::mode::InteractionMode> = if keyboard {
            Box::new(ColorPickMode::flip(variant, enter_pick))
        } else {
            Box::new(ColorPickMode::new(variant))
        };
        let label = new_mode.name().to_string();
        let usage = new_mode.usage().to_string();
        self.with_mode(|modes, ctx| modes.reset(Some(new_mode), ctx));
        self.set_status(format!("{label}: {usage}"), now);
    }

    /// "最后一次指针事件的位置"，模型坐标
    /// （`Document.get_last_event_info`，`gui/document.py:145`）。
    ///
    /// 键盘起手的取色模式要靠它在**不按键**的情况下立刻取一次色
    /// （`colorpicker.py:84-90`）。指针不在画布上（或还没有指针位置）时返回
    /// `None`，与源码"`x`/`y` 为 `None` 就不取"一致。
    fn pointer_model_pos(&self, ctx: &egui::Context) -> Option<(f32, f32)> {
        let rect = self.canvas_rect?;
        let pos = ctx.input(|i| i.pointer.latest_pos())?;
        if !rect.contains(pos) {
            return None;
        }
        let local = pos - rect.min;
        let [x, y] = self.view.screen_to_world(local.x, local.y);
        Some((x, y))
    }

    /// 视口中心的模型坐标（整数），用于第一次开启对称时定中心。
    ///
    /// 对应 `symmetry_active_toggled_cb` 里的
    /// `tdw.display_to_model(alloc.width/2, alloc.height/2)`（`gui/document.py:1899-1901`），
    /// 取整规则照 `symmetry.py:226` 的 `int(round(c))`。画布矩形来自上一帧
    /// —— 菜单/快捷键触发时本帧还没画到画布，这与源码用 `get_allocation()`
    /// 是同一类"上次布局"的信息。
    fn canvas_center_model(&self) -> Option<(i32, i32)> {
        let rect = self.canvas_rect?;
        let [x, y] = self.view.screen_to_world(rect.width() * 0.5, rect.height() * 0.5);
        Some((x.round() as i32, y.round() as i32))
    }

    /// 当前模式的图标名（`mode.get_icon_name()`：取关联动作的 icon-name）。
    pub(crate) fn mode_icon_name(&self) -> Option<&'static str> {
        crate::shell::actions::by_id(self.modes.top().action_name()?).and_then(|a| a.icon_name)
    }

    /// 构造模式上下文（等价 Python 里模式通过 `self.doc` 拿到的一切）。
    ///
    /// 字段级拆分借用：`doc` 可变、笔刷只读、笔刷状态可变，三者互不重叠。
    pub(crate) fn with_mode<R>(
        &mut self,
        f: impl FnOnce(
            &mut pigment_engine::mode::ModeStack,
            &mut pigment_engine::mode::ModeContext<'_>,
        ) -> R,
    ) -> R {
        // 取色模式要读"当前画笔颜色"（`colorpicker.py:83`）—— 在借用拆开之前取好
        let brush_color = self.color_hsv();
        let PigmentApp {
            modes,
            doc,
            brush,
            brush_name,
            stroke_state,
            mode_prefs,
            ..
        } = self;
        // 取色出口（B3-4）：取色模式把新颜色（HSV）写进这个槽，本函数返回前
        // 落到管理器 —— 等价于 `BrushColorManager.set_color()`
        // （`gui/brushcolor.py:33-37`）。其它模式不碰它。
        let mut picked: Option<[f32; 3]> = None;
        let r = {
            let mut ctx = pigment_engine::mode::ModeContext {
                doc,
                brush,
                brush_name,
                brush_state: stroke_state,
                fakepressure: mode_prefs.fakepressure,
                fakerotation: mode_prefs.fakerotation,
                hide_cursor_while_painting: mode_prefs.hide_cursor_while_painting,
                status: None,
                color: brush_color,
                picked_color: Some(&mut picked),
            };
            f(modes, &mut ctx)
        };
        if let Some(hsv) = picked {
            // `picked_color` 是 HSV（笔刷 `color_h/s/v` 同一通道），
            // `set_color_hsv` 会顺手 `derive_active()` 推给笔刷
            self.set_color_hsv(hsv);
        }
        r
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
        let painting = self.modes.top().is_painting();
        if painting {
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
        // `document.py:1717-1719`：`misc.context_restores_color` 开着时调
        // `restore_context_of_selected_brush()`，即从槽位快照把颜色取回来。
        // 开关的判断在管理器里（它也是偏好的持有者）。
        self.color_manager
            .restore_brush_context(color::to_f64_3(slot.color));
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
            color: color::to_f32_3(self.color_manager.brush_slot_color()),
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
            // 尺寸/不透明度四个动作改的是**笔刷设置**（`document.py:1571-1590`），
            // 颜色六个动作改的是**绘画颜色**（`document.py:1592-1644`）。
            "Bigger" | "Smaller" | "MoreOpaque" | "LessOpaque" => true,
            "Brighter" | "Darker" | "IncreaseHue" | "DecreaseHue" | "Purer" | "Grayer" => true,
            "BlendModeNormal" | "BlendModeEraser" => true,
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
            // ---- 工具模式（B3：无线电动作 + 它们的 Flip 快捷键版本）----
            "FreehandMode" | "StraightMode" | "SequenceMode" | "EllipseMode" | "FloodFillMode" => {
                true
            }
            "FlipFreehandMode" | "FlipStraightMode" | "FlipSequenceMode" | "FlipEllipseMode"
            | "FlipFloodFillMode" => true,
            // ---- 取色（B3-4：`gui/colorpicker.py` 的四个单选动作 + 四个 Flip）----
            "ColorPickMode" | "ColorPickModeH" | "ColorPickModeC" | "ColorPickModeY" => true,
            "FlipColorPickMode" | "FlipColorPickModeH" | "FlipColorPickModeC"
            | "FlipColorPickModeY" => true,
            // ---- 对称 / 画框开关（B3-5：两个 GtkToggleAction）----
            // 源码里这两个 toggle 从来没被 set_sensitive(False)
            // （`resources.xml:852-869`，回调在 `gui/document.py:1066-1072`
            // 与 `:1889-1905`），所以恒定可用。
            "SymmetryActive" | "FrameToggle" => true,
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
            // B3-5：两个 GtkToggleAction 的 active（`resources.xml:854,863`）。
            // 源码把它们与模型状态双向同步（`gui/document.py:1074-1078`、
            // `:1907-1916`）；egui 是立即模式，读模型即可。
            "SymmetryActive" => Some(self.doc.symmetry_active()),
            "FrameToggle" => Some(self.doc.get_frame_enabled()),
            // `ContextRestoreColor` 是 GtkToggleAction（`resources.xml:381`），
            // active 状态 = 偏好 `misc.context_restores_color`（`document.py:367-370`
            // 启动时用偏好初始化它，`:1721-1724` 反向写回）。
            "ContextRestoreColor" => Some(self.color_manager.context_restores_color()),
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
            // ---- 笔刷尺寸 / 不透明度（`brush_bigger_cb` / `brush_smaller_cb` /
            //      `more_opaque_cb` / `less_opaque_cb`，`gui/document.py:1571-1590`）----
            //
            // 源码：`adj = app.brush_adjustment[cname]; adj.set_value(adj.get_value() OP x)`
            // —— `GtkAdjustment` 用设置元数据的 `[lower, upper]` 夹取
            // （`gui/application.py:563-570`），所以半径夹在 [-2, 6]、不透明度夹在
            // [0, 2]。步长/系数与夹取规则都在引擎里（`ColorManager` 的同族
            // `BrushStep`），这里只负责把结果写回笔刷的两个落点。
            "Bigger" | "Smaller" | "MoreOpaque" | "LessOpaque" => {
                let step = BrushStep::from_action_name(id)
                    .expect("这四个动作都在 BrushStep 里");
                // 读的是**当前生效**的基值（含已有的覆盖），照 `adj.get_value()`
                let cur = self.brush.get(
                    BrushSetting::from_cname(step.cname())
                        .expect("BrushStep 的 cname 都是笔刷设置"),
                ).base_value as f64;
                let next = step.apply(cur) as f32;
                match step {
                    // 半径走 `radius_override`：本应用用它表示"当前这支笔被改过尺寸"
                    // （换笔刷时清空），与源码改 app.brush 的基值、再在换笔刷时被覆盖
                    // 是同一效果。
                    BrushStep::Bigger | BrushStep::Smaller => self.radius_override = Some(next),
                    // 不透明度没有覆盖机制，直接落到 base_brush 的基值上
                    // （等同 `app.brush.set_base_value("opaque", v)`）。
                    BrushStep::MoreOpaque | BrushStep::LessOpaque => {
                        self.base_brush.get_mut(BrushSetting::Opaque).base_value = next;
                    }
                }
                self.derive_active();
                ctx.request_repaint();
            }
            // ---- 颜色调整（六个 GtkAction，`gui/document.py:1592-1644`）----
            //
            // 回调读 `app.brush.get_color_hsv()`、改一个分量、写回 `set_color_hsv()`，
            // 随后 `BrushColorManager.__settings_changed_cb` 把 `HSVColor` 回填管理器
            // （`gui/brushcolor.py:39-48`）—— 这三步在 `ColorManager::adjust` 里。
            "Brighter" | "Darker" | "IncreaseHue" | "DecreaseHue" | "Purer" | "Grayer" => {
                let act = ColorAction::from_action_name(id)
                    .expect("这六个动作都在 ColorAction 里");
                if self.color_manager.adjust(act) {
                    // 颜色变了才重推笔刷（源码的 `if brush_color == self.get_color(): return`）
                    self.derive_active();
                }
                ctx.request_repaint();
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
            // ---- 对称开关（B3-5：`symmetry_active_toggled_cb`，
            //      `gui/document.py:1889-1905`）----
            "SymmetryActive" => {
                // 源码读的是 `action.get_active()`（toggle 的新状态）；立即模式里
                // 动作状态就是模型状态，所以"新状态"= 取反。
                let want_active = !self.doc.symmetry_active();
                // 从"中心还没设置过"到"要开启"时，中心取**视口中心**
                // （`:1895-1901`：`tdw.display_to_model(alloc.w/2, alloc.h/2)`），
                // 取整照 `symmetry.py:226` 的 `int(round(c))`。
                let mut center = None;
                if self.doc.symmetry_unset() && want_active {
                    self.doc.set_symmetry_unset(false);
                    center = self.canvas_center_model();
                }
                // `:1903-1905`：`want_active != already_active or center is not None`
                // 才落账。源码那句判断是为了容忍动作与模型不同步（`set_active`
                // 是异步的）；这里 `already_active` 恒等于 `!want_active`。
                let already_active = self.doc.symmetry_active();
                if want_active != already_active || center.is_some() {
                    self.doc
                        .set_symmetry_state(Some(want_active), center, None, None, None);
                    let state = if want_active { "开" } else { "关" };
                    self.set_status(format!("对称绘画：{state}"), now);
                }
                ctx.request_repaint();
            }
            // ---- 画框开关（B3-5：`toggle_frame_cb`，`gui/document.py:1066-1072`）----
            "FrameToggle" => {
                let enabled = self.doc.get_frame_enabled();
                let desired = !enabled;
                if enabled != desired {
                    self.doc.set_frame_enabled(desired);
                    let (x, y, w, h) = self.doc.get_frame();
                    // 源码第一次启用画框是「Edit Frame」模式做的
                    // （`framewindow.py:138-145`：没尺寸就按视口摆一个）；
                    // 那个模式没移植，这里只报告尺寸，免得用户以为坏了
                    // （0×0 的画框按 even-odd 填充会整块变暗，与源码一致）。
                    if desired && (w <= 0 || h <= 0) {
                        self.set_status(
                            "画框已启用，但尺寸还是 0×0（编辑画框模式未移植）".to_string(),
                            now,
                        );
                    } else {
                        let state = if desired { "开" } else { "关" };
                        self.set_status(format!("画框：{state}（{x},{y} {w}×{h}）"), now);
                    }
                }
                ctx.request_repaint();
            }
            // ---- 工具模式切换（B3-1/B3-2/B3-3）----
            // `mode_flip_action_activated_cb` / 无线电动作都落到"把模式压栈"上；
            // 本移植直接重置模式栈（不保留之前的临时视图模式）。
            "FreehandMode" | "StraightMode" | "SequenceMode" | "EllipseMode" | "FloodFillMode" => {
                self.switch_mode(id, now);
                ctx.request_repaint();
            }
            "FlipFreehandMode" | "FlipStraightMode" | "FlipSequenceMode" | "FlipEllipseMode"
            | "FlipFloodFillMode" => {
                let base = id.trim_start_matches("Flip");
                self.switch_mode(base, now);
                ctx.request_repaint();
            }
            // ---- 取色（B3-4）----
            // 单选动作：只压栈，不立即取色（`colorpicker.py:34-39`）
            "ColorPickMode" | "ColorPickModeH" | "ColorPickModeC" | "ColorPickModeY" => {
                let variant = pigment_engine::mode::pick::PickVariant::from_action_name(id)
                    .expect("动作表里的 ColorPickMode* 都有对应变体");
                self.switch_pick_mode(variant, false, None, now);
                ctx.request_repaint();
            }
            // `Flip*`（键盘 `r` / `Ctrl+r` / `Shift+r` / `Alt+r`）：
            // 照 `mode_flip_action_activated_cb`（`document.py:2163-2201`）——
            // 同一个模式已在栈顶就弹掉，否则按"键盘起手"新建并立即取一次色。
            "FlipColorPickMode" | "FlipColorPickModeH" | "FlipColorPickModeC"
            | "FlipColorPickModeY" => {
                let base = id.trim_start_matches("Flip");
                let same_on_top = self.modes.top().action_name().map(|a| a == base).unwrap_or(false);
                if same_on_top {
                    self.with_mode(|modes, ctx| {
                        modes.pop(ctx);
                    });
                } else {
                    let variant = pigment_engine::mode::pick::PickVariant::from_action_name(base)
                        .expect("Flip 动作名去掉前缀就是单选动作名");
                    let enter_pick = self.pointer_model_pos(ctx);
                    self.switch_pick_mode(variant, true, enter_pick, now);
                }
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
            // 源码 `context_toggle_color_cb`（`document.py:1721-1724`）只是写偏好
            // `misc.context_restores_color`；开关由管理器持有（默认 True，
            // `userconfig.py:93`）。
            "ContextRestoreColor" => {
                let value = self.color_manager.toggle_context_restores_color();
                let state = if value { "开" } else { "关" };
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
