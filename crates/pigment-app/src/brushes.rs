//! 笔刷面板 —— 照搬 MyPaint 的 `BrushGroupTool`：**一个面板 = 一个分组**。
//!
//! 源码对照：
//!
//! | 本文件 | MyPaint | 位置 |
//! |---|---|---|
//! | [`ICON_SIZE`] / [`cell`] | `BrushList.ICON_SIZE = 48` | `gui/brushselectionwindow.py:66` |
//! | [`total_border`] / [`grid_geometry`] | `PixbufList.update` 的 `total_w`/`tiles_w` | `gui/pixbuflist.py:240` |
//! | [`index_at`] | `PixbufList.index` | `gui/pixbuflist.py:308` |
//! | [`stroke_selection`] | `PixbufList.draw_cb` | `gui/pixbuflist.py:373` |
//! | [`thumb_for`] | `managedbrush_pixbuffunc` → `helpers.pixbuf_thumbnail` | `lib/helpers.py:454` |
//! | `tooltips` | `managedbrush_namefunc` | `gui/brushselectionwindow.py:46` |
//! | `BrushPanel::header` | `_Notebook._make_tab_label` + `BrushGroupTool.tool_widget_*` | `gui/toolstack.py:464`、`gui/brushselectionwindow.py:384` |
//! | [`header_icon_for`] | `BrushGroupTool.tool_widget_get_icon_pixbuf` | `gui/brushselectionwindow.py:401` |
//! | `BrushPanel::group_menu` | `BrushGroupsMenu` | `gui/brushselectionwindow.py:550` |
//! | [`brush_count_desc`] | `tool_widget_description` 的 ngettext | `gui/brushselectionwindow.py:388` |
//! | [`TOOL_WIDGET_MIN_WIDTH`] | `TOOL_WIDGET_MIN_WIDTH` | `gui/toolstack.py:40` |
//!
//! MyPaint 的形态：**每个分组一个可停靠的 `BrushGroupTool` 面板**。面板页签上是
//! 组内**第一支**笔刷的预览图（`TAB_ICON_SIZE` = 16px，拉伸到正方），
//! tooltip 是 `<b>{标题}</b>\n{N brush(es)}`；面板内容是**该分组的** `BrushList`
//! （48px 流式网格）。分组之间靠工具条上的 `BrushGroupsMenu`
//! （「New Group…／Import Brushes…／Get More Brushes…／各分组」）来开面板。
//!
//! 三处**技术栈适配**（结果等价，手段不同）：
//!
//! 1. **一分组一面板 → 单面板 + 分组菜单**：egui 的 `Panel` 是固定 id 的，
//!    没有 `Gtk.Notebook` 那种「为每个分组动态开一页」的能力。这里用一个左侧
//!    面板显示当前分组，菜单项语义等价于 MyPaint 的
//!    `workspace.add_tool_widget(BrushGroupTool, (name,))`（`:618`）。
//! 2. **按需建纹理**：`PixbufList.update()` 会为列表里**每一项**立刻取 pixbuf
//!    （196 张 128×128 常驻 RAM）。我们只给滚动视口内的项建 GPU 纹理，
//!    否则 egui 里会一次性炸出 196 个纹理绑定。
//! 3. **跨分组搜索框 + 重扫按钮**（本项目新增，MyPaint 无这两样）：官方包
//!    196 支笔刷按分组翻找较慢，保留一个跨分组搜索入口；**搜索框为空时面板
//!    行为与 MyPaint 完全一致**。
//!
//! 权限：官方包图标为 CC0，见 `assets/brushes/COPYING`。

use std::collections::{HashMap, VecDeque};

use eframe::egui;
use pigment_engine::brushlib::{translate_group_name, BrushEntry, BrushLibrary};
use pigment_engine::hokusai::Brush;

use crate::thumb;

/// 图标边长 —— `BrushList.ICON_SIZE`（`gui/brushselectionwindow.py:66`）。
pub const ICON_SIZE: f32 = 48.0;
/// 工具面板最小宽度 —— `TOOL_WIDGET_MIN_WIDTH`（`gui/toolstack.py:40`）。
pub const TOOL_WIDGET_MIN_WIDTH: f32 = 220.0;
/// 面板头图标边长 —— `TAB_ICON_SIZE = Gtk.IconSize.MENU`（`gui/toolstack.py:203`）。
///
/// GTK3 的 `Gtk.IconSize.MENU` 就是 16×16。
pub const TAB_ICON_SIZE: f32 = 16.0;
/// 选中框线宽 —— `PixbufList.border_visible`（`gui/pixbuflist.py:69`）。
const BORDER_VISIBLE: f32 = 2.0;
/// 格内留白 —— `PixbufList.spacing_inside`。
const SPACING_INSIDE: f32 = 0.0;
/// 格外留白 —— `PixbufList.spacing_outside`。
const SPACING_OUTSIDE: f32 = 0.0;
/// 选中框外扩 —— `PixbufList.border_visible_outside_cell`。
const BORDER_VISIBLE_OUTSIDE_CELL: f32 = 1.0;

/// MyPaint 的启动默认笔刷 —— 对齐 `gui/brushmanager.py` 的 `_DEFAULT_BRUSH`。
pub const DEFAULT_BRUSH: &str = "Dieterle/Fan#1";
/// MyPaint 的默认橡皮 —— 对齐 `_DEFAULT_ERASER`。
///
/// 官方包实际目录名是全小写 `deevad/`，order.conf 里的分组展示名才是 `Deevad`。
#[allow(dead_code)]
pub const DEFAULT_ERASER: &str = "deevad/kneaded_eraser_large";
/// `DEFAULT_BRUSH` 缺失时的兜底（官方包被裁剪等情况）。
const FALLBACK_BRUSH: &str = "classic/dry_brush";

/// 面板描述文字 —— `BrushGroupTool.tool_widget_description`
/// （`gui/brushselectionwindow.py:388`）。
///
/// 原式 `ngettext("{count} brush", "{count} brushes", nbrushes)`：
/// 单数取第一式，其余（含 0）取第二式。
fn brush_count_desc(n: usize) -> String {
    if n == 1 {
        "1 brush".to_string()
    } else {
        format!("{n} brushes")
    }
}

/// `PixbufList.update()` 的 `total_border`（`gui/pixbuflist.py:243`）。
fn total_border() -> f32 {
    BORDER_VISIBLE + SPACING_INSIDE + SPACING_OUTSIDE
}

/// 一个格子的边长 —— 同函数的 `total_w` / `total_h`。
fn cell() -> f32 {
    ICON_SIZE + 2.0 * total_border()
}

/// 网格几何 —— `PixbufList.update()` 的 `tiles_w` / `tiles_h`。
///
/// 返回 `(每行格数, 总行数)`。
fn grid_geometry(width: f32, count: usize) -> (usize, usize) {
    let c = cell();
    let tiles_w = ((width / c) as usize).max(1);
    let tiles_h = count.div_ceil(tiles_w).max(1);
    (tiles_w, tiles_h)
}

/// 命中测试 —— `PixbufList.index`（`gui/pixbuflist.py:301-311`）。
///
/// 坐标为**内容坐标**（相对网格左上角）。只把列/行夹到非负与最后一列；
/// **不**把结果夹到 `count-1`：源码返回原始格子下标，调用方再用
/// `i < len(itemlist)` 拒绝点空白处（`pixbuflist.py:322-325`）。
fn index_at(x: f32, y: f32, tiles_w: usize) -> usize {
    if tiles_w == 0 {
        return 0;
    }
    let c = cell();
    let mut col = (x / c).floor() as i64;
    col = col.clamp(0, tiles_w as i64 - 1);
    let row = (y / c).floor() as i64;
    (col + tiles_w as i64 * row).max(0) as usize
}

