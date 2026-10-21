//! pigment-surface —— pigment 项目的自家 tile 表面后端（无限画布存储层）。
//!
//! 实现 hokusai 的 [`TiledSurface`] trait。混合算法（`draw_dab` / `get_color` /
//! `get_color_pigment`）**刻意用 hokusai-core 的默认实现**，保证与参考后端
//! `MemSurface` 逐像素一致（行为对等由 `tests/parity.rs` 守住）。
//!
//! 唯一的覆写是 [`symmetry`]：libmypaint 把对称展开放在表面自己的 `draw_dab`
//! 里（`mypaint-tiled-surface.c:690-800`），所以这里也照做 —— `draw_dab` =
//! 默认实现画原始 dab + 对称 pass。**对称关闭时逐像素与 MemSurface 全等**，
//! 这条由 `tests/symmetry.rs` 单独守（见 `symmetry_off_is_bit_identical_to_reference`）。
//!
//! 相对 MemSurface 的增量（为 M2 wgpu 渲染层铺路）：
//!
//! 1. **内容版本号**：每个 tile 携带单调递增的 `revision`。GPU 纹理镜像
//!    只需记录「已上传的 revision」，`revision > uploaded` 即需重传——
//!    增量上传不需要 diff 像素。
//! 2. **LRU 元数据**：`last_touched` 记录最近访问帧号，为显存/内存驻留
//!    淘汰策略（M2 的 LRU 显存驻留）提供依据。
//! 3. **原子脏区**：`begin_atomic` / `end_atomic` 收集一次笔画内被修改的
//!    tile 列表（与 libmypaint 的 `tile_request` 语义对齐）。
//! 4. **写时复制快照**：`save_snapshot` / `load_snapshot` 是 M4 撤销体系的
//!    基石（见下）。
//! 5. **mipmap 金字塔**（[`mipmap`]）：按 MyPaint 的做法为每个表面维护
//!    level 1..4 的降采样 tile 缓存（`_create_mipmap_surfaces`，
//!    `tiledsurface.py:159-181`），合成路径可按级别取样
//!    （[`LayerStack::composite_tile_at`]）。它是**纯缓存**：`Clone` 不复制，
//!    写路径靠 [`PigmentSurface::mark_mipmap_dirty`] /
//!    [`PigmentSurface::invalidate_mipmap`] 作废，读取时惰性重建。
//!
//! # 快照（照搬 MyPaint `lib/tiledsurface.py:461-494`）
//!
//! MyPaint 的做法是：`save_snapshot()` 把全部 tile 标记为 `readonly` 并
//! **浅拷贝** tiledict（共享 tile 对象），之后任何写借出遇到 `readonly`
//! 的 tile 就先复制一份再写——即经典的 copy-on-write；`load_snapshot()`
//! 把 tiledict 换回快照那份，靠**对象同一性**的对称差算出变化的 tile 并
//! 通知观察者（未变的 tile 对象被两个 dict 共享，天然"无变化"）。
//!
//! Rust 侧用 `Arc<TilePixels>` + [`Arc::make_mut`] 表达同一件事：
//!
//! - `save_snapshot` 浅拷贝一张坐标→`Arc` 的表（不复制像素）；
//! - 写借出时 `Arc::make_mut` 在**引用计数 > 1**（即与某个快照共享）时
//!   自动复制像素、脱离共享，计数为 1 时零开销——对应 MyPaint 的
//!   `readonly` 分支；
//! - `load_snapshot` 用 `Arc::ptr_eq` 判"对象是否同一个"，等价于 MyPaint
//!   的 `set(tiledict.items())` 对称差。
//!
//! 因此快照成本与撤销内存都只落在「真正被改过的 tile」上。
//!
//! 扩展点（暂未启用）：tooth 纹理沉积（M6 超越点）将在本 crate 覆盖
//! `draw_dab` 时注入高度图系数；届时以 parity 测试的对照快照做回归门禁。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use hokusai::brushmodes::GpuDabParams;
use rayon::prelude::*;
use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};
use hokusai::{Dab, DeferredSmudge, SmudgeBucket, TiledSurface};

/// 运行期计数器：用来回答「这一笔到底走 GPU 还是 CPU」这类问题。
///
/// 只累加原子量，开销可以忽略；读取端按差值打印，不需要清空。
pub mod perf {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// GPU 队列里实际排进去的 dab 数。
    pub static DABS_GPU: AtomicU64 = AtomicU64::new(0);
    /// 无法用 GPU 表达、退回 CPU 光栅的 dab 数。
    pub static DABS_CPU: AtomicU64 = AtomicU64::new(0);
    /// `flush_gpu_dabs` 的总耗时（纳秒），**包含事件内的调用**。
    pub static FLUSH_NS: AtomicU64 = AtomicU64::new(0);
    /// `flush_gpu_dabs` 调用次数。
    pub static FLUSH_CALLS: AtomicU64 = AtomicU64::new(0);
    /// 回读并落地到表面的 tile 数。
    pub static FLUSH_TILES: AtomicU64 = AtomicU64::new(0);
    /// GPU 侧采样（smudge）总耗时与次数。
    pub static SAMPLE_NS: AtomicU64 = AtomicU64::new(0);
    pub static SAMPLE_CALLS: AtomicU64 = AtomicU64::new(0);

    #[inline]
    pub fn add(counter: &AtomicU64, v: u64) {
        counter.fetch_add(v, Ordering::Relaxed);
    }

    #[inline]
    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

pub mod color;
pub(crate) mod color_sample;
pub mod combine;
pub mod layer;
pub mod mipmap;
pub mod png;
pub mod strokemap;
pub mod symmetry;
pub mod thumb;
pub mod vector;

pub use layer::{Layer, LayerStack};
pub use mipmap::{MipMap, MipState, MipTile};
pub use strokemap::StrokeShape;
pub use symmetry::{warm_parallel_pool, SymmetryData, SymmetryState, SymmetryType, Transform};
pub use vector::VectorLayerData;

/// 单个 tile：像素数据 + 同步/淘汰元数据。
///
/// `pixels` 是 `Arc`：与快照共享时写借出自动复制（见模块文档的写时复制一节）。
pub struct Tile {
    pixels: Arc<TilePixels>,
    /// 内容版本号：每次「写意图」借出（`tile_request_start`）时递增。
    /// 保守策略——借出即视为可能脏，宁可多传不可漏传。
    revision: u64,
    /// 最近访问的全局时钟值（LRU 依据）。
    last_touched: u64,
}

impl Tile {
    fn empty() -> Self {
        Self {
            pixels: Arc::new(*empty_tile()),
            revision: 0,
            last_touched: 0,
        }
    }

    #[inline]
    pub fn pixels(&self) -> &TilePixels {
        &self.pixels
    }

    /// 共享句柄（mipmap 降采样 / GPU 上传时要把像素交给另一条借用链，
    /// 克隆 `Arc` 只加引用计数、不复制 32 KiB 像素）。
    #[inline]
    pub fn pixels_arc(&self) -> &Arc<TilePixels> {
        &self.pixels
    }

    /// 内容版本号（0 = 从未写入）。
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 最近访问帧号。
    #[inline]
    pub fn last_touched(&self) -> u64 {
        self.last_touched
    }
}

/// 表面快照（不可变）：坐标 → 共享像素。
///
/// 只在 Undo/Redo（`lib/command.py:Brushwork`）与表面复制中使用，
/// 对应 MyPaint 的 `_SurfaceSnapshot`（`lib/tiledsurface.py:94`）。
#[derive(Clone, Default)]
pub struct SurfaceSnapshot {
    tiles: HashMap<(i32, i32), Arc<TilePixels>>,
}

impl SurfaceSnapshot {
    /// 快照里的 tile 数。
    #[inline]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// 快照像素数据的内存占用（只统计**独占**的 tile，共享部分不计）。
    pub fn memory_bytes(&self) -> usize {
        self.tiles
            .values()
            .filter(|px| Arc::strong_count(px) == 1)
            .count()
            * TILE_SIZE
            * TILE_SIZE
            * 4
            * 2
    }

    /// 只读读取某个 tile（不存在则 `None`）。
    #[inline]
    pub fn tile(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tiles.get(&(tx, ty)).map(|px| &**px)
    }

    /// 遍历快照持有的 tile 坐标。
    #[inline]
    pub fn coords(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.tiles.keys().copied()
    }
}

/// `load_snapshot` 的结果：哪些 tile 需要重新上传、哪些需要从显存剔除。
///
/// MyPaint 用同一个对称差去 `notify_observers(*bbox)`（`tiledsurface.py:492`）；
/// 我们的渲染层是"显存镜像"模型，所以除了脏 tile 还要知道**被删掉**的 tile，
/// 否则撤销到较早状态后，显存里会残留已经不存在的 tile（画布漏出旧内容）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SnapshotDiff {
    /// 内容变化（含新增）的 tile，需要重新上传。
    pub changed: Vec<(i32, i32)>,
    /// 快照中不存在、需从显存剔除的 tile。
    pub removed: Vec<(i32, i32)>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

/// 可选的 GPU dab 光栅器。
///
/// 画笔引擎是一步一步同步调用 `draw_dab` 的，但 GPU 适合把一次指针事件里
/// 的所有 dab 批量提交。实现方在 `queue_dab` 里只记录 dab；在 `flush` 里
/// 上传本次事件涉及的 tile、按顺序执行、再把结果写回表面。表面在
/// `end_atomic`（每次事件末尾）和任何采样读取前调用 `flush`，因此 CPU
/// 侧看到的像素语义与逐 dab 光栅完全一致。
pub trait GpuDabSink: Send {
    /// 记录一个可完整表达为 [`GpuDabParams`] 的 dab。
    ///
    /// 返回 `false` 表示该 dab 不受支持（调用方会先 flush，再用 CPU 画）。
    fn queue_dab(&mut self, dab: &Dab, params: &GpuDabParams, tiles: &[(i32, i32)]) -> bool;

    /// 执行所有已排队的 dab，并通过 [`PigmentSurface::write_gpu_tile`]
    /// 把结果写回表面。
    fn flush(&mut self, surface: &mut PigmentSurface);

    /// 渲染帧使用的限量 flush：只处理队列前缀，剩余留给下一帧。
    /// 默认退化成完整 flush。
    fn flush_budgeted(&mut self, surface: &mut PigmentSurface) {
        self.flush(surface);
    }

    /// 采样前的 flush：执行排队 dab，但**不回读 tile**；像素回读留给
    /// 帧末。默认退化成完整 flush。
    fn flush_for_sample(&mut self, surface: &mut PigmentSurface) {
        self.flush(surface);
    }

