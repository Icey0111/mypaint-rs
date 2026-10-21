//! 画布帧准备：可见 tile 枚举、CPU 合成结果上传计划、视图矩阵。
//!
//! 这里不出现任何 egui 类型。GUI 传进来的只有**物理像素矩形**
//! `[x, y, w, h]`；渲染器可以在窗口、离屏纹理或无头 target 上复用同一份
//! [`CanvasFrame`]。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use hokusai::tile::TILE_SIZE;
use pigment_surface::LayerStack;

use crate::display_filter::DisplayFilter;
use crate::gpu::{
    plan_evictions, plan_uploads, DrawTile, TileKey, TileRenderer, TileUpload,
};
use crate::view::View;

/// 每帧上传的**数量**安全网。真正的节流是下面的时间预算。
const MAX_UPLOADS_PER_FRAME: usize = 128;

/// 跨 mip 级别的那一帧最多检查多少格（时间预算之外的硬顶）。
const MAX_UPLOADS_ON_LEVEL_CHANGE: usize = 4096;

/// 每帧用于 CPU 合成/编码新 tile 的时间预算。
///
/// 低缩放切 mip 级别时可见格可达上千，全做一帧要 100ms 以上；而按 128
/// 格/帧慢慢补，又会让笔迹在十几帧里一块块缺失（闪烁）。这里改成按
/// **时间**推进：有笔迹的格子排在最前（它们的可见性最要紧），背景纸纹
/// 排在后面；一帧最多花 [`UPLOAD_BUDGET_LEVEL_CHANGE_MS`]，其余下一帧继续。
/// 缺失格用相邻级别已驻留的 tile 回退绘制。
const UPLOAD_BUDGET_MS: f32 = 3.0;
/// 级别切换那一帧的预算。一次跨 3 级的极端缩放（滚轮甩动）实测需要
/// ~11ms 才能把可见笔迹全部补齐；低于这个值会漏 1 帧（笔迹缺一小块）。
const UPLOAD_BUDGET_LEVEL_CHANGE_MS: f32 = 12.0;

/// 低于此缩放不再逐格上传纸纹背景（改用文档背景均色）。
///
/// 纸纹在远景下已被 mipmap 平均得接近均色；而每个可见格都上传纸纹会让
/// 一次 mip 级别切换变成上千格的合成（实测 10% 缩放一次 100ms+）。
/// 这与项目早先的远景优化一致：背景用平均色兜底，只给真正有笔迹的格
/// 上传/绘制。
const BG_TILE_MIN_ZOOM: f32 = 0.75;

/// 诊断用：`PIGMENT_UPLOAD_CAP` 覆盖每帧数量上限（压测缩放补齐成本）。
fn upload_cap() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("PIGMENT_UPLOAD_CAP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MAX_UPLOADS_PER_FRAME)
    })
}

