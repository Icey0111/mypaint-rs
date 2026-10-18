//! 工具条 —— `gui/toolbar.xml`（框架）+ 8 个可开关片段 + `toolbar2`（S4）。
//!
//! 源码结构（`gui/toolbar.py:45-126` 的 `ToolbarManager`）：
//!
//! - **toolbar1** 由框架声明（占位符 + 内联条目），8 个片段文件
//!   （`toolbar-file.xml` 等）各含一个同名占位符，按偏好
//!   `ui.toolbar_items` 在运行期**合并**进框架（GTK UI manager 的 merge
//!   行为：片段条目追加在占位符内联条目之后）。
//! - **toolbar1 右键**弹 `toolbar1-settings-menu`（框架 XML 里的 popup），
//!   8 个开关动作是 `toolbar.py:87-99` 运行期注册的 `GtkToggleAction`
//!   （不在 `resources.xml`），label 来自 `toolbar.py:26-39` 的
//!   `MERGEABLE_XML`。
//! - **toolbar2** 固定：`ColorAdjusters` `BrushGroups` │ `Toggle×5` │
//!   `Fullscreen`。
//! - 两条工具条都是 `Gtk.ToolbarStyle.ICONS`（纯图标按钮），图标尺寸取
//!   偏好 `ui.toolbar_icon_size`（默认 "large" = LARGE_TOOLBAR，24×24，
//!   `gui/widgets.py:164-172`）。
//! - toolbar1 的 `<separator/>` 全部 `set_draw(False)`
//!   （`toolbar.py:68-70`）—— 组间只有留白，没有可见竖线。
//!
//! **技术栈适配**：
//! - 图标：MyPaint 的 hicolor scalable symbolic SVG（随项目内嵌于
//!   `assets/icons/`，GPL 同源）由 `build.rs` 用 resvg 栅格化成 24×24 PNG；
//!   GTK 的 symbolic 着色用"运行期按前景色重写 RGB、保留 alpha"复现。
//! - `set_show_arrow(True)`（toolbar1 溢出折叠）没有对应物，条目超出宽度
//!   时 egui 按序截断 —— 桌面窗口宽度下不触发，差异记账不阻塞。

use eframe::egui;

/// 工具条节点（生成物用；与 `toolbar*.xml` 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TbNode {
    /// `<toolitem action="…"/>`
    Item(&'static str),
    /// `<separator/>`（源码里 `set_draw(False)`：不可见，只留白）
    Separator,
    /// `<placeholder name="…">`，`items` 为框架内联条目
    Placeholder {
        name: &'static str,
        items: &'static [TbNode],
    },
}

pub(crate) mod generated {
    //! 构建期生成物（`build.rs` → `$OUT_DIR/toolbars_generated.rs`）。
    use super::TbNode;
    include!(concat!(env!("OUT_DIR"), "/toolbars_generated.rs"));
}

pub use generated::{FRAGMENT_ITEMS, TOOLBAR1_FRAMEWORK, TOOLBAR2};

pub(crate) mod icons_generated {
    //! 构建期栅格化的图标 PNG（`build.rs` → `$OUT_DIR/icons/*.png`）。
    include!(concat!(env!("OUT_DIR"), "/icons_generated.rs"));
}
use icons_generated::ICONS;

/// 片段开关（`toolbar.py:26-39` 的 `MERGEABLE_XML`）—— 动作名与显示名。
///
/// 这些动作是运行期注册的 toggle，不在动作表里；序号与
/// [`FRAGMENT_ITEMS`] 的顺序一致（框架 XML 的 settings 菜单按同一顺序引用）。
pub struct Fragment {
    /// 片段开关动作名（`toolbar1_file` 等）。UI 只用 label；这个名字同时是
    /// 偏好 `ui.toolbar_items` 的键（`gui/userconfig.py:76-85`），
    /// [`fragments_from_prefs`] 用它读回开关状态。
    pub action: &'static str,
    pub label: &'static str,
}