    /// 用 GPU 显存里的当前像素做一次采样（smudge 用）。
    ///
    /// 返回 `None` 表示这条路径不可用，调用方退回 CPU 采样（会先 flush）。
    /// 实现方默认不支持。
    #[allow(unused_variables)]
    fn sample(
        &mut self,
        surface: &PigmentSurface,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> Option<[f32; 4]> {
        None
    }

    /// Queue a dab whose smudge colour must be resolved device-side, in queue
    /// order. `params` is `None` for degenerate dabs that still update the
    /// smudge bucket but cover no pixels. Returns `false` when the sink cannot
    /// express the request (the caller falls back to CPU sampling).
    #[allow(unused_variables)]
    fn defer_smudge(
        &mut self,
        surface: &PigmentSurface,
        dab: &Dab,
        params: Option<&GpuDabParams>,
        tiles: &[(i32, i32)],
        req: &DeferredSmudge,
    ) -> bool {
        false
    }

    /// True when the device smudge buckets are newer than the CPU mirror.
    fn deferred_smudge_dirty(&self) -> bool {
        false
    }

    /// Queued GPU operations not yet submitted.
    fn pending_ops(&self) -> usize {
        0
    }

    /// Flush deferred work and copy the device bucket state back into `buckets`
    /// when the device chain still belongs to `reset_epoch`.
    #[allow(unused_variables)]
    fn sync_deferred_smudge(
        &mut self,
        surface: &mut PigmentSurface,
        reset_epoch: u64,
        buckets: &mut [SmudgeBucket],
    ) {
    }
}

/// 无限画布：`HashMap<(tx, ty), Tile>` 稀疏存储，画到哪里分配到哪里。
///
/// 低缩放 Instant Preview 的临时覆盖面（当前显示 mip 级别）。
///
/// Krita 的 `KisPrescaledProjection` 等价物的最小版：笔画先在这个低分辨率
/// 表面上画，合成/显示走它；真实笔画随后按全分辨率重放。只影响显示，
/// 不进快照、不进文档像素。
struct PreviewOverlay {
    level: u32,
    surface: PigmentSurface,
}

/// 单 tile 为 64×64 RGBA fix15（预乘 alpha、线性 sRGB），32 KiB，
/// 与 libmypaint / hokusai 的 tile 几何完全一致。
#[derive(Default)]
pub struct PigmentSurface {
    tiles: HashMap<(i32, i32), Tile>,
    /// 逐 tile 的 mipmap 金字塔（[`mipmap`] 模块；`_create_mipmap_surfaces`，
    /// `tiledsurface.py:159-181`）。
    ///
    /// 放在 `RefCell` 里是 Rust 侧的取舍：源码的 `_get_tile_numpy` 读取时会
    /// 顺手把脏哨兵换成真实 tile（**读操作改字典**），而我们的合成路径全是
    /// `&self`（`LayerStack::composite_tile` 等几十处调用点）。用
    /// `RefCell` 把"惰性重建"关进内层可变借用，就不必把整条合成链改成
    /// `&mut self`，语义与源码一一对应。
    ///
    /// 借用纪律：只在 `mip_tile` / `mip_tile_revision` 内部短借，返回的是
    /// `Arc` 克隆，绝不把 `Ref`/`RefMut` 泄漏出去，因此不会出现重入 panic。
    mipmap: RefCell<MipMap>,
    /// 原子窗口内被写借出的 tile（`begin_atomic` ~ `end_atomic`）。
    dirty: HashSet<(i32, i32)>,
    in_atomic: bool,
    /// 全局时钟：revision 与 last_touched 的共同来源。
    clock: u64,
    /// 对称绘制状态（`mypaint-tiled-surface.h:46` 的 `symmetry_data`）。
    ///
    /// 挂在表面上而不是笔刷引擎上，是 libmypaint 的做法：一次 `draw_dab`
    /// 由表面自己展开成 N 个 dab（`mypaint-tiled-surface.c:690-800`），
    /// 见 [`symmetry`] 模块。
    symmetry: SymmetryData,
    /// 笔画形状图（`lib/layer/data.py:1481` 的 `self.strokes`）。
    strokes: Vec<StrokeShape>,
    /// 可选的 GPU dab 光栅器（见 [`GpuDabSink`]）。
    gpu_sink: Option<Box<dyn GpuDabSink>>,
    /// 瞬态低分辨率预览覆盖（见 [`PreviewOverlay`]）。
    preview: Option<Box<PreviewOverlay>>,
    /// 预览面不需要金字塔（每次 dab 标脏/补丁纯属浪费）。
    skip_mipmaps: bool,
}

/// 克隆：与快照一样按 tile 共享 `Arc`（写借出时 COW 自动分离）。
///
/// mipmap 金字塔**不克隆**：它是可以由基表完全重建的纯缓存，而 Clone 是
/// autosave 快照 / 图层复制的热路径（"300 个 tile 的毫秒级克隆"）。复制
/// 金字塔只会让克隆付出成倍的 32 KiB 拷贝与哈希表重建，收益为零——重建是
/// 惰性的，只有真正被渲染到的那几格才会重新算。语义上也更安全：金字塔
/// 里可能残留与基表不一致的中间态。
impl Clone for PigmentSurface {
    fn clone(&self) -> Self {
        Self {
            tiles: self
                .tiles
                .iter()
                .map(|(&coord, t)| {
                    (
                        coord,
                        Tile {
                            pixels: Arc::clone(&t.pixels),
                            revision: t.revision,
                            last_touched: t.last_touched,
                        },
                    )
                })
                .collect(),
            mipmap: RefCell::new(MipMap::new()),
            dirty: HashSet::new(),
            in_atomic: false,
            clock: self.clock,
            symmetry: self.symmetry.clone(),
            strokes: self.strokes.clone(),
            // 克隆只承载像素/结构；排队的 GPU dab 属于原表面当前事件的
            // 瞬时状态，不能复制给快照或图层副本。
            gpu_sink: None,
            // 预览是瞬时显示状态，快照/图层副本不继承。
            preview: None,
            skip_mipmaps: self.skip_mipmaps,
        }
    }
}

/// 采样时是否推迟 GPU 同步（`PIGMENT_LAZY_SMUDGE_SAMPLE=1`）。
///
/// smudge 笔刷每个 dab 都要重采样画布（`smudge_length_log == 0` 时阈值恒为
/// 1，于是每次都重采样），而采样必须读到之前 dab 的像素 —— 队列在 GPU 上时
/// 这等于「每个 dab 一次 submit + fence」。实测单次往返 0.16ms（wait 就占
/// 0.11ms，纯延迟），240Hz 输入下每帧 140 个 dab 就是 20ms。
///
/// 打开这个开关后，一次指针事件内不再同步：事件里的所有 dab 都用**事件开始
/// 时**的画布做采样，事件结束（`end_atomic`）才统一落地一次。smudge 的采样
/// 粒度因此从「每个 dab」变成「每个输入事件」—— 手感上等价于按事件距离采样。
fn lazy_smudge_sample() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PIGMENT_LAZY_SMUDGE_SAMPLE").is_some())
}

/// 一块 fix15 tile 的原始字节视图（GPU 上传/回读用，逐位无损）。
///
/// `TilePixels` 是 `[[[u16; 4]; 64]; 64]`，没有填充，按 u8 视图读出的是
/// 同一段连续内存；GPU 端的 tile 布局与此完全一致（每像素 8 字节）。
#[inline]
pub fn tile_bytes(tile: &TilePixels) -> &[u8] {
    // SAFETY: `[[u16; 4]; N]` 是连续且无填充的 POD 数组；u8 视图只会
    // 缩短对齐要求，长度取 `size_of` 保证不越界。
    unsafe {
        std::slice::from_raw_parts(
            tile.as_ptr().cast::<u8>(),
            std::mem::size_of::<TilePixels>(),
        )
    }
}

/// [`tile_bytes`] 的可变版本（GPU 回读落地用）。
#[inline]
pub(crate) fn tile_bytes_mut(tile: &mut TilePixels) -> &mut [u8] {
    // SAFETY: 同上；`&mut` 保证独占，不存在别名。
    unsafe {
        std::slice::from_raw_parts_mut(
            tile.as_mut_ptr().cast::<u8>(),
            std::mem::size_of::<TilePixels>(),
        )
    }
}

impl PigmentSurface {
    pub fn new() -> Self {
        Self::default()
    }

    /// 安装/卸载 GPU dab 光栅器。
    pub fn set_gpu_sink(&mut self, sink: Option<Box<dyn GpuDabSink>>) {
        self.gpu_sink = sink;
    }

    #[inline]
    pub fn has_gpu_sink(&self) -> bool {
        self.gpu_sink.is_some()
    }

    /// 执行所有排队中的 GPU dab。表面在事件末尾与采样前自动调用。
    /// 用 GPU 侧像素采样一次；没有 GPU 队列时返回 `None`。
    ///
    /// 语义与 `get_color_pigment` 一致，但**不触发** GPU 同步：像素本来就
    /// 在显存里（含已排队但还没落地的 dab）。这正是把「每个 dab 一次
    /// submit+fence」去掉的接口。
    pub fn sample_gpu(&mut self, x: f32, y: f32, radius: f32, paint: f32) -> Option<[f32; 4]> {
        let mut sink = self.gpu_sink.take()?;
        let r = sink.sample(self, x, y, radius, paint);
        self.gpu_sink = Some(sink);
        r
    }

    /// Queue a smudge dab whose sample + bucket + colour chain runs on the
    /// device. `None` means this surface cannot defer it; the engine then
    /// falls back to the CPU sampling path.
    pub fn draw_dab_deferred_smudge(
        &mut self,
        dab: &Dab,
        req: &DeferredSmudge,
    ) -> Option<bool> {
        if self.symmetry.active
            || self.gpu_sink.is_none()
            || std::env::var_os("PIGMENT_NO_DEFERRED_SMUDGE").is_some()
        {
            return None;
        }
        let prepared = hokusai::brushmodes::prepare_dab(dab);
        let params = prepared.as_ref().and_then(|p| p.gpu_params());
        if prepared.is_some() && params.is_none() {
            return None;
        }
        let tiles: Vec<(i32, i32)> = prepared
            .as_ref()
            .map(|p| {
                p.tiles()
                    .filter(|&(tx, ty)| p.row_range(tx, ty).is_some())
                    .collect()
            })
            .unwrap_or_default();
        let mut sink = self.gpu_sink.take()?;
        let queued = sink.defer_smudge(self, dab, params.as_ref(), &tiles, req);
        self.gpu_sink = Some(sink);
        if !queued {
            return None;
        }
        Some(!tiles.is_empty())
    }

    /// Whether the device smudge buckets are newer than the CPU mirror.
    pub fn deferred_smudge_dirty(&self) -> bool {
        self.gpu_sink
            .as_ref()
            .is_some_and(|s| s.deferred_smudge_dirty())
    }

    /// GPU dabs queued for the next flush (smudge or raster).
    pub fn gpu_pending_ops(&self) -> usize {
        self.gpu_sink.as_ref().map_or(0, |s| s.pending_ops())
    }

    /// Flush queued GPU work and, when the device chain still matches
    /// `reset_epoch`, refresh the CPU smudge buckets from the device.
    pub fn sync_deferred_smudge(&mut self, reset_epoch: u64, buckets: &mut [SmudgeBucket]) {
        let Some(mut sink) = self.gpu_sink.take() else {
            return;
        };
        sink.sync_deferred_smudge(self, reset_epoch, buckets);
        self.gpu_sink = Some(sink);
    }