/// 笔刷历史长度（`brushmanager.py:_BRUSH_HISTORY_SIZE = 5`）。
pub const BRUSH_HISTORY_SIZE: usize = 5;

/// 一条笔刷历史：源码存的是 `ManagedBrush` 克隆（含当时的全部设置），
/// 预览图取**被选中笔刷**的 `preview`（`brushmanager.py:1089-1093`）。
#[derive(Clone)]
pub struct BrushHistoryEntry {
    /// 作画时的实际笔刷（含编辑器改动）。
    pub brush: Brush,
    /// `parent_brush_name` —— 历史去重的身份（源码首选判据）。
    pub name: String,
    /// 预览图对应的 library 下标（用于 `BrushPanel::icon`）。
    pub preview: usize,
}

/// 一个分组面板 —— MyPaint 的 `BrushGroupTool`（`gui/brushselectionwindow.py:347`）。
pub struct BrushPanel {
    pub library: BrushLibrary,
    /// 当前选中项在 `library.entries` 中的下标。
    pub selected: usize,
    /// 面板当前显示的**分组 id** —— `BrushGroupTool._group`。
    ///
    /// 注意本项目这里存的是资产**目录名**（`deevad`），而 MyPaint 的 `_group`
    /// 是 `order.conf` 的 `Group:` 名（`Deevad`）。面板标题一律走
    /// `BrushEntry::group_display` / `GroupInfo::title()`，所以显示结果一致；
    /// 差异只在 key 语义（影响 `order.conf` 三方合并与拖放排序，见比对报告 P1）。
    pub group: String,
    /// 跨分组搜索关键字（本项目**新增**入口，MyPaint 无此控件）。
    pub search: String,
    /// 缩略图缓存：library 下标 → 纹理（扮演 `PixbufList.thumbnails` 的角色）。
    thumbnails: HashMap<usize, egui::TextureHandle>,
    /// 面板头图标缓存：`(笔刷下标, 边长)` → 纹理。
    header_icons: HashMap<(usize, usize), egui::TextureHandle>,
    /// **原始尺寸**预览图缓存：笔刷下标 → 128×128 纹理。
    ///
    /// 底栏笔刷指示器要用：`footer.py:_draw_cb` 自己算缩放与对齐
    /// （等比缩放到 `max(aw,ah)-2`、顶部对齐），所以取图时不能把比例定死。
    previews: HashMap<usize, egui::TextureHandle>,
    /// 面板内容的 library 下标列表（`BrushList.brushes` 的下标形式）。
    visible: Vec<usize>,
    /// 与 [`Self::visible`] 一一对应的 tooltip 文本（`managedbrush_namefunc` 的结果）。
    ///
    /// 在 [`Self::rebuild_visible`] 里一并算好：tooltip 需要闭包读它，
    /// 提前算好就不必在绘制闭包里再借 `&self`。
    tooltips: Vec<String>,
    /// 最近用过的笔刷（最旧 → 最新，`_BRUSH_HISTORY_SIZE` 条）。
    pub history: VecDeque<BrushHistoryEntry>,
    /// 拖放起点（`visible` 下标）与当前悬停落点 —— `PixbufList` 的 DnD。
    pub drag_from: Option<usize>,
    pub drag_hover: Option<usize>,
    /// 右键菜单目标：`(entries 下标, 屏幕位置)`。
    pub context: Option<(usize, egui::Pos2)>,
    /// 「New Group…」对话框的输入缓冲；`None` = 未打开。
    pub new_group: Option<String>,
    /// 对话框首帧是否还要抢一次焦点（`request_focus()` 不能每帧调）。
    pub new_group_focus: bool,
    /// 删除确认对话框的目标（`entries` 下标）。
    pub delete_confirm: Option<usize>,
    /// 省略 `paint_mode` 的笔刷默认是否开颜料 ——
    /// `BrushManager.set_pigment_by_default`（`brushmanager.py:609-643`）：
    /// 2.x 兼容模式 true、1.x false。由 `PigmentApp::apply_compat` 维护。
    pub pigment_by_default: bool,
    /// 分组菜单请求：导入笔刷包 / 打开下载页 / 导出当前分组（由 app 消费）。
    pub import_requested: bool,
    pub download_requested: bool,
    pub export_group_requested: bool,
}

