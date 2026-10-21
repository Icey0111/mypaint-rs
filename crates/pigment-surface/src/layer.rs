//! 图层与图层栈 —— 移植 `_mypaint_ref/lib/layer/`（core/data/group/tree）中
//! 与「可嵌套图层树 + 显示合成」有关的部分。
//!
//! | 参考源 | 本文件 |
//! |---|---|
//! | `core.py:84-92` `LayerBase` 默认值 | [`Layer::new`] |
//! | `group.py:42-59` `LayerStack`（**index 0 = 最顶层**） | [`LayerStack`] |
//! | `modes.py:16` `PASS_THROUGH_MODE = -1` + `:24` `STACK_MODES` | [`LayerMode::PassThrough`] |
//! | `core.py:473-494` `mode` 设置器（直通强制 opacity=1、非法回落默认） | [`Layer::set_mode`] |
//! | `core.py:281-303` `opacity` 设置器（直通组不改） | [`Layer::set_opacity`] |
//! | `group.py:380-434` `get_render_ops`（PUSH/POP、直通不隔离） | `composite_layer` |
//! | `tree.py:722-738` `_validate_layer_bbox_arg` | [`Layer::data_bbox`] |
//! | `tree.py:796-811` `RootLayerStack.get_render_ops`（背景 BLIT 最先，子层自底向上） | [`LayerStack::composite_tile`] |
//! | `tree.py:55-58` 背景默认 (255,255,255) + `_background_visible` | [`LayerStack::background_*`] |
//! | `tree.py:1514-1580` `walk()`（组在前、子层在后、根不出现） | [`LayerStack::walk`] |
//! | `tree.py:1600-1719` `deepget/deepinsert/deeppop` | [`LayerStack::layer_at`] 等 |
//! | `tree.py:1754-1769` `deepindex`（按对象找路径） | [`LayerStack::path_of_id`] |
//! | `core.py:359-391` `branch_visible` / `:412` `branch_locked` | [`LayerStack::branch_visible`] |
//! | `data.py:1364-1371` `get_paintable` | [`LayerStack::paintable_surface_mut`] |
//! | `tiledsurface.py:440-457` `blit_tile_into`（透明源的跳过/清除规则） | `composite_layer` |
//!
//! 组语义（照 `group.py`）：
//! - 组是**层**（`LayerStack` 是 `LayerBase` 子类）：有自己的 mode/opacity/visible/name；
//! - 渲染时**隔离组** PUSH 一块全透明缓冲给子层、再 POP 用组自己的 mode/opacity 合到
//!   父背板；**直通组**（`PASS_THROUGH_MODE`）不隔离，子层直接作用于父背板，
//!   组自身的 mode/opacity 不参与；
//! - 直通组的 opacity 恒 1.0（设置模式时强制，之后不可改）；
//! - 组不能画（`get_paintable` = False），笔刷跳过非可画层。
//!
//! 记账：`VectorLayer`/`FallbackBitmapLayer` 不移植（MyPaint 2.0 已弃用建立）；
//! 背景层支持纯色档（图案档记账）；`RestackLayer` 的「拖进数据层 → 新建父组」
//! 分支未移植（面板只允许拖进组或插到行间）。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};

use crate::color;
use crate::color::TILE_RGBA8_BYTES;
use crate::combine::{tile_combine, CombineMode, DEFAULT_MODE};
use crate::mipmap::MAX_MIPMAP_LEVEL;
use crate::PigmentSurface;

/// 单 tile 的 u16 通道数。
const TILE_CHANNELS: usize = TILE_SIZE * TILE_SIZE * 4;

/// 合成目标：**哪一级**的**哪个 tile**。
///
/// `tx/ty` 是该级别下的 tile 索引（level L 一格覆盖 `2^L × 2^L` 个基 tile），
/// 与 `MyPaintSurface.blit_tile_into(dst, has_alpha, tx, ty, mipmap_level)`
/// （`tiledsurface.py:367-400`）的 `mipmap_level` 参数同义。
///
/// 捆成一个 `Copy` 结构体而不是给每个递归函数再加两个参数：递推链
/// （`composite_layers` → `composite_layer` → `composite_layers`）本来就有
/// 7 个参数了。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TileSel {
    level: u32,
    tx: i32,
    ty: i32,
}

impl TileSel {
    /// 构造（level 钳位到 `MAX_MIPMAP_LEVEL`，与 `tileddrawwidget.py:1301` 一致）。
    #[inline]
    fn new(level: u32, tx: i32, ty: i32) -> Self {
        Self {
            level: level.min(MAX_MIPMAP_LEVEL),
            tx,
            ty,
        }
    }

    /// level 0 版本（老路径，逐像素不变）。
    #[inline]
    fn base(tx: i32, ty: i32) -> Self {
        Self {
            level: 0,
            tx,
            ty,
        }
    }
}

/// 平铺背景图 —— `tiledsurface.Background`（`lib/tiledsurface.py:1178-1217`）。
///
/// 背景是一张**可重复平铺**的图（宽高建议 64 的倍数），按世界坐标取模采样。
/// 这里同时保存 8 位 RGBA（写 `.ora` / 导出用）与预乘 fix15（合成用），
/// 避免每个 tile 重复做 EOTF 转换。
///
/// 背景**自带一条降采样链**（`levels`）：源码 `Background` 不用通用的 tile
/// mipmap 链（`_create_mipmap_surfaces` 被覆写成返回 `None`，`:1219-1221`），
/// 而是在构造时用 `tile_downscale_rgba16` 把**整幅图**逐级减半、把下一级
/// 作为自己的 `mipmap` 子表面（`:1205-1217`）：
///
/// ```python
/// mipmap_obj = np.zeros((height, width, 4), dtype="uint16")
/// for ty in range(height // N * 2):
///     for tx in range(width // N * 2):
///         mypaintlib.tile_downscale_rgba16(src, mipmap_obj, tx * N // 2, ty * N // 2)
/// self.mipmap = Background(mipmap_obj, mipmap_level + 1)
/// ```
///
/// 所以远景的背景是**盒式平均**，不是点采样（点采样会让纸纹在远景出摩尔纹）。
///
/// 源码文档要求背景"不应带 alpha"；本实现按不透明处理（alpha 强制 255），
/// 与 `BackgroundLayer` 的 `CombineNormal` 不透明语义一致。
#[derive(Clone)]
pub struct BackgroundImage {
    width: u32,
    height: u32,
    /// 行主序 RGBA8（alpha 恒 255）。
    rgba8: Arc<Vec<u8>>,
    /// 行主序预乘 fix15（与 `rgba8_to_fix15(r,g,b,255)` 相同）。
    fix15: Arc<Vec<[u16; 4]>>,
    /// 平均色（RGB8），用于面板底色/纯色回退（`#EDEFE3` 就是默认纸纹的均值）。
    average: [u8; 3],
    /// 每级的盒式平均图（下标 = level，0 = 原图借用 `fix15`），懒建。
    ///
    /// 用 `OnceLock` 而不是 `RefCell`：`BackgroundImage` 会被装进
    /// `pigment-engine` 的 `static OnceLock<Option<BackgroundImage>>`（内置纸纹
    /// 只解码一次并全进程共享），那要求 `Sync`；`OnceLock` 既是 `Sync`，
    /// 初始化之后取值又是无锁的。
    ///
    /// 惰性构建是刻意的：源码在构造时把整条链接着建完（`:1205-1217`），
    /// 但大多数会话根本缩不到 level 3/4，没必要为不会用到的级别付内存。
    levels: [OnceLock<Arc<LevelImage>>; MAX_MIPMAP_LEVEL as usize + 1],
}

/// 背景在某一级的降采样图（宽高已按 `2^level` 缩小）。
struct LevelImage {
    w: i64,
    h: i64,
    px: Arc<Vec<[u16; 4]>>,
}

impl LevelImage {
    /// 2×2 盒式平均：每通道**各自 /4 后相加**（`lib/pixops.cpp:31-46`
    /// `tile_downscale_rgba16_c` 的算式，与 [`crate::mipmap`] 的 tile 链同源）。
    ///
    /// 尺寸取 `max(1, w/2)`；奇数边长时右/下边缘重复取样（源码的 numpy
    /// 链会直接丢掉余数行，这里保留一行而不是丢内容）。
    fn halve(src: &LevelImage) -> Self {
        let w = (src.w / 2).max(1);
        let h = (src.h / 2).max(1);
        let mut px = vec![[0u16; 4]; (w * h) as usize];
        let at = |x: i64, y: i64| -> [u16; 4] {
            let xx = x.min(src.w - 1).max(0);
            let yy = y.min(src.h - 1).max(0);
            src.px[(yy * src.w + xx) as usize]
        };
        for y in 0..h {
            for x in 0..w {
                let p00 = at(2 * x, 2 * y);
                let p01 = at(2 * x + 1, 2 * y);
                let p10 = at(2 * x, 2 * y + 1);
                let p11 = at(2 * x + 1, 2 * y + 1);
                let mut o = [0u16; 4];
                for c in 0..4 {
                    o[c] = (p00[c] as u32 / 4
                        + p01[c] as u32 / 4
                        + p10[c] as u32 / 4
                        + p11[c] as u32 / 4) as u16;
                }
                px[(y * w + x) as usize] = o;
            }
        }
        Self {
            w,
            h,
            px: Arc::new(px),
        }
    }

    /// 本级空间坐标 `(x, y)` 处的 fix15（按本级尺寸平铺回绕）。
    #[inline]
    fn pixel(&self, x: i64, y: i64) -> [u16; 4] {
        let sx = x.rem_euclid(self.w) as usize;
        let sy = y.rem_euclid(self.h) as usize;
        self.px[sy * self.w as usize + sx]
    }
}

impl std::fmt::Debug for BackgroundImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("average", &self.average)
            .finish()
    }
}

impl BackgroundImage {
    /// 从 RGBA8 像素构造。长度必须是 `w*h*4`。
    pub fn from_rgba8(width: u32, height: u32, rgba: &[u8]) -> Result<Self, String> {
        let n = (width as usize) * (height as usize);
        if width == 0 || height == 0 || rgba.len() != n * 4 {
            return Err(format!(
                "背景图尺寸不匹配：{width}×{height}，像素 {} 字节",
                rgba.len()
            ));
        }
        let mut out = Vec::with_capacity(n * 4);
        let mut fix = Vec::with_capacity(n);
        let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
        for px in rgba.chunks_exact(4) {
            let (r, g, b) = (px[0], px[1], px[2]);
            out.extend_from_slice(&[r, g, b, 255]);
            fix.push(crate::color::rgba8_to_fix15(r, g, b, 255));
            sr += r as u64;
            sg += g as u64;
            sb += b as u64;
        }
        let average = [
            (sr / n as u64) as u8,
            (sg / n as u64) as u8,
            (sb / n as u64) as u8,
        ];
        Ok(Self {
            width,
            height,
            rgba8: Arc::new(out),
            fix15: Arc::new(fix),
            average,
            levels: std::array::from_fn(|_| OnceLock::new()),
        })
    }

    /// 从 PNG 字节构造（`.ora` 背景 tile / 内置纸纹）。
    pub fn from_png_bytes(bytes: &[u8]) -> Result<Self, String> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .map_err(|e| format!("背景 PNG 解码失败：{e}"))?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Self::from_rgba8(w, h, rgba.as_raw())
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 平均色（RGB8）。
    #[inline]
    pub fn average_rgb(&self) -> [u8; 3] {
        self.average
    }

    /// 平铺采样：世界像素 `(x, y)` → fix15（负坐标按 Python 的 `%` 语义回绕）。
    #[inline]
    pub fn pixel_fix15(&self, x: i64, y: i64) -> [u16; 4] {
        let sx = x.rem_euclid(self.width as i64) as usize;
        let sy = y.rem_euclid(self.height as i64) as usize;
        self.fix15[sy * self.width as usize + sx]
    }

    /// 平铺采样：世界像素 → RGBA8（写 `.ora` 用）。
    #[inline]
    pub fn pixel_rgba8(&self, x: i64, y: i64) -> [u8; 4] {
        let sx = x.rem_euclid(self.width as i64) as usize;
        let sy = y.rem_euclid(self.height as i64) as usize;
        let i = (sy * self.width as usize + sx) * 4;
        [
            self.rgba8[i],
            self.rgba8[i + 1],
            self.rgba8[i + 2],
            self.rgba8[i + 3],
        ]
    }

    /// 原始 RGBA8（写 `background-tile.png` 用）。
    #[inline]
    pub fn rgba8(&self) -> &[u8] {
        &self.rgba8
    }

    /// 某一级的降采样图（惰性构建、按级缓存、跨帧复用）。
    ///
    /// 对应源码 `Background` 构造时逐级 `tile_downscale_rgba16` 的那条链
    /// （`tiledsurface.py:1205-1217`）。0 级零拷贝借用原图。
    fn level_image(&self, level: u32) -> Arc<LevelImage> {
        let level = level.min(MAX_MIPMAP_LEVEL) as usize;
        let cell = &self.levels[level];
        if let Some(img) = cell.get() {
            return Arc::clone(img);
        }
        // 递归的是**上一级**的 cell，不会重入同一个 OnceLock。
        let img = if level == 0 {
            Arc::new(LevelImage {
                w: self.width as i64,
                h: self.height as i64,
                px: Arc::clone(&self.fix15),
            })
        } else {
            let prev = self.level_image(level as u32 - 1);
            Arc::new(LevelImage::halve(&prev))
        };
        Arc::clone(cell.get_or_init(|| img))
    }
}

/// 图层 id 发生器。MyPaint 的命令持有**图层对象引用**（weakref），与栈序无关；
/// Rust 不能持引用，用单调 id 等价替代（Clone/快照恢复保留原 id）。
static NEXT_LAYER_ID: AtomicU64 = AtomicU64::new(1);
fn next_layer_id() -> u64 {
    NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed)
}

/// 分配一个新图层 id（`DuplicateLayer` 克隆层用；公开因为引擎层
/// 需要给 COW 克隆出的副本换 id）。
pub fn allocate_layer_id() -> u64 {
    next_layer_id()
}

// ---- 合成模式（`lib/modes.py`）----

/// 图层的合成模式取值。
///
/// `modes.py:16` 用哨兵整数 `PASS_THROUGH_MODE = -1` 表示组的「直通」。这里用
/// 枚举表达同一件事：塞进 [`CombineMode`] 会污染那份逐位移植的 22 值枚举
/// （`from_index(u8)`、`ora_name`、`tile_combine` 都只认 0..21）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum LayerMode {
    /// 22 种标准合成/混合模式（`modes.py:20` `STANDARD_MODES`）。
    Standard(CombineMode),
    /// 组专用：子层直接作用于组的背板（`modes.py:44`）。
    PassThrough,
}

impl LayerMode {
    /// `core.py:89` 默认模式 = `lib.modes.default_mode()`。
    pub const DEFAULT: LayerMode = LayerMode::Standard(DEFAULT_MODE);

    /// 是否是组的直通模式。
    #[inline]
    pub fn is_pass_through(self) -> bool {
        matches!(self, LayerMode::PassThrough)
    }

    /// 标准模式部分；直通模式返回 `None`。
    #[inline]
    pub fn combine(self) -> Option<CombineMode> {
        match self {
            LayerMode::Standard(m) => Some(m),
            LayerMode::PassThrough => None,
        }
    }

    /// UI 文案（`modes.py:MODE_STRINGS` 的 label 列）。
    pub fn label(self) -> &'static str {
        match self {
            LayerMode::Standard(m) => m.label(),
            LayerMode::PassThrough => "Pass-through",
        }
    }

    /// `.ora` 的 `composite-op` 名。直通模式在 OpenRaster 里没有对应条目
    /// （保存端改写成 `isolation="auto"` 并丢掉 opacity/composite-op，
    /// 见 `group.py:519-524`）。
    pub fn ora_name(self) -> Option<&'static str> {
        self.combine().map(|m| m.ora_name())
    }

    /// 该模式对本类图层是否合法（`core.py:70` / `group.py:74` 的
    /// `PERMITTED_MODES`）。
    #[inline]
    pub fn permitted(self, is_group: bool) -> bool {
        match self {
            LayerMode::Standard(_) => true,
            LayerMode::PassThrough => is_group,
        }
    }
}

// ---- 图层内容 ----

/// 图层内容：绘画层有自己的表面，组有自己的子栈。
///
/// 对应源码的两条类链：`data.SurfaceBackedLayer`（`data.py:60`）与
/// `group.LayerStack`（`group.py:42`）——后者是 `LayerBase` 的子类，即
/// **组也是层**，所以这里用「层 + 内容枚举」表达，而不是另立一种节点。
#[derive(Clone)]
pub enum LayerContent {
    /// 绘画层（`data.PaintingLayer`）。
    Painting(PigmentSurface),
    /// 组（`group.LayerStack`）：子栈的 `layers[0]` 同样是最顶层。
    Group(LayerStack),
}

/// 单个图层（`core.py` `LayerBase` 默认值 + 内容）。
#[derive(Clone)]
pub struct Layer {
    /// 稳定 id：命令按它寻址（等价 MyPaint 的 layer 对象引用）。
    pub id: u64,
    /// 内容：绘画层的表面，或组的子栈。
    pub content: LayerContent,
    /// `core.py:85` `self._name`（默认 `None` → 显示时用 UI 兜底名）。
    pub name: Option<String>,
    /// `core.py:85` 默认 1.0。
    pub opacity: f32,
    /// `core.py:86` 默认 true。
    pub visible: bool,
    /// `core.py:87` 默认 false。
    pub locked: bool,
    /// `core.py:88` 默认 `lib.modes.default_mode()` = SpectralWGM；
    /// 组还可以是 [`LayerMode::PassThrough`]。
    pub mode: LayerMode,
    /// 矢量层（`data.py:1153-1239` `VectorLayer`）的 SVG 权威数据。
    ///
    /// `Some` 时 `content` 仍是 `Painting`：表面只是 SVG 的位图预览
    /// （源码 `FileBackedLayer` 的 `_surface` 语义），但该层不可直接笔刷作画。
    pub vector: Option<crate::vector::VectorLayerData>,
}

impl Default for Layer {
    fn default() -> Self {
        Self::new()
    }
}

impl Layer {
    /// 新绘画层（`data.PaintingLayer` 默认值）。
    pub fn new() -> Self {
        Self {
            id: next_layer_id(),
            content: LayerContent::Painting(PigmentSurface::new()),
            name: None,
            opacity: 1.0,
            visible: true,
            locked: false,
            mode: LayerMode::DEFAULT,
            vector: None,
        }
    }

    /// 新组（`group.LayerStack` 默认值：空子层、默认模式 = 隔离的 SpectralWGM）。
    pub fn new_group() -> Self {
        Self {
            id: next_layer_id(),
            content: LayerContent::Group(LayerStack::empty()),
            name: None,
            opacity: 1.0,
            visible: true,
            locked: false,
            mode: LayerMode::DEFAULT,
            vector: None,
        }
    }

    /// 用现成表面造一个绘画层（测试/载入路径用）。
    pub fn with_surface(surface: PigmentSurface) -> Self {
        Self {
            content: LayerContent::Painting(surface),
            ..Self::new()
        }
    }

    /// 新建矢量层（`data.VectorLayer`）：SVG + 栅格化预览。
    ///
    /// 与源码 `FileBackedLayer._ensure_valid_working_file()` 同序：先有工作
    /// 文件（这里是 SVG 文本），再 `load_surface_from_pixbuf_file(x,y)` 得到
    /// 预览表面。SVG 无法解析时返回错误（不建层）。
    pub fn new_vector(svg: impl Into<std::sync::Arc<str>>, x: i32, y: i32) -> Result<Self, String> {
        let data = crate::vector::VectorLayerData::new(svg, x, y);
        let surface = crate::vector::rasterize_svg(&data.svg, data.x, data.y)?;
        let mut layer = Self::with_surface(surface);
        layer.vector = Some(data);
        Ok(layer)
    }

    /// 是否是矢量层（`isinstance(layer, data.VectorLayer)`）。
    #[inline]
    pub fn is_vector(&self) -> bool {
        self.vector.is_some()
    }

    /// 矢量层的 SVG 数据；绘画层/组返回 `None`。
    #[inline]
    pub fn vector_data(&self) -> Option<&crate::vector::VectorLayerData> {
        self.vector.as_ref()
    }