    /// Event-level smudge sample: flush everything queued so the atlas is the
    /// event-start canvas, then run the sampling kernel once and read the
    /// colour back. One sync per pointer event.
    pub fn get_color_pigment_gpu(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> Option<hokusai::color::RgbaF32> {
        self.flush_gpu_dabs();
        let mut sink = self.gpu_sink.take()?;
        let sample = sink.sample(self, x, y, radius, paint);
        self.gpu_sink = Some(sink);
        sample.map(|c| hokusai::color::RgbaF32 {
            r: c[0],
            g: c[1],
            b: c[2],
            a: c[3],
        })
    }

    pub fn flush_gpu_dabs(&mut self) {
        if self.gpu_sink.is_none() {
            return;
        }
        let t = std::time::Instant::now();
        if let Some(mut sink) = self.gpu_sink.take() {
            sink.flush(self);
            self.gpu_sink = Some(sink);
        }
        perf::add(&perf::FLUSH_NS, t.elapsed().as_nanos() as u64);
        perf::add(&perf::FLUSH_CALLS, 1);
    }

    /// 渲染帧使用的限量 flush：给下一帧留一部分 GPU 工作，避免单帧尖峰。
    /// 快照/撤销/采样前仍然调用完整的 [`Self::flush_gpu_dabs`]。
    pub fn flush_gpu_dabs_budgeted(&mut self) {
        if self.gpu_sink.is_none() {
            return;
        }
        let t = std::time::Instant::now();
        if let Some(mut sink) = self.gpu_sink.take() {
            sink.flush_budgeted(self);
            self.gpu_sink = Some(sink);
        }
        perf::add(&perf::FLUSH_NS, t.elapsed().as_nanos() as u64);
        perf::add(&perf::FLUSH_CALLS, 1);
    }

    /// GPU 光栅结果写回：与 `tile_request_start` 相同的 revision / 脏区 /
    /// 写时复制语义，外加整块像素替换。
    ///
    /// 这里**不复用** `tile_request_start`：那一版在原子窗口外会立刻
    /// `mark_dirty`（整座金字塔标脏，等读取时整块重建）。GPU 回写发生在
    /// `end_atomic` 之后，是所有低缩放绘制的真正落笔点，所以改走增量补丁
    /// （[`mipmap::MipMap::patch_base`]）：金字塔已经是 Ready 时只重算
    /// 受影响的 4×4 小块，链条不完整时它自己退回标脏。
    pub fn write_gpu_tile(&mut self, tx: i32, ty: i32, bytes: &[u8]) -> u64 {
        assert_eq!(
            bytes.len(),
            TILE_SIZE * TILE_SIZE * 4 * 2,
            "GPU tile 字节数必须正好是一块 fix15 tile"
        );
        self.clock += 1;
        let clock = self.clock;
        let tile = self.tiles.entry((tx, ty)).or_insert_with(Tile::empty);
        tile.revision = clock;
        tile.last_touched = clock;
        // 写时复制：与快照共享像素时先复制（同 `tile_request_start`）。
        tile_bytes_mut(Arc::make_mut(&mut tile.pixels)).copy_from_slice(bytes);
        self.mipmap.get_mut().patch_base(tx, ty, &self.tiles);
        clock
    }

    /// GPU 回读的**批量**落地：与 [`Self::write_gpu_tile`] 逐字节等价，
    /// 但把「32 KiB 像素拷进 tile」这一段按 tile 切给 rayon。
    ///
    /// 单次 GPU 同步回读几十万到上百万字节（一块 tile 是 32 KiB，大笔刷
    /// 一个事件几百块），这段拷贝是纯内存带宽、彼此独立；串行时它是整个
    /// 回写里最贵的一段（实测 10µs/块，远高于全分辨率合成的 1µs/块）。
    /// 金字塔补丁仍然串行——它要跨 tile 共享的上级格子，并行会撞同一批
    /// `Arc`。
    ///
    /// `items` 里允许重复坐标（后者覆盖前者），与逐个调用的语义一致。
    pub fn write_gpu_tiles(&mut self, items: &[((i32, i32), &[u8])]) {
        if items.is_empty() {
            return;
        }
        let tile_bytes_expected = TILE_SIZE * TILE_SIZE * 4 * 2;
        assert!(
            items.iter().all(|(_, b)| b.len() == tile_bytes_expected),
            "GPU tile 字节数必须正好是一块 fix15 tile"
        );

        // 把要写的 tile 从表里摘出来：独占所有权才能在多线程里原地改写
        // （留在 HashMap 里只能拿到 `&mut Tile` 的一整个借用）。
        let mut jobs: Vec<((i32, i32), Tile)> = Vec::with_capacity(items.len());
        for &(c, _) in items {
            self.clock += 1;
            let clock = self.clock;
            let mut tile = self.tiles.remove(&c).unwrap_or_else(Tile::empty);
            tile.revision = clock;
            tile.last_touched = clock;
            jobs.push((c, tile));
        }

        // 并行拷贝（阈值取 8：再少调度开销就盖过收益）。
        if jobs.len() >= 8 {
            jobs.par_iter_mut()
                .enumerate()
                .for_each(|(i, (_, tile))| {
                    let px = Arc::make_mut(&mut tile.pixels);
                    tile_bytes_mut(px).copy_from_slice(items[i].1);
                });
        } else {
            for (i, (_, tile)) in jobs.iter_mut().enumerate() {
                let px = Arc::make_mut(&mut tile.pixels);
                tile_bytes_mut(px).copy_from_slice(items[i].1);
            }
        }

        let __t_insert = std::time::Instant::now();
        for (c, tile) in jobs {
            self.tiles.insert(c, tile);
        }
        let __insert = __t_insert.elapsed();
        // 金字塔补丁：必须等所有基格都到位（补丁会读基表）。
        let coords: Vec<(i32, i32)> = items.iter().map(|(c, _)| *c).collect();
        let __t_patch = std::time::Instant::now();
        self.mipmap.get_mut().patch_many(&coords, &self.tiles);
        if std::env::var_os("PIGMENT_GPU_PERF").is_some() && items.len() >= 100 {
            eprintln!(
                "  wt n={} ins={:.2}ms patch={:.2}ms",
                items.len(),
                __insert.as_secs_f64() * 1e3,
                __t_patch.elapsed().as_secs_f64() * 1e3
            );
        }
    }

    /// 已记录的笔画形状（按绘制顺序）。
    #[inline]
    pub fn strokes(&self) -> &[StrokeShape] {
        &self.strokes
    }

    /// 整体替换 strokemap（移动图层的撤销需要恢复平移后的形状列表）。
    pub fn replace_strokes(&mut self, strokes: Vec<StrokeShape>) {
        self.strokes = strokes;
    }

    /// 记录一笔的形状（`StrokemappedPaintingLayer.add_stroke_shape`）。
    pub fn add_stroke_shape(
        &mut self,
        before: &SurfaceSnapshot,
        after: &SurfaceSnapshot,
        brush: &str,
    ) -> bool {
        let Some(mut shape) = StrokeShape::from_snapshots(before, after) else {
            return false;
        };
        shape.brush = brush.to_string();
        self.strokes.push(shape);
        true
    }

    /// 撤销时移除最后一笔形状（`strokes.pop()`）。
    pub fn pop_stroke(&mut self) -> Option<StrokeShape> {
        self.strokes.pop()
    }

    /// 重做时把命令保存的形状放回（顺序与绘制一致）。
    pub fn push_stroke(&mut self, shape: StrokeShape) {
        self.strokes.push(shape);
    }

    /// 多笔 `.dat` 字节串（`_write_strokemap`），`dx/dy` 为图层原点。
    pub fn strokemap_dat(&self, dx: i32, dy: i32) -> Vec<u8> {
        strokemap::write_dat(&self.strokes, dx, dy)
    }

    /// 从 `.dat` 读入（`_load_strokemap_from_file`）。
    pub fn load_strokemap_dat(&mut self, data: &[u8], dx: i32, dy: i32) -> Result<(), String> {
        self.strokes = strokemap::read_dat(data, dx, dy)?;
        Ok(())
    }

    /// 只读访问 tile 像素（不存在则 `None`）。
    #[inline]
    pub fn tile(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tiles.get(&(tx, ty)).map(|t| t.pixels())
    }

    /// 只读访问 tile（含元数据）。
    #[inline]
    pub fn tile_entry(&self, tx: i32, ty: i32) -> Option<&Tile> {
        self.tiles.get(&(tx, ty))
    }

    /// 共享像素句柄（预览层种子用；只加引用计数）。
    #[inline]
    pub fn tile_pixels_arc(&self, tx: i32, ty: i32) -> Option<Arc<TilePixels>> {
        self.tiles
            .get(&(tx, ty))
            .map(|t| Arc::clone(t.pixels_arc()))
    }

    /// 插入一块共享像素的 tile（预览面播种）。
    pub fn insert_tile_arc(
        &mut self,
        tx: i32,
        ty: i32,
        pixels: Arc<TilePixels>,
        revision: u64,
    ) {
        self.clock = self.clock.max(revision);
        self.tiles.insert(
            (tx, ty),
            Tile {
                pixels,
                revision,
                last_touched: revision,
            },
        );
    }

    /// 预览面播种：从本级已 `Ready` 的 mip 格子克隆 `Arc`，不做像素拷贝。
    ///
    /// 预览面和文档面共用同一坐标系（都是该 mip 级别的 64×64 格），
    /// 写借出时 `Arc::make_mut` 自动 COW，未触及的格子零成本。
    pub fn seed_preview_at(&self, level: u32) -> PigmentSurface {
        let mut preview = PigmentSurface::new();
        preview.skip_mipmaps = true;
        // 对称轴参数跟真实表面一致，预览里的镜像 dab 才不会缺席。
        preview.symmetry = self.symmetry.clone();
        for ((tx, ty), pixels, revision) in self.mipmap.borrow().ready_entries(level) {
            preview.insert_tile_arc(tx, ty, pixels, revision);
        }
        preview.clock = preview.clock.max(self.clock);
        preview
    }

    /// 挂上当前级别的预览覆盖（Instant Preview）。
    pub fn set_preview_overlay(&mut self, level: u32, surface: PigmentSurface) {
        self.preview = Some(Box::new(PreviewOverlay {
            level: level.min(mipmap::MAX_MIPMAP_LEVEL),
            surface,
        }));
    }

    /// 预览覆盖的级别。
    #[inline]
    pub fn preview_level(&self) -> Option<u32> {
        self.preview.as_ref().map(|p| p.level)
    }

    /// 预览面（笔画引擎往这里画）。
    #[inline]
    pub fn preview_surface_mut(&mut self) -> Option<&mut PigmentSurface> {
        self.preview.as_mut().map(|p| &mut p.surface)
    }

    /// 取走预览覆盖；返回是否有覆盖。
    pub fn clear_preview_overlay(&mut self) -> bool {
        self.preview.take().is_some()
    }

    #[inline]
    pub fn has_preview_overlay(&self) -> bool {
        self.preview.is_some()
    }

    // ---- mipmap（`_get_tile_numpy` / `_mark_mipmap_dirty` 的 Rust 面）----

    /// 取某级 mipmap tile 的像素，**惰性重建**；空块返回 `None`。
    ///
    /// 对应 `_get_tile_numpy(tx, ty, readonly)`（`tiledsurface.py:324-350`）：
    /// 读到脏哨兵才重建，重建结果缓存下来。`level == 0` 直接透传基表
    /// （不经过金字塔，也就没有额外开销）。
    ///
    /// 返回 `Arc` 克隆而不是引用：合成路径是 `&self`，而重建需要 `&mut`
    /// 金字塔（`RefCell` 的借用守卫不能带出函数体）。
    pub fn mip_tile(&self, level: u32, tx: i32, ty: i32) -> Option<Arc<TilePixels>> {
        if let Some(p) = &self.preview {
            if p.level == level {
                if let Some(px) = p.surface.tile_pixels_arc(tx, ty) {
                    return Some(px);
                }
            }
        }
        if level == 0 {
            return self.tiles.get(&(tx, ty)).map(|t| Arc::clone(t.pixels_arc()));
        }
        self.mipmap
            .borrow_mut()
            .tile_arc(level, tx, ty, &self.tiles)
    }

    /// 不触发重建的内容判断（渲染规划热路径）。
    ///
    /// 与 [`Self::mip_tile`] 的区别：脏哨兵不会再现场重建，直接视为
    /// "可能有内容"。低缩放下一次要检查数百格，避免规划阶段把整层 mip
    /// 都算一遍。空/从没写过的坐标返回 `false`。
    pub fn mip_has_content_cached(&self, level: u32, tx: i32, ty: i32) -> bool {
        if let Some(p) = &self.preview {
            if p.level == level && p.surface.tile_entry(tx, ty).is_some() {
                return true;
            }
        }
        if level == 0 {
            return self.tiles.contains_key(&(tx, ty));
        }
        self.mipmap
            .borrow()
            .tile_entry_at(level, tx, ty)
            .is_some_and(|t| !t.is_empty())
    }

    /// 不触发重建的 mip revision。
    ///
    /// 从未重建/已标脏的格子返回 0（视作需要重新上传）；已重建的格子
    /// 返回其真实 revision。上传时先合成再取这个值，水位线仍然一致。
    pub fn mip_tile_revision_cached(&self, level: u32, tx: i32, ty: i32) -> u64 {
        if let Some(p) = &self.preview {
            if p.level == level {
                if let Some(t) = p.surface.tile_entry(tx, ty) {
                    return t.revision();
                }
            }
        }
        if level == 0 {
            return self.tiles.get(&(tx, ty)).map(Tile::revision).unwrap_or(0);
        }
        // 脏哨兵的 revision 每次写入都会递增（见 `MipMap::mark_dirty`），
        // 因此这里可以直接返回它；绝不能把脏态统一当成 0，否则会与
        // "背景格已上传水位 0" 混淆。
        self.mipmap
            .borrow()
            .tile_entry_at(level, tx, ty)
            .map(MipTile::revision)
            .unwrap_or(0)
    }

    /// 某级 tile 的内容版本号；`0` = 空/从未重建。
    ///
    /// 会先把该格**材料化**（必要时重建）再取版本号，因此脏 → 已重建的
    /// 中间态不会多出一个"假变化"：GPU 侧把返回值当水位线即可，一次编辑
    /// 只会让该 tile 重新合成一次。
    pub fn mip_tile_revision(&self, level: u32, tx: i32, ty: i32) -> u64 {
        if let Some(p) = &self.preview {
            if p.level == level {
                if let Some(t) = p.surface.tile_entry(tx, ty) {
                    return t.revision();
                }
            }
        }
        if level == 0 {
            return self.tiles.get(&(tx, ty)).map(Tile::revision).unwrap_or(0);
        }
        let mut mip = self.mipmap.borrow_mut();
        // 材料化（结果丢弃，只取版本号）
        mip.tile_arc(level, tx, ty, &self.tiles);
        mip.tile_entry_at(level, tx, ty)
            .map(MipTile::revision)
            .unwrap_or(0)
    }

    /// 把基表坐标在所有 mipmap 级别标脏（源码每次写借出后都做，见
    /// `tiledsurface.py:347-349` 的 `self._mark_mipmap_dirty(tx, ty)`）。
    pub fn mark_mipmap_dirty(&self, tx: i32, ty: i32) {
        self.mipmap.borrow_mut().mark_dirty(tx, ty);
    }

    /// 整座金字塔作废（结构性操作后用；见 `clear()` / `trim()` 等处的注释）。
    pub fn invalidate_mipmap(&self) {
        self.mipmap.borrow_mut().invalidate_all();
    }

    /// 只读访问金字塔（统计/调试/测试用）。
    pub fn mipmap(&self) -> std::cell::Ref<'_, MipMap> {
        self.mipmap.borrow()
    }