impl BrushPanel {
    pub fn new() -> Self {
        let library = BrushLibrary::load();
        // 对齐 MyPaint：默认选中 Dieterle/Fan#1（`brushmanager.py:_DEFAULT_BRUSH`）
        let selected = library
            .find(DEFAULT_BRUSH)
            .or_else(|| library.find(FALLBACK_BRUSH))
            .unwrap_or(0);
        // 面板初始分组 —— 对齐 `_init_default_brushkeys_and_history` 挑启动分组的规则
        // （`set#2` 不存在 → 成员最多的分组）
        let group = library.startup_group().unwrap_or_default();
        let mut p = Self {
            library,
            selected,
            group,
            search: String::new(),
            thumbnails: HashMap::new(),
            header_icons: HashMap::new(),
            previews: HashMap::new(),
            visible: Vec::new(),
            tooltips: Vec::new(),
            history: VecDeque::new(),
            drag_from: None,
            drag_hover: None,
            context: None,
            new_group: None,
            new_group_focus: false,
            delete_confirm: None,
            pigment_by_default: true,
            import_requested: false,
            download_requested: false,
            export_group_requested: false,
        };
        // `_init_default_brushkeys_and_history`（brushmanager.py:463-474）：
        // 空的历史槽位用启动分组的前 5 支补满（`group_i = i % len`）。
        let seed: Vec<usize> = p
            .library
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.group == p.group)
            .map(|(i, _)| i)
            .take(BRUSH_HISTORY_SIZE)
            .collect();
        for idx in seed {
            if let Some(entry) = p.library.entries.get(idx) {
                if let Ok(brush) = entry.load() {
                    let name = entry.mypaint_name();
                    p.history.push_back(BrushHistoryEntry {
                        brush,
                        name,
                        preview: idx,
                    });
                }
            }
        }
        p.rebuild_visible();
        p
    }

    pub fn selected_entry(&self) -> Option<&BrushEntry> {
        self.library.entries.get(self.selected)
    }

    /// 某支笔刷的 48×48 图标（与我方笔刷列表用的是同一张，含缓存）。
    ///
    /// 供笔刷编辑器顶部复用 —— MyPaint 那里也是直接取 `ManagedBrush.preview`。
    pub fn icon(&mut self, ctx: &egui::Context, idx: usize) -> egui::TextureHandle {
        thumb_for(ctx, &self.library, &mut self.thumbnails, idx)
    }

    /// 某支笔刷的**原始预览图**纹理（128×128，既不缩放也不合成白底）。
    ///
    /// 供底栏笔刷指示器使用 —— MyPaint 那里画的就是 `ManagedBrush.preview`
    /// 原图（`gui/footer.py:151`），缩放与对齐由指示器自己按 `_draw_cb` 算。
    pub fn preview(&mut self, ctx: &egui::Context, idx: usize) -> egui::TextureHandle {
        if let Some(t) = self.previews.get(&idx) {
            return t.clone();
        }
        let (w, h) = (thumb::PREVIEW_W, thumb::PREVIEW_H);
        let rgba = raw_icon(&self.library, idx, w, h);
        let tex = ctx.load_texture(
            format!("preview:{idx}"),
            icon_image(&rgba, w, h),
            egui::TextureOptions::LINEAR,
        );
        self.previews.insert(idx, tex.clone());
        tex
    }

    /// 解析当前选中笔刷（解析失败极少见，调用方以兜底笔刷处理）。
    pub fn load_selected(&self) -> Result<Brush, String> {
        self.library
            .entries
            .get(self.selected)
            .ok_or_else(|| "笔刷库为空".to_string())?
            .load_with_default(self.pigment_by_default)
            .map_err(|e| e.to_string())
    }

    /// 把已解析/校验过的笔刷包落盘并重新入库
    /// （`BrushManager.import_brushpack`，`brushmanager.py:647-812` 的 app 侧）。
    pub fn import_pack(
        &mut self,
        pack: &pigment_engine::brushpack::Brushpack,
    ) -> Result<pigment_engine::brushpack::ImportReport, String> {
        let existing_keys: std::collections::HashSet<String> =
            self.library.entries.iter().map(|e| e.key.clone()).collect();
        let existing_groups: std::collections::HashSet<String> =
            self.library.groups.iter().map(|g| g.id.clone()).collect();
        let user_dir = self.library.user_brush_dir();
        let report = pigment_engine::brushpack::install_brushpack(
            pack,
            &user_dir,
            &existing_keys,
            &existing_groups,
        )
        .map_err(|e| format!("写入笔刷失败：{e}"))?;
        self.library.scan_dir(&user_dir);
        let _ = self.library.save_order_conf();
        self.after_brushpack_import();
        Ok(report)
    }

    /// 笔刷包导入后的缓存刷新（缩略图/预览/可见列表）。
    pub fn after_brushpack_import(&mut self) {
        self.thumbnails.clear();
        self.header_icons.clear();
        self.previews.clear();
        self.rebuild_visible();
    }

    /// 记录一笔用过的笔刷 —— `BrushManager._input_stroke_ended_cb`
    /// （`brushmanager.py:1064-1100`）。
    ///
    /// 源码的判据：**优先比对 `parent_brush_name`**（同一支笔在编辑器里改过
    /// 设置也只算一条，历史里保留最新设置）；没有 parent 名时退回
    /// `brushinfo.matches()`。本移植的笔刷都有 MyPaint 名，所以只做前者；
    /// 后者（老 `.ora` 里没有 parent 名的笔画）记账。
    pub fn push_history(&mut self, brush: Brush, name: String, preview: usize) {
        if let Some(i) = self.history.iter().position(|e| e.name == name) {
            self.history.remove(i);
        }
        self.history.push_back(BrushHistoryEntry {
            brush,
            name,
            preview,
        });
        while self.history.len() > BRUSH_HISTORY_SIZE {
            self.history.pop_front();
        }
    }

    /// 历史里的 `i` 项（最旧 = 0）—— `BrushHistoryView` 的 `bm.history[i]`。
    pub fn history_entry(&self, i: usize) -> Option<&BrushHistoryEntry> {
        self.history.get(i)
    }

    /// 在面板内容中前后移动选择，返回是否发生变化。
    ///
    /// MyPaint 的 `,` / `.` 也是在**当前列表**（= 当前分组）里前后走。
    pub fn step(&mut self, delta: isize) -> bool {
        if self.visible.is_empty() {
            return false;
        }
        let cur = self.visible.iter().position(|&i| i == self.selected);
        let next = match cur {
            Some(p) => (p as isize + delta).rem_euclid(self.visible.len() as isize) as usize,
            None => 0,
        };
        let target = self.visible[next];
        if target == self.selected {
            return false;
        }
        self.selected = target;
        true
    }

    /// 当前分组在 `library.entries` 里的成员下标 —— `BrushList.brushes`
    /// （= `bm.get_group_brushes(group_name)`，`gui/brushselectionwindow.py:95`）。
    fn group_members(&self) -> Vec<usize> {
        self.library.group_members(&self.group)
    }

    /// 组内**第一支**笔刷的下标 —— `tool_widget_get_icon_pixbuf` 取的 `brushes[0]`。
    fn first_in_group(&self) -> Option<usize> {
        self.group_members().first().copied()
    }

    /// 面板标题 —— `tool_widget_title`（`gui/brushselectionwindow.py:384`）。
    ///
    /// `translate_group_name(self._group)`；本项目用 `group_display`
    /// （`order.conf` 的 `Group:` 名）过一次同一张表。
    fn group_title(&self) -> String {
        let display = self
            .library
            .groups()
            .into_iter()
            .find(|g| g.id == self.group)
            .map(|g| g.display)
            .unwrap_or_else(|| self.group.clone());
        translate_group_name(&display).to_string()
    }

    /// 面板描述 —— `tool_widget_description`（`gui/brushselectionwindow.py:388`）。
    fn group_count(&self) -> usize {
        self.library.group_members(&self.group).len()
    }

    fn rebuild_visible(&mut self) {
        let needle = self.search.trim().to_lowercase();
        self.visible = if needle.is_empty() {
            // ---- BrushGroupTool 形态：内容 = 该分组的 `BrushList.brushes` ----
            self.group_members()
        } else {
            // ---- 跨分组搜索（本项目新增；MyPaint 无搜索，只能翻分组）----
            self.library
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.display.to_lowercase().contains(&needle)
                        || e.name.to_lowercase().contains(&needle)
                        || e.key.to_lowercase().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect()
        };
        // 当前选中项不在面板内容里时，自动切到第一条可见项（避免"选中了但看不见"）
        if !self.visible.is_empty() && !self.visible.contains(&self.selected) {
            self.selected = self.visible[0];
        }
        self.rebuild_tooltips();
    }

    /// 预生成 tooltip 列表 —— `managedbrush_namefunc`（`gui/brushselectionwindow.py:46`）。
    ///
    /// tooltip 模板：`"{name}\n{description}"`（description 为空时只有 name）。
    /// 我们额外补一行路径与来源（MyPaint 的 tooltip 只有两行，这是新增信息）。
    fn rebuild_tooltips(&mut self) {
        self.tooltips.clear();
        for &i in &self.visible {
            let Some(e) = self.library.entries.get(i) else {
                continue;
            };
            let mut tip = e.mypaint_name();
            if !e.display.is_empty() {
                tip.push('\n');
                tip.push_str(&e.display);
            }
            tip.push_str(&format!(
                "\n{}\n{}",
                e.key,
                if e.is_embedded() {
                    "内嵌官方包 (CC0)"
                } else {
                    "用户目录"
                }
            ));
            self.tooltips.push(tip);
        }
    }

    /// 绘制面板。返回 `true` 表示**选中笔刷发生变化**（调用方需重载笔刷）。
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        // ---- 面板头（照搬工具面板页签：图标 + 标题 + 描述）----
        let switched = self.header(ui);
        // ---- 搜索框（本项目新增；为空时面板行为与 MyPaint 完全一致）----
        let searched = ui
            .add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("搜索笔刷…（跨分组；MyPaint 无此控件）")
                    .desired_width(f32::INFINITY),
            )
            .changed();
        if let Some(g) = switched {
            // 切分组时清掉搜索，否则网格仍是跨分组结果、看起来像「菜单没反应」
            self.group = g;
            self.search.clear();
            self.rebuild_visible();
        } else if searched {
            self.rebuild_visible();
        }

        ui.separator();
        let mut changed = self.grid(ui);
        let ctx = ui.ctx().clone();
        changed |= self.context_menu(&ctx);
        self.new_group_dialog(&ctx);
        self.delete_brush_dialog(&ctx);
        changed
    }

    /// 面板头 —— 照搬工具面板页签的构造（`gui/toolstack.py:464`
    /// `_Notebook._make_tab_label`）。
    ///
    /// * 图标：**组内第一支**笔刷的 `preview`（`tool_widget_get_icon_pixbuf`，`:401`）
    /// * 标题：`tool_widget_title` = `translate_group_name(group)`（`:384`）
    /// * 描述：`tool_widget_description` = `"{N} brush(es)"`（`:388`），进 tooltip
    /// * 右侧 ▾：`BrushGroupsMenu`（`:550`）
    ///
    /// 返回被选中的新分组（`None` = 没换分组）。
    fn header(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let title = self.group_title();
        let desc = brush_count_desc(self.group_count());
        let icon = self.first_in_group().map(|i| {
            header_icon_for(ui.ctx(), &self.library, &mut self.header_icons, i)
        });
        // 菜单候选先收集好：`menu_button` 的闭包里不能再借 `self`
        let menu = group_menu_items(&self.library);
        let current = self.group.clone();

        let mut picked: Option<String> = None;
        let mut rescan = false;
        let mut new_group_requested = false;
        let mut import_requested = false;
        let mut download_requested = false;
        let mut export_requested = false;
        ui.horizontal(|ui| {
            match &icon {
                Some(t) => {
                    ui.image((t.id(), egui::vec2(TAB_ICON_SIZE, TAB_ICON_SIZE)));
                }
                None => {
                    ui.allocate_space(egui::vec2(TAB_ICON_SIZE, TAB_ICON_SIZE));
                }
            }
            ui.label(egui::RichText::new(&title).strong())
                .on_hover_text(format!("{title}\n{desc}"));
            ui.label(egui::RichText::new(&desc).weak().small());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("▾", |ui| {
                    for (id, label) in &menu {
                        if ui.selectable_label(*id == current, label).clicked() {
                            picked = Some(id.clone());
                            ui.close();
                        }
                    }
                    ui.separator();
                    // 静态项（`BrushGroupsMenu.__init__`，`:559-569`）：
                    if ui.button("New Group…").clicked() {
                        new_group_requested = true;
                        ui.close();
                    }
                    if ui.button("Import Brushes…").clicked() {
                        import_requested = true;
                        ui.close();
                    }
                    if ui.button("Get More Brushes…").clicked() {
                        download_requested = true;
                        ui.close();
                    }
                    if ui.button("Export Group…").clicked() {
                        export_requested = true;
                        ui.close();
                    }
                    ui.separator();
                    // 本项目新增入口（MyPaint 靠重启重扫笔刷目录）
                    if ui.button("重新扫描笔刷目录（本项目新增）").clicked() {
                        rescan = true;
                        ui.close();
                    }
                });
            });
        });
        if rescan {
            self.rescan();
        }
        if new_group_requested {
            self.new_group = Some(String::new());
            self.new_group_focus = true;
        }
        self.import_requested |= import_requested;
        self.download_requested |= download_requested;
        self.export_group_requested |= export_requested;
        picked
    }

    /// 48px 流式图标网格 —— `PixbufList`。
    fn grid(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let count = self.visible.len();
        if count == 0 {
            ui.label(egui::RichText::new("没有匹配的笔刷").weak());
            return false;
        }
        let (tiles_w, tiles_h) = grid_geometry(ui.available_width(), count);
        let c = cell();
        let total_size = egui::vec2(tiles_w as f32 * c, tiles_h as f32 * c);

        let visuals = ui.visuals().clone();
        let bg = visuals.extreme_bg_color;
        let selected_fill = visuals.selection.bg_fill;
        let hover_fill = visuals.widgets.hovered.bg_fill;

        let mut clicked_idx: Option<usize> = None;
        let mut context: Option<(usize, egui::Pos2)> = None;
        let mut reorder: Option<(usize, usize)> = None;

        egui::ScrollArea::vertical()
            .id_salt("brush_grid_scroll")
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let (rect, response) =
                    ui.allocate_exact_size(total_size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(rect);
                // `PixbufList.draw_cb` 先铺主题背景
                painter.rect_filled(rect, egui::CornerRadius::ZERO, bg);

                // 只画滚动视口覆盖到的行（技术栈适配：避免一次性建 196 个纹理）
                let first_row = (viewport.min.y / c).floor().max(0.0) as usize;
                let last_row = (((viewport.max.y / c).ceil() as usize) + 1).min(tiles_h);

                let hover_pos = response.hover_pos().filter(|p| rect.contains(*p));
                let mut hovered: Option<usize> = None;
                // 拖拽期间 `hover_pos()` 可能为 None（指针被 drag 捕获），
                // 落点要用 `interact_pointer_pos()` 另算一份。
                let pointer_slot = response
                    .interact_pointer_pos()
                    .or(hover_pos)
                    .filter(|p| rect.contains(*p))
                    .map(|p| {
                        let local = p - rect.min;
                        index_at(local.x, local.y, tiles_w)
                    });

                for row in first_row..last_row {
                    for col in 0..tiles_w {
                        let slot = row * tiles_w + col;
                        if slot >= count {
                            break;
                        }
                        let idx = self.visible[slot];
                        let origin = rect.min + egui::vec2(col as f32 * c, row as f32 * c);
                        let cell_rect = egui::Rect::from_min_size(origin, egui::vec2(c, c));
                        let icon_rect = egui::Rect::from_min_size(
                            origin + egui::vec2(total_border(), total_border()),
                            egui::vec2(ICON_SIZE, ICON_SIZE),
                        );

                        let is_hovered = hover_pos.is_some_and(|p| cell_rect.contains(p));
                        if is_hovered {
                            hovered = Some(slot);
                        }

                        if idx == self.selected {
                            stroke_selection(&painter, cell_rect, selected_fill);
                        } else if is_hovered {
                            stroke_selection(&painter, cell_rect, hover_fill);
                        }
                        if self.drag_hover == Some(slot) {
                            stroke_selection(&painter, cell_rect, visuals.selection.stroke.color);
                        }

                        let tex = thumb_for(ui.ctx(), &self.library, &mut self.thumbnails, idx);
                        painter.image(
                            tex.id(),
                            icon_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }

                // ---- 拖放重排 / 右键菜单 ----
                if response.drag_started() {
                    self.drag_from = pointer_slot;
                }
                if response.dragged() && self.drag_from.is_some() {
                    self.drag_hover = pointer_slot;
                }
                if response.drag_stopped() {
                    if let (Some(a), Some(b)) = (self.drag_from, self.drag_hover) {
                        reorder = Some((a, b));
                    }
                    self.drag_from = None;
                    self.drag_hover = None;
                }
                if response.secondary_clicked() {
                    if let (Some(slot), Some(pos)) =
                        (hovered, response.interact_pointer_pos())
                    {
                        if let Some(&idx) = self.visible.get(slot) {
                            context = Some((idx, pos));
                        }
                    }
                }

                // ---- tooltip（`managedbrush_namefunc`）----
                let response = match hovered.and_then(|slot| self.tooltips.get(slot)) {
                    Some(tip) => response.on_hover_text(tip.clone()),
                    None => response,
                };

                // ---- 点击选中：照搬 `button_press_cb`（左键选中；右键菜单留待后续）----
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let local = pos - rect.min;
                        let slot = index_at(local.x, local.y, tiles_w);
                        clicked_idx = self.visible.get(slot).copied();
                    }
                }
            });

        if let Some((from, to)) = reorder {
            // 搜索状态下的 visible 不是组内顺序，禁止重排（MyPaint 无搜索）
            if self.search.trim().is_empty() {
                if let (Some(&key), Some(&target_key)) =
                    (self.visible.get(from), self.visible.get(to))
                {
                    if from != to {
                        if let Some(target) = self
                            .group_members()
                            .iter()
                            .position(|&i| i == target_key)
                        {
                            let key = self.library.entries[key].key.clone();
                            if self.library.move_in_group(&self.group, &key, target) {
                                self.save_order();
                                self.rebuild_visible();
                            }
                        }
                    }
                }
            }
        }
        if let Some(c) = context {
            self.context = Some(c);
        }

        if let Some(idx) = clicked_idx {
            if idx != self.selected {
                self.selected = idx;
                changed = true;
            }
        }
        changed
    }

    /// 把当前分组/顺序写回用户 `order.conf`（`save_brushorder`）。
    fn save_order(&self) {
        if self.library.order_conf_path.is_none() {
            return;
        }
        if let Err(e) = self.library.save_order_conf() {
            eprintln!("写回 order.conf 失败: {e}");
        }
    }

    /// 克隆一支笔刷（`BrushPopupMenu._clone_cb` + `ManagedBrush.clone`）。
    fn clone_brush_at(&mut self, idx: usize) -> bool {
        let Some(entry) = self.library.entries.get(idx).cloned() else {
            return false;
        };
        let Ok(mut brush) = entry.load_with_default(self.pigment_by_default) else {
            eprintln!("克隆失败：{} 解析不了", entry.key);
            return false;
        };
        let preview = crate::thumb::render(&brush);
        match self.library.clone_brush(
            &entry.key,
            &mut brush,
            Some((crate::thumb::PREVIEW_W as u32, crate::thumb::PREVIEW_H as u32, &preview)),
        ) {
            Ok(new_idx) => {
                self.save_order();
                self.thumbnails.clear();
                self.header_icons.clear();
                self.previews.clear();
                self.selected = new_idx;
                self.rebuild_visible();
                true
            }
            Err(e) => {
                eprintln!("克隆失败: {e}");
                false
            }
        }
    }

    /// 落盘后刷新缩略图缓存并写回 order.conf（保存/克隆共用）。
    pub(crate) fn refresh_after_save(&mut self, idx: usize) {
        self.thumbnails.remove(&idx);
        self.previews.remove(&idx);
        self.header_icons.clear();
        self.rebuild_visible();
        self.save_order();
    }

    /// 右键菜单（`BrushPopupMenu`，`brushselectionwindow.py:210-345`）。
    ///
    /// 已移植：Add/Remove Favorites、Remove from Group、Edit Brush Settings；
    /// Clone 依赖 `.myb` 写回，置灰并说明。
    fn context_menu(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        let Some((idx, pos)) = self.context else {
            return false;
        };
        let Some(entry) = self.library.entries.get(idx).cloned() else {
            self.context = None;
            return false;
        };
        let key = entry.key.clone();
        let is_fav = self.library.is_favorite(&key);
        let mut open = true;
        egui::Window::new("Brush")
            .fixed_pos(pos)
            .resizable(false)
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                if ui
                    .button(if is_fav {
                        "Remove from Favorites"
                    } else {
                        "Add to Favorites"
                    })
                    .clicked()
                {
                    self.library.toggle_favorite(&key);
                    self.save_order();
                    self.context = None;
                    ui.close();
                }
                if self.group != pigment_engine::brushlib::FAVORITES_BRUSH_GROUP
                    && ui.button("Remove from Group").clicked()
                {
                    self.library.remove_from_group(&self.group, &key);
                    self.save_order();
                    self.rebuild_visible();
                    self.context = None;
                    ui.close();
                }
                if ui.button("Clone…").clicked() {
                    if self.clone_brush_at(idx) {
                        changed = true;
                    }
                    self.context = None;
                    ui.close();
                }
                if ui.button("Delete Brush…").clicked() {
                    self.delete_confirm = Some(idx);
                    self.context = None;
                    ui.close();
                }
                if ui.button("Edit Brush Settings").clicked() {
                    if idx != self.selected {
                        self.selected = idx;
                        changed = true;
                    }
                    self.context = None;
                    ui.close();
                }
            });
        if !open {
            self.context = None;
        }
        changed
    }

    /// 「New Group…」对话框（`BrushGroupsMenu._new_brush_group_cb`，`:591-610`）。
    fn new_group_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut buf) = self.new_group.take() else {
            return;
        };
        let mut open = true;
        let mut create = false;
        egui::Window::new("Create Group")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let resp = ui.text_edit_singleline(&mut buf);
                if self.new_group_focus {
                    resp.request_focus();
                    self.new_group_focus = false;
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    create = true;
                }
                if ui.button("Create").clicked() {
                    create = true;
                }
            });
        if create {
            if let Some(id) = self.library.create_group(&buf) {
                self.save_order();
                self.group = id;
                self.search.clear();
                self.rebuild_visible();
            }
        } else if open {
            self.new_group = Some(buf);
        }
    }

    /// 删除确认（`brusheditor.py:1085-1115` 的确认 + `_delete_brush`）。
    fn delete_brush_dialog(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.delete_confirm else {
            return;
        };
        let Some(entry) = self.library.entries.get(idx).cloned() else {
            self.delete_confirm = None;
            return;
        };
        let mut open = true;
        let mut delete = false;
        let mut cancel = false;
        egui::Window::new("Delete Brush")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Really delete “{}”?", entry.mypaint_name()));
                if entry.is_embedded() {
                    ui.label(
                        egui::RichText::new(
                            "官方内嵌笔刷删不掉，会移到 Deleted 组（源码行为）。",
                        )
                        .weak()
                        .small(),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        delete = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if delete {
            let key = entry.key.clone();
            match self.library.delete_brush(&key) {
                Ok(removed) => {
                    self.save_order();
                    self.thumbnails.clear();
                    self.previews.clear();
                    self.header_icons.clear();
                    if removed {
                        self.rescan();
                    } else {
                        self.rebuild_visible();
                    }
                }
                Err(e) => eprintln!("删除笔刷失败: {e}"),
            }
            self.delete_confirm = None;
        } else if !open || cancel {
            self.delete_confirm = None;
        }
    }

    /// 重新扫描用户笔刷目录（本项目新增入口；MyPaint 靠重启）。
    fn rescan(&mut self) {
        let selected_key = self.selected_entry().map(|e| e.key.clone());
        let group = self.group.clone();
        self.library = BrushLibrary::load();
        self.thumbnails.clear();
        self.header_icons.clear();
        // 分组可能随用户目录一起消失 → 退回启动分组规则
        self.group = if self.library.groups.iter().any(|g| g.id == group) {
            group
        } else {
            self.library.startup_group().unwrap_or_default()
        };
        if let Some(k) = selected_key {
            if let Some(i) = self.library.find(&k) {
                self.selected = i;
            }
        }
        self.rebuild_visible();
    }
}

/// 分组菜单的候选项 `(分组 id, 标题)` —— `BrushGroupsMenu._update`
/// （`gui/brushselectionwindow.py:591`）。
///
/// 原式对 `sorted(bm.groups)` 的结果 `reversed` 后逐个 `prepend`，净效果就是
/// **按分组名正序**；MyPaint 的 key 本身就是展示名，所以这里也按标题排，
/// 标题并列时再按 id 做稳定的 tie-break。
fn group_menu_items(library: &BrushLibrary) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = library
        .groups()
        .into_iter()
        .map(|g| {
            let title = g.title().to_string();
            (g.id, title)
        })
        .collect();
    items.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    items
}