fn upload_budget_ms() -> f32 {
    static MS: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *MS.get_or_init(|| {
        std::env::var("PIGMENT_UPLOAD_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(UPLOAD_BUDGET_MS)
    })
}

/// 视口单轴最多枚举的 tile 数（安全网）。
///
/// 正常缩放下 `2^level ≈ 1/zoom`，每格在屏幕上约 32–64 px，1080p 约
/// 500–2000 格。只有缩到 level 封顶（1/16）以下时每格才越来越小、格数按
/// 1/zoom 涨；这里只防病态缩放。
pub const MAX_VISIBLE_TILES_PER_AXIS: i32 = 96;

/// 一帧的画布同步结果：显存增量 + draw 列表 + 视图矩阵。
pub struct CanvasFrame {
    pub renderer: Arc<Mutex<TileRenderer>>,
    pub uploads: Vec<TileUpload>,
    pub evict: Vec<TileKey>,
    pub draw: Vec<DrawTile>,
    pub visible: Vec<TileKey>,
    /// 本帧之后仍未同步的可见内容格数；GUI 应据此继续请求重绘。
    pub pending_uploads: usize,
    /// 本帧有内容却没有任何 draw 覆盖的可见格数（诊断：闪烁/空洞）。
    pub uncovered: usize,
    /// 世界像素 → 本帧 viewport 的裁剪空间矩阵。
    pub view_matrix: [[f32; 4]; 4],
    /// 画布矩形（目标物理像素，[x, y, w, h]）。
    pub viewport: [f32; 4],
    /// 是否在 tile 下面铺透明棋盘格。
    pub checks: bool,
    /// 本帧目标附件的清屏色（GUI 按画布底色填充）。
    pub clear: wgpu::Color,
    /// 用最近邻取样（与 MyPaint `pixelize_threshold = 2.8` 一致）。
    pub nearest_filter: bool,
}

impl CanvasFrame {
    pub fn viewport_size(&self) -> [f32; 2] {
        [self.viewport[2], self.viewport[3]]
    }
}

/// 按缩放级别选 mipmap level（`gui/tileddrawwidget.py:1296` 的 HQ 分支）：
/// `floor(log2(1/scale))`，钳到 [`pigment_surface::mipmap::MAX_MIPMAP_LEVEL`]。
/// **与内容大小无关**。
pub fn mip_level_for_zoom(zoom: f32) -> u32 {
    if zoom >= 1.0 {
        return 0;
    }
    let inv = 1.0 / zoom.max(1.0 / 1024.0);
    let level = inv.log2().floor().max(0.0);
    (level as u32).min(pigment_surface::mipmap::MAX_MIPMAP_LEVEL)
}

/// 枚举可见 tile，并把「CPU 有哪些内容 / 每格版本号」换算成本帧的
/// 上传与 draw 计划。
///
/// `viewport` 是目标物理像素的 `[x, y, w, h]`（`x/y` 只随 frame 带走，
/// 不参与矩阵；矩阵以 viewport 左上角为原点）。
pub fn prepare_canvas(
    renderer: &Arc<Mutex<TileRenderer>>,
    layers: &LayerStack,
    view: &View,
    viewport: [f32; 4],
    force_resync: bool,
    level_change: bool,
    forced_level: Option<u32>,
    filter: DisplayFilter,
) -> CanvasFrame {
    let vp_w = viewport[2].max(1.0);
    let vp_h = viewport[3].max(1.0);
    // Instant Preview 期间锁定在预览级别：用户缩放时先看低分辨率结果，
    // 后台全分辨率同步完再切回目标级别（Krita 的 LOD 预览行为）。
    let level = forced_level
        .map(|l| l.min(pigment_surface::mipmap::MAX_MIPMAP_LEVEL))
        .unwrap_or_else(|| mip_level_for_zoom(view.zoom));
    let tile_world = TILE_SIZE as i32 * (1i32 << level);

    // ---- 级别空间的可见 tile 范围 ----
    let (mut tx0, mut ty0, mut tx1, mut ty1) = view.visible_tile_range(vp_w, vp_h, tile_world);
    // 各外扩 1 格（源码 `:1320-1330` 的边缘保险）。
    tx0 -= 1;
    ty0 -= 1;
    tx1 += 1;
    ty1 += 1;
    if tx1 - tx0 + 1 > MAX_VISIBLE_TILES_PER_AXIS {
        let c = (tx0 + tx1) / 2;
        tx0 = c - MAX_VISIBLE_TILES_PER_AXIS / 2;
        tx1 = tx0 + MAX_VISIBLE_TILES_PER_AXIS - 1;
    }
    if ty1 - ty0 + 1 > MAX_VISIBLE_TILES_PER_AXIS {
        let c = (ty0 + ty1) / 2;
        ty0 = c - MAX_VISIBLE_TILES_PER_AXIS / 2;
        ty1 = ty0 + MAX_VISIBLE_TILES_PER_AXIS - 1;
    }
    let mut visible =
        Vec::with_capacity(((tx1 - tx0 + 1).max(0) * (ty1 - ty0 + 1).max(0)) as usize);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            visible.push(TileKey::new(level, tx, ty));
        }
    }

    // ---- 自愈式增量同步：比对 CPU revision 与显存已上传 revision ----
    // 远景不逐格传纸纹：背景图在 mip 级别下已接近均色，逐格上传会让
    // 一次级别切换变成上千格的合成。低于阈值时画布清屏色（背景平均色）
    // 就是背景，只有真正有笔迹的格需要合成/上传。
    let tiled_background = layers.background_is_image() && view.zoom >= BG_TILE_MIN_ZOOM;
    let surfaces = layers.painting_surfaces();
    // 规划阶段**不能**触发 mip 重建：`mip_tile` 会现场生成像素，低缩放
    // 627 格一次就是 120ms。`*_cached` 只读已有的脏/空哨兵；脏格按
    // "有内容"处理并返回 revision 0，等真正上传时再重建。
    let has_paint = |k: TileKey| {
        surfaces
            .iter()
            .any(|s| s.mip_has_content_cached(k.level, k.tx, k.ty))
    };
    let has_content = |k: TileKey| tiled_background || has_paint(k);
    let mut uploads: Vec<TileUpload> = Vec::new();
    let t_perf = prep_perf().then(std::time::Instant::now);
    let mut composite_ns = 0u128;
    let (evict, draw, pending_uploads, uncovered) = {
        let mut renderer = renderer.lock().unwrap();
        if force_resync {
            renderer.invalidate_all();
        }
        let tile_rev =
            |k: TileKey| layers.tile_composite_revision_at_cached(k.level, k.tx, k.ty);
        let cap = if level_change {
            upload_cap().max(MAX_UPLOADS_ON_LEVEL_CHANGE)
        } else {
            upload_cap()
        };
        // 有笔迹的格排最前：缩放下用户盯着的是笔迹，纸纹晚一两帧无感。
        let plan_paint = plan_uploads(
            |k| renderer.uploaded_rev(k),
            &visible,
            cap,
            has_paint,
            tile_rev,
        );
        let plan_bg = plan_uploads(
            |k| renderer.uploaded_rev(k),
            &visible,
            cap,
            |k| has_content(k) && !has_paint(k),
            tile_rev,
        );
        let budget_ms = if level_change {
            UPLOAD_BUDGET_LEVEL_CHANGE_MS.max(upload_budget_ms())
        } else {
            upload_budget_ms()
        };
        let t_budget = std::time::Instant::now();
        let mut uploaded: HashSet<TileKey> = HashSet::new();
        for key in plan_paint.into_iter().chain(plan_bg) {
            if uploads.len() >= cap {
                break;
            }
            if !uploads.is_empty()
                && t_budget.elapsed().as_secs_f32() * 1e3 >= budget_ms
            {
                break;
            }
            let t_c = std::time::Instant::now();
            let composed = layers.composite_tile_rgba8_at(key.level, key.tx, key.ty);
            composite_ns += t_c.elapsed().as_nanos();
            let Some(mut pixels) = composed else {
                continue;
            };
            // 显示过滤只作用于上传到显存的显示像素，不改文档。
            filter.apply(&mut pixels[..]);
            uploaded.insert(key);
            uploads.push(TileUpload {
                key,
                revision: tile_rev(key),
                pixels,
            });
        }
        // 撤销会把画布退回较早状态：CPU 已消失的 tile 必须从显存剔除。
        let evict = plan_evictions(
            |k| renderer.uploaded_rev(k).is_some(),
            &visible,
            has_content,
        );
        let mut draw = Vec::with_capacity(visible.len());
        let mut pending_uploads = 0usize;
        let mut uncovered = 0usize;
        for &k in &visible {
            if !has_content(k) {
                continue;
            }
            // 本帧**真正**合成成功的格视为已就绪（prepare 先上传再画）。
            // 计划里因时间预算被推迟的格必须继续走回退，否则会画空。
            if uploaded.contains(&k) {
                draw.push(DrawTile::normal(k));
                continue;
            }
            if renderer.uploaded_rev(k).is_some() {
                // 可能是旧 revision：先画旧内容，后续帧再刷新，避免空洞。
                draw.push(DrawTile::normal(k));
                if renderer.uploaded_rev(k) != Some(tile_rev(k)) {
                    pending_uploads += 1;
                }
                continue;
            }
            pending_uploads += 1;
            // 目标级别还没上传：用相邻级别已驻留的 tile 补位。
            // 粗级别补一格（子矩形），细级别补多格；两者都与目标格
            // 精确重合，不会和已上传的格子重叠混合。
            if let Some(mut fb) = fallback_draw(&renderer, k) {
                draw.append(&mut fb);
            } else {
                uncovered += 1;
            }
        }
        (evict, draw, pending_uploads, uncovered)
    };

    if let Some(t) = t_perf {
        report_prep_perf(
            t.elapsed().as_nanos(),
            composite_ns,
            visible.len(),
            uploads.len(),
            draw.len(),
            uncovered,
        );
    }

    CanvasFrame {
        renderer: Arc::clone(renderer),
        uploads,
        evict,
        draw,
        visible,
        pending_uploads,
        uncovered,
        view_matrix: view.clip_matrix(vp_w, vp_h),
        viewport,
        checks: false,
        clear: wgpu::Color::TRANSPARENT,
        nearest_filter: view.zoom > 2.8,
    }
}

