//! 文档模型 —— 照搬 MyPaint `lib/document.py` 中与撤销/重做、文档 I/O
//! 有关的部分。
//!
//! 完整移植 `lib/document.py`（2200 行，含图层栈、缓存目录、自动保存、
//! 帧/画框、分辨率、图层视图…）不在本轮范围；这里建立**命令栈的宿主**
//! 与**画幅/分辨率/设置**这些文档 I/O 必需的属性。
//!
//! | MyPaint | 本文件 |
//! |---|---|
//! | `lib/document.py:247` `Document` | [`Document`] |
//! | `:59` `DEFAULT_UNDO_STACK_SIZE = 40` | [`DEFAULT_UNDO_STACK_SIZE`] |
//! | `:309` `self.command_stack = CommandStack(max_undo_stack_size)` | [`Document::new`] |
//! | `:1158` `undo()` / `:1166` `redo()`（连跳 `automatic_undo`） | [`Document::undo`] / [`Document::redo`] |
//! | `:1174` `do(cmd)` | [`Document::do_command`] |
//! | `:1179`/`:1184` `update_last_command`/`get_last_command` | 同名方法 |
//! | `:1191` `get_bbox()` | [`Document::data_bbox`] |
//! | `:1229` `get_user_bbox()` | [`Document::get_user_bbox`] |
//! | `:835` `update_frame()` / `:864` `get_frame_enabled()` / `:867` `set_frame_enabled()` | [`Document::set_frame`] 等 |
//! | `:923` `clear()` | [`Document::clear`] |
//! | `:1046` `unsaved_painting_time` | [`Document::unsaved_painting_time`] |
//! | `:1432` `save()` / `:1488` `load()`（按扩展名分派） | [`Document::save`] / [`Document::load`] |
//! | `:1770`/`:1804` `save_ora`/`load_ora`、`:1646`/`:1732` `save_png`/`load_png` | [`Document::save_ora`] 等（实现在 `fileio`） |
//! | `:452` `settings`（随 ORA 往返的 JSON 字典） | [`Document::settings`] |
//!
//! 尚未移植（属后续里程碑）：`layer_stack`（P1 图层）、`sync_pending_changes`
//! 的异步收尾（我们的作画是同步落盘的，见同名方法）、自动保存与缓存目录、
//! `.jpg`、`strokemap`。

use std::collections::BTreeMap;

use hokusai::tile::TILE_SIZE;
use pigment_surface::{Layer, LayerStack, PigmentSurface};

use crate::command::{
    Brushwork, Command, CommandStack, CommandUpdate, DocContext, DEFAULT_UNDO_STACK_SIZE,
};
use crate::fileio::{self, FileError, Format, OraSaveOptions};

/// 文档模型：一个图层栈 + 一条撤销栈 + 画幅/分辨率/设置。
pub struct Document {
    layers: LayerStack,
    /// 撤销/重做栈（`lib/document.py:309`）。
    pub command_stack: CommandStack,
    /// 累计的"未保存作画时长"（秒），`.ora` 的 JSON 设置会写它
    /// （`lib/document.py:1046`；`Brushwork` 的 redo/undo 会改写）。
    pub unsaved_painting_time: f64,
    /// 画框 `(x, y, w, h)`（`lib/document.py:338` 的 `self._frame`）。
    frame: (i32, i32, i32, i32),
    /// 画框是否启用（`:339` 的 `self._frame_enabled`）。
    frame_enabled: bool,
    /// 标称分辨率（`:1780` 的 `self._xres` / `self._yres`，未指定为 `None`）。
    pub xres: Option<u32>,
    pub yres: Option<u32>,
    /// 文档级设置字典（`:452` 的 `settings`）：随 `.ora` 往返，可放任意
    /// JSON 值。MyPaint 用它给扩展留位（默认空）。
    pub settings: BTreeMap<String, serde_json::Value>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// 用 MyPaint 的默认栈深新建（偏好 `command.max_undo_stack_size`
    /// 覆盖它，见 `gui/application.py:338-343`）。
    pub fn new() -> Self {
        Self::with_max_undo_stack_size(DEFAULT_UNDO_STACK_SIZE)
    }