/// 取（必要时解码并缓存）某支笔刷的 48×48 网格图标。
///
/// 照搬 `BrushList` 的取图链：`managedbrush_pixbuffunc` → `ManagedBrush.preview`
/// → 优先随附 `_prev.png`、否则实时渲染（[`thumb::render`]）；
/// 再经 `helpers.pixbuf_thumbnail(pixbuf, 48, 48)` 等比缩小并居中到白底。
fn thumb_for(
    ctx: &egui::Context,
    library: &BrushLibrary,
    cache: &mut HashMap<usize, egui::TextureHandle>,
    idx: usize,
) -> egui::TextureHandle {
    if let Some(t) = cache.get(&idx) {
        return t.clone();
    }
    let side = ICON_SIZE as usize;
    let Some(entry) = library.entries.get(idx) else {
        return ctx.load_texture(
            "thumb-missing",
            icon_image(&white_icon(side, side), side, side),
            egui::TextureOptions::LINEAR,
        );
    };
    let rgba = match entry.preview_png() {
        // `ManagedBrush._load_preview()`：读随附的 128×128 预览图
        Some(bytes) => decode_png(&bytes),
        // 回退：`drawutils.render_brush_preview_pixbuf`
        None => entry.load().ok().map(|b| thumb::render(&b)),
    };
    let (key, rgba) = match rgba {
        Some(src) => (
            entry.key.clone(),
            thumbnail_on_white(&src, thumb::PREVIEW_W, thumb::PREVIEW_H, side, side),
        ),
        None => (format!("{}-broken", entry.key), white_icon(side, side)),
    };
    let tex = ctx.load_texture(
        format!("thumb:{key}"),
        icon_image(&rgba, side, side),
        egui::TextureOptions::LINEAR,
    );
    cache.insert(idx, tex.clone());
    tex
}