    /// 用新的 SVG 文本替换矢量层内容并重新栅格化。
    ///
    /// 对应 `FileBackedLayer.load_from_external_edit_tempfile()`
    /// （`data.py:831-845`）：重新加载预览表面、保留 `_x/_y`。
    pub fn set_vector_svg(&mut self, svg: impl Into<std::sync::Arc<str>>) -> Result<(), String> {
        let Some(data) = self.vector.as_mut() else {
            return Err("不是矢量层".to_string());
        };
        let svg = svg.into();
        let surface = crate::vector::rasterize_svg(&svg, data.x, data.y)?;
        data.svg = svg;
        self.content = LayerContent::Painting(surface);
        Ok(())
    }

    /// 是否是组（`group.LayerStack` 的实例判定）。
    #[inline]
    pub fn is_group(&self) -> bool {
        matches!(self.content, LayerContent::Group(_))
    }

    /// 组的直接子层数（绘画层恒 0）。
    #[inline]
    pub fn child_count(&self) -> usize {
        self.stack().map(|s| s.len()).unwrap_or(0)
    }

    /// 绘画层的表面；组返回 `None`。
    #[inline]
    pub fn surface(&self) -> Option<&PigmentSurface> {
        match &self.content {
            LayerContent::Painting(s) => Some(s),
            LayerContent::Group(_) => None,
        }
    }

    /// 绘画层的可变表面；组返回 `None`。
    #[inline]
    pub fn surface_mut(&mut self) -> Option<&mut PigmentSurface> {
        match &mut self.content {
            LayerContent::Painting(s) => Some(s),
            LayerContent::Group(_) => None,
        }
    }

    /// 组的子栈；绘画层返回 `None`。
    #[inline]
    pub fn stack(&self) -> Option<&LayerStack> {
        match &self.content {
            LayerContent::Group(st) => Some(st),
            LayerContent::Painting(_) => None,
        }
    }

    /// 组的可变子栈；绘画层返回 `None`。
    #[inline]
    pub fn stack_mut(&mut self) -> Option<&mut LayerStack> {
        match &mut self.content {
            LayerContent::Group(st) => Some(st),
            LayerContent::Painting(_) => None,
        }
    }

    /// 合成时使用的不透明度：不可见层为 0（`group.py:370-376`）。
    pub fn effective_opacity(&self) -> f32 {
        if self.visible {
            self.opacity
        } else {
            0.0
        }
    }

    /// 图层显示名（无名字时用 UI 兜底名，对应 `layerswindow` 的惯例）。
    pub fn display_name(&self, index: usize) -> String {
        self.name.clone().unwrap_or_else(|| {
            if self.is_vector() {
                // `data.py:1173-1177` `VectorLayer.DEFAULT_NAME = "Vectors"`
                "Vectors".to_string()
            } else if self.is_group() {
                "Group".to_string()
            } else {
                format!("Layer {}", index + 1)
            }
        })
    }

    /// 图层类型描述（`data.py` 的 `TYPE_DESCRIPTION`）：Layers 面板/属性用。
    pub fn type_description(&self) -> &'static str {
        if self.is_vector() {
            "Vector Layer"
        } else if self.is_group() {
            "Layer Group"
        } else {
            "Painting Layer"
        }
    }

    /// 本层自身是否可被笔刷绘制（`data.py:1364 get_paintable` 中与祖先无关的
    /// 那半；`branch_visible`/`branch_locked` 由 [`LayerStack::paintable_surface_mut`]
    /// 补齐）。
    pub fn paintable_self(&self) -> bool {
        // 矢量层不可直接作画（`FileBackedLayer` 只把表面当预览；
        // `data.py:597-609`）。
        !self.is_group() && !self.is_vector() && self.visible && !self.locked
    }

    /// `core.py:473-494` 的 `mode` 设置器语义：
    /// - 对普通层非法的（直通）→ 回落默认模式；
    /// - 设成直通 → **opacity 强制 1.0**。
    ///
    /// 返回 `(mode_changed, opacity_changed)`；调用方据此决定要不要重算/记撤销。
    pub fn set_mode(&mut self, mode: LayerMode) -> (bool, bool) {
        let mode = if mode.permitted(self.is_group()) {
            mode
        } else {
            LayerMode::DEFAULT
        };
        if mode == self.mode {
            return (false, false);
        }
        let mut opacity_changed = false;
        if mode.is_pass_through() && self.opacity != 1.0 {
            self.opacity = 1.0;
            opacity_changed = true;
        }
        self.mode = mode;
        (true, opacity_changed)
    }

    /// `core.py:281-303` 的 `opacity` 设置器语义：直通组的 opacity 不可改。
    ///
    /// 返回是否真的改了（值不变也算没改）。
    pub fn set_opacity(&mut self, opacity: f32) -> bool {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity == self.opacity || self.mode.is_pass_through() {
            return false;
        }
        self.opacity = opacity;
        true
    }

    /// 图层数据外接框（`lib/layer/data.py:314 get_bbox`）：
    /// 已分配 tile 的最小外接矩形，**tile 对齐**；空层/空组返回全 0。
    pub fn data_bbox(&self) -> (i32, i32, i32, i32) {
        match &self.content {
            LayerContent::Painting(surface) => surface_bbox(surface),
            LayerContent::Group(stack) => stack.data_bbox(),
        }
    }

    /// 本层是否为空：绘画层看 tile，组看是否有子层
    /// （`data.py:318` / `group.py:367`）。
    pub fn is_empty(&self) -> bool {
        match &self.content {
            LayerContent::Painting(surface) => surface.is_empty(),
            LayerContent::Group(stack) => stack.is_empty(),
        }
    }

    /// 清空本层内容，保留 id/name/opacity/visible/locked/mode
    /// （`data.py:296-299`、`group.py:198-204`）。
    pub fn clear_content(&mut self) {
        match &mut self.content {
            LayerContent::Painting(surface) => {
                surface.clear();
            }
            LayerContent::Group(stack) => {
                *stack = LayerStack::empty();
            }
        }
    }

    /// `core.py:656-674` 的 `has_interesting_name`：空白、默认名、默认名+编号
    /// （如 `Layer 3`）都不“有趣”。
    pub fn has_interesting_name(&self) -> bool {
        let Some(name) = self.name.as_deref() else {
            return false;
        };
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let default = if self.is_group() {
            "Group"
        } else if self.is_vector() {
            "Vectors"
        } else {
            "Layer"
        };
        if name == default {
            return false;
        }
        if let Some((base, number)) = name.rsplit_once(' ') {
            if base == default && number.parse::<u32>().is_ok() {
                return false;
            }
        }
        true
    }

    /// 本层（含子层）占用的 tile 坐标（`get_tile_coords`；合并/归一化用）。
    pub fn tile_coords(&self) -> Vec<(i32, i32)> {
        match &self.content {
            LayerContent::Painting(surface) => surface.tiles().map(|(c, _)| c).collect(),
            LayerContent::Group(stack) => {
                let mut out: Vec<(i32, i32)> = Vec::new();
                for (_, layer) in stack.walk() {
                    for c in layer.tile_coords() {
                        if !out.contains(&c) {
                            out.push(c);
                        }
                    }
                }
                out
            }
        }
    }

    /// 把本层**自己的 op**（含组的隔离/直通与子层）合成进 `dst` 的 `(tx, ty)`
    /// tile（先清零）。返回是否有内容。
    ///
    /// 等价 `render_layer_preview` 的逐 tile 版：`COMPOSITE(surface, mode,
    /// opacity)` 落透明底；组走 PUSH/POP（`group.py:424-432`）。合并下行
    /// （`tree.py:1920 layer_new_normalized`）与缩略图共用它。
    pub fn compose_tile_into(&self, tx: i32, ty: i32, dst: &mut [u16]) -> bool {
        self.compose_tile_into_at(0, tx, ty, dst)
    }

    /// 同 [`Self::compose_tile_into`]，但可按 mipmap 级别取样。
    ///
    /// `tx/ty` 是该级别下的 tile 索引；`level == 0` 时与上面逐字节一致。
    /// 组的隔离缓冲、mode/opacity 语义在各级别下完全相同——级别只影响
    /// **源像素从哪一层金字塔取**（`blit_tile_into` 的 `mipmap_level`）。
    pub fn compose_tile_into_at(&self, level: u32, tx: i32, ty: i32, dst: &mut [u16]) -> bool {
        debug_assert_eq!(dst.len(), TILE_CHANNELS);
        dst.fill(0);
        let mut pool: Vec<Vec<u16>> = Vec::new();
        composite_layer(
            self,
            TileSel::new(level, tx, ty),
            dst,
            true,
            false,
            0,
            &mut pool,
        )
    }

    /// **图层**缩略图（`lib/layer/core.py:922 render_thumbnail` →
    /// `tree.py:564 render_layer_preview(layer, bbox, alpha=True)`）。
    ///
    /// 渲染的是本层**自己的 op**：绘画层是 `COMPOSITE(surface, mode, opacity)`
    /// 落在全透明底上（`data.py:376-383`），组是「子层 PUSH/POP 后再用组自己的
    /// mode/opacity」（`group.py:424-432`）——所以模式与不透明度都会体现在缩略图里。
    /// solo 只把「不可见」强制成可见（`data.py:353-358` / `group.py:412-419`），
    /// **不**重置 mode/opacity。
    ///
    /// `rect` 传 level 0 的模型坐标；减半规则见 [`crate::thumb::render_halved`]。
    pub fn render_thumbnail_rgba8(
        &self,
        rect: (i32, i32, i32, i32),
        level: u32,
    ) -> (Vec<u8>, u32, u32) {
        let mut pool: Vec<Vec<u16>> = Vec::new();
        let mut scratch = vec![0u16; TILE_CHANNELS];
        let src = tile_source(|(tx, ty), buf| {
            // 本层自身的 visible 按 solo 规则忽略（`data.py:353-358`）
            scratch.fill(0);
            let content = composite_layer(
                self,
                TileSel::base(tx, ty),
                &mut scratch,
                true,
                false,
                0,
                &mut pool,
            );
            buf.copy_from_slice(&scratch);
            content
        });
        crate::thumb::render_halved(rect, level, crate::thumb::encode_alpha, src)
    }
}