    /// 指定栈深新建。
    pub fn with_max_undo_stack_size(max_undo_stack_size: usize) -> Self {
        Self {
            layers: LayerStack::new(),
            command_stack: CommandStack::new(max_undo_stack_size),
            unsaved_painting_time: 0.0,
            frame: (0, 0, 0, 0),
            frame_enabled: false,
            xres: None,
            yres: None,
            settings: BTreeMap::new(),
        }
    }

    // ---- 图层栈（B2-3）----

    /// 图层栈（只读）。
    #[inline]
    pub fn layers(&self) -> &LayerStack {
        &self.layers
    }

    /// 图层栈（可变；结构操作请走命令，这里供渲染/导出读）。
    #[inline]
    pub fn layers_mut(&mut self) -> &mut LayerStack {
        &mut self.layers
    }

    /// **当前层**的表面。作画请走 [`Document::surface_mut`]；
    /// 显示合成应走 [`LayerStack::composite_tile`]（含背景与各层混合模式）。
    #[inline]
    pub fn surface(&self) -> &PigmentSurface {
        &self.layers.current().surface
    }

    /// 当前层表面的可变借用（照旧供作画热路径使用）。
    #[inline]
    pub fn surface_mut(&mut self) -> &mut PigmentSurface {
        &mut self.layers.current_mut().surface
    }

    /// 当前层（只读）。
    #[inline]
    pub fn current_layer(&self) -> &Layer {
        self.layers.current()
    }

    /// 命令上下文（`DocContext` 的两个字段都来自本对象）。
    ///
    /// 直接解构 `self` 而不是调 `self.context()`：栈与表面要同时可变借用，
    /// 只能靠字段级拆分（Rust 里没有 `weakref` 那种绕法）。
    fn split(&mut self) -> (&mut CommandStack, DocContext<'_>) {
        let Document {
            command_stack,
            layers,
            unsaved_painting_time,
            ..
        } = self;
        (
            command_stack,
            DocContext {
                layers,
                unsaved_painting_time,
            },
        )
    }

    // ---- 图层操作入口（构造命令 + 入栈；B2-3）----

    /// 新建图层并插入到当前层之上（`lib/document.py` 的 `add_layer`；
    /// UI 层 `layerswindow` 的 AddLayerAction 走这里）。
    pub fn add_layer(&mut self) -> bool {
        let index = self.layers.current_index();
        self.do_command(Box::new(crate::command::AddLayer::new(index, Layer::new(), false)))
    }

    /// 移除当前层（`RemoveLayer`）。
    pub fn remove_layer(&mut self) -> bool {
        let (stack, mut ctx) = self.split();
        let cmd = crate::command::RemoveLayer::new(&mut ctx);
        stack.do_command(Box::new(cmd), &mut ctx)
    }

    /// 切换当前层（`SelectLayer`，自动撤销类）。
    pub fn select_layer(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        let prev = self.layers.current_id();
        let new = self.layers.layer(index).id;
        if prev == new {
            return;
        }
        self.do_command(Box::new(crate::command::SelectLayer::new(prev, new)));
    }

    /// 当前层上移一格（朝栈顶；`BubbleLayerUp`）。
    pub fn bubble_layer_up(&mut self) -> bool {
        let id = self.layers.current_id();
        self.do_command(Box::new(crate::command::BubbleLayer::up(id)))
    }

    /// 当前层下移一格（`BubbleLayerDown`）。
    pub fn bubble_layer_down(&mut self) -> bool {
        let id = self.layers.current_id();
        self.do_command(Box::new(crate::command::BubbleLayer::down(id)))
    }

    /// 合并当前层到下层（`MergeLayerDown`；不可合并时静默不做——
    /// UI 侧的动作可用性检查共用 [`crate::command::MergeLayerDown::new`] 的判据）。
    pub fn merge_layer_down(&mut self) -> bool {
        let (stack, mut ctx) = self.split();
        let Some(cmd) = crate::command::MergeLayerDown::new(&mut ctx) else {
            return false;
        };
        stack.do_command(Box::new(cmd), &mut ctx)
    }