/// 面板头图标 —— `BrushGroupTool.tool_widget_get_icon_pixbuf()`
/// （`gui/brushselectionwindow.py:401`）。
///
/// 取**组内第一支**笔刷的 `preview`（不是当前选中那支）；尺寸不等于原图时
/// `scale_simple(size, size, BILINEAR)` —— 直接**拉伸**到正方，既不保持长宽比、
/// 也不合成白底（与网格走 `helpers.pixbuf_thumbnail` 不是同一条路）。
fn header_icon_for(
    ctx: &egui::Context,
    library: &BrushLibrary,
    cache: &mut HashMap<(usize, usize), egui::TextureHandle>,
    idx: usize,
) -> egui::TextureHandle {
    let size = TAB_ICON_SIZE as usize;
    if let Some(t) = cache.get(&(idx, size)) {
        return t.clone();
    }
    let rgba = raw_icon(library, idx, size, size);
    let tex = ctx.load_texture(
        format!("header-icon:{idx}:{size}"),
        icon_image(&rgba, size, size),
        egui::TextureOptions::LINEAR,
    );
    cache.insert((idx, size), tex.clone());
    tex
}

/// 笔刷原始 `preview` 缩放到 `w×h`（**不做**白底合成）。
fn raw_icon(library: &BrushLibrary, idx: usize, w: usize, h: usize) -> Vec<u8> {
    let Some(entry) = library.entries.get(idx) else {
        return white_icon(w, h);
    };
    let src = match entry.preview_png() {
        Some(bytes) => decode_png(&bytes),
        None => entry.load().ok().map(|b| thumb::render(&b)),
    };
    match src {
        Some(src) => scale_to(&src, thumb::PREVIEW_W, thumb::PREVIEW_H, w, h),
        None => white_icon(w, h),
    }
}