/// 表面数据的 tile 对齐外接框（`data.py:314 get_bbox` 的图层面）。
fn surface_bbox(surface: &PigmentSurface) -> (i32, i32, i32, i32) {
    let mut coords = surface.tiles().map(|(c, _)| c);
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

/// 把**一个**图层合进 `dst`（`_process_ops_list` 的 COMPOSITE/PUSH/POP 语义）。
///
/// - `respect_visible = false` 时忽略本层自己的 `visible`（solo 预览用，
///   `data.py:353-358`）；子层始终按自身可见性；
/// - 绘画层：`tile_combine(mode, tile, dst, has_alpha, opacity)`，源 tile 缺席时
///   仍按**透明源**跑一次（DestIn/DestAtop 零 alpha 也生效，`tiledsurface.py:440`）；
/// - 隔离组：PUSH 全透明缓冲 → 子层 → POP（组自己的 mode/opacity）；
/// - 直通组：子层直接写 `dst`（组自身的 mode/opacity 不参与，`group.py:424`）。
///
/// 返回「本层在该 tile 是否可能有可见内容」（GPU 上传/剔除判据用）。
fn composite_layer(
    layer: &Layer,
    sel: TileSel,
    dst: &mut [u16],
    has_alpha: bool,
    respect_visible: bool,
    depth: usize,
    pool: &mut Vec<Vec<u16>>,
) -> bool {
    if respect_visible && !layer.visible {
        return false;
    }
    match &layer.content {
        LayerContent::Painting(surface) => {
            let Some(mode) = layer.mode.combine() else {
                return false; // 直通只对组合法；普通层不会走到这里
            };
            // 源像素：level 0 直读基表，其余级别走 mipmap 惰性重建
            // （`MyPaintSurface.blit_tile_into` 的 `mipmap_level` 分支，
            // `tiledsurface.py:367-400`）。
            match surface.mip_tile(sel.level, sel.tx, sel.ty) {
                Some(tile) => {
                    tile_combine(
                        mode,
                        tile.as_flattened().as_flattened(),
                        dst,
                        has_alpha,
                        layer.opacity,
                    );
                    true
                }
                None => {
                    // 透明源规则（`tiledsurface.py:443-452`）：先跳过零处理，
                    // 其余（DestIn/DestAtop）用全零源照常合成。
                    if !mode.zero_alpha_has_effect() {
                        if has_alpha && mode.zero_alpha_clears_backdrop() {
                            dst.fill(0);
                        }
                        return false;
                    }
                    let zero = empty_tile();
                    tile_combine(
                        mode,
                        zero.as_flattened().as_flattened(),
                        dst,
                        has_alpha,
                        layer.opacity,
                    );
                    false
                }
            }
        }
        LayerContent::Group(stack) => {
            if layer.mode.is_pass_through() {
                // 不隔离：子层直接作用于 dst
                return composite_layers(&stack.layers, sel, dst, has_alpha, depth, pool);
            }
            let mode = layer.mode.combine().expect("隔离组必有标准模式");
            // PUSH：子层画到全透明缓冲（dst_has_alpha = true）
            while pool.len() <= depth {
                pool.push(vec![0u16; TILE_CHANNELS]);
            }
            // 借出本深度的缓冲（递归期间该槽位留空，更深的组用自己的槽）
            let mut sub = std::mem::take(&mut pool[depth]);
            sub.fill(0);
            let sub_content =
                composite_layers(&stack.layers, sel, &mut sub, true, depth + 1, pool);
            // POP：用组自己的 mode/opacity 合到父背板
            tile_combine(mode, &sub, dst, has_alpha, layer.opacity);
            pool[depth] = sub;
            sub_content
        }
    }
}

/// 把一组**同层**图层自底向上合进 `dst`（`get_render_ops` 的自底向上次序）。
fn composite_layers(
    layers: &[Layer],
    sel: TileSel,
    dst: &mut [u16],
    has_alpha: bool,
    depth: usize,
    pool: &mut Vec<Vec<u16>>,
) -> bool {
    let mut content = false;
    for layer in layers.iter().rev() {
        content |= composite_layer(layer, sel, dst, has_alpha, true, depth, pool);
    }
    content
}

/// 预览态合成一棵子树（`group.py:393-398`、`data.py:348-352`）：
/// 忽略所有可见性，组一律直通，普通层用默认模式且不透明度 1。
fn composite_preview_layer(
    layer: &Layer,
    sel: TileSel,
    dst: &mut [u16],
    dst_has_alpha: bool,
) -> bool {
    match &layer.content {
        LayerContent::Painting(surface) => {
            let Some(tile) = surface.mip_tile(sel.level, sel.tx, sel.ty) else {
                return false;
            };
            tile_combine(
                DEFAULT_MODE,
                tile.as_flattened().as_flattened(),
                dst,
                dst_has_alpha,
                1.0,
            );
            true
        }
        LayerContent::Group(stack) => {
            let mut content = false;
            for child in stack.layers.iter().rev() {
                content |= composite_preview_layer(child, sel, dst, dst_has_alpha);
            }
            content
        }
    }
}

/// 惰性「逐 tile 取源像素」访问器：按需调 `compose` 生成整块 tile 并缓存 4 槽
/// （减半块边长 ≤ 2^4 = 16，最多跨 2×2 个 tile），块内行进方向连续、命中率高。
fn tile_source<'a, F>(mut compose: F) -> impl FnMut(i32, i32) -> [u16; 4] + 'a
where
    F: FnMut((i32, i32), &mut [u16]) -> bool + 'a,
{
    let ts = TILE_SIZE as i32;
    let mut slots: Vec<((i32, i32), bool, Vec<u16>)> = Vec::with_capacity(4);
    move |x: i32, y: i32| {
        let key = (x.div_euclid(ts), y.div_euclid(ts));
        let idx = match slots.iter().position(|(k, _, _)| *k == key) {
            Some(i) => i,
            None => {
                if slots.len() == 4 {
                    slots.remove(0);
                }
                let mut buf = vec![0u16; TILE_CHANNELS];
                let composed = compose(key, &mut buf);
                slots.push((key, composed, buf));
                slots.len() - 1
            }
        };
        let (_, composed, buf) = &slots[idx];
        if !composed {
            return [0, 0, 0, 0];
        }
        let ix = x.rem_euclid(ts) as usize;
        let iy = y.rem_euclid(ts) as usize;
        let o = (iy * TILE_SIZE + ix) * 4;
        buf[o..o + 4].try_into().unwrap()
    }
}

/// `pixops.cpp:589-654` 的 `tile_flat2rgba`：把「源层合到背板后」的 tile
/// 减去背板，得到可重新叠在同一背板上的最小 alpha 表示。
fn flat2rgba(dst: &mut [u16], bg: &[u16]) {
    debug_assert_eq!(dst.len(), TILE_CHANNELS);
    debug_assert_eq!(bg.len(), TILE_CHANNELS);
    for (d, b) in dst.chunks_exact_mut(4).zip(bg.chunks_exact(4)) {
        let mut final_alpha = d[3];
        for i in 0..3 {
            let change = d[i] as i32 - b[i] as i32;
            let minimal = if change > 0 {
                (change as i64 * (1 << 15)) / ((1 << 15) - b[i] as i64)
            } else if change < 0 {
                (-change as i64 * (1 << 15)) / (b[i] as i64).max(1)
            } else {
                0
            };
            final_alpha = final_alpha.max(minimal.clamp(0, 1 << 15) as u16);
        }
        d[3] = final_alpha;
        if final_alpha > 0 {
            for i in 0..3 {
                let change = d[i] as i32 - b[i] as i32;
                let res = (b[i] as i64 * final_alpha as i64) / (1 << 15) + change as i64;
                d[i] = res.clamp(0, final_alpha as i64) as u16;
            }
        } else {
            d[0] = 0;
            d[1] = 0;
            d[2] = 0;
        }
    }
}

/// 图层树的一层：`layers[0]` 是**最顶层**（`group.py:44-48`）。
///
/// 根栈（`RootLayerStack`）的背景按 `tree.py:54-58` 独立保管（不进树、不受
/// 图层操作影响）；组自己的子栈没有背景（`tree.py` 里只有根有背景层）。
pub struct LayerStack {
    layers: Vec<Layer>,
    /// 当前图层**路径**（`tree.py:954` `_current_path`；`(0,)` = 顶层的第一层）。
    /// 空路径 = 没有当前层（空栈）。
    current: Vec<usize>,
    /// 背景色（`tree.py:55` 默认 (255, 255, 255)）。有背景图时是它的平均值。
    background_color: [u8; 3],
    /// 平铺背景图（`tiledsurface.Background`；`None` = 纯色档）。
    background_image: Option<Arc<BackgroundImage>>,
    /// `tree.py:58` `_background_visible`。
    background_visible: bool,
    /// `tree.py:140` `_current_layer_solo`：只显示当前层/组及其子树，
    /// 祖先组按直通处理，背景不参与（`tree.py:281-304`）。
    current_layer_solo: bool,
    /// 合成版本号：栈结构/背景/图层属性变化时 +1（tile 内容变化由各层
    /// `surface.clock()` 独立计数，见 [`LayerStack::composite_revision`]）。
    rev: u64,
    /// 图层闪烁预览（`tree.py:1172-1193` 的 `current_layer_previewing`）：
    /// `Some(path)` 时只合成该层/组，并**反转**背景可见性。
    preview: Option<Vec<usize>>,
}

impl Default for LayerStack {
    fn default() -> Self {
        Self::new()
    }
}

/// 克隆共享各层 tile 的 `Arc`（COW），id 原样保留。
impl Clone for LayerStack {
    fn clone(&self) -> Self {
        Self {
            layers: self.layers.clone(),
            current: self.current.clone(),
            background_color: self.background_color,
            background_image: self.background_image.clone(),
            background_visible: self.background_visible,
            current_layer_solo: self.current_layer_solo,
            rev: self.rev,
            preview: self.preview.clone(),
        }
    }
}

impl LayerStack {
    /// 根栈：默认带一层空绘画层（`tree.py:230-237` 的 `ensure_initial_layer`）。
    pub fn new() -> Self {
        Self::from_children(vec![Layer::new()])
    }

    /// **组**的空栈（`group.py:78`：初始无子层；组可以为空）。
    pub fn empty() -> Self {
        Self {
            layers: Vec::new(),
            current: Vec::new(),
            background_color: [255, 255, 255],
            background_image: None,
            background_visible: true,
            current_layer_solo: false,
            rev: 0,
            preview: None,
        }
    }

    /// 栈/背景/图层属性变化后调用：抬高合成版本号。
    pub fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// 由一组**自顶向下**的层直接构造（`.ora` 载入路径；index 0 = 最顶层）。
    ///
    /// 空列表等价于 [`LayerStack::new`]（补一层空绘画层）。
    pub fn from_layers(layers: Vec<Layer>) -> Self {
        let mut layers = layers;
        if layers.is_empty() {
            layers.push(Layer::new());
        }
        Self::from_children(layers)
    }

    /// 直接接管一组子层（不补层；载入组/根栈共用）。
    pub fn from_children(layers: Vec<Layer>) -> Self {
        let current = if layers.is_empty() {
            Vec::new()
        } else {
            vec![0]
        };
        Self {
            layers,
            current,
            background_color: [255, 255, 255],
            background_image: None,
            background_visible: true,
            current_layer_solo: false,
            rev: 0,
            preview: None,
        }
    }

    // ---- 列表访问（`group.py` 的 list 语义；均为**本层**子层）----

    /// 子层数。
    /// 全树删除空 tile（`tree.py` 的 `remove_empty_tiles`）。
    /// 返回 `(removed, total)`，不生成撤销命令（源码 Debug 动作同样不可撤销）。
    pub fn remove_empty_tiles(&mut self) -> (usize, usize) {
        fn rec(layer: &mut Layer) -> (usize, usize) {
            match &mut layer.content {
                LayerContent::Painting(surface) => surface.remove_empty_tiles(),
                LayerContent::Group(stack) => {
                    let mut removed = 0;
                    let mut total = 0;
                    for child in &mut stack.layers {
                        let (r, t) = rec(child);
                        removed += r;
                        total += t;
                    }
                    (removed, total)
                }
            }
        }
        let mut removed = 0;
        let mut total = 0;
        for layer in &mut self.layers {
            let (r, t) = rec(layer);
            removed += r;
            total += t;
        }
        if removed > 0 {
            self.touch();
        }
        (removed, total)
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// 按索引取层（0 = 最顶层）；越界返回 `None`。
    #[inline]
    pub fn try_layer(&self, index: usize) -> Option<&Layer> {
        self.layers.get(index)
    }

    /// 按索引取层（0 = 最顶层）。
    #[inline]
    pub fn layer(&self, index: usize) -> &Layer {
        &self.layers[index]
    }

    #[inline]
    pub fn layer_mut(&mut self, index: usize) -> &mut Layer {
        &mut self.layers[index]
    }

    /// 自底向上迭代（显示合成顺序；`group.py:429` `reversed(self._layers)`）。
    pub fn iter_bottom_up(&self) -> impl Iterator<Item = (usize, &Layer)> {
        self.layers.iter().enumerate().rev()
    }

    /// 自顶向下迭代（UI 列表与 `.ora` 保存的条目顺序）。
    pub fn iter_top_down(&self) -> impl Iterator<Item = (usize, &Layer)> {
        self.layers.iter().enumerate()
    }

    // ---- 树 / 路径寻址（`tree.py` 的 walk/deepget/deepindex/deepinsert/deeppop）----

    /// 遍历整棵树，产出 `(path, layer)`（`tree.py:1514 walk`）：
    /// **组在子层之前**、子层紧跟自己的组、根不出现。
    ///
    /// 源码是 `queue[:0] = children`（**插到队首**），所以次序不是纯 BFS，而是
    /// 「组的子层立刻排在组后面、其余兄弟依次」——`tree.py` 的 doctest 给出
    /// `[(0,), (1,), (1,0), (1,1), (2,)]`。UI 列表与 `.ora` 保存都按这个序。
    pub fn walk(&self) -> Vec<(Vec<usize>, &Layer)> {
        let mut out: Vec<(Vec<usize>, &Layer)> = Vec::new();
        let mut queue: Vec<(Vec<usize>, &Layer)> = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (vec![i], l))
            .collect();
        while !queue.is_empty() {
            let (path, layer) = queue.remove(0);
            out.push((path.clone(), layer));
            if let Some(sub) = layer.stack() {
                let children: Vec<(Vec<usize>, &Layer)> = sub
                    .layers
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let mut p = path.clone();
                        p.push(i);
                        (p, l)
                    })
                    .collect();
                queue.splice(0..0, children);
            }
        }
        out
    }

    /// 按路径取层（`tree.py:1600 deepget`；空路径 = 栈自己，这里返回 `None`）。
    pub fn layer_at(&self, path: &[usize]) -> Option<&Layer> {
        let (last, rest) = path.split_last()?;
        let mut stack = self;
        for i in rest {
            stack = stack.layers.get(*i)?.stack()?;
        }
        stack.layers.get(*last)
    }

    /// 按路径取可变层。
    pub fn layer_at_mut(&mut self, path: &[usize]) -> Option<&mut Layer> {
        let (last, rest) = path.split_last()?;
        let mut stack = self;
        for i in rest {
            let layer = stack.layers.get_mut(*i)?;
            stack = match &mut layer.content {
                LayerContent::Group(s) => s,
                LayerContent::Painting(_) => return None,
            };
        }
        stack.layers.get_mut(*last)
    }

    /// 按 id 求路径（`tree.py:1754 deepindex`）。
    pub fn path_of_id(&self, id: u64) -> Option<Vec<usize>> {
        if self.current.len() > 0 {
            if let Some(l) = self.layer_at(&self.current) {
                if l.id == id {
                    return Some(self.current.clone());
                }
            }
        }
        self.walk()
            .into_iter()
            .find(|(_, l)| l.id == id)
            .map(|(p, _)| p)
    }

    /// 路径上的所有祖先层的可见性都成立（`core.py:359 branch_visible`）。
    pub fn branch_visible(&self, path: &[usize]) -> bool {
        let mut stack = self;
        for i in path {
            let Some(layer) = stack.layers.get(*i) else {
                return true;
            };
            if !layer.visible {
                return false;
            }
            stack = match layer.stack() {
                Some(s) => s,
                None => return true,
            };
        }
        true
    }

    /// 路径上有任一祖先被锁（`core.py:412 branch_locked`）。
    pub fn branch_locked(&self, path: &[usize]) -> bool {
        let mut stack = self;
        for i in path {
            let Some(layer) = stack.layers.get(*i) else {
                return false;
            };
            if layer.locked {
                return true;
            }
            stack = match layer.stack() {
                Some(s) => s,
                None => return false,
            };
        }
        false
    }

    /// 「此刻能不能往这层上画」：自身可画 + 分支可见 + 分支未锁
    /// （`data.py:1364 get_paintable` + `tree.py` 的当前层语义）。
    pub fn paintable_at(&self, path: &[usize]) -> Option<&Layer> {
        let layer = self.layer_at(path)?;
        // 祖先（不含自身）的 visible/locked
        let (_, parent_path) = path.split_last()?;
        if !layer.paintable_self() {
            return None;
        }
        if !self.branch_visible(parent_path) || self.branch_locked(parent_path) {
            return None;
        }
        Some(layer)
    }

    /// 当前层上可画的表面（笔刷热路径；组/锁/隐藏分支返回 `None`）。
    pub fn paintable_surface_mut(&mut self) -> Option<&mut PigmentSurface> {
        let path = self.current.clone();
        {
            let parent_len = path.len().saturating_sub(1);
            let layer = self.layer_at(&path)?;
            if !layer.paintable_self() || layer.mode.combine().is_none() {
                return None;
            }
            if !self.branch_visible(&path[..parent_len]) || self.branch_locked(&path[..parent_len])
            {
                return None;
            }
        }
        self.layer_at_mut(&path)?.surface_mut()
    }

    // ---- 当前层 ----

    /// 当前层路径（`tree.py:954 get_current_path`；空 = 没有当前层）。
    pub fn current_path(&self) -> &[usize] {
        &self.current
    }

    /// 当前层的**顶层**索引（平栈时就是索引；嵌套时是其祖先链首元素）。
    pub fn current_index(&self) -> usize {
        *self.current.first().unwrap_or(&0)
    }

    /// 顶层索引的便捷入口（平栈语义）。
    pub fn set_current_index(&mut self, index: usize) {
        let index = index.min(self.layers.len().saturating_sub(1));
        self.current = if self.layers.is_empty() {
            Vec::new()
        } else {
            vec![index]
        };
        self.touch();
    }

    /// 切到指定路径；路径不存在时逐级回退（`tree.py:968 set_current_path` 的
    /// "will be trimmed until it fits"）。
    pub fn set_current_path(&mut self, path: &[usize]) {
        let mut p = path.to_vec();
        while !p.is_empty() && self.layer_at(&p).is_none() {
            p.pop();
        }
        if p.is_empty() && !self.layers.is_empty() {
            p = vec![0];
        }
        if p != self.current {
            self.current = p;
            self.touch();
        }
    }

    /// 把当前层切到 id 对应的那层；返回是否找到（找不到不动）。
    pub fn set_current_by_id(&mut self, id: u64) -> bool {
        match self.path_of_id(id) {
            Some(p) => {
                self.current = p;
                self.touch();
                true
            }
            None => false,
        }
    }

    /// 当前层是否接受填充（`get_fillable()`：`data.py:1335` 的
    /// `not self.locked`，组恒 False，`group.py:459`）。
    ///
    /// 命令用它决定是否"自动新建一层再填"（`fill.py:215-216`）。
    pub fn current_is_fillable(&self) -> bool {
        self.layer_at(&self.current)
            .map(|l| !l.is_group() && !l.locked)
            .unwrap_or(false)
    }

    /// 当前位置是否允许收集种子（`fill.py:279-286` 的
    /// `target_layer.visible and not target_layer.locked`）。
    ///
    /// 与 [`Self::current_is_fillable`] 的区别：组在这里是 True（组可见时
    /// 用户可以在组上点，命令随后新建层）；锁定层两者都 False。
    pub fn current_fill_permitted(&self) -> bool {
        self.layer_at(&self.current)
            .map(|l| l.visible && !l.locked)
            .unwrap_or(false)
    }

    /// 当前层（空栈时 panic：根栈恒有 ≥1 层，见 `RemoveLayer` 的补层逻辑）。
    pub fn current(&self) -> &Layer {
        self.layer_at(&self.current)
            .expect("当前层路径失效（应当由结构操作维护）")
    }

    pub fn current_mut(&mut self) -> &mut Layer {
        let path = self.current.clone();
        self.layer_at_mut(&path)
            .expect("当前层路径失效（应当由结构操作维护）")
    }

    pub fn current_id(&self) -> u64 {
        self.current().id
    }

    /// 顶层 id 查找（平栈便捷入口；嵌套请用 [`LayerStack::path_of_id`]）。
    pub fn index_of_id(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// 按 id 取层（深搜整棵树，等价 MyPaint 的按对象引用寻址）。
    pub fn layer_by_id(&self, id: u64) -> Option<&Layer> {
        self.walk()
            .into_iter()
            .find(|(_, l)| l.id == id)
            .map(|(_, l)| l)
    }

    pub fn layer_by_id_mut(&mut self, id: u64) -> Option<&mut Layer> {
        let path = self.path_of_id(id)?;
        self.layer_at_mut(&path)
    }

    /// 按 id 取该层表面（供命令系统就地改像素）。
    pub fn surface_by_id(&mut self, id: u64) -> Option<&mut PigmentSurface> {
        self.layer_by_id_mut(id).and_then(|l| l.surface_mut())
    }

    /// 用户界面里“上一层”的路径（`tree.py:1219 path_above`）：
    /// walk 序中当前项的前一项，可跨出/进入组；栈顶返回 `None`。
    pub fn path_above(&self, path: &[usize]) -> Option<Vec<usize>> {
        let mut prev = None;
        for (p, _) in self.walk() {
            if p == path {
                return prev;
            }
            prev = Some(p);
        }
        None
    }

    /// 用户界面里“下一层”的路径（`tree.py:1293 path_below`）：
    /// walk 序中当前项的后一项，可跨出/进入组；栈底返回 `None`。
    pub fn path_below(&self, path: &[usize]) -> Option<Vec<usize>> {
        let walk = self.walk();
        walk.iter()
            .position(|(p, _)| p == path)
            .and_then(|i| walk.get(i + 1).map(|(p, _)| p.clone()))
    }

    /// `path_below(path, insert=True)`（`tree.py:1360-1369`）：
    /// 普通层插到当前层后一位；组插到组内最上层；越界钳位。
    pub fn path_below_insert(&self, path: &[usize]) -> Option<Vec<usize>> {
        let (&index, parent) = path.split_last()?;
        if self.layer_at(path)?.is_group() {
            let mut p = path.to_vec();
            p.push(0);
            return Some(p);
        }
        let len = self.layers_in(parent).map(|v| v.len())?;
        let mut p = parent.to_vec();
        p.push((index + 1).min(len));
        Some(p)
    }

    /// 层是否在**组内**（`_update_layer_bubble_actions` 的 `deep`）。
    pub fn is_deep(&self, path: &[usize]) -> bool {
        path.len() > 1
    }

    /// 冒泡动作是否可用 —— 逐行照 `document.py:1393-1405` 的
    /// `_update_layer_bubble_actions`：组内恒可（会出组），根层看端点。
    pub fn can_bubble(&self, path: &[usize], up: bool) -> bool {
        let Some((&index, _)) = path.split_last() else {
            return false;
        };
        if self.is_deep(path) {
            return true;
        }
        if up {
            index > 0
        } else {
            index + 1 < self.len()
        }
    }

    /// 冒泡一层（`tree.py:1380 _bubble_layer`）：在不破坏树结构的前提下
    /// 上移/下移。与纯兄弟换位不同，**到组端点时会出组**（插到父组在
    /// 祖父组里的前/后），相邻兄弟是组时会**入组**（放到该组近端）。
    ///
    /// 逆操作就是反方向再冒一次（源码 `BubbleLayerUp.undo` 就是这么写的，
    /// 两端语义对称）。返回是否真的动了。
    pub fn bubble_layer(&mut self, path: &[usize], upstack: bool) -> bool {
        let path = path.to_vec();
        let Some((&index, parent_path)) = path.split_last() else {
            return false;
        };
        let parent_path = parent_path.to_vec();
        let Some(count) = self.layers_in(&parent_path).map(|v| v.len()) else {
            return false;
        };
        if index >= count {
            return false;
        }
        let Some(id) = self.layer_at(&path).map(|l| l.id) else {
            return false;
        };

        // 已在本栈端点：出组，插到父组在祖父组里的前/后一格。
        let end_index = if upstack { 0 } else { count - 1 };
        if index == end_index {
            if parent_path.is_empty() {
                return false;
            }
            let grandparent_path = &parent_path[..parent_path.len() - 1];
            let parent_index = *parent_path.last().expect("parent_path 非空");
            let Some(layer) = self.remove_at(&path) else {
                return false;
            };
            let beyond_parent = parent_index + usize::from(!upstack);
            let mut insert_path = grandparent_path.to_vec();
            insert_path.push(beyond_parent);
            let moved = self.insert_at(&insert_path, layer).is_some();
            if moved {
                self.set_current_by_id(id);
            }
            return moved;
        }

        // 同栈内移动：相邻兄弟是组则入组，否则换位。
        let new_index = if upstack { index - 1 } else { index + 1 };
        let mut sibling_path = parent_path.clone();
        sibling_path.push(new_index);
        if self.layer_at(&sibling_path).is_some_and(|l| l.is_group()) {
            let Some(sibling_id) = self.layer_at(&sibling_path).map(|l| l.id) else {
                return false;
            };
            let Some(layer) = self.remove_at(&path) else {
                return false;
            };
            let Some(sibling_path) = self.path_of_id(sibling_id) else {
                // 极端情况：兄弟也随移除失效（正常结构下不会发生）。
                // 把层放回原位，不算移动。
                let mut restore = parent_path.clone();
                restore.push(index);
                self.insert_at(&restore, layer);
                self.set_current_by_id(id);
                return false;
            };
            let target_index = if upstack {
                self.layer_at(&sibling_path)
                    .map(|l| l.child_count())
                    .unwrap_or(0)
            } else {
                0
            };
            let mut insert_path = sibling_path;
            insert_path.push(target_index);
            let moved = self.insert_at(&insert_path, layer).is_some();
            if moved {
                self.set_current_by_id(id);
            }
            return moved;
        }

        let moved = self.swap_siblings(&parent_path, index, new_index);
        if moved {
            self.set_current_by_id(id);
        }
        moved
    }

    /// 交换同一父层下的两个兄弟（`BubbleLayerUp/Down` 用；空 `parent` = 根层）。
    ///
    /// 换位是自逆操作，撤销就是再换一次。
    pub fn swap_siblings(&mut self, parent: &[usize], a: usize, b: usize) -> bool {
        let Some(layers) = self.layers_in_mut(parent) else {
            return false;
        };
        if a >= layers.len() || b >= layers.len() || a == b {
            return false;
        }
        layers.swap(a, b);
        self.touch();
        true
    }

    /// 某坐标是否有**笔迹**（整棵树任一绘画层有 tile；不含背景）。
    ///
    /// GPU 同步的判据：背景 tile 不上传——面板底色就是文档背景色
    /// （`workspace` 的填充取 [`LayerStack::background_color`]），二者无缝衔接。
    pub fn has_paint_at(&self, tx: i32, ty: i32) -> bool {
        self.walk()
            .into_iter()
            .any(|(_, l)| l.surface().is_some_and(|s| s.tile_entry(tx, ty).is_some()))
    }

    /// 某坐标是否需要 GPU 上的合成结果（背景可见，或任一绘画层有 tile）。
    pub fn has_content_at(&self, tx: i32, ty: i32) -> bool {
        self.background_visible || self.has_paint_at(tx, ty)
    }

    /// 所有**绘画层**表面的引用（顺序同 [`Self::walk`]，组在前）。
    ///
    /// 逐帧的屏幕 tile 循环要对每个可见格问"这里有没有内容"，而 `walk()`
    /// 每次都会为路径 `Vec` 分配内存：一帧上千格就是上千次分配。这里一次性
    /// 取出来复用。
    pub fn painting_surfaces(&self) -> Vec<&PigmentSurface> {
        self.walk()
            .into_iter()
            .filter_map(|(_, l)| l.surface())
            .collect()
    }

    /// 某坐标在**某个 mipmap 级别**下是否有笔迹。
    ///
    /// `level == 0` 与 [`Self::has_paint_at`] 等价。级别 > 0 时问的是
    /// "这一格的金字塔内容在不在"——直接读 mipmap（一次哈希查找，必要时
    /// 惰性重建并缓存）。**不能**退化成遍历 `2^L × 2^L` 个基 tile：level 4
    /// 是 256 次查找/格，远景一帧上千格就直接爆了。
    ///
    /// 逐帧热路径请先取 [`Self::painting_surfaces`] 再自己遍历，省掉
    /// `walk()` 的分配。
    pub fn has_paint_at_level(&self, level: u32, tx: i32, ty: i32) -> bool {
        self.painting_surfaces()
            .into_iter()
            .any(|s| s.mip_tile(level, tx, ty).is_some())
    }

    /// 级别版的 [`Self::has_content_at`]。
    pub fn has_content_at_level(&self, level: u32, tx: i32, ty: i32) -> bool {
        self.background_visible || self.has_paint_at_level(level, tx, ty)
    }

    /// 不触发 mip 重建的级别版内容判断（渲染规划热路径）。
    pub fn has_paint_at_level_cached(&self, level: u32, tx: i32, ty: i32) -> bool {
        self.painting_surfaces()
            .into_iter()
            .any(|s| s.mip_has_content_cached(level, tx, ty))
    }

    /// 不触发 mip 重建的级别版 [`Self::has_content_at`]。
    pub fn has_content_at_level_cached(&self, level: u32, tx: i32, ty: i32) -> bool {
        self.background_visible || self.has_paint_at_level_cached(level, tx, ty)
    }

    /// 合成版本号：任何影响"所有 tile 合成结果"的变化（结构/背景/属性）
    /// 都会抬高它；tile 级内容变化看各层 `surface.clock()`。GPU 同步把
    /// 两者合起来当水位线。
    pub fn composite_revision(&self) -> u64 {
        let mut rev = self.rev;
        for (_, layer) in self.walk() {
            if let Some(surface) = layer.surface() {
                rev = rev.wrapping_add(surface.clock().wrapping_mul(0x9E37_79B9_7F4A_7C15));
            }
        }
        rev
    }

    /// 单个 tile 的合成版本号。
    ///
    /// [`Self::composite_revision`] 是全局水位线，任何一笔都会让它变化；用它
    /// 驱动 GPU 同步会把**全屏所有可见 tile** 都判成脏，这是绘画卡顿的主因。
    /// 这里把版本号细化到 tile：只有该坐标上真正参与合成的层内容/属性变了，
    /// 值才变。
    ///
    /// 组合了四类输入：
    /// - 栈结构/背景/预览/solo（`self.rev` 与背景状态）；
    /// - 每层的 id/visible/opacity/mode（属性变化即使没 bump `rev` 也会变）；
    /// - 该层在该坐标的 `Tile::revision()`（0 = 没有 tile）；
    /// - 遍历顺序（用顺序 mix，而非可交换的加法）。
    ///
    /// 这是精确到 tile 的保守上界：某层在该坐标改动只会让这一个 tile
    /// 重新合成；任何全局属性改动会自然让所有 tile 的哈希一起变化。
    pub fn tile_composite_revision(&self, tx: i32, ty: i32) -> u64 {
        self.tile_composite_revision_at(0, tx, ty)
    }

    /// 单个 tile 在**某个 mipmap 级别**下的合成版本号。
    ///
    /// `tx/ty` 是该级别下的 tile 索引。`level == 0` 的返回值与
    /// [`Self::tile_composite_revision`] **完全相同**（级别为 0 时不额外 mix，
    /// 保持既有水位线的可比性——缓存/存档里存过旧值的地方不会集体失效）。
    ///
    /// 级别 > 0 时，参与哈希的"tile 内容版本"换成
    /// [`crate::PigmentSurface::mip_tile_revision`]：它会把该格材料化
    /// （必要时重建金字塔）再取版本号，所以脏 → 已重建的中间态不会多产生
    /// 一次"假变化"。**否则 M3 的脏检测会把 level 4 的内容当成 level 0 的
    /// 版本**，远景就永远不刷新。
    pub fn tile_composite_revision_at(&self, level: u32, tx: i32, ty: i32) -> u64 {
        self.tile_composite_revision_impl(level, tx, ty, true)
    }

    /// 不触发 mip 重建的版本号，供渲染规划使用。
    ///
    /// 未重建/已标脏的 mip 格返回 0，因此会被判成"需要上传"；真正上传前
    /// 会先合成，届时再用 [`Self::tile_composite_revision_at`] 记录水位线。
    pub fn tile_composite_revision_at_cached(&self, level: u32, tx: i32, ty: i32) -> u64 {
        self.tile_composite_revision_impl(level, tx, ty, false)
    }

    fn tile_composite_revision_impl(
        &self,
        level: u32,
        tx: i32,
        ty: i32,
        materialize: bool,
    ) -> u64 {
        #[inline]
        fn mix(h: u64, v: u64) -> u64 {
            (h ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17)
        }
        let mut h = mix(0xCBF2_9CE4_8422_2325, self.rev);
        h = mix(h, self.background_visible as u64);
        h = mix(
            h,
            u64::from(self.background_color[0])
                | (u64::from(self.background_color[1]) << 8)
                | (u64::from(self.background_color[2]) << 16),
        );
        h = mix(
            h,
            self.background_image
                .as_ref()
                .map_or(0, |img| std::sync::Arc::as_ptr(img) as u64),
        );
        h = mix(h, self.current_layer_solo as u64);
        if let Some(preview) = &self.preview {
            for &i in preview {
                h = mix(h, i as u64);
            }
        }
        // 级别非 0 才 mix：保证 level 0 的水位线与旧版逐位一致。
        let level = level.min(MAX_MIPMAP_LEVEL);
        if level != 0 {
            h = mix(h, level as u64);
        }
        for (_, layer) in self.walk() {
            let mode = match layer.mode {
                LayerMode::PassThrough => u64::MAX,
                LayerMode::Standard(m) => m as u64,
            };
            let mut lh = mix(layer.id, layer.visible as u64);
            lh = mix(lh, u64::from(layer.opacity.to_bits()));
            lh = mix(lh, mode);
            if let Some(surface) = layer.surface() {
                let tile_rev = if level == 0 {
                    surface
                        .tile_entry(tx, ty)
                        .map(crate::Tile::revision)
                        .unwrap_or(0)
                } else if materialize {
                    surface.mip_tile_revision(level, tx, ty)
                } else {
                    surface.mip_tile_revision_cached(level, tx, ty)
                };
                lh = mix(lh, tile_rev);
            }
            h = mix(h, lh);
        }
        h
    }

    // ---- 结构操作（`group.py` insert/pop；撤销命令在 pigment-engine 侧做）----

    /// 在 `path` 处插入层并返回实际路径（`tree.py:1638 deepinsert`）。
    ///
    /// 路径的**最后一段**是插入索引（钳位，允许 9999/负数语义按 `insert` 处理），
    /// 前面的段必须指向存在的组。
    pub fn insert_at(&mut self, path: &[usize], layer: Layer) -> Option<Vec<usize>> {
        let (index, parent) = path.split_last()?;
        let index = (*index).min(self.layers_in(parent)?.len());
        let inserted_id = layer.id;
        self.layers_in_mut(parent)?.insert(index, layer);
        let mut p = parent.to_vec();
        p.push(index);
        // 当前层在插入点之后 → 跟着后移（`group.py:320` insert 的语义）
        if self.current.len() > parent.len() && self.current.starts_with(parent) {
            let shifted = &self.current[parent.len()];
            if *shifted >= index {
                let mut c = self.current.clone();
                c[parent.len()] += 1;
                self.current = c;
            }
        }
        self.touch();
        debug_assert_eq!(self.path_of_id(inserted_id), Some(p.clone()));
        Some(p)
    }

    /// 按路径移除层并返回它（`tree.py:1691 deeppop`）。
    pub fn remove_at(&mut self, path: &[usize]) -> Option<Layer> {
        let (index, parent) = path.split_last()?;
        let removed = {
            let layers = self.layers_in_mut(parent)?;
            if *index >= layers.len() {
                return None;
            }
            layers.remove(*index)
        };
        // 当前层失效/后移
        if self.current.starts_with(parent) && self.current.len() > parent.len() {
            let shifted = self.current[parent.len()];
            if shifted > *index {
                let mut c = self.current.clone();
                c[parent.len()] -= 1;
                self.current = c;
            } else if shifted == *index {
                let mut c = self.current.clone();
                c.truncate(parent.len());
                self.current = c;
            }
        }
        self.fix_current();
        self.touch();
        Some(removed)
    }

    /// 取任意深度子栈（空路径 = 自己）。
    fn layers_in(&self, path: &[usize]) -> Option<&Vec<Layer>> {
        let mut stack = self;
        for i in path {
            stack = stack.layers.get(*i)?.stack()?;
        }
        Some(&stack.layers)
    }

    fn layers_in_mut(&mut self, path: &[usize]) -> Option<&mut Vec<Layer>> {
        let mut stack = self;
        for i in path {
            let layer = stack.layers.get_mut(*i)?;
            stack = match &mut layer.content {
                LayerContent::Group(s) => s,
                LayerContent::Painting(_) => return None,
            };
        }
        Some(&mut stack.layers)
    }

    /// 当前路径失效时回退到最近的可用层（`set_current_path` 的裁剪语义）。
    fn fix_current(&mut self) {
        let mut p = std::mem::take(&mut self.current);
        while !p.is_empty() && self.layer_at(&p).is_none() {
            p.pop();
        }
        if p.is_empty() && !self.layers.is_empty() {
            p = vec![0];
        }
        self.current = p;
    }

    /// 在 `index` 前插入层并返回实际索引（`group.py:320` `insert` 的钳位语义）。
    pub fn insert(&mut self, index: usize, layer: Layer) -> usize {
        let index = index.min(self.layers.len());
        self.layers.insert(index, layer);
        if let Some(first) = self.current.first_mut() {
            if *first >= index {
                *first += 1;
            }
        }
        self.touch();
        index
    }

    /// 按顶层索引移除层（`group.py:286` `pop`）。
    ///
    /// 允许把栈删空：「至少一层」的约束由 RemoveLayer 命令负责（照
    /// `lib/command.py:1040-1055` 的 `CREATE_PAINTING_LAYER_IF_EMPTY`）；
    /// 补层逻辑在命令里，撤销才能精确恢复结构。
    /// 按 id 移除层（返回被移除的层；id 不存在时 `None`）。
    pub fn remove_by_id(&mut self, id: u64) -> Option<Layer> {
        let path = self.path_of_id(id)?;
        self.remove_at(&path)
    }

    /// 按顶层索引移除层。
    pub fn remove(&mut self, index: usize) -> Layer {
        let removed = self.layers.remove(index);
        self.fix_current();
        self.touch();
        removed
    }

    /// 整棵树的数据外接框（`group.py:348 get_bbox` = 各子层并集）。
    pub fn data_bbox(&self) -> (i32, i32, i32, i32) {
        union_bbox(self.walk().into_iter().map(|(_, l)| l.data_bbox()))
    }

    // ---- 背景层（`tree.py:54-58` / `data.py:1023-1060`）----

    pub fn background_color(&self) -> [u8; 3] {
        self.background_color
    }

    /// 当前平铺背景图（纯色档为 `None`）。
    #[inline]
    pub fn background_image(&self) -> Option<&Arc<BackgroundImage>> {
        self.background_image.as_ref()
    }

    /// 是否使用平铺背景图 —— GPU 上传判据（纯色背景仍由面板底色兜底）。
    #[inline]
    pub fn background_is_image(&self) -> bool {
        self.background_image.is_some()
    }

    pub fn set_background_color(&mut self, rgb: [u8; 3]) {
        // 对应 `set_background((r,g,b))`：换成纯色后不再平铺旧图。
        self.background_color = rgb;
        self.background_image = None;
        self.touch();
    }

    /// 换成平铺背景图（`set_background(pixbuf)`，`tree.py:1025-1038`）。
    ///
    /// `background_color` 同步成图的平均色，供面板底色/纯色回退使用。
    pub fn set_background_image(&mut self, image: BackgroundImage) {
        self.background_color = image.average_rgb();
        self.background_image = Some(Arc::new(image));
        self.touch();
    }

    /// 清掉平铺图，回到当前平均色（不重置颜色）。
    pub fn clear_background_image(&mut self) {
        if self.background_image.take().is_some() {
            self.touch();
        }
    }

    pub fn background_visible(&self) -> bool {
        self.background_visible
    }

    pub fn set_background_visible(&mut self, visible: bool) {
        self.background_visible = visible;
        self.touch();
    }

    /// 背景在 `(tx, ty)` 处的 fix15 tile（`BackgroundLayer`：mode=Normal、opacity=1）。
    ///
    /// 纯色档填满单色；图案档从**本级**的降采样图平铺采样（`Background` 的循环
    /// 平铺 —— `tiledsurface.py:1178-1217`；`sel.level` 决定用哪一级的图）。
    ///
    /// `level > 0` 时用盒式平均图而不是"取 2^L 块左上角"：源码的背景 mipmap
    /// 就是逐级 `tile_downscale_rgba16`（`:1205-1217`），点采样会让纸纹在远景
    /// 出摩尔纹。`level == 0` 与旧实现逐像素一致（0 级图就是原图）。
    fn background_tile_at(&self, sel: TileSel) -> Box<TilePixels> {
        let mut tile = empty_tile();
        if let Some(img) = &self.background_image {
            let lv = img.level_image(sel.level);
            // 级别空间的平铺原点（一格 = 64 个本级像素 = 64<<level 世界像素）
            let ox = sel.tx as i64 * TILE_SIZE as i64;
            let oy = sel.ty as i64 * TILE_SIZE as i64;
            for (y, row) in tile.iter_mut().enumerate() {
                for (x, p) in row.iter_mut().enumerate() {
                    *p = lv.pixel(ox + x as i64, oy + y as i64);
                }
            }
            return tile;
        }
        let [r, g, b] = self.background_color;
        // 背景与载入 PNG 同一条转换路径（EOTF 2.2 + 预乘，alpha 满）
        let px = color::rgba8_to_fix15(r, g, b, 255);
        for row in tile.iter_mut() {
            for p in row.iter_mut() {
                *p = px;
            }
        }
        tile
    }

    /// `get_render_is_opaque`（`tree.py:306-330`）：背景参与、且**任何可见层**
    /// 都不是 alpha 递减模式（dst-in/out/atop）时，合成结果恒不透明。
    pub fn render_is_opaque(&self) -> bool {
        self.render_is_opaque_with_previewing(true)
    }

    /// 忽略闪烁预览的不透明判定：导出/合并走 `respect_previewing=False`
    /// （`tree.py:653-669`、`:2178`）。
    fn render_is_opaque_with_previewing(&self, previewing: bool) -> bool {
        // `_get_render_background`（`tree.py:281-304`）：预览反转背景可见性，
        // 且优先于 solo；solo 本身永不显示背景。
        let background_shown = if previewing && self.preview.is_some() {
            !self.background_visible
        } else if self.current_layer_solo {
            false
        } else {
            self.background_visible
        };
        if !background_shown {
            return false;
        }
        for (path, layer) in self.walk() {
            if !self.branch_visible(&path) {
                continue;
            }
            if let LayerMode::Standard(m) = layer.mode {
                if m.can_decrease_alpha() {
                    return false;
                }
            }
        }
        true
    }

    /// 图层闪烁预览的路径（`tree.py:1172` 的 `current_layer_previewing`）。
    pub fn preview_path(&self) -> Option<&[usize]> {
        self.preview.as_deref()
    }

    /// `tree.py:1146-1153` 的 `current_layer_solo`。
    pub fn current_layer_solo(&self) -> bool {
        self.current_layer_solo
    }

    /// 开关 Solo（`tree.py:1155-1163`）；变化时抬高合成版本号。
    pub fn set_current_layer_solo(&mut self, solo: bool) {
        if self.current_layer_solo != solo {
            self.current_layer_solo = solo;
            self.touch();
        }
    }

    /// 把本栈的子层合成到**已有内容**的 `dst` 上（不清空）。
    ///
    /// 供 `layer_new_normalized` 的「先在背板上渲染源层」步骤用；调用方负责
    /// 传入与 `dst` 一致的 `has_alpha`。背景不参与。
    pub fn composite_tile_over(&self, tx: i32, ty: i32, dst: &mut [u16], has_alpha: bool) -> bool {
        self.composite_tile_over_at(0, tx, ty, dst, has_alpha)
    }

    /// 同 [`Self::composite_tile_over`]，但按 mipmap 级别取样
    /// （`blit_tile_into(..., mipmap_level)` 的等价路径）。
    pub fn composite_tile_over_at(
        &self,
        level: u32,
        tx: i32,
        ty: i32,
        dst: &mut [u16],
        has_alpha: bool,
    ) -> bool {
        debug_assert_eq!(dst.len(), TILE_CHANNELS);
        let mut pool: Vec<Vec<u16>> = Vec::new();
        composite_layers(
            &self.layers,
            TileSel::new(level, tx, ty),
            dst,
            has_alpha,
            0,
            &mut pool,
        )
    }

    /// 构造 Solo 渲染用的裁剪栈（`tree.py:365-383` + `data.py:353-358`
    /// + `group.py:400-427`）：
    /// - 只保留当前路径上的祖先链和当前层子树；
    /// - 祖先组强制可见、直通、opacity=1（祖先的 mode/visibility 不参与）；
    /// - 当前层强制 visible=true，但保留自己的 mode/opacity；
    /// - 当前层的子层保持自身可见性/模式。
    fn solo_stack(&self, path: &[usize]) -> Option<LayerStack> {
        fn prune(stack: &LayerStack, path: &[usize]) -> Option<Vec<Layer>> {
            let (&idx, rest) = path.split_first()?;
            let mut layer = stack.layers.get(idx)?.clone();
            if rest.is_empty() {
                layer.visible = true;
                return Some(vec![layer]);
            }
            {
                let sub = layer.stack_mut()?;
                let children = prune(sub, rest)?;
                *sub = LayerStack::from_children(children);
            }
            layer.visible = true;
            layer.opacity = 1.0;
            layer.set_mode(LayerMode::PassThrough);
            Some(vec![layer])
        }
        let layers = prune(self, path)?;
        Some(LayerStack::from_children(layers))
    }

    /// 构造 **背板** 裁剪栈（`tree.py:385-409` 的
    /// `_get_backdrop_render_spec_for_layer`）：保留当前层以下的所有兄弟层，
    /// 以及路径上的祖先组；当前层子树和它上面的内容全部排除。
    fn backdrop_stack(&self, path: &[usize]) -> Option<LayerStack> {
        fn rec(stack: &LayerStack, path: &[usize]) -> Option<Vec<Layer>> {
            let (&idx, rest) = path.split_first()?;
            if rest.is_empty() {
                return Some(stack.layers.iter().skip(idx + 1).cloned().collect());
            }
            let mut group = stack.layers.get(idx)?.clone();
            {
                let sub = group.stack_mut()?;
                let children = rec(sub, rest)?;
                *sub = LayerStack::from_children(children);
            }
            let mut out = vec![group];
            out.extend(stack.layers.iter().skip(idx + 1).cloned());
            Some(out)
        }
        Some(LayerStack::from_children(rec(self, path)?))
    }

    /// `tree.py:1887-2001` 的 `layer_new_normalized`：把 `path` 指向的图层/组
    /// 转成外观等价的普通绘画层。
    ///
    /// - 不可见层 → 空绘画层；
    /// - 普通绘画层（Normal + opacity=1）或光谱绘画层 → COW 克隆；
    /// - 其余层/组：solo 渲染源层，必要时再渲染背板并做 `tile_flat2rgba`
    ///   减法，最后落到一张全不透明、Normal（光谱源除外）的绘画层。
    pub fn layer_new_normalized(&self, path: &[usize]) -> Option<Layer> {
        let src = self.layer_at(path)?;
        if !src.visible {
            let mut out = Layer::new();
            out.name = src.name.clone();
            return Some(out);
        }
        let spectral = src.mode == LayerMode::Standard(CombineMode::SpectralWGM);
        let simple_normal =
            src.mode == LayerMode::Standard(CombineMode::Normal) && src.opacity == 1.0;
        if src.surface().is_some() && (spectral || simple_normal) {
            return Some(src.clone());
        }

        let mut out = Layer::new();
        out.name = src.name.clone();
        out.set_mode(if spectral {
            LayerMode::Standard(CombineMode::SpectralWGM)
        } else {
            LayerMode::Standard(CombineMode::Normal)
        });

        let mut tiles = BTreeSet::new();
        let mut strokes = Vec::new();
        for (p, layer) in self.walk() {
            if !p.starts_with(path) {
                continue;
            }
            tiles.extend(layer.tile_coords());
            if let Some(surface) = layer.surface() {
                if !layer.locked && !self.branch_locked(&p) {
                    // 源码 `dstlayer.strokes[:0] = layer.strokes`：后访问的
                    // 层排在前面。
                    let mut merged = surface.strokes().to_vec();
                    merged.extend(strokes);
                    strokes = merged;
                }
            }
        }

        let source = self.solo_stack(path)?;
        let needs_backdrop = !simple_normal;
        let backdrop = if needs_backdrop {
            self.backdrop_stack(path)
        } else {
            None
        };
        let out_surface = out.surface_mut()?;
        for (tx, ty) in tiles {
            let mut bd = vec![0u16; TILE_CHANNELS];
            if let Some(backdrop) = &backdrop {
                backdrop.composite_tile_with_background(tx, ty, &mut bd, false);
            }
            let mut dst = bd.clone();
            source.composite_tile_over(tx, ty, &mut dst, true);
            if needs_backdrop {
                // 丢弃原 alpha，让 flat2rgba 重新计算“相对背板的最小 alpha”。
                for px in dst.chunks_exact_mut(4) {
                    px[3] = 0;
                }
                flat2rgba(&mut dst, &bd);
            }
            out_surface.paste_tile(tx, ty, &dst);
        }
        out_surface.replace_strokes(strokes);
        Some(out)
    }

    /// `tree.py:2122-2197` 的 `layer_new_merge_visible`：把所有可见层合成到
    /// 一张新的 Normal 绘画层；背景可见时减去背景，保证叠回原背景外观不变。
    pub fn layer_new_merge_visible(&self) -> Layer {
        let mut tiles = BTreeSet::new();
        let mut strokes = Vec::new();
        let mut names = Vec::new();
        for (path, layer) in self.walk() {
            if !layer.visible || !self.branch_visible(&path) {
                continue;
            }
            tiles.extend(layer.tile_coords());
            if let Some(surface) = layer.surface() {
                if !layer.locked && !self.branch_locked(&path) {
                    let mut merged = surface.strokes().to_vec();
                    merged.extend(strokes);
                    strokes = merged;
                }
            }
            if layer.has_interesting_name() {
                if let Some(name) = &layer.name {
                    names.push(name.clone());
                }
            }
        }
        let mut out = Layer::new();
        if !names.is_empty() {
            out.name = Some(names.join(", "));
        }
        out.set_mode(LayerMode::Standard(CombineMode::Normal));
        let show_bg = self.background_visible;
        let out_surface = out.surface_mut().expect("new painting layer");
        for (tx, ty) in tiles {
            let mut dst = vec![0u16; TILE_CHANNELS];
            self.composite_tile_with_background(tx, ty, &mut dst, show_bg);
            if show_bg {
                let bg = self.background_tile_at(TileSel::base(tx, ty));
                for px in dst.chunks_exact_mut(4) {
                    px[3] = 0;
                }
                flat2rgba(&mut dst, bg.as_flattened().as_flattened());
            }
            out_surface.paste_tile(tx, ty, &dst);
        }
        out_surface.replace_strokes(strokes);
        out
    }

    /// `tree.py:2201-2244` 的 `uniq_layer`：删除当前绘画层中**不改变背板**的
    /// tile；`pixels=true` 时再把 tile 内不改变背板的像素清零。
    ///
    /// 返回修改后的新层（保留原模式/opacity/strokemap）；不可用于组。
    pub fn uniq_layer(&self, path: &[usize], pixels: bool) -> Option<Layer> {
        let src = self.layer_at(path)?;
        let surface = src.surface()?;
        let source = self.solo_stack(path)?;
        let backdrop = self.backdrop_stack(path);
        let mut out = src.clone();
        let mut carved = PigmentSurface::new();
        for ((tx, ty), tile) in surface.tiles() {
            let mut bd = vec![0u16; TILE_CHANNELS];
            if let Some(backdrop) = &backdrop {
                backdrop.composite_tile_with_background(tx, ty, &mut bd, false);
            }
            let mut target = bd.clone();
            source.composite_tile_over(tx, ty, &mut target, true);
            let mut keep = tile.pixels().as_flattened().as_flattened().to_vec();
            let mut all_equal = true;
            for i in 0..TILE_SIZE * TILE_SIZE {
                let equal = target[i * 4..i * 4 + 4] == bd[i * 4..i * 4 + 4];
                all_equal &= equal;
                if pixels && equal {
                    keep[i * 4..i * 4 + 4].fill(0);
                }
            }
            if all_equal {
                continue;
            }
            carved.paste_tile(tx, ty, &keep);
        }
        carved.replace_strokes(surface.strokes().to_vec());
        *out.surface_mut()? = carved;
        Some(out)
    }

    /// `tree.py:2246-2335` 的 `refactor_layer_group`：把组内各子层的公共
    /// tile/像素抽到新建的 `Common` 层，并从原子层删除。
    ///
    /// 只支持**隔离组 + 绘画子层**；pass-through 组、空组、含子组的组返回
    /// `None`（源码对子组也会走 `_surface`，实际不可用）。
    pub fn refactor_layer_group(&self, path: &[usize], pixels: bool) -> Option<Layer> {
        let group = self.layer_at(path)?;
        if !group.is_group() || group.mode.is_pass_through() {
            return None;
        }
        let stack = group.stack()?;
        if stack.layers.is_empty() {
            return None;
        }
        // 源码只归一化 mode != Normal 的子层；子组不做。
        let mut children = Vec::with_capacity(stack.layers.len());
        for (i, child) in stack.layers.iter().enumerate() {
            if child.is_group() {
                return None;
            }
            if child.mode == LayerMode::Standard(CombineMode::Normal) {
                children.push(child.clone());
            } else {
                let mut child_path = path.to_vec();
                child_path.push(i);
                children.push(self.layer_new_normalized(&child_path)?);
            }
        }
        let mut maps: Vec<std::collections::BTreeMap<(i32, i32), Vec<u16>>> = children
            .iter()
            .map(|child| {
                child
                    .surface()
                    .expect("painting child")
                    .tiles()
                    .map(|(c, t)| (c, t.pixels().as_flattened().as_flattened().to_vec()))
                    .collect()
            })
            .collect();
        let mut tiles = BTreeSet::new();
        for map in &maps {
            tiles.extend(map.keys().copied());
        }
        let mut common_map = std::collections::BTreeMap::new();
        for (tx, ty) in tiles {
            let mut common = vec![true; TILE_SIZE * TILE_SIZE];
            let mut rgba0: Option<Vec<u16>> = None;
            for child in &children {
                let mut rgba = vec![0u16; TILE_CHANNELS];
                let solo = LayerStack::from_children(vec![child.clone()]);
                solo.composite_tile_with_background(tx, ty, &mut rgba, false);
                if let Some(first) = &rgba0 {
                    for i in 0..TILE_SIZE * TILE_SIZE {
                        if rgba[i * 4..i * 4 + 4] != first[i * 4..i * 4 + 4] {
                            common[i] = false;
                        }
                    }
                } else {
                    rgba0 = Some(rgba);
                }
            }
            let all = common.iter().all(|x| *x);
            let any = common.iter().any(|x| *x);
            if all {
                if let Some(tile) = maps[0].get(&(tx, ty)) {
                    common_map.insert((tx, ty), tile.clone());
                }
                for map in &mut maps {
                    map.remove(&(tx, ty));
                }
            } else if pixels && any {
                let Some(tile0) = maps[0].get(&(tx, ty)).cloned() else {
                    continue;
                };
                let mut extracted = tile0.clone();
                for i in 0..TILE_SIZE * TILE_SIZE {
                    if !common[i] {
                        extracted[i * 4..i * 4 + 4].fill(0);
                    }
                }
                common_map.insert((tx, ty), extracted);
                for map in &mut maps {
                    if let Some(tile) = map.get_mut(&(tx, ty)) {
                        for i in 0..TILE_SIZE * TILE_SIZE {
                            if common[i] {
                                tile[i * 4..i * 4 + 4].fill(0);
                            }
                        }
                    }
                }
            }
        }
        let build_surface = |map: &std::collections::BTreeMap<(i32, i32), Vec<u16>>,
                             strokes: &[crate::StrokeShape]| {
            let mut surface = PigmentSurface::new();
            for (&(tx, ty), px) in map {
                surface.paste_tile(tx, ty, px);
            }
            surface.replace_strokes(strokes.to_vec());
            surface
        };
        let mut new_children = Vec::with_capacity(children.len() + 1);
        for (child, map) in children.into_iter().zip(maps.iter()) {
            let strokes = child
                .surface()
                .map(|s| s.strokes().to_vec())
                .unwrap_or_default();
            let mut child = child;
            *child.surface_mut().expect("painting child") = build_surface(map, &strokes);
            new_children.push(child);
        }
        let mut common = Layer::new();
        common.name = Some("Common".to_string());
        common.set_mode(LayerMode::Standard(CombineMode::Normal));
        *common.surface_mut().expect("painting common") = build_surface(&common_map, &[]);
        new_children.push(common);

        let mut out = group.clone();
        *out.stack_mut()? = LayerStack::from_children(new_children);
        Some(out)
    }

    /// 开关图层闪烁预览；变化时抬高合成版本号。
    pub fn set_preview_path(&mut self, path: Option<Vec<usize>>) {
        if self.preview != path {
            self.preview = path;
            self.touch();
        }
    }

    /// 只合成闪烁预览层（+ 反转后的背景）——`tree.py:288-306/320-323`。
    ///
    /// 预览渲染规格（`_get_render_spec`，`tree.py:365-383`）把当前路径及其
    /// 子树之外的图层全部排除；子树内强制可见、组直通、普通层用默认模式且
    /// 不透明度 1（`group.py:393-398`、`data.py:348-352`）。
    fn composite_preview(&self, sel: TileSel, dst: &mut [u16]) -> bool {
        debug_assert_eq!(dst.len(), TILE_CHANNELS);
        dst.fill(0);
        let dst_has_alpha = !self.render_is_opaque();
        // `_get_render_background`：previewing 时**反转**背景可见性
        let show_bg = !self.background_visible;
        let mut content = false;
        if show_bg {
            let bg = self.background_tile_at(sel);
            dst.copy_from_slice(bg.as_flattened().as_flattened());
            content = true;
        }
        let Some(path) = self.preview.as_deref() else {
            return content;
        };
        let Some(layer) = self.layer_at(path) else {
            return content;
        };
        content |= composite_preview_layer(layer, sel, dst, dst_has_alpha);
        content
    }

    /// 把 `(tx, ty)` 处的栈合成结果写入 `dst`（fix15 预乘，长度 = tile 通道数）。
    ///
    /// 返回 `true` 表示结果非空透明块：背景可见，或该坐标存在任一层的内容。
    /// 语义照 `tree.py:796-811` 的 op 流（背景 BLIT → 子层自底向上）与
    /// `group.py:424-432` 的 PUSH/POP（组的隔离缓冲/直通决定），透明源规则见
    /// `tiledsurface.py:440-457`。若处于图层闪烁预览，只合成当前层。
    pub fn composite_tile(&self, tx: i32, ty: i32, dst: &mut [u16]) -> bool {
        self.composite_tile_at(0, tx, ty, dst)
    }

    /// 同 [`Self::composite_tile`]，但按 mipmap 级别合成。
    ///
    /// `tx/ty` 是**该级别下的 tile 索引**（level L 一格 = `2^L × 2^L` 个基
    /// tile 的内容，由 [`crate::PigmentSurface::mip_tile`] 惰性提供）。
    /// `level == 0` 时与 [`Self::composite_tile`] 逐字节一致——这是远景
    /// 屏幕 tile 循环（M3）的入口：每个屏幕 tile 只需要一格源 tile。
    pub fn composite_tile_at(&self, level: u32, tx: i32, ty: i32, dst: &mut [u16]) -> bool {
        let sel = TileSel::new(level, tx, ty);
        if self.preview.is_some() {
            return self.composite_preview(sel, dst);
        }
        if self.current_layer_solo {
            if let Some(solo) = self.solo_stack(self.current_path()) {
                // Solo 永不画背景（`tree.py:291-302`）。
                return solo.composite_tile_with_background_at(sel.level, sel.tx, sel.ty, dst, false);
            }
        }
        self.composite_tile_with_background_at(sel.level, sel.tx, sel.ty, dst, true)
    }

    /// 同 [`Self::composite_tile`]，但可显式排除背景层。
    ///
    /// 导出 PNG 时源码传 `render_background = not alpha`（`document.py:1677`）：
    /// `alpha=True` 的导出**不画背景**（保留透明），`alpha=False` 才画。
    /// 旧实现两者都从背景 tile 起，导致带 alpha 的 PNG 变成不透明纸色。
    pub fn composite_tile_with_background(
        &self,
        tx: i32,
        ty: i32,
        dst: &mut [u16],
        include_background: bool,
    ) -> bool {
        self.composite_tile_with_background_at(0, tx, ty, dst, include_background)
    }

    /// 同 [`Self::composite_tile_with_background`]，但按 mipmap 级别合成。
    pub fn composite_tile_with_background_at(
        &self,
        level: u32,
        tx: i32,
        ty: i32,
        dst: &mut [u16],
        include_background: bool,
    ) -> bool {
        debug_assert_eq!(dst.len(), TILE_CHANNELS);
        let sel = TileSel::new(level, tx, ty);

        // 初始 dst：背景可见且合成结果不透明 → 背景 tile 且 dst_has_alpha=false；
        // 否则全透明。**整块 tile 的 dst_has_alpha 是常量**（源码
        // `tree.py:461` 的 `get_render_is_opaque`），不能合成一层后就改成 true。
        // 导出/合并等标准化渲染不看预览（`tree.py:653-669`）。
        let opaque = include_background && self.render_is_opaque_with_previewing(false);
        let dst_has_alpha = !opaque;
        let mut has_content = false;
        if include_background && self.background_visible {
            let bg = self.background_tile_at(sel);
            dst.copy_from_slice(bg.as_flattened().as_flattened());
            has_content = true;
        } else {
            dst.fill(0);
        }

        // 子层自底向上（组的隔离/直通在 composite_layer 里递归处理）
        let mut pool: Vec<Vec<u16>> = Vec::new();
        has_content |= composite_layers(&self.layers, sel, dst, dst_has_alpha, 0, &mut pool);
        has_content
    }

    /// 只合成 `path` 指向的**单个图层/组**（填充源用，
    /// `tree.py:640-651` 的 `get_tile_accessible_layer_rendering`）。
    ///
    /// 背景不参与（只有根栈才有背景，而这里 `path` 恒非空）。组按它自己的
    /// 模式/不透明度隔离合成；普通层按自己的模式/不透明度合进透明背板。
    /// 记账：祖先组的可见性/不透明度未参与（源码 solo 渲染会带祖先 PUSH/POP）。
    pub fn composite_layer_tile(&self, path: &[usize], tx: i32, ty: i32, dst: &mut [u16]) -> bool {
        self.composite_layer_tile_at(0, path, tx, ty, dst)
    }

    /// 同 [`Self::composite_layer_tile`]，但按 mipmap 级别取样。
    pub fn composite_layer_tile_at(
        &self,
        level: u32,
        path: &[usize],
        tx: i32,
        ty: i32,
        dst: &mut [u16],
    ) -> bool {
        debug_assert_eq!(dst.len(), TILE_CHANNELS);
        dst.fill(0);
        let Some(layer) = self.layer_at(path) else {
            return false;
        };
        let mut pool: Vec<Vec<u16>> = Vec::new();
        composite_layer(
            layer,
            TileSel::new(level, tx, ty),
            dst,
            true,
            false,
            0,
            &mut pool,
        )
    }

    /// 只渲染 `path` 指向的图层/组为 RGBA8（`render_layer_as_pixbuf`，
    /// `gui/document.py:927-931` 的 CopyLayer 用；alpha 保留）。
    ///
    /// `rect` 传图层自己的 `data_bbox()`（源码 `current.get_bbox()` 的
    /// tile 对齐版）。输出包含本层自己的 mode/opacity，背景不参与。
    pub fn render_layer_rgba8(&self, path: &[usize], rect: (i32, i32, i32, i32)) -> Vec<u8> {
        let (rx, ry, rw, rh) = rect;
        if rw <= 0 || rh <= 0 {
            return Vec::new();
        }
        let Some(layer) = self.layer_at(path) else {
            return Vec::new();
        };
        let ts = TILE_SIZE as i32;
        let mut out = vec![0u8; (rw as usize) * (rh as usize) * 4];
        let mut scratch = vec![0u16; TILE_CHANNELS];
        let mut last: Option<(i32, i32, bool)> = None;
        for y in 0..rh {
            let py = ry + y;
            let ty = py.div_euclid(ts);
            let iy = py.rem_euclid(ts) as usize;
            for x in 0..rw {
                let px = rx + x;
                let tx = px.div_euclid(ts);
                let has_content = match last {
                    Some((lx, ly, ok)) if (lx, ly) == (tx, ty) => ok,
                    _ => {
                        let ok = layer.compose_tile_into(tx, ty, &mut scratch);
                        last = Some((tx, ty, ok));
                        ok
                    }
                };
                if !has_content {
                    continue;
                }
                let ix = px.rem_euclid(ts) as usize;
                let p: [u16; 4] = scratch[(iy * TILE_SIZE + ix) * 4..][..4]
                    .try_into()
                    .unwrap();
                let o = ((y as usize) * (rw as usize) + x as usize) * 4;
                out[o..o + 4].copy_from_slice(&color::tile_to_rgba8_alpha_single(p));
            }
        }
        out
    }

    /// 合成 `(tx, ty)` 处的 tile 并编码为显示用 RGBA8（**不透明**，GPU 上传用）。
    ///
    /// 背景参与合成（`tree.py` 的背景 BLIT 语义）；背景隐藏时内容直接落在
    /// 纸色上——与现渲染管线的"CPU 预合成、无混合"模型一致。坐标无内容时
    /// 返回 `None`（调用方据此从显存剔除）。
    pub fn composite_tile_rgba8(&self, tx: i32, ty: i32) -> Option<Box<[u8; TILE_RGBA8_BYTES]>> {
        self.composite_tile_rgba8_at(0, tx, ty)
    }

    /// 同 [`Self::composite_tile_rgba8`]，但按 mipmap 级别合成（远景上传用）。
    ///
    /// `tx/ty` 是该级别下的 tile 索引；`level == 0` 与旧路径逐字节一致
    /// （连"先 cheap 剔除、再合成"的顺序都保持不变）。
    ///
    /// 级别 > 0 时**不做**"空块返回 None"的剔除：背景参与合成后每一块都有
    /// 内容（`render_is_opaque()` 本身就蕴含 `background_visible`），远景
    /// 仍然需要那张底色；真正该跳过的是视口外的坐标，那由 M3 的屏幕 tile
    /// 循环按可见范围裁剪，不需要在这里猜。
    pub fn composite_tile_rgba8_at(
        &self,
        level: u32,
        tx: i32,
        ty: i32,
    ) -> Option<Box<[u8; TILE_RGBA8_BYTES]>> {
        if level.min(MAX_MIPMAP_LEVEL) == 0 {
            // 预览时即使当前层是空的也要输出；背景不可见时也要输出透明 tile
            // （否则旧的不透明 tile 会留在显存里，棋盘格只在部分区域出现）。
            if self.preview.is_none() && self.render_is_opaque() && !self.has_content_at(tx, ty) {
                return None;
            }
        }
        let mut scratch = vec![0u16; TILE_CHANNELS];
        self.composite_tile_at(level, tx, ty, &mut scratch);
        // 背景不可见/透明性无法保证时保留 alpha，交给 GPU 与棋盘格合成；
        // 否则仍预合成到底色（不透明，走最快路径）。
        let keep_alpha = !self.render_is_opaque();
        let bg = self.background_color;
        let bg_lin = color::bg_to_linear(bg);
        let mut out = Box::new([0u8; TILE_RGBA8_BYTES]);
        for i in 0..TILE_SIZE * TILE_SIZE {
            let px: [u16; 4] = scratch[i * 4..i * 4 + 4].try_into().unwrap();
            if keep_alpha {
                out[i * 4..i * 4 + 4].copy_from_slice(&color::fix15_pixel_rgba8_premultiplied(px));
            } else {
                out[i * 4..i * 4 + 4].copy_from_slice(&color::tile_to_rgba8_single(px, bg, bg_lin));
            }
        }
        Some(out)
    }

    /// 合成矩形区域为 RGBA8 显示像素（导出/预览用），按 tile 整块合成。
    ///
    /// `dst` 长度 = `w*h*4`；`with_alpha=false` 时输出不透明 RGB
    /// （alpha 写 255，合成保证不透明）。
    pub fn render_rect_rgba8(&self, rect: (i32, i32, u32, u32), with_alpha: bool, dst: &mut [u8]) {
        let (rx, ry, rw, rh) = rect;
        debug_assert_eq!(dst.len(), (rw * rh * 4) as usize);
        let ts = TILE_SIZE as i32;
        let bg = color::bg_to_linear(self.background_color);
        let mut scratch: Vec<u16> = vec![0; TILE_CHANNELS];
        // 行进方向缓存：同一 tile 只合成一次（导出 3456×3008 这种大画幅时，
        // 每像素重合成整 tile 是 64×64 倍的无谓开销）。
        let mut last: (i32, i32) = (i32::MIN, i32::MIN);
        let mut last_composed = false;
        for y in 0..rh as i32 {
            let py = ry + y;
            let row_ty = py.div_euclid(ts);
            let in_y = py.rem_euclid(ts) as usize;
            for x in 0..rw as i32 {
                let px = rx + x;
                let tx = px.div_euclid(ts);
                if (tx, row_ty) != last {
                    last = (tx, row_ty);
                    // `render_background = not alpha`（`document.py:1677`）
                    last_composed =
                        self.composite_tile_with_background(tx, row_ty, &mut scratch, !with_alpha);
                }
                let composed = last_composed;
                let in_x = px.rem_euclid(ts) as usize;
                let src = &scratch[(in_y * TILE_SIZE + in_x) * 4..][..4];
                let mut p4: [u16; 4] = src.try_into().unwrap();
                if !composed {
                    p4 = [0; 4];
                }
                let o = (y as usize * rw as usize + x as usize) * 4;
                let out = if with_alpha {
                    color::tile_to_rgba8_alpha_single(p4)
                } else {
                    color::tile_to_rgba8_single(p4, self.background_color, bg)
                };
                dst[o..o + 4].copy_from_slice(&out);
            }
        }
    }

    /// **栈**缩略图渲染（`root_stack.render_thumbnail` 的 fix15 版）：
    /// 合成（含背景）后按 `level` 级固定点减半，输出不透明 RGBA8。
    ///
    /// 与 [`LayerStack::render_rect_rgba8`] 同一套合成语义（背景 + 各层模式），
    /// 差别只在于**在 fix15 预乘域做 mipmap 平均**（MyPaint 的 mipmap 就在
    /// 15 位域），最后一步才转 8 位。
    ///
    /// 调用方是 `pigment-engine` 的预览/autosave（`fileio.rs:469,506`），
    /// **不在逐帧路径上**：画布远景已改为屏幕 tile 循环（走
    /// [`LayerStack::composite_tile_rgba8_at`] + tile mipmap），所以这里不再
    /// 需要 content-tile 缓存 —— 原来的 `ThumbTileCache` 已随远景路径一起删除。
    ///
    /// # 为什么不能逐输出像素调用 `render_halved`
    ///
    /// `render_halved` 对每个输出像素采样 `2^level × 2^level` 个源像素，
    /// 而源像素闭包每次都要判 tile/合成。远景 4k×4k 文档、level 4 时是
    /// 6800 万次闭包调用，实测单次缩略图重建接近 **1 秒**，会把 UI 卡死。
    ///
    /// 这里改成「按 content tile 合成一次 + 每 tile 只做一遍块平均」：
    /// - 只有真正有 tile 的坐标才进合成；
    /// - 块内平均复用 [`crate::thumb::average_tile_block`]，不逐像素查 hash；
    /// - 空块直接填背景。
    ///
    /// 合成结果仍与旧路径逐块一致（块不会跨 64px tile，因为
    /// 2^level ∈ {1,2,4,8,16} 均整除 64）。
    pub fn render_thumbnail_rgba8(
        &self,
        rect: (i32, i32, i32, i32),
        level: u32,
    ) -> (Vec<u8>, u32, u32) {
        let (x, y, w, h) = rect;
        if w <= 0 || h <= 0 {
            return (Vec::new(), 0, 0);
        }
        let tw = (w >> level).max(1) as usize;
        let th = (h >> level).max(1) as usize;
        let bg = self.background_color;
        let block = 1i32 << level;
        let mut out = vec![0u8; tw * th * 4];

        // 1) 背景铺底：图案档用**本级**的盒式平均图（与画布远景同一条链），
        //    纯色/背景隐藏时用底色或透明。
        if let Some(img) = self
            .background_image
            .as_ref()
            .filter(|_| self.background_visible)
        {
            let lv = img.level_image(level.min(MAX_MIPMAP_LEVEL));
            for oy in 0..th {
                let gy = (y >> level) + oy as i32;
                for ox in 0..tw {
                    let gx = (x >> level) + ox as i32;
                    let p = lv.pixel(gx as i64, gy as i64);
                    let o = (oy * tw + ox) * 4;
                    out[o..o + 4].copy_from_slice(&crate::thumb::encode_opaque(p, bg));
                }
            }
        } else {
            let empty = crate::thumb::encode_opaque([0; 4], bg);
            for px in out.chunks_exact_mut(4) {
                px.copy_from_slice(&empty);
            }
        }

        // 2) 只处理真正有 tile 的坐标；没有内容的块已经填好背景。
        let mut coords: Vec<(i32, i32)> = self
            .walk()
            .into_iter()
            .filter_map(|(_, l)| l.surface())
            .flat_map(|s| s.tiles().map(|(c, _)| c))
            .collect();
        coords.sort_unstable();
        coords.dedup();
        if coords.is_empty() {
            return (out, tw as u32, th as u32);
        }

        let bs = (TILE_SIZE as i32 >> level).max(1) as usize;
        let mut tile = vec![0u16; TILE_CHANNELS];
        for (tx, ty) in coords {
            // 缩略图矩形之外的内容 tile 全部跳过。
            let tile_x0 = tx * TILE_SIZE as i32;
            let tile_y0 = ty * TILE_SIZE as i32;
            let ox0 = (tile_x0 >> level) - (x >> level);
            let oy0 = (tile_y0 >> level) - (y >> level);
            if ox0 >= tw as i32 || oy0 >= th as i32 || ox0 + bs as i32 <= 0 || oy0 + bs as i32 <= 0
            {
                continue;
            }
            // 完整背景合成：Multiply/Screen 等模式必须和背景一起算，减去背景
            // 再叠均色会变色。
            self.composite_tile(tx, ty, &mut tile);
            let mut blocks = vec![0u8; bs * bs * 4];
            for by in 0..bs {
                for bx in 0..bs {
                    let p = crate::thumb::average_tile_block(
                        &tile,
                        bx * block as usize,
                        by * block as usize,
                        level,
                    );
                    let o = (by * bs + bx) * 4;
                    blocks[o..o + 4].copy_from_slice(&crate::thumb::encode_opaque(p, bg));
                }
            }
            for by in 0..bs {
                let oy = oy0 + by as i32;
                if oy < 0 || oy >= th as i32 {
                    continue;
                }
                for bx in 0..bs {
                    let ox = ox0 + bx as i32;
                    if ox < 0 || ox >= tw as i32 {
                        continue;
                    }
                    let o = (oy as usize * tw + ox as usize) * 4;
                    let s = (by * bs + bx) * 4;
                    out[o..o + 4].copy_from_slice(&blocks[s..s + 4]);
                }
            }
        }
        (out, tw as u32, th as u32)
    }
}