    /// 金字塔占用的像素内存（不含基表）。
    pub fn mipmap_bytes(&self) -> usize {
        self.mipmap.borrow().storage_bytes()
    }

    /// 累计整块重建的格子数（诊断用）。
    ///
    /// 增量补丁把"每帧重建若干整块"换成"每次事件只重算受影响的小块"，
    /// 这个计数在正常绘制中应该几乎不涨——它就是那条捷径是否生效的证据。
    pub fn mip_rebuild_count(&self) -> u64 {
        self.mipmap.borrow().rebuild_count()
    }

    /// 模型坐标 `(x, y)` 周围半径 `radius` 内的平均 alpha（0..1）。
    ///
    /// 对应 `Document._layer_is_pickable` 用的 `layer.get_alpha(x, y,
    /// PICKING_RADIUS)`（`gui/document.py:1167`，`PICKING_RADIUS=5`）：
    /// 源码取圆形邻域的平均值，用来判断"这一点下面有没有东西"。
    /// 空 tile 按 alpha 0 计。
    pub fn average_alpha(&self, x: i32, y: i32, radius: i32) -> f32 {
        let r = radius.max(0);
        let r2 = r * r;
        let mut sum: u64 = 0;
        let mut count: u64 = 0;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                let px = x + dx;
                let py = y + dy;
                let tx = px.div_euclid(TILE_SIZE as i32);
                let ty = py.div_euclid(TILE_SIZE as i32);
                let ix = px.rem_euclid(TILE_SIZE as i32) as usize;
                let iy = py.rem_euclid(TILE_SIZE as i32) as usize;
                let a = self.tile(tx, ty).map(|t| t[iy][ix][3] as u64).unwrap_or(0);
                sum += a;
                count += 1;
            }
        }
        if count == 0 {
            return 0.0;
        }
        sum as f32 / (count as f32 * 32768.0)
    }

    /// 已分配 tile 数。
    #[inline]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// 数据外接框（tile 对齐）：`(x, y, w, h)`；空表面为 `(0,0,0,0)`。
    ///
    /// 等价 `Layer::data_bbox` 的绘画层分支（`tree.py:722-738`）。
    pub fn data_bbox(&self) -> (i32, i32, i32, i32) {
        let ts = TILE_SIZE as i32;
        let mut it = self.tiles.keys();
        let Some(&(x0, y0)) = it.next() else {
            return (0, 0, 0, 0);
        };
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (x0, y0, x0, y0);
        for &(x, y) in it {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        (
            min_x * ts,
            min_y * ts,
            (max_x - min_x + 1) * ts,
            (max_y - min_y + 1) * ts,
        )
    }

    /// 画布占用的内存字节数（每 tile 32 KiB）。
    #[inline]
    pub fn memory_bytes(&self) -> usize {
        self.tiles.len() * TILE_SIZE * TILE_SIZE * 4 * 2
    }

    /// 遍历全部已分配 tile（供整图上传 / 拼合导出）。
    #[inline]
    pub fn tiles(&self) -> impl Iterator<Item = ((i32, i32), &Tile)> {
        self.tiles.iter().map(|(&coord, t)| (coord, t))
    }

    /// 返回 `revision > since` 的 tile 坐标列表。
    ///
    /// GPU 纹理镜像的增量同步入口：渲染层记录上次同步到的全局时钟，
    /// 每帧调用本方法取回需要重新上传的 tile。
    pub fn changed_since(&self, since: u64) -> Vec<(i32, i32)> {
        self.tiles
            .iter()
            .filter(|(_, t)| t.revision > since)
            .map(|(&coord, _)| coord)
            .collect()
    }

    /// 当前全局时钟值（GPU 侧完成一次全量同步后记录此值）。
    #[inline]
    pub fn clock(&self) -> u64 {
        self.clock
    }

    // ---- 快照（照搬 `lib/tiledsurface.py:461-494`）----

    /// 创建并返回表面快照（`tiledsurface.py:461`）。
    ///
    /// 浅拷贝：像素仍与表面共享（`Arc` 计数 +1），只有之后的**写借出**
    /// 才会真正复制被碰到的那几个 tile。等价于 MyPaint 的
    /// `for t in itervalues(self.tiledict): t.readonly = True` + dict 浅拷贝。
    pub fn save_snapshot(&self) -> SurfaceSnapshot {
        SurfaceSnapshot {
            tiles: self
                .tiles
                .iter()
                .map(|(&coord, t)| (coord, Arc::clone(&t.pixels)))
                .collect(),
        }
    }

    /// 加载快照，替换内部 tiledict（`tiledsurface.py:475`）。
    ///
    /// 返回需要同步到显存的差异；内容未变的 tile 会**保留原 `Tile`**
    /// （revision 不变 → 显存无需重传），变化的 tile 拿到新 revision。
    pub fn load_snapshot(&mut self, sshot: &SurfaceSnapshot) -> SnapshotDiff {
        self.preview = None;
        let mut diff = SnapshotDiff::default();
        // 与 MyPaint 的 `if d == self.tiledict: return` 同理：同一张快照
        // 重复加载（典型场景：redo 后再 undo）不该产生任何重传。
        if self.tiles.len() == sshot.tiles.len()
            && self.tiles.iter().all(|(c, t)| {
                sshot
                    .tiles
                    .get(c)
                    .is_some_and(|px| Arc::ptr_eq(&t.pixels, px))
            })
        {
            return diff;
        }

        self.clock += 1;
        let clock = self.clock;
        let old = std::mem::take(&mut self.tiles);
        let mut tiles = HashMap::with_capacity(sshot.tiles.len());
        for (&coord, px) in &sshot.tiles {
            let tile = match old.get(&coord) {
                // 同一个像素对象 → 内容未变（MyPaint 的集合对称差不含它）
                Some(t) if Arc::ptr_eq(&t.pixels, px) => Tile {
                    pixels: Arc::clone(px),
                    revision: t.revision,
                    last_touched: clock,
                },
                _ => {
                    diff.changed.push(coord);
                    Tile {
                        pixels: Arc::clone(px),
                        revision: clock,
                        last_touched: clock,
                    }
                }
            };
            tiles.insert(coord, tile);
        }
        // 快照里没有、当前却有的 tile：撤销到较早状态时被"删掉"的内容
        diff.removed
            .extend(old.keys().filter(|c| !sshot.tiles.contains_key(c)).copied());
        self.tiles = tiles;
        // 撤销/重做是最容易残留陈旧 mipmap 缓存的路径：被换回旧内容的 tile
        // 与彻底消失的 tile 都要作废。按 diff 精确标脏（不做全清，否则一次
        // Ctrl+Z 会把整个远景金字塔打回重算）。
        {
            let mip = self.mipmap.get_mut();
            for &(tx, ty) in &diff.changed {
                mip.mark_dirty(tx, ty);
            }
            for &(tx, ty) in &diff.removed {
                mip.mark_dirty(tx, ty);
            }
        }
        diff.changed.sort_unstable();
        diff.removed.sort_unstable();
        diff
    }

    /// 清空表面与 strokemap（`lib/tiledsurface.py:197` 的 `clear()`）。
    ///
    /// 源码先清 tile 字典再清 strokes；返回是否真的清掉了东西。
    pub fn clear(&mut self) -> bool {
        let had_preview = self.clear_preview_overlay();
        if self.tiles.is_empty() && self.strokes.is_empty() {
            return had_preview;
        }
        self.clock += 1;
        self.tiles.clear();
        self.strokes.clear();
        // 源码 `clear()` 会顺带 `self.mipmap.clear()`（`tiledsurface.py:201-202`）。
        self.mipmap.get_mut().invalidate_all();
        true
    }

    /// 表面是否没有任何 tile（`DataLayer.is_empty`，`lib/layer/data.py:318`）。
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// 删除全零 tile（`tiledsurface.py` 的 `remove_empty_tiles`）。
    /// 返回 `(removed, total)`。
    pub fn remove_empty_tiles(&mut self) -> (usize, usize) {
        let total = self.tiles.len();
        let empty: Vec<(i32, i32)> = self
            .tiles
            .iter()
            .filter(|(_, tile)| {
                tile.pixels()
                    .iter()
                    .all(|row| row.iter().all(|px| *px == [0; 4]))
            })
            .map(|(&c, _)| c)
            .collect();
        for c in &empty {
            self.tiles.remove(c);
        }
        if !empty.is_empty() {
            self.clock += 1;
            // 删掉的是"内容变成全零"的 tile：上层 mipmap 里对应的格子必须
            // 重算成空。这里直接整座作废而不是逐格 mark_dirty —— 与
            // trim/translate 同理，删除往往成片发生，逐格标记的收益抵不上
            // 代码复杂度，而重建是惰性的（只有被渲染到的格子才付代价）。
            self.mipmap.get_mut().invalidate_all();
        }
        (empty.len(), total)
    }

    /// 用另一个表面的内容替换自身（`tiledsurface.py:498` 的 `load_from_surface`）。
    pub fn load_from_surface(&mut self, other: &PigmentSurface) -> SnapshotDiff {
        let sshot = other.save_snapshot();
        self.load_snapshot(&sshot)
    }

    // ---- 文档 I/O（照搬 `lib/surface.py` 的 TileBlittable 层）----

    /// 把 `rect` 指定的矩形渲染成 RGBA8 像素（行主序、每像素 4 字节）。
    ///
    /// `rect` 是绝对模型坐标 `(x, y, w, h)`；`alpha = false` 时把内容合成到
    /// `bg` 底色上并输出不透明像素，`true` 时保留透明。
    ///
    /// 与 `lib/surface.py:save_as_png` 一致：**空矩形退化成 1×1**，这样空文档
    /// 也能写出一个合法图像（源码注释：「Save at least one tile to allow empty
    /// docs to be written」）。
    pub fn render_rect_rgba8(
        &self,
        rect: (i32, i32, i32, i32),
        alpha: bool,
        bg: [u8; 3],
    ) -> Vec<u8> {
        let (x, y, mut w, mut h) = rect;
        if w == 0 || h == 0 {
            // 照源码：`if w == 0 or h == 0: x, y, w, h = (0, 0, 1, 1)`
            w = 1;
            h = 1;
        }
        let w = w.max(0) as usize;
        let h = h.max(0) as usize;
        let mut out = vec![0u8; w * h * 4];
        // 底色只解码一次（线性域合成）
        let bg_lin = color::bg_to_linear(bg);
        for row in 0..h {
            let wy = y + row as i32;
            let ty = wy.div_euclid(TILE_SIZE as i32);
            let iy = wy.rem_euclid(TILE_SIZE as i32) as usize;
            for col in 0..w {
                let wx = x + col as i32;
                let tx = wx.div_euclid(TILE_SIZE as i32);
                let ix = wx.rem_euclid(TILE_SIZE as i32) as usize;
                let o = (row * w + col) * 4;
                match self.tiles.get(&(tx, ty)) {
                    None => {
                        // 未分配 = 空：合成模式下写底色，alpha 模式写全透明
                        if alpha {
                            out[o..o + 4].copy_from_slice(&[0, 0, 0, 0]);
                        } else {
                            out[o..o + 3].copy_from_slice(&bg);
                            out[o + 3] = 255;
                        }
                    }
                    Some(tile) => {
                        let p = tile.pixels[iy][ix];
                        if alpha {
                            let px = color::tile_to_rgba8_alpha_single(p);
                            out[o..o + 4].copy_from_slice(&px);
                        } else {
                            let px = color::tile_to_rgba8_single(p, bg, bg_lin);
                            out[o..o + 4].copy_from_slice(&px);
                        }
                    }
                }
            }
        }
        out
    }

    /// 把 `rect` 写成 PNG（`lib/surface.py:220` 的 `save_as_png`）。    ///
    /// 返回实际写出的矩形 `(x, y, w, h)` —— 空表面会被退化成 1×1，
    /// 调用方需要知道真正落盘的尺寸（ORA 的 stack.xml 要用）。
    pub fn save_as_png(
        &self,
        filename: impl AsRef<Path>,
        rect: (i32, i32, i32, i32),
        alpha: bool,
        bg: [u8; 3],
    ) -> Result<(i32, i32, i32, i32), String> {
        let (x, y, mut w, mut h) = rect;
        if w == 0 || h == 0 {
            w = 1;
            h = 1;
        }
        png::write_png(
            filename.as_ref(),
            self.render_rect_rgba8(rect, alpha, bg),
            w as u32,
            h as u32,
            alpha,
        )?;
        Ok((x, y, w, h))
    }

    /// 用解码后的 RGBA8 像素替换表面内容（`load_layer_from_png` 的核心）。
    ///
    /// `(x, y)` 是内容左上角的模型坐标；像素按 tile 切分写入。
    /// 与 MyPaint 一致：**整层替换**，调用方应先 `clear()`。
    /// 把一块已经处于 fix15 预乘格式的 tile 原样写入表面（MergeLayerDown 用）。
    ///
    /// 与写借出路径一致地分配缺 tile、抬 revision。长度必须是整 tile。
    pub fn paste_tile(&mut self, tx: i32, ty: i32, pixels: &[u16]) {
        assert_eq!(pixels.len(), TILE_SIZE * TILE_SIZE * 4);
        self.clock += 1;
        self.mipmap.get_mut().mark_dirty(tx, ty);
        let entry = self.tiles.entry((tx, ty)).or_insert_with(|| Tile {
            pixels: Arc::new(*empty_tile()),
            revision: 0,
            last_touched: 0,
        });
        let px = Arc::make_mut(&mut entry.pixels);
        let flat = px.as_flattened_mut().as_flattened_mut();
        flat.copy_from_slice(pixels);
        entry.revision = self.clock;
        entry.last_touched = self.clock;
    }

    /// 按像素平移整个表面与 strokemap（`StrokemappedPaintingLayerMove` /
    /// `layer.translate(dx, dy)`，`lib/layer/data.py:1607`）。
    ///
    /// 平移是逐像素的（源码允许任意整数 dx/dy，不要求 tile 对齐）；透明像素
    /// 跳过，保持稀疏存储。返回是否有内容被移动。
    pub fn translate(&mut self, dx: i32, dy: i32) -> bool {
        if dx == 0 && dy == 0 {
            return false;
        }
        let ts = TILE_SIZE as i32;
        let mut moved: HashMap<(i32, i32), Vec<u16>> = HashMap::new();
        for ((tx, ty), tile) in &self.tiles {
            let src = tile.pixels();
            for y in 0..ts {
                for x in 0..ts {
                    let px = src[y as usize][x as usize];
                    if px == [0, 0, 0, 0] {
                        continue;
                    }
                    let wx = tx * ts + x + dx;
                    let wy = ty * ts + y + dy;
                    let dtx = wx.div_euclid(ts);
                    let dty = wy.div_euclid(ts);
                    let dix = wx.rem_euclid(ts) as usize;
                    let diy = wy.rem_euclid(ts) as usize;
                    let arr = moved
                        .entry((dtx, dty))
                        .or_insert_with(|| vec![0u16; TILE_SIZE * TILE_SIZE * 4]);
                    let o = (diy * TILE_SIZE + dix) * 4;
                    arr[o..o + 4].copy_from_slice(&px);
                }
            }
        }
        self.tiles.clear();
        if moved.is_empty() {
            self.clock += 1;
        } else {
            for ((tx, ty), arr) in moved {
                self.paste_tile(tx, ty, &arr);
            }
        }
        // 平移后**每一个** tile 都换了坐标：旧坐标的金字塔内容全部失效，
        // 逐格 mark_dirty 既标不全（要同时覆盖新旧坐标与所有祖先）也没有
        // 意义，直接整座作废。
        self.mipmap.get_mut().invalidate_all();
        for stroke in &mut self.strokes {
            stroke.translate(dx, dy);
        }
        true
    }

    /// 按矩形裁剪表面与 strokemap（`lib/tiledsurface.py:204` 的 `trim`）。
    ///
    /// 与源码逐条对齐：
    /// - 完全在矩形外的 tile 删除；
    /// - 部分覆盖的 tile **清除**矩形外的像素（保留 tile，哪怕变全零）；
    /// - 边界相等时保留 tile（源码用 `tx*N+N < x` / `tx*N > x+w` 的严格比较）。
    ///
    /// 返回是否真的改动了像素或 strokemap。所有 tile 都走写时复制路径，
    /// 因此 `before` 快照不会被污染。
    pub fn trim(&mut self, rect: (i32, i32, i32, i32)) -> bool {
        let (x, y, w, h) = rect;
        if w <= 0 || h <= 0 {
            return false;
        }
        let n = TILE_SIZE as i64;
        let (x, y, right, bottom) = (x as i64, y as i64, x as i64 + w as i64, y as i64 + h as i64);
        let mut changed = false;
        let mut pixel_changed = false;
        let clock = self.clock + 1;
        let old = std::mem::take(&mut self.tiles);
        let mut tiles = HashMap::with_capacity(old.len());
        for (coord, mut tile) in old {
            let x0 = coord.0 as i64 * n;
            let y0 = coord.1 as i64 * n;
            if x0 + n < x || y0 + n < y || x0 > right || y0 > bottom {
                changed = true;
                continue;
            }
            // 源码四个判定（`tiledsurface.py:223-232`）：只处理矩形边界
            // **严格落在 tile 内部**的方向。
            let clear_left = x0 < x && x < x0 + n;
            let clear_top = y0 < y && y < y0 + n;
            let clear_right = x0 < right && right < x0 + n;
            let clear_bottom = y0 < bottom && bottom < y0 + n;
            if clear_left || clear_top || clear_right || clear_bottom {
                let px = Arc::make_mut(&mut tile.pixels);
                let (x0u, y0u) = (coord.0 * TILE_SIZE as i32, coord.1 * TILE_SIZE as i32);
                for row in 0..TILE_SIZE {
                    for col in 0..TILE_SIZE {
                        let wx = x0u + col as i32;
                        let wy = y0u + row as i32;
                        let outside = (clear_left && wx < x as i32)
                            || (clear_top && wy < y as i32)
                            || (clear_right && wx >= right as i32)
                            || (clear_bottom && wy >= bottom as i32);
                        if outside {
                            px[row][col] = [0; 4];
                        }
                    }
                }
                tile.revision = clock;
                tile.last_touched = clock;
                changed = true;
                pixel_changed = true;
            }
            tiles.insert(coord, tile);
        }
        self.tiles = tiles;
        if pixel_changed {
            self.clock = clock;
        }
        if changed {
            // 裁剪会**删掉**整块 tile 或把边界像素清零，受影响坐标分散在
            // 各条边界上：逐格 mark_dirty 需要按矩形遍历全部祖先格，而
            // 整座作废只需要 O(1)，重建又只在真正被渲染到时发生，所以这里
            // 选整清（源码 `trim()` 是按块 `_mark_mipmap_dirty` 的，因为它
            // 的 mipmap 字典已经存在且逐块遍历成本低；我们的取舍见上）。
            self.mipmap.get_mut().invalidate_all();
        }
        let before = self.strokes.len();
        self.strokes.retain_mut(|s| s.trim(rect));
        changed || self.strokes.len() != before
    }

    /// 把解码后的 RGBA8 像素写入表面（`load_layer_from_png` 的底层路径）。
    ///
    /// `(x, y)` 是内容左上角的模型坐标；全透明像素不分配 tile。
    pub fn load_from_rgba8(&mut self, pixels: &[u8], x: i32, y: i32, w: u32, h: u32) {
        // 源码 `load_from_surface` 会清空 strokes（data.py:1494-1499）
        self.strokes.clear();
        let (w, h) = (w as i32, h as i32);
        let ts = TILE_SIZE as i32;
        // 行内按 tile 分段处理：一段只做一次 tile 请求与 fix15 转换缓冲，
        // 避免 276M 像素级别的逐像素 div_euclid + 哈希查找（载入整份
        // fill_outlines.ora 时这是分钟级 → 秒级的差别）。
        let mut seg: Vec<[u16; 4]> = Vec::with_capacity(TILE_SIZE as usize);
        for row in 0..h {
            let wy = y + row;
            let ty = wy.div_euclid(ts);
            let iy = wy.rem_euclid(ts) as usize;
            let mut col = 0;
            while col < w {
                let tx = (x + col).div_euclid(ts);
                let seg_len = (ts - (x + col).rem_euclid(ts)).min(w - col);
                let ix0 = (x + col).rem_euclid(ts) as usize;
                // 转换本段（跳过全透明像素 —— 不必分配 tile，与 MyPaint 一致）
                seg.clear();
                let mut any = false;
                for c in 0..seg_len {
                    let o = ((row as usize) * (w as usize) + (col + c) as usize) * 4;
                    let px = color::rgba8_to_fix15(
                        pixels[o],
                        pixels[o + 1],
                        pixels[o + 2],
                        pixels[o + 3],
                    );
                    any |= px[3] != 0;
                    seg.push(px);
                }
                if any {
                    let tile = self.tile_request_start(tx, ty);
                    for (c, px) in seg.iter().enumerate() {
                        if px[3] == 0 {
                            continue;
                        }
                        tile[iy][ix0 + c] = *px;
                    }
                    self.tile_request_end(tx, ty);
                }
                col += seg_len;
            }
        }
    }

    /// 整体替换：清空旧 tile 后载入（`load_from_surface` 的整层语义）。
    ///
    /// 与 [`PigmentSurface::load_from_rgba8`] 的区别：后者是"把一块像素
    /// 铺上去"，供测试/叠加用；整层文件载入必须用这个，否则透明区域会
    /// 透出旧内容。
    pub fn replace_from_rgba8(&mut self, pixels: &[u8], x: i32, y: i32, w: u32, h: u32) {
        self.tiles.clear();
        self.preview = None;
        // 旧 tile 整批消失（不只是被覆盖）→ 金字塔必须整座作废，
        // 否则旧内容会在远景里留下"鬼影"。
        self.mipmap.get_mut().invalidate_all();
        self.load_from_rgba8(pixels, x, y, w, h);
    }

    /// 从 PNG 读取并载入表面（`load_png` → `load_layer_from_png`）。
    ///
    /// 返回内容外接矩形 `(x, y, w, h)`，与 `load_layer_from_png` 的返回值
    /// 语义一致（供 `Document` 设画框用）。
    pub fn load_from_png(
        &mut self,
        filename: impl AsRef<Path>,
        x: i32,
        y: i32,
    ) -> Result<(i32, i32, i32, i32), String> {
        let (px, w, h) = png::read_rgba8_png(filename.as_ref())?;
        self.replace_from_rgba8(&px, x, y, w, h);
        Ok((x, y, w as i32, h as i32))
    }
}