    /// 设置图层可见性（`SetLayerVisibility`）。
    pub fn set_layer_visible(&mut self, index: usize, visible: bool) -> bool {
        let Some(id) = self.layers.try_layer(index).map(|l| l.id) else {
            return false;
        };
        self.do_command(Box::new(crate::command::SetLayerVisibility::new(id, visible)))
    }

    /// 设置图层锁定（`SetLayerLocked`）。
    pub fn set_layer_locked(&mut self, index: usize, locked: bool) -> bool {
        let Some(id) = self.layers.try_layer(index).map(|l| l.id) else {
            return false;
        };
        self.do_command(Box::new(crate::command::SetLayerLocked::new(id, locked)))
    }

    /// 设置图层不透明度（`SetLayerOpacity`）。
    pub fn set_layer_opacity(&mut self, index: usize, opacity: f32) -> bool {
        let Some(id) = self.layers.try_layer(index).map(|l| l.id) else {
            return false;
        };
        self.do_command(Box::new(crate::command::SetLayerOpacity::new(id, opacity)))
    }

    /// 设置图层混合模式（`SetLayerMode`）。
    pub fn set_layer_mode(&mut self, index: usize, mode: pigment_surface::combine::CombineMode) -> bool {
        let Some(id) = self.layers.try_layer(index).map(|l| l.id) else {
            return false;
        };
        self.do_command(Box::new(crate::command::SetLayerMode::new(id, mode)))
    }

    /// 重命名图层（`RenameLayer`）。
    pub fn rename_layer(&mut self, index: usize, name: Option<String>) -> bool {
        let Some(id) = self.layers.try_layer(index).map(|l| l.id) else {
            return false;
        };
        self.do_command(Box::new(crate::command::RenameLayer::new(id, name)))
    }

    /// 撤销最近一次命令（`lib/document.py:1158`），返回被撤销命令的展示名。
    ///
    /// 与源码一致地连跳 `automatic_undo` 命令（那些命令的撤销是自动行为，
    /// 不该占用用户的一次撤销）。
    pub fn undo(&mut self) -> Option<String> {
        self.sync_pending_changes();
        let (stack, mut ctx) = self.split();
        loop {
            let outcome = stack.undo(&mut ctx)?;
            if !outcome.automatic_undo {
                return Some(outcome.display_name);
            }
        }
    }

    /// 重做最近一次撤销（`lib/document.py:1166`）。
    pub fn redo(&mut self) -> Option<String> {
        self.sync_pending_changes();
        let (stack, mut ctx) = self.split();
        loop {
            let outcome = stack.redo(&mut ctx)?;
            if !outcome.applied {
                // 命令被取消：栈没动，继续重做也没有意义
                return None;
            }
            if !outcome.automatic_undo {
                return Some(outcome.display_name);
            }
        }
    }

    /// 执行一条命令并入栈（`lib/document.py:1174`）。
    pub fn do_command(&mut self, command: Box<dyn Command>) -> bool {
        let (stack, mut ctx) = self.split();
        stack.do_command(command, &mut ctx)
    }

    /// 执行一段已收尾的作画（`lib/command.py:Brushwork` 的常规入口）。
    pub fn do_brushwork(&mut self, brushwork: Brushwork) -> bool {
        self.do_command(Box::new(brushwork))
    }

    /// 更新最近一次命令（`lib/document.py:1179`），供笔刷编辑器的
    /// Live Update 用。
    pub fn update_last_command(&mut self, update: &CommandUpdate<'_>) -> Option<String> {
        let (stack, mut ctx) = self.split();
        stack.update_last_command(&mut ctx, update)
    }

    /// 最近一次命令（`lib/document.py:1184`）。
    pub fn get_last_command(&self) -> Option<&dyn Command> {
        self.command_stack.get_last_command()
    }