/// 若干个 `(x, y, w, h)` 的并集（忽略空矩形；全空返回全 0）。
fn union_bbox(rects: impl Iterator<Item = (i32, i32, i32, i32)>) -> (i32, i32, i32, i32) {
    let mut acc: Option<(i32, i32, i32, i32)> = None;
    for b in rects {
        if b.2 <= 0 || b.3 <= 0 {
            continue;
        }
        acc = Some(match acc {
            None => b,
            Some((x0, y0, w, h)) => {
                let nx = x0.min(b.0);
                let ny = y0.min(b.1);
                let nx1 = (x0 + w).max(b.0 + b.2);
                let ny1 = (y0 + h).max(b.1 + b.3);
                (nx, ny, nx1 - nx, ny1 - ny)
            }
        });
    }
    acc.unwrap_or((0, 0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hokusai::TiledSurface;

    /// 造一块纯色 tile（8 位 → fix15，同载入路径）。
    fn solid_surface(rgb: [u8; 3], a: u8) -> PigmentSurface {
        let mut s = PigmentSurface::new();
        let w = TILE_SIZE as u32;
        let mut px = Vec::with_capacity((w * w * 4) as usize);
        for _ in 0..w * w {
            px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
        s.load_from_rgba8(&px, 0, 0, w, w);
        s
    }

    #[test]
    fn solo_renders_only_the_current_branch_without_background() {
        let mut top = Layer::with_surface(solid_surface([255, 0, 0], 255));
        top.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut bottom = Layer::with_surface(solid_surface([0, 0, 255], 255));
        bottom.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut st = LayerStack::from_children(vec![top, bottom]);
        st.set_current_path(&[1]);
        let pixel = |st: &LayerStack| {
            let mut buf = vec![0u16; TILE_CHANNELS];
            assert!(st.composite_tile(0, 0, &mut buf));
            color::tile_to_rgba8_alpha_single(buf[..4].try_into().unwrap())
        };

        // 正常合成：顶层红
        assert_eq!(pixel(&st), [255, 0, 0, 255]);
        // Solo 当前层（底层蓝）：背景和无关顶层都不参与
        st.set_current_layer_solo(true);
        assert_eq!(pixel(&st), [0, 0, 255, 255]);
        // 当前层自身 visible=false 时，Solo 仍强制可见（data.py:353-358）
        st.layers[1].visible = false;
        assert_eq!(pixel(&st), [0, 0, 255, 255]);
        assert!(!st.render_is_opaque(), "Solo 恒有透明区域");
    }

    #[test]
    fn solo_of_a_group_keeps_its_visible_children_only() {
        let mut child_red = Layer::with_surface(solid_surface([255, 0, 0], 255));
        child_red.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut child_hidden = Layer::with_surface(solid_surface([0, 255, 0], 255));
        child_hidden.set_mode(LayerMode::Standard(CombineMode::Normal));
        child_hidden.visible = false;
        let mut group = Layer::new_group();
        group
            .stack_mut()
            .unwrap()
            .layers
            .extend([child_red, child_hidden]);
        let mut top_green = Layer::with_surface(solid_surface([0, 255, 0], 255));
        top_green.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut st = LayerStack::from_children(vec![top_green, group]);
        st.set_current_path(&[1]);
        st.set_current_layer_solo(true);
        let mut buf = vec![0u16; TILE_CHANNELS];
        assert!(st.composite_tile(0, 0, &mut buf));
        let px = color::tile_to_rgba8_alpha_single(buf[..4].try_into().unwrap());
        assert_eq!(px, [255, 0, 0, 255], "只保留组内 visible 子层");
    }

    #[test]
    fn render_layer_rgba8_keeps_alpha_and_layer_opacity() {
        let mut l = Layer::with_surface(solid_surface([255, 0, 0], 255));
        l.set_mode(LayerMode::Standard(CombineMode::Normal));
        l.opacity = 0.5;
        let st = LayerStack::from_children(vec![l]);
        let bbox = st.current().data_bbox();
        assert_eq!(bbox, (0, 0, 64, 64));
        let out = st.render_layer_rgba8(&[0], bbox);
        assert_eq!(out.len(), 64 * 64 * 4);
        // 0.5 不透明度的红层合到透明底：RGB 预乘，A≈128
        assert_eq!(out[0], 255);
        assert_eq!(out[1], 0);
        assert_eq!(out[3], 128, "opacity=0.5 应保留 alpha，而不是合成背景");
    }

    #[test]
    fn normalize_painting_layer_preserves_appearance() {
        let mut bottom = Layer::with_surface(solid_surface([0, 0, 255], 255));
        bottom.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut top = Layer::with_surface(solid_surface([255, 0, 0], 255));
        top.set_mode(LayerMode::Standard(CombineMode::Normal));
        top.opacity = 0.5;
        let mut st = LayerStack::from_children(vec![top, bottom]);
        st.set_current_path(&[0]);

        let mut before = vec![0u16; TILE_CHANNELS];
        st.composite_tile_with_background(0, 0, &mut before, false);
        let norm = st.layer_new_normalized(&[0]).expect("应可归一化");
        assert_eq!(norm.mode, LayerMode::Standard(CombineMode::Normal));
        assert_eq!(norm.opacity, 1.0);
        assert_eq!(norm.name, st.current().name);

        *st.layer_at_mut(&[0]).unwrap() = norm;
        let mut after = vec![0u16; TILE_CHANNELS];
        st.composite_tile_with_background(0, 0, &mut after, false);
        assert_eq!(before, after, "归一化层叠回同一背板应还原外观");
    }

    #[test]
    fn normalize_isolated_group_preserves_appearance() {
        let mut child = Layer::with_surface(solid_surface([255, 0, 0], 255));
        child.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut group = Layer::new_group();
        group.set_mode(LayerMode::Standard(CombineMode::Normal));
        group.stack_mut().unwrap().layers.push(child);
        let mut bottom = Layer::with_surface(solid_surface([0, 0, 255], 255));
        bottom.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut st = LayerStack::from_children(vec![group, bottom]);

        let mut before = vec![0u16; TILE_CHANNELS];
        st.composite_tile_with_background(0, 0, &mut before, false);
        let norm = st.layer_new_normalized(&[0]).expect("组应可归一化");
        assert_eq!(norm.mode, LayerMode::Standard(CombineMode::Normal));
        *st.layer_at_mut(&[0]).unwrap() = norm;
        let mut after = vec![0u16; TILE_CHANNELS];
        st.composite_tile_with_background(0, 0, &mut after, false);
        assert_eq!(before, after, "隔离组归一化后外观应一致");
    }

    #[test]
    fn merge_visible_preserves_appearance_over_the_same_background() {
        let mut bottom = Layer::with_surface(solid_surface([0, 0, 255], 255));
        bottom.set_mode(LayerMode::Standard(CombineMode::Normal));
        bottom.name = Some("bottom".into());
        let mut top = Layer::with_surface(solid_surface([255, 0, 0], 255));
        top.set_mode(LayerMode::Standard(CombineMode::Normal));
        top.opacity = 0.5;
        top.name = Some("top".into());
        let st = LayerStack::from_children(vec![top, bottom]);
        // 默认背景可见（白纸），走 flat2rgba 减法路径
        let mut before = vec![0u16; TILE_CHANNELS];
        st.composite_tile_with_background(0, 0, &mut before, true);

        let merged = st.layer_new_merge_visible();
        assert_eq!(merged.mode, LayerMode::Standard(CombineMode::Normal));
        assert_eq!(merged.opacity, 1.0);
        assert_eq!(merged.name.as_deref(), Some("top, bottom"));
        let merged_stack = LayerStack::from_children(vec![merged]);
        let mut after = vec![0u16; TILE_CHANNELS];
        merged_stack.composite_tile_with_background(0, 0, &mut after, true);
        assert_eq!(before, after, "合并可见层叠回同一背景应还原外观");
    }

    #[test]
    fn uniq_removes_unchanged_tiles_and_pixels() {
        let mut bottom = Layer::with_surface(solid_surface([0, 0, 255], 255));
        bottom.set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut blue = vec![0u8; 64 * 64 * 4];
        for px in blue.chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 0, 255, 255]);
        }
        let mut top = Layer::with_surface(PigmentSurface::new());
        top.set_mode(LayerMode::Standard(CombineMode::Normal));
        top.surface_mut()
            .unwrap()
            .load_from_rgba8(&blue, 0, 0, 64, 64);
        let st = LayerStack::from_children(vec![top.clone(), bottom.clone()]);
        let unique = st.uniq_layer(&[0], false).expect("可 uniq");
        assert_eq!(
            unique.surface().unwrap().tile_count(),
            0,
            "整块与背板相同 → 删 tile"
        );

        // 一个像素改成红色，整块保留；pixels=true 时仅同色像素清零
        let mut mixed = blue;
        mixed[..4].copy_from_slice(&[255, 0, 0, 255]);
        let mut top = Layer::with_surface(PigmentSurface::new());
        top.set_mode(LayerMode::Standard(CombineMode::Normal));
        top.surface_mut()
            .unwrap()
            .load_from_rgba8(&mixed, 0, 0, 64, 64);
        let st = LayerStack::from_children(vec![top, bottom]);
        let tiles = st.uniq_layer(&[0], false).expect("可 uniq");
        assert_eq!(tiles.surface().unwrap().tile_count(), 1);
        assert_eq!(
            tiles.surface().unwrap().tile(0, 0).unwrap()[0][0][3],
            1 << 15
        );

        let pixels = st.uniq_layer(&[0], true).expect("可 uniq");
        let t = pixels.surface().unwrap().tile(0, 0).unwrap();
        assert_eq!(t[0][0], [1 << 15, 0, 0, 1 << 15], "不同色像素保留");
        assert_eq!(t[0][1], [0; 4], "同色像素清零");
    }

    #[test]
    fn refactor_group_extracts_common_pixels_and_tiles() {
        let mut a = vec![0u8; 64 * 64 * 4];
        let mut b = vec![0u8; 64 * 64 * 4];
        for px in a.chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 0, 255, 255]);
        }
        for px in b.chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 255, 0, 255]);
        }
        a[..4].copy_from_slice(&[255, 0, 0, 255]);
        b[..4].copy_from_slice(&[255, 0, 0, 255]);
        let mut child_a = Layer::with_surface(PigmentSurface::new());
        child_a.set_mode(LayerMode::Standard(CombineMode::Normal));
        child_a
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&a, 0, 0, 64, 64);
        let mut child_b = Layer::with_surface(PigmentSurface::new());
        child_b.set_mode(LayerMode::Standard(CombineMode::Normal));
        child_b
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&b, 0, 0, 64, 64);
        let mut group = Layer::new_group();
        group.set_mode(LayerMode::Standard(CombineMode::Normal));
        group.stack_mut().unwrap().layers.extend([child_a, child_b]);
        let st = LayerStack::from_children(vec![group]);

        let out = st
            .refactor_layer_group(&[0], true)
            .expect("隔离组应可 refactor");
        let stack = out.stack().unwrap();
        assert_eq!(stack.len(), 3, "两个子层 + Common");
        let common = stack.layers.last().unwrap();
        assert_eq!(common.name.as_deref(), Some("Common"));
        let ct = common.surface().unwrap().tile(0, 0).unwrap();
        assert_eq!(ct[0][0], [1 << 15, 0, 0, 1 << 15], "公共像素进 Common");
        for child in &stack.layers[0..2] {
            let t = child.surface().unwrap().tile(0, 0).unwrap();
            assert_eq!(t[0][0], [0; 4], "公共像素从子层删除");
            assert_ne!(t[0][1], [0; 4], "非公共像素保留");
        }

        // pixels=false：部分公共的 tile 不抽取
        let group = out.clone();
        let st2 = LayerStack::from_children(vec![group]);
        let out2 = st2
            .refactor_layer_group(&[0], false)
            .expect("隔离组应可 refactor");
        let stack2 = out2.stack().unwrap();
        assert_eq!(stack2.len(), 4, "已有 Common 上再插一个空 Common");
        assert_eq!(
            stack2
                .layers
                .last()
                .unwrap()
                .surface()
                .unwrap()
                .tile_count(),
            0,
            "pixels=false 不抽部分 tile"
        );
    }

    #[test]
    fn defaults_match_layer_base() {
        // core.py:84-92
        let l = Layer::new();
        assert_eq!(l.opacity, 1.0);
        assert!(l.visible);
        assert!(!l.locked);
        assert_eq!(l.mode, LayerMode::DEFAULT);
        assert_eq!(l.effective_opacity(), 1.0);
        // group.py:370-376：不可见 → 0
        let mut l2 = Layer::new();
        l2.visible = false;
        assert_eq!(l2.effective_opacity(), 0.0);
    }

    /// 图层缩略图：减半在 fix15 预乘域做，且**每级每通道各自整除再相加**
    /// （`pixops.cpp:39` 的 `v0/4 + v1/4 + v2/4 + v3/4`）。
    #[test]
    fn layer_thumbnail_halves_in_fix15_space() {
        let mut l = Layer::new();
        l.set_mode(LayerMode::Standard(CombineMode::Normal)); // 直接透传，便于逐位比对
                                                              // 2×2 全不透明，R 通道取 1/2/3/4（fix15 直接写，跳过 8 位量化）
        let vals: [[u16; 4]; 4] = [
            [1, 0, 0, 1 << 15],
            [2, 0, 0, 1 << 15],
            [3, 0, 0, 1 << 15],
            [4, 0, 0, 1 << 15],
        ];
        {
            let t = l.surface_mut().unwrap().tile_request_start(0, 0);
            t[0][0] = vals[0];
            t[0][1] = vals[1];
            t[1][0] = vals[2];
            t[1][1] = vals[3];
            l.surface_mut().unwrap().tile_request_end(0, 0);
        }
        // 64×64 的 bbox 取 level 1 → 32×32
        let (px, w, h) = l.render_thumbnail_rgba8((0, 0, 64, 64), 1);
        assert_eq!((w, h), (32, 32));

        // 逐位期望：各自 /4 后相加（1/4 + 2/4 + 3/4 + 4/4 = 0+0+0+1 = 1），
        // 不是 (1+2+3+4)/4 = 2
        let mut avg = [0u16; 4];
        for c in 0..4 {
            avg[c] = vals[0][c] / 4 + vals[1][c] / 4 + vals[2][c] / 4 + vals[3][c] / 4;
        }
        let expected = color::tile_to_rgba8_alpha_single(avg);
        assert_eq!(avg[0], 1, "先各自整除再相加");
        assert_eq!(&px[0..4], &expected[..]);
        // 块外（1,1 那个 2×2 只占左上角）应为全透明
        assert_eq!(&px[4..8], &[0, 0, 0, 0]);
    }

    /// 块边界按 **mipmap 网格**对齐（源码先 `x //= 2` 再交给 mipmap 层）。
    #[test]
    fn layer_thumbnail_blocks_align_to_the_mipmap_grid() {
        let mut l = Layer::new();
        l.set_mode(LayerMode::Standard(CombineMode::Normal));
        {
            let t = l.surface_mut().unwrap().tile_request_start(0, 0);
            t[0][0] = [1 << 15, 1 << 15, 1 << 15, 1 << 15];
            l.surface_mut().unwrap().tile_request_end(0, 0);
        }
        let (px, w, h) = l.render_thumbnail_rgba8((0, 0, 64, 64), 1);
        assert_eq!((w, h), (32, 32));
        // (0,0) 的 2×2 块含 1 个不透明像素 → 平均 alpha = 32768/4 = 8192
        assert_eq!(px[3], (8192.0f32 / 32768.0 * 255.0).round() as u8);
        assert_eq!(px[0], 255, "去预乘后仍是满强度白");
        assert_eq!(&px[4..8], &[0, 0, 0, 0], "相邻块全空");
    }

    /// 缩略图走**本层自己的 op**：不透明度会体现在缩略图里
    /// （`data.py:376-383` 的 `COMPOSITE(surface, mode, opacity)`）。
    #[test]
    fn layer_thumbnail_applies_own_opacity() {
        let mut l = Layer::new();
        l.set_mode(LayerMode::Standard(CombineMode::Normal));
        l.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));
        let (full, _, _) = l.render_thumbnail_rgba8((0, 0, 64, 64), 0);
        assert_eq!(&full[0..4], &[255, 0, 0, 255]);

        l.opacity = 0.5;
        let (half, _, _) = l.render_thumbnail_rgba8((0, 0, 64, 64), 0);
        assert!(
            half[3] < 250 && half[3] > 100,
            "alpha 落到约一半：{}",
            half[3]
        );
    }

    #[test]
    fn index_zero_is_the_topmost_layer() {
        // group.py:44-48：_layers[0] 是最顶层
        let mut st = LayerStack::new();
        st.layer_mut(0)
            .set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut red = Layer::new();
        red.set_mode(LayerMode::Standard(CombineMode::Normal));
        red.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));
        st.insert(0, red);
        // index 0 = red（最顶），index 1 = 底下的空层
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        assert_eq!([px[0], px[1], px[2]], [255, 0, 0], "顶层红色应盖住背景");
    }

    #[test]
    fn invisible_layer_shows_what_is_below() {
        let mut st = LayerStack::new();
        st.layer_mut(0)
            .set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut blue = Layer::new();
        blue.set_mode(LayerMode::Standard(CombineMode::Normal));
        blue.content = LayerContent::Painting(solid_surface([0, 0, 255], 255));
        st.insert(0, blue);
        st.layer_mut(0).visible = false;
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        // 背景是白色（tree.py:55）
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        assert_eq!([px[0], px[1], px[2]], [255, 255, 255]);
    }

    #[test]
    fn half_opacity_layer_blends_over_background() {
        let mut st = LayerStack::new();
        st.layer_mut(0)
            .set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut red = Layer::new();
        red.set_mode(LayerMode::Standard(CombineMode::Normal));
        red.opacity = 0.5;
        red.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));
        st.insert(0, red);
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        // 线性域 50% 红 + 50% 白，EOTF 2.2 编码后应在两者之间且偏亮
        assert!(px[0] > 200, "r={}", px[0]);
        assert!(px[1] < 255 && px[1] > 100, "g={}", px[1]);
        assert_eq!(px[1], px[2]);
        assert!(px[3] > 240);
    }

    #[test]
    fn hidden_background_yields_transparency() {
        let mut st = LayerStack::new();
        st.layer_mut(0)
            .set_mode(LayerMode::Standard(CombineMode::Normal));
        st.set_background_visible(false);
        let mut dst = vec![0u16; TILE_CHANNELS];
        assert!(!st.composite_tile(0, 0, &mut dst), "无内容应返回 false");
        assert!(dst.iter().all(|&v| v == 0));
        // 有内容的层仍应出现
        let mut red = Layer::new();
        red.set_mode(LayerMode::Standard(CombineMode::Normal));
        red.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));
        st.insert(0, red);
        let mut dst2 = vec![0u16; TILE_CHANNELS];
        assert!(st.composite_tile(0, 0, &mut dst2));
        assert!(dst2.iter().any(|&v| v != 0));
    }

    #[test]
    fn insert_and_remove_keep_current_sane() {
        let mut st = LayerStack::new(); // [空], current=0
        let l1 = Layer::new();
        assert_eq!(st.insert(0, l1), 0); // [new, 空], current=1
        assert_eq!(st.current_index(), 1);
        st.remove(1); // [new], current=0
        assert_eq!(st.current_index(), 0);
        assert_eq!(st.len(), 1);
        // 照源码语义：允许删空（补层由 RemoveLayer 命令负责）
        st.remove(0);
        assert_eq!(st.len(), 0);
    }

    #[test]
    fn render_rect_rgba8_matches_tile_composition() {
        let mut st = LayerStack::new();
        st.layer_mut(0)
            .set_mode(LayerMode::Standard(CombineMode::Normal));
        let mut red = Layer::new();
        red.set_mode(LayerMode::Standard(CombineMode::Normal));
        red.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));
        st.insert(0, red);
        let w = TILE_SIZE as u32;
        let mut buf = vec![0u8; (w * w * 4) as usize];
        st.render_rect_rgba8((0, 0, w, w), true, &mut buf);
        assert_eq!(&buf[0..4], &[255, 0, 0, 255]);
        // 越出内容区一格：alpha=true 不画背景（`render_background=not alpha`）
        // → 全透明；alpha=false 才落背景白。
        let mut buf2 = vec![0u8; 4];
        st.render_rect_rgba8((TILE_SIZE as i32 * 5, 0, 1, 1), true, &mut buf2);
        assert_eq!(&buf2, &[0, 0, 0, 0]);
        st.render_rect_rgba8((TILE_SIZE as i32 * 5, 0, 1, 1), false, &mut buf2);
        assert_eq!(&buf2, &[255, 255, 255, 255]);
    }

    // ---- B2-6 组（`group.py`）----

    /// 组默认值：空子栈、默认模式（隔离的 SpectralWGM）、`is_group` 为真。
    #[test]
    fn group_defaults_match_layer_stack() {
        let g = Layer::new_group();
        assert!(g.is_group());
        assert_eq!(g.child_count(), 0, "组初始无子层（group.py:78）");
        assert_eq!(g.mode, LayerMode::DEFAULT);
        assert_eq!(g.opacity, 1.0);
        assert!(g.surface().is_none(), "组没有表面");
        assert!(
            !g.paintable_self(),
            "组不可画（LayerBase.get_paintable=False）"
        );
        // 组也能当 False 用？源码特意保证「len=0 的组永不为假」——这里用类型保证
        assert!(Layer::new().surface().is_some());
    }

    /// 直通组不隔离：子层直接作用于父背板，组自身 mode/opacity 不参与；
    /// 隔离组 PUSH/POP：子层先进透明缓冲，再用组的 opacity 合上来。
    #[test]
    fn isolated_vs_pass_through_group_compositing() {
        // 子层：不透明红
        let mut child = Layer::new();
        child.set_mode(LayerMode::Standard(CombineMode::Normal));
        child.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));

        // 隔离组（默认模式）+ 组不透明度 0.5
        let mut g = Layer::new_group();
        g.set_mode(LayerMode::Standard(CombineMode::Normal));
        g.stack_mut().unwrap().layers.push(child.clone());
        assert!(g.set_opacity(0.5));
        let mut st = LayerStack::from_children(vec![g.clone()]);
        st.set_background_visible(false);
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        assert_eq!([px[0], px[1], px[2]], [255, 0, 0], "色相不变");
        assert!(
            (px[3] as i32 - 128).abs() <= 3,
            "组不透明度 0.5 应体现在 alpha：{}",
            px[3]
        );

        // 直通组：组 opacity 被强制 1.0，子层直接作用于背板 → alpha 满
        let mut g2 = Layer::new_group();
        g2.stack_mut().unwrap().layers.push(child);
        let (mode_changed, opacity_changed) = g2.set_mode(LayerMode::PassThrough);
        assert!(mode_changed);
        assert!(opacity_changed || g2.opacity == 1.0);
        assert!(
            !g2.set_opacity(0.3),
            "直通组的 opacity 不可改（core.py:281）"
        );
        let mut st = LayerStack::from_children(vec![g2]);
        st.set_background_visible(false);
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        assert_eq!(
            [px[0], px[1], px[2], px[3]],
            [255, 0, 0, 255],
            "直通=直接落背板"
        );
    }

    /// 组不可见 → 整棵子树都不出现（`get_render_ops` 对不可见返回 []）。
    #[test]
    fn invisible_group_hides_children() {
        let mut child = Layer::new();
        child.set_mode(LayerMode::Standard(CombineMode::Normal));
        child.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));
        let mut g = Layer::new_group();
        g.set_mode(LayerMode::Standard(CombineMode::Normal));
        g.stack_mut().unwrap().layers.push(child);
        g.visible = false;
        let mut st = LayerStack::from_children(vec![g]);
        // 背景隐藏 → 什么都不该有
        st.set_background_visible(false);
        let mut dst = vec![0u16; TILE_CHANNELS];
        let content = st.composite_tile(0, 0, &mut dst);
        assert!(!content);
        assert!(dst.iter().all(|&v| v == 0), "隐藏组的子层也不能漏出来");
        // branch_visible 对子层为假
        assert!(!st.branch_visible(&[0, 0]));
    }

    /// `_bubble_layer`（`tree.py:1380`）：同栈换位、端点出组、兄弟是组时入组；
    /// 反方向冒泡必须精确还原（`BubbleLayerUp.undo` 依赖这一点）。
    #[test]
    fn bubble_layer_crosses_group_boundaries() {
        let a = Layer::new();
        let mut g = Layer::new_group();
        let b = Layer::new();
        let c = Layer::new();
        let d = Layer::new();
        let gid = g.id;
        g.stack_mut().unwrap().layers = vec![b.clone(), c.clone()];
        let mut st = LayerStack::from_children(vec![a.clone(), g, d.clone()]);

        let ids = |st: &LayerStack| -> Vec<(Vec<usize>, u64)> {
            st.walk().into_iter().map(|(p, l)| (p, l.id)).collect()
        };

        // 初始：root [A, G[B, C], D]
        assert_eq!(
            ids(&st),
            vec![
                (vec![0], a.id),
                (vec![1], gid),
                (vec![1, 0], b.id),
                (vec![1, 1], c.id),
                (vec![2], d.id),
            ]
        );

        // 组内顶层 B 上移 → 出组，插到组前面。
        assert!(st.bubble_layer(&[1, 0], true));
        assert_eq!(
            ids(&st),
            vec![
                (vec![0], a.id),
                (vec![1], b.id),
                (vec![2], gid),
                (vec![2, 0], c.id),
                (vec![3], d.id),
            ]
        );
        assert_eq!(st.current_id(), b.id, "当前层跟着被移动的层");
        // 反方向还原。
        let b_path = st.path_of_id(b.id).unwrap();
        assert!(st.bubble_layer(&b_path, false));
        assert_eq!(st.path_of_id(b.id), Some(vec![1, 0]));
        assert_eq!(st.path_of_id(c.id), Some(vec![1, 1]));

        // 组内底层 C 下移 → 出组，插到组后面。
        assert!(st.bubble_layer(&[1, 1], false));
        assert_eq!(st.path_of_id(c.id), Some(vec![2]));
        assert_eq!(st.path_of_id(b.id), Some(vec![1, 0]));
        let c_path = st.path_of_id(c.id).unwrap();
        assert!(st.bubble_layer(&c_path, true));
        assert_eq!(st.path_of_id(c.id), Some(vec![1, 1]));

        // 根层 A 下移 → 相邻兄弟 G 是组，入组（放到组近端 index 0）。
        assert!(st.bubble_layer(&[0], false));
        assert_eq!(st.path_of_id(a.id), Some(vec![0, 0]));
        assert_eq!(st.path_of_id(b.id), Some(vec![0, 1]));
        assert_eq!(st.path_of_id(c.id), Some(vec![0, 2]));
        // 反方向冒泡：从组近端出去，回到组前。
        let a_path = st.path_of_id(a.id).unwrap();
        assert!(st.bubble_layer(&a_path, true));
        assert_eq!(st.path_of_id(a.id), Some(vec![0]));
        assert_eq!(st.path_of_id(b.id), Some(vec![1, 0]));

        // 端点：根栈顶不能再上、根栈底不能再下；组内两端仍可（会出组）。
        assert!(!st.can_bubble(&[0], true));
        assert!(st.can_bubble(&[0], false));
        assert!(!st.can_bubble(&[3], false));
        assert!(st.can_bubble(&[1, 0], true));
        assert!(st.can_bubble(&[1, 0], false));
        assert!(!st.bubble_layer(&[0], true));
        assert!(!st.bubble_layer(&[3], false));
    }

    /// 树/路径寻址：walk 序 = 组在前、子层紧跟；deepget/deepindex 对得上。
    #[test]
    fn walk_paths_and_current_tracking() {
        let mut a = Layer::new(); // 顶层 0
        a.name = Some("A".into());
        let mut g = Layer::new_group(); // 顶层 1（组）
        g.name = Some("G".into());
        let mut b = Layer::new(); // (1, 0)
        b.name = Some("B".into());
        let mut c = Layer::new(); // (1, 1)
        c.name = Some("C".into());
        g.stack_mut().unwrap().layers.push(b.clone());
        g.stack_mut().unwrap().layers.push(c.clone());
        let mut d = Layer::new(); // 顶层 2
        d.name = Some("D".into());

        let mut st = LayerStack::from_children(vec![a.clone(), g.clone(), d.clone()]);
        let names: Vec<(Vec<usize>, String)> = st
            .walk()
            .into_iter()
            .map(|(p, l)| (p, l.name.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            names,
            vec![
                (vec![0], "A".into()),
                (vec![1], "G".into()),
                (vec![1, 0], "B".into()),
                (vec![1, 1], "C".into()),
                (vec![2], "D".into()),
            ]
        );
        assert_eq!(st.path_of_id(b.id), Some(vec![1, 0]));
        assert_eq!(st.layer_at(&[1, 1]).map(|l| l.id), Some(c.id));
        assert!(st.layer_at(&[0, 0]).is_none(), "非组不能再往里走");
        assert!(st.layer_at(&[9]).is_none());

        // path_above/path_below = walk 序的前后项（tree.py doctest 同款）：
        // 可以跨出/进入组，端点返回 None。
        assert_eq!(st.path_above(&[0]), None);
        assert_eq!(st.path_above(&[1]), Some(vec![0]));
        assert_eq!(st.path_above(&[1, 0]), Some(vec![1]));
        assert_eq!(st.path_above(&[1, 1]), Some(vec![1, 0]));
        assert_eq!(st.path_above(&[2]), Some(vec![1, 1]));
        assert_eq!(st.path_below(&[0]), Some(vec![1]));
        assert_eq!(st.path_below(&[1]), Some(vec![1, 0]));
        assert_eq!(st.path_below(&[1, 1]), Some(vec![2]));
        assert_eq!(st.path_below(&[2]), None);

        // 当前层跟踪：切到 (1,1)，删掉 (1,0) 后仍是同一层（索引前移）
        st.set_current_path(&[1, 1]);
        assert_eq!(st.current_path(), &[1, 1]);
        assert_eq!(st.current_id(), c.id);

        let mut st2 = st.clone();
        st2.remove_at(&[1, 0]).unwrap();
        assert_eq!(st2.current_path(), &[1, 0], "C 前移到索引 0");
        assert_eq!(st2.current_id(), c.id, "当前层不变");
    }

    /// `insert_at` 的钳位与当前层后移语义（`group.py:320`）。
    #[test]
    fn insert_at_clamps_and_shifts_current() {
        let mut st = LayerStack::from_children(vec![Layer::new(), Layer::new()]);
        st.set_current_path(&[1]);
        let new = Layer::new();
        let new_id = new.id;
        let p = st.insert_at(&[0], new).unwrap();
        assert_eq!(p, vec![0]);
        assert_eq!(st.current_path(), &[2], "当前层跟着后移");
        // 越界索引钳位到末尾
        let last = Layer::new();
        let last_id = last.id;
        assert_eq!(st.insert_at(&[999], last).unwrap(), vec![3]);
        assert_eq!(st.path_of_id(last_id), Some(vec![3]));
        assert_eq!(st.len(), 4);
        // 组内插入
        let mut g = Layer::new_group();
        g.name = Some("G".into());
        st.insert_at(&[4], g).unwrap();
        let inner = Layer::new();
        let inner_id = inner.id;
        assert_eq!(st.insert_at(&[4, 0], inner).unwrap(), vec![4, 0]);
        assert_eq!(st.path_of_id(inner_id), Some(vec![4, 0]));
        let _ = new_id;
    }

    /// 模式/不透明度的设置器语义（`core.py:281`/`:473`）。
    #[test]
    fn mode_and_opacity_rules() {
        let mut l = Layer::new();
        // 普通层不接受直通 → 回落默认
        let (changed, _) = l.set_mode(LayerMode::PassThrough);
        assert!(!changed);
        assert_eq!(l.mode, LayerMode::DEFAULT);

        let mut g = Layer::new_group();
        g.opacity = 0.4;
        let (changed, opacity_changed) = g.set_mode(LayerMode::PassThrough);
        assert!(changed && opacity_changed);
        assert_eq!(g.opacity, 1.0, "直通模式强制 opacity=1（core.py:483）");
        assert!(!g.set_opacity(0.2), "直通的 opacity 不可改");
        // 从直通切回标准模式后 opacity 可以改
        g.set_mode(LayerMode::Standard(CombineMode::Normal));
        assert!(g.set_opacity(0.2));
    }

    /// 可画性：组、锁、隐藏分支都不可画（`data.py:1364 get_paintable`）。
    #[test]
    fn paintable_surface_rules() {
        let mut st = LayerStack::new(); // [空绘画层]
        assert!(st.paintable_surface_mut().is_some());

        // 当前层是组 → 不可画
        let mut g = Layer::new_group();
        g.stack_mut().unwrap().layers.push(Layer::new());
        let g_id = g.id;
        st.insert_at(&[0], g).unwrap();
        st.set_current_by_id(g_id);
        assert!(st.paintable_surface_mut().is_none(), "组不可画");

        // 组内的绘画层：组可见且未锁 → 可画
        st.set_current_path(&[0, 0]);
        assert!(st.paintable_surface_mut().is_some());

        // 组锁定 → 子层不可画（branch_locked）
        st.layer_at_mut(&[0]).unwrap().locked = true;
        assert!(st.paintable_surface_mut().is_none());
        st.layer_at_mut(&[0]).unwrap().locked = false;

        // 组隐藏 → 子层不可画（branch_visible）
        st.layer_at_mut(&[0]).unwrap().visible = false;
        assert!(st.paintable_surface_mut().is_none());
    }

    /// 组的 bbox = 子层并集；组缩略图能画出子层内容（solo + 组 op）。
    #[test]
    fn group_bbox_and_thumbnail() {
        let mut child = Layer::new();
        child.set_mode(LayerMode::Standard(CombineMode::Normal));
        child.content = LayerContent::Painting(solid_surface([255, 0, 0], 255));
        let mut g = Layer::new_group();
        g.set_mode(LayerMode::Standard(CombineMode::Normal));
        g.stack_mut().unwrap().layers.push(child);
        assert_eq!(g.data_bbox(), (0, 0, TILE_SIZE as i32, TILE_SIZE as i32));

        let (px, w, h) = g.render_thumbnail_rgba8((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), 0);
        assert_eq!((w, h), (TILE_SIZE as u32, TILE_SIZE as u32));
        assert_eq!(&px[0..4], &[255, 0, 0, 255], "组的缩略图含子层内容");

        // 组不透明度 0.5 → 缩略图 alpha 减半（solo 不重置 opacity）
        g.set_opacity(0.5);
        let (px2, _, _) = g.render_thumbnail_rgba8((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), 0);
        assert!(px2[3] < 200 && px2[3] > 60, "alpha={}", px2[3]);
    }

    /// 图案背景按世界坐标平铺（`tiledsurface.Background` 的循环采样）。
    #[test]
    fn stack_thumbnail_matches_reference_renderer() {
        let mut st = LayerStack::new();
        st.set_background_color([210, 230, 190]);
        let mut px = vec![0u8; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let o = (y * 64 + x) * 4;
                px[o] = (x * 3) as u8;
                px[o + 1] = (y * 5) as u8;
                px[o + 2] = ((x + y) * 2) as u8;
                px[o + 3] = if (x + y) % 7 == 0 { 0 } else { 255 };
            }
        }
        st.current_mut()
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&px, 0, 0, 64, 64);
        let rect = (5, 7, 70, 66);
        for level in 0..=3 {
            let bg = st.background_color;
            let old = {
                let src = tile_source(|(tx, ty), buf| st.composite_tile(tx, ty, buf));
                crate::thumb::render_halved(
                    rect,
                    level,
                    |p| crate::thumb::encode_opaque(p, bg),
                    src,
                )
                .0
            };
            let new = st.render_thumbnail_rgba8(rect, level).0;
            assert_eq!(new, old, "level={level} 必须与逐像素参考路径一致");
        }
    }

    #[test]
    fn render_is_opaque_matches_source_rule() {
        let mut st = LayerStack::new();
        assert!(st.render_is_opaque(), "背景可见且无递减模式 → 不透明");
        let mut top = Layer::new();
        top.set_mode(LayerMode::Standard(CombineMode::DestinationOut));
        st.insert(0, top);
        assert!(
            !st.render_is_opaque(),
            "任一可见 dst-out/in/atop → 允许透明（tree.py:326-330）"
        );
        st.layer_mut(0).visible = false;
        assert!(st.render_is_opaque(), "隐藏的递减层不算");
        st.set_background_visible(false);
        assert!(!st.render_is_opaque(), "背景不参与 → 允许透明");
    }

    #[test]
    fn background_image_tiles_by_world_coordinates() {
        let mut st = LayerStack::new();
        // 2×2 四色图：红/绿/蓝/黄
        let px: Vec<u8> = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
        ]
        .concat();
        st.set_background_image(BackgroundImage::from_rgba8(2, 2, &px).unwrap());
        assert!(st.background_is_image());
        let img = st.background_image().unwrap().clone();

        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(1, 0, &mut dst); // 世界原点 (64, 0)
        assert_eq!(
            &dst[0..4],
            &img.pixel_fix15(64, 0),
            "tile 左上是 x=64 的采样"
        );
        assert_eq!(&dst[4..8], &img.pixel_fix15(65, 0));
        // (64,0) % 2 = (0,0) → 红；(65,0) → 绿
        assert_eq!(img.pixel_rgba8(64, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel_rgba8(65, 0), [0, 255, 0, 255]);
        // 256 px 之后回到同一像素（纸纹 256 的整数倍）
        assert_eq!(img.pixel_fix15(0, 0), img.pixel_fix15(512, 1024));

        // 设回纯色会丢掉图案
        st.set_background_color([10, 20, 30]);
        assert!(!st.background_is_image());
        assert_eq!(st.background_color(), [10, 20, 30]);
    }

    /// 闪烁预览：只画当前层，并反转背景可见性（`tree.py:288-306/320-323`）。
    #[test]
    fn previewing_isolates_current_layer_and_inverts_background() {
        let mut st = LayerStack::new();
        let mut red = vec![0u8; 64 * 64 * 4];
        for px in red.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        st.layer_mut(0)
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&red, 0, 0, 64, 64);
        st.set_background_color([0, 0, 255]);
        let mut dst = vec![0u16; TILE_CHANNELS];

        // 背景可见 + 预览当前层 → 背景被反转成隐藏，只剩红层
        st.set_preview_path(Some(vec![0]));
        st.composite_tile(0, 0, &mut dst);
        assert!(dst[0] > 30_000, "预览应只有红层");
        assert!(dst[1] < 1_000 && dst[2] < 1_000, "背景应被隐藏");

        // 在红层上面插一个空层：预览空层时只剩透明（背景仍被反转隐藏）
        st.insert(0, Layer::new());
        st.set_preview_path(Some(vec![0]));
        st.composite_tile(0, 0, &mut dst);
        assert_eq!(&dst[..4], &[0, 0, 0, 0]);

        // 背景本来隐藏 → 预览把它显示出来
        st.set_background_visible(false);
        st.composite_tile(0, 0, &mut dst);
        assert!(dst[2] > 30_000, "背景隐藏时预览应显示背景");

        st.set_preview_path(None);
    }

    /// 预览忽略当前层的可见性、不透明度和合成模式（`data.py:348-352`）。
    #[test]
    fn previewing_ignores_layer_visibility_opacity_and_mode() {
        let mut st = LayerStack::new();
        let mut red = vec![0u8; 64 * 64 * 4];
        for px in red.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        let layer = st.layer_mut(0);
        layer
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&red, 0, 0, 64, 64);
        layer.visible = false;
        layer.opacity = 0.2;
        layer.mode = LayerMode::Standard(CombineMode::DestinationOut);

        st.set_preview_path(Some(vec![0]));
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        assert!(
            dst[0] > 30_000,
            "预览应按默认模式、opacity=1、可见重绘，实得红 {}",
            dst[0]
        );
        st.set_preview_path(None);
    }

    /// 预览递归整棵子树：组强制直通，子层忽略可见性/不透明度/模式。
    #[test]
    fn previewing_descends_into_groups_ignoring_their_style() {
        let mut st = LayerStack::new();
        let mut red = vec![0u8; 64 * 64 * 4];
        for px in red.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        let mut child = Layer::new();
        child
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&red, 0, 0, 64, 64);
        child.visible = false;
        child.opacity = 0.2;
        child.mode = LayerMode::Standard(CombineMode::DestinationOut);

        let mut group = Layer::new_group();
        group.visible = false;
        group.opacity = 0.2;
        group.mode = LayerMode::Standard(CombineMode::DestinationIn);
        group.stack_mut().unwrap().layers.push(child);
        st.insert(0, group);

        st.set_preview_path(Some(vec![0]));
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        assert!(
            dst[0] > 30_000,
            "组预览应直通并显示隐藏子层，实得红 {}",
            dst[0]
        );
        st.set_preview_path(None);
    }

    /// 预览反转背景可见性，`render_is_opaque` 与 RGBA8 输出都要跟着变。
    #[test]
    fn previewing_inverts_background_opacity_for_checks() {
        let mut st = LayerStack::new();
        st.set_background_color([0, 0, 255]);
        st.set_preview_path(Some(vec![0]));

        // 背景可见 → 预览隐藏它，空区域必须保留透明（棋盘格由 GPU 画）
        assert!(!st.render_is_opaque(), "预览隐藏背景 → 不能按不透明处理");
        let out = st.composite_tile_rgba8(0, 0).expect("预览空层也要输出");
        assert_eq!(out[3], 0, "透明区域不应被压成不透明底色");

        // 背景本来隐藏 → 预览显示它，结果不透明
        st.set_background_visible(false);
        assert!(st.render_is_opaque(), "预览显示背景 → 不透明");
        let out = st.composite_tile_rgba8(0, 0).unwrap();
        assert_eq!(out[3], 255);
        assert!(out[2] > 200, "应显示蓝色背景，实得 B={}", out[2]);
        st.set_preview_path(None);
    }

    /// 背景隐藏时 `composite_tile_rgba8` 必须保留 alpha（供 GPU 与棋盘格混合）。
    #[test]
    fn transparent_background_keeps_alpha_in_rgba8() {
        let mut st = LayerStack::new();
        st.set_background_visible(false);
        let mut px = vec![0u8; 64 * 64 * 4];
        // 左上角一个半透明红像素，其余全透明
        px[0..4].copy_from_slice(&[255, 0, 0, 128]);
        st.layer_mut(0)
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&px, 0, 0, 64, 64);
        let out = st.composite_tile_rgba8(0, 0).expect("有内容");
        assert!(
            (100..=150).contains(&out[3]),
            "alpha 应保留为 ~128，实得 {}",
            out[3]
        );
        assert!(
            out[0] > 150,
            "预乘红色应约 186（alpha 0.5），实得 {}",
            out[0]
        );
        // 不透明像素在背景隐藏时也必须保留 alpha=255
        px[0..4].copy_from_slice(&[0, 255, 0, 255]);
        st.layer_mut(0)
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&px, 0, 0, 64, 64);
        let out = st.composite_tile_rgba8(0, 0).unwrap();
        assert_eq!(out[3], 255, "不透明像素 alpha 应为 255");
        assert!(out[1] > 200, "绿色应可见，实得 {}", out[1]);
        // 背景可见时仍走不透明预合成
        st.set_background_visible(true);
        let out = st.composite_tile_rgba8(0, 0).unwrap();
        assert_eq!(out[3], 255);
    }

    #[test]
    fn vector_layer_is_a_non_paintable_svg_preview() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6">
            <rect width="8" height="6" fill="#ff0000"/></svg>"##;
        let mut layer = Layer::new_vector(svg, 10, 20).unwrap();
        assert!(layer.is_vector());
        assert!(!layer.is_group());
        assert!(!layer.paintable_self(), "FileBackedLayer 不能直接画");
        assert_eq!(layer.display_name(0), "Vectors");
        assert_eq!(layer.type_description(), "Vector Layer");
        assert_eq!(layer.vector_data().unwrap().x, 10);
        assert_eq!(layer.vector_data().unwrap().y, 20);
        // 预览表面已栅格化到 (10,20)；外接框按 tile 对齐。
        assert_eq!(layer.surface().unwrap().data_bbox(), (0, 0, 64, 64));

        // 换成更小的 SVG：预览重算、原点不变。
        let svg2 = r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">
            <rect width="2" height="2" fill="#00ff00"/></svg>"##;
        layer.set_vector_svg(svg2).unwrap();
        assert_eq!(layer.vector_data().unwrap().x, 10);
        assert_eq!(layer.vector_data().unwrap().y, 20);
        assert!(layer.surface().unwrap().tile(0, 0).is_some());
        assert!(layer.vector_data().unwrap().svg.contains("#00ff00"));
    }

    #[test]
    fn vector_layer_is_skipped_by_paintable_stack_entries() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"##;
        let mut st = LayerStack::from_children(vec![Layer::new_vector(svg, 0, 0).unwrap()]);
        st.set_current_path(&[0]);
        assert!(st.paintable_surface_mut().is_none());
        assert!(
            st.surface_by_id(st.current_id()).is_some(),
            "预览表面仍可读"
        );
    }
}