impl TiledSurface for PigmentSurface {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        self.clock += 1;
        let clock = self.clock;
        if self.in_atomic {
            self.dirty.insert((tx, ty));
        } else if !self.skip_mipmaps {
            // 原子窗口外的零散写入：立刻作废 mipmap。
            self.mipmap.get_mut().mark_dirty(tx, ty);
        }
        // 原子窗口内（= 一个指针事件）推迟到 `end_atomic` 按**唯一基格**
        // 批量标脏。大刮刀一个事件会写同一格几百次；逐 dab 标脏会在
        // mipmap 的 4 个层级上反复查哈希，纯属浪费。渲染发生在事件之间，
        // 窗口内没有 mip 读取。
        let tile = self.tiles.entry((tx, ty)).or_insert_with(Tile::empty);
        tile.revision = clock;
        tile.last_touched = clock;
        // 写时复制：与快照共享像素（引用计数 > 1）时先复制一份再写，
        // 对应 MyPaint `tile_request` 里遇到 `tile.readonly` 的复制分支
        // （`tiledsurface.py:280-291`）。独占时零开销。
        Arc::make_mut(&mut tile.pixels)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tile(tx, ty)
    }

    /// 大半径 smudge 走按 tile 并行的取色；小半径保持标量路径。
    fn get_color_pigment(&self, x: f32, y: f32, radius: f32, paint: f32) -> hokusai::color::RgbaF32 {
        if paint < 0.0 {
            return self.get_color(x, y, radius);
        }
        if radius >= 128.0 {
            if let Some(c) =
                crate::color_sample::get_color_pigment_parallel(self, x, y, radius, paint)
            {
                return c;
            }
        }
        hokusai::brushmodes::get_color_pigment_default(self, x, y, radius, paint)
    }

    fn begin_atomic(&mut self) {
        // 对称矩阵在这里重算：源码的 `mypaint_tiled_surface_begin_atomic`
        // 做的就是 `mypaint_update_symmetry_state(&self->symmetry_data)`
        // （`mypaint-tiled-surface.c:94-99`），而 `begin_atomic` 只在"一笔的开头"
        // 被调用，所以重算永远落在 dab 热路径之外。
        self.symmetry.update();
        self.in_atomic = true;
        self.dirty.clear();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        // GPU 队列**不在这里**同步：一次指针事件一个 submit+wait 会
        // 把低缩放笔画的帧时间全耗在驱动往返上。GUI 在渲染前调用
        // `flush_gpu_dabs`（一帧一次），引擎在采样前调用 `flush_dabs`，
        // 收笔/快照前也会 flush；像素语义因此保持不变。
        self.in_atomic = false;
        if lazy_smudge_sample() && self.gpu_sink.is_some() {
            self.flush_gpu_dabs();
        }
        let dirty = std::mem::take(&mut self.dirty);
        // 一次事件里改过的基格：逐格做 mipmap 增量补丁（只重算受影响的那
        // 一小块，见 `MipMap::patch_base`）。走不了的格子自己退回标脏，
        // 由渲染前的惰性重建兜底。预览面没有金字塔，跳过。
        if !self.skip_mipmaps {
            let mip = self.mipmap.get_mut();
            for &(tx, ty) in &dirty {
                mip.patch_base(tx, ty, &self.tiles);
            }
        }
        dirty.into_iter().collect()
    }

    fn flush_dabs(&mut self) {
        // 采样前的这次同步是「每 dab 一次驱动往返」的唯一来源。
        // 推迟模式下交给 `end_atomic` 统一做（一事件一次）。
        if !lazy_smudge_sample() {
            self.flush_gpu_dabs();
        }
    }

    fn defer_smudge(&self) -> bool {
        self.gpu_sink.is_some()
            && !self.symmetry.active
            && std::env::var_os("PIGMENT_NO_DEFERRED_SMUDGE").is_none()
    }

    fn draw_dab_deferred_smudge(
        &mut self,
        dab: &Dab,
        req: &DeferredSmudge,
    ) -> Option<bool> {
        PigmentSurface::draw_dab_deferred_smudge(self, dab, req)
    }

    fn deferred_smudge_dirty(&self) -> bool {
        PigmentSurface::deferred_smudge_dirty(self)
    }

    fn sync_deferred_smudge(&mut self, reset_epoch: u64, buckets: &mut [SmudgeBucket]) {
        PigmentSurface::sync_deferred_smudge(self, reset_epoch, buckets);
    }

    fn get_color_pigment_gpu(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> Option<hokusai::color::RgbaF32> {
        PigmentSurface::get_color_pigment_gpu(self, x, y, radius, paint)
    }

    /// 一次 dab = 原始 dab + 对称展开（`mypaint-tiled-surface.c:690-800`）。
    ///
    /// libmypaint 把对称放在**表面**的 `draw_dab` 里，所以笔刷引擎照旧一个
    /// 事件一个 dab；没开启对称时这里就是原来的默认实现，一个像素都不差。
    fn draw_dab(&mut self, dab: &Dab) -> bool {
        self.draw_dab_with_symmetry(dab)
    }

    // get_color / get_color_pigment：全部走 hokusai-core 默认实现，
    // 与 MemSurface 保持逐像素一致。
}