/// `GdkPixbuf.Pixbuf.scale_simple(w, h, BILINEAR)`：**拉伸**到目标尺寸。
fn scale_to(src: &[u8], sw: usize, sh: usize, w: usize, h: usize) -> Vec<u8> {
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 || (sw, sh) == (w, h) {
        return src.to_vec();
    }
    let buf = image::RgbaImage::from_raw(sw as u32, sh as u32, src.to_vec())
        .expect("源图尺寸与缓冲不符");
    image::imageops::resize(
        &buf,
        w as u32,
        h as u32,
        image::imageops::FilterType::Triangle,
    )
    .into_raw()
}

fn icon_image(rgba: &[u8], w: usize, h: usize) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([w, h], rgba)
}

/// 选中/悬停描边 —— `PixbufList.draw_cb`（`gui/pixbuflist.py:373`）。
///
/// 原算法：先把格子按 `spacing_outside` 内缩，再按
/// `border_visible_outside_cell` 外扩一格，然后描
/// `border_visible + border_visible_outside_cell` 层、逐层内缩 1px 的边框。
fn stroke_selection(painter: &egui::Painter, cell_rect: egui::Rect, color: egui::Color32) {
    let shrink = |r: egui::Rect, p: f32| -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(r.min.x + p, r.min.y + p),
            egui::pos2(r.max.x - p, r.max.y - p),
        )
    };
    let mut r = shrink(cell_rect, SPACING_OUTSIDE);
    for _ in 0..BORDER_VISIBLE_OUTSIDE_CELL as usize {
        r = shrink(r, -1.0);
    }
    let layers = (BORDER_VISIBLE + BORDER_VISIBLE_OUTSIDE_CELL) as usize;
    for _ in 0..layers {
        painter.rect_stroke(
            egui::Rect::from_min_max(r.min, egui::pos2(r.max.x - 1.0, r.max.y - 1.0)),
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Middle,
        );
        r = shrink(r, 1.0);
    }
}

/// 解码 `_prev.png` → 128×128 的 RGBA8，失败返回 `None`。
fn decode_png(bytes: &[u8]) -> Option<Vec<u8>> {
    // MyPaint 的 `_prev.png` 有 indexed / gray / RGB / RGBA 四种色型，
    // 统一归到 RGBA8（`get_preview()` 拿到的是带 alpha 的 pixbuf）。
    let img = image::load_from_memory(bytes).ok()?;
    Some(img.to_rgba8().into_raw())
}