    /// 内容外接框（`lib/document.py:1191` 的 `get_bbox()` 的 tile 对齐版）：
    /// 已分配 tile 的最小外接矩形，返回 `(x, y, w, h)`；空画布返回全 0。
    ///
    /// 与 MyPaint 一致地**按 tile 边界取整**（`lib/tiledsurface.py` 的
    /// `get_bbox()`），因为它会被 `.ora`/`.png` 导出直接当画幅用。
    pub fn data_bbox(&self) -> (i32, i32, i32, i32) {
        let mut coords = self
            .layers
            .iter_top_down()
            .flat_map(|(_, l)| l.surface.tiles().map(|(c, _)| c));
        let Some(first) = coords.next() else {
            return (0, 0, 0, 0);
        };
        let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
        for (tx, ty) in coords {
            x0 = x0.min(tx);
            y0 = y0.min(ty);
            x1 = x1.max(tx);
            y1 = y1.max(ty);
        }
        let ts = TILE_SIZE as i32;
        (x0 * ts, y0 * ts, (x1 - x0 + 1) * ts, (y1 - y0 + 1) * ts)
    }

    /// 清空文档（`lib/document.py:923`）：丢内容、清栈、作画时长归零。
    pub fn clear(&mut self) {
        self.layers = LayerStack::new();
        self.command_stack.clear();
        self.unsaved_painting_time = 0.0;
    }

    // ---- 画框（`lib/document.py:827-878`）----

    /// 画框矩形（`:827` 的 `get_frame()`）。
    #[inline]
    pub fn get_frame(&self) -> (i32, i32, i32, i32) {
        self.frame
    }

    /// 设置画框（`:835` 的 `update_frame()`）。
    ///
    /// 与源码一致：宽高被钳到非负（源码在 `:846-849` 处对 w/h 取 `max(0, …)`）。
    pub fn set_frame(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.frame = (x, y, w.max(0), h.max(0));
    }

    /// 画框是否启用（`:864`）。
    #[inline]
    pub fn get_frame_enabled(&self) -> bool {
        self.frame_enabled
    }

    /// 启用/停用画框（`:867`）。
    #[inline]
    pub fn set_frame_enabled(&mut self, enabled: bool) {
        self.frame_enabled = enabled;
    }

    /// 用户视角的导出外接框（`lib/document.py:1229` 的 `get_user_bbox()`）。
    ///
    /// 源码逻辑：画框启用时用它；否则用内容外接框，并按**背景图尺寸**向上取整
    /// （「宽度与高度至少是一个背景图大小」）。我们没有背景图，而
    /// [`Document::data_bbox`] 本身就是 tile 对齐的，取整退化为恒等 ——
    /// 所以在"无背景层 + 画框关闭"这一档上，两者结果相同。
    ///
    /// 内容为空时回退成画框（源码靠背景图保证最小尺寸；我们靠画框）。
    pub fn get_user_bbox(&self) -> (i32, i32, i32, i32) {
        if self.frame_enabled {
            return self.frame;
        }
        let bbox = self.data_bbox();
        if bbox.2 == 0 || bbox.3 == 0 {
            // 空文档：源码断言的 `bg_bbox.w/h > 0` 保证非零；我们退回画框，
            // 画框也空时交给导出层（`save_as_png` 会退化成 1×1）
            return self.frame;
        }
        bbox
    }

    // ---- 文档 I/O（照搬 `lib/document.py:1432` save / `:1488` load 的分派）----

    /// 保存到文件，按扩展名分派（`lib/document.py:1432` 的 `save`）。
    ///
    /// 与源码一致：成功后把 `unsaved_painting_time` 归零（`:1485`）。
    ///
    /// `alpha` 只在 `.png` 分支有意义；`None` 时用源码的默认
    /// `not background_visible`（我们无背景层，等价于 `true`，但为给用户
    /// "像有背景层那样导出"的选择，调用方可显式传 `Some(false)`）。
    pub fn save(
        &mut self,
        filename: impl AsRef<std::path::Path>,
        alpha: Option<bool>,
    ) -> Result<SaveOutcome, FileError> {
        self.sync_pending_changes();
        let path = filename.as_ref();
        let outcome = match fileio::format_for_path(path) {
            Some(Format::Ora) => {
                let opts = OraSaveOptions {
                    bbox: None,
                    xres: self.xres,
                    yres: self.yres,
                    frame_active: self.frame_enabled,
                    settings_json: Some(self.settings_json()),
                    layer_name: None,
                };
                let bbox = self.data_bbox();
                let saved = fileio::save_ora(self.surface(), path, &opts, bbox)?;
                SaveOutcome::Ora { bbox: saved.bbox }
            }
            Some(Format::Png) => {
                let bbox = self.get_user_bbox();
                let saved = fileio::save_png(self.surface(), path, bbox, alpha.unwrap_or(true))?;
                SaveOutcome::Png { bbox: saved.bbox }
            }
            _ => return Err(fileio::unsupported_message(path)),
        };
        self.unsaved_painting_time = 0.0;
        Ok(outcome)
    }

