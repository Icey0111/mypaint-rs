//! 暂存区（Scratchpad）—— **第二份文档 + 它自己的模式栈**。
//!
//! # 源码结论（先搞清"暂存区是什么"）
//!
//! 1. **它是一个独立的文档**。`gui/application.py:361-369` 给暂存区单建一份
//!    `lib.document.Document`（`painting_only=True`，缓存只有主文档的 1/4）
//!    和一个独立的 `TiledDrawWidget`，再包一个 `gui.document.Document`
//!    控制器（`self.scratchpad_doc`）。主文档是另一套
//!    （`application.py:349-355`）。
//! 2. **它有自己的撤销栈**。命令栈属于 `lib.document.Document`
//!    （`lib/document.py:309`：`self.command_stack = command.CommandStack(...)`，
//!    栈深同样取 `DEFAULT_UNDO_STACK_SIZE`=40，`:59`），所以暂存区的每一笔
//!    只进暂存区自己的栈。
//! 3. **它有自己的模式栈**。`gui/document.py:78-88` 的 `CanvasController.__init__`
//!    每个控制器各建一条 `ModeStack`（`:87`），默认模式 `FreehandMode`（`:88`）。
//!    所以"每个 TDW 有自己的 mode stack"成立。
//! 4. **共享的是笔刷与颜色**：暂存区模型用的是**同一个** `self.brush` 对象
//!    （`application.py:350` 与 `:362` 传的是同一个 `self.brush`），
//!    颜色归 `app.brush_color_manager` 独一份（`application.py:399`）。
//!    所以在暂存区上取色会改主画布的画笔颜色，反之亦然。
//! 5. **但它没有自己的动作组**：`gui/document.py:349-365` 里只有"主控制器"
//!    （`get_primary_instance()`，`:262-268`）才 `_init_actions`；暂存区控制器
//!    走 `self.action_group = leader.action_group`（`:351-354`）。
//!    因此全局的 `Undo`/`Redo` 动作回调**始终作用在主文档**上
//!    （`document.py:552-558` 的 `undo_cb` 是主控制器的绑定方法）。
//!    暂存区的撤销**在源码里没有动作入口** —— 本移植在面板里补两个按钮，
//!    见 `shell/scratch_panel.rs` 的说明（差异记账）。
//!
//! # 与源码的对应关系
//!
//! | 源码 | 这里 |
//! |---|---|
//! | `app.scratchpad_doc.model` | [`Scratchpad::doc`] |
//! | `app.scratchpad_doc.modes` | [`Scratchpad::modes`] |
//! | `app.scratchpad_doc.tdw.scale` / 视图位置 | [`Scratchpad::view`] |
//! | `app.scratchpad_filename` | [`Scratchpad::path`] |
//! | `filehandler.get_scratchpad_prefix()` | [`Scratchpad::dir`] |
//! | `scratchpad_doc.reset_view(True, True, True)` | [`Scratchpad::reset_view`] |
//!
//! 文件层的规则（目录/文件名/存盘判据）在 `pigment_engine::scratchpad`。

use std::path::{Path, PathBuf};

use pigment_engine::hokusai::{Brush, BrushState};
use pigment_engine::mode::{ModeContext, ModeStack};
use pigment_engine::prefs::{self, Prefs};
use pigment_engine::scratchpad as files;
use pigment_engine::{Document, FileError};

use crate::view::View;

/// 一次模式调用要用到的"共享输入"（笔刷与颜色都是应用级的，暂存区不持有）。
///
/// 对应源码里模式通过 `self.doc.app` 拿到的东西：`brush` 是**同一个对象**
/// （`application.py:350,362`），颜色归 `brush_color_manager`
/// （`application.py:399`）。
#[derive(Clone, Copy)]
pub(crate) struct ModeInputs<'a> {
    pub(crate) brush: &'a Brush,
    pub(crate) brush_name: &'a str,
    pub(crate) fakepressure: f32,
    pub(crate) fakerotation: f32,
    pub(crate) hide_cursor_while_painting: bool,
    /// 当前画笔颜色（HSV），即 `brush_color_manager.get_color()` 的投影。
    pub(crate) color: [f32; 3],
}

