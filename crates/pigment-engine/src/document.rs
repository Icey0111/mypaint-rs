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
//! | `gui/tileddrawwidget.py:977` `pick_color()`（B3-4 取色） | [`Document::pick_color`] |
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
use std::path::Path;
use std::sync::OnceLock;

use pigment_surface::layer::{BackgroundImage, LayerContent, LayerMode};
use pigment_surface::symmetry::SymmetryType;
use pigment_surface::{Layer, LayerStack, PigmentSurface};

use crate::command::{
    Brushwork, Command, CommandStack, CommandUpdate, DocContext, LayerProp,
    DEFAULT_UNDO_STACK_SIZE,
};
use crate::fileio::{self, FileError, Format, OraSaveOptions};
use crate::layer_views::LayerViews;

/// 把 `fileio` 载入的层树转成 `pigment_surface` 的图层树（`_layers` 序不变）。
fn ora_layers_to_layers(nodes: Vec<fileio::OraLayerData>) -> Vec<Layer> {
    nodes
        .into_iter()
        .map(|d| {
            let content = match d.content {
                fileio::OraContent::Painting(surface) => LayerContent::Painting(surface),
                fileio::OraContent::Group(children) => {
                    // 组：子层递归建栈（组可以是空栈，group.py:78）
                    LayerContent::Group(LayerStack::from_children(ora_layers_to_layers(children)))
                }
            };
            let mut layer = Layer::new();
            layer.content = content;
            layer.name = d.name;
            layer.mode = d.mode;
            layer.opacity = d.opacity;
            layer.visible = d.visible;
            layer.locked = d.locked;
            layer
        })
        .collect()
}

/// 取色取样方块的边长（`tileddrawwidget.py:977` 的 `pick_color(x, y, size=3)`）。
pub const PICK_COLOR_SIZE: i32 = 3;

/// 中央对称状态 —— `RootLayerStack` 的 `_symmetry_*` 字段（`tree.py:130-138`）。
///
/// 源码把这份状态放在**根图层栈**上（`RootLayerStack`，`tree.py:72`），
/// 由 `set_symmetry_state` 复制给当前层（`tree.py:880-934`）；层的
/// `set_symmetry_state` 再把它落到表面的 C 结构里（`data.py:499-511`）。
///
/// 本项目的三个对应物：
/// 1. 本结构（文档级的唯一一份状态）；
/// 2. [`Document::set_symmetry_state`]（写入 + 传播）；
/// 3. [`pigment_surface::PigmentSurface::set_symmetry_state`]（表面上的执行端）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SymmetrySetup {
    /// 对称是否开启（`_symmetry_active`）。
    pub active: bool,
    /// 中心是否"还没被正式设置过"（`_symmetry_unset`）。
    ///
    /// 第一次开启对称时用它决定"要不要按视口中心推一个位置"
    /// （`gui/document.py:1895-1901`）。源码初值是 `True`（`tree.py:133`），
    /// 所以第一次开对称会落在视口中心，而不是原点。
    pub unset: bool,
    /// 对称中心（模型坐标；源码是**整数**元组，`tree.py:903` 用 `int(round())`）。
    pub center: (i32, i32),
    /// 对称类型（`_symmetry_type`）。
    pub type_: SymmetryType,
    /// 旋转对称的线条数（`_symmetry_lines`）。
    pub lines: i32,
    /// 对称线角度（度，`_symmetry_angle`）。
    pub angle: f32,
}