#[cfg(test)]
mod tile_revision_tests {
    use super::*;

    #[test]
    fn tile_composite_revision_is_local_and_property_sensitive() {
        let mut st = LayerStack::new();
        let px = [200u8, 100, 50, 255].repeat(64 * 64);
        let s = st.current_mut().surface_mut().unwrap();
        s.load_from_rgba8(&px, 0, 0, 64, 64);
        s.load_from_rgba8(&px, 64, 0, 64, 64);
        let a = st.tile_composite_revision(0, 0);
        let b = st.tile_composite_revision(1, 0);
        assert_ne!(a, b, "不同 tile 的版本没有必要相同");
        assert_ne!(
            a,
            st.tile_composite_revision(2, 0),
            "空白 tile 也是独立版本"
        );

        // 只改 (1,0)：另一个 tile 的版本必须完全不变。
        st.current_mut()
            .surface_mut()
            .unwrap()
            .load_from_rgba8(&px, 64, 0, 64, 64);
        assert_eq!(st.tile_composite_revision(0, 0), a, "(0,0) 不该被连坐");
        assert_ne!(st.tile_composite_revision(1, 0), b, "(1,0) 必须变脏");

        // 图层属性变化则所有 tile 一起变。
        let before = st.tile_composite_revision(0, 0);
        st.current_mut().opacity = 0.5;
        assert_ne!(st.tile_composite_revision(0, 0), before);
    }
}