#[cfg(test)]
mod tests {
    /// 缓存 revision 必须在同一 mip 父格内每次写入都变化，供 GPU 水位使用。
    #[test]
    fn cached_mip_revision_changes_on_each_write() {
        use hokusai::TiledSurface;
        let mut s = PigmentSurface::new();
        {
            let t = s.tile_request_start(0, 0);
            t[0][0] = [1, 2, 3, 1 << 15];
            s.tile_request_end(0, 0);
        }
        let r1 = s.mip_tile_revision_cached(4, 0, 0);
        {
            let t = s.tile_request_start(1, 1);
            t[0][0] = [4, 5, 6, 1 << 15];
            s.tile_request_end(1, 1);
        }
        let r2 = s.mip_tile_revision_cached(4, 0, 0);
        assert_ne!(r1, r2, "同一 mip 父格的第二次写入必须换 revision");

        // 物化后再写一次，revision 仍要变。
        let _ = s.mip_tile(4, 0, 0);
        let r3 = s.mip_tile_revision_cached(4, 0, 0);
        assert!(r3 > 0);
        {
            let t = s.tile_request_start(0, 1);
            t[0][0] = [7, 8, 9, 1 << 15];
            s.tile_request_end(0, 1);
        }
        let r4 = s.mip_tile_revision_cached(4, 0, 0);
        assert!(r4 > r3, "物化后的写入必须抬高 revision: {r3} → {r4}");
    }

    use super::*;

    fn write(surface: &mut PigmentSurface, tx: i32, ty: i32, v: u16) {
        let t = surface.tile_request_start(tx, ty);
        t[0][0] = [v, v, v, 1 << 15];
        surface.tile_request_end(tx, ty);
    }

    fn pixel(surface: &PigmentSurface, tx: i32, ty: i32) -> Option<[u16; 4]> {
        surface.tile(tx, ty).map(|t| t[0][0])
    }

    #[test]
    fn average_alpha_samples_a_circular_neighbourhood() {
        let mut s = PigmentSurface::new();
        // 在 (10,10) 附近铺一块满 alpha，半径 5 的圆内大部分被覆盖
        let t = s.tile_request_start(0, 0);
        for y in 0..21 {
            for x in 0..21 {
                t[y][x] = [0, 0, 0, 1 << 15];
            }
        }
        s.tile_request_end(0, 0);
        assert!((s.average_alpha(10, 10, 5) - 1.0).abs() < 1e-6);
        // 圆完全落在外面 → 0
        assert_eq!(s.average_alpha(50, 50, 5), 0.0);
        // 部分覆盖：把 (10,10) 右边一半清空
        let t = s.tile_request_start(0, 0);
        for y in 0..21 {
            for x in 11..21 {
                t[y][x] = [0; 4];
            }
        }
        s.tile_request_end(0, 0);
        let a = s.average_alpha(10, 10, 5);
        assert!(a > 0.2 && a < 0.8, "半覆盖应落在中间（实得 {a}）");
    }

    #[test]
    fn lends_records_dirty_and_stamps_metadata() {
        let mut s = PigmentSurface::new();
        s.begin_atomic();
        {
            let t = s.tile_request_start(2, -1);
            t[0][0] = [1, 2, 3, 4];
            s.tile_request_end(2, -1);
        }
        let dirty = s.end_atomic();
        assert_eq!(dirty, vec![(2, -1)]);

        let tile = s.tile_entry(2, -1).unwrap();
        assert_eq!(tile.pixels()[0][0], [1, 2, 3, 4]);
        assert!(tile.revision() > 0);
        assert_eq!(tile.revision(), tile.last_touched());
        assert_eq!(s.tile(0, 0), None);
        assert_eq!(s.tile_count(), 1);
        assert_eq!(s.memory_bytes(), 32 * 1024);
    }