impl Default for SymmetrySetup {
    /// `tree.py:130-138` 的初值：未设置中心、未开启、垂直镜像、原点、0°、2 条线。
    fn default() -> Self {
        Self {
            active: false,
            unset: true,
            center: (0, 0),
            type_: SymmetryType::Vertical,
            lines: 2,
            angle: 0.0,
        }
    }
}

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
    /// 中央对称状态（`RootLayerStack._symmetry_*`，`tree.py:130-138`）。
    symmetry: SymmetrySetup,
    /// 标称分辨率（`:1780` 的 `self._xres` / `self._yres`，未指定为 `None`）。
    pub xres: Option<u32>,
    pub yres: Option<u32>,
    /// 文档级设置字典（`:452` 的 `settings`）：随 `.ora` 往返，可放任意
    /// JSON 值。MyPaint 用它给扩展留位（默认空）。
    pub settings: BTreeMap<String, serde_json::Value>,
    /// 保存时写进 `mypaint:eotf` 的值 —— `None` 表示"不做转换也不打标签"
    /// （偏好 `display.colorspace == "unknown"`，`lib/eotf.py`）。
    pub save_eotf: Option<f32>,
    /// 新建绘画层的默认混合模式是否 Pigment ——
    /// `compability_settings[mode].pigment_layer_is_default`（`compatibility.py:473-482`）。
    pub default_layer_pigment: bool,
    /// 命名可见性视图（`lib/layervis.py`；设置键 `layervis`）。
    pub views: LayerViews,
    /// 「最后作画位置」——`tdw.set_last_painting_pos`（`tileddrawwidget.py:303`）。
    ///
    /// 供 SequenceMode（连点成线）起点、以及 last-painting-pos 覆盖层使用；
    /// 自由手绘在压力 > 0 时每事件更新（`freehand.py:614-617`），直线/序列
    /// 在收笔时更新（`linemode.py:525-530`）。源码把它放在 TDW 上（并注明
    /// FIXME 应属于 model），本移植按 FIXME 放进文档模型。
    pub last_painting_pos: Option<(f32, f32)>,
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
            symmetry: SymmetrySetup::default(),
            xres: None,
            yres: None,
            settings: BTreeMap::new(),
            save_eotf: Some(crate::compat::EOTF_2X),
            default_layer_pigment: true,
            views: LayerViews::default(),
            last_painting_pos: None,
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

    /// **当前层**的表面（只对绘画层有意义；组会 panic）。
    ///
    /// 作画请走 [`Document::surface_mut`]（它会跳过组/锁/隐藏分支）；
    /// 显示合成走 [`LayerStack::composite_tile`]（含背景与各层混合模式）。
    #[inline]
    pub fn surface(&self) -> &PigmentSurface {
        self.layers
            .current()
            .surface()
            .expect("当前层是组，没有表面（用 paintable_surface_mut / composite_tile）")
    }

    /// 当前层表面的可变借用（供作画热路径使用；组会 panic）。
    ///
    /// UI 作画入口请用 [`Document::paintable_surface_mut`]：当前层是组、
    /// 被锁或祖先不可见时它是 `None`（`data.py:1364 get_paintable`）。
    #[inline]
    pub fn surface_mut(&mut self) -> &mut PigmentSurface {
        self.layers
            .current_mut()
            .surface_mut()
            .expect("当前层是组，没有表面（用 paintable_surface_mut）")
    }

    /// 可画的当前表面；不可画（组/锁/隐藏分支）时 `None`。
    #[inline]
    pub fn paintable_surface_mut(&mut self) -> Option<&mut PigmentSurface> {
        self.layers.paintable_surface_mut()
    }

    /// 当前层的**只读**表面（组 → `None`；诊断/导出用）。
    #[inline]
    pub fn paintable_surface(&self) -> Option<&PigmentSurface> {
        self.layers.current().surface()
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
            views,
            unsaved_painting_time,
            frame,
            ..
        } = self;
        (
            command_stack,
            DocContext {
                layers,
                views,
                unsaved_painting_time,
                frame,
            },
        )
    }

    // ---- 图层操作入口（构造命令 + 入栈；B2-3 起按路径寻址，B2-6 起支持组）----

    /// 当前层路径（`tree.py:954`）。
    pub fn current_path(&self) -> &[usize] {
        self.layers.current_path()
    }

    /// 在 `insert_path` 处新建一层（`document.py:1290 add_layer`）。
    ///
    /// 与源码 `deepinsert`（`tree.py:1687`）一致：新层插入后就吃一份当前
    /// 中央对称状态（新层的表面是干净的，对称参数要补上）。
    pub fn add_layer_at(&mut self, insert_path: Vec<usize>, layer: Layer, is_import: bool) -> bool {
        let ok = self.do_command(Box::new(crate::command::AddLayer::new(
            insert_path.clone(),
            layer,
            is_import,
        )));
        if ok {
            self.propagate_symmetry_to(&insert_path);
        }
        ok
    }

    /// 在**当前层位置**新建绘画层（`NewPaintingLayerAbove`：
    /// `add_layer(path=current_path)`，新层落在当前层的同父同索引）。
    pub fn add_layer(&mut self) -> bool {
        let path = self.layers.current_path().to_vec();
        let mut layer = Layer::new();
        // `set_default_mode`（`compatibility.py:473-482`）：2.x 默认 Pigment，
        // 1.x 默认 Normal。
        layer.set_mode(pigment_surface::layer::LayerMode::Standard(
            if self.default_layer_pigment {
                pigment_surface::combine::CombineMode::SpectralWGM
            } else {
                pigment_surface::combine::CombineMode::Normal
            },
        ));
        self.add_layer_at(path, layer, false)
    }

    /// 在**当前层位置**新建组（`NewLayerGroupAbove`：`layer_class=LayerStack`）。
    pub fn add_group(&mut self) -> bool {
        let path = self.layers.current_path().to_vec();
        self.add_layer_at(path, Layer::new_group(), false)
    }

    /// 移除当前层（`RemoveLayer`）。
    pub fn remove_layer(&mut self) -> bool {
        let (stack, mut ctx) = self.split();
        let cmd = crate::command::RemoveLayer::new(&mut ctx);
        let ok = stack.do_command(Box::new(cmd), &mut ctx);
        self.sync_layer_views();
        ok
    }

    /// 切换当前层（`SelectLayer`，自动撤销类）。
    ///
    /// 与源码 `RootLayerStack.set_current_path` 一致，切完把中央对称状态
    /// 复制给新当前层（`tree.py:973-983` 调 `_propagate_symmetry_state`）。
    pub fn select_layer(&mut self, path: &[usize]) {
        let Some(layer) = self.layers.layer_at(path) else {
            return;
        };
        let new = layer.id;
        let prev = self.layers.current_id();
        if prev == new {
            return;
        }
        self.do_command(Box::new(crate::command::SelectLayer::new(prev, new)));
        self.propagate_symmetry_to(path);
    }

    /// 当前层上移一格（朝同层栈顶；`BubbleLayerUp`）。
    pub fn bubble_layer_up(&mut self) -> bool {
        let id = self.layers.current_id();
        self.do_command(Box::new(crate::command::BubbleLayer::up(id)))
    }

    /// 当前层下移一格（`BubbleLayerDown`）。
    pub fn bubble_layer_down(&mut self) -> bool {
        let id = self.layers.current_id();
        self.do_command(Box::new(crate::command::BubbleLayer::down(id)))
    }

    /// 把 `src_path` 的层挪到 `targ_path`（deepinsert 语义；可跨组）。
    /// 返回命令是否入栈（非法目标 / 目标父不是组时返回 false）。
    pub fn restack_layer(&mut self, src_path: Vec<usize>, targ_path: Vec<usize>) -> bool {
        let (stack, mut ctx) = self.split();
        let Some(cmd) = crate::command::RestackLayer::new(&mut ctx, src_path, targ_path) else {
            return false;
        };
        let ok = stack.do_command(Box::new(cmd), &mut ctx);
        self.sync_layer_views();
        ok
    }

    /// 合并当前层到下一个兄弟（`MergeLayerDown`；不可合并时静默不做——
    /// UI 侧的动作可用性检查共用 [`crate::command::MergeLayerDown::new`] 的判据）。
    pub fn merge_layer_down(&mut self) -> bool {
        let (stack, mut ctx) = self.split();
        let Some(cmd) = crate::command::MergeLayerDown::new(&mut ctx) else {
            return false;
        };
        let ok = stack.do_command(Box::new(cmd), &mut ctx);
        self.sync_layer_views();
        ok
    }

    /// 原位复制当前层（`lib/document.py:1009` 的 `DuplicateLayer`）。
    pub fn duplicate_layer(&mut self) -> bool {
        let (stack, mut ctx) = self.split();
        let cmd = crate::command::DuplicateLayer::new(&mut ctx);
        let ok = stack.do_command(Box::new(cmd), &mut ctx);
        self.sync_layer_views();
        ok
    }

    /// 命名视图的名字表（`layer_view_manager.view_names`）。
    pub fn layer_view_names(&self) -> Vec<String> {
        self.views.names()
    }

    /// 当前视图名；`None` = 源码的「No active view」。
    pub fn current_layer_view_name(&self) -> Option<&str> {
        self.views.current_name()
    }

    /// 当前视图是否锁定（锁定时禁止切换层可见性，`gui/layers.py:566`）。
    pub fn current_layer_view_locked(&self) -> bool {
        self.views.current_locked()
    }

    /// 新建视图并激活（`AddLayerView`）。
    pub fn add_layer_view(&mut self, name: Option<&str>) -> bool {
        self.do_command(Box::new(crate::command::AddLayerView::new(
            name.map(str::to_string),
        )))
    }

    /// 删除当前视图（`RemoveActiveLayerView`）。
    pub fn remove_active_layer_view(&mut self) -> bool {
        self.do_command(Box::new(crate::command::RemoveActiveLayerView::new()))
    }

    /// 重命名当前视图（`RenameActiveLayerView`）。
    pub fn rename_active_layer_view(&mut self, name: &str) -> bool {
        self.do_command(Box::new(crate::command::RenameActiveLayerView::new(name)))
    }

    /// 激活/取消激活视图（`ActivateLayerView`）；`None` = 回到未命名视图。
    pub fn activate_layer_view(&mut self, name: Option<&str>) -> bool {
        self.do_command(Box::new(crate::command::ActivateLayerView::new(
            name.map(str::to_string),
        )))
    }

    /// 设置当前视图的锁定位（`SetActiveLayerViewLocked`）。
    pub fn set_active_layer_view_locked(&mut self, locked: bool) -> bool {
        self.do_command(Box::new(crate::command::SetActiveLayerViewLocked::new(
            locked,
        )))
    }

    /// 设置图层可见性（`SetLayerVisibility`）。
    pub fn set_layer_visible(&mut self, path: &[usize], visible: bool) -> bool {
        let Some(id) = self.layers.layer_at(path).map(|l| l.id) else {
            return false;
        };
        // 合并语义（document.py:1368-1378）：栈顶已是**同层同类**命令 → update
        if self
            .command_stack
            .top_is_layer_update(LayerProp::Visibility, id)
        {
            self.update_last_command(&crate::command::CommandUpdate::LayerVisibility(visible));
            return true;
        }
        self.do_command(Box::new(crate::command::SetLayerVisibility::new(id, visible)))
    }

    /// 设置图层锁定（`SetLayerLocked`）。
    pub fn set_layer_locked(&mut self, path: &[usize], locked: bool) -> bool {
        let Some(id) = self.layers.layer_at(path).map(|l| l.id) else {
            return false;
        };
        // 合并语义（document.py:1380-1390）
        if self.command_stack.top_is_layer_update(LayerProp::Locked, id) {
            self.update_last_command(&crate::command::CommandUpdate::LayerLocked(locked));
            return true;
        }
        self.do_command(Box::new(crate::command::SetLayerLocked::new(id, locked)))
    }

    /// 设置图层不透明度（`SetLayerOpacity`）。
    pub fn set_layer_opacity(&mut self, path: &[usize], opacity: f32) -> bool {
        let Some(id) = self.layers.layer_at(path).map(|l| l.id) else {
            return false;
        };
        // 合并语义（document.py:1392-1410）：滑条连续拖动只在栈顶更新
        if self.command_stack.top_is_layer_update(LayerProp::Opacity, id) {
            self.update_last_command(&crate::command::CommandUpdate::LayerOpacity(opacity));
            return true;
        }
        self.do_command(Box::new(crate::command::SetLayerOpacity::new(id, opacity)))
    }

    /// 设置图层混合模式（`SetLayerMode`；组还可以设 `PassThrough`）。
    pub fn set_layer_mode(&mut self, path: &[usize], mode: LayerMode) -> bool {
        let Some(id) = self.layers.layer_at(path).map(|l| l.id) else {
            return false;
        };
        self.do_command(Box::new(crate::command::SetLayerMode::new(id, mode)))
    }

    /// 重命名图层（`RenameLayer`）。
    pub fn rename_layer(&mut self, path: &[usize], name: Option<String>) -> bool {
        let Some(id) = self.layers.layer_at(path).map(|l| l.id) else {
            return false;
        };
        // 合并语义（document.py:1305-1317）：连续重命名只留栈顶一条
        if self.command_stack.top_is_layer_update(LayerProp::Name, id) {
            let update =
                crate::command::CommandUpdate::Rename(name.as_deref().unwrap_or(""));
            self.update_last_command(&update);
            return true;
        }
        self.do_command(Box::new(crate::command::RenameLayer::new(id, name)))
    }

    /// 撤销最近一次命令（`lib/document.py:1158`），返回被撤销命令的展示名。
    ///
    /// 与源码一致地连跳 `automatic_undo` 命令（那些命令的撤销是自动行为，
    /// 不该占用用户的一次撤销）。
    pub fn undo(&mut self) -> Option<String> {
        self.sync_pending_changes();
        let result = (|| {
            let (stack, mut ctx) = self.split();
            loop {
                let outcome = stack.undo(&mut ctx)?;
                if !outcome.automatic_undo {
                    return Some(outcome.display_name);
                }
            }
        })();
        self.sync_layer_views();
        result
    }

    /// 重做最近一次撤销（`lib/document.py:1166`）。
    pub fn redo(&mut self) -> Option<String> {
        self.sync_pending_changes();
        let result = (|| {
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
        })();
        self.sync_layer_views();
        result
    }

    /// 执行一条命令并入栈（`lib/document.py:1174`）。
    pub fn do_command(&mut self, command: Box<dyn Command>) -> bool {
        let (stack, mut ctx) = self.split();
        let applied = stack.do_command(command, &mut ctx);
        self.sync_layer_views();
        applied
    }

    /// 命令之后把外部可见性改动记回当前命名视图（`layervis.py:399-412`）。
    fn sync_layer_views(&mut self) {
        self.views.sync(&self.layers);
    }

    /// 执行一段已收尾的作画（`lib/command.py:Brushwork` 的常规入口）。
    pub fn do_brushwork(&mut self, brushwork: Brushwork) -> bool {
        self.do_command(Box::new(brushwork))
    }

    /// 更新最近一次命令（`lib/document.py:1179`），供笔刷编辑器的
    /// Live Update 用。
    pub fn update_last_command(&mut self, update: &CommandUpdate<'_>) -> Option<String> {
        let (stack, mut ctx) = self.split();
        let result = stack.update_last_command(&mut ctx, update);
        self.sync_layer_views();
        result
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
    /// 整棵树逐层取 [`Layer::data_bbox`] 再并（照 `helpers.coordinate_bounds`）。
    pub fn data_bbox(&self) -> (i32, i32, i32, i32) {
        self.layers.data_bbox()
    }

    /// 清空文档（`lib/document.py:922`）：丢内容、清栈、作画时长归零。
    ///
    /// 与源码一致地把**画框、分辨率、文档设置、对称状态**一并复位
    /// （`:944-961`：`symmetry_unset = True` +
    /// `set_symmetry_state(False, (0,0), SymmetryVertical, 2, 0)`、
    /// `set_frame([0,0,0,0])`、`set_frame_enabled(False)`、`_xres/_yres = None`、
    /// `_settings.clear()`）。
    pub fn clear(&mut self) {
        self.layers = LayerStack::new();
        self.command_stack.clear();
        self.unsaved_painting_time = 0.0;
        self.frame = (0, 0, 0, 0);
        self.frame_enabled = false;
        self.xres = None;
        self.yres = None;
        self.settings.clear();
        self.views = LayerViews::default();
        self.symmetry = SymmetrySetup::default();
        self.last_painting_pos = None;
    }

    // ---- 画框（`lib/document.py:827-878`）----

    /// 画框矩形（`:827` 的 `get_frame()`）。
    #[inline]
    pub fn get_frame(&self) -> (i32, i32, i32, i32) {
        self.frame
    }

    /// 设置画框（`:829` `set_frame` → `:835` `update_frame`）。
    ///
    /// **不做钳位**，照源码 `new_frame[i] = int(var)`（`:846-849`）：源码里
    /// 唯一保证画框不退化的是编辑模式的拖拽逻辑（`framewindow.py:376-386`
    /// 用 `_MIN_FRAME_SIZE = 1` 兜底），以及文件加载时给的图像尺寸。
    /// 本项目没有移植编辑模式，所以这里如实保存调用方给的值。
    pub fn set_frame(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.frame = (x, y, w, h);
    }

    /// 画框是否启用（`:864`）。
    #[inline]
    pub fn get_frame_enabled(&self) -> bool {
        self.frame_enabled
    }

    /// 启用/停用画框（`:867`）。
    ///
    /// 源码在 `user_initiated=True` 时走 `command.SetFrameEnabled` 入撤销栈
    /// （`:871-873`）；本移植只做状态（画框的撤销动作未移植，见记账）。
    #[inline]
    pub fn set_frame_enabled(&mut self, enabled: bool) {
        self.frame_enabled = enabled;
    }

    /// 内置默认纸纹（`backgrounds/mrmamurk/mamurk_e_1.png`，CC0）。
    ///
    /// 源码 `reset_background()`（`document.py:1327-1380`）先找用户
    /// `backgrounds/default.png`，找不到再用这个 fallback；这里先只移植
    /// 内置 fallback（用户背景目录/选择窗口记账）。
    pub fn default_paper_background() -> Option<BackgroundImage> {
        static PAPER: OnceLock<Option<BackgroundImage>> = OnceLock::new();
        PAPER
            .get_or_init(|| {
                BackgroundImage::from_png_bytes(include_bytes!(
                    "../assets/backgrounds/mamurk_e_1.png"
                ))
                .ok()
            })
            .clone()
    }

    /// 把背景重置为默认纸纹（`Document.reset_background`）。
    ///
    /// 新文档/启动时调用；`load_*` 不走这里（背景来自文件）。
    /// 查找顺序照 `gui/document.py:1327-1377`：用户
    /// `<user>/backgrounds/default.png` → 内置 fallback 纸纹 → 纯色 `#A8A498`。
    pub fn reset_background(&mut self) {
        self.reset_background_from(None);
    }

    /// 带用户数据目录的 `reset_background`（应用启动/新建时传入
    /// `prefs::config_dir()`；测试可传临时目录）。
    pub fn reset_background_from(&mut self, user_dir: Option<&Path>) {
        if let Some(dir) = user_dir {
            let path = dir.join("backgrounds").join("default.png");
            if path.is_file() {
                if let Ok(img) = background_from_file(&path) {
                    self.layers.set_background_image(img);
                    return;
                }
            }
        }
        if let Some(paper) = Self::default_paper_background() {
            self.layers.set_background_image(paper);
        } else {
            // 源码最后的兜底色（`gui/document.py:1375`）
            self.layers.set_background_color([0xA8, 0xA4, 0x98]);
        }
    }

    /// 从图片文件换背景（背景选择窗口的 Pattern 页；
    /// `gui/backgroundwindow.py:131-135` 的 `set_background`）。
    ///
    /// 返回错误字符串给 GUI 显示（源码用 `load_background` 的 errors 列表）。
    pub fn set_background_from_file(&mut self, path: &Path) -> Result<(), String> {
        let img = background_from_file(path)?;
        self.layers.set_background_image(img);
        Ok(())
    }

    /// 用户视角的导出外接框（`lib/document.py:1229` 的 `get_user_bbox()`）。
    ///
    /// 源码逻辑：画框启用时用它；否则用内容外接框，并按**背景图尺寸**
    /// 向上取整到整数倍（`document.py:1243-1282`）。默认纸纹是 256×256，
    /// 所以空文档的导出框是 256×256，与 MyPaint 一致。
    pub fn get_user_bbox(&self) -> (i32, i32, i32, i32) {
        if self.frame_enabled {
            return self.frame;
        }
        let (mut x, mut y, mut w, mut h) = self.data_bbox();
        let bg_w = self
            .layers
            .background_image()
            .map(|i| i.width() as i32)
            .unwrap_or(64);
        let bg_h = self
            .layers
            .background_image()
            .map(|i| i.height() as i32)
            .unwrap_or(64);

        let corr_x = x.rem_euclid(bg_w);
        if corr_x > 0 {
            x -= corr_x;
            w += corr_x;
        }
        let corr_y = y.rem_euclid(bg_h);
        if corr_y > 0 {
            y -= corr_y;
            h += corr_y;
        }
        if w == 0 {
            w = bg_w;
        } else {
            let orphan = w.rem_euclid(bg_w);
            if orphan > 0 {
                w += bg_w - orphan;
            }
        }
        if h == 0 {
            h = bg_h;
        } else {
            let orphan = h.rem_euclid(bg_h);
            if orphan > 0 {
                h += bg_h - orphan;
            }
        }
        (x, y, w, h)
    }

    // ---- 对称（`lib/layer/tree.py:809-950` 的 RootLayerStack 部分）----

    /// 中央对称状态（`tree.py:130-138`）。
    #[inline]
    pub fn symmetry(&self) -> SymmetrySetup {
        self.symmetry
    }

    /// 对称是否开启（`tree.py:819-830` 的 `symmetry_active`）。
    #[inline]
    pub fn symmetry_active(&self) -> bool {
        self.symmetry.active
    }

    /// 对称中心是否还没被设置过（`tree.py:811-817` 的 `symmetry_unset`）。
    #[inline]
    pub fn symmetry_unset(&self) -> bool {
        self.symmetry.unset
    }

    /// 直接改 `symmetry_unset`（`tree.py:815-817` 的 setter）。
    ///
    /// 唯一的用处是第一次开启对称时把它清掉（`gui/document.py:1897-1898`），
    /// 让之后的开关不再重新推中心。
    #[inline]
    pub fn set_symmetry_unset(&mut self, unset: bool) {
        self.symmetry.unset = unset;
    }

    /// 对称中心（`tree.py:832-838`；整数模型坐标）。
    #[inline]
    pub fn symmetry_center(&self) -> (i32, i32) {
        self.symmetry.center
    }

    /// 对称类型（`tree.py:856-862`）。
    #[inline]
    pub fn symmetry_type(&self) -> SymmetryType {
        self.symmetry.type_
    }

    /// 旋转对称的线条数（`tree.py:864-870`）。
    #[inline]
    pub fn symmetry_lines(&self) -> i32 {
        self.symmetry.lines
    }

    /// 对称线角度（`tree.py:872-878`，单位度）。
    #[inline]
    pub fn symmetry_angle(&self) -> f32 {
        self.symmetry.angle
    }

    /// 设置中央对称状态并传播给当前层（`tree.py:880-922` 的
    /// `RootLayerStack.set_symmetry_state`）。
    ///
    /// `None` = "这一项没变"，与源码的用法一致（`tree.py:940-949` 的
    /// docstring 说明事件参数里的 None 就是这个意思）；中心按源码取整到
    /// 整数模型坐标（`:901-903`：`int(round(center[0])), int(round(center[1]))`）。
    ///
    /// 真正的 dab 展开在表面上（`pigment_surface::PigmentSurface::draw_dab`），
    /// 这里只负责"中央状态 → 当前层表面"这一跳（`:913-915`）。
    pub fn set_symmetry_state(
        &mut self,
        active: Option<bool>,
        center: Option<(i32, i32)>,
        symmetry_type: Option<SymmetryType>,
        symmetry_lines: Option<i32>,
        angle: Option<f32>,
    ) {
        if let Some(v) = active {
            self.symmetry.active = v;
        }
        if let Some(c) = center {
            self.symmetry.center = c;
        }
        if let Some(t) = symmetry_type {
            self.symmetry.type_ = t;
        }
        if let Some(l) = symmetry_lines {
            self.symmetry.lines = l;
        }
        if let Some(a) = angle {
            self.symmetry.angle = a;
        }
        let path = self.layers.current_path().to_vec();
        self.propagate_symmetry_to(&path);
    }

    /// 把中央状态复制给 `path` 指向的层（`tree.py:924-934`
    /// `_propagate_symmetry_state`）。
    ///
    /// 源码是 `layer.set_symmetry_state(...)`：绘画层把参数落到表面的 C 结构
    /// （`data.py:499-511`），组层命中 `LayerBase` 的空实现
    /// （`core.py:851-870`，注释原话 "The base implementation does nothing"）。
    /// 所以组层这里什么都不做 —— 与源码一致；这也意味着**组里的子层拿不到
    /// 对称**（上游行为，记账）。
    fn propagate_symmetry_to(&mut self, path: &[usize]) {
        let s = self.symmetry;
        if let Some(layer) = self.layers.layer_at_mut(path) {
            if let Some(surface) = layer.surface_mut() {
                // 参数顺序照 C++ 包装层 `TiledSurface::set_symmetry_state`
                // （`lib/tiledsurface.hpp:57-68`）与 `data.py:505-511` 的调用。
                surface.set_symmetry_state(
                    s.active,
                    s.center.0 as f32,
                    s.center.1 as f32,
                    s.type_,
                    s.lines,
                    s.angle,
                );
            }
        }
    }

    // ---- 洪水填充的包围盒（`lib/document.py:1031` 的 `flood_fill`）----

    /// 一次填充的像素包围盒（`Document.flood_fill` 的前半段，
    /// `lib/document.py:1064-1089`）。
    ///
    /// 为什么填充一定要有 bbox：画布是无限的，没有上界就没法开始（源码
    /// docstring 原话 "Filling an infinite canvas requires limits"）。规则：
    ///
    /// 1. 起点是 `get_effective_bbox()`（画框启用 → 画框；否则 → 内容外接框）；
    /// 2. 起点为空（空白层、且画框没启用）→ **退化成种子所在 tile 的外接框**
    ///    （`document.py:1067-1079`）。源码注释承认这"有点怪"，但它让空白层上
    ///    的大面积填充能一次 tile 一次 tile 地长大；
    /// 3. 画框没启用时，把每个种子点扩进 bbox（`document.py:1080-1082`）——
    ///    否则在已有内容的边缘外点击就填不动。
    ///
    /// `view_bbox` 是 "Limit to View"（`fill.py:222-224`）：调用方把视口四角
    /// 投到模型坐标后取外接框（`helpers.rotated_rectangle_bbox`）。规则照
    /// `document.py:1083-1088`：
    ///
    /// - 填充框**完全包含**视口 → 直接改成视口（把大画布上的填充限制在屏幕内）；
    /// - 两者相交 → 取交集；
    /// - 完全不相交 → 不限制（源码此时 `view_bbox` 没被用上）。
    pub fn fill_bbox(
        &self,
        seeds: &[(i32, i32)],
        view_bbox: Option<(i32, i32, i32, i32)>,
    ) -> (i32, i32, i32, i32) {
        let ts = crate::hokusai::tile::TILE_SIZE as i32;
        let mut rect = if self.frame_enabled {
            self.frame
        } else {
            self.data_bbox()
        };
        if rect.2 == 0 || rect.3 == 0 {
            // `document.py:1068-1079`：tile 对齐的种子外接框
            let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) = (
                seeds.iter().map(|s| s.0).min(),
                seeds.iter().map(|s| s.0).max(),
                seeds.iter().map(|s| s.1).min(),
                seeds.iter().map(|s| s.1).max(),
            ) else {
                // 没种子就无从填充；源码这里会因 `min([])` 抛异常
                return (0, 0, 0, 0);
            };
            let x = ts * min_x.div_euclid(ts);
            let y = ts * min_y.div_euclid(ts);
            let w = ts * max_x.div_euclid(ts) - x + ts;
            let h = ts * max_y.div_euclid(ts) - y + ts;
            rect = (x, y, w, h);
        } else if !self.frame_enabled {
            // `document.py:1080-1082`：画框没启用时把种子扩进 bbox
            for &(x, y) in seeds {
                expand_to_include_point(&mut rect, x, y);
            }
        }
        if let Some(view) = view_bbox {
            if rect_contains(rect, view) {
                rect = view;
            } else if rects_overlap(rect, view) {
                rect = rect_intersection(rect, view);
            }
        }
        rect
    }

    // ---- 取色（`gui/tileddrawwidget.py:977` 的 `pick_color`）----

    /// 取样"屏幕上看到"的颜色：整份图层栈的合成结果（`sample merged`）。
    ///
    /// 逐条照 `TiledDrawWidget.pick_color`（`tileddrawwidget.py:977-1013`）：
    ///
    /// 1. **取样源是合成结果，不是当前层**。源码注释直说"目前没法关掉这种
    ///    sample merged"（`:991-992`，issue #333），所以当前层是组、被锁、
    ///    被隐藏，都不影响取到的东西；
    /// 2. **含背景**：合成把背景层算进来（`LayerStack::composite_tile` 的先
    ///    BLIT 背景）；背景关闭时我们的渲染管线仍然落在纸色
    ///    `background_color` 上（CPU 预合成、无棋盘格，见模块记账）；
    /// 3. **色彩空间是 8bpc sRGB**：`GdkPixbuf` 是 sRGB，`new_from_pixbuf_average`
    ///    直接把 8 位 R/G/B 各自累加求平均，**不做任何线性化**
    ///    （`lib/color.py:223-262`）；
    /// 4. **alpha 被丢掉**：平均只累加 R/G/B，一个像素的 alpha 完全不参与
    ///    （`lib/color.py:245-262`）—— 返回的是 RGB，不是 RGBA；
    /// 5. **取样方块 3×3**：`size = 3`、`r = size // 2 = 1`，左上角是
    ///    `(int(x) - r, int(y) - r)`，也就是说取的是以取样点为中心的九宫格
    ///    平均（`:995-1007`）；
    /// 6. `use_filter=False`（`:1006`）：取样时**不套**显示滤镜（对比度/亮度
    ///    这类只影响屏幕观感的调整），所以我们也不套。
    ///
    /// 坐标是**模型坐标**：源码收到的是控件（设备）像素坐标，本引擎没有视图
    /// 变换，换算交给调用方 —— 缩放 1:1 时两者等价，缩放不为 1 时源码取的是
    /// "屏幕上的 3×3 像素"、我们取"画布上的 3×3 像素"，差异见模块记账。
    ///
    /// 返回 `[r, g, b]`（各分量 0..=1，sRGB）。坐标非有限值时返回 `None`；
    /// 除此之外永远有值（源码此处也有值与位置无关：黑底/棋盘格兜底）。
    /// 按**屏幕像素间距**取色（`tdw.pick_color`，`tileddrawwidget.py:977-1050`）。
    ///
    /// 源码在 device 坐标系里取 3×3 方形，再按当前视图变换渲染；本实现把
    /// 采样间距换成 `step = 1 / zoom` 个模型像素，取 3×3 个点求平均。
    /// 旋转视图下严格来说应取旋转后的方格，这里按轴对齐近似（记账）。
    pub fn pick_color_scaled(&self, x: f32, y: f32, step: f32) -> Option<[f32; 3]> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let step = if step.is_finite() && step > 0.0 {
            step
        } else {
            1.0
        };
        let mut acc = [0f32; 3];
        let mut n = 0f32;
        for dy in [-step, 0.0, step] {
            for dx in [-step, 0.0, step] {
                let px = (x + dx).floor() as i32;
                let py = (y + dy).floor() as i32;
                let mut pixels = vec![0u8; 4];
                // `with_alpha = false`：取"屏幕上看到的"合成分量
                self.layers
                    .render_rect_rgba8((px, py, 1, 1), false, &mut pixels);
                acc[0] += pixels[0] as f32;
                acc[1] += pixels[1] as f32;
                acc[2] += pixels[2] as f32;
                n += 1.0;
            }
        }
        Some([
            acc[0] / n / 255.0,
            acc[1] / n / 255.0,
            acc[2] / n / 255.0,
        ])
    }

    pub fn pick_color(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let size = PICK_COLOR_SIZE;
        let r = size / 2;
        // 源码 `int(x)`：向零截断（GTK 给的是整数；引擎里收的是浮点）
        let origin = (x as i32 - r, y as i32 - r);
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        // `with_alpha = false`：输出不透明像素，与"屏幕上看到的"一致
        self.layers.render_rect_rgba8(
            (origin.0, origin.1, size as u32, size as u32),
            false,
            &mut pixels,
        );
        let n_pixels = (size * size) as f32;
        let mut acc = [0f32; 3];
        for px in pixels.chunks_exact(4) {
            acc[0] += px[0] as f32;
            acc[1] += px[1] as f32;
            acc[2] += px[2] as f32;
        }
        // 源码先按像素数求平均、再除 255（`color.py:259-262`）
        Some([
            acc[0] / n_pixels / 255.0,
            acc[1] / n_pixels / 255.0,
            acc[2] / n_pixels / 255.0,
        ])
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
                    eotf: self.save_eotf,
                };
                // `save_ora` 传的是 `get_user_bbox()`（`document.py:1778`）：
                // 画框启用时就是画框本身，否则才是内容外接框。
                let bbox = self.get_user_bbox();
                let saved = fileio::save_ora(&self.layers, path, &opts, bbox)?;
                SaveOutcome::Ora { bbox: saved.bbox }
            }
            Some(Format::Png) => {
                let bbox = self.get_user_bbox();
                let saved = fileio::save_png(&self.layers, path, bbox, alpha.unwrap_or(true))?;
                SaveOutcome::Png { bbox: saved.bbox }
            }
            Some(Format::Jpg) => {
                // `save_jpg` 恒为不透明（JPEG 无 alpha），quality=90
                let bbox = self.get_user_bbox();
                let saved = fileio::save_jpg(&self.layers, path, bbox, 90)?;
                SaveOutcome::Jpg { bbox: saved.bbox }
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
        // 源码 `Document.load` 先 `clear()`，而 `RootLayerStack.clear()` 会把
        // 背景重置为**应用默认背景**（`reset_background` 设的纸纹）。文件里
        // 没写背景 tile 时保留它；写了就覆盖。
        self.reset_background();
        let outcome = match fileio::format_for_path(path) {
            Some(Format::Ora) => {
                let loaded = fileio::load_ora(path)?;
                // 组栈：XML 文档序 = 自顶向下；组递归成嵌套 LayerStack
                let vec = ora_layers_to_layers(loaded.layers);
                let mut st = LayerStack::from_layers(vec);
                if let Some(p) = &loaded.current_path {
                    st.set_current_path(p);
                }
                if loaded.background_found {
                    if let Some(img) = loaded.background_image {
                        st.set_background_image(img);
                    } else {
                        st.set_background_color(loaded.background_color);
                    }
                } else if let Some(paper) = Self::default_paper_background() {
                    // 文件没带背景 → 应用默认纸纹（源码 clear 的默认背景）
                    st.set_background_image(paper);
                }
                self.layers = st;
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
                    layer_count: self.layers.len(),
                    mypaint_version: loaded.mypaint_version,
                    compatibility: loaded.compatibility,
                    eotf: loaded.eotf,
                }
            }
            Some(Format::Png) => {
                let loaded = fileio::load_png(self.surface_mut(), path)?;
                // `load_png` 末行 `self.set_frame(bbox, user_initiated=False)`（`:1736`）
                self.set_frame(loaded.bbox.0, loaded.bbox.1, loaded.bbox.2, loaded.bbox.3);
                LoadOutcome::Png { bbox: loaded.bbox }
            }
            Some(Format::Jpg) => {
                let loaded = fileio::load_jpg(self.surface_mut(), path)?;
                self.set_frame(loaded.bbox.0, loaded.bbox.1, loaded.bbox.2, loaded.bbox.3);
                LoadOutcome::Jpg { bbox: loaded.bbox }
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
        // 命名视图总是随设置写盘（`LayerViewManager._doc_settings_sync_pending_changes_cb`，
        // `layervis.py:214-251`）；键名与结构逐字照源码。
        let mut obj: serde_json::Map<String, serde_json::Value> = self
            .settings
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        obj.insert(
            "layervis".to_string(),
            self.views.to_json(&self.layers),
        );
        serde_json::to_string_pretty(&serde_json::Value::Object(obj))
            .unwrap_or_else(|_| "{}".to_string())
    }

    /// 从 JSON 串恢复 `settings`（照 `:1871` 的 `_settings.update(json.loads(...))`）。
    ///
    /// 非法 JSON 时保持现有设置不变（源码会记日志并继续）。
    pub fn set_settings_json(&mut self, json: &str) {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(json) {
            self.settings = map.into_iter().collect();
            // `_doc_settings_modified_cb`（layervis.py:253-328）：重建视图并应用当前视图。
            if let Some(value) = self.settings.get("layervis").cloned() {
                self.views.load_json(&value, &mut self.layers);
            }
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
    Jpg { bbox: (i32, i32, i32, i32) },
}

/// [`Document::load`] 的结果。
#[derive(Debug, Clone)]
pub enum LoadOutcome {
    Ora {
        bbox: (i32, i32, i32, i32),
        /// 载入的图层数（不含背景层）。
        layer_count: usize,
        mypaint_version: Option<String>,
        compatibility: Option<fileio::Compatibility>,
        /// 文件里的 `mypaint:eotf`（缺失为 `None`）—— 兼容模式判定用。
        eotf: Option<f32>,
    },
    Png {
        bbox: (i32, i32, i32, i32),
    },
    Jpg {
        bbox: (i32, i32, i32, i32),
    },
}

/// 背景图单边最大"重复填充"尺寸（`backgroundwindow.py:39` `BLOAT_MAX_SIZE`）。
const BACKGROUND_BLOAT_MAX_SIZE: u32 = 1024;

/// 读一张背景图并整理成 tile 网格对齐（`backgroundwindow.py:359-481`
/// 的 `load_background`）：
///
/// 1. 解码 PNG/JPEG；带 alpha 的图先合成到**黑底**（源码同样处理，`:410-434`）；
/// 2. 尺寸不是 64 的整数倍时，先尝试重复平铺到能整除（最多 1024）；
/// 3. 仍不整除则缩放到 `max(1, w/64)*64`（双线性，源码 `scale_simple`）。
pub fn background_from_file(path: &Path) -> Result<BackgroundImage, String> {
    let img = image::open(path)
        .map_err(|e| format!("无法读取背景图 {}：{e}", path.display()))?
        .to_rgba8();
    let (w0, h0) = img.dimensions();
    if w0 == 0 || h0 == 0 {
        return Err(format!("背景图 {} 尺寸为 0", path.display()));
    }
    let mut img = img;
    // Flatten：alpha 合成到黑底，输出 alpha 恒 255。
    for px in img.pixels_mut() {
        let a = px[3] as u32;
        px[0] = ((px[0] as u32 * a) / 255) as u8;
        px[1] = ((px[1] as u32 * a) / 255) as u8;
        px[2] = ((px[2] as u32 * a) / 255) as u8;
        px[3] = 255;
    }
    let ts = crate::hokusai::tile::TILE_SIZE as u32;
    let (mut w, mut h) = img.dimensions();
    if w % ts != 0 || h % ts != 0 {
        let rx = best_repeats_for_scaling(w, BACKGROUND_BLOAT_MAX_SIZE);
        let ry = best_repeats_for_scaling(h, BACKGROUND_BLOAT_MAX_SIZE);
        if rx > 1 || ry > 1 {
            let mut tiled = image::RgbaImage::new(w * rx, h * ry);
            for yi in 0..ry {
                for xi in 0..rx {
                    image::imageops::replace(&mut tiled, &img, (xi * w) as i64, (yi * h) as i64);
                }
            }
            img = tiled;
            (w, h) = img.dimensions();
        }
        if w % ts != 0 || h % ts != 0 {
            w = (w / ts).max(1) * ts;
            h = (h / ts).max(1) * ts;
            img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        }
    }
    BackgroundImage::from_rgba8(w, h, img.as_raw())
}

/// `_best_nrepeats_for_scaling`（`backgroundwindow.py:494-508`）。
fn best_repeats_for_scaling(src_size: u32, max_dest_size: u32) -> u32 {
    let ts = crate::hokusai::tile::TILE_SIZE as u32;
    let mut min_remainder = ts;
    let mut best = 1;
    let mut n = 0;
    let mut dest = 0;
    while dest <= max_dest_size {
        n += 1;
        dest += src_size;
        let remainder = dest % ts;
        if remainder < min_remainder {
            best = n;
            min_remainder = remainder;
        }
        if remainder == 0 {
            break;
        }
    }
    best
}

/// `Rect.expand_to_include_point`（`lib/helpers.py:159-175`）的 `(x, y, w, h)` 版。
///
/// 只给 [`Document::fill_bbox`] 用：源码把种子点逐个并进 bbox，空矩形则直接
/// 变成 1×1。
fn expand_to_include_point(rect: &mut (i32, i32, i32, i32), x: i32, y: i32) {
    let (rx, ry, rw, rh) = *rect;
    if rw == 0 || rh == 0 {
        *rect = (x, y, 1, 1);
        return;
    }
    let (mut x0, mut y0, mut w, mut h) = (rx, ry, rw, rh);
    if x < x0 {
        w += x0 - x;
        x0 = x;
    }
    if y < y0 {
        h += y0 - y;
        y0 = y;
    }
    if x > x0 + w - 1 {
        w += x - (x0 + w - 1);
    }
    if y > y0 + h - 1 {
        h += y - (y0 + h - 1);
    }
    *rect = (x0, y0, w, h);
}

/// `Rect.contains`（`lib/helpers.py`）：`a` 完全包含 `b`。
fn rect_contains(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    aw > 0
        && ah > 0
        && bw > 0
        && bh > 0
        && bx >= ax
        && by >= ay
        && bx + bw <= ax + aw
        && by + bh <= ay + ah
}

/// `Rect.overlaps`（`lib/helpers.py`）：两个矩形有非空交集。
fn rects_overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    aw > 0
        && ah > 0
        && bw > 0
        && bh > 0
        && ax < bx + bw
        && bx < ax + aw
        && ay < by + bh
        && by < ay + ah
}

/// `Rect.intersection`（`lib/helpers.py`）。
fn rect_intersection(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    (x0, y0, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushlib::default_brush;
    use crate::stroke::StrokeEvent;
    use hokusai::{Brush, BrushSetting, BrushState, TiledSurface};

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
        let ts = crate::hokusai::tile::TILE_SIZE as i32;
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

    // ---- B2-5：DuplicateLayer 与图层属性命令的合并 ----

    // ---- 命名可见性视图（lib/layervis.py）----

    #[test]
    fn layer_views_are_undoable_and_track_visibility() {
        let mut doc = Document::new();
        doc.add_layer(); // 两层
        let top = doc.layers().current_id();

        assert!(doc.add_layer_view(None));
        assert_eq!(doc.current_layer_view_name(), Some("View 1"));
        assert_eq!(doc.layer_view_names(), vec!["View 1"]);

        // 当前视图激活时关掉顶层 → 记回 View 1
        doc.set_layer_visible(&[0], false);
        assert!(!doc.layers().layer(0).visible);

        // 再建一个视图：此时顶层隐藏
        doc.add_layer_view(Some("Other"));
        assert_eq!(doc.layer_view_names(), vec!["View 1", "Other"]);

        // 切回 View 1：顶层保持隐藏
        assert!(doc.activate_layer_view(Some("View 1")));
        assert!(!doc.layers().layer(0).visible);
        // 切到 Other 再撤销：回到 View 1
        assert!(doc.activate_layer_view(Some("Other")));
        let name = doc.current_layer_view_name().map(str::to_string);
        assert_eq!(name.as_deref(), Some("Other"));
        doc.undo();
        assert_eq!(doc.current_layer_view_name(), Some("View 1"));

        // 锁定位也走命令；锁定后 UI 会禁止改可见性
        assert!(doc.set_active_layer_view_locked(false));
        assert!(!doc.current_layer_view_locked());
        doc.undo();
        assert!(doc.current_layer_view_locked());

        // 删除当前视图 → 撤销恢复
        assert!(doc.remove_active_layer_view());
        assert!(doc.current_layer_view_name().is_none());
        doc.undo();
        assert_eq!(doc.current_layer_view_name(), Some("View 1"));
        assert_eq!(doc.layers().current_id(), top);
    }

    #[test]
    fn layer_views_roundtrip_through_settings_json() {
        let mut doc = Document::new();
        doc.add_layer_view(None);
        doc.add_layer_view(Some("Alt"));
        let json = doc.settings_json();
        assert!(json.contains("\"layervis\""), "设置里必须有 layervis 键");

        let mut other = Document::new();
        other.add_layer(); // 路径 (1,) 才存在
        other.set_settings_json(&json);
        assert_eq!(other.layer_view_names(), vec!["View 1", "Alt"]);
        assert_eq!(other.current_layer_view_name(), Some("Alt"));
    }

    #[test]
    fn duplicate_layer_lands_in_place_and_undo_restores_selection() {
        let mut doc = Document::new();
        let orig_id = doc.layers().current_id();
        assert!(doc.duplicate_layer());

        assert_eq!(doc.layers().len(), 2);
        assert_eq!(doc.layers().current_index(), 0, "副本落在原层位置（原位）");
        let copy_id = doc.layers().current_id();
        assert_ne!(copy_id, orig_id, "副本必须是新 id（照 deepcopy）");
        assert_eq!(doc.layers().layer(1).id, orig_id, "原层被推到其下");

        doc.undo();
        assert_eq!(doc.layers().len(), 1);
        assert_eq!(doc.layers().current_id(), orig_id, "撤销后当前层回到原层");

        doc.redo();
        assert_eq!(doc.layers().len(), 2);
        assert_eq!(doc.layers().current_index(), 0);
        assert_eq!(doc.layers().layer(1).id, orig_id);
    }

    #[test]
    fn duplicate_layer_redo_copies_the_current_layer_again() {
        let mut doc = Document::new();
        doc.duplicate_layer();
        doc.undo();
        // 撤销后又在原层上改名（等价于"撤销后继续改内容"），再 redo
        doc.rename_layer(&[0], Some("After Undo".to_string()));
        doc.redo();

        // 源码每次 redo 都重新 deepcopy(layers.current)，副本要含新改动
        assert_eq!(doc.layers().layer(0).name.as_deref(), Some("After Undo"));
    }

    #[test]
    fn opacity_drag_merges_into_one_undo_entry() {
        let mut doc = Document::new();
        let idx = doc.layers().current_index();
        for pct in [90, 80, 70, 60] {
            doc.set_layer_opacity(&[idx], pct as f32 / 100.0);
        }
        assert_eq!(doc.layers().layer(idx).opacity, 0.6);
        assert_eq!(doc.command_stack.undo_len(), 1, "滑条连续拖动只入栈一条");
        doc.undo();
        assert_eq!(doc.layers().layer(idx).opacity, 1.0, "撤销回到拖动前的值");
    }

    #[test]
    fn property_change_of_another_kind_is_not_swallowed() {
        let mut doc = Document::new();
        let idx = doc.layers().current_index();
        doc.set_layer_opacity(&[idx], 0.5);
        // 只比图层 id 的判据会误判成"可合并"，这句改动会被 update 拒绝后丢弃
        doc.set_layer_visible(&[idx], false);

        assert!(!doc.layers().layer(idx).visible, "可见性改动不能被吃掉");
        assert_eq!(doc.command_stack.undo_len(), 2);
    }

    #[test]
    fn repeated_visibility_toggles_merge_but_keep_the_first_old_value() {
        let mut doc = Document::new();
        let idx = doc.layers().current_index();
        doc.set_layer_visible(&[idx], false);
        doc.set_layer_visible(&[idx], true);
        assert_eq!(doc.command_stack.undo_len(), 1);
        assert!(doc.layers().layer(idx).visible);
        doc.undo();
        assert!(doc.layers().layer(idx).visible, "撤销回到最初（可见）");
    }

    #[test]
    fn renames_merge_and_undo_returns_to_the_original_name() {
        let mut doc = Document::new();
        let idx = doc.layers().current_index();
        doc.rename_layer(&[idx], Some("A".to_string()));
        doc.rename_layer(&[idx], Some("B".to_string()));
        assert_eq!(doc.command_stack.undo_len(), 1, "连续重命名只留栈顶一条");
        assert_eq!(doc.layers().layer(idx).name.as_deref(), Some("B"));

        doc.undo();
        assert_eq!(doc.layers().layer(idx).name, None, "回到最初的无自定义名");
    }

    /// 图层属性命令必须抬高**合成版本号**（GPU 上传水位线），否则
    /// 「隐藏图层」后画布还是旧内容 —— B2-5 GUI 实测抓到的真 bug。
    #[test]
    fn layer_property_commands_bump_the_composite_revision() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        let idx = doc.layers().current_index();

        let r0 = doc.layers().composite_revision();
        doc.set_layer_visible(&[idx], false);
        let r1 = doc.layers().composite_revision();
        assert_ne!(r0, r1, "可见性变化要抬高水位线");
        doc.set_layer_opacity(&[idx], 0.5);
        let r2 = doc.layers().composite_revision();
        assert_ne!(r1, r2, "不透明度变化要抬高水位线");
        doc.set_layer_mode(&[idx], LayerMode::Standard(pigment_surface::combine::CombineMode::Multiply));
        let r3 = doc.layers().composite_revision();
        assert_ne!(r2, r3, "混合模式变化要抬高水位线");

        doc.undo();
        assert_ne!(r3, doc.layers().composite_revision(), "撤销同样要抬高");
    }

    /// 隐藏图层后**合成结果**也要变（CPU 侧），与水位线一致。
    #[test]
    fn hiding_a_layer_removes_it_from_the_composite() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        let idx = doc.layers().current_index();
        assert!(doc.layers().has_paint_at(0, 0), "先确认有笔迹");
        let before = doc.layers().composite_tile_rgba8(0, 0).unwrap();

        doc.set_layer_visible(&[idx], false);
        let after = doc.layers().composite_tile_rgba8(0, 0);
        // 背景仍可见 → tile 还在，但内容应是纯背景色（白）
        let after = after.expect("背景可见时 tile 仍有内容");
        assert_ne!(&before[..], &after[..], "隐藏后合成像素应变化");
        assert!(
            after.chunks_exact(4).all(|p| p[0] == 255 && p[1] == 255 && p[2] == 255),
            "画布只剩背景层 → 全白"
        );
    }

    // ---- B3-3：洪水填充的包围盒（`document.py:1064-1089`）----

    /// 空白文档没有内容外接框 → 退化成"种子所在 tile"（`document.py:1067-1079`）。
    #[test]
    fn fill_bbox_degrades_to_the_seed_tile_on_a_blank_document() {
        let doc = Document::new();
        let ts = crate::hokusai::tile::TILE_SIZE as i32;
        assert_eq!(doc.data_bbox(), (0, 0, 0, 0));
        // 单个种子 → 一块 tile
        assert_eq!(doc.fill_bbox(&[(10, 10)], None), (0, 0, ts, ts));
        // 跨 tile 的种子 → 覆盖两者的外接框
        assert_eq!(
            doc.fill_bbox(&[(10, 10), (70, -1)], None),
            (0, -ts, 2 * ts, 2 * ts),
            "负数种子按向下取整落在 tile -1"
        );
    }

    fn bg_tmpdir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pigment-bg-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 背景图载入：alpha 合成到黑底、尺寸缩放到 64 的整数倍。
    #[test]
    fn background_from_file_flattens_alpha_and_fits_the_tile_grid() {
        let dir = bg_tmpdir("load");
        let path = dir.join("bg.png");
        let (w, h) = (100u32, 70u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.chunks_exact_mut(4) {
            p.copy_from_slice(&[200, 100, 50, 128]);
        }
        pigment_surface::png::write_rgba8_png(&path, px, w, h).unwrap();
        let img = background_from_file(&path).unwrap();
        assert_eq!(img.width() % 64, 0, "宽要落在 tile 网格上");
        assert_eq!(img.height() % 64, 0, "高要落在 tile 网格上");
        assert!(img.width() >= 64 && img.height() >= 64);
        assert_eq!(img.rgba8()[3], 255, "alpha 必须被压平");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `reset_background_from` 优先用 `<user>/backgrounds/default.png`
    /// （`gui/document.py:1332-1344`）。
    #[test]
    fn reset_background_prefers_the_user_default() {
        let dir = bg_tmpdir("default");
        let bgs = dir.join("backgrounds");
        std::fs::create_dir_all(&bgs).unwrap();
        let path = bgs.join("default.png");
        let px = vec![[10u8, 20, 30, 255]; 64 * 64].concat();
        pigment_surface::png::write_rgba8_png(&path, px, 64, 64).unwrap();

        let mut doc = Document::new();
        doc.reset_background_from(Some(&dir));
        let img = doc.layers().background_image().expect("载入用户背景");
        assert_eq!(img.average_rgb(), [10, 20, 30]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_background_from_file_reports_missing_files() {
        let mut doc = Document::new();
        let err = doc
            .set_background_from_file(std::path::Path::new("definitely-missing-bg.png"))
            .unwrap_err();
        assert!(err.contains("无法读取背景图"), "{err}");
    }

    /// "Limit to View"（`document.py:1083-1088`）：包含→取视口，相交→取交集，
    /// 不相交→不限制。
    #[test]
    fn fill_bbox_limit_to_view_uses_contains_and_intersection() {
        let mut doc = Document::new();
        doc.layers_mut().set_background_visible(false);
        // 造一块 0..64 的内容
        {
            let surface = doc.surface_mut();
            let tile = surface.tile_request_start(0, 0);
            for row in tile.iter_mut() {
                for px in row.iter_mut() {
                    *px = [1, 1, 1, 1];
                }
            }
            surface.tile_request_end(0, 0);
        }
        assert_eq!(doc.fill_bbox(&[(10, 10)], None), (0, 0, 64, 64));
        // 视口完全在 bbox 内 → 直接取视口
        assert_eq!(
            doc.fill_bbox(&[(10, 10)], Some((8, 8, 10, 10))),
            (8, 8, 10, 10)
        );
        // 部分相交 → 交集
        assert_eq!(
            doc.fill_bbox(&[(10, 10)], Some((48, 48, 40, 40))),
            (48, 48, 16, 16)
        );
        // 完全不相交 → 不限制
        assert_eq!(
            doc.fill_bbox(&[(10, 10)], Some((200, 200, 10, 10))),
            (0, 0, 64, 64)
        );
    }

    /// 画框启用时直接用画框；否则用内容外接框并把种子并进来。
    #[test]
    fn fill_bbox_uses_the_frame_or_expands_to_the_seeds() {
        let mut doc = Document::new();
        paint_and_commit(&mut doc, 2.0);
        let content = doc.data_bbox();

        // 种子在内容里 → bbox 就是内容外接框
        assert_eq!(doc.fill_bbox(&[(10, 20)], None), content);

        // 种子远在内容之外 → 并进来（`document.py:1080-1082`）
        let far = content.0 + content.2 + 100;
        let bbox = doc.fill_bbox(&[(far, content.1)], None);
        assert!(bbox.0 + bbox.2 > far, "外接框必须包含远处种子: {bbox:?}");

        // 画框启用 → 只认画框，种子再远也不扩
        doc.set_frame(0, 0, 30, 40);
        doc.set_frame_enabled(true);
        assert_eq!(doc.fill_bbox(&[(far, far)], None), (0, 0, 30, 40));
    }

    // ---- B3-4：取色（`gui/tileddrawwidget.py:977` 的 `pick_color`）----

    /// 用 8bpc 像素直接铺一块（绕开笔刷，颜色可精确断言）。
    fn fill_block(doc: &mut Document, x: i32, y: i32, w: u32, h: u32, px: [u8; 4]) {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&px);
        }
        doc.surface_mut().load_from_rgba8(&buf, x, y, w, h);
    }

    /// 取样方块是 3×3、以取样点为中心，且结果是九宫格的**算术平均**：
    /// 左上角 `(int(x) - 1, int(y) - 1)`（`tileddrawwidget.py:995-1007`），
    /// 平均在 8bpc sRGB 上做（`lib/color.py:245-262`）。
    #[test]
    fn pick_color_averages_a_centred_3x3_sample_square() {
        let mut doc = Document::new();
        // (0,0)-(2,2) 三列三色：x=0 红、x=1 灰、x=2 蓝
        fill_block(&mut doc, 0, 0, 1, 3, [204, 51, 51, 255]);
        fill_block(&mut doc, 1, 0, 1, 3, [128, 128, 128, 255]);
        fill_block(&mut doc, 2, 0, 1, 3, [51, 51, 204, 255]);

        // 取样点 (1,1) → 方块 (0,0)-(2,2)，正好盖满九宫格
        let got = doc.pick_color(1.0, 1.0).expect("有限坐标永远有值");
        let want_r = (204.0 + 128.0 + 51.0) / 3.0 / 255.0;
        let want_g = (51.0 + 128.0 + 51.0) / 3.0 / 255.0;
        let want_b = (51.0 + 128.0 + 204.0) / 3.0 / 255.0;
        assert!((got[0] - want_r).abs() < 2.0 / 255.0, "R {} vs {want_r}", got[0]);
        assert!((got[1] - want_g).abs() < 2.0 / 255.0, "G {} vs {want_g}", got[1]);
        assert!((got[2] - want_b).abs() < 2.0 / 255.0, "B {} vs {want_b}", got[2]);

        // 只铺了单像素时更能看出方块的位置：取样点 (0,0) → 方块 (-1,-1)-(1,1)
        // 里只有 1 个红像素，其余 8 格是背景白
        let mut doc = Document::new();
        fill_block(&mut doc, 0, 0, 1, 1, [204, 51, 51, 255]);
        let got = doc.pick_color(0.0, 0.0).expect("有限坐标永远有值");
        let want_r = (204.0 + 8.0 * 255.0) / 9.0 / 255.0;
        assert!((got[0] - want_r).abs() < 2.0 / 255.0, "R {} vs {want_r}", got[0]);
    }

    /// 取样源是**合成结果**（含背景、跳过隐藏层），不是当前层：
    /// `colorpicker.py:143-145` 用的就是"屏幕上看到的"那一路。
    #[test]
    fn pick_color_scaled_matches_the_pixel_square_at_zoom_one() {
        let mut doc = Document::new();
        fill_block(&mut doc, 0, 0, 1, 3, [204, 51, 51, 255]);
        fill_block(&mut doc, 1, 0, 1, 3, [128, 128, 128, 255]);
        fill_block(&mut doc, 2, 0, 1, 3, [51, 51, 204, 255]);
        let a = doc.pick_color(1.0, 1.0).unwrap();
        let b = doc.pick_color_scaled(1.0, 1.0, 1.0).unwrap();
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1.0 / 255.0, "通道 {i}: {a:?} vs {b:?}");
        }
        // 放大 4 倍：取样间距 0.25 模型像素 → 9 个样本都落在同一个源像素，
        // 结果应等于该像素本身的颜色（(1,1) 是灰色那一列）。
        let c = doc.pick_color_scaled(1.5, 1.5, 0.25).unwrap();
        assert!((c[0] * 255.0 - 128.0).abs() <= 1.0);
        assert!((c[1] * 255.0 - 128.0).abs() <= 1.0);
        assert!((c[2] * 255.0 - 128.0).abs() <= 1.0);
    }

    #[test]
    fn pick_color_samples_the_composite_including_the_background() {
        let mut doc = Document::new();
        // 空白画布 → 背景白（默认 255,255,255，且背景可见）
        let got = doc.pick_color(4.0, 4.0).expect("有限坐标永远有值");
        assert!((got[0] - 1.0).abs() < 1e-6 && (got[1] - 1.0).abs() < 1e-6);

        // 换背景色 → 取色跟着变（背景参与合成）
        doc.layers_mut().set_background_color([10, 200, 30]);
        let got = doc.pick_color(4.0, 4.0).expect("有限坐标永远有值");
        assert!((got[0] * 255.0 - 10.0).abs() <= 2.0, "R {}", got[0] * 255.0);
        assert!((got[1] * 255.0 - 200.0).abs() <= 2.0, "G {}", got[1] * 255.0);

        // 有内容的层盖住背景
        fill_block(&mut doc, 0, 0, 8, 8, [204, 51, 51, 255]);
        let got = doc.pick_color(4.0, 4.0).expect("有限坐标永远有值");
        assert!((got[0] * 255.0 - 204.0).abs() <= 2.0, "应取到红，R {}", got[0] * 255.0);

        // 隐藏这层 → 它不再参与合成，回到背景色
        let idx = doc.layers().current_index();
        doc.set_layer_visible(&[idx], false);
        let got = doc.pick_color(4.0, 4.0).expect("有限坐标永远有值");
        assert!((got[0] * 255.0 - 10.0).abs() <= 2.0, "隐藏后应回到背景，R {}", got[0] * 255.0);
    }

    /// 非有限坐标没有对应的像素，返回 `None`（源码此处永远有值，但那是
    /// GTK 事件坐标保证的；本移植要挡住 NaN/Inf）。
    #[test]
    fn pick_color_rejects_non_finite_coordinates() {
        let doc = Document::new();
        assert!(doc.pick_color(f32::NAN, 0.0).is_none());
        assert!(doc.pick_color(0.0, f32::INFINITY).is_none());
        assert!(doc.pick_color(f32::NEG_INFINITY, 0.0).is_none());
    }

    /// 兼容模式的两个落点：新层默认混合模式、保存时的 EOTF 标签。
    #[test]
    fn default_layer_mode_and_saved_eotf_follow_compat() {
        use pigment_surface::combine::CombineMode;
        let mut doc = Document::new();
        doc.default_layer_pigment = false;
        doc.add_layer();
        assert_eq!(
            doc.layers().current().mode,
            LayerMode::Standard(CombineMode::Normal),
            "1.x 新层应为 Normal"
        );
        doc.default_layer_pigment = true;
        doc.add_layer();
        assert_eq!(
            doc.layers().current().mode,
            LayerMode::Standard(CombineMode::SpectralWGM),
            "2.x 新层应为 Pigment"
        );

        // 1.x 保存写 mypaint:eotf="1.0"，peek 能读回
        let path = std::env::temp_dir().join(format!(
            "pigment-compat-save-{}.ora",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        doc.save_eotf = Some(1.0);
        doc.save(&path, None).unwrap();
        let meta = crate::fileio::peek_ora(&path).unwrap();
        assert_eq!(meta.eotf, Some(1.0));
        // unknown：不写标签
        doc.save_eotf = None;
        doc.save(&path, None).unwrap();
        assert_eq!(crate::fileio::peek_ora(&path).unwrap().eotf, None);
        let _ = std::fs::remove_file(&path);
    }
}