#[cfg(test)]
mod mipmap_composite_tests {
    use super::*;

    /// 四个纯色（都是 4 的倍数，降采样后数值可精确预期）。
    const A: [u16; 4] = [400, 800, 1200, 32768];
    const B: [u16; 4] = [40, 80, 160, 1600];
    const C: [u16; 4] = [4000, 8000, 12000, 24000];
    const D: [u16; 4] = [200, 40, 4, 8000];

    /// 整块填满固定 fix15 值的 tile 载荷。
    fn filled(p: [u16; 4]) -> Vec<u16> {
        let mut v = vec![0u16; TILE_CHANNELS];
        for c in v.chunks_exact_mut(4) {
            c.copy_from_slice(&p);
        }
        v
    }

    /// 只留一层、关掉背景的栈（这样合成结果就是图层内容本身）。
    fn stack() -> LayerStack {
        let mut st = LayerStack::new();
        st.set_background_visible(false);
        st
    }

    /// 合成 `(level, tx, ty)` 并取 tile 内 `(x, y)` 的 fix15 像素。
    fn at(st: &LayerStack, level: u32, tx: i32, ty: i32, x: usize, y: usize) -> [u16; 4] {
        let mut buf = vec![0u16; TILE_CHANNELS];
        st.composite_tile_at(level, tx, ty, &mut buf);
        buf[(y * TILE_SIZE + x) * 4..][..4].try_into().unwrap()
    }