/// 在给定的 (文档, 模式栈, 笔刷状态) 上跑一次模式调用。
///
/// 主画布（[`crate::app::PigmentApp::with_mode`]）与暂存区
/// （[`Scratchpad::dispatch`]）共用这一份借用拆分 —— 它们唯一的区别是
/// 拿哪一份 `Document`/`ModeStack`/`BrushState`。
///
/// 返回 `(闭包结果, 取色模式写回的颜色)`：取色模式（`gui/colorpicker.py`）
/// 把新颜色写进 `picked_color` 槽，由调用方落到颜色管理器
/// （`BrushColorManager.set_color()`，`gui/brushcolor.py:33-37`）。
pub(crate) fn mode_call<R>(
    doc: &mut Document,
    modes: &mut ModeStack,
    brush_state: &mut BrushState,
    inputs: ModeInputs<'_>,
    f: impl FnOnce(&mut ModeStack, &mut ModeContext<'_>) -> R,
) -> (R, Option<[f32; 3]>) {
    let mut picked: Option<[f32; 3]> = None;
    let r = {
        let mut ctx = ModeContext {
            doc,
            brush: inputs.brush,
            brush_name: inputs.brush_name,
            brush_state,
            fakepressure: inputs.fakepressure,
            fakerotation: inputs.fakerotation,
            hide_cursor_while_painting: inputs.hide_cursor_while_painting,
            status: None,
            color: inputs.color,
            picked_color: Some(&mut picked),
        };
        f(modes, &mut ctx)
    };
    (r, picked)
}

/// 暂存区状态：第二份文档 + 独立模式栈 + 独立视图 + 它自己的文件名。
pub(crate) struct Scratchpad {
    /// 独立文档（`app.scratchpad_doc.model`）：自己的图层栈、自己的撤销栈。
    pub(crate) doc: Document,
    /// 独立模式栈（`document.py:87`）：每个 `CanvasController` 一条。
    pub(crate) modes: ModeStack,
    /// 独立的笔刷引擎状态（每次起笔重置，与主画布互不干扰）。
    pub(crate) stroke_state: BrushState,
    /// 独立视图（`app.scratchpad_doc.tdw`）：缩放/平移与主画布无关。
    pub(crate) view: View,
    /// 暂存区目录（`filehandler.get_scratchpad_prefix()`）。
    pub(crate) dir: PathBuf,
    /// 当前暂存页的路径（`app.scratchpad_filename`）。
    ///
    /// 初值 = `autosave.ora`；启动时若偏好 `scratchpad.last_opened_scratchpad`
    /// 里有路径就用它（[`Scratchpad::startup_page`]，
    /// `gui/application.py:393-397` / `:474-486`）。
    pub(crate) path: PathBuf,
    /// 下一帧强制重传显存（换文档/撤销后残留 tile 的清理）。
    pub(crate) force_resync: bool,
    /// 本帧有笔迹变化，需要持续重绘。
    pub(crate) stroke_dirty: bool,
    /// 本帧计划上传的 tile 数（命中上限说明还有欠账）。
    pub(crate) pending_uploads: usize,
}

impl Scratchpad {
    /// 建一个空白暂存区（`application.py:361-369`）。
    ///
    /// 撤销栈深用 `Document::new()` 的默认值 40 —— 与源码一致
    /// （`lib/document.py:59` 的 `DEFAULT_UNDO_STACK_SIZE`；暂存区模型没有
    /// 传 `max_undo_stack_size`，所以用同一个默认）。
    ///
    /// 顺带把目录建出来：源码启动时建 `scratchpads`（`application.py:232-236`），
    /// 保存前还会再兜底建一次（`filehandling.py:1363-1366`）。
    pub(crate) fn new(dir: PathBuf) -> Self {
        if let Err(e) = files::ensure_dir(&dir) {
            eprintln!("暂存区目录建不出来（{}）：{e}", dir.display());
        }
        let path = files::autosave_path(&dir);
        let mut doc = Document::new();
        // 源码把主文档的背景层直接给暂存区（`drawwindow.py:655`）；
        // 这里没有引用主文档，先统一用默认纸纹。
        doc.reset_background();
        Self {
            doc,
            modes: ModeStack::new(|| Box::new(pigment_engine::mode::freehand::FreehandMode::new())),
            stroke_state: BrushState::default(),
            view: View::default(),
            dir,
            path,
            force_resync: true,
            stroke_dirty: false,
            pending_uploads: 0,
        }
    }

    /// 在给定的 (模式栈, 上下文) 上跑一段逻辑，并把取色结果交给调用方落地。
    ///
    /// 与 [`crate::app::PigmentApp::with_mode`] 同构，只是换了一份
    /// `doc`/`modes`/`stroke_state`。
    pub(crate) fn dispatch<R>(
        &mut self,
        inputs: ModeInputs<'_>,
        f: impl FnOnce(&mut ModeStack, &mut ModeContext<'_>) -> R,
    ) -> (R, Option<[f32; 3]>) {
        mode_call(
            &mut self.doc,
            &mut self.modes,
            &mut self.stroke_state,
            inputs,
            f,
        )
    }

    /// 处理模式排队的输入（`GLib.idle_add` 的等价物：每帧一次）。
    #[cfg(test)]
    fn process_pending(&mut self, inputs: ModeInputs<'_>) {
        let _ = self.dispatch(inputs, |modes, ctx| modes.process_pending(ctx));
    }

    /// 视图复位 —— `document.py:976` 的 `self.app.scratchpad_doc.reset_view(True, True, True)`。
    ///
    /// 源码那三个 True = 居中 / 复位缩放 / 复位旋转，缩放取偏好
    /// `view.default_zoom`（`document.py:339-340`，出厂 1.0）。复位缩放的偏好
    /// 接线未做（记账：B6-1 只接了暂存页路径 `scratchpad.last_opened_scratchpad`）。
    /// 另外暂存区的 TDW 是 `scroll_on_allocate = False`
    /// （`application.py:367`），所以不做"加载后居中"那一步。
    pub(crate) fn reset_view(&mut self) {
        self.view = View::default();
    }

    // ---- 文件（`gui/filehandling.py` 的 open_scratchpad / save_scratchpad）----

    /// 从文件载入暂存页 —— `filehandling.py:956-976` 的 `open_scratchpad`。
    ///
    /// 与源码一致：`model.load()` 会先 `clear()`，失败时暂存区就空了
    /// （源码不做回滚，主文档的 `open_file` 也不做）。
    pub(crate) fn open(&mut self, path: &Path) -> Result<(), FileError> {
        self.doc.load(path)?;
        self.path = abspath(path);
        self.force_resync = true;
        self.reset_view();
        Ok(())
    }

    /// 启动时载回上次的暂存页 —— `gui/application.py:393-397` + `:474-486`。
    ///
    /// 源码两步：
    /// 1. `__init__` 里（`:393-397`）—— 偏好 `scratchpad.last_opened_scratchpad`
    ///    缺失/为空就写回 `get_scratchpad_autosave()`（`autosave.ora`），再把它
    ///    记成 `app.scratchpad_filename`；
    /// 2. `_at_application_start` 里（`:474-486`）—— 读同一个键，文件存在就
    ///    `open_scratchpad()`（成功时把**绝对路径**写回偏好，`filehandling.py:965-974`）。
    ///
    /// 返回是否真的载入了一份暂存页。
    pub(crate) fn startup_page(&mut self, prefs: &mut Prefs) -> bool {
        // `:393-397`
        let mut name = prefs
            .str_or(prefs::KEY_LAST_OPENED_SCRATCHPAD, "")
            .to_string();
        if name.is_empty() {
            let autosave = abspath(&files::autosave_path(&self.dir));
            name = autosave.display().to_string();
            prefs.set_str(prefs::KEY_LAST_OPENED_SCRATCHPAD, name.clone());
        }
        self.path = PathBuf::from(&name);
        // `:481-485`
        let path = self.path.clone();
        if !path.is_file() {
            return false;
        }
        match self.open(&path) {
            Ok(()) => {
                // `open_scratchpad` 成功时写回绝对路径（`filehandling.py:965-974`）
                let abs = abspath(&self.path);
                self.path = abs.clone();
                prefs.set_str(
                    prefs::KEY_LAST_OPENED_SCRATCHPAD,
                    abs.display().to_string(),
                );
                true
            }
            Err(e) => {
                eprintln!("暂存区载入失败（{}）：{e}", path.display());
                false
            }
        }
    }

    /// 存到当前暂存页 —— `filehandling.py:1017-1036` 的 `save_scratchpad`。
    ///
    /// 返回是否真的写了盘（`save_needed` 为假时什么都不做）。
    pub(crate) fn save(&mut self) -> Result<bool, FileError> {
        let path = self.path.clone();
        let wrote = files::save_scratchpad(&mut self.doc, &path, false)?;
        // `:1032-1036`：export=False 时把（已经 abspath 的）文件名记回来
        self.path = abspath(&path);
        Ok(wrote)
    }

    /// 另存到指定路径 —— `filehandling.py:1256-1266` 的 `save_scratchpad_as_dialog`
    /// 提交之后做的事（`save_scratchpad(filename, export=False)`）。
    pub(crate) fn save_as(&mut self, path: &Path) -> Result<(), FileError> {
        files::save_scratchpad_as(&mut self.doc, path)?;
        self.path = abspath(path);
        Ok(())
    }

    /// 存成"默认暂存页" —— `drawwindow.py:591-595` 的 `save_scratchpad_as_default_cb`
    /// → `filehandler.save_scratchpad(get_scratchpad_default())`。
    ///
    /// **注意源码的副作用**：`save_scratchpad` 里 `export=False` 分支会把
    /// `app.scratchpad_filename` 改成刚存的文件名（`filehandling.py:1032-1036`），
    /// 所以"存成默认"之后当前暂存页也变成 `scratchpad_default.ora` 了。
    /// 这是源码行为（看着像 bug），本移植照搬并在此标明。
    pub(crate) fn save_as_default(&mut self) -> Result<bool, FileError> {
        let path = files::default_path(&self.dir);
        let wrote = files::save_scratchpad(&mut self.doc, &path, false)?;
        self.path = abspath(&path);
        Ok(wrote)
    }

    /// 删掉默认暂存页文件 —— `drawwindow.py:597-598` 的 `clear_default_scratchpad_cb`
    /// → `filehandler.delete_default_scratchpad()`（`filehandling.py:1599-1602`）。
    pub(crate) fn clear_default(&mut self) -> bool {
        files::delete_default(&self.dir)
    }

    /// 回退到上次存盘的版本 —— `drawwindow.py:634-640` 的
    /// `revert_current_scratchpad_cb`：文件存在才重载。
    pub(crate) fn revert(&mut self) -> Result<bool, FileError> {
        let path = self.path.clone();
        if !path.is_file() {
            return Ok(false);
        }
        self.open(&path)?;
        Ok(true)
    }

    /// 新开一页 —— `drawwindow.py:600-611` 的 `new_scratchpad_cb`：
    ///
    /// ```python
    /// default_scratchpad_path = app.filehandler.get_scratchpad_default()
    /// if os.path.isfile(default_scratchpad_path):
    ///     app.filehandler.open_scratchpad(default_scratchpad_path)
    /// else:
    ///     scratchpad_model.clear()
    ///     self._copy_main_background_to_scratchpad()
    /// scratchpad_path = app.filehandler.get_scratchpad_autosave()
    /// app.scratchpad_filename = scratchpad_path
    /// ```
    ///
    /// 返回是否是"从默认页载入"（否则是清空 + 复制背景）。
    pub(crate) fn new_page(&mut self, main_background: [u8; 3]) -> Result<bool, FileError> {
        let default = files::default_path(&self.dir);
        let from_default = if default.is_file() {
            self.open(&default)?;
            true
        } else {
            self.doc.clear();
            self.copy_background(main_background);
            false
        };
        // 无论哪条分支，当前文件名都回到 autosave（`:609-611`）
        self.path = files::autosave_path(&self.dir);
        self.force_resync = true;
        Ok(from_default)
    }

    /// 把主文档的背景复制到暂存区 —— `drawwindow.py:645-655` 的
    /// `scratchpad_copy_background_cb`。
    ///
    /// 源码复制的是背景**图层**（`insert_layer` 一份 `BackgroundLayer`）；
    /// 本项目的背景只有"颜色"这一种形态（图片背景属 B2 记账项），
    /// 所以这里复制背景色。差异记账：图片背景不会被复制过来。
    pub(crate) fn copy_background(&mut self, rgb: [u8; 3]) {
        self.doc.layers_mut().set_background_color(rgb);
        self.force_resync = true;
    }

    // ---- 撤销 / 重做（作用在暂存区自己的栈上）----

    /// 撤销一笔（只影响暂存区）。
    pub(crate) fn undo(&mut self) -> Option<String> {
        let name = self.doc.undo();
        if name.is_some() {
            self.force_resync = true;
        }
        name
    }

    /// 重做一笔（只影响暂存区）。
    pub(crate) fn redo(&mut self) -> Option<String> {
        let name = self.doc.redo();
        if name.is_some() {
            self.force_resync = true;
        }
        name
    }

    /// 把一串指针事件喂给暂存区的模式栈（一次完整的按下-移动-松开）。
    ///
    /// **只给测试用**：面板的实际输入走 [`crate::app::PigmentApp`] 的
    /// `scratch_mode_press`/`_motion`/`_release`（它们是逐事件增量投递的）。
    /// 这里把三段合成一次调用，事件序列与 `shell/workspace.rs` 的画布分支
    /// 完全同构（press → motion* → release → process_pending）。
    #[cfg(test)]
    pub(crate) fn stroke(&mut self, inputs: ModeInputs<'_>, points: &[(f32, f32, f64)]) -> bool {
        let Some((&(x0, y0, t0), rest)) = points.split_first() else {
            return false;
        };
        let mut handled = false;
        let _ = self.dispatch(inputs, |modes, ctx| {
            handled = modes.top_mut().button_press(ctx, &pigment_engine::mode::PointerEvent::button(x0, y0, t0, 1));
        });
        self.process_pending(inputs);
        for &(x, y, t) in rest {
            let _ = self.dispatch(inputs, |modes, ctx| {
                modes.top_mut().motion(ctx, &pigment_engine::mode::PointerEvent::motion(x, y, t));
            });
            self.process_pending(inputs);
        }
        let &(x1, y1, t1) = points.last().unwrap();
        let _ = self.dispatch(inputs, |modes, ctx| {
            modes.top_mut().button_release(ctx, &pigment_engine::mode::PointerEvent::button(x1, y1, t1, 1));
        });
        self.process_pending(inputs);
        handled
    }
}

/// 把文件名补成绝对路径（照 `os.path.abspath`，`filehandling.py:933`）——
/// 与 [`crate::app`] 里的同名工具同义；这里单独一份是因为 app.rs 的那份是
/// 私有的。
fn abspath(p: &Path) -> PathBuf {
    if p.is_absolute() {
        return p.to_path_buf();
    }
    std::env::current_dir()
        .map(|d| d.join(p))
        .unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    use pigment_engine::hokusai::BrushSetting;

    /// 一份"什么都没有"的偏好（默认表 + 全部默认值，不读真实 settings.json）。
    fn fresh_prefs(dir: &Path) -> Prefs {
        Prefs::load(dir.join("no-settings.json"))
    }

    /// 每个测试一个干净目录。
    fn tmpdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-app-scratch-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        files::ensure_dir(&p).unwrap();
        p
    }

    /// 一支能出墨的笔（半径 1.0）。
    fn test_brush() -> Brush {
        let mut b = pigment_engine::brushlib::default_brush();
        b.get_mut(BrushSetting::Radius).base_value = 1.0;
        b
    }

    /// 造一份模式输入（笔刷 + 默认颜色）。
    fn inputs(brush: &Brush) -> ModeInputs<'_> {
        ModeInputs {
            brush,
            brush_name: "Test Brush",
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            color: [0.0, 0.0, 0.0],
        }
    }

    /// 一条横穿两块 tile 的笔画（press → 五个 motion → release）。
    ///
    /// 两个"必须这样写"的细节（都实测过）：
    /// - 时间戳从 1000ms 起（不是 0）：`FreehandMode::process_queued_event`
    ///   把 `last_handled_event_time == 0.0` 当"还没有时间基准"
    ///   （`freehand.py:550` 的第一个事件只定基准）；从 0 起会让前两个事件
    ///   都被当成基准吞掉。
    /// - 至少三四个 motion：hokusai 的笔刷引擎对**全新**的 `BrushState`
    ///   第一个 `stroke_to` 只做初始化（不落 dab），随后才是真正的笔迹。
    const STROKE: &[(f32, f32, f64)] = &[
        (8.0, 8.0, 1000.0),
        (16.0, 16.0, 1016.0),
        (32.0, 32.0, 1032.0),
        (48.0, 48.0, 1048.0),
        (64.0, 64.0, 1064.0),
        (100.0, 100.0, 1080.0),
    ];

    #[test]
    fn creating_the_scratchpad_makes_its_directory() {
        // application.py:232-236 / filehandling.py:1363-1366
        let root = tmpdir("mkdir");
        let dir = root.join("deep").join("scratchpads");
        assert!(!dir.exists());
        let mut sp = Scratchpad::new(dir.clone());
        assert!(dir.is_dir(), "构造时就该把目录建出来（否则第一次保存会失败）");
        assert!(sp.save().unwrap(), "目录刚建好，这次保存要成功");
        assert!(sp.path.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn starts_on_the_autosave_page() {
        // application.py:393-397（偏好缺失时的默认值）+ filehandling.py:1445-1447
        let dir = tmpdir("starts");
        let sp = Scratchpad::new(dir.clone());
        assert_eq!(sp.path, dir.join("autosave.ora"));
        assert_eq!(sp.view, View::default(), "缩放/偏移与主画布无关，各自一份");
        assert_eq!(sp.modes.len(), 1, "默认只有 FreehandMode（document.py:87-88）");
        assert_eq!(sp.modes.top().name(), "Freehand Drawing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn painting_lands_in_the_scratchpad_undo_stack_only() {
        // 语义验收：独立文档 + 独立撤销栈（lib/document.py:309、document.py:87）
        let dir = tmpdir("undo");
        let mut sp = Scratchpad::new(dir.clone());
        let main = Document::new();
        let brush = test_brush();
        sp.stroke(inputs(&brush), STROKE);

        assert_eq!(sp.doc.command_stack.undo_len(), 1, "暂存区自己记一笔");
        assert_eq!(main.command_stack.undo_len(), 0, "主文档的撤销栈不许被碰");
        assert!(
            sp.doc.layers().has_paint_at(0, 0),
            "暂存区上真的画上了（tile 0,0）"
        );
        assert!(
            !main.layers().has_paint_at(0, 0),
            "主文档表面必须干净（两份文档互不影响）"
        );
        assert!(sp.doc.unsaved_painting_time > 0.0, "画过 → 有未保存时长");

        // 撤销只退暂存区
        assert!(sp.undo().is_some());
        assert_eq!(sp.doc.command_stack.undo_len(), 0);
        assert!(!sp.doc.layers().has_paint_at(0, 0));
        assert!(sp.redo().is_some(), "重做同样只作用在暂存区");
        assert!(sp.doc.layers().has_paint_at(0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_skips_writing_when_nothing_changed() {
        // filehandling.py:1019-1023：没画过 + 文件已存在 → 不写
        let dir = tmpdir("save-needed");
        let mut sp = Scratchpad::new(dir.clone());
        // 第一次：文件不存在 → 写
        assert!(sp.save().unwrap());
        assert!(sp.path.is_file());
        // 第二次：已存在且没画过 → 不写
        assert!(!sp.save().unwrap());
        // 画一笔后 → 写，并把 unsaved_painting_time 归零
        let brush = test_brush();
        sp.stroke(inputs(&brush), STROKE);
        assert!(sp.doc.unsaved_painting_time > 0.0);
        assert!(sp.save().unwrap());
        assert_eq!(sp.doc.unsaved_painting_time, 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_as_repoints_the_current_page_and_reloads() {
        // filehandling.py:1032-1036（export=False 时改写当前文件名）+ 956-976
        let dir = tmpdir("save-as");
        let mut sp = Scratchpad::new(dir.clone());
        let brush = test_brush();
        sp.stroke(inputs(&brush), STROKE);
        let target = dir.join("my_page.ora");
        sp.save_as(&target).unwrap();
        assert_eq!(sp.path, target.clone(), "另存为要把当前页指过去");
        assert!(target.is_file());

        // 读回：另在一份新文档里载入，内容还在
        let mut other = Scratchpad::new(dir.clone());
        other.open(&target).unwrap();
        assert_eq!(other.path, target);
        assert!(
            other.doc.layers().has_paint_at(0, 0),
            "保存后必须能读回内容"
        );
        // 打开会清空撤销栈（lib/document.py:1554-1555）
        assert_eq!(other.doc.command_stack.undo_len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_page_prefers_the_default_file_then_falls_back_to_a_blank_page() {
        // drawwindow.py:600-611
        let dir = tmpdir("new-page");
        let mut sp = Scratchpad::new(dir.clone());
        let brush = test_brush();

        // 没有默认页 → 清空 + 复制主文档背景色
        sp.stroke(inputs(&brush), STROKE);
        assert!(sp.doc.layers().has_paint_at(0, 0));
        assert!(!sp.new_page([10, 20, 30]).unwrap());
        assert!(!sp.doc.layers().has_paint_at(0, 0), "清空成空白页");
        assert_eq!(sp.doc.layers().background_color(), [10, 20, 30]);
        assert_eq!(sp.path, dir.join("autosave.ora"), "新页总是存到 autosave");

        // 有默认页 → 载入它（而不是清空）
        let mut src = Scratchpad::new(dir.clone());
        src.stroke(inputs(&brush), STROKE);
        src.save_as(&files::default_path(&dir)).unwrap();
        assert!(sp.new_page([0, 0, 0]).unwrap(), "这次走的是载入分支");
        assert!(sp.doc.layers().has_paint_at(0, 0), "默认页的内容被载入");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_as_default_also_becomes_the_current_page() {
        // drawwindow.py:591-595 → filehandling.py:1018-1036 的已知副作用
        let dir = tmpdir("as-default");
        let mut sp = Scratchpad::new(dir.clone());
        let brush = test_brush();
        sp.stroke(inputs(&brush), STROKE);
        assert!(sp.save_as_default().unwrap());
        assert_eq!(sp.path, files::default_path(&dir));
        assert!(sp.path.is_file());
        // 清默认 = 删那个文件（drawwindow.py:597-598）
        assert!(sp.clear_default());
        assert!(!files::default_path(&dir).exists());
        assert!(!sp.clear_default(), "文件没了就返回 false");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_reloads_from_disk_and_keeps_the_page_selected() {
        // drawwindow.py:634-640
        let dir = tmpdir("revert");
        let mut sp = Scratchpad::new(dir.clone());
        let brush = test_brush();
        sp.stroke(inputs(&brush), STROKE);
        sp.save().unwrap();
        let saved_path = sp.path.clone();
        // 再画一笔（未保存），然后回退
        sp.stroke(
            inputs(&brush),
            &[
                (200.0, 200.0, 2000.0),
                (208.0, 208.0, 2016.0),
                (220.0, 220.0, 2032.0),
                (240.0, 240.0, 2048.0),
                (260.0, 260.0, 2064.0),
            ],
        );
        assert!(sp.doc.layers().has_paint_at(3, 3));
        assert!(sp.revert().unwrap());
        assert_eq!(sp.path, saved_path, "回退不改文件名");
        assert!(!sp.doc.layers().has_paint_at(3, 3), "磁盘上没有的笔迹被丢掉");
        assert!(sp.doc.layers().has_paint_at(0, 0));

        // 文件不存在时什么都不做
        let mut empty = Scratchpad::new(dir.join("nope"));
        assert!(!empty.revert().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_background_only_touches_the_scratchpad() {
        // drawwindow.py:645-655（背景图层 → 本项目只有背景色，差异记账）
        let dir = tmpdir("bg");
        let mut sp = Scratchpad::new(dir.clone());
        let mut main = Document::new();
        main.layers_mut().set_background_color([200, 100, 50]);
        sp.copy_background(main.layers().background_color());
        assert_eq!(sp.doc.layers().background_color(), [200, 100, 50]);
        assert_eq!(
            main.layers().background_color(),
            [200, 100, 50],
            "复制方向是主 → 暂存，主文档不变"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_page_loads_the_autosave_and_writes_the_pref_back() {
        // application.py:393-397（偏好缺省 → 写回 autosave 路径）+ :474-486（载入）
        let dir = tmpdir("startup");
        // 先造一份 autosave
        {
            let mut sp = Scratchpad::new(dir.clone());
            let brush = test_brush();
            sp.stroke(inputs(&brush), STROKE);
            sp.save().unwrap();
        }
        // 再模拟一次启动（偏好里还没有这条键）
        let mut prefs = fresh_prefs(&dir);
        let mut booted = Scratchpad::new(dir.clone());
        assert!(!booted.doc.layers().has_paint_at(0, 0));
        assert!(booted.startup_page(&mut prefs), "启动时载回 autosave");
        assert!(booted.doc.layers().has_paint_at(0, 0));
        assert_eq!(booted.path, dir.join("autosave.ora"));
        assert_eq!(
            prefs.str_or(prefs::KEY_LAST_OPENED_SCRATCHPAD, ""),
            dir.join("autosave.ora").display().to_string(),
            "偏好缺失时要写回 autosave 的路径（application.py:394-396）"
        );

        // 没有文件时保持空白，不报错，但偏好照样被写成 autosave 路径
        let mut prefs = fresh_prefs(&dir);
        let mut blank = Scratchpad::new(dir.join("empty"));
        assert!(!blank.startup_page(&mut prefs));
        assert!(!blank.doc.layers().has_paint_at(0, 0));
        assert_eq!(blank.path, dir.join("empty").join("autosave.ora"));
        assert_eq!(
            prefs.str_or(prefs::KEY_LAST_OPENED_SCRATCHPAD, ""),
            dir.join("empty").join("autosave.ora").display().to_string()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_page_honours_the_last_opened_scratchpad_preference() {
        // application.py:397 + :474-486：偏好里有路径就用它（而不是 autosave）
        let dir = tmpdir("startup-pref");
        let mut sp = Scratchpad::new(dir.clone());
        let brush = test_brush();
        sp.stroke(inputs(&brush), STROKE);
        let page = dir.join("my_page.ora");
        sp.save_as(&page).unwrap();

        // 只放偏好键：启动应当载入 my_page.ora
        let mut prefs = fresh_prefs(&dir);
        prefs.set_str(
            prefs::KEY_LAST_OPENED_SCRATCHPAD,
            page.display().to_string(),
        );
        let mut booted = Scratchpad::new(dir.clone());
        assert!(booted.startup_page(&mut prefs));
        assert_eq!(booted.path, page, "当前页就是偏好里的那一页");
        assert!(booted.doc.layers().has_paint_at(0, 0));

        // 偏好指向不存在的文件：源码不回落（application.py:481），只是不载入
        let mut prefs = fresh_prefs(&dir);
        prefs.set_str(
            prefs::KEY_LAST_OPENED_SCRATCHPAD,
            dir.join("nope.ora").display().to_string(),
        );
        let mut booted = Scratchpad::new(dir.clone());
        assert!(!booted.startup_page(&mut prefs));
        assert_eq!(booted.path, dir.join("nope.ora"), "路径保持偏好里的值");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scratchpad_actions_match_resources_xml() {
        // resources.xml:1094-1154（ScratchNew/ScratchLoad/ScratchSaveNow/
        // ScratchSaveAs/ScratchRevert/ScratchSaveAsDefault/ScratchClearDefault/
        // ScratchCopyBackground）与 :1465-1483（面板开关 + reveal）
        use crate::shell::actions;
        for id in [
            "ScratchNew",
            "ScratchLoad",
            "ScratchSaveNow",
            "ScratchSaveAs",
            "ScratchRevert",
            "ScratchSaveAsDefault",
            "ScratchClearDefault",
            "ScratchCopyBackground",
            "ToggleScratchpadTool",
            "RevealScratchpadTool",
        ] {
            let a = actions::by_id(id).unwrap_or_else(|| panic!("动作表缺 {id}"));
            assert!(a.handler.is_some(), "{id} 应有 activate 回调");
        }
        // 菜单动作没有加速键；面板开关是 Shift+K、reveal 是 Ctrl+K
        assert!(actions::by_id("ScratchNew").unwrap().accel.is_none());
        let toggle = actions::by_id("ToggleScratchpadTool").unwrap();
        assert_eq!(toggle.kind, actions::Kind::Toggle);
        assert_eq!(toggle.accel.map(|a| a.label()), Some("Shift+K".to_string()));
        let reveal = actions::by_id("RevealScratchpadTool").unwrap();
        assert_eq!(reveal.accel.map(|a| a.label()), Some("Ctrl+K".to_string()));
    }

}