pub static FRAGMENTS: &[Fragment] = &[
    Fragment { action: "toolbar1_file", label: "File handling" },
    Fragment { action: "toolbar1_scrap", label: "Scraps switcher" },
    Fragment { action: "toolbar1_edit", label: "Undo and Redo" },
    Fragment { action: "toolbar1_blendmodes", label: "Blend Modes" },
    Fragment { action: "toolbar1_linemodes", label: "Line Modes" },
    Fragment { action: "toolbar1_view_modes", label: "View (Main)" },
    Fragment {
        action: "toolbar1_view_manips",
        label: "View (Alternative/Secondary)",
    },
    Fragment { action: "toolbar1_view_resets", label: "View (Resetting)" },
];

/// 出厂默认的片段开关（`gui/userconfig.py:76-85`）：
/// file / edit / linemodes / view_modes / view_resets 开，其余关。
pub const DEFAULT_FRAGMENTS: [bool; 8] = [true, false, true, false, true, true, false, true];

/// 从偏好读片段开关 —— 偏好 `ui.toolbar_items`（`gui/userconfig.py:76-85`）。
///
/// 源码把 8 个 toggle 动作的状态挂在 `ui.toolbar_items` 这个 dict 里
/// （`gui/toolbar.py` 的 `MERGEABLE_XML` + `toolbar1_file` 等 toggle 动作）；
/// 缺失/类型不对就用出厂值（保守，见 `pigment_engine::prefs` 的模块文档）。
pub fn fragments_from_prefs(prefs: &pigment_engine::prefs::Prefs) -> [bool; 8] {
    let mut out = DEFAULT_FRAGMENTS;
    if let Some(obj) = prefs.get_object("ui.toolbar_items") {
        for (i, frag) in FRAGMENTS.iter().enumerate() {
            if let Some(v) = obj.get(frag.action).and_then(|v| v.as_bool()) {
                out[i] = v;
            }
        }
    }
    out
}

/// 图标纹理缓存键：图标名（已含 -symbolic 后缀解析）。
pub type IconCache = std::collections::HashMap<&'static str, egui::TextureHandle>;

/// 按图标名取纹理；找不到时试 `-symbolic` 后缀
/// （`Fullscreen` 的 icon-name 是 `mypaint-fullscreen`，文件带后缀）。
pub fn icon_texture(
    ctx: &egui::Context,
    cache: &mut IconCache,
    name: &str,
) -> Option<egui::TextureHandle> {
    if let Some(t) = cache.get(name) {
        return Some(t.clone());
    }
    let (resolved, bytes) = lookup_png(name)
        .map(|b| (name, b))
        .or_else(|| {
            lookup_png(&format!("{name}-symbolic"))
                .map(|b| (Box::leak(format!("{name}-symbolic").into_boxed_str()) as &str, b))
        })?;
    let t = load_png(ctx, resolved, bytes);
    cache.insert(leak(resolved), t.clone());
    Some(t)
}