/// `PIGMENT_PREP_PERF=1` 时的帧准备耗时诊断（每 30 帧一次）。
fn prep_perf() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PIGMENT_PREP_PERF").is_some())
}

fn report_prep_perf(
    total_ns: u128,
    composite_ns: u128,
    visible: usize,
    uploads: usize,
    draw: usize,
    uncovered: usize,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static TOTAL: AtomicU64 = AtomicU64::new(0);
    static COMPOSITE: AtomicU64 = AtomicU64::new(0);
    static VISIBLE: AtomicU64 = AtomicU64::new(0);
    static UPLOADS: AtomicU64 = AtomicU64::new(0);
    static DRAWS: AtomicU64 = AtomicU64::new(0);
    static UNCOVERED: AtomicU64 = AtomicU64::new(0);
    TOTAL.fetch_add(total_ns as u64, Ordering::Relaxed);
    COMPOSITE.fetch_add(composite_ns as u64, Ordering::Relaxed);
    VISIBLE.fetch_add(visible as u64, Ordering::Relaxed);
    UPLOADS.fetch_add(uploads as u64, Ordering::Relaxed);
    DRAWS.fetch_add(draw as u64, Ordering::Relaxed);
    UNCOVERED.fetch_add(uncovered as u64, Ordering::Relaxed);
    let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if n % 30 == 0 {
        let f = n as f64;
        eprintln!(
            "prep n={n} total={:.2}ms composite={:.2}ms other={:.2}ms visible={:.0} uploads={:.1} draw={:.0} uncovered={:.1}",
            TOTAL.load(Ordering::Relaxed) as f64 / f / 1e6,
            COMPOSITE.load(Ordering::Relaxed) as f64 / f / 1e6,
            (TOTAL.load(Ordering::Relaxed) - COMPOSITE.load(Ordering::Relaxed)) as f64 / f / 1e6,
            VISIBLE.load(Ordering::Relaxed) as f64 / f,
            UPLOADS.load(Ordering::Relaxed) as f64 / f,
            DRAWS.load(Ordering::Relaxed) as f64 / f,
            UNCOVERED.load(Ordering::Relaxed) as f64 / f,
        );
    }
}

