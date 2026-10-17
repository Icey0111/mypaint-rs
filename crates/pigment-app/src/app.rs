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
//! - 暂存区（B5）：`Shift+K` 开关面板（`ToggleScratchpadTool`，`resources.xml:1467-1474`）、
//!   `Ctrl+K` 只显示不收起（`RevealScratchpadTool`，`:1475-1483`）；
//!   八个 `Scratch*` 菜单动作（`:1087-1155`）都没有加速键，与源码一致
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
//!
//! 暂存区（B5）是**第二份文档**：`scratch: Scratchpad` 有自己的 `Document`、
//! 自己的 `ModeStack`、自己的 `View` 与自己的显存缓存，但**共享**这一份
//! `brush`/`color_manager` —— 语义与源码的 `app.scratchpad_doc`
//! （`gui/application.py:361-369`）一致，细节见 [`crate::scratch`] 的模块文档。
//!
//! 用户偏好（B6-1）：[`PigmentApp::prefs`] 就是源码的 `app.preferences`
//! （`gui/application.py:280`）。启动时 `Prefs::load`（默认表 + `settings.json`
//! 浅覆盖，`:524-539`），退出时 [`PigmentApp::save_gui_config`] 写回
//! （`:624-628`）；`settings.json` 的路径、默认键表、未知键保留都在
//! `pigment_engine::prefs`。
//!
//! 偏好窗口（B6-3）：`PreferencesWindow` 动作开的是 `gui/preferenceswindow.py`
//! 那个独立对话框（`windowing.Dialog`，非模态），八个页签的顺序/标题与
//! 控件↔偏好键的对应表都在 [`crate::shell::prefs_window`]；"Keys" 页里嵌的是
//! B6-2 的加速键编辑器（`gui/preferenceswindow.glade:439`）。

use std::sync::{Arc, Mutex};

use eframe::egui;

use pigment_engine::brushlib::NUM_BRUSHKEYS;
use pigment_engine::color::{BrushStep, Color, ColorAction, ColorManager};
use pigment_engine::hokusai::{Brush, BrushSetting, BrushState};
use pigment_engine::mode::freehand::FreehandConfig;
use pigment_engine::prefs::{self, Prefs};
use pigment_engine::{CommandUpdate, Document};

use crate::brushes::BrushPanel;
use crate::color;
use crate::editor::BrushEditor;
use crate::gpu::TileRenderer;
use crate::scratch::{mode_call, ModeInputs};
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

/// 一次性的视图操纵工具（`gui/viewmanip.py`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewTool {
    Pan,
    Zoom,
    Rotate,
}

/// `inktool._LayoutNode`：浮动按钮布局的斥力/弹簧顶点。
#[derive(Clone, Copy, Debug)]
struct LayoutNode {
    x: f32,
    y: f32,
    fx: f32,
    fy: f32,
    vx: f32,
    vy: f32,
}

impl LayoutNode {
    fn new(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            fx: 0.0,
            fy: 0.0,
            vx: 0.0,
            vy: 0.0,
        }
    }

    /// `add_forces_inverse_square`。
    fn inverse_square(&mut self, others: &[LayoutNode], k: f32) {
        for o in others {
            let rsq = (self.x - o.x).powi(2) + (self.y - o.y).powi(2);
            if rsq != 0.0 {
                self.fx += k * (o.x - self.x) / rsq;
                self.fy += k * (o.y - self.y) / rsq;
            }
        }
    }

    /// `add_forces_linear`。
    fn linear(&mut self, others: &[LayoutNode], k: f32) {
        for o in others {
            self.fx += k * (o.x - self.x);
            self.fy += k * (o.y - self.y);
        }
    }

    /// `update_position(damping=0.85)`。
    fn update(&mut self) {
        self.vx = (self.vx + self.fx) * 0.85;
        self.vy = (self.vy + self.fy) * 0.85;
        self.fx = 0.0;
        self.fy = 0.0;
        self.x += self.vx;
        self.y += self.vy;
    }

    /// `constrain_position`。
    fn constrain(&mut self, x0: f32, x1: f32, y0: f32, y1: f32) {
        if self.x < x0 {
            self.x = x0;
            self.vx = 0.0;
        } else if self.x > x1 {
            self.x = x1;
            self.vx = 0.0;
        }
        if self.y < y0 {
            self.y = y0;
            self.vy = 0.0;
        } else if self.y > y1 {
            self.y = y1;
            self.vy = 0.0;
        }
    }

    fn speed(&self) -> f32 {
        self.vx.hypot(self.vy)
    }

    fn pos(&self) -> egui::Pos2 {
        egui::pos2(self.x, self.y)
    }
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

/// 预览面板的可变状态（`gui/previewwindow.py` 的 `PreviewTool`）。
///
/// 纹理缓存键 = `(合成版本, 有效外接框, mip 级)`；拖动状态记
/// `(按下时的模型坐标, 按下时的主视图中心)`，与源码 `_drag_start`
/// （`previewwindow.py:418`）同构。
#[derive(Default)]
pub(crate) struct PreviewState {
    pub(crate) tex: Option<egui::TextureHandle>,
    pub(crate) key: Option<(u64, [i32; 4], u32)>,
    pub(crate) size: [u32; 2],
    pub(crate) drag: Option<([f32; 2], [f32; 2])>,
}