    /// level 1 的合成结果来自四个不同的基 tile（四象限各取一格）。
    #[test]
    fn level1_composite_takes_content_from_four_base_tiles() {
        let mut st = stack();
        {
            let s = st.current_mut().surface_mut().unwrap();
            s.paste_tile(0, 0, &filled(A));
            s.paste_tile(1, 0, &filled(B));
            s.paste_tile(0, 1, &filled(C));
            s.paste_tile(1, 1, &filled(D));
        }
        assert_eq!(at(&st, 1, 0, 0, 0, 0), A, "左上象限 ← 基 (0,0)");
        assert_eq!(at(&st, 1, 0, 0, 63, 0), B, "右上象限 ← 基 (1,0)");
        assert_eq!(at(&st, 1, 0, 0, 0, 63), C, "左下象限 ← 基 (0,1)");
        assert_eq!(at(&st, 1, 0, 0, 63, 63), D, "右下象限 ← 基 (1,1)");

        // level 0 必须没被改变：每块仍是自己的纯色
        assert_eq!(at(&st, 0, 0, 0, 0, 0), A);
        assert_eq!(at(&st, 0, 1, 1, 63, 63), D);
        // 更远一级：金字塔**不在 tile 边界上混合**（每级只是 2×2 平均，
        // 而象限边界永远落在偶数坐标上），所以 level 2 是小一号的同一张图：
        // A 占 (0..16)²、B 占 (16..32)×(0..16)、D 占 (16..32)²。
        assert_eq!(at(&st, 2, 0, 0, 0, 0), A);
        assert_eq!(at(&st, 2, 0, 0, 15, 15), A);
        assert_eq!(at(&st, 2, 0, 0, 16, 0), B);
        assert_eq!(at(&st, 2, 0, 0, 0, 16), C);
        assert_eq!(at(&st, 2, 0, 0, 16, 16), D);
        assert_eq!(at(&st, 2, 0, 0, 31, 31), D, "右下角仍是 D");
    }