    /// 从文件载入，按扩展名分派（`lib/document.py:1488` 的 `load`）。
    ///
    /// 与源码一致：成功后清空命令栈并把作画时长归零（`:1554-1555`）。
    pub fn load(&mut self, filename: impl AsRef<std::path::Path>) -> Result<LoadOutcome, FileError> {
        let path = filename.as_ref();
        // 源码的前置检查（`:1513` / `:1519`）
        if !path.is_file() {
            return Err(FileError::NotFound(path.display().to_string()));
        }
        self.clear();
        let outcome = match fileio::format_for_path(path) {
            Some(Format::Ora) => {
                let loaded = fileio::load_ora(self.surface_mut(), path)?;
                self.xres = loaded.xres;
                self.yres = loaded.yres;
                // 照源码：画幅设为文件里声明的尺寸，并按 `frame-active` 决定启用
                self.set_frame(0, 0, loaded.bbox.2, loaded.bbox.3);
                self.set_frame_enabled(loaded.frame_enabled);
                if let Some(json) = &loaded.settings_json {
                    self.set_settings_json(json);
                }
                LoadOutcome::Ora {
                    bbox: loaded.bbox,
                    layer_name: loaded.layer_name,
                    mypaint_version: loaded.mypaint_version,
                    compatibility: loaded.compatibility,
                }
            }
            Some(Format::Png) => {
                let loaded = fileio::load_png(self.surface_mut(), path)?;
                // `load_png` 末行 `self.set_frame(bbox, user_initiated=False)`（`:1736`）
                self.set_frame(loaded.bbox.0, loaded.bbox.1, loaded.bbox.2, loaded.bbox.3);
                LoadOutcome::Png { bbox: loaded.bbox }
            }
            _ => return Err(fileio::unsupported_message(path)),
        };
        self.command_stack.clear();
        self.unsaved_painting_time = 0.0;
        Ok(outcome)
    }

    /// `settings` 的 JSON 序列化（照 `json.dumps(dict(settings), indent=2)`）。
    ///
    /// 空字典对应源码的 `"{}"`（默认文档就是这一档）。
    pub fn settings_json(&self) -> String {
        if self.settings.is_empty() {
            return "{}".to_string();
        }
        let obj = serde_json::Value::Object(
            self.settings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string())
    }

    /// 从 JSON 串恢复 `settings`（照 `:1871` 的 `_settings.update(json.loads(...))`）。
    ///
    /// 非法 JSON 时保持现有设置不变（源码会记日志并继续）。
    pub fn set_settings_json(&mut self, json: &str) {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(json) {
            self.settings = map.into_iter().collect();
        }
    }

    /// `sync_pending_changes()`（`lib/document.py:1127`）的等价物。
    ///
    /// MyPaint 需要它把"待刷的笔画工作"提交给模型（Brushwork 的记录期是
    /// 异步收尾的）；我们的 [`Brushwork`] 在 `stop_recording` 时同步完成，
    /// 因此这里无事可做。保留这个方法名是为了让调用点与源码一一对应。
    #[inline]
    pub fn sync_pending_changes(&self) {}
}

/// [`Document::save`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    Ora { bbox: (i32, i32, i32, i32) },
    Png { bbox: (i32, i32, i32, i32) },
}