fn lookup_png(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// 水印用的**带 1px 黑描边**图标（`drawutils.py:291-367` 的
/// `load_symbolic_icon(..., outline=(0,0,0,1))`）。
///
/// GTK 的做法不是给图形描边，而是：图形缩小到 `size-2`（水印传 18 →
/// 图形 16），画布 `size`，把**黑色、alpha=1/3** 的同一图形在周围 8 个
/// 偏移各盖一次，再把正常颜色的图形放到 `(1,1)`。这样黑边只从图形外缘
/// 露出来，且与图形 alpha 相乘（半透明边缘的描边也半透明）。
///
/// 图标 PNG 是 24×24（`build.rs` 栅格化尺寸），这里重采样到 16×16；
/// 上游是按矢量在 16px 渲染，曲线细节可能差 1px，属技术栈适配。
pub fn outlined_icon_texture(
    ctx: &egui::Context,
    cache: &mut IconCache,
    name: &str,
) -> Option<egui::TextureHandle> {
    let key: &'static str = leak(&format!("outlined-icon-{name}"));
    if let Some(t) = cache.get(key) {
        return Some(t.clone());
    }
    let resolved = if lookup_png(name).is_some() {
        name.to_string()
    } else {
        format!("{name}-symbolic")
    };
    let bytes = lookup_png(&resolved)?;
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let base = image::imageops::resize(&img, 16, 16, image::imageops::FilterType::Lanczos3);
    let pixels = outlined_icon_pixels(&base);
    let t = ctx.load_texture(
        key,
        egui::ColorImage {
            size: [18, 18],
            source_size: egui::Vec2::new(18.0, 18.0),
            pixels,
        },
        egui::TextureOptions::LINEAR,
    );
    cache.insert(key, t.clone());
    Some(t)
}

/// `outlined_icon_texture` 的像素部分：16×16 图形 → 18×18 黑描边成品。
fn outlined_icon_pixels(base: &image::RgbaImage) -> Vec<egui::Color32> {
    const SIZE: usize = 18;
    const ICON: i32 = 16;
    // 直通 alpha 的 f32 RGBA 累加缓冲。
    let mut buf = vec![[0.0f32; 4]; SIZE * SIZE];

    fn over(dst: &mut [f32; 4], rgb: [f32; 3], sa: f32) {
        if sa <= 0.0 {
            return;
        }
        let da = dst[3];
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            return;
        }
        for c in 0..3 {
            dst[c] = (rgb[c] * sa + dst[c] * da * (1.0 - sa)) / out_a;
        }
        dst[3] = out_a;
    }

    const OFFSETS: [(i32, i32); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    // 第一遍：8 个偏移的黑描边，alpha = 图形 alpha / 3（`outline[3] /= 3.0`）。
    for y in 0..ICON {
        for x in 0..ICON {
            let a = base.get_pixel(x as u32, y as u32)[3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            for (dx, dy) in OFFSETS {
                let (px, py) = (x + 1 + dx, y + 1 + dy);
                if px >= 0 && py >= 0 && (px as usize) < SIZE && (py as usize) < SIZE {
                    let dst = &mut buf[py as usize * SIZE + px as usize];
                    over(dst, [0.0, 0.0, 0.0], a / 3.0);
                }
            }
        }
    }
    // 第二遍：正常颜色（symbolic 前景 215）的图形放在 (1,1)。
    for y in 0..ICON {
        for x in 0..ICON {
            let a = base.get_pixel(x as u32, y as u32)[3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            let dst = &mut buf[(y + 1) as usize * SIZE + (x + 1) as usize];
            over(dst, [215.0 / 255.0; 3], a);
        }
    }

    buf.into_iter()
        .map(|p| {
            egui::Color32::from_rgba_unmultiplied(
                (p[0] * 255.0).round() as u8,
                (p[1] * 255.0).round() as u8,
                (p[2] * 255.0).round() as u8,
                (p[3] * 255.0).round() as u8,
            )
        })
        .collect()
}

/// 图标文件是否存在（精确名；单测用）。
#[cfg(test)]
pub fn icon_png_exists(name: &str) -> bool {
    ICONS.iter().any(|(n, _)| *n == name)
}

/// 解码 PNG 并按 symbolic 语义着色：RGB 一律重写为前景色（亮灰 215，
/// 与 egui 暗色的可交互文本一致），保留原 alpha —— 复现 GTK 主题对
/// symbolic 图标的着色。
fn load_png(ctx: &egui::Context, name: &str, bytes: &[u8]) -> egui::TextureHandle {
    let img = image::load_from_memory(bytes).expect("图标 PNG 解码失败").to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<egui::Color32> = img
        .pixels()
        .map(|p| {
            let [_, _, _, a] = p.0;
            // 原图 fill=#bebebe；alpha（含抗锯齿边缘）就是形状
            egui::Color32::from_rgba_unmultiplied(215, 215, 215, a)
        })
        .collect();
    ctx.load_texture(
        leak(&format!("icon-{name}")),
        egui::ColorImage {
            size: [w, h],
            source_size: egui::Vec2::new(w as f32, h as f32),
            pixels,
        },
        egui::TextureOptions::LINEAR,
    )
}

/// 动态纹理名不是热路径（每图标只加载一次），一次性泄漏换 `'static` 键。
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// 把片段按开关状态合并进框架，得到 toolbar1 的**有效条目序列**。
///
/// 顺序语义照 GTK UI manager：占位符 = 内联条目 + （片段启用时的）片段条目。
pub fn expand(framework: &'static [TbNode], fragments: &[bool; 8]) -> Vec<&'static TbNode> {
    let mut out = Vec::new();
    for node in framework {
        match node {
            TbNode::Placeholder { name, items } => {
                for it in items.iter() {
                    out.push(it);
                }
                // 该占位符对应的片段（按 FRAGMENT_ITEMS 的顺序对位）
                if let Some(i) = FRAGMENT_ITEMS.iter().position(|f| f.0 == *name) {
                    if fragments[i] {
                        for id in FRAGMENT_ITEMS[i].1 {
                            // 每个片段条目包一层 Item 节点
                            out.push(emit_item(id));
                        }
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

// leak 一个 Item 节点（expand 的产出是 &'static TbNode）
fn emit_item(id: &'static str) -> &'static TbNode {
    Box::leak(Box::new(TbNode::Item(id)))
}

/// 画一条 toolitem：24×24 图标按钮（ICONS 风格）。
///
/// 入参全是**数据快照**（可用性/按下态/icon-name/tooltip 由调用方从
/// `PigmentApp` 取好，避免 `&mut self` 与图标缓存的双重借用）：
/// - 置灰条目图标 alpha 减半、不显示 tooltip（GTK insensitive 不弹提示）；
/// - toggle 语义的动作（`pressed`）用 `Button::selected` 的高亮。
///
/// 返回是否被点击；执行动作是调用方的事（`run_action`）。
pub struct ToolBtn {
    pub available: bool,
    pub pressed: bool,
    pub icon_name: Option<&'static str>,
    /// tooltip 文本（label，可含加速键）。`None` = 置灰，不弹。
    pub tip: Option<String>,
    /// 图标缺失时的文本回退（动作 short-label）。
    pub fallback_label: String,
}

/// 工具栏图标边长 —— 偏好 `ui.toolbar_icon_size`
/// （`gui/widgets.py:164-172`：small = SMALL_TOOLBAR 16px，large = LARGE_TOOLBAR 24px）。
pub fn icon_size(prefs: &pigment_engine::prefs::Prefs) -> f32 {
    if prefs.str_or("ui.toolbar_icon_size", "large") == "small" {
        16.0
    } else {
        24.0
    }
}

pub fn tool_button(
    ui: &mut egui::Ui,
    cache: &mut IconCache,
    b: ToolBtn,
    icon_size: f32,
) -> bool {
    let mut btn = match b.icon_name.and_then(|n| icon_texture(ui.ctx(), cache, n)) {
        Some(tex) => {
            let mut img =
                egui::Image::new(&tex).max_size(egui::vec2(icon_size, icon_size));
            if !b.available {
                img = img.tint(egui::Color32::from_rgba_unmultiplied(215, 215, 215, 90));
            }
            egui::Button::image(img).selected(b.pressed)
        }
        None => {
            // 图标缺失：退回 short-label 文本（GTK 画"缺图"占位，文字更能表达动作）
            let mut btn = egui::Button::new(b.fallback_label).selected(b.pressed);
            if !b.available {
                btn = btn.small();
            }
            btn
        }
    };
    btn = btn.min_size(egui::vec2(30.0, 30.0));
    let resp = ui.add(btn);
    if b.available {
        if let Some(tip) = b.tip {
            // on_hover_text 消费 Response，clone 保住点击判定
            let _ = resp.clone().on_hover_text(tip);
        }
    }
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_icon_size_follows_the_preference() {
        // widgets.py:164-172：large=24、small=16；缺键回落 large
        let path = std::env::temp_dir().join(format!(
            "pigment-toolbar-icon-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut prefs = pigment_engine::prefs::Prefs::load(&path);
        assert_eq!(icon_size(&prefs), 24.0);
        prefs.set_str("ui.toolbar_icon_size", "small");
        assert_eq!(icon_size(&prefs), 16.0);
        prefs.set_str("ui.toolbar_icon_size", "large");
        assert_eq!(icon_size(&prefs), 24.0);
        let _ = std::fs::remove_file(&path);
    }

    fn count_nodes<'a>(nodes: impl IntoIterator<Item = &'a TbNode>) -> (usize, usize) {
        // (toolitem 数, separator 数)，占位符递归
        let mut items = 0;
        let mut seps = 0;
        for n in nodes {
            match n {
                TbNode::Item(_) => items += 1,
                TbNode::Separator => seps += 1,
                TbNode::Placeholder { items: inner, .. } => {
                    let (i, s) = count_nodes(inner.iter());
                    items += i;
                    seps += s;
                }
            }
        }
        (items, seps)
    }

    #[test]
    fn framework_matches_toolbar_xml() {
        // toolbar.xml 的 toolbar1：3 个占位符为空 + blendmodes/linemodes 各 1 内联
        // + 5 个内联 toolitem + 3 个空占位符
        let (items, seps) = count_nodes(TOOLBAR1_FRAMEWORK.iter());
        assert_eq!(items, 7, "框架内联 toolitem（BlendModeEraser、FreehandMode、ColorPickMode、FloodFillMode、LayerMoveMode、FrameEditMode、SymmetryEditMode）");
        assert_eq!(seps, 5, "框架分隔线数（toolbar.xml 实测）");
        // toolbar2：ColorAdjusters BrushGroups | sep | Toggle×5 | sep | Fullscreen
        let (items2, seps2) = count_nodes(TOOLBAR2.iter());
        assert_eq!(items2, 8, "toolbar2 条目数");
        assert_eq!(seps2, 2, "toolbar2 分隔线数");
    }

    #[test]
    fn fragments_table_aligns_with_mergeable_xml() {
        // toolbar.py:26-39 的 8 个片段，顺序与 FRAGMENT_ITEMS 一一对应
        assert_eq!(FRAGMENTS.len(), 8);
        assert_eq!(FRAGMENT_ITEMS.len(), 8);
        let expected_actions = [
            "toolbar1_file",
            "toolbar1_scrap",
            "toolbar1_edit",
            "toolbar1_blendmodes",
            "toolbar1_linemodes",
            "toolbar1_view_modes",
            "toolbar1_view_manips",
            "toolbar1_view_resets",
        ];
        for (i, frag) in FRAGMENTS.iter().enumerate() {
            assert_eq!(frag.action, expected_actions[i]);
            assert!(!frag.label.is_empty());
        }
        // 片段内容与源文件一致（toolbar-file.xml: New Open Save 等）
        assert_eq!(FRAGMENT_ITEMS[0].1, &["New", "Open", "Save"]);
        assert_eq!(FRAGMENT_ITEMS[2].1, &["Undo", "Redo"]);
        assert_eq!(
            FRAGMENT_ITEMS[4].1,
            &["StraightMode", "SequenceMode", "EllipseMode", "InkingMode"]
        );
        // 片段引用的动作都在动作表里
        for (_, ids) in FRAGMENT_ITEMS {
            for id in *ids {
                assert!(
                    crate::shell::actions::by_id(id).is_some(),
                    "片段引用了未知动作：{id}"
                );
            }
        }
    }

    #[test]
    fn expand_respects_fragment_switches() {
        // 全关 = 只有框架内联条目
        let all_off = expand(TOOLBAR1_FRAMEWORK, &[false; 8]);
        let (items, _) = count_nodes(all_off.iter().copied());
        assert_eq!(items, 7);
        // 出厂默认：+file(3) +edit(2) +linemodes(4) +view_modes(4) +view_resets(2)
        let defaults = expand(TOOLBAR1_FRAMEWORK, &DEFAULT_FRAGMENTS);
        let (items, _) = count_nodes(defaults.iter().copied());
        assert_eq!(items, 7 + 3 + 2 + 4 + 4 + 2, "出厂默认的工具条条目数");
        // 顺序：file 片段条目追加在 file 占位符内联条目（空）之后，
        // 即 toolbar1 的最前面是 New Open Save
        let first = match defaults.first() {
            Some(TbNode::Item(id)) => *id,
            other => panic!("首条目应是 Item，实得 {other:?}"),
        };
        assert_eq!(first, "New");
    }

    #[test]
    fn every_toolbar_action_has_an_icon_file() {
        // toolbar1/toolbar2 + 片段引用的每个动作的 icon-name 都要能解析出
        // PNG（精确名或 -symbolic 后缀）——缺图的按钮会退化成文字，这里卡死
        fn check(id: &str) {
            let a = crate::shell::actions::by_id(id).unwrap_or_else(|| panic!("未知动作 {id}"));
            if let Some(icon) = a.icon_name {
                let hit = ICONS.iter().any(|(n, _)| *n == icon)
                    || ICONS.iter().any(|(n, _)| *n == format!("{icon}-symbolic"));
                assert!(hit, "{id} 的图标 {icon} 缺文件");
            }
        }
        fn walk<'a>(nodes: impl IntoIterator<Item = &'a TbNode>) {
            for n in nodes {
                match n {
                    TbNode::Item(id) => check(id),
                    TbNode::Separator => {}
                    TbNode::Placeholder { items, .. } => walk(items.iter()),
                }
            }
        }
        walk(TOOLBAR1_FRAMEWORK.iter());
        walk(TOOLBAR2.iter());
        for (_, ids) in FRAGMENT_ITEMS {
            for id in *ids {
                check(id);
            }
        }
    }

    #[test]
    fn default_fragments_match_userconfig_py() {
        // userconfig.py:76-85 的出厂默认
        assert_eq!(
            DEFAULT_FRAGMENTS,
            [true, false, true, false, true, true, false, true]
        );
    }

    #[test]
    fn fragments_from_prefs_reads_the_toolbar_items_dict() {
        // 偏好 `ui.toolbar_items`（userconfig.py:76-85）→ 8 个片段开关。
        // 先决：没有 settings.json 时就是出厂默认。
        let dir = std::env::temp_dir().join(format!(
            "pigment-toolbar-prefs-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let defaults = fragments_from_prefs(&pigment_engine::prefs::Prefs::load(&path));
        assert_eq!(defaults, DEFAULT_FRAGMENTS);

        // 用户改了三个开关（含一个非 bool，应回落默认）
        std::fs::write(
            &path,
            r#"{"ui.toolbar_items": {"toolbar1_file": false,
                                      "toolbar1_scrap": true,
                                      "toolbar1_edit": "yes"}}"#,
        )
        .unwrap();
        let read = fragments_from_prefs(&pigment_engine::prefs::Prefs::load(&path));
        assert!(!read[0], "toolbar1_file 被关掉");
        assert!(read[1], "toolbar1_scrap 被打开");
        assert!(read[2], "toolbar1_edit 类型不对 → 出厂值 true");
        assert_eq!(&read[3..], &DEFAULT_FRAGMENTS[3..], "其余保持出厂值");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 水印描边：16px 图形 → 18px，外缘 1px 黑（alpha 1/3），图形本身放在 (1,1)。
    #[test]
    fn outlined_icon_adds_a_one_pixel_black_rim() {
        let mut base = image::RgbaImage::new(16, 16);
        for y in 6..10 {
            for x in 6..10 {
                base.put_pixel(x, y, image::Rgba([200, 200, 200, 255]));
            }
        }
        let px = outlined_icon_pixels(&base);
        assert_eq!(px.len(), 18 * 18);

        let at = |x: usize, y: usize| px[y * 18 + x];
        // 图形中心：白（symbolic 前景 215）不透明。
        assert_eq!(at(8 + 1, 8 + 1), egui::Color32::from_rgba_unmultiplied(215, 215, 215, 255));
        // 图形左缘外一像素：黑描边，alpha 由 1~2 次 1/3 叠加（50% 边缘）。
        let rim = at(6, 10);
        assert_eq!((rim.r(), rim.g(), rim.b()), (0, 0, 0), "描边是黑");
        assert!(
            (85..=170).contains(&rim.a()),
            "单次描边 alpha=255/3、重叠可到 ~2/3，实得 {}",
            rim.a()
        );
        // 远离图形处保持透明。
        assert_eq!(at(0, 0).a(), 0);
        assert_eq!(at(17, 17).a(), 0);
    }
}