    /// level 1 的空 tile 被跳过：不算内容、不占金字塔内存。
    #[test]
    fn empty_level1_tile_is_skipped() {
        let mut st = stack();
        st.current_mut()
            .surface_mut()
            .unwrap()
            .paste_tile(0, 0, &filled(A));

        let mut buf = vec![0u16; TILE_CHANNELS];
        let has_content = st.composite_tile_at(1, 7, 7, &mut buf);
        assert!(!has_content, "远处空块不该报有内容");
        assert!(buf.iter().all(|v| *v == 0), "空块合成结果为全透明");
        assert_eq!(
            st.current().surface().unwrap().mipmap_bytes(),
            0,
            "空块被跳过，不该留下像素内存"
        );
    }

    /// 合成版本号必须带级别：同一 tile 的 level 0 与 level 1 版本不同，
    /// 且子 tile 变化会让 level 1 的版本变。
    #[test]
    fn composite_revision_is_level_aware() {
        let mut st = stack();
        st.current_mut()
            .surface_mut()
            .unwrap()
            .paste_tile(0, 0, &filled(A));

        let r0 = st.tile_composite_revision(0, 0);
        let r1 = st.tile_composite_revision_at(1, 0, 0);
        assert_ne!(r0, r1, "同一坐标在两级的版本不能相同");
        assert_eq!(r1, st.tile_composite_revision_at(1, 0, 0), "同级稳定");
        assert_eq!(
            r0,
            st.tile_composite_revision_at(0, 0, 0),
            "level 0 的旧入口与新入口必须完全一致"
        );

        // 改**子** tile（基 (1,1) 落在 level 1 的 (0,0) 里）→ level 1 变脏
        st.current_mut()
            .surface_mut()
            .unwrap()
            .paste_tile(1, 1, &filled(D));
        assert_ne!(
            st.tile_composite_revision_at(1, 0, 0),
            r1,
            "子 tile 变化必须让 level 1 的版本变化，否则远景永不刷新"
        );
    }

    /// 背景在 level > 0 用**盒式平均**而不是点采样。
    ///
    /// 源码 `Background` 构造时逐级 `tile_downscale_rgba16`（`tiledsurface.py`
    /// `:1205-1217`）。用 1px 棋盘格纸纹来区分两种实现：盒式平均后整级变成
    /// 均匀中灰，点采样会保留棋盘格（远景出摩尔纹）。
    #[test]
    fn background_is_box_averaged_at_higher_levels() {
        let mut st = stack();
        st.set_background_visible(true);
        let mut rgba = Vec::with_capacity(64 * 64 * 4);
        for y in 0..64 {
            for x in 0..64 {
                let v = if (x + y) % 2 == 0 { 255u8 } else { 0 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        st.set_background_image(BackgroundImage::from_rgba8(64, 64, &rgba).expect("背景"));

        let mut buf = vec![0u16; TILE_CHANNELS];
        let pix = |buf: &[u16], x: usize, y: usize| -> [u16; 4] {
            buf[(y * TILE_SIZE + x) * 4..][..4].try_into().unwrap()
        };

        // level 0：棋盘格原样（白 = fix15 32768，黑 = 0）
        st.composite_tile_at(0, 0, 0, &mut buf);
        assert_eq!(pix(&buf, 0, 0), [32768, 32768, 32768, 32768]);
        assert_eq!(pix(&buf, 1, 0), [0, 0, 0, 32768]);

        // level 1：2×2 盒式平均 → 每格 = 32768/4*2 = 16384，整级均匀
        st.composite_tile_at(1, 0, 0, &mut buf);
        for (x, y) in [(0, 0), (1, 0), (0, 1), (31, 31)] {
            assert_eq!(
                pix(&buf, x, y),
                [16384, 16384, 16384, 32768],
                "level 1 ({x},{y}) 必须是平均值（点采样会保留棋盘格）"
            );
        }
        // level 2 继续平均 → 仍是 16384（各级 4 个相同 tap 各 /4 再相加）
        st.composite_tile_at(2, 0, 0, &mut buf);
        assert_eq!(pix(&buf, 0, 0), [16384, 16384, 16384, 32768]);
    }

    /// 合成级别的 RGBA8 输出：远景块的上传路径（M3 用）。
    #[test]
    fn composite_rgba8_at_matches_the_level_composite() {
        let mut st = stack();
        st.current_mut()
            .surface_mut()
            .unwrap()
            .paste_tile(0, 0, &filled(A));

        let l0 = st.composite_tile_rgba8_at(0, 0, 0).expect("level 0 有内容");
        let l1 = st.composite_tile_rgba8_at(1, 0, 0).expect("level 1 有内容");
        // 两者编码同一套像素（基 (0,0) 在 level 1 里占左上象限，两处首像素相同）
        assert_eq!(&l0[0..4], &l1[0..4]);
        assert_eq!(l1[3], 255, "A 是满 alpha，保留 alpha 模式也是 255");
        // 旧入口与新入口在 level 0 完全等价
        let legacy = st.composite_tile_rgba8(0, 0).expect("有内容");
        assert_eq!(&legacy[..], &l0[..]);

        // 空块：背景不可见时按 level 0 的既有约定输出透明 tile（不能返回
        // None，否则显存里会留着旧的远景块）。
        let far = st.composite_tile_rgba8_at(1, 9, 9).expect("透明块也要有输出");
        assert!(far.iter().all(|b| *b == 0), "空远景块是全透明");
        // 不透明背景下，空远景块输出纯底色（远景不会露出棋盘格）
        let mut opaque = LayerStack::new();
        opaque
            .current_mut()
            .surface_mut()
            .unwrap()
            .paste_tile(0, 0, &filled(A));
        let white = opaque.composite_tile_rgba8_at(1, 9, 9).expect("有输出");
        assert_eq!(&white[0..4], &[255, 255, 255, 255], "远景是纸色");
    }
}