/// 字段一律 `pub(crate)`：UI 已经按 `gui/` 的模块边界拆到 [`crate::shell`] 的
/// 子模块里（顶栏 / 工作区 / 底栏），那些模块要读写这些状态。
/// 「谁能改谁」的约束靠方法（`set_color`、`derive_active` 等）而不是字段可见性。
pub struct PigmentApp {
    /// 文档模型（画布表面 + 撤销栈），对应 MyPaint 的 `app.doc.model`。
    pub(crate) doc: Document,
    // ---- 用户偏好（B6-1）----
    /// 用户偏好 —— `app.preferences`（`gui/application.py:280` 的 `ObservableDict`）。
    ///
    /// 启动时 `Prefs::load`（默认表 + `settings.json` 浅覆盖，`application.py:524-539`），
    /// 退出时 `save_gui_config` 写回（`:624-628`）。类型与路径规则在
    /// `pigment_engine::prefs`。
    pub(crate) prefs: Prefs,
    // ---- 加速键映射（B6-2）----
    /// 加速键表：默认表（`resources.xml` + `document.py` 的槽位键）+ 用户覆盖
    /// （`accelmap.conf`，`gui/application.py:417` 载入 / `:625` 保存）。
    ///
    /// 查表入口是 [`crate::shell::accelmap::AccelMap::lookup`]（`handle_keys`
    /// 用它，取代了原来直接调 `actions::lookup`）。
    pub(crate) accel_map: crate::shell::accelmap::AccelMap,
    /// "Keys" 编辑器的状态（B6-2）：过滤框 / 捕获对话框 / 状态文字。
    ///
    /// 编辑器本身没有窗口 —— 它是偏好窗口 "Keys" 页的页面体
    /// （`gui/preferenceswindow.glade:439-450`），见 [`Self::prefs_window`]。
    pub(crate) key_editor: crate::shell::accelmap::KeyEditorState,
    // ---- 偏好窗口（B6-3）----
    /// 偏好窗口的界面状态 —— `gui/preferenceswindow.py` 那个
    /// `windowing.Dialog`（当前页签 / 是否可见 / 状态文字）。
    pub(crate) prefs_window: crate::shell::prefs_window::PrefsWindowState,
    // ---- 预览面板 ----
    /// 当前视图重命名对话框的文本（`ask_for_name` 的 egui 技术栈适配）。
    pub(crate) layer_view_rename: Option<String>,
    /// 预览面板的纹理/拖动状态（`gui/previewwindow.py`）。
    pub(crate) preview: PreviewState,
    /// 笔刷图标编辑器（`gui/brushiconeditor.py`）。
    pub(crate) icon_editor: crate::shell::icon_editor::IconEditorState,
    // ---- 暂存区（B5）----
    /// 暂存区：**第二份文档** + 独立模式栈 + 独立视图。
    ///
    /// 对应 `gui/application.py:361-369` 的 `self.scratchpad_doc`：源码给它单独建
    /// 一个 `lib.document.Document`（`painting_only=True`）和一个独立的
    /// `TiledDrawWidget`，控制器 `gui.document.Document` 自带一条 `ModeStack`
    /// （`gui/document.py:87`）。笔刷与颜色仍是**同一个**（`application.py:350,362`）。
    pub(crate) scratch: crate::scratch::Scratchpad,
    /// 暂存区专用的 GPU tile 渲染器。
    ///
    /// 渲染器本身不绑文档，但它带一份显存 tile 缓存 —— 两个画布共用一份会互相
    /// 覆盖对方的 tile，所以各一个实例（见 `shell/scratch_panel.rs` 的模块文档）。
    pub(crate) scratch_renderer: Arc<Mutex<TileRenderer>>,
    /// 上一帧暂存区面板是否还在停靠系统里。    ///
    /// 面板从"在"变成"不在"时要存盘 —— `scratchwindow.py:57-58` 把
    /// `destroy-event` / `delete-event` 接到 `_save_cb`（`:64-67`）。
    pub(crate) scratch_was_added: bool,
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
    /// `BrushModifier._last_selected_color`：切到「固定颜色」笔刷前用户选的色。
    ///
    /// 只影响 `restore_color` 笔刷（官方包里 2 支）：从固定色笔刷切回普通笔刷时
    /// 恢复它（`brushmodifier.py:108-115`）。
    pub(crate) last_selected_color: [f32; 3],
    /// 正在执行 `brush_selected_cb`（`brushmodifier.py:94-133`）：这期间的
    /// 换色不回写 `last_selected_color`。
    pub(crate) in_brush_selected: bool,
    // ---- 笔刷快捷键 ----
    /// 10 个槽位（`BrushManager.contexts`）。
    pub(crate) brushkeys: [Option<BrushKeySlot>; NUM_BRUSHKEYS],
    /// 最近一次保存/恢复的槽位（`BrushManager.selected_context`，初值 `None`）。
    pub(crate) selected_context: Option<usize>,
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
    /// 最近打开/保存的文件（`Gtk.RecentManager` 的替代，持久化在偏好）。
    pub(crate) recent_files: Vec<String>,
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
    /// 模式层的可配置项（假压力/假旋转/隐藏光标）。
    ///
    /// 启动与偏好变化时由 [`ModePrefs::from_prefs`] 从 [`PigmentApp::prefs`] 重推
    /// （见 [`PigmentApp::apply_settings`]）；桶旋转那三个键不在这里，它们进
    /// `FreehandConfig` 并广播给两条模式栈。
    pub(crate) mode_prefs: ModePrefs,
    pub(crate) view: View,
    /// 正在拖拽的视图工具（`gui/viewmanip.py` 的三个 Oneshot 模式）。
    pub(crate) view_tool: Option<ViewTool>,
    /// 视图工具上一帧的指针位置（画布局部坐标，逐帧增量用）。
    pub(crate) view_drag_prev: Option<egui::Pos2>,
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
    // ---- 颜色器页签（B4-3 / B4-4a）----
    /// 颜色面板当前页 —— [`crate::shell::color_panel::COLOR_PAGES`] 的下标。
    ///
    /// 源码里六个颜色器是六个独立停靠工具，各自有 `tool_widget_added` 状态；
    /// 本项目把它们合成一个面板 + 一条页签条（见 `shell/color_panel.rs` 的模块
    /// 文档），所以"当前是哪一页"需要应用自己记住。默认是分量滑条页
    /// （B4-3 先落地的那一页）。
    pub(crate) color_page: usize,
    /// HSV 三页（B4-4b）的控件状态：六个 `ColorChanger` + 立方的面顺序。
    ///
    /// 源码里每个控件都是独立的 `ColorAdjusterWidget`
    /// （`gui/colors/adjbases.py:432-830`），按下/拖动状态挂在控件自己身上；
    /// egui 立即模式没有控件对象，只能由应用持有。
    pub(crate) wheel_pages: crate::shell::wheel_pages::WheelPagesState,
    /// HCY 页（B4-4c）的控件状态：两个 `ColorChanger` + 色域遮罩。
    ///
    /// 源码里遮罩挂在 `MaskableWheelMixin` 实例上（`hcywheel.py:70-71` 的
    /// `__mask` 与 `mask_toggle`），egui 没有控件对象，只能由应用持有。
    pub(crate) hcy_wheel: crate::shell::hcy_wheel_page::HcyWheelState,
    // ---- 工具条（S4）----
    /// 图标纹理缓存（`shell::toolbars::IconCache`）。
    pub(crate) icon_cache: crate::shell::toolbars::IconCache,
    /// toolbar1 的 8 个片段开关（偏好 `ui.toolbar_items`，出厂默认见
    /// `userconfig.py:76-85`）。启动时由 [`crate::shell::toolbars::fragments_from_prefs`]
    /// 从偏好读入；运行期开关仍走片段菜单的动作（写回偏好属 B6-3）。
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
/// 三个字段的来源不同，别一律当偏好：
/// - `hide_cursor_while_painting` 是**软键** `ui.hide_cursor_while_painting`
///   （默认表里没有，`gui/freehand.py:280` / `preferenceswindow.py:202` 都用
///   `.get(key, False)` 读）；
/// - `fakepressure` / `fakerotation` 在源码里**不是偏好项**：`Application.__init__`
///   写死 0.5（`application.py:299-300`），之后由假输入滑条改
///   （`freehand.py:697-716`），不进 `settings.json`。所以这里读偏好也仍是 0.5，
///   等 B6-3 把假输入滑条接上再变成运行期状态。
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModePrefs {
    /// 鼠标/无压感设备的假压力（默认 0.5）。
    pub fakepressure: f32,
    /// 假桶旋转（默认 0.5）。
    pub fakerotation: f32,
    /// 画的时候隐藏系统光标（软键，默认 false）。
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

impl ModePrefs {
    /// 从偏好读 —— 三个字段里只有隐藏光标是偏好项（见类型文档）。
    ///
    /// 与 [`FreehandConfig::from_prefs`] 共用同一处读取，保证喂给模式上下文的
    /// 假输入（`ctx.fakepressure` 等）和喂给模式对象的桶旋转配置不会打架。
    pub(crate) fn from_prefs(prefs: &Prefs) -> Self {
        let cfg = FreehandConfig::from_prefs(prefs);
        Self {
            fakepressure: cfg.fakepressure,
            fakerotation: cfg.fakerotation,
            hide_cursor_while_painting: cfg.hide_cursor_while_painting,
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
        /// `load_scratchpad_cb`（打开暂存页，`drawwindow.py:613-631`）——
        /// 目标文档是**暂存区**，不是主文档。
        ScratchpadOpen,
        /// `save_as_scratchpad_cb`（导出暂存页，`drawwindow.py:631-632`）
        ScratchpadSaveAs,
        /// 调色板 `.gpl` 导入（`palette.py:132-196`）
        PaletteOpen,
        /// 调色板 `.gpl` 导出（`palette.py:198-222`）
        PaletteSaveAs,
    }

    pub(crate) struct FilePrompt {
        pub(crate) action: FileAction,
        path: String,
        /// 提交时补的扩展名（带点，如 `".ora"`）—— 文档/暂存页取偏好
        /// `saving.default_format`（`filehandling.py:1408-1411`），导出恒为 `.png`。
        ext: &'static str,
        /// 上一次的错误（校验失败时显示，不关闭弹层）。
        error: Option<String>,
        /// 是否还没抢过键盘焦点。
        ///
        /// 只在第一帧抢：否则每帧 `request_focus()` 会把焦点一直锁在输入框上，
        /// 「确定 / 取消」按钮永远点不到。
        want_focus: bool,
    }

    impl FilePrompt {
        pub(crate) fn new(
            action: FileAction,
            seed: Option<&std::path::Path>,
            doc_ext: &'static str,
        ) -> Self {
            let ext = action_extension(action, doc_ext);
            let path = seed
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| default_document_path(ext));
            Self {
                action,
                path,
                ext,
                error: None,
                want_focus: true,
            }
        }

        fn title(&self) -> &'static str {
            match self.action {
                FileAction::SaveAs => "另存为（Save As）",
                FileAction::Open => "打开（Open）",
                FileAction::Export => "导出 PNG（Export）",
                // `open_scratchpad_dialog`（filehandling.py:1140-1143）
                FileAction::ScratchpadOpen => "打开暂存页（Open Scratchpad…）",
                // `save_scratchpad_as_dialog`（filehandling.py:1256-1266）
                FileAction::ScratchpadSaveAs => "暂存页另存为（Export Scratchpad As…）",
                FileAction::PaletteOpen => "导入调色板 .gpl",
                FileAction::PaletteSaveAs => "导出调色板 .gpl",
            }
        }
    }

/// 最近文件栈：新→旧、去重、最多 20 条（Gtk.RecentManager 的 `add_item` 语义）。
fn push_recent(list: &mut Vec<String>, path: &str) {
    list.retain(|p| p != path);
    list.insert(0, path.to_string());
    list.truncate(20);
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

/// 某个动作补扩展名时用的后缀。
///
/// 导出恒 `.png`（`Export` 走 PNG）；其余（另存为 / 暂存页另存为）用偏好
/// `saving.default_format` 映射出来的扩展名 —— 源码 `save_autoincrement_file`
/// 就是这么取扩展名的（`filehandling.py:1408-1411`，映射表 `:598-602`）。
fn action_extension(action: FileAction, doc_ext: &'static str) -> &'static str {
    match action {
        FileAction::Export => ".png",
        FileAction::PaletteOpen | FileAction::PaletteSaveAs => ".gpl",
        _ => doc_ext,
    }
}

/// 弹层里给的路径要按动作补扩展名。
///
/// MyPaint 靠文件对话框的过滤器自动补扩展名（`save_as_dialog` 的
/// `set_current_name`）；我们没有对话框，就按动作给个默认后缀。
fn with_default_extension(action: FileAction, p: &str, doc_ext: &'static str) -> String {
    let ext = action_extension(action, doc_ext).trim_start_matches('.');
    let path = std::path::Path::new(p);
    match path.extension() {
        Some(_) => p.to_string(),
        None => format!("{p}.{ext}"),
    }
}

/// 弹层的默认文件名（照 `save_as_dialog` 的 `suggested_filename`；
/// 没有当前文件时 MyPaint 用文档名 "Untitled"，我们用 "untitled"）。
fn default_document_path(ext: &str) -> String {
    let ext = ext.trim_start_matches('.');
    std::env::current_dir()
        .map(|d| d.join(format!("untitled.{ext}")).display().to_string())
        .unwrap_or_else(|_| format!("untitled.{ext}"))
}

/// 模式调用的共享输入（笔刷 / 笔刷名 / 假输入 / 当前色）。
///
/// 主画布（[`PigmentApp::with_mode`]）与暂存区
/// （[`PigmentApp::with_scratch_mode`]）都从这里取，保证两块画布看到的是
/// **同一支笔、同一个颜色** —— 源码里两者传的就是同一个 `self.brush`
/// （`application.py:350,362`）和同一个 `brush_color_manager`（`:399`）。
///
/// 做成自由函数而不是 `&self` 方法：`&self` 会把整个 `PigmentApp` 借住，
/// 而调用点同时要 `&mut self.doc` —— 只能按字段借（笔刷只读、文档可变）。
fn mode_inputs<'a>(
    brush: &'a Brush,
    brush_name: &'a str,
    prefs: ModePrefs,
    color: [f32; 3],
) -> ModeInputs<'a> {
    ModeInputs {
        brush,
        brush_name,
        fakepressure: prefs.fakepressure,
        fakerotation: prefs.fakerotation,
        hide_cursor_while_painting: prefs.hide_cursor_while_painting,
        color,
    }
}


impl PigmentApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("pigment 需要 wgpu renderer（NativeOptions.renderer = Wgpu）");
        let renderer = TileRenderer::new(&rs.device, rs.target_format);
        // 暂存区是第二块画布 → 第二份显存缓存（见字段文档）
        let scratch_renderer = TileRenderer::new(&rs.device, rs.target_format);

        let brushes = BrushPanel::new();
        let base_brush = brushes.load_selected().unwrap_or_else(|e| {
            eprintln!("笔刷加载失败，退回内嵌默认笔刷: {e}");
            pigment_engine::brushlib::default_brush()
        });

        // ---- 用户偏好（B6-1）：启动时载入（`application.py:279-281`）----
        // 源码在 `Application.__init__` 里建好配置目录（`:228-231`）再
        // `load_settings()`。`Prefs::load` 走的是同一套默认表 + 浅覆盖。
        let config_path = prefs::settings_path();
        if let Some(dir) = config_path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("配置目录建不出来（{}）：{e}", dir.display());
            }
        }
        let prefs = Prefs::load(config_path);
        let mode_prefs = ModePrefs::from_prefs(&prefs);
        // 片段开关从偏好 `ui.toolbar_items` 读（`userconfig.py:76-85`）；
        // 在 `prefs` 被移进结构体之前算好。
        let toolbar_fragments = crate::shell::toolbars::fragments_from_prefs(&prefs);

        // 当前色/历史/调色板/色轮类型都从偏好来（`adjbases.py:121-147`）
        let color_manager = ColorManager::from_prefs(&prefs);
        let initial_color = color::hsv_of(&color_manager.get_color());
        let mut brushkeys = seed_brushkeys(&brushes);
        // 源码启动时用用户目录里的 `contextNN.myb` 覆盖默认播种
        // （`brushmanager.py:414-416`）；颜色随 brush 的 color_h/s/v 一起回来。
        for (i, saved) in brushes.library.load_contexts().into_iter().enumerate() {
            let Some(brush) = saved else { continue };
            let color = [
                brush.get(BrushSetting::ColorH).base_value,
                brush.get(BrushSetting::ColorS).base_value,
                brush.get(BrushSetting::ColorV).base_value,
            ];
            let name = brush
                .parent_brush_name
                .clone()
                .unwrap_or_else(|| format!("context{i:02}"));
            brushkeys[i] = Some(BrushKeySlot { brush, color, name });
        }
        let brush_name = brushes
            .selected_entry()
            .map(|e| e.mypaint_name())
            .unwrap_or_else(|| "Unknown Brush".to_string());
        // 暂存区目录（`filehandling.py:1431-1438`）与启动页（`application.py:474-486`）
        let scratch = {
            // 源码在启动时建好数据子目录（`application.py:232-236`），
            // 保存前还会再兜底建一次（`filehandling.py:1363-1366`）。
            let dir = pigment_engine::scratchpad::default_dir();
            if let Err(e) = pigment_engine::scratchpad::ensure_dir(&dir) {
                eprintln!("暂存区目录建不出来（{}）：{e}", dir.display());
            }
            crate::scratch::Scratchpad::new(dir)
        };
        // 最近文件：MyPaint 用 Gtk.RecentManager（XDG），这里落到偏好数组
        let recent_files: Vec<String> = prefs
            .get_array("recent_files")
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut app = Self {
            doc: Document::new(),
            prefs,
            // 加速键：先建默认表再加载用户覆盖（`gui/application.py:417` 的
            // 顺序：GTK 先注册 `resources.xml`/`document.py` 的绑定，
            // 之后才 `Gtk.AccelMap.load()`）
            accel_map: crate::shell::accelmap::AccelMap::with_default_path(),
            key_editor: Default::default(),
            prefs_window: Default::default(),
            layer_view_rename: None,
            preview: Default::default(),
            icon_editor: Default::default(),
            scratch,
            scratch_renderer: Arc::new(Mutex::new(scratch_renderer)),
            scratch_was_added: false,
            brushes,
            editor: BrushEditor::new(),
            base_brush,
            brush: Brush::new(),
            brush_name,
            radius_override: None,
            eraser: false,
            paint_override: None,
            color_manager,
            last_selected_color: initial_color,
            in_brush_selected: false,
            brushkeys,
            selected_context: None,
            live_update: false,
            status: None,
            current_file: None,
            file_prompt: None,
            recent_files,
            stroke_state: BrushState::default(),
            modes: pigment_engine::mode::ModeStack::new(|| {
                Box::new(pigment_engine::mode::freehand::FreehandMode::new())
            }),
            pending_pointer_time: 0.0,
            mode_prefs,
            view: View::default(),
            view_tool: None,
            view_drag_prev: None,
            canvas_rect: None,
            renderer: Arc::new(Mutex::new(renderer)),
            force_resync: false,
            pending_uploads: 0,
            stroke_dirty: false,
            fullscreen: false,
            icon_cache: Default::default(),
            toolbar_fragments,
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
            // 默认打开分量滑条页（B4-3 先落地的那一页）
            color_page: crate::shell::color_panel::PAGE_COMPONENT_SLIDERS,
            wheel_pages: crate::shell::wheel_pages::WheelPagesState::new(),
            hcy_wheel: crate::shell::hcy_wheel_page::HcyWheelState::new(),
        };
        app.derive_active();
        // 桶旋转配置推给两条模式栈（源码是每事件现读偏好；见
        // `ModeStack::set_freehand_config`）。走 `apply_settings` 是为了让
        // 启动与偏好对话框 ACCEPT 用同一条路径（`application.py:487`/`:514-522`）。
        app.apply_settings();
        // 载入用户按键覆盖 —— `gui/application.py:417`。放在 `apply_settings`
        // 之后：与源码一致（那时动作表/槽位键都注册完了）。
        app.load_accel_map();
        // 启动时载回上次的暂存页（`application.py:474-486`）：偏好
        // `scratchpad.last_opened_scratchpad` 有就用它，没有就写回 autosave 路径。
        app.scratch.startup_page(&mut app.prefs);
        app.doc.reset_background();
        app
    }

    /// 应用当前偏好 —— `Application.apply_settings()`（`gui/application.py:514-522`）
    /// 的数据侧。
    ///
    /// 源码三步（`_apply_pressure_mapping_settings` / `_apply_button_mapping_settings`
    /// / `_apply_autosave_settings`）依赖压力曲线、按键映射与自动备份三个子系统，
    /// 本移植都还没有（B6-2 / B6-3 记账），所以这里只做本块真正消费得到的部分：
    /// 把假输入/隐藏光标与桶旋转配置重推给模式层。
    ///
    /// 调用点：启动（`new`）与偏好对话框 ACCEPT（`preferenceswindow.py:128-132`，
    /// 对话框本身属 B6-3）。
    pub(crate) fn apply_settings(&mut self) {
        self.mode_prefs = ModePrefs::from_prefs(&self.prefs);
        let cfg = FreehandConfig::from_prefs(&self.prefs);
        self.modes.set_freehand_config(cfg);
        self.scratch.modes.set_freehand_config(cfg);
    }

    /// 保存配置 —— `Application.save_gui_config()`（`gui/application.py:624-628`）。
    ///
    /// 源码三件事，逐条对应：
    /// 1. `Gtk.AccelMap.save(<conf>/accelmap.conf)`（`:625`）—— B6-2 已接线，
    ///    见 [`PigmentApp::save_accel_map`]；
    /// 2. `preferences["workspace.layout"] = wkspace.get_layout()`（`:626-627`）——
    ///    窗口/停靠布局的序列化属 **B6-3**（`shell/dock.rs` 的记账）；
    /// 3. `save_settings()`（`:628`）—— 本方法做的就是把当前状态写回偏好再落盘。
    ///
    /// 调用点：退出（`quit_cb`，`drawwindow.py:688-690`；eframe 侧是
    /// [`eframe::App::on_exit`]）与偏好对话框 ACCEPT（`preferenceswindow.py:130`）。
    pub(crate) fn save_gui_config(&mut self) {
        // 1. 按键表整表落盘（与 `:625` 同一位置）
        self.save_accel_map();
        // 颜色管理器的五条键（源码"改动即写"，这里是保存时整份同步，见
        // `ColorManager::store_prefs` 的说明）
        self.color_manager.store_prefs(&mut self.prefs);
        // 暂存页路径（源码在 open/save 时写，`filehandling.py:965-974`/`:1034-1036`；
        // 这里在保存配置时同步，效果一致、崩溃前不落盘 —— 记账）
        let scratch_path = abspath(&self.scratch.path.to_string_lossy());
        self.prefs.set_str(
            prefs::KEY_LAST_OPENED_SCRATCHPAD,
            scratch_path.display().to_string(),
        );
        // 3. 偏好整表落盘 —— `save_settings()`（`:628`）。存盘这一步的实现在
        //    `prefs_window::settings_on_accept`：它同时是偏好对话框 ACCEPT
        //    （`preferenceswindow.py:130`）的存盘动作，失败消息进窗口状态行。
        match crate::shell::prefs_window::settings_on_accept(&self.prefs) {
            Ok(()) => {}
            Err(msg) => {
                eprintln!("{msg}");
                self.prefs_window.status = Some(msg);
            }
        }
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
    ///
    /// 颜色处理照 `BrushModifier.brush_selected_cb`（`brushmodifier.py:94-133`）：
    /// 普通笔刷保留当前色；`restore_color` 笔刷把当前色与笔刷自带色按
    /// `mix` 混合；从固定色笔刷切回普通笔刷时恢复 `_last_selected_color`。
    pub(crate) fn reload_selected_brush(&mut self) {
        self.in_brush_selected = true;
        let prev_hsv = self.color_hsv();
        let prev_rgb = pigment_engine::color::hsv_to_rgb(color::to_f64_3(prev_hsv));
        let mix_old = self.base_brush.get(BrushSetting::RestoreColor).base_value;

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

        let mix = self.base_brush.get(BrushSetting::RestoreColor).base_value;
        let new_hsv = if mix > 0.0 {
            let c2 = pigment_engine::color::hsv_to_rgb([
                self.base_brush.get(BrushSetting::ColorH).base_value as f64,
                self.base_brush.get(BrushSetting::ColorS).base_value as f64,
                self.base_brush.get(BrushSetting::ColorV).base_value as f64,
            ]);
            let mix = mix as f64;
            let c3 = [
                (1.0 - mix) * prev_rgb[0] + mix * c2[0],
                (1.0 - mix) * prev_rgb[1] + mix * c2[1],
                (1.0 - mix) * prev_rgb[2] + mix * c2[2],
            ];
            color::to_f32_3(pigment_engine::color::rgb_to_hsv(c3))
        } else if mix_old > 0.0 {
            self.last_selected_color
        } else {
            prev_hsv
        };
        // 直接写管理器：这次换色是「选笔刷」的一部分，不能覆盖
        // `_last_selected_color`（源码用 `_in_brush_selected_cb` 区分）。
        self.color_manager.set_color(color::hsv_color(new_hsv));
        self.in_brush_selected = false;
        self.derive_active();
    }

    /// 用当前编辑器设置新建一支笔刷（BrushEditor 的 clone 按钮）。
    pub(crate) fn new_brush_from_current(&mut self) -> bool {
        let mut brush = self.base_brush.clone();
        let preview = crate::thumb::render(&brush);
        match self.brushes.library.new_brush(
            &mut brush,
            Some((
                crate::thumb::PREVIEW_W as u32,
                crate::thumb::PREVIEW_H as u32,
                &preview,
            )),
        ) {
            Ok(idx) => {
                self.brushes.group = pigment_engine::brushlib::NEW_BRUSH_GROUP.to_string();
                self.brushes.selected = idx;
                self.brushes.search.clear();
                self.brushes.refresh_after_save(idx);
                true
            }
            Err(e) => {
                eprintln!("新建笔刷失败: {e}");
                false
            }
        }
    }

    /// 保存当前笔刷到用户目录 `.myb` + `_prev.png`（`ManagedBrush.save`）。
    ///
    /// 技术栈适配：MyPaint 的 BrushEditor 在 OK/Apply 时写盘；本项目编辑器
    /// 实时生效，所以给一个显式 "Save Brush" 按钮。
    pub(crate) fn save_current_brush_to_disk(&mut self) -> bool {
        let Some(key) = self.brushes.selected_entry().map(|e| e.key.clone()) else {
            return false;
        };
        let brush = self.base_brush.clone();
        let preview = crate::thumb::render(&brush);
        match self.brushes.library.save_brush_entry(
            &key,
            &brush,
            Some((
                crate::thumb::PREVIEW_W as u32,
                crate::thumb::PREVIEW_H as u32,
                &preview,
            )),
        ) {
            Ok(idx) => {
                self.brushes.refresh_after_save(idx);
                true
            }
            Err(e) => {
                eprintln!("保存笔刷失败: {e}");
                false
            }
        }
    }


    /// （`brushmanager.py:1064-1100`）。
    ///
    /// 源码的 `input_stroke_ended` **只由自由手绘模式发出**
    /// （`freehand.py:352`；填充/直线/取色不发），所以这里也只在栈顶是
    /// 自由手绘时记录。
    pub(crate) fn record_brush_history(&mut self) {
        if self.modes.top().name() != "Freehand Drawing" {
            return;
        }
        let brush = self.brush.clone();
        let name = self.brush_name.clone();
        let preview = self.brushes.selected;
        self.brushes.push_history(brush, name, preview);
    }

    /// 选中一条笔刷历史 —— `BrushHistoryView._history_button_clicked_cb`
    /// （`gui/history.py:66-76`）：`bm.select_brush(history[i])`。
    pub(crate) fn select_brush_history(&mut self, i: usize) -> bool {
        let Some(entry) = self.brushes.history_entry(i).cloned() else {
            return false;
        };
        self.base_brush = entry.brush;
        self.brush_name = entry.name;
        self.radius_override = None;
        self.eraser = false;
        self.paint_override = None;
        // 面板高亮跟着历史项对应的库条目走（源码里历史笔刷不在任何分组，
        // 选中后列表不高亮；这里退一步，让预览与工具条指示器一致）。
        if entry.preview < self.brushes.library.entries.len() {
            self.brushes.selected = entry.preview;
        }
        self.derive_active();
        true
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
        if !self.in_brush_selected {
            self.last_selected_color = color::hsv_of(&color);
        }
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

    // ---- 调色板动作的两个数据侧（B4-4a，`gui/drawwindow.py:657-684`）----

    /// `PaletteNext` / `PalettePrev` 的数据部分：
    /// `newcolor = mgr.palette.move_match_position(direction, mgr.get_color())`，
    /// 只有真的产生新颜色时才 `mgr.set_color(newcolor)`
    /// （`drawwindow.py:659-672`）。
    ///
    /// 返回是否换了当前色。
    pub(crate) fn palette_step(&mut self, direction: i32) -> bool {
        let current = self.color_manager.get_color();
        let new = self
            .color_manager
            .palette_mut()
            .move_match_position(direction, &current);
        match new {
            Some(color) => {
                // `set_color()` 会顺手让调色板重新命中（`adjbases.py:219`）
                self.set_color(Color::Rgb(color));
                true
            }
            None => false,
        }
    }

    /// `PaletteAddCurrentColor` 的数据部分（`drawwindow.py:677-684`）：
    /// `mgr.palette.append(mgr.get_color(), name=None, unique=True, match=True)`。
    ///
    /// 返回追加后的调色板长度。
    pub(crate) fn palette_add_current_color(&mut self) -> usize {
        let color = self.color_manager.get_color();
        let palette = self.color_manager.palette_mut();
        palette.append(Some(&color), None, true, true);
        palette.len()
    }

    /// `Ctrl+N` → `DocNew`：新建空白文档（`gui/filehandling.py:1088` 的
    /// `open_cb` 之前那段 `app.doc.model.clear()` 语义 —— 新文档把
    /// 内容、撤销栈、作画时长、当前文件名全部重置）。
    pub(crate) fn new_document(&mut self, now: f64) {
        self.flush_pending_stroke();
        // `Document::clear` 已经把画框/分辨率/文档设置/对称状态一并复位
        // （`lib/document.py:944-961`），这里不再重复设一遍。
        self.doc.clear();
        // `RootLayerStack.clear()` 回到「默认背景」——应用层默认是纸纹
        // （`reset_background()`，`document.py:1327-1380`）。
        self.doc.reset_background();
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
                self.record_recent(path);
                let verb = if export { "已导出" } else { "已保存" };
                self.set_status(format!("{verb}：{}", path.display()), now);
            }
            Err(e) => self.set_status(format!("保存失败：{e}"), now),
        }
    }

    /// 记一条最近文件（新→旧、去重、最多 20 条），写进偏好并落盘。
    fn record_recent(&mut self, path: &std::path::Path) {
        let s = path.display().to_string();
        push_recent(&mut self.recent_files, &s);
        self.prefs.set_str_array(
            "recent_files",
            self.recent_files.iter().map(|s| s.as_str()),
        );
        if let Err(e) = self.prefs.save() {
            eprintln!("保存最近文件列表失败: {e}");
        }
    }

    /// 最近文件列表（菜单用）。
    pub(crate) fn recent_files(&self) -> &[String] {
        &self.recent_files
    }

    /// 打开最近文件列表中的第 `idx` 条。
    pub(crate) fn open_recent(&mut self, idx: usize, now: f64) {
        let Some(path) = self.recent_files.get(idx).map(std::path::PathBuf::from) else {
            return;
        };
        self.open_from_path(&path, now);
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
                self.record_recent(path);
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
                    pigment_engine::LoadOutcome::Jpg { bbox } => {
                        format!("已打开 {}×{} 的 JPEG", bbox.2, bbox.3)
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

    /// 当前默认保存扩展名（带点）—— 偏好 `saving.default_format`。
    ///
    /// 出厂 `"openraster"` → `".ora"`（`userconfig.py:89` +
    /// `filehandling.py:598-602`/`:1408-1411`）。
    pub(crate) fn save_extension(&self) -> &'static str {
        prefs::save_extension(&self.prefs)
    }

    /// 建一个文件弹层，扩展名按当前偏好（`saving.default_format`）。
    fn new_file_prompt(
        &self,
        action: FileAction,
        seed: Option<&std::path::Path>,
    ) -> FilePrompt {
        FilePrompt::new(action, seed, self.save_extension())
    }

    /// 处理弹层提交。
    fn apply_file_prompt(&mut self, now: f64) {        let Some(prompt) = self.file_prompt.take() else {
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
        let candidate = if matches!(
            action,
            FileAction::Open | FileAction::ScratchpadOpen | FileAction::PaletteOpen
        ) {
            raw.to_string()
        } else {
            with_default_extension(action, raw, prompt.ext)
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
            // ---- 暂存区（作用在第二份文档上，主文档一概不动）----
            FileAction::ScratchpadOpen => {
                if !path.is_file() {
                    self.file_prompt = Some(FilePrompt {
                        error: Some(format!("文件不存在：{}", path.display())),
                        ..prompt
                    });
                    return;
                }
                match self.scratch.open(&path) {
                    Ok(()) => self.set_status(
                        format!("已打开暂存页：{}", self.scratch.path.display()),
                        now,
                    ),
                    Err(e) => self.set_status(format!("打开暂存页失败：{e}"), now),
                }
            }
            FileAction::ScratchpadSaveAs => {
                if let Some(dir) = path.parent() {
                    if !dir.is_dir() {
                        self.file_prompt = Some(FilePrompt {
                            error: Some(format!("目录不存在：{}", dir.display())),
                            ..prompt
                        });
                        return;
                    }
                }
                match self.scratch.save_as(&path) {
                    Ok(()) => self.set_status(
                        format!("暂存页已另存为：{}", self.scratch.path.display()),
                        now,
                    ),
                    Err(e) => self.set_status(format!("暂存页保存失败：{e}"), now),
                }
            }
            FileAction::PaletteOpen => {
                if !path.is_file() {
                    self.file_prompt = Some(FilePrompt {
                        error: Some(format!("文件不存在：{}", path.display())),
                        ..prompt
                    });
                    return;
                }
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let mut palette = pigment_engine::color::Palette::new();
                        match palette.load_str(&text) {
                            Ok(()) => {
                                let n = palette.len();
                                *self.color_manager.palette_mut() = palette;
                                self.set_status(
                                    format!("已导入调色板 {}（{n} 色）", path.display()),
                                    now,
                                );
                            }
                            Err(e) => {
                                self.set_status(format!("导入调色板失败：{e}"), now)
                            }
                        }
                    }
                    Err(e) => self.set_status(format!("读取调色板失败：{e}"), now),
                }
            }
            FileAction::PaletteSaveAs => {
                if let Some(dir) = path.parent() {
                    if !dir.is_dir() {
                        self.file_prompt = Some(FilePrompt {
                            error: Some(format!("目录不存在：{}", dir.display())),
                            ..prompt
                        });
                        return;
                    }
                }
                let text = self.color_manager.palette().to_gimp_palette_string();
                match std::fs::write(&path, text) {
                    Ok(()) => self.set_status(
                        format!("已导出调色板：{}", path.display()),
                        now,
                    ),
                    Err(e) => self.set_status(format!("导出调色板失败：{e}"), now),
                }
            }
        }
    }

    /// 打开调色板 `.gpl` 的文件弹层（PaletteEditorDialog 的 Load 按钮）。
    pub(crate) fn prompt_load_palette(&mut self) {
        let prompt = self.new_file_prompt(FileAction::PaletteOpen, None);
        self.file_prompt = Some(prompt);
    }

    /// 导出调色板 `.gpl` 的文件弹层（PaletteEditorDialog 的 Save 按钮）。
    pub(crate) fn prompt_save_palette(&mut self) {
        let prompt = self.new_file_prompt(FileAction::PaletteSaveAs, None);
        self.file_prompt = Some(prompt);
    }

    /// `Ctrl+S`：有当前文件就直接存，否则走另存为（照 `save_cb`，`:1230`）。
    pub(crate) fn save_command(&mut self, now: f64) {
        match self.current_file.clone() {
            Some(p) => self.save_to_path(&p, false, now),
            None => {
                let prompt = self.new_file_prompt(FileAction::SaveAs, None);
                self.file_prompt = Some(prompt);
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
                FileAction::ScratchpadOpen => "要打开到暂存区的文件路径（.ora / .png）：",
                FileAction::ScratchpadSaveAs => "暂存页另存的路径（不带后缀时按 .ora 处理）：",
                FileAction::PaletteOpen => "要导入的 GIMP 调色板路径（.gpl）：",
                FileAction::PaletteSaveAs => "导出调色板的路径（不带后缀时按 .gpl 处理）：",
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
            // 源码 `freehand.py:403-404`：`viewzoom = tdw.scale`、
            // `viewrotation = tdw.rotation`（弧度，cairo 的 `Matrix.rotate`）。
            viewrotation: self.view.rotation,
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
        // 笔刷尺寸模式在 `drag_stop_cb` 里写回半径（`brushmanip.py:287-290`）。
        // 必须在 `finish_oneshot()` 弹栈**之前**取走，否则模式带着请求一起没了。
        self.apply_pending_brush_radius();
        handled
    }

    /// 把模式请求的 `radius_logarithmic` 落到应用（`brush_adjustment`
    /// 的对应物：`radius_override` + `derive_active()`）。
    fn apply_pending_brush_radius(&mut self) {
        let request = self.with_mode(|modes, _ctx| modes.top_mut().take_brush_radius());
        if let Some(radius) = request {
            self.radius_override = Some(radius);
            self.derive_active();
        }
    }

    /// 一次拖拽结束后按 Oneshot 规则出栈（`OneshotDragMode.drag_stop_cb`，
    /// `mode.py:1302-1317`）：取色模式用工具条起手时，取完就回到原工具。
    pub(crate) fn finish_oneshot(
        &mut self,
        modifiers: pigment_engine::mode::Modifiers,
        now: f64,
    ) {
        let info = self.with_mode(|modes, ctx| {
            if modes.names().len() > 1 && modes.top_pops_after_drag(modifiers) {
                modes.pop(ctx);
                let top = modes.top();
                Some(format!("{}: {}", top.name(), top.usage()))
            } else {
                None
            }
        });
        if let Some(text) = info {
            self.set_status(text, now);
        }
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
        use pigment_engine::mode::ink::InkingMode;
        use pigment_engine::mode::line::{LineKind, LineMode};
        let new_mode: Box<dyn pigment_engine::mode::InteractionMode> = match action {
            "FreehandMode" => Box::new(FreehandMode::new()),
            "InkingMode" => Box::new(InkingMode::new()),
            "LayerMoveMode" => Box::new(pigment_engine::mode::layermove::LayerMoveMode::new()),
            "FrameEditMode" => Box::new(pigment_engine::mode::frameedit::FrameEditMode::new()),
            "SymmetryEditMode" => {
                Box::new(pigment_engine::mode::symmetryedit::SymmetryEditMode::new())
            }
            "StraightMode" => Box::new(LineMode::new(LineKind::Straight)),
            "SequenceMode" => Box::new(LineMode::new(LineKind::Sequence)),
            "EllipseMode" => Box::new(LineMode::new(LineKind::Ellipse)),
            // B3-3：填充（`gui/fill.py:40` 的 FloodFillMode）
            "FloodFillMode" => Box::new(FillMode::new()),
            _ => return,
        };
        let label = new_mode.name().to_string();
        let usage = new_mode.usage().to_string();
        // 选绘画模式即取消未完成的视图手势（源码里是模式栈 pop/替换）
        self.view_tool = None;
        self.view_drag_prev = None;
        self.with_mode(|modes, ctx| modes.reset(Some(new_mode), ctx));
        self.set_status(format!("{label}: {usage}"), now);
    }

    /// 进入视图操纵工具（`viewmanip.py` 的三个 Oneshot 模式；画布拖拽结束后
    /// 自动回到原绘画工具）。源码是压栈模式，这里用一次性手势状态实现，
    /// 因为画布平移/缩放/旋转本来就在 `shell/workspace.rs` 里直接改 `View`。
    pub(crate) fn start_view_tool(&mut self, tool: ViewTool, now: f64) {
        self.view_tool = Some(tool);
        self.view_drag_prev = None;
        let (name, usage) = match tool {
            ViewTool::Pan => ("Scroll View", "Drag the canvas view"),
            ViewTool::Zoom => ("Zoom View", "Zoom the canvas view"),
            ViewTool::Rotate => ("Rotate View", "Rotate the canvas view"),
        };
        self.set_status(format!("{name}: {usage}"), now);
    }

    /// 进入笔刷尺寸拖拽模式（`gui/brushmanip.py:159` 的 `BrushResizeMode`）。
    ///
    /// 源码是从按钮映射（`input.button_mapping`）里以 `mode_class` 处理器
    /// **压栈**进入的（`document.py:765-772`），拖完由
    /// `OneshotDragMode.drag_stop_cb` 弹回原模式。这里照搬压栈/弹栈：
    /// - `ModeStack::push` 保留下面那个模式，所以弹回后仍是原来的画笔/直线；
    /// - 因为 `ButtonMappingEditor` 还没移植（见路线图），暂由应用本地键
    ///   `Ctrl+Shift+R` 触发；接上按钮映射后应撤掉这个键。
    pub(crate) fn begin_brush_resize(&mut self, now: f64) {
        if self.modes.top().action_name() == Some("BrushResizeMode") {
            return;
        }
        let mode: Box<dyn pigment_engine::mode::InteractionMode> =
            Box::new(pigment_engine::mode::brushresize::BrushResizeMode::new());
        let label = mode.name().to_string();
        let usage = mode.usage().to_string();
        // 起手前取消未完成的视图手势（与 `switch_mode` 的规则一致）
        self.view_tool = None;
        self.view_drag_prev = None;
        self.with_mode(|modes, ctx| modes.push(mode, ctx));
        self.set_status(format!("{label}: {usage}"), now);
    }

    /// 视图操纵的一次拖拽增量（`viewmanip.py` 的三个 `drag_update_cb`）。
    pub(crate) fn apply_view_drag(
        view: &mut View,
        tool: ViewTool,
        prev: egui::Pos2,
        cur: egui::Pos2,
        center: egui::Vec2,
    ) {
        let d = cur - prev;
        match tool {
            ViewTool::Pan => view.pan(d.x, d.y),
            ViewTool::Zoom => {
                view.pan(-d.x, -d.y);
                view.zoom_at(cur.x, cur.y, (d.y / 100.0).exp());
            }
            ViewTool::Rotate => {
                let a1 = (prev.y - center.y).atan2(prev.x - center.x);
                let a2 = (cur.y - center.y).atan2(cur.x - center.x);
                let mut step = a2 - a1;
                while step > std::f32::consts::PI {
                    step -= std::f32::consts::TAU;
                }
                while step < -std::f32::consts::PI {
                    step += std::f32::consts::TAU;
                }
                view.rotate_at(step, center.x, center.y);
            }
        }
    }

    /// Inking 画布上 OK/垃圾桶按钮的位置（`Overlay.update_button_positions`，
    /// `inktool.py:967-1060`）。返回画布局部坐标 `(accept, reject)`。
    pub(crate) fn ink_button_positions(&self, rect: egui::Rect) -> Option<(egui::Pos2, egui::Pos2)> {
        let top = self.modes.top();
        if !top.ink_editing() || top.ink_dragging() {
            return None;
        }
        let nodes = top.ink_nodes()?;
        if nodes.is_empty() {
            return None;
        }
        let fixed: Vec<LayoutNode> = nodes
            .iter()
            .map(|n| {
                let s = self.view.world_to_screen(n.x, n.y);
                LayoutNode::new(s[0], s[1])
            })
            .collect();
        Self::layout_ink_buttons(&fixed, rect.width(), rect.height())
    }

/// 纯布局：给定节点的画布坐标，返回 OK/垃圾桶按钮位置
/// （`Overlay.update_button_positions` 的力导向循环）。
fn layout_ink_buttons(
    fixed: &[LayoutNode],
    width: f32,
    height: f32,
) -> Option<(egui::Pos2, egui::Pos2)> {
    let last = fixed.len().checked_sub(1)?;
    let quarter = (last * 3) / 4;
    let mut tail = (
        fixed[last].x - fixed[quarter].x,
        fixed[last].y - fixed[quarter].y,
    );
    let len = tail.0.hypot(tail.1);
    if len <= 0.0 {
        tail = (0.0, 1.0);
    } else {
        tail = (tail.0 / len, tail.1 / len);
    }
    let margin = 1.5 * 16.0; // FLOATING_BUTTON_RADIUS
    let mut accept = LayoutNode::new(
        fixed[last].x + tail.0 * margin,
        fixed[last].y + tail.1 * margin,
    );
    let mut reject = LayoutNode::new(
        fixed[0].x - tail.0 * margin,
        fixed[0].y - tail.1 * margin,
    );
    let reject_bbox = (margin, width - margin, margin, height - 2.666 * margin);
    let accept_bbox = (margin, width - margin, 2.666 * margin, height - margin);
    for _ in 0..100 {
        accept.inverse_square(fixed, -25.0);
        accept.inverse_square(&[reject], -25.0);
        accept.linear(&[fixed[last]], 0.05);
        reject.inverse_square(fixed, -25.0);
        reject.inverse_square(&[accept], -25.0);
        reject.linear(&[fixed[0]], 0.05);
        reject.update();
        reject.constrain(reject_bbox.0, reject_bbox.1, reject_bbox.2, reject_bbox.3);
        accept.update();
        accept.constrain(accept_bbox.0, accept_bbox.1, accept_bbox.2, accept_bbox.3);
        if accept.speed() < 0.5 && reject.speed() < 0.5 {
            break;
        }
    }
    Some((accept.pos(), reject.pos()))
}

    /// 画布按钮命中：`Some(true)` = OK，`Some(false)` = 垃圾桶。
    pub(crate) fn ink_button_at(
        &self,
        rect: egui::Rect,
        pos: egui::Pos2,
    ) -> Option<bool> {
        let (accept, reject) = self.ink_button_positions(rect)?;
        let local = egui::pos2(pos.x - rect.min.x, pos.y - rect.min.y);
        if local.distance(accept) <= 16.0 {
            Some(true)
        } else if local.distance(reject) <= 16.0 {
            Some(false)
        } else {
            None
        }
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
        // 取色也取消未完成的视图手势
        self.view_tool = None;
        self.view_drag_prev = None;
        // `context_push`：保留下面的绘画模式，取完色/松开 `r` 后 pop 回来
        // （`document.py:2201`）。
        self.with_mode(|modes, ctx| modes.context_push(new_mode, ctx));
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
    ///
    /// `BrushResizeMode` 没有自己的 GtkAction，源码直接返回上一个模式的图标
    /// （`brushmanip.py:192-193` 的 `get_icon_name()`），所以这里走
    /// [`pigment_engine::mode::ModeStack::options_mode`]。
    pub(crate) fn mode_icon_name(&self) -> Option<&'static str> {
        crate::shell::actions::by_id(self.modes.options_mode().action_name()?)
            .and_then(|a| a.icon_name)
    }

    /// 构造模式上下文（等价 Python 里模式通过 `self.doc` 拿到的一切）。
    ///
    /// 借用拆分、取色出口的处理都在 [`crate::scratch::mode_call`] 里 ——
    /// 主画布与暂存区（[`PigmentApp::with_scratch_mode`]）共用同一份实现。
    pub(crate) fn with_mode<R>(
        &mut self,
        f: impl FnOnce(
            &mut pigment_engine::mode::ModeStack,
            &mut pigment_engine::mode::ModeContext<'_>,
        ) -> R,
    ) -> R {
        // 取色模式要读"当前画笔颜色"（`colorpicker.py:83`）—— 在借用拆开之前取好
        let color = self.color_hsv();
        let (r, picked) = mode_call(
            &mut self.doc,
            &mut self.modes,
            &mut self.stroke_state,
            mode_inputs(&self.brush, &self.brush_name, self.mode_prefs, color),
            f,
        );
        if let Some(hsv) = picked {
            // `picked_color` 是 HSV（笔刷 `color_h/s/v` 同一通道），
            // `set_color_hsv` 会顺手 `derive_active()` 推给笔刷
            self.set_color_hsv(hsv);
        }
        r
    }

    /// 在**暂存区**的 (文档, 模式栈, 笔刷状态) 上跑一段模式逻辑。
    ///
    /// 与 [`PigmentApp::with_mode`] 的唯一区别是换了那一份 `Document` /
    /// `ModeStack` —— 对应源码里暂存区有自己的 `Document` 控制器与
    /// `ModeStack`（`gui/document.py:87`），但笔刷/颜色是同一份
    /// （`gui/application.py:350,362`）。
    pub(crate) fn with_scratch_mode<R>(
        &mut self,
        f: impl FnOnce(
            &mut pigment_engine::mode::ModeStack,
            &mut pigment_engine::mode::ModeContext<'_>,
        ) -> R,
    ) -> R {
        let color = self.color_hsv();
        let (r, picked) = self
            .scratch
            .dispatch(mode_inputs(&self.brush, &self.brush_name, self.mode_prefs, color), f);
        if let Some(hsv) = picked {
            self.set_color_hsv(hsv);
        }
        r
    }

    // ---- 暂存区的指针事件（`shell/scratch_panel.rs` 调用）----
    /// 在暂存区上按下（`gui/mode.py` 的 `button_press_cb`）。
    pub(crate) fn scratch_mode_press(
        &mut self,
        ev: pigment_engine::mode::PointerEvent,
    ) -> bool {
        let mut handled = false;
        self.with_scratch_mode(|modes, ctx| {
            handled = modes.top_mut().button_press(ctx, &ev);
        });
        self.scratch_process_pending();
        handled
    }

    /// 在暂存区上移动。
    pub(crate) fn scratch_mode_motion(
        &mut self,
        ev: pigment_engine::mode::PointerEvent,
    ) -> bool {
        let mut handled = false;
        self.with_scratch_mode(|modes, ctx| {
            handled = modes.top_mut().motion(ctx, &ev);
        });
        self.scratch_process_pending();
        handled
    }

    /// 在暂存区上松开。
    pub(crate) fn scratch_mode_release(
        &mut self,
        ev: pigment_engine::mode::PointerEvent,
    ) -> bool {
        let mut handled = false;
        self.with_scratch_mode(|modes, ctx| {
            handled = modes.top_mut().button_release(ctx, &ev);
        });
        self.scratch_process_pending();
        handled
    }

    /// 处理暂存区模式排队的输入（每帧一次，`GLib.idle_add` 的等价物）。
    pub(crate) fn scratch_process_pending(&mut self) {
        self.with_scratch_mode(|modes, ctx| modes.process_pending(ctx));
    }

    /// 把暂存区在途的作画落账（撤销/重做、存盘前）。
    pub(crate) fn flush_scratch_stroke(&mut self) {
        self.with_scratch_mode(|modes, ctx| modes.checkpoint(ctx));
    }

    /// 暂存区面板被关掉时存盘 —— `gui/scratchwindow.py:57-58` + `:64-67`：
    ///
    /// ```python
    /// self.connect("destroy-event", self._save_cb)
    /// self.connect("delete-event", self._save_cb)
    /// ...
    /// def _save_cb(self, action):
    ///     self.app.filehandler.save_scratchpad(self.app.scratchpad_filename)
    /// ```
    ///
    /// 源码是"面板销毁/关闭"两个信号；egui 这边对应"面板从停靠系统里没了"，
    /// 所以每帧末比对一次"上一帧在不在"。存盘本身仍走 `save_scratchpad`
    /// 的判据（没画过就不写盘，`filehandling.py:1019-1023`）。
    pub(crate) fn save_scratchpad_on_close(&mut self, now: f64) {
        let added = self
            .dock
            .is_added(crate::shell::dock::PanelId::ScratchpadTool);
        if self.scratch_was_added && !added {
            self.flush_scratch_stroke();
            match self.scratch.save() {
                Ok(true) => self.set_status(
                    format!("暂存区已保存：{}", self.scratch.path.display()),
                    now,
                ),
                Ok(false) => {}
                Err(e) => self.set_status(format!("暂存区保存失败：{e}"), now),
            }
        }
        self.scratch_was_added = added;
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
        self.selected_context = Some(i);
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
        self.selected_context = Some(i);
        // `context.save()`：整份 brushinfo 写 `contextNN.myb`（颜色也在里面）
        if let Err(e) = self
            .brushes
            .library
            .save_context(i, &self.base_brush.clone())
        {
            self.set_status(format!("槽位 {i} 保存失败：{e}"), now);
            return;
        }
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
        // 按键捕获对话框开着时，键事件归捕获流程（`shell::accelmap`），
        // 不能顺手触发一次快捷键 —— 对应 GTK 那边"对话框是模态的"。
        if self.key_editor.capture.is_some() {
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
        let releases: Vec<(egui::Key, actions::Mods)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: false,
                        modifiers,
                        ..
                    } => Some((*key, actions::Mods::from_egui(modifiers))),
                    _ => None,
                })
                .collect()
        });
        let now = ctx.input(|i| i.time);
        // 松开：Escape 紧急复位（`keyboard.py:214-221`），键盘起手的 Oneshot
        // （取色 `r`）回原工具（`document.py:2200` / `colorpicker.py:53`）。
        for (key, _mods) in releases {
            if key == egui::Key::Escape {
                self.with_mode(|modes, ctx| modes.reset(None, ctx));
                self.set_status("已取消当前操作".to_string(), now);
                ctx.request_repaint();
            } else if self.with_mode(|modes, ctx| {
                if modes.names().len() > 1 && modes.top_pops_on_key_release() {
                    modes.pop(ctx);
                    true
                } else {
                    false
                }
            }) {
                let text = self.with_mode(|modes, _ctx| {
                    let top = modes.top();
                    format!("{}: {}", top.name(), top.usage())
                });
                self.set_status(text, now);
                ctx.request_repaint();
            }
        }
        for (key, mods) in events {
            self.dispatch_key(ctx, key, mods, now);
        }
    }

    /// 一个按键事件 → 一个动作。
    fn dispatch_key(&mut self, ctx: &egui::Context, key: egui::Key, mods: actions::Mods, now: f64) {
        // Escape 先交给当前模式（`freehand.py` 清队列 / `linemode.py` 回滚），
        // 松开时再由 handle_keys 统一 reset（`keyboard.py:214-221`）。
        if key == egui::Key::Escape {
            let handled = self.with_mode(|modes, ctx| {
                modes.top_mut().key_press(ctx, pigment_engine::mode::Key::Escape)
            });
            if handled {
                return;
            }
        }
        // 应用本地扩展键优先（它们占的是源码没碰过的键位，见 `actions::APP_KEYS`）
        if let Some(k) = actions::lookup_app(key, mods) {
            if self.run_app_key(k.id, now) {
                return;
            }
        }
        // 查表走 `AccelMap`（默认表 + `accelmap.conf` 的用户覆盖 + 兜底），
        // 对应 `gui/keyboard.py:139-142` 的两级查找。兜底键可以被偏好关掉
        // （`keyboard.py:159-160` 的 `fallbacks_disabled()`）。
        let allow_fallbacks = !self
            .prefs
            .bool_or(pigment_engine::prefs::KEY_DISABLE_FALLBACKS, false);
        match self.accel_map.lookup(key, mods, allow_fallbacks) {
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
            // 绘画模式单选按钮保持按下态（`_modestack_changed_cb`）
            "FreehandMode" | "InkingMode" | "StraightMode" | "SequenceMode" | "EllipseMode"
            | "FloodFillMode" | "ColorPickMode" | "ColorPickModeH" | "ColorPickModeC"
            | "ColorPickModeY" | "LayerMoveMode" | "FrameEditMode" | "SymmetryEditMode" => {
                self.modes.top().action_name() == Some(id)
            }
            "PanViewMode" => self.view_tool == Some(ViewTool::Pan),
            "ZoomViewMode" => self.view_tool == Some(ViewTool::Zoom),
            "RotateViewMode" => self.view_tool == Some(ViewTool::Rotate),
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
            // 颜色器（B4-3）：GtkToggleAction 的 active = 面板已添加
            // （`get_tool_widget_added`，drawwindow.py:357-358）
            "ComponentSlidersTool" => self
                .dock
                .is_added(crate::shell::dock::PanelId::ComponentSlidersTool),
            // 调色板工具（B4-4a）：`PaletteTool` 是 `GtkToggleAction`
            // （resources.xml:1400-1406），active 同样是"工具已添加"；
            // 本项目两个工具共用一个面板，所以还要看当前是不是调色板页
            "PaletteTool" => {
                self.dock
                    .is_added(crate::shell::dock::PanelId::ComponentSlidersTool)
                    && self.color_page == crate::shell::color_panel::PAGE_PALETTE
            }
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
            "RotateLeft" | "RotateRight" | "RotateLeftCentered" | "RotateRightCentered" => true,
            "MirrorHorizontal" | "MirrorVertical" => true,
            "ResetView" | "ResetRotation" | "ResetZoom" | "ResetMirror" => true,
            // ---- 停靠面板（S6）----
            "ToggleModeOptionsTool" | "ToggleLayersTool" | "ToggleScratchpadTool"
            | "TogglePreviewTool" | "ToggleHistoryPanel" | "BrushGroups" => true,
            // `RevealScratchpadTool`（resources.xml:1475-1483，`Ctrl+K`）——
            // `reveal_dockpanel_cb`（drawwindow.py:340-353），恒可用
            "RevealScratchpadTool" => true,
            // ---- 暂存区（B5）----
            // `resources.xml:1094-1154` 的八个菜单动作。源码里它们都没有
            // `set_sensitive(False)` 的地方（`drawwindow.py:591-655`），
            // 所以恒可用；`ScratchRevert` 在源码里也只是"文件不在就什么都不做"。
            "ScratchNew" | "ScratchLoad" | "ScratchSaveNow" | "ScratchSaveAs"
            | "ScratchRevert" | "ScratchSaveAsDefault" | "ScratchClearDefault"
            | "ScratchCopyBackground" => true,
            // ---- 颜色器（B4-3）----
            // `ComponentSlidersTool` 是 `GtkToggleAction`（resources.xml:1433），
            // 打开的就是本块移植的分量滑条面板；`ColorAdjusters` 是 toolbar2 的
            // 工厂动作（resources.xml:198，label "Color Adjusters"/short "Colors"），
            // 源码弹一个九种颜色器的菜单 —— 本项目只实现了其中一页，动作直接
            // reveal 该面板并记账（见 B4-3 交付说明）。
            "ComponentSlidersTool" | "ColorAdjusters" => true,
            // 颜色器页签的动作（B4-4b / B4-4c）。源码里六个颜色器是六个独立的
            // `GtkToggleAction`（`resources.xml:1392-1438`），各自 toggle 自己的
            // 停靠窗口；本项目合成一个面板，所以每个动作都是"切到那一页"。
            "HCYWheelTool" | "HSVWheelTool" | "HSVSquareTool" | "HSVCubeTool" => true,
            // ---- 调色板（B4-4a）----
            // 工具开关 + 三个动作（resources.xml:1034-1053 的 `PaletteNext` /
            // `PalettePrev` / `PaletteAddCurrentColor`，:1400-1406 的
            // `PaletteTool`）。源码里它们都没有 `set_sensitive(False)` 的地方。
            "PaletteTool" | "PaletteNext" | "PalettePrev" | "PaletteAddCurrentColor" => true,
            // ---- 图层（B2-5/B2-6；动态 sensitivity 照源码 enable 语义）----
            "NewPaintingLayerAbove" | "NewLayerGroupAbove" | "DuplicateLayer" | "RemoveLayer" => {
                true
            }
            "RaiseLayerInStack" | "LowerLayerInStack" => {
                // `_update_layer_bubble_actions`（document.py:1393-1405）：
                // 组内恒可（冒泡会出组），根层看端点。
                let ls = self.doc.layers();
                let path = ls.current_path().to_vec();
                ls.can_bubble(&path, id == "RaiseLayerInStack")
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
            "FreehandMode" | "InkingMode" | "StraightMode" | "SequenceMode" | "EllipseMode"
            | "FloodFillMode" | "LayerMoveMode" => true,
            "FlipFreehandMode" | "FlipInkingMode" | "FlipStraightMode" | "FlipSequenceMode"
            | "FlipEllipseMode" | "FlipFloodFillMode" | "FlipLayerMoveMode" => true,
            "FrameEditMode" | "FlipFrameEditMode" => true,
            "SymmetryEditMode" | "FlipSymmetryEditMode" => true,
            // 视图操纵（`gui/viewmanip.py` 的三个单选 + 三个 Flip）
            "PanViewMode" | "ZoomViewMode" | "RotateViewMode" => true,
            "FlipPanViewMode" | "FlipZoomViewMode" | "FlipRotateViewMode" => true,
            // Inking 的节点编辑动作（`document.py:2318-2328`）只在 ADJUST 阶段可用
            "SimplifyNodes" | "CullNodes" => self.modes.top().ink_editing(),
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
            // ---- 偏好窗口（B6-3）----
            "PreferencesWindow" => true,
            // ---- 笔刷槽位（`ContextRestoreColor` 与 20 个动态槽位动作）----
            "ContextRestoreColor" => true,
            // `ContextStore`（`q`）：把当前笔刷存进最近一次选中/存过的槽位
            // （`document.py:1692-1709`）；没有选中槽位时源码只记日志。
            "ContextStore" => true,
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
            // 停靠面板开关都是 `GtkToggleAction`（resources.xml 的
            // Toggle*Tool / ComponentSlidersTool），active = 面板已添加
            // （`toggle_dockpanel_cb` 读的就是 `get_tool_widget_added`，
            // drawwindow.py:357-358）。B4-3 一并补上，菜单里才有勾选标记。
            "ToggleModeOptionsTool" => {
                Some(self.dock.is_added(crate::shell::dock::PanelId::ModeOptionsTool))
            }
            "ToggleLayersTool" => Some(self.dock.is_added(crate::shell::dock::PanelId::LayersTool)),
            "ToggleScratchpadTool" => {
                Some(self.dock.is_added(crate::shell::dock::PanelId::ScratchpadTool))
            }
            "TogglePreviewTool" => {
                Some(self.dock.is_added(crate::shell::dock::PanelId::PreviewTool))
            }
            "ToggleHistoryPanel" => {
                Some(self.dock.is_added(crate::shell::dock::PanelId::HistoryPanel))
            }
            "ComponentSlidersTool" => Some(
                self.dock
                    .is_added(crate::shell::dock::PanelId::ComponentSlidersTool),
            ),
            // `PaletteTool`（resources.xml:1400-1406）的 active 在源码里是
            // "调色板工具已添加"；两个工具共用一个面板，这里再加上"当前是
            // 调色板页"（见 `shell/color_panel.rs` 的模块文档）。
            "PaletteTool" => Some(
                self.dock
                    .is_added(crate::shell::dock::PanelId::ComponentSlidersTool)
                    && self.color_page == crate::shell::color_panel::PAGE_PALETTE,
            ),
            // 偏好窗口（B6-3）是 `GtkToggleAction`（`resources.xml:1330-1335`），
            // active = 窗口可见（`drawwindow.py:378-386` 的 `toggle_window_cb`）。
            "PreferencesWindow" => Some(self.prefs_window.open),
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
            "Open" => {
                let prompt = self.new_file_prompt(FileAction::Open, None);
                self.file_prompt = Some(prompt);
            }
            "Save" => {
                self.save_command(now);
                ctx.request_repaint();
            }
            "SaveAs" => {
                let prompt =
                    self.new_file_prompt(FileAction::SaveAs, self.current_file.as_deref());
                self.file_prompt = Some(prompt);
            }
            "Export" => {
                // 建议名 = 当前文件名换 .png（照 `save_as_dialog` 的 suggested_filename）
                let seed = self
                    .current_file
                    .as_ref()
                    .map(|p| p.with_extension("png"));
                let prompt = self.new_file_prompt(FileAction::Export, seed.as_deref());
                self.file_prompt = Some(prompt);
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
            // ---- 视图：旋转 / 镜像（`document.py:1818-1870` + `:1920-1936`）----
            // 步长 = 2π/16（`ROTATION_STEP`）；RotateLeft 顺时针、RotateRight 逆时针
            "RotateLeft" | "RotateRight" | "RotateLeftCentered" | "RotateRightCentered" => {
                let clockwise = !id.starts_with("RotateRight");
                let viewport_centered = id.ends_with("Centered");
                let anchor = self
                    .canvas_anchor(ctx, !viewport_centered)
                    .unwrap_or([0.0, 0.0]);
                let step = if clockwise {
                    std::f32::consts::TAU / 16.0
                } else {
                    -std::f32::consts::TAU / 16.0
                };
                self.view.rotate_at(step, anchor[0], anchor[1]);
                ctx.request_repaint();
            }
            "MirrorHorizontal" => {
                let anchor = self.canvas_anchor(ctx, false).unwrap_or([0.0, 0.0]);
                self.view.mirror_at(anchor[0], anchor[1]);
                ctx.request_repaint();
            }
            "MirrorVertical" => {
                // 源码：`tdw.rotate(pi)` 再 `tdw.mirror()`
                let anchor = self.canvas_anchor(ctx, false).unwrap_or([0.0, 0.0]);
                self.view
                    .rotate_at(std::f32::consts::PI, anchor[0], anchor[1]);
                self.view.mirror_at(anchor[0], anchor[1]);
                ctx.request_repaint();
            }
            "ResetRotation" | "ResetView" | "ResetZoom" | "ResetMirror" => {
                let anchor = self.canvas_anchor(ctx, false).unwrap_or([0.0, 0.0]);
                if id != "ResetMirror" && id != "ResetZoom" {
                    self.view.rotate_at(-self.view.rotation, anchor[0], anchor[1]);
                }
                if id != "ResetRotation" && id != "ResetZoom" && self.view.mirrored {
                    self.view.mirror_at(anchor[0], anchor[1]);
                }
                if id == "ResetView" || id == "ResetZoom" {
                    let default_zoom = self.prefs.f64_or("view.default_zoom", 1.0) as f32;
                    self.view.set_zoom_at(default_zoom, anchor[0], anchor[1]);
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
            "FreehandMode" | "InkingMode" | "StraightMode" | "SequenceMode" | "EllipseMode"
            | "FloodFillMode" | "LayerMoveMode" | "FrameEditMode" | "SymmetryEditMode" => {
                self.switch_mode(id, now);
                ctx.request_repaint();
            }
            "FlipFreehandMode" | "FlipInkingMode" | "FlipStraightMode" | "FlipSequenceMode"
            | "FlipEllipseMode" | "FlipFloodFillMode" | "FlipLayerMoveMode"
            | "FlipFrameEditMode" | "FlipSymmetryEditMode" => {
                let base = id.trim_start_matches("Flip");
                self.switch_mode(base, now);
                ctx.request_repaint();
            }
            // 视图操纵（`viewmanip.py`）；Flip 与单选用同一套手势
            "PanViewMode" | "FlipPanViewMode" => {
                self.start_view_tool(ViewTool::Pan, now);
                ctx.request_repaint();
            }
            "ZoomViewMode" | "FlipZoomViewMode" => {
                self.start_view_tool(ViewTool::Zoom, now);
                ctx.request_repaint();
            }
            "RotateViewMode" | "FlipRotateViewMode" => {
                self.start_view_tool(ViewTool::Rotate, now);
                ctx.request_repaint();
            }
            // Inking 节点编辑（`simplify_nodes_cb` / `cull_nodes_cb`）
            "SimplifyNodes" => {
                let n = self.with_mode(|modes, ctx| modes.top_mut().simplify_nodes(ctx));
                self.set_status(format!("简化掉 {n} 个点"), now);
                ctx.request_repaint();
            }
            "CullNodes" => {
                let n = self.with_mode(|modes, ctx| modes.top_mut().cull_nodes(ctx));
                self.set_status(format!("剔除 {n} 个点"), now);
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
            // `RevealScratchpadTool`（resources.xml:1475-1483）的
            // `reveal_dockpanel_cb`：不在就加、在就切到它（不收起）
            "RevealScratchpadTool" => self
                .dock
                .reveal(crate::shell::dock::PanelId::ScratchpadTool),
            "TogglePreviewTool" => self.dock.toggle(crate::shell::dock::PanelId::PreviewTool),
            "ToggleHistoryPanel" => self.dock.toggle(crate::shell::dock::PanelId::HistoryPanel),
            "BrushGroups" => self.dock.toggle(crate::shell::dock::PanelId::BrushGroups),
            // ---- 颜色器（B4-3）----
            // `ComponentSlidersTool`（resources.xml:1433-1438）的 activate 走
            // `toggle_window_cb` → `toggle_dockpanel_cb`（drawwindow.py:364-375）：
            // 动作 active 就 add、取消就 remove —— 与 `Toggle*Tool` 同一套语义。
            "ComponentSlidersTool" => {
                self.dock
                    .toggle(crate::shell::dock::PanelId::ComponentSlidersTool);
                self.color_page = crate::shell::color_panel::PAGE_COMPONENT_SLIDERS;
            }
            // `PaletteTool`（resources.xml:1400-1406）在源码里也是独立工具，
            // 与 Sliders 面板同一套 toggle 语义。本项目共用面板：已经停在
            // 调色板页就收起面板，否则 reveal 并切到调色板页。
            "PaletteTool" => {
                let panel = crate::shell::dock::PanelId::ComponentSlidersTool;
                let on_palette = self.color_page == crate::shell::color_panel::PAGE_PALETTE;
                if self.dock.is_added(panel) && on_palette {
                    self.dock.toggle(panel);
                } else {
                    self.dock.reveal(panel);
                    self.color_page = crate::shell::color_panel::PAGE_PALETTE;
                }
            }
            // `PaletteNext` / `PalettePrev`（`drawwindow.py:659-675`）：
            // 取当前色 → `move_match_position(±1, 当前色)` → 有颜色就 `set_color`；
            // 最后 reveal 调色板工具（"Show the palette panel if hidden"）。
            "PaletteNext" | "PalettePrev" => {
                let direction = if id == "PaletteNext" { 1 } else { -1 };
                self.palette_step(direction);
                self.dock
                    .reveal(crate::shell::dock::PanelId::ComponentSlidersTool);
                self.color_page = crate::shell::color_panel::PAGE_PALETTE;
                ctx.request_repaint();
            }
            // `HCYWheelTool` / `HSVWheelTool` / `HSVSquareTool` / `HSVCubeTool`
            // （`resources.xml:1392-1431`）在源码里也是四个独立工具
            // （`colortools.py:74-96`）。与 `PaletteTool` 同一处置：共用面板，
            // 已停在该页就收起，否则 reveal 并切页。
            "HCYWheelTool" | "HSVWheelTool" | "HSVSquareTool" | "HSVCubeTool" => {
                let page = match id {
                    "HCYWheelTool" => crate::shell::color_panel::PAGE_HCY_WHEEL,
                    "HSVWheelTool" => crate::shell::color_panel::PAGE_HSV_WHEEL,
                    "HSVSquareTool" => crate::shell::color_panel::PAGE_HSV_SQUARE,
                    _ => crate::shell::color_panel::PAGE_HSV_CUBE,
                };
                let panel = crate::shell::dock::PanelId::ComponentSlidersTool;
                if self.dock.is_added(panel) && self.color_page == page {
                    self.dock.toggle(panel);
                } else {
                    self.dock.reveal(panel);
                    self.color_page = page;
                }
            }
            // `mgr.palette.append(color, name=None, unique=True, match=True)`，
            // 然后 reveal 调色板工具。
            "PaletteAddCurrentColor" => {
                self.palette_add_current_color();
                self.dock
                    .reveal(crate::shell::dock::PanelId::ComponentSlidersTool);
                self.color_page = crate::shell::color_panel::PAGE_PALETTE;
                ctx.request_repaint();
            }
            // `ColorAdjusters`（resources.xml:198-204）在源码里是弹菜单的工厂动作
            // （`gui/colortools.py:207-235` 的 `ColorAdjustersToolItem`）。egui 的
            // 菜单不能在动作回调里弹出，这里直接 reveal 已移植的页。
            "ColorAdjusters" => {
                self.dock
                    .reveal(crate::shell::dock::PanelId::ComponentSlidersTool);
                self.set_status("颜色器：六页全部已移植（B4-4c）".to_string(), now);
            }
            // ---- 暂存区（B5）：`gui/drawwindow.py:591-655` 的八个回调 ----
            //
            // 全部作用在**第二份文档**上（`app.scratchpad_doc`），主文档的
            // 撤销栈/文件名一概不动。
            "ScratchNew" => {
                // `new_scratchpad_cb`（drawwindow.py:600-611）
                self.flush_scratch_stroke();
                let bg = self.doc.layers().background_color();
                match self.scratch.new_page(bg) {
                    Ok(from_default) => {
                        let msg = if from_default {
                            "暂存区：已载入默认暂存页".to_string()
                        } else {
                            "暂存区：新建空白页（已复制主文档背景色）".to_string()
                        };
                        self.set_status(msg, now);
                    }
                    Err(e) => self.set_status(format!("暂存区新建失败：{e}"), now),
                }
                ctx.request_repaint();
            }
            "ScratchLoad" => {
                // `load_scratchpad_cb`（drawwindow.py:613-631）：先把当前页存盘，
                // 再开文件对话框（这里是应用内嵌的路径弹层）
                self.flush_scratch_stroke();
                if let Err(e) = self.scratch.save() {
                    self.set_status(format!("暂存区保存失败：{e}"), now);
                }
                let prompt = self.new_file_prompt(
                    FileAction::ScratchpadOpen,
                    Some(&self.scratch.path),
                );
                self.file_prompt = Some(prompt);
            }
            "ScratchSaveNow" => {
                // `save_current_scratchpad_cb`（drawwindow.py:642-643）
                self.flush_scratch_stroke();
                match self.scratch.save() {
                    Ok(wrote) => {
                        let msg = if wrote {
                            format!("暂存区已保存：{}", self.scratch.path.display())
                        } else {
                            "暂存区没有改动，无需保存".to_string()
                        };
                        self.set_status(msg, now);
                    }
                    Err(e) => self.set_status(format!("暂存区保存失败：{e}"), now),
                }
                ctx.request_repaint();
            }
            "ScratchSaveAs" => {
                // `save_as_scratchpad_cb`（drawwindow.py:631-632）
                self.flush_scratch_stroke();
                let prompt = self.new_file_prompt(
                    FileAction::ScratchpadSaveAs,
                    Some(&self.scratch.path),
                );
                self.file_prompt = Some(prompt);
            }
            "ScratchRevert" => {
                // `revert_current_scratchpad_cb`（drawwindow.py:634-640）
                match self.scratch.revert() {
                    Ok(true) => self.set_status(
                        format!("暂存区已回退：{}", self.scratch.path.display()),
                        now,
                    ),
                    Ok(false) => self.set_status(
                        format!("暂存区还没存过盘：{}", self.scratch.path.display()),
                        now,
                    ),
                    Err(e) => self.set_status(format!("暂存区回退失败：{e}"), now),
                }
                ctx.request_repaint();
            }
            "ScratchSaveAsDefault" => {
                // `save_scratchpad_as_default_cb`（drawwindow.py:591-595）
                match self.scratch.save_as_default() {
                    Ok(_) => self.set_status(
                        format!("已存为默认暂存页：{}", self.scratch.path.display()),
                        now,
                    ),
                    Err(e) => self.set_status(format!("默认暂存页保存失败：{e}"), now),
                }
            }
            "ScratchClearDefault" => {
                // `clear_default_scratchpad_cb`（drawwindow.py:597-598）
                let removed = self.scratch.clear_default();
                let msg = if removed {
                    "已删除默认暂存页（下一次「新建」会是空白页）"
                } else {
                    "没有默认暂存页可删"
                };
                self.set_status(msg.to_string(), now);
            }
            "ScratchCopyBackground" => {
                // `scratchpad_copy_background_cb`（drawwindow.py:645-648）
                // 差异记账：源码复制背景**图层**，本项目只有背景色
                let bg = self.doc.layers().background_color();
                self.scratch.copy_background(bg);
                self.set_status("已把主文档的背景色复制到暂存区".to_string(), now);
                ctx.request_repaint();
            }
            // ---- 窗口 ----
            "Fullscreen" => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            "Quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            // ---- 偏好窗口（B6-3）----
            // `PreferencesWindow`（`resources.xml:1330-1335`，label
            // "Edit Preferences"，handler `toggle_window_cb`）是
            // `GtkToggleAction`：动作的 active 状态 = 窗口可见性，按下就是
            // 开/关（`drawwindow.py:364-388`；窗口被藏起来时
            // `_subwindow_hide_cb` 会把动作取消勾选，`application.py:752-756`）。
            "PreferencesWindow" => {
                self.prefs_window.open = !self.prefs_window.open;
                if !self.prefs_window.open {
                    // 窗口藏起来时捕获对话框也收（见 prefs_window 的关窗语义）
                    self.key_editor.capture = None;
                }
                ctx.request_repaint();
            }
            // ---- 笔刷快捷键是否连颜色一起还原（`ContextRestoreColor`，按键 `）----
            // 源码 `context_toggle_color_cb`（`document.py:1721-1724`）只是写偏好
            // `misc.context_restores_color`；开关由管理器持有（默认 True，
            // `userconfig.py:93`）。
            "ContextRestoreColor" => {
                let value = self.color_manager.toggle_context_restores_color();
                let state = if value { "开" } else { "关" };
                self.set_status(format!("恢复笔刷槽位时连颜色一起还原：{state}"), now);
            }
            // `ContextStore`（`q`）：存进 `selected_context`；没有则忽略
            "ContextStore" => match self.selected_context {
                Some(i) => self.store_brushkey(i, now),
                None => self.set_status("没有选中的笔刷槽位".to_string(), now),
            },
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
            // 笔刷尺寸拖拽（`gui/brushmanip.py` 的 `BrushResizeMode`）。
            // 源码经按钮映射进入，本应用暂用本地键（见 `begin_brush_resize`）。
            "pigment:brush-resize" => self.begin_brush_resize(now),
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

        // 暂存区面板关掉时存盘（`scratchwindow.py:57-58` 的 destroy/delete-event）
        let now = ui.input(|i| i.time);
        self.save_scratchpad_on_close(now);

        // 文件路径弹层放最后画（浮在最上层）。它打开时输入框会拿到键盘焦点，
        // 于是 `handle_keys` 的 `text_edit_focused()` 早退会顺带挡住所有快捷键。
        //
        // 偏好窗口（B6-3）在它之前：窗口里的 Keys 页自带按键捕获对话框
        // （`egui::Modal`），与文件弹层不会同时打开（捕获时 `handle_keys` 直接早退）。
        self.preferences_window(&ctx);
        self.show_file_prompt(&ctx, now);
        if self.file_prompt.is_some() {
            ctx.request_repaint();
        }
    }

    /// 退出时保存配置 —— `quit_cb` 里的 `app.save_gui_config()`
    /// （`gui/drawwindow.py:688-690`）。
    ///
    /// 源码在 `quit_cb` 的**确认对话框之前**保存（`:690`），即使最后取消了退出
    /// 也写盘；eframe 侧 `on_exit` 在窗口真正关闭时调用一次，语义相当。
    fn on_exit(&mut self) {
        self.save_gui_config();
    }
}
// 测试：偏好接线的纯函数侧（B6-1）。PigmentApp 本体要 GPU，构造不了；
// 这里只测不需要 eframe 的那几层。

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份"什么都没有"的偏好：默认表 + 全部出厂值，不读真实 settings.json。
    fn fresh_prefs(tag: &str) -> Prefs {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-app-prefs-{}-{tag}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Prefs::load(p)
    }

    #[test]
    fn mode_prefs_reads_hide_cursor_from_the_prefs() {
        // gui/freehand.py:280 / preferenceswindow.py:202：软键，默认 false
        let mut prefs = fresh_prefs("cursor");
        assert!(!ModePrefs::from_prefs(&prefs).hide_cursor_while_painting);
        prefs.set_bool(prefs::KEY_HIDE_CURSOR_WHILE_PAINTING, true);
        let mp = ModePrefs::from_prefs(&prefs);
        assert!(mp.hide_cursor_while_painting);
        // 假输入不是偏好项（application.py:299-300 写死 0.5）
        assert!((mp.fakepressure - 0.5).abs() < 1e-6);
        assert!((mp.fakerotation - 0.5).abs() < 1e-6);
        let _ = std::fs::remove_file(prefs.path());
    }

    #[test]
    fn mode_prefs_ignores_a_type_error_and_keeps_the_source_default() {
        // 类型不对 → 回落 false（源码 .get(key, False) 的保守版）。
        // 直接从磁盘读一份坏 settings.json（app 侧没有 serde_json 依赖）。
        let dir = std::env::temp_dir().join(format!(
            "pigment-app-badprefs-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"ui.hide_cursor_while_painting": "yes"}"#).unwrap();
        let prefs = Prefs::load(&path);
        assert!(!ModePrefs::from_prefs(&prefs).hide_cursor_while_painting);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_extension_follows_saving_default_format() {
        // userconfig.py:89（出厂 openraster）+ filehandling.py:598-602
        let mut prefs = fresh_prefs("ext");
        assert_eq!(prefs::save_extension(&prefs), ".ora");
        prefs.set_str(prefs::KEY_SAVING_DEFAULT_FORMAT, "png-solid");
        assert_eq!(prefs::save_extension(&prefs), ".png");
        prefs.set_str(prefs::KEY_SAVING_DEFAULT_FORMAT, "jpeg-90%");
        assert_eq!(prefs::save_extension(&prefs), ".jpg");
        let _ = std::fs::remove_file(prefs.path());
    }

    #[test]
    fn file_prompt_default_name_uses_the_document_extension() {
        // 默认文件名 + 提交时补扩展名都跟偏好走（导出恒 .png）
        let prompt = FilePrompt::new(FileAction::SaveAs, None, ".png");
        assert!(prompt.path.replace('\\', "/").ends_with("untitled.png"), "{}", prompt.path);
        assert_eq!(prompt.ext, ".png");
        let prompt = FilePrompt::new(FileAction::Export, None, ".ora");
        assert!(prompt.path.replace('\\', "/").ends_with("untitled.png"), "导出恒 png");
        assert_eq!(prompt.ext, ".png");

        // 有 seed 就原样用 seed 的路径
        let seed = std::path::Path::new(r"D:\paint\a.ora");
        let prompt = FilePrompt::new(FileAction::SaveAs, Some(seed), ".png");
        assert_eq!(prompt.path, r"D:\paint\a.ora");
    }

    #[test]
    fn submit_extends_only_when_there_is_no_extension() {
        // 有扩展名就尊重用户写的（`.jpg` / `.ora` 都不改）
        assert_eq!(
            with_default_extension(FileAction::SaveAs, "noext", ".png"),
            "noext.png"
        );
        assert_eq!(
            with_default_extension(FileAction::SaveAs, "mine.ora", ".png"),
            "mine.ora"
        );
        assert_eq!(
            with_default_extension(FileAction::Export, "shot", ".ora"),
            "shot.png",
            "导出恒 .png（Export 走 PNG，不看 saving.default_format）"
        );
        assert_eq!(action_extension(FileAction::Export, ".ora"), ".png");
        assert_eq!(action_extension(FileAction::ScratchpadSaveAs, ".jpeg"), ".jpeg");
    }

    #[test]
    fn save_gui_config_writes_the_color_and_scratchpad_keys() {
        // application.py:624-628 的数据侧：颜色五条键 + 暂存页路径
        let dir = std::env::temp_dir().join(format!(
            "pigment-app-savecfg-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut prefs = Prefs::load(dir.join("settings.json"));
        // 拿不到 PigmentApp，就分别验证两段的输入/输出：颜色管理器写回 + 路径写入
        let mgr = ColorManager::new();
        mgr.store_prefs(&mut prefs);
        assert_eq!(
            prefs.get_str(pigment_engine::color::PREFS_KEY_CURRENT_COLOR),
            Some("#003153")
        );
        prefs.set_str(prefs::KEY_LAST_OPENED_SCRATCHPAD, r"D:\scrap\page.ora");
        prefs.save().unwrap();
        let reloaded = Prefs::load(dir.join("settings.json"));
        assert_eq!(
            reloaded.str_or(prefs::KEY_LAST_OPENED_SCRATCHPAD, ""),
            r"D:\scrap\page.ora"
        );
        assert_eq!(
            reloaded.get_str(pigment_engine::color::PREFS_KEY_CURRENT_COLOR),
            Some("#003153")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_files_dedupe_and_cap_at_twenty() {
        let mut list = Vec::new();
        push_recent(&mut list, "a.ora");
        push_recent(&mut list, "b.ora");
        push_recent(&mut list, "a.ora");
        assert_eq!(list, vec!["a.ora".to_string(), "b.ora".to_string()]);
        for i in 0..30 {
            push_recent(&mut list, &format!("{i}.ora"));
        }
        assert_eq!(list.len(), 20);
        assert_eq!(list[0], "29.ora");
    }

    #[test]
    fn ink_button_layout_keeps_buttons_in_bounds() {
        let fixed = [
            LayoutNode::new(100.0, 100.0),
            LayoutNode::new(200.0, 200.0),
            LayoutNode::new(300.0, 300.0),
        ];
        let (accept, reject) = PigmentApp::layout_ink_buttons(&fixed, 800.0, 600.0).unwrap();
        let margin = 24.0;
        for p in [accept, reject] {
            assert!(p.x >= margin - 1e-3 && p.x <= 800.0 - margin + 1e-3);
            assert!(p.y >= margin - 1e-3 && p.y <= 600.0 - margin + 1e-3);
        }
        assert!(accept.distance(reject) > 1.0, "两个按钮不能重叠");
        // 尾部方向 (1,1)/√2：OK 在末点外、垃圾桶在首点内
        assert!(accept.x > 300.0 && accept.y > 300.0);
        assert!(reject.x < 100.0 && reject.y < 100.0);
    }

    #[test]
    fn view_tool_drag_matches_viewmanip_math() {
        // Pan：画布跟手（tdw.scroll(-dx,-dy)）
        let mut v = View::default();
        PigmentApp::apply_view_drag(
            &mut v,
            ViewTool::Pan,
            egui::pos2(10.0, 10.0),
            egui::pos2(30.0, 25.0),
            egui::vec2(100.0, 100.0),
        );
        assert_eq!(v.offset, [20.0, 15.0]);

        // Zoom：以指针为锚，factor = exp(dy/100)
        let mut v = View::default();
        let anchor = egui::pos2(40.0, 30.0);
        PigmentApp::apply_view_drag(
            &mut v,
            ViewTool::Zoom,
            egui::pos2(40.0, 20.0),
            anchor,
            egui::vec2(100.0, 100.0),
        );
        assert!((v.zoom - 0.1f32.exp()).abs() < 1e-5, "zoom={}", v.zoom);

        // Rotate：绕视图中心，从右到下的 90°
        let mut v = View::default();
        PigmentApp::apply_view_drag(
            &mut v,
            ViewTool::Rotate,
            egui::pos2(200.0, 100.0),
            egui::pos2(100.0, 200.0),
            egui::vec2(100.0, 100.0),
        );
        assert!((v.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }
}