/// [`Document::load`] 的结果。
#[derive(Debug, Clone)]
pub enum LoadOutcome {
    Ora {
        bbox: (i32, i32, i32, i32),
        layer_name: Option<String>,
        mypaint_version: Option<String>,
        compatibility: Option<fileio::Compatibility>,
    },
    Png {
        bbox: (i32, i32, i32, i32),
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushlib::default_brush;
    use crate::stroke::StrokeEvent;
    use hokusai::{Brush, BrushSetting, BrushState};

    fn test_brush(radius: f32) -> Brush {
        let mut b = default_brush();
        b.get_mut(BrushSetting::Radius).base_value = radius;
        b
    }

    /// 在文档上画一笔并提交（走 app 相同的三段式流程）。
    fn paint_and_commit(doc: &mut Document, radius: f32) -> bool {
        let brush = test_brush(radius);
        let mut state = BrushState::default();
        let mut bw = Brushwork::new();
        for i in 0..40 {
            let ev = StrokeEvent::new(
                0.016,
                i as f32 * 2.0,
                radius * 10.0,
                1.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
            );
            bw.stroke_to(doc.surface_mut(), &brush, "Test Brush", &mut state, ev);
        }
        let changed = bw.stop_recording(doc.surface_mut(), &state, false);
        if changed {
            doc.do_brushwork(bw);
        }
        changed
    }

    #[test]
    fn default_stack_size_matches_mypaint() {
        assert_eq!(DEFAULT_UNDO_STACK_SIZE, 40);
        let doc = Document::new();
        assert_eq!(doc.command_stack.max_stack_size(), 40);
    }

    #[test]
    fn undo_and_redo_round_trip_pixels() {
        let mut doc = Document::new();
        assert!(paint_and_commit(&mut doc, 2.0));
        assert!(doc.surface().tile_count() > 0);
        assert!(doc.unsaved_painting_time > 0.0);

        doc.undo();
        assert_eq!(doc.surface().tile_count(), 0);
        assert_eq!(doc.unsaved_painting_time, 0.0);

        doc.redo();
        assert!(doc.surface().tile_count() > 0);
    }

    #[test]
    fn undo_returns_the_command_display_name() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        let name = doc.undo().unwrap();
        assert!(name.ends_with("of painting with Test Brush"), "{name}");
        assert!(doc.undo().is_none(), "栈空后返回 None");
    }

    #[test]
    fn empty_stroke_is_not_pushed_onto_the_stack() {
        let mut doc = Document::new();
        // 半径极小 + 只喂一个事件 → 引擎什么都没画
        let brush = test_brush(-6.0);
        let mut state = BrushState::default();
        let mut bw = Brushwork::new();
        let ev = StrokeEvent::new(0.001, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        bw.stroke_to(doc.surface_mut(), &brush, "Tiny", &mut state, ev);
        let changed = bw.stop_recording(doc.surface_mut(), &state, false);
        assert!(!changed, "零时长笔画应被判为空");
        assert_eq!(doc.command_stack.undo_len(), 0);
    }

    #[test]
    fn data_bbox_is_tile_aligned() {
        let mut doc = Document::new();
        assert_eq!(doc.data_bbox(), (0, 0, 0, 0), "空画布无内容");
        paint_and_commit(&mut doc, 2.0);
        let (x, y, w, h) = doc.data_bbox();
        let ts = TILE_SIZE as i32;
        assert_eq!(x % ts, 0);
        assert_eq!(y % ts, 0);
        assert_eq!(w % ts, 0);
        assert_eq!(h % ts, 0);
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn clear_drops_content_and_history() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        doc.clear();
        assert_eq!(doc.surface().tile_count(), 0);
        assert_eq!(doc.command_stack.undo_len(), 0);
        assert_eq!(doc.command_stack.redo_len(), 0);
        assert_eq!(doc.unsaved_painting_time, 0.0);
    }

    #[test]
    fn update_last_command_rerenders_with_another_brush() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        let tile_before = doc.surface().tile_count();
        let fatter = test_brush(4.0);
        let name = doc.update_last_command(&CommandUpdate::Brush {
            brush: &fatter,
            name: "Fatter",
        });
        assert!(name.is_some());
        assert!(doc.surface().tile_count() >= tile_before);
        assert!(doc.get_last_command().unwrap().display_name().ends_with("with Fatter"));
    }
}