    #[test]
    fn revision_is_monotonic_and_changed_since_filters() {
        let mut s = PigmentSurface::new();
        let sync_point = s.clock();

        s.tile_request_start(0, 0);
        s.tile_request_start(1, 0);
        s.tile_request_end(0, 0);
        s.tile_request_end(1, 0);

        let mut changed = s.changed_since(sync_point);
        changed.sort();
        assert_eq!(changed, vec![(0, 0), (1, 0)]);

        let after_first_writes = s.clock();
        // 只重写 (0,0)：changed_since 应只剩它
        s.tile_request_start(0, 0);
        s.tile_request_end(0, 0);
        assert_eq!(s.changed_since(after_first_writes), vec![(0, 0)]);
        // 重复借出仍递增（保守脏策略）
        let r1 = s.tile_entry(1, 0).unwrap().revision();
        assert!(r1 < s.tile_entry(0, 0).unwrap().revision());
    }

    /// 快照的核心契约：**之后的写入不得污染快照**（写时复制）。
    #[test]
    fn snapshot_survives_later_writes() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 111);
        let sshot = s.save_snapshot();

        write(&mut s, 0, 0, 222);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 222, "表面应写入新值");
        assert_eq!(
            sshot.tile(0, 0).unwrap()[0][0][0],
            111,
            "快照必须保留旧值（写时复制）"
        );

        // 未触碰的 tile 不复制：像素对象仍与快照同一个
        write(&mut s, 5, 5, 9);
        let s2 = s.save_snapshot();
        assert!(s2.tile(0, 0).is_some() && sshot.tile(0, 0).is_some());

        // 回滚
        s.load_snapshot(&sshot);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 111, "回滚应恢复旧值");
    }

    /// 多张快照共存（undo + redo 两份状态）各自独立。
    #[test]
    fn multiple_snapshots_coexist() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let a = s.save_snapshot();
        write(&mut s, 0, 0, 2);
        let b = s.save_snapshot();
        write(&mut s, 0, 0, 3);

        s.load_snapshot(&b);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 2);
        s.load_snapshot(&a);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 1);
        s.load_snapshot(&b);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 2, "两次加载互不影响");
    }

    /// 加载快照要报告差异：变化的 tile 需重传，缺席的 tile 需从显存剔除。
    #[test]
    fn load_snapshot_reports_changed_and_removed() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let sshot = s.save_snapshot();

        write(&mut s, 0, 0, 2); // 变化
        write(&mut s, 9, 9, 3); // 新增（快照里没有）

        let diff = s.load_snapshot(&sshot);
        assert_eq!(diff.changed, vec![(0, 0)], "恢复后 (0,0) 内容变了");
        assert_eq!(diff.removed, vec![(9, 9)], "(9,9) 不在快照里 → 需剔除");
    }

    /// 内容未变的 tile 保留原 revision：显存无需重传。
    #[test]
    fn unchanged_tiles_keep_revision() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        write(&mut s, 1, 0, 7);
        let sshot = s.save_snapshot();
        let before = s.tile_entry(0, 0).unwrap().revision();

        // 只改 (1,0)，再撤销 —— (0,0) 两个状态完全一致
        write(&mut s, 1, 0, 8);
        let diff = s.load_snapshot(&sshot);
        assert_eq!(diff.changed, vec![(1, 0)]);
        assert_eq!(
            s.tile_entry(0, 0).unwrap().revision(),
            before,
            "未变 tile 的 revision 不该动，否则会白白重传"
        );
        assert!(s.tile_entry(1, 0).unwrap().revision() > before);
    }

    /// 重复加载同一张快照是幂等的（redo → undo → redo 的老路径）。
    #[test]
    fn reloading_same_snapshot_is_idempotent() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let sshot = s.save_snapshot();
        let diff = s.load_snapshot(&sshot);
        // 首次加载：状态本来就一致（像素对象同一个）→ 无需任何同步
        assert!(diff.is_empty());
        let rev = s.tile_entry(0, 0).unwrap().revision();
        assert!(s.load_snapshot(&sshot).is_empty());
        assert_eq!(s.tile_entry(0, 0).unwrap().revision(), rev, "revision 不动");
    }

    /// 写时复制只在共享时发生：独占 tile 的像素对象原地复用。
    #[test]
    fn copy_on_write_only_when_shared() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let lone = Arc::as_ptr(&s.tiles[&(0, 0)].pixels);
        write(&mut s, 0, 0, 2);
        assert_eq!(
            Arc::as_ptr(&s.tiles[&(0, 0)].pixels),
            lone,
            "独占写入不该复制像素"
        );

        let _sshot = s.save_snapshot();
        let shared = Arc::as_ptr(&s.tiles[&(0, 0)].pixels);
        write(&mut s, 0, 0, 3);
        assert_ne!(
            Arc::as_ptr(&s.tiles[&(0, 0)].pixels),
            shared,
            "与快照共享时必须复制像素"
        );
    }

    /// 空表面快照 → 恢复后应把画布清空（document.clear 与 redo 的路径）。
    #[test]
    fn loading_empty_snapshot_clears_surface() {
        let mut s = PigmentSurface::new();
        let empty = s.save_snapshot();
        write(&mut s, 0, 0, 1);
        write(&mut s, 1, 1, 1);
        let diff = s.load_snapshot(&empty);
        assert_eq!(diff.removed, vec![(0, 0), (1, 1)]);
        assert_eq!(s.tile_count(), 0);
    }

    #[test]
    fn load_from_surface_copies_content() {
        let mut a = PigmentSurface::new();
        write(&mut a, 3, 4, 5);
        let mut b = PigmentSurface::new();
        let diff = b.load_from_surface(&a);
        assert_eq!(b.tile(3, 4).unwrap()[0][0][0], 5);
        assert_eq!(diff.changed, vec![(3, 4)]);
        // 复制是写时复制语义：改 a 不影响 b
        write(&mut a, 3, 4, 6);
        assert_eq!(b.tile(3, 4).unwrap()[0][0][0], 5);
    }

    #[test]
    fn translate_moves_pixels_across_tiles() {
        let mut s = PigmentSurface::new();
        s.load_from_rgba8(&[255, 0, 0, 255], 1, 1, 1, 1);
        assert!(s.tile(0, 0).is_some());
        assert!(s.translate(64, 0));
        // (1,1) + (64,0) = (65,1) → tile (1,0)，块内 (1,1)
        let t = s.tile(1, 0).expect("应移到右侧 tile");
        assert_eq!(t[1][1][0], 1 << 15, "红色 + 全 alpha");
        assert!(s.tile(0, 0).is_none(), "旧 tile 应清掉");
    }

    #[test]
    fn trim_clips_partial_tiles_and_removes_outside_tiles() {
        let mut s = PigmentSurface::new();
        let red = [255u8, 0, 0, 255].repeat(64 * 64);
        s.load_from_rgba8(&red, 0, 0, 64, 64);
        s.load_from_rgba8(&red, 128, 0, 64, 64);
        assert_eq!(s.tile_count(), 2);

        assert!(s.trim((0, 0, 32, 64)), "部分裁剪必须算改动");
        assert_eq!(s.tile_count(), 1, "右侧整块 tile 应删除");
        let t = s.tile(0, 0).expect("部分覆盖的 tile 保留");
        assert_eq!(t[0][31][3], 1 << 15, "画框内最后一列保留");
        assert_eq!(t[0][32], [0; 4], "画框外第一列清空");
        assert_eq!(t[63][31][3], 1 << 15, "下边界仍保留");
    }

    #[test]
    fn trim_keeps_tiles_that_only_touch_the_boundary_like_source() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 7);
        // tiledsurface.py:219 用 `tx*N+N < x` 的严格比较：右边界正好等于
        // tile 起点时不删除。照抄这个边界语义。
        assert!(!s.trim((64, 0, 64, 64)));
        assert!(s.tile(0, 0).is_some());
    }

    #[test]
    fn remove_empty_tiles_discards_only_all_zero_tiles() {
        let mut s = PigmentSurface::new();
        let zeros = vec![0u16; 64 * 64 * 4];
        s.paste_tile(0, 0, &zeros);
        write(&mut s, 1, 0, 7);
        assert_eq!(s.tile_count(), 2);
        assert_eq!(s.remove_empty_tiles(), (1, 2));
        assert_eq!(s.tile_count(), 1);
        assert!(s.tile(1, 0).is_some());
        assert_eq!(s.remove_empty_tiles(), (0, 1));
    }

    #[test]
    fn load_from_rgba8_replaces_the_whole_surface() {
        let mut s = PigmentSurface::new();
        let red = [255u8, 0, 0, 255];
        s.replace_from_rgba8(&red, 0, 0, 1, 1);
        assert_eq!(s.tile_count(), 1);
        // 载入一张全透明图：旧内容必须被清掉（整体替换）
        let clear = vec![0u8; 64 * 64 * 4];
        s.replace_from_rgba8(&clear, 0, 0, 64, 64);
        assert_eq!(s.tile_count(), 0);
    }

    // ---- M2：mipmap 金字塔接进表面 ----

    /// 整块填满的写借出（模拟一个 dab 覆盖整块 tile）。
    fn fill_tile(s: &mut PigmentSurface, tx: i32, ty: i32, v: u16) {
        let t = s.tile_request_start(tx, ty);
        for row in t.iter_mut() {
            for p in row.iter_mut() {
                *p = [v, v, v, 1 << 15];
            }
        }
        s.tile_request_end(tx, ty);
    }

    /// level 1 左上象限（= 基 tile `(tx*2, ty*2)` 的那一块）的像素。
    fn l1(s: &PigmentSurface, tx: i32, ty: i32) -> Option<[u16; 4]> {
        s.mip_tile(1, tx, ty).map(|t| t[0][0])
    }

    /// 每一条写路径都必须让金字塔作废，否则远景会留下旧内容。
    ///
    /// 这是 M2 接入最容易漏的地方，所以按路径逐条钉住：
    /// dab 写借出 / 撤销快照 / 裁剪 / 平移 / 清空 / 整层替换。
    #[test]
    fn every_write_path_invalidates_the_pyramid() {
        let mut s = PigmentSurface::new();
        fill_tile(&mut s, 0, 0, 400);
        assert_eq!(l1(&s, 0, 0).unwrap()[0], 400);

        // 1) dab 写借出（tile_request_start 里 mark_dirty）
        fill_tile(&mut s, 0, 0, 800);
        assert_eq!(l1(&s, 0, 0).unwrap()[0], 800, "写借出后金字塔必须刷新");

        // 2) 撤销快照（load_snapshot 按 diff 标脏）
        let sshot = s.save_snapshot();
        fill_tile(&mut s, 0, 0, 1200);
        assert_eq!(l1(&s, 0, 0).unwrap()[0], 1200);
        s.load_snapshot(&sshot);
        assert_eq!(l1(&s, 0, 0).unwrap()[0], 800, "撤销后金字塔必须回到旧值");

        // 3) 裁剪（trim → invalidate_all）
        assert!(s.trim((0, 0, 64, 32)), "裁掉下半块");
        assert_eq!(
            s.mip_tile(1, 0, 0).unwrap()[20][0],
            [0, 0, 0, 0],
            "被裁掉的区域不能在远景里残留"
        );

        // 4) 平移（translate → invalidate_all）
        let mut t = PigmentSurface::new();
        fill_tile(&mut t, 0, 0, 400);
        assert!(t.mip_tile(1, 0, 0).is_some());
        assert!(t.translate(64, 0));
        assert_eq!(
            t.mip_tile(1, 0, 0).unwrap()[0][32],
            [400, 400, 400, 32768],
            "内容移到右边之后 level 1 必须跟着走"
        );

        // 5) 清空（clear → invalidate_all）
        assert!(s.clear());
        assert!(s.mip_tile(1, 0, 0).is_none(), "清空后远景必须是空的");

        // 6) 整层替换（replace_from_rgba8 先清旧 tile）
        let mut u = PigmentSurface::new();
        fill_tile(&mut u, 0, 0, 400);
        assert!(u.mip_tile(1, 0, 0).is_some());
        let red = [255u8, 0, 0, 255].repeat(64 * 64);
        u.replace_from_rgba8(&red, 128, 128, 64, 64);
        assert!(
            u.mip_tile(1, 0, 0).is_none(),
            "旧内容不能在远景里留鬼影（整块都空了）"
        );
        assert!(
            u.mip_tile(1, 1, 1).is_some(),
            "新位置（基 (2,2) → level1 (1,1)）有内容"
        );
    }

    /// `Clone` 不复制金字塔（纯缓存），但内容仍可通过重建拿到。
    #[test]
    fn clone_drops_the_pyramid_but_keeps_content() {
        let mut s = PigmentSurface::new();
        fill_tile(&mut s, 0, 0, 400);
        assert!(s.mip_tile(1, 0, 0).is_some(), "先让金字塔真的算出内容");
        assert!(s.mipmap_bytes() > 0, "金字塔应占像素内存");

        let c = s.clone();
        assert_eq!(c.mipmap_bytes(), 0, "克隆不该复制 32 KiB 级的缓存");
        assert_eq!(
            c.mip_tile(1, 0, 0).unwrap()[0][0],
            [400, 400, 400, 32768],
            "克隆之后按需重建，内容一致"
        );
        // 写时复制仍然独立：改原件不影响克隆
        fill_tile(&mut s, 0, 0, 1200);
        assert_eq!(c.mip_tile(1, 0, 0).unwrap()[0][0][0], 400);
    }

    /// `mip_tile_revision` 会把格子材料化，脏 → 已重建不会多出一次假变化。
    #[test]
    fn mip_revision_is_stable_after_materialising() {
        let mut s = PigmentSurface::new();
        fill_tile(&mut s, 0, 0, 400);
        let r1 = s.mip_tile_revision(1, 0, 0);
        assert!(r1 > 0);
        assert_eq!(s.mip_tile_revision(1, 0, 0), r1, "读两次版本相同");

        fill_tile(&mut s, 0, 0, 800);
        let r2 = s.mip_tile_revision(1, 0, 0);
        assert_ne!(r2, r1, "内容变了版本必须变");
        assert_eq!(s.mip_tile_revision(1, 0, 0), r2, "材料化之后稳定");
        // level 0 用基表 revision，与原来一致
        assert_eq!(s.mip_tile_revision(0, 0, 0), s.tile_entry(0, 0).unwrap().revision());
    }

    /// 增量补丁（`MipMap::patch_base`）与整块重建**逐字节一致**。
    ///
    /// 事件内的写入走 begin/end_atomic，结束时对每个改过的基格做子区域补丁；
    /// 参照物是同一表面的克隆——克隆不带金字塔，读取时只能从基表整块重建，
    /// 也就是"没有补丁这条捷径时"的结果。逐级逐格比较像素与"有无内容"。
    #[test]
    fn patched_pyramid_matches_full_rebuild() {
        let mut s = PigmentSurface::new();
        // 铺满 level 4 的一整格（16×16 个基 tile），并让金字塔全部物化。
        for ty in 0..16i32 {
            for tx in 0..16i32 {
                fill_tile(&mut s, tx, ty, 900);
            }
        }
        for level in 1..=mipmap::MAX_MIPMAP_LEVEL {
            let n = 16i32 >> level;
            for ty in 0..n {
                for tx in 0..n {
                    assert!(s.mip_tile(level, tx, ty).is_some(), "level {level} 应已物化");
                }
            }
        }

        // 12 个事件，每次只改若干基格里的几个像素（典型 dab 边缘）。
        for step in 0..12i32 {
            s.begin_atomic();
            for k in 0..3 {
                let tx = (step * 3 + k) % 16;
                let ty = (step * 5 + k) % 16;
                let t = <PigmentSurface as TiledSurface>::tile_request_start(&mut s, tx, ty);
                let y = ((step * 7 + k * 11) % 64) as usize;
                let x = ((step * 13 + k * 17) % 64) as usize;
                t[y][x] = [1000 + (step as u16) * 300, 4000, 800, 32768];
            }
            s.end_atomic();
        }

        // 参照物：克隆的金字塔是空的，读到什么都只能整块重建。
        let rebuilt = s.clone();
        // 补丁路径必须真的避开了整块重建：先在读之前记下计数，读完再比。
        let rebuilds_before = s.mip_rebuild_count();
        let mut compared = 0usize;
        for level in 1..=mipmap::MAX_MIPMAP_LEVEL {
            // 只比已经物化过的范围：范围外的格子在参照物里本来就要重建，
            // 拿它们比会把"读取即重建"的正常行为当成补丁失效。
            let n = 16i32 >> level;
            for ty in 0..n {
                for tx in 0..n {
                    let a = s.mip_tile(level, tx, ty);
                    let b = rebuilt.mip_tile(level, tx, ty);
                    match (a, b) {
                        (None, None) => {}
                        (Some(a), Some(b)) => {
                            for y in 0..hokusai::tile::TILE_SIZE {
                                for x in 0..hokusai::tile::TILE_SIZE {
                                    assert_eq!(
                                        a[y][x], b[y][x],
                                        "level {level} ({tx},{ty}) 像素 ({x},{y}) 不一致"
                                    );
                                }
                            }
                            compared += 1;
                        }
                        (a, b) => panic!(
                            "level {level} ({tx},{ty}) 内容存在性不一致：补丁={} 重建={}",
                            a.is_some(),
                            b.is_some()
                        ),
                    }
                }
            }
        }
        assert!(compared >= 4, "至少应比较到若干格（实际 {compared}）");
        assert_eq!(compared, 85, "四个层级共 85 格应全部比过");
        assert_eq!(
            s.mip_rebuild_count(),
            rebuilds_before,
            "补丁已经把金字塔保持为 Ready，读取不该再触发整块重建"
        );
    }
    /// `patch_many`（GPU 回写批量补丁）与「整座金字塔重建」逐字节一致。
    #[test]
    fn patch_many_matches_full_rebuild() {
        let mut s = PigmentSurface::new();
        for ty in 0..16i32 {
            for tx in 0..16i32 {
                fill_tile(&mut s, tx, ty, 900);
            }
        }
        // 物化整座金字塔（16×16 基格 → level 4 只有 1 格）。
        for level in 1..=mipmap::MAX_MIPMAP_LEVEL {
            let n = 16i32 >> level;
            for ty in 0..n.max(1) {
                for tx in 0..n.max(1) {
                    assert!(s.mip_tile(level, tx, ty).is_some(), "level {level} 应已物化");
                }
            }
        }

        // 一次改 24 块基格（大笔刷一事件的典型规模），坐标刻意扎堆，
        // 让多个基格共享同一个上级格——这正是去重逻辑要覆盖的情形。
        let mut coords: Vec<(i32, i32)> = Vec::new();
        let mut items: Vec<((i32, i32), Vec<u8>)> = Vec::new();
        for k in 0..24i32 {
            let (tx, ty) = ((k % 5) + 3, (k % 7) + 2);
            let mut px = vec![0u8; hokusai::tile::TILE_SIZE * hokusai::tile::TILE_SIZE * 8];
            // 一半写入新内容、一半保持全零，覆盖 Ready / Empty 两种结果。
            if k % 2 == 0 {
                for chunk in px.chunks_exact_mut(8) {
                    chunk[0..2].copy_from_slice(&((2000 + k * 100) as u16).to_ne_bytes());
                    chunk[2..4].copy_from_slice(&5000u16.to_ne_bytes());
                    chunk[6..8].copy_from_slice(&32768u16.to_ne_bytes());
                }
            }
            coords.push((tx, ty));
            items.push(((tx, ty), px));
        }
        let borrowed: Vec<((i32, i32), &[u8])> =
            items.iter().map(|(c, v)| (*c, v.as_slice())).collect();
        s.write_gpu_tiles(&borrowed);

        // 补丁结果留底，再整座重建，逐字节对比。
        let mut patched: Vec<(u32, i32, i32, Option<Vec<[u16; 4]>>)> = Vec::new();
        for level in 1..=mipmap::MAX_MIPMAP_LEVEL {
            let n = 16i32 >> level;
            for ty in 0..n.max(1) {
                for tx in 0..n.max(1) {
                    let px = s.mip_tile(level, tx, ty).map(|a| a.as_flattened().to_vec());
                    patched.push((level, tx, ty, px));
                }
            }
        }
        s.invalidate_mipmap();
        let mut compared = 0usize;
        for (level, tx, ty, want) in patched {
            let got = s.mip_tile(level, tx, ty).map(|a| a.as_flattened().to_vec());
            match (want, got) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert_eq!(a, b, "level {level} ({tx},{ty}) 与重建结果不一致");
                    compared += 1;
                }
                (a, b) => panic!(
                    "level {level} ({tx},{ty}) 存在性不一致：补丁={} 重建={}",
                    a.is_some(),
                    b.is_some()
                ),
            }
        }
        assert!(compared >= 4, "至少应比较到若干格（实际 {compared}）");
    }

    /// 预览覆盖：同一 mip 级别下，合成/内容判断/水位线都读预览面；
    /// 清除后回落到真实 mip，且播种是 Arc 共享（未写的格子不复制像素）。
    #[test]
    fn preview_overlay_shadows_mip_at_one_level_and_clears() {
        let mut s = PigmentSurface::new();
        let a = [1000u16, 2000, 3000, 20000];
        for ty in 0..4 {
            for tx in 0..4 {
                let t = s.tile_request_start(tx, ty);
                for row in t.iter_mut() {
                    for px in row.iter_mut() {
                        *px = a;
                    }
                }
                s.tile_request_end(tx, ty);
            }
        }
        // 让 level-2 的真实 mip 存在。
        let _ = s.mip_tile(2, 0, 0);
        let seeded_rev = s.mip_tile_revision_cached(2, 0, 0);
        let shared_ptr = Arc::as_ptr(&s.mip_tile(2, 0, 0).unwrap());

        let mut preview = s.seed_preview_at(2);
        // 未写过的格子共享同一份 Arc。
        assert_eq!(
            Arc::as_ptr(&preview.tile_pixels_arc(0, 0).unwrap()),
            shared_ptr,
            "播种必须共享像素，不能复制 32 KiB"
        );
        s.set_preview_overlay(2, preview);
        assert_eq!(s.preview_level(), Some(2));
        assert_eq!(s.mip_tile_revision_cached(2, 0, 0), seeded_rev);

        // 在预览面上写一个像素：合成读预览值，水位线变化。
        let b = [9000u16, 100, 200, 30000];
        {
            let p = s.preview_surface_mut().unwrap();
            let t = p.tile_request_start(0, 0);
            t[3][4] = b;
            p.tile_request_end(0, 0);
        }
        assert_eq!(s.mip_tile(2, 0, 0).unwrap()[3][4], b);
        assert!(s.mip_tile_revision_cached(2, 0, 0) > seeded_rev);

        // 真实 mip 未播种的格子里也能靠预览产生内容。
        {
            let p = s.preview_surface_mut().unwrap();
            let t = p.tile_request_start(5, 5);
            t[0][0] = b;
            p.tile_request_end(5, 5);
        }
        assert!(s.mip_has_content_cached(2, 5, 5), "预览笔画必须算内容");
        assert!(s.mip_tile_revision_cached(2, 5, 5) > 0);

        // 清除预览 → 回落到真实 mip。
        assert!(s.clear_preview_overlay());
        assert_eq!(s.mip_tile(2, 0, 0).unwrap()[3][4], a);
        assert!(!s.mip_has_content_cached(2, 5, 5));
    }
}