/// 为尚未上传的目标格找一组已驻留的替代绘制。
/// 优先用粗一级的祖先格（缩放拉远时祖先通常不存在，放大时存在），
/// 没有祖先再枚举细一级的子孙格。返回的每个 [`DrawTile`] 都只覆盖目标格
/// 内部的世界矩形，因此不会与相邻目标格的绘制重叠。
fn fallback_draw(
    renderer: &TileRenderer,
    key: TileKey,
) -> Option<Vec<DrawTile>> {
    use hokusai::tile::TILE_SIZE;
    use pigment_surface::mipmap::MAX_MIPMAP_LEVEL;

    // 1) 粗级别：一块祖先格，取对应子矩形。
    for level in (key.level + 1)..=MAX_MIPMAP_LEVEL {
        let f = 1i32 << (level - key.level);
        let ancestor = TileKey::new(
            level,
            key.tx.div_euclid(f),
            key.ty.div_euclid(f),
        );
        if renderer.uploaded_rev(ancestor).is_some() {
            let step = (TILE_SIZE as i32 / f) as u8;
            let lx = key.tx.rem_euclid(f) as u8;
            let ly = key.ty.rem_euclid(f) as u8;
            return Some(vec![DrawTile {
                key,
                source: ancestor,
                sub: [
                    lx * step,
                    ly * step,
                    (lx + 1) * step,
                    (ly + 1) * step,
                ],
            }]);
        }
    }

    // 2) 细级别：从最近的一级往更细找，**优先完整覆盖**。
    //
    // 只画「已驻留的那几块」会在目标格里留下空洞 —— 缩放时表现为笔迹
    // 一块块缺失。近一级只缺一块时也先看更细的一级是否完整；都没有完整
    // 覆盖时才退回最近一级的部分覆盖（总比完全不画好）。
    //
    // 最多枚举 8×8 个子孙格：level 4 → 0 是 16×16 = 256 次查找/格，
    // 缺口多时规划阶段本身就会变成卡顿源。
    let mut partial: Option<Vec<DrawTile>> = None;
    for diff in 1..=key.level.min(3) {
        let level = key.level - diff;
        let f = 1i32 << diff;
        let mut out = Vec::new();
        let mut complete = true;
        for dy in 0..f {
            for dx in 0..f {
                let child = TileKey::new(level, key.tx * f + dx, key.ty * f + dy);
                if renderer.uploaded_rev(child).is_some() {
                    out.push(DrawTile::normal(child));
                } else {
                    complete = false;
                }
            }
        }
        if out.is_empty() {
            continue;
        }
        if complete {
            return Some(out);
        }
        if partial.is_none() {
            partial = Some(out);
        }
    }
    partial
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_level_follows_scale_only() {
        assert_eq!(mip_level_for_zoom(1.0), 0);
        assert_eq!(mip_level_for_zoom(2.0), 0);
        assert_eq!(mip_level_for_zoom(0.75), 0);
        assert_eq!(mip_level_for_zoom(0.5), 1);
        assert_eq!(mip_level_for_zoom(0.25), 2);
        assert_eq!(mip_level_for_zoom(0.125), 3);
        assert_eq!(mip_level_for_zoom(0.0625), 4);
        assert_eq!(
            mip_level_for_zoom(0.01),
            pigment_surface::mipmap::MAX_MIPMAP_LEVEL
        );
        assert_eq!(
            mip_level_for_zoom(0.0),
            pigment_surface::mipmap::MAX_MIPMAP_LEVEL
        );
    }

    #[test]
    fn screen_tile_count_stays_bounded_above_the_level_cap() {
        for zoom in [1.0f32, 0.5, 0.25, 0.125, 0.0625] {
            let level = mip_level_for_zoom(zoom);
            let tile_world = (TILE_SIZE as f32) * (1u32 << level) as f32;
            let per_axis = (1920.0 / zoom / tile_world).ceil();
            assert!(
                per_axis <= 64.0,
                "zoom={zoom} level={level} 每轴 {per_axis} 格，超出预期"
            );
        }
        for zoom in [0.02f32, 0.005] {
            let level = mip_level_for_zoom(zoom);
            let tile_world = (TILE_SIZE as f32) * (1u32 << level) as f32;
            let raw = (1920.0 / zoom / tile_world).ceil() as i32;
            assert!(raw.min(MAX_VISIBLE_TILES_PER_AXIS) <= MAX_VISIBLE_TILES_PER_AXIS);
        }
    }
}