/// `helpers.pixbuf_thumbnail(src, w, h)`（`lib/helpers.py:454`）：
/// `scale_proportionally`（只缩不放）后**居中合成到白底**。
fn thumbnail_on_white(src: &[u8], sw: usize, sh: usize, w: usize, h: usize) -> Vec<u8> {
    let mut dst = vec![255u8; w * h * 4];
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return dst;
    }
    let scale = (w as f32 / sw as f32).min(h as f32 / sh as f32);
    // `shrink_only=True`：scale >= 1 时原样使用
    let (nw, nh) = if scale >= 1.0 {
        (sw, sh)
    } else {
        (
            ((sw as f32 * scale) as usize).max(1),
            ((sh as f32 * scale) as usize).max(1),
        )
    };
    let scaled: Vec<u8> = if (nw, nh) == (sw, sh) {
        src[..sw * sh * 4].to_vec()
    } else {
        let buf = image::RgbaImage::from_raw(sw as u32, sh as u32, src.to_vec())
            .expect("源图尺寸与缓冲不符");
        image::imageops::resize(
            &buf,
            nw as u32,
            nh as u32,
            image::imageops::FilterType::Triangle,
        )
        .into_raw()
    };
    let ox = (w - nw) / 2;
    let oy = (h - nh) / 2;
    for y in 0..nh {
        for x in 0..nw {
            let si = (y * nw + x) * 4;
            let a = scaled[si + 3] as u32;
            let di = ((y + oy) * w + (x + ox)) * 4;
            // 源图带 alpha → 与白底合成（`src2.composite(dst, ...)`）
            for ch in 0..3 {
                dst[di + ch] = ((scaled[si + ch] as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
            dst[di + 3] = 255;
        }
    }
    dst
}

/// 全白图标（解析失败时的占位）。
fn white_icon(w: usize, h: usize) -> Vec<u8> {
    vec![255u8; w * h * 4]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `grid_geometry` 照搬 `PixbufList.update`：240px 宽 / 52px 格 → 4 列。
    #[test]
    fn grid_geometry_matches_pixbuflist() {
        assert_eq!(grid_geometry(240.0, 196), (4, 49));
        assert_eq!(grid_geometry(52.0, 10), (1, 10));
        // 宽度不足一格时至少 1 列
        assert_eq!(grid_geometry(10.0, 3), (1, 3));
        // ceil 的边界
        assert_eq!(grid_geometry(208.0, 8), (4, 2));
        assert_eq!(grid_geometry(208.0, 9), (4, 3));
    }

    /// `index_at` 照搬 `PixbufList.index`：超出右边界夹到最后一列。
    #[test]
    fn index_at_clamps_to_last_column() {
        let (tw, c) = (4usize, cell());
        assert_eq!(index_at(0.0, 0.0, tw), 0);
        assert_eq!(index_at(c * 3.5, 0.0, tw), 3);
        assert_eq!(index_at(5.0, c * 1.5, tw), 4);
        // 负坐标夹到 0
        assert_eq!(index_at(-30.0, -30.0, tw), 0);
        // 末尾空行**不**夹到最后一个有效下标：源码返回原始格子下标，
        // 由调用方 `visible.get(slot)` 拒绝。
        assert_eq!(index_at(c * 3.5, c * 9.5, tw), 39);
    }

    #[test]
    fn cell_is_52px_like_mypaint() {
        assert_eq!(cell(), 52.0);
        assert_eq!(ICON_SIZE, 48.0);
    }

    /// `pixbuf_thumbnail` 的等比缩小：128×128 → 48×48 铺满整格。
    #[test]
    fn thumbnail_fills_square_cell() {
        let mut src = vec![0u8; 128 * 128 * 4];
        for px in src.chunks_mut(4) {
            px[0] = 10;
            px[1] = 10;
            px[2] = 10;
            px[3] = 255;
        }
        let out = thumbnail_on_white(&src, 128, 128, 48, 48);
        assert_eq!(out.len(), 48 * 48 * 4);
        assert_eq!(&out[0..3], &[10, 10, 10]);
        assert_eq!(out[3], 255);
    }

    /// 半透明源图与白底合成（`src2.composite(dst, ...)`）。
    #[test]
    fn thumbnail_composites_alpha_on_white() {
        // 全透明 → 输出应全是白底
        let src = vec![0u8; 128 * 128 * 4];
        let out = thumbnail_on_white(&src, 128, 128, 48, 48);
        assert!(out.iter().all(|&b| b == 255), "全透明源图应为纯白");
    }

    /// 非正方形源图：等比缩放后居中，上下留白为白底。
    #[test]
    fn thumbnail_centers_non_square() {
        let mut src = vec![0u8; 128 * 64 * 4];
        for px in src.chunks_mut(4) {
            px[0] = 200;
            px[3] = 255;
        }
        let out = thumbnail_on_white(&src, 128, 64, 48, 48);
        assert_eq!(&out[0..3], &[255, 255, 255], "首行应为白底留白");
        let mid = (24 * 48 + 24) * 4;
        assert_eq!(out[mid], 200);
    }

    /// 「只缩不放」：源图比目标小则原样居中。
    #[test]
    fn thumbnail_never_upscales() {
        let mut src = vec![0u8; 16 * 16 * 4];
        for px in src.chunks_mut(4) {
            px[0] = 77;
            px[3] = 255;
        }
        let out = thumbnail_on_white(&src, 16, 16, 48, 48);
        assert_eq!(out.len(), 48 * 48 * 4);
        let mid = (24 * 48 + 24) * 4;
        assert_eq!(out[mid], 77);
        assert_eq!(&out[0..3], &[255, 255, 255]);
    }

    /// 官方包 196 支全部自带 `_prev.png`，且都能解码成 128×128 的**不透明**图。
    ///
    /// 不带 alpha 是这个资产包的事实（`PixbufList` 的白底合成路径对官方包是
    /// no-op）：官方预渲染图把性格底色烘进去了，橡皮类也不例外 ——
    /// 我们实测 30 张 RGB / 133 张调色板 / 33 张灰度，alpha 全为 255。
    #[test]
    fn every_embedded_brush_has_decodable_preview() {
        let lib = BrushLibrary::embedded();
        assert_eq!(lib.len(), 196);
        let mut bad = Vec::new();
        for e in &lib.entries {
            match e.preview_png().and_then(|b| decode_png(&b)) {
                Some(px) => {
                    assert_eq!(px.len(), 128 * 128 * 4, "{} 尺寸不对", e.key);
                    assert!(
                        px.chunks(4).all(|p| p[3] == 255),
                        "{} 的 _prev.png 不应含透明像素",
                        e.key
                    );
                }
                None => bad.push(e.key.clone()),
            }
        }
        assert!(bad.is_empty(), "缺少/无法解码预览图: {bad:?}");
    }

    /// 每个条目都有 tooltip，且首行是笔刷名（`managedbrush_namefunc` 的模板）。
    #[test]
    fn tooltips_follow_mypaint_template() {
        let p = BrushPanel::new();
        assert_eq!(p.tooltips.len(), p.visible.len());
        for (slot, &idx) in p.visible.iter().enumerate() {
            let e = &p.library.entries[idx];
            let first_line = p.tooltips[slot].lines().next().unwrap_or_default();
            assert_eq!(first_line, e.mypaint_name());
        }
    }

    /// 面板描述文本 —— `ngettext("{count} brush", "{count} brushes", n)`。
    #[test]
    fn brush_count_desc_matches_ngettext() {
        assert_eq!(brush_count_desc(0), "0 brushes");
        assert_eq!(brush_count_desc(1), "1 brush");
        assert_eq!(brush_count_desc(2), "2 brushes");
        assert_eq!(brush_count_desc(36), "36 brushes");
    }

    /// 面板尺寸常量 —— `TOOL_WIDGET_MIN_WIDTH` / `TAB_ICON_SIZE`。
    #[test]
    fn tool_widget_metrics_match_mypaint() {
        // gui/toolstack.py:40
        assert_eq!(TOOL_WIDGET_MIN_WIDTH, 220.0);
        // gui/toolstack.py:203 `TAB_ICON_SIZE = Gtk.IconSize.MENU` = 16×16
        assert_eq!(TAB_ICON_SIZE, 16.0);
        // 220px 至少放得下 4 列 52px 格（BrushList.NATURAL_WIDTH_NICONS = 4）
        assert!(TOOL_WIDGET_MIN_WIDTH >= cell() * 4.0);
    }

    /// 笔刷历史：按 `parent_brush_name` 去重、追加到尾部、裁到 5
    /// （`brushmanager.py:1064-1100`）。
    #[test]
    fn brush_history_dedupes_by_name_and_caps_at_five() {
        let mut p = BrushPanel::new();
        p.history.clear();
        assert_eq!(BRUSH_HISTORY_SIZE, 5);
        for i in 0..7 {
            let name = format!("brush{}", i % 5);
            p.push_history(pigment_engine::brushlib::default_brush(), name, i % 5);
        }
        let names: Vec<&str> = p.history.iter().map(|e| e.name.as_str()).collect();
        // 0..6 依次推入，0/1 被第二次推入时删掉 → [2,3,4,0,1]
        assert_eq!(names, vec!["brush2", "brush3", "brush4", "brush0", "brush1"]);
        assert!(p.history_entry(0).is_some());
        assert!(p.history_entry(5).is_none());
    }

    /// 面板开在启动分组上 —— `_init_default_brushkeys_and_history` 挑分组的规则。
    #[test]
    fn panel_opens_on_the_startup_group() {
        let p = BrushPanel::new();
        assert_eq!(Some(p.group.clone()), p.library.startup_group());
        assert_eq!(p.group, "deevad", "官方包最大分组");
        assert!(!p.visible.is_empty());
    }

    /// 面板内容 = **该分组**的成员列表（`BrushList.brushes`），顺序取库顺序。
    #[test]
    fn panel_content_is_the_group_list() {
        let p = BrushPanel::new();
        let expect: Vec<usize> = p
            .library
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.group == p.group)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(p.visible, expect);
        assert_eq!(p.group_count(), expect.len());
        // 全库 196 支按分组切开，各组之和等于全库
        let total: usize = p.library.groups().iter().map(|g| g.count).sum();
        assert_eq!(total, p.library.len());
    }

    /// 面板标题用 `order.conf` 的 `Group:` 名（经 `translate_group_name` 兜底）。
    #[test]
    fn panel_title_uses_group_display_name() {
        let p = BrushPanel::new();
        assert_eq!(p.group_title(), "Deevad", "目录名 deevad → 展示名 Deevad");
    }

    /// 搜索框为空时面板 == 分组列表；非空时跨分组（本项目新增入口）。
    #[test]
    fn empty_search_equals_group_and_search_spans_groups() {
        let mut p = BrushPanel::new();
        let in_group = p.visible.clone();
        assert!(p
            .visible
            .iter()
            .all(|&i| p.library.entries[i].group == p.group));

        p.search = "watercolor".to_string();
        p.rebuild_visible();
        assert!(!p.visible.is_empty(), "官方包里应有 watercolor 系笔刷");
        let groups: std::collections::HashSet<&str> = p
            .visible
            .iter()
            .map(|&i| p.library.entries[i].group.as_str())
            .collect();
        assert!(groups.len() > 1, "跨分组搜索应命中多个分组: {groups:?}");

        p.search.clear();
        p.rebuild_visible();
        assert_eq!(p.visible, in_group, "清空搜索应回到分组列表");
    }

    /// `,` / `.` 在当前分组内环行，不会跳到别的分组去。
    #[test]
    fn step_stays_inside_the_group() {
        let mut p = BrushPanel::new();
        let n = p.visible.len();
        let group = p.group.clone();
        let start = p.visible.iter().position(|&i| i == p.selected).unwrap();
        for k in 1..=(n * 2 + 1) {
            p.step(1);
            assert_eq!(p.selected, p.visible[(start + k) % n], "第 {k} 步位置不对");
            assert_eq!(p.library.entries[p.selected].group, group);
        }
        // 反向也应该环行回来
        p.step(-1);
        assert_eq!(p.selected, p.visible[(start + n * 2) % n]);
    }

    /// 面板头图标取**组内第一支**笔刷，不是当前选中那支。
    #[test]
    fn header_icon_comes_from_first_brush_of_the_group() {
        let p = BrushPanel::new();
        let first = p.first_in_group().expect("分组非空");
        let expect = p
            .library
            .entries
            .iter()
            .position(|e| e.group == p.group)
            .unwrap();
        assert_eq!(first, expect);
        assert_eq!(
            p.library.entries[first].key,
            p.library.entries[p.visible[0]].key
        );
        // 选中项换成别的笔刷也不影响面板头（`tool_widget_get_icon_pixbuf` 只看组内第一支）
        let mut q = BrushPanel::new();
        q.selected = q.visible[3];
        assert_eq!(q.first_in_group(), Some(first));
    }

    /// `scale_simple` 的拉伸语义：非方源图也被**拉成正方**（不保持长宽比）。
    #[test]
    fn header_icon_scales_by_stretching() {
        // 128×64 → 16×16：GdkPixbuf 的 scale_simple 不做 letterbox
        let src: Vec<u8> = (0..128 * 64 * 4)
            .map(|i| if i % 4 == 3 { 255 } else { 40 })
            .collect();
        let out = scale_to(&src, 128, 64, 16, 16);
        assert_eq!(out.len(), 16 * 16 * 4);
        assert_eq!(&out[0..3], &[40, 40, 40], "整幅都被拉满，没有白边");
        assert_eq!(out[3], 255);
        // 尺寸相同则原样返回（`tool_widget_get_icon_pixbuf` 的 copy() 分支）
        assert_eq!(scale_to(&src, 128, 64, 128, 64), src);
    }

    /// 面板头图标与网格图标是**两条路**：前者直接拉伸、不做白底合成。
    ///
    /// 官方 196 张 `_prev.png` 全是 128×128 且**都不带 alpha 通道**
    /// （30 张 RGB / 133 张调色板 / 33 张灰度），所以这条差异在官方包上看不出来；
    /// 它只在自制笔刷走 [`thumb::render`]（橡皮会擦出 alpha）时才显现。
    #[test]
    fn header_icon_keeps_alpha_where_thumbnail_paints_white() {
        // 4×4 源图：四周全透明，中心 2×2 涂红
        let mut src = vec![0u8; 4 * 4 * 4];
        for y in 1..3 {
            for x in 1..3 {
                let i = (y * 4 + x) * 4;
                src[i] = 255;
                src[i + 3] = 255;
            }
        }
        let head = scale_to(&src, 4, 4, 16, 16);
        assert_eq!(head.len(), 16 * 16 * 4);
        assert!(
            head.chunks(4).any(|px| px[3] < 255),
            "面板头不做白底合成，应保留源图 alpha"
        );
        let grid = thumbnail_on_white(&src, 4, 4, 48, 48);
        assert!(
            grid.chunks(4).all(|px| px[3] == 255),
            "网格图经 `pixbuf_thumbnail` 一律合成到白底"
        );
        assert_eq!(&grid[0..3], &[255, 255, 255], "透明处应被填白");
    }

    /// 分组菜单：候选 = 全部分组，按标题正序。
    #[test]
    fn group_menu_is_sorted_by_title() {
        let lib = BrushLibrary::embedded();
        let items = group_menu_items(&lib);
        assert_eq!(items.len(), lib.groups().len());
        let titles: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
        let mut sorted = titles.clone();
        sorted.sort_unstable();
        assert_eq!(titles, sorted, "菜单应按展示名正序");
        // 官方包的分组都在（`Group:` 名，不是目录名）；Favorites 是空组。
        for want in [
            "Dieterle",
            "Classic",
            "Deevad",
            "Favorites",
            "Ramon",
            "Tanda",
            "Kaerhon",
        ] {
            assert!(titles.contains(&want), "菜单缺少分组 {want}");
        }
        // ids 仍是目录名（用于过滤条目）；只有 Favorites 允许为空
        for (id, _) in &items {
            if id.as_str() == pigment_engine::brushlib::FAVORITES_BRUSH_GROUP {
                assert!(lib.group_members(id).is_empty());
            } else {
                assert!(!lib.group_members(id).is_empty(), "{id} 不应为空组");
            }
        }
    }

    /// 笔刷包安装后能被 `scan_dir` 扫进库并挂到新分组
    /// （`import_brushpack` 的 app 侧链路）。
    #[test]
    fn installed_brushpack_is_scanned_into_the_library() {
        use std::collections::HashSet;
        let dir = std::env::temp_dir().join(format!(
            "pigment-app-brushpack-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("user")).unwrap();
        let zip = dir.join("pack.zip");
        let mut z = pigment_engine::zip::ZipWriter::create(&zip).unwrap();
        z.add_stored("order.conf", b"Group: TestPack\nTestPack/pencil\n")
            .unwrap();
        z.add_stored("TestPack/pencil.myb", br#"{"version":3,"settings":{}}"#)
            .unwrap();
        z.add_stored("TestPack/pencil_prev.png", b"png").unwrap();
        z.finish().unwrap();

        let pack = pigment_engine::brushpack::read_brushpack(&zip).unwrap();
        let user = dir.join("user");
        pigment_engine::brushpack::install_brushpack(
            &pack,
            &user,
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        let mut lib = pigment_engine::brushlib::BrushLibrary::embedded();
        lib.scan_dir(&user);
        assert!(lib.find("TestPack/pencil").is_some());
        assert!(lib.groups.iter().any(|g| g.id == "TestPack"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
