//! 逐 tile 的 mipmap 金字塔 —— `lib/tiledsurface.py:159-361` 的 Rust 对应物。
//!
//! # 为什么是「逐 tile」而不是「整幅缩略图」
//!
//! MyPaint 的远景渲染走的是**每个表面自带一份 mipmap 金字塔**
//! （`MyPaintSurface._create_mipmap_surfaces`，`tiledsurface.py:159-181`）：
//! level 0 是基表面，level 1..N 各自是"一个表面对象"，只是它的 tiledict
//! 每个 tile 的内容来自上一级 2×2 个 tile 的降采样（`_regenerate_mipmap`，
//! `:297-322`）。渲染时按缩放级别**直接选层 blit tile**，不需要为整幅画
//! 生成一张缩略图 —— 视口外的内容根本不参与计算，这是无限画布在远景下
//! 仍然能跑起来的根本原因（对比 [`crate::thumb`] 的整幅缩略图路径）。
//!
//! 本模块只负责金字塔这一层：数据结构 + 惰性重建 + 脏传播。上层（M2 合成、
//! M3 远景屏幕 tile 循环、M4 损伤区域）另行接入。
//!
//! # 与源码的对应关系
//!
//! | Rust | 源码 |
//! | --- | --- |
//! | [`MipMap::levels`] | `_create_mipmap_surfaces` 的 `mipmaps` 列表（`:166-181`）|
//! | [`MipTile`] 的 `Dirty` 状态 | 哨兵 `mipmap_dirty_tile`（`:86-88`）|
//! | [`MipMap::mark_dirty`] | `_mark_mipmap_dirty`（`:355-365`）|
//! | [`MipMap::ensure`] | `_regenerate_mipmap`（`:297-322`）|
//! | [`MipMap::tile_at`] | `_get_tile_numpy` 的 `mipmap_dirty_tile` 分支（`:341-342`）|
//! | [`MipMap::invalidate_all`] | `clear()` 里的 `if self.mipmap: self.mipmap.clear()`（`:198-202`）|
//!
//! # 数值语义
//!
//! 降采样逐通道 `v00/4 + v01/4 + v10/4 + v11/4` —— **各自整除后再相加**，
//! 不是先求和再除（`lib/pixops.cpp:31-46` `tile_downscale_rgba16_c`）。
//! 全部在 fix15（u16、预乘 alpha）整数域完成；预乘语义天然适配平均，
//! 反预乘再平均反而会在透明边缘引入误差。
//!
//! # 层数上限
//!
//! 与源码一致取 4（`_libmypaint_ref/mypaint-config.h:13
//! #define MYPAINT_MAX_MIPMAP_LEVEL 4`，`tiledsurface.hpp:35` 直接引用，
//! `tiledsurface.py:43` 再导出）。远景缩到 1/16 以下时 level 被钳到 4，
//! 剩余 4 倍以内的缩小交给纹理插值 —— 与 `gui/tileddrawwidget.py:1301` 的
//! `min(mipmap_level, MAX_MIPMAP_LEVEL)` 完全一致。
//!
//! # 唯一一处有意偏离
//!
//! **「空」按内容判定，而不是按对象同一性**。源码 `_regenerate_mipmap` 用
//! `src.rgba is not transparent_tile.rgba` 判非空（`:316`），所以"存在但
//! 全零"的子 tile 会让父级保留一个全零 tile。这里改成"降采样结果全零就
//! 记成 [`MipState::Empty`]"：渲染结果完全相同（全零 tile 与缺席在读取端
//! 等价），但不会残留 32 KiB 的全零条目。

use std::collections::HashMap;

use rayon::prelude::*;
use std::sync::Arc;

use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};

use crate::Tile;

/// mipmap 层数上限：level 1..=4。
///
/// 与源码常量一致（`mypaint-config.h:13`）。一个 64×64 基 tile 在 level 4
/// 覆盖 16×16 个基 tile（1024×1024 模型像素），内容退化为 4×4 px；
/// 更远的缩小交给纹理插值（`tileddrawwidget.py:1301` 的钳位语义）。
pub const MAX_MIPMAP_LEVEL: u32 = 4;

/// tile 边长的一半：降采样时每个子 tile 占据目标 tile 的一个 `32×32` 象限
/// （`_regenerate_mipmap` 里的 `x * N // 2` / `y * N // 2`，`:314`）。
const HALF: usize = TILE_SIZE / 2;

/// 金字塔里一格的三态。
///
/// 源码用一个"没有 rgba 的 `_Tile`"当哨兵（`mipmap_dirty_tile`，`:86-88`），
/// 并且"缺席"与"已知空"是两种不同的状态：
///
/// - `Dirty` = 存在但像素无效（哨兵）→ 读的时候必须重建；
/// - `Empty` = 已知四子全空 → 读到就返回 `None`，**永不重建**
///   （源码是 `del self.tiledict[...]` 后返回 `transparent_tile`，
///   `:318-321`；缺席在 `_get_tile_numpy` 里也是"直接给透明 tile 不重建"）；
/// - `Ready` = 有像素。
///
/// 三态是必须的：只把"空"表示成 `Dirty` 会让空白坐标每次读取都重跑一遍
/// 4 次哈希查找 + 递归检查，远景屏幕 tile 循环一帧读上千个格子、空白占比
/// 很大，那会成为热点。
#[derive(Clone)]
pub enum MipState {
    /// 脏哨兵：需要（重新）降采样。
    Dirty,
    /// 已知空：四子全空，读到就是 `None`。
    Empty,
    /// 可用像素。
    Ready(Arc<TilePixels>),
}

/// 金字塔里的一格：状态 + 版本号。
#[derive(Clone)]
pub struct MipTile {
    state: MipState,
    /// 内容版本号：每次真正重建时取 [`MipMap::clock`]（供 GPU 纹理镜像做增量上传）。
    revision: u64,
}

impl Default for MipTile {
    fn default() -> Self {
        Self::dirty()
    }
}

impl MipTile {
    /// 脏哨兵（`mipmap_dirty_tile`）。
    #[inline]
    pub fn dirty() -> Self {
        Self {
            state: MipState::Dirty,
            revision: 0,
        }
    }

    /// 已知空（源码的 `transparent_tile` 缺席态）。
    #[inline]
    pub fn empty() -> Self {
        Self {
            state: MipState::Empty,
            revision: 0,
        }
    }

    /// 是否是脏哨兵（像素无效、等待重建）。
    #[inline]
    pub fn is_dirty(&self) -> bool {
        matches!(self.state, MipState::Dirty)
    }

    /// 是否已知为空（读到就是 `None`，不再触发重建）。
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self.state, MipState::Empty)
    }

    /// 当前状态。
    #[inline]
    pub fn state(&self) -> &MipState {
        &self.state
    }

    /// 可用像素；`Dirty` / `Empty` 返回 `None`。
    #[inline]
    pub fn pixels(&self) -> Option<&TilePixels> {
        match &self.state {
            MipState::Ready(px) => Some(px),
            _ => None,
        }
    }

    /// 共享句柄；`Dirty` / `Empty` 返回 `None`。
    #[inline]
    pub fn pixels_arc(&self) -> Option<&Arc<TilePixels>> {
        match &self.state {
            MipState::Ready(px) => Some(px),
            _ => None,
        }
    }

    /// 内容版本号（0 = 从未重建 / 已知空）。
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// per-tile mipmap 金字塔：`levels[i]` 存 **level `i + 1`** 的 tile 字典。
///
/// level 0 不在这里存 —— 那就是调用方手里的基表
/// （[`crate::PigmentSurface`] 的 `tiles`），所有接口都以 `base` 参数传入。
/// 这样做的好处是 level 0 的写入路径（`tile_request_start` 的写时复制、
/// revision 戳记）完全不受 mipmap 影响，行为与像素结果保持原样。
///
/// 源码里每一级都是一个完整的 `MyPaintSurface` 对象（带 `parent` / `mipmap`
/// 双向指针，`:171-181`）。Rust 侧用"下标即层级"的 `Vec` 表达同一张链：
/// `levels[k]` 的父级是 `levels[k-1]`（`k == 0` 时父级是 `base`），
/// 不需要指针，也就没有借用检查的环形引用问题。
pub struct MipMap {
    /// `levels[i]` = level `i + 1` 的 tile 字典。
    levels: Vec<HashMap<(i32, i32), MipTile>>,
    /// 重建计数器：每次真正重建一个 tile 递增，作为该 tile 的 `revision`。
    clock: u64,
    /// 累计重建次数（只增不减，测试/性能观测用）。
    rebuilds: u64,
}

impl Default for MipMap {
    fn default() -> Self {
        let mut levels = Vec::with_capacity(MAX_MIPMAP_LEVEL as usize);
        for _ in 0..MAX_MIPMAP_LEVEL {
            levels.push(HashMap::new());
        }
        Self {
            levels,
            clock: 0,
            rebuilds: 0,
        }
    }
}

impl MipMap {
    /// 空金字塔（所有层级都是空字典，等价 `_create_mipmap_surfaces`）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 重建计数器当前值（GPU 侧记录同步点用）。
    #[inline]
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// 累计重建了多少个格子（只增不减）。
    ///
    /// 用来证明"已知空的坐标不会反复重建"：远景屏幕 tile 循环里空白区域
    /// 占多数，重复重建会直接变成帧时间。
    #[inline]
    pub fn rebuild_count(&self) -> u64 {
        self.rebuilds
    }

    /// level 相对基表的坐标缩放因子：`2^level`（level 0/越界都会被钳位）。
    ///
    /// 对应 `_mark_mipmap_dirty` 里的 `fac = 2 ** level`（`:362`）。
    #[inline]
    pub fn level_scale(level: u32) -> i32 {
        1i32 << level.min(MAX_MIPMAP_LEVEL)
    }

    /// 基表坐标 → level 层坐标（向下取整，负数同样向下）。
    ///
    /// Python 的 `tx // fac` 对负数是向下取整，Rust 侧用 `div_euclid`
    /// （除数为正时二者等价），所以 `(-1, -1)` 在 level 1 落到 `(-1, -1)`
    /// 而不是 `(0, 0)`。
    #[inline]
    pub fn to_level_coord(level: u32, tx: i32, ty: i32) -> (i32, i32) {
        let fac = Self::level_scale(level);
        (tx.div_euclid(fac), ty.div_euclid(fac))
    }

    /// 读一个 tile 的像素；惰性重建。全透明（或四子全空）返回 `None`。
    ///
    /// 对应 `_get_tile_numpy` 的惰性分支（`:341-342`）：
    ///
    /// ```python
    /// if t is mipmap_dirty_tile:
    ///     t = self._regenerate_mipmap(t, tx, ty)
    /// ```
    ///
    /// `base` 是 level 0 的 tile 字典（[`crate::PigmentSurface`] 的 `tiles`）。
    /// `level == 0` 直接透传基表；`level > MAX_MIPMAP_LEVEL` 钳位到上限
    /// （对齐 `gui/tileddrawwidget.py:1301` 的 `min(mipmap_level, MAX)`）。
    ///
    /// 两个生命周期绑在一起是刻意的：`level == 0` 时返回值借用 `base`，
    /// 其余级别借用 `self`，调用方必须把二者当成互斥借用（实践上就是把
    /// `surface.tiles` 与 `surface.mipmap` 分开借）。
    pub fn tile_at<'a>(
        &'a mut self,
        level: u32,
        tx: i32,
        ty: i32,
        base: &'a HashMap<(i32, i32), Tile>,
    ) -> Option<&'a TilePixels> {
        if level == 0 {
            return base.get(&(tx, ty)).map(|t| t.pixels());
        }
        let level = level.min(MAX_MIPMAP_LEVEL);
        self.ensure(level, tx, ty, base);
        self.levels[(level - 1) as usize]
            .get(&(tx, ty))
            .and_then(|t| t.pixels())
    }

    /// 同 [`Self::tile_at`]，但返回共享句柄（`Arc` 克隆）。
    ///
    /// 给渲染层用：GPU 上传要把像素交给另一条借用链，拿着 `&TilePixels`
    /// 会和 `&mut self` 打架，`Arc` 克隆（引用计数 +1，不复制像素）更顺手。
    pub fn tile_arc(
        &mut self,
        level: u32,
        tx: i32,
        ty: i32,
        base: &HashMap<(i32, i32), Tile>,
    ) -> Option<Arc<TilePixels>> {
        if level == 0 {
            // level 0 的像素在基表里同样是 `Arc`，直接克隆即可。
            return base.get(&(tx, ty)).map(|t| Arc::clone(t.pixels_arc()));
        }
        let level = level.min(MAX_MIPMAP_LEVEL);
        self.ensure(level, tx, ty, base);
        self.levels[(level - 1) as usize]
            .get(&(tx, ty))
            .and_then(|t| t.pixels_arc().map(Arc::clone))
    }

    /// 读一格的原始条目（可能是脏哨兵 / 已知空），供渲染层查 revision。
    #[inline]
    pub fn tile_entry_at(&self, level: u32, tx: i32, ty: i32) -> Option<&MipTile> {
        if level == 0 || level > MAX_MIPMAP_LEVEL {
            return None;
        }
        self.levels[(level - 1) as usize].get(&(tx, ty))
    }

    /// 把基表坐标 `(tx, ty)` 在所有层级标记为脏。
    ///
    /// 对应 `_mark_mipmap_dirty`（`:355-365`）。level L 的坐标是
    /// `floor(tx / 2^L)` —— 一个基 tile 的改动会影响**所有**祖先 tile，
    /// 因为每一级都是整块平均。
    ///
    /// 源码在"已经是脏哨兵"时 `break`（`:363-364`）：脏传播永远自下而上，
    /// 所以某个层级已脏意味着它上面的层级也一定已脏 —— 这个不变式在本模块
    /// 所有路径（重建只清自己那一格、[`Self::invalidate_all`] 全清）下都成立，
    /// 因此提前退出不会漏标。
    ///
    /// 注意 `Empty` **不能**当作脏来 `break`：它是"上一次算出来是空的"
    /// 这个结论，本次写入可能让它有内容，必须改回 `Dirty`。
    pub fn mark_dirty(&mut self, tx: i32, ty: i32) {
        // 每个祖先都要拿到一个**新的** revision，即使它已经是脏哨兵。
        //
        // 上游的 `break` 只服务"是否脏"这个布尔判断；GPU 同步把 revision
        // 当水位线，必须每次写入都变化。否则：
        // - 同一 mip 格内的第二次 dab 会复用旧 revision → 不重传（断断续续）；
        // - 背景格的初始水位是 0，脏哨兵也是 0 → 永远判成已同步（画完不显示）。
        self.clock += 1;
        let revision = self.clock;
        for level in 1..=MAX_MIPMAP_LEVEL {
            let fac = Self::level_scale(level);
            let coord = (tx.div_euclid(fac), ty.div_euclid(fac));
            let slot = &mut self.levels[(level - 1) as usize];
            match slot.get_mut(&coord) {
                Some(entry) if entry.is_dirty() => {
                    entry.revision = revision;
                }
                Some(entry) => {
                    *entry = MipTile {
                        state: MipState::Dirty,
                        revision,
                    };
                }
                None => {
                    slot.insert(
                        coord,
                        MipTile {
                            state: MipState::Dirty,
                            revision,
                        },
                    );
                }
            }
        }
    }

    /// 一次事件结束时，用**增量补丁**代替整座金字塔的标脏。
    ///
    /// # 为什么
    ///
    /// [`Self::mark_dirty`] 只标脏，真正的重建发生在读取时（[`Self::ensure`]）。
    /// 重建一格 level-4 要把 64 个 level-1、16 个 level-2、4 个 level-3 连同
    /// 自己全部重算一遍——实测 34.8 万次像素运算，约 1.1ms。而一次 dab 只可能
    /// 改动其中几格里的**几个像素**：一块基 tile 的变化在 level 4 上只影响
    /// 一个 4×4 的方块。
    ///
    /// 这里沿变化区域自下而上重算：64×64（基 tile）→ 32×32 → 16×16 → 8×8
    /// → 4×4，合计 1360 次像素运算（约 250 分之一），写入目标格的那一块。
    /// 每一级的 2×2 降采样公式、通道顺序、`/4 后相加` 的定点语义都与
    /// [`Self::ensure`] 逐字相同，因此结果与整块重建**逐字节一致**。
    ///
    /// # 何时不适用
    ///
    /// 补丁要求四级祖先都已经有过像素（`Ready`）。任一格是脏哨兵或「已知空」
    /// 时没有可增量修改的目标，退回 [`Self::mark_dirty`]，交给 `ensure` 整块
    /// 重建——真正绘制过的地方只有第一次会走这条慢路径。
    pub fn patch_base(&mut self, tx: i32, ty: i32, base: &HashMap<(i32, i32), Tile>) {
        // 先确认四级祖先都能增量修改；有任何一个不能就整条链退回标脏。
        let ready = (1..=MAX_MIPMAP_LEVEL).all(|level| {
            let fac = 1i32 << level;
            self.levels[(level - 1) as usize]
                .get(&(tx.div_euclid(fac), ty.div_euclid(fac)))
                .is_some_and(|t| matches!(t.state, MipState::Ready(_)))
        });
        if !ready {
            self.mark_dirty(tx, ty);
            return;
        }

        self.clock += 1;
        let revision = self.clock;
        // 当前这一级 tile 里「变了的区域」，局部像素坐标（x0, y0, x1, y1）。
        let mut cur = (0i32, 0i32, TILE_SIZE as i32, TILE_SIZE as i32);
        for level in 1..=MAX_MIPMAP_LEVEL {
            let fac = 1i32 << level;
            let parent = (tx.div_euclid(fac), ty.div_euclid(fac));
            let child = (tx.div_euclid(fac / 2), ty.div_euclid(fac / 2));
            // 子格是父格的哪个象限（`ensure` 的 `(i % 2) * HALF`）。
            let qx = child.0.rem_euclid(2) * HALF as i32;
            let qy = child.1.rem_euclid(2) * HALF as i32;
            // 父格里受影响的区域：象限偏移 + 子区域折半。
            let dst = (
                qx + cur.0 / 2,
                qy + cur.1 / 2,
                qx + (cur.2 + 1) / 2,
                qy + (cur.3 + 1) / 2,
            );
            let slot = (level - 1) as usize;
            // 子级像素：level 1 取基表，其余取上一级（刚刚补过的那一格）。
            // 持有 `Arc` 克隆而不是借用，才能在同一个循环体里再可变借用父格。
            let src: Option<Arc<TilePixels>> = if level == 1 {
                base.get(&child).map(|t| Arc::clone(t.pixels_arc()))
            } else {
                self.levels[slot - 1]
                    .get(&child)
                    .and_then(|t| t.pixels_arc())
                    .map(Arc::clone)
            };
            let Some(src) = src else {
                // 理论上不可达（上面已确认四级祖先都是 Ready）；保守退回标脏。
                self.mark_dirty(tx, ty);
                return;
            };
            let entry = self.levels[slot]
                .get_mut(&parent)
                .expect("上面已确认这一格是 Ready");
            let MipState::Ready(arc) = &mut entry.state else {
                unreachable!("上面已确认这一格是 Ready");
            };
            let all_zero = {
                let tile = Arc::make_mut(arc);
                let mut any = false;
                for y in dst.1..dst.3 {
                    for x in dst.0..dst.2 {
                        let sx = 2 * (x - qx);
                        let sy = 2 * (y - qy);
                        let p00 = src[sy as usize][sx as usize];
                        let p01 = src[sy as usize][sx as usize + 1];
                        let p10 = src[sy as usize + 1][sx as usize];
                        let p11 = src[sy as usize + 1][sx as usize + 1];
                        let mut px = [0u16; 4];
                        for c in 0..4 {
                            // `pixops.cpp:31-34`：各自 /4 后相加（与 `ensure`
                            // 的循环逐字相同，否则逐字节一致就不成立）。
                            px[c] = (p00[c] as u32 / 4
                                + p01[c] as u32 / 4
                                + p10[c] as u32 / 4
                                + p11[c] as u32 / 4)
                                as u16;
                        }
                        any |= (px[0] | px[1] | px[2] | px[3]) != 0;
                        tile[y as usize][x as usize] = px;
                    }
                }
                // 只有补丁区全零时整格才有可能变成 `Empty`；否则跳过整格扫描
                //（64×64×5 级 = 每块基 tile 两万次比较，这才是补丁的主要成本）。
                if any {
                    false
                } else {
                    tile.iter().all(|row| row.iter().all(|p| p == &[0u16; 4]))
                }
            };
            entry.revision = revision;
            // 补丁可能把整格擦成全零：与 `ensure` 保持同一不变式
            //（全零记成 `Empty`，读取端不必保留 32 KiB 的空格子）。
            if all_zero {
                entry.state = MipState::Empty;
            }
            cur = dst;
        }
    }

    /// 批量补丁：一组基格变了之后，把变化沿金字塔向上传播，**按层并行**。
    ///
    /// 与逐个调 [`Self::patch_base`] 的**数值结果完全相同**（两者都等价于
    /// [`Self::ensure`] 的重建），但组织方式不同：
    ///
    /// - 先按层把父格去重——大笔刷一事件改几百块基格，而它们只落在几十个
    ///   上级格子里，一个父格只重算一次；
    /// - 重算时把父格从表里摘出来独占，于是可以按格切给 rayon；
    /// - 不再要求「四级祖先都已 Ready」：缺的格子直接算出来，比原来
    ///   "整条链退回 `mark_dirty`、等读取时整块重建" 省得多。
    ///
    /// 补丁是 GPU 回写里最贵的一段（每块基格要多读写约 50 KiB 上级像素），
    /// 串行做就是纯内存带宽瓶颈。
    pub fn patch_many(&mut self, coords: &[(i32, i32)], base: &HashMap<(i32, i32), Tile>) {
        if coords.is_empty() {
            return;
        }
        let mut cur: Vec<(i32, i32)> = coords.to_vec();
        for level in 1..=MAX_MIPMAP_LEVEL {
            let slot = (level - 1) as usize;
            // `cur` 里已经是**上一级**的坐标，父格永远只除以 2
            //（不是 `1<<level`：那是"从基格一次跨到第 level 级"的换算）。
            let fac = 2i32;
            let mut seen = std::collections::HashSet::with_capacity(cur.len());
            let mut parents: Vec<(i32, i32)> = Vec::with_capacity(cur.len());
            for &(x, y) in &cur {
                let p = (x.div_euclid(fac), y.div_euclid(fac));
                if seen.insert(p) {
                    parents.push(p);
                }
            }
            if parents.is_empty() {
                break;
            }

            // 摘出来独占：留在 HashMap 里只能拿到整表的可变借用，没法并行。
            let mut owned: Vec<((i32, i32), MipTile)> = Vec::with_capacity(parents.len());
            for &p in &parents {
                let t = self.levels[slot]
                    .remove(&p)
                    .unwrap_or_else(MipTile::empty);
                owned.push((p, t));
            }

            {
                let prev = if level == 1 {
                    None
                } else {
                    Some(&self.levels[slot - 1])
                };
                if owned.len() >= 4 {
                    owned
                        .par_iter_mut()
                        .for_each(|(p, tile)| *tile = rebuild_parent(*p, base, prev));
                } else {
                    for (p, tile) in owned.iter_mut() {
                        *tile = rebuild_parent(*p, base, prev);
                    }
                }
            }

            self.clock += 1;
            let revision = self.clock;
            for (p, mut t) in owned {
                t.revision = revision;
                self.levels[slot].insert(p, t);
            }
            cur = parents;
        }
    }

    /// 清空整座金字塔（`clear()` 的 `if self.mipmap: self.mipmap.clear()`，
    /// `:198-202`）。
    ///
    /// 与源码一致：**不**把格子标成脏，而是直接丢弃整份缓存。重建的输入
    /// 永远是基表，所以下一次读取会按需从基表重新建出来（"缺席"在这里等价
    /// 于"没算过"，见 [`Self::ensure`]）。
    pub fn invalidate_all(&mut self) {
        for map in &mut self.levels {
            map.clear();
        }
    }

    /// 清空某一级（源码的 `mipmap.clear()` 递归形态，`:201-202`）。
    #[inline]
    pub fn clear_level(&mut self, level: u32) {
        if (1..=MAX_MIPMAP_LEVEL).contains(&level) {
            self.levels[(level - 1) as usize].clear();
        }
    }

    /// 某一级当前分配的条目数（含脏哨兵与已知空）。
    #[inline]
    pub fn allocated_tiles(&self, level: u32) -> usize {
        if level == 0 || level > MAX_MIPMAP_LEVEL {
            return 0;
        }
        self.levels[(level - 1) as usize].len()
    }

    /// 某一级所有 `Ready` 格子的 `(坐标, 像素, revision)`。
    ///
    /// 预览面播种用：直接克隆 `Arc`，不复制像素。
    pub fn ready_entries(&self, level: u32) -> Vec<((i32, i32), Arc<TilePixels>, u64)> {
        if level == 0 || level > MAX_MIPMAP_LEVEL {
            return Vec::new();
        }
        self.levels[(level - 1) as usize]
            .iter()
            .filter_map(|(&coord, tile)| {
                tile.pixels_arc()
                    .map(|px| (coord, Arc::clone(px), tile.revision()))
            })
            .collect()
    }

    /// 某一级里**已重建出像素**的格子数。
    #[inline]
    pub fn ready_tiles(&self, level: u32) -> usize {
        if level == 0 || level > MAX_MIPMAP_LEVEL {
            return 0;
        }
        self.levels[(level - 1) as usize]
            .values()
            .filter(|t| t.pixels().is_some())
            .count()
    }

    /// 金字塔实际占用的像素内存（`Dirty`/`Empty` 不占像素）。
    pub fn storage_bytes(&self) -> usize {
        self.levels
            .iter()
            .flat_map(|map| map.values())
            .filter(|t| t.pixels().is_some())
            .count()
            * TILE_SIZE
            * TILE_SIZE
            * 4
            * 2
    }

    /// 惰性重建 level `level` 的 `(tx, ty)`（对应 `_regenerate_mipmap`，`:297-322`）。
    ///
    /// 流程与源码一致：
    ///
    /// 1. 已经是可用格子 → 直接返回（源码只在 `t is mipmap_dirty_tile` 时重建）；
    /// 2. 四个子 tile 若仍是脏哨兵 → 先递归重建（`:307-312`）；
    /// 3. 每个子 tile 经 `tile_downscale_rgba16(src, dst, x*N/2, y*N/2)` 落到
    ///    目标 tile 的对应象限（`:313-315`）；
    /// 4. 四子全空 → **删除字典条目**，不占内存（`:318-321`，源码用
    ///    `transparent_tile` 代替）。
    ///
    /// 实现上先整体重建 4 个子级、再一次性取引用、最后写回，
    /// 避免 `&mut self`（递归）与 `&self.levels`（读子级）同时存在。
    fn ensure(&mut self, level: u32, tx: i32, ty: i32, base: &HashMap<(i32, i32), Tile>) {
        debug_assert!(
            (1..=MAX_MIPMAP_LEVEL).contains(&level),
            "level {level} 超出 1..={MAX_MIPMAP_LEVEL}"
        );
        let slot = (level - 1) as usize;
        if self.levels[slot].get(&(tx, ty)).is_some_and(|t| !t.is_dirty()) {
            // 已重建 / 已知空：都是稳定状态，不重算。
            return;
        }
        // 脏哨兵与**缺席**都要往下走：缺席意味着"没人算过"，内容还可能存在
        // （新克隆的表面金字塔是空的、而基表有内容；源码之所以能"缺席即空"，
        // 是因为它的 `_mark_mipmap_dirty` 在每次写入后都会建出哨兵条目）。
        // 重建结果会被缓存成 `Empty`/`Ready`，所以每个坐标最多算一次。
        self.rebuilds += 1;

        // 子级重建（惰性脏传播的第二段：源码里是 `parent._regenerate_mipmap`）。
        if level > 1 {
            for qy in 0..2i32 {
                for qx in 0..2i32 {
                    self.ensure(level - 1, tx * 2 + qx, ty * 2 + qy, base);
                }
            }
        }

        // 取 4 个子 tile 的像素引用 → 降采样 → 写回。整个"读"过程放在块里，
        // 出块即释放对 self.levels 的不可变借用。
        let rendered: Option<Arc<TilePixels>> = {
            // 象限顺序与源码的双重循环一致：`for x(0..2) { for y(0..2) }`，
            // 子坐标 `(tx*2 + x, ty*2 + y)`，目标偏移 `(x*N/2, y*N/2)`。
            let mut kids: [Option<&TilePixels>; 4] = [None; 4];
            for qy in 0..2i32 {
                for qx in 0..2i32 {
                    let coord = (tx * 2 + qx, ty * 2 + qy);
                    kids[(qy * 2 + qx) as usize] = if level == 1 {
                        base.get(&coord).map(|t| t.pixels())
                    } else {
                        self.levels[(level - 2) as usize]
                            .get(&coord)
                            .and_then(|t| t.pixels())
                    };
                }
            }

            if kids.iter().all(|k| k.is_none()) {
                // 四子全空：源码走 `del self.tiledict[(tx, ty)]`（`:320`）。
                None
            } else {
                // 一次分配，直接往 `Arc` 里写（省掉 Box → Arc 的 32 KiB 拷贝）。
                let mut dst = Arc::new(*empty_tile());
                let mut any = false;
                {
                    let out = Arc::make_mut(&mut dst);
                    for (i, kid) in kids.iter().enumerate() {
                        let Some(src) = *kid else { continue };
                        let (dx0, dy0) = ((i % 2) * HALF, (i / 2) * HALF);
                        for y in 0..HALF {
                            for x in 0..HALF {
                                let p00 = src[2 * y][2 * x];
                                let p01 = src[2 * y][2 * x + 1];
                                let p10 = src[2 * y + 1][2 * x];
                                let p11 = src[2 * y + 1][2 * x + 1];
                                let mut px = [0u16; 4];
                                for c in 0..4 {
                                    // `pixops.cpp:31-34`：各自 /4 后相加。
                                    // 每项 ≤ 16383，和 ≤ 65532，不会溢出 u16。
                                    px[c] = (p00[c] as u32 / 4
                                        + p01[c] as u32 / 4
                                        + p10[c] as u32 / 4
                                        + p11[c] as u32 / 4)
                                        as u16;
                                }
                                any |= px != [0u16; 4];
                                out[dy0 + y][dx0 + x] = px;
                            }
                        }
                    }
                }
                if any {
                    Some(dst)
                } else {
                    None
                }
            }
        };

        let tile = match rendered {
            Some(pixels) => {
                self.clock += 1;
                MipTile {
                    state: MipState::Ready(pixels),
                    revision: self.clock,
                }
            }
            // 四子全空 → 记成 `Empty`：这条记录从此不再触发任何重建。
            None => MipTile::empty(),
        };
        self.levels[slot].insert((tx, ty), tile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// level 0 的基表类型（= `PigmentSurface.tiles`）。
    type Base = HashMap<(i32, i32), Tile>;

    /// 造一个整块填满 `p` 的基 tile（与写借出后的形状一致）。
    fn solid(p: [u16; 4]) -> Tile {
        let mut px = empty_tile();
        for row in px.iter_mut() {
            for cell in row.iter_mut() {
                *cell = p;
            }
        }
        Tile {
            pixels: Arc::new(*px),
            revision: 1,
            last_touched: 1,
        }
    }

    /// 只写一个像素的基 tile（其余全零）。
    fn dotted(p: [u16; 4], x: usize, y: usize) -> Tile {
        let mut px = empty_tile();
        px[y][x] = p;
        Tile {
            pixels: Arc::new(*px),
            revision: 1,
            last_touched: 1,
        }
    }

    /// 测试用的四个纯色：全部是 4 的倍数，降采样后数值可精确预期。
    const A: [u16; 4] = [400, 800, 1200, 32768];
    const B: [u16; 4] = [40, 80, 160, 1600];
    const C: [u16; 4] = [4000, 8000, 12000, 24000];
    const D: [u16; 4] = [200, 40, 4, 8000];

    /// 2×2 四个不同纯色基 tile → level 1 的四个象限各自等于对应子 tile 的值。
    #[test]
    fn level1_downscales_the_four_child_quadrants() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((0, 0), solid(A));
        base.insert((1, 0), solid(B));
        base.insert((0, 1), solid(C));
        base.insert((1, 1), solid(D));

        let t = mip.tile_at(1, 0, 0, &base).expect("有内容，必须返回 tile");
        // 子 (0,0) → 左上象限；(1,0) → 右上；(0,1) → 左下；(1,1) → 右下
        assert_eq!(t[0][0], A, "左上象限");
        assert_eq!(t[0][32], B, "右上象限");
        assert_eq!(t[32][0], C, "左下象限");
        assert_eq!(t[32][32], D, "右下象限");
        // 象限边界前后一格不能串味
        assert_eq!(t[31][31], A);
        assert_eq!(t[31][32], B);
        assert_eq!(t[32][31], C);
        assert_eq!(t[63][63], D);
        // 同级只有一个 tile
        assert_eq!(mip.allocated_tiles(1), 1);
        assert_eq!(mip.storage_bytes(), 32 * 1024);
    }

    /// 只有一部分子 tile 存在时，缺席的象限保持全透明。
    #[test]
    fn absent_children_stay_transparent() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((1, 1), solid(D)); // 只有右下角有内容

        let t = mip.tile_at(1, 0, 0, &base).expect("有内容");
        assert_eq!(t[0][0], [0; 4]);
        assert_eq!(t[0][32], [0; 4]);
        assert_eq!(t[32][0], [0; 4]);
        assert_eq!(t[32][32], D);
        // 相邻的 level-1 tile 完全空 → None
        assert!(mip.tile_at(1, 1, 0, &base).is_none());
    }

    /// 脏传播：写入后从基表读到的必须是新数据（level 1 与 level 2 都要刷新）。
    #[test]
    fn dirty_mark_propagates_new_data_up_the_pyramid() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((2, 2), solid(A));
        base.insert((3, 2), solid(B));
        base.insert((2, 3), solid(C));
        base.insert((3, 3), solid(D));

        let before1 = mip.tile_at(1, 1, 1, &base).unwrap()[0][0];
        assert_eq!(before1, A);
        // level 2 的 (0,0) 覆盖基表 (0..4)²；基 (2..4)² 落在它的右下象限，
        // 每个基 tile 在其中占 32×32 的纯色块。
        assert_eq!(mip.tile_at(2, 0, 0, &base).unwrap()[32][32], A);
        assert_eq!(mip.tile_at(2, 0, 0, &base).unwrap()[63][63], D);
        assert_eq!(
            mip.tile_at(2, 0, 0, &base).unwrap()[0][0],
            [0; 4],
            "左上象限（基 (0..2)²）本来是空的"
        );

        // 改写基表 (3,3) 并标脏（源码的 tile_request 写借出后调用 _mark_mipmap_dirty）
        const E: [u16; 4] = [800, 160, 8, 16000];
        base.insert((3, 3), solid(E));
        mip.mark_dirty(3, 3);
        assert!(mip.tile_entry_at(1, 1, 1).unwrap().is_dirty());
        assert!(mip.tile_entry_at(2, 0, 0).unwrap().is_dirty());
        assert!(mip.tile_entry_at(3, 0, 0).unwrap().is_dirty());

        assert_eq!(mip.tile_at(1, 1, 1, &base).unwrap()[32][32], E, "level 1 刷新");
        assert_eq!(
            mip.tile_at(2, 0, 0, &base).unwrap()[63][63],
            E,
            "level 2 也应反映新数据"
        );
        assert_eq!(
            mip.tile_at(2, 0, 0, &base).unwrap()[32][32],
            A,
            "未改动的相邻块保持原值"
        );
    }

    /// 「各自整除后再相加」的截断：单个像素 `[4,8,12,16]` → `[1,2,3,4]`；
    /// 值小于 4 的像素直接消失（这就是全空判定的来源）。
    #[test]
    fn downscale_truncates_each_tap_before_adding() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((0, 0), dotted([4, 8, 12, 16], 0, 0));
        let t = mip.tile_at(1, 0, 0, &base).expect("还有内容");
        assert_eq!(t[0][0], [1, 2, 3, 4], "四个 tap 只有一个非零：v/4");
        assert_eq!(t[0][1], [0; 4]);

        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((0, 0), dotted([3, 3, 3, 3], 0, 0));
        assert!(
            mip.tile_at(1, 0, 0, &base).is_none(),
            "3/4 = 0 → 整块降采样后全空"
        );
        // 全空不消失，而是被记成 `Empty`：之后读到就是 None，不再重建。
        assert!(mip.tile_entry_at(1, 0, 0).unwrap().is_empty());
        assert_eq!(mip.ready_tiles(1), 0);
        assert_eq!(mip.storage_bytes(), 0);
    }

    /// 每次写入都必须抬高所有祖先的 revision，即使祖先已经是脏哨兵。
    ///
    /// GPU 同步把 revision 当水位线；旧实现遇到已脏的层级就 break，导致
    /// 同一 mip 格里的后续笔迹 revision 不变、永远不重传。
    #[test]
    fn mark_dirty_bumps_revisions_on_every_write() {
        let mut mip = MipMap::new();
        mip.mark_dirty(0, 0);
        let r1: Vec<u64> = (1..=MAX_MIPMAP_LEVEL)
            .map(|l| mip.tile_entry_at(l, 0, 0).unwrap().revision())
            .collect();
        assert!(r1.iter().all(|&r| r > 0), "脏哨兵也要有非零 revision");

        // 同一 mip 格覆盖下的另一块基 tile。
        mip.mark_dirty(1, 1);
        let r2: Vec<u64> = (1..=MAX_MIPMAP_LEVEL)
            .map(|l| mip.tile_entry_at(l, 0, 0).unwrap().revision())
            .collect();
        for (l, (a, b)) in r1.iter().zip(&r2).enumerate() {
            assert!(b > a, "level {} revision 未变化: {a} → {b}", l + 1);
        }

        // 重建后再写：Ready → Dirty，revision 仍需变化。
        let mut base: Base = HashMap::new();
        base.insert((0, 0), solid(A));
        base.insert((1, 1), solid(A));
        let _ = mip.tile_at(MAX_MIPMAP_LEVEL, 0, 0, &base);
        let ready = mip
            .tile_entry_at(MAX_MIPMAP_LEVEL, 0, 0)
            .unwrap()
            .revision();
        assert!(!mip.tile_entry_at(MAX_MIPMAP_LEVEL, 0, 0).unwrap().is_dirty());
        mip.mark_dirty(0, 0);
        let after = mip
            .tile_entry_at(MAX_MIPMAP_LEVEL, 0, 0)
            .unwrap()
            .revision();
        assert!(after > ready, "重建后的写入未抬高 revision: {ready} → {after}");
    }

    /// `mark_dirty` 只标"覆盖该坐标"的祖先：远端的 tile 不受影响。
    #[test]
    fn mark_dirty_only_touches_covering_ancestors() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((0, 0), solid(A));
        base.insert((8, 0), solid(B));
        // 先把两级都建出来
        mip.tile_at(1, 0, 0, &base);
        mip.tile_at(1, 4, 0, &base);

        mip.mark_dirty(0, 0);
        assert!(mip.tile_entry_at(1, 0, 0).unwrap().is_dirty());
        assert!(
            !mip.tile_entry_at(1, 4, 0).unwrap().is_dirty(),
            "远端 tile 不该被标脏"
        );
        // level 2 的 (0,0) 覆盖基 (0..4)²，不含 (8,0) → 只该被标一次脏
        assert!(mip.tile_entry_at(2, 0, 0).unwrap().is_dirty());
        assert!(mip.tile_entry_at(3, 0, 0).unwrap().is_dirty());
    }

    /// 四个子 tile 全透明（哪怕是"存在但全零"）→ None，且不占内存。
    ///
    /// 与 M1 的区别：现在会留一条 `Empty` 记录（源码的 `del` + 缺席语义），
    /// 这样后续读取不必再走一遍重建。
    #[test]
    fn all_transparent_children_free_the_entry() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        for coord in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            base.insert(coord, solid([0; 4]));
        }
        assert!(mip.tile_at(1, 0, 0, &base).is_none());
        assert!(mip.tile_entry_at(1, 0, 0).unwrap().is_empty());
        assert_eq!(mip.ready_tiles(1), 0, "全空不能占像素内存");
        assert_eq!(mip.storage_bytes(), 0);

        // 从没被标脏过的空基表：先算一次证明它空，然后缓存成 Empty
        let empty: Base = HashMap::new();
        assert!(mip.tile_at(2, 0, 0, &empty).is_none());
        assert!(mip.tile_entry_at(2, 0, 0).unwrap().is_empty());
        let after = mip.rebuild_count();
        assert!(mip.tile_at(2, 0, 0, &empty).is_none());
        assert_eq!(mip.rebuild_count(), after, "第二次不该重算");
        assert_eq!(mip.ready_tiles(2), 0);
        assert_eq!(mip.storage_bytes(), 0);
    }

    /// 已知空的坐标不该反复重建：连续读三次只算一次重建。
    ///
    /// 远景屏幕 tile 循环里空白区域占多数，重复重建会直接变成帧时间
    /// （源码 `_get_tile_numpy` 对缺席坐标直接给 `transparent_tile`，不重建）。
    #[test]
    fn known_empty_coordinate_is_not_rebuilt() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        // 有内容但降采样后全空（值太小被截断成 0）→ 走"重建出 Empty"的路径
        base.insert((0, 0), dotted([3, 3, 3, 3], 0, 0));
        mip.mark_dirty(0, 0);

        assert!(mip.tile_at(1, 0, 0, &base).is_none());
        let after_first = mip.rebuild_count();
        assert!(after_first >= 1, "第一次必须真的算一遍");
        assert!(mip.tile_entry_at(1, 0, 0).unwrap().is_empty());

        assert!(mip.tile_at(1, 0, 0, &base).is_none());
        assert!(mip.tile_at(1, 0, 0, &base).is_none());
        assert_eq!(
            mip.rebuild_count(),
            after_first,
            "已知空的坐标连续读三次，重建次数不能增加"
        );

        // 写入后必须能重新变脏并算出内容（Empty → Dirty → Ready）
        base.insert((0, 0), solid(A));
        mip.mark_dirty(0, 0);
        assert!(mip.tile_entry_at(1, 0, 0).unwrap().is_dirty(), "Empty 要能被标脏");
        assert_eq!(mip.tile_at(1, 0, 0, &base).unwrap()[0][0], A);
        assert_eq!(mip.rebuild_count(), after_first + 1);
    }

    /// level 4（上限）：一个 64×64 基 tile 落成 4×4 px 的一块，位置正确。
    ///
    /// level 4 的 tile 覆盖 16×16 个基 tile（1024×1024 模型像素），
    /// 一个基 tile 在其中占 4×4 px（每 px = 16×16 基像素）。
    #[test]
    fn max_level_collapses_a_base_tile_to_a_4x4_block() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        // 基 (5,7) → level4 (5/16, 7/16) = (0,0)，像素块 x=5*4..6*4、y=7*4..8*4
        base.insert((5, 7), solid([0, 0, 0, 32768]));

        let t = mip.tile_at(MAX_MIPMAP_LEVEL, 0, 0, &base).expect("有内容");
        // 纯色块在每一级平均后仍是原值（每级 4 个相同 tap 各 /4 再相加）
        for (y, row) in t.iter().enumerate().take(32).skip(28) {
            for (x, px) in row.iter().enumerate().take(24).skip(20) {
                assert_eq!(*px, [0, 0, 0, 32768], "px ({x},{y})");
            }
        }
        assert_eq!(t[27][20], [0; 4], "块上方是空的");
        assert_eq!(t[28][19], [0; 4], "块左侧是空的");
        assert_eq!(t[31][23 + 1], [0; 4], "块右侧是空的");
        let nonzero = t.iter().flatten().filter(|px| **px != [0u16; 4]).count();
        assert_eq!(nonzero, 16, "4×4 = 16 个像素有内容");
        // 惰性递归把整条链都建了出来（每级 1 个 tile）
        for level in 1..=MAX_MIPMAP_LEVEL {
            assert_eq!(mip.ready_tiles(level), 1, "level {level}");
        }
        assert_eq!(mip.storage_bytes(), MAX_MIPMAP_LEVEL as usize * 32 * 1024);
        assert_eq!(MipMap::level_scale(MAX_MIPMAP_LEVEL), 16);
    }

    /// 负坐标：向下取整的层级映射不能错位，也不能 panic。
    #[test]
    fn negative_coordinates_map_like_python_floor_division() {
        assert_eq!(MipMap::to_level_coord(1, -1, -1), (-1, -1));
        assert_eq!(MipMap::to_level_coord(1, 0, 0), (0, 0));
        assert_eq!(MipMap::to_level_coord(1, 1, 1), (0, 0));
        assert_eq!(MipMap::to_level_coord(2, -1, -1), (-1, -1));
        assert_eq!(MipMap::to_level_coord(6, -1, -1), (-1, -1));

        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        // 基 tile (-1,-1) 是 level-1 tile (-1,-1) 的右下子块
        base.insert((-1, -1), solid(A));
        base.insert((0, 0), solid(B));

        let t = mip.tile_at(1, -1, -1, &base).expect("负坐标也要有内容");
        assert_eq!(t[32][32], A, "基 (-1,-1) → level1(-1,-1) 的右下象限");
        assert_eq!(t[0][0], [0; 4]);

        let t0 = mip.tile_at(1, 0, 0, &base).expect("基 (0,0) 在 level1(0,0)");
        assert_eq!(t0[0][0], B, "基 (0,0) → level1(0,0) 的左上象限");
        assert_eq!(t0[32][32], [0; 4]);

        // level 2：基 (-1,-1) 落到 level2(-1,-1) 像素 (48,48)
        let t2 = mip.tile_at(2, -1, -1, &base).expect("level 2 有内容");
        assert_eq!(t2[48][48], A);
        assert_eq!(t2[32][32], [0; 4]);

        // 标脏（含负坐标的层级换算）不 panic
        mip.mark_dirty(-1, -1);
        assert!(mip.tile_entry_at(1, -1, -1).unwrap().is_dirty());
        assert!(mip.tile_entry_at(2, -1, -1).unwrap().is_dirty());
        base.insert((-1, -1), solid(C));
        assert_eq!(mip.tile_at(1, -1, -1, &base).unwrap()[32][32], C);
    }

    /// level 0 透传基表；越界 level 钳位到上限。
    #[test]
    fn level_zero_passes_through_and_high_levels_clamp() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        // 用纯色整块（不是单个像素）：逐级平均后仍然存在，能一路走到 level 4
        base.insert((0, 0), solid([10, 20, 30, 32768]));

        let t = mip.tile_at(0, 0, 0, &base).expect("level 0 直读基表");
        assert_eq!(t[4][3], [10, 20, 30, 32768], "level 0 是原样透传");
        assert!(mip.tile_at(0, 5, 5, &base).is_none());
        assert_eq!(mip.allocated_tiles(0), 0, "level 0 不由金字塔持有");

        // 越界钳位：level 99 与 level 4 是同一格
        let hi = *mip
            .tile_at(99, 0, 0, &base)
            .expect("钳位到 MAX_MIPMAP_LEVEL");
        let cap = *mip.tile_at(MAX_MIPMAP_LEVEL, 0, 0, &base).expect("同上");
        assert_eq!(hi, cap);
        // 每一级都是"4 个相同值各自 /4 再相加"：10→8→8…，20→20（20/4=5），
        // 30→28→28（30/4=7，7*4=28），alpha 32768 无量化的损失。
        assert_eq!(hi[0][0], [8, 20, 28, 32768], "每级各自 /4 后的均匀值");
        assert!(mip.tile_entry_at(99, 0, 0).is_none(), "越界不建条目");
    }

    /// `invalidate_all` 丢掉全部层级；重建后内容与基表一致。
    #[test]
    fn invalidate_all_drops_every_level() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((0, 0), solid(A));
        mip.tile_at(3, 0, 0, &base);
        assert!(mip.storage_bytes() > 0);

        mip.invalidate_all();
        for level in 1..=MAX_MIPMAP_LEVEL {
            assert_eq!(mip.allocated_tiles(level), 0, "level {level}");
        }
        assert_eq!(mip.storage_bytes(), 0);

        // 重新读取应重建出正确内容（基表还在，只是缓存被丢了 —— 这正是
        // trim/translate/remove_empty_tiles 之后必须能恢复的情况）
        assert_eq!(mip.tile_at(1, 0, 0, &base).unwrap()[0][0], A);

        // 作废之后，**没有内容**的坐标也必须重建一次并缓存成 Empty，
        // 不能因为"缺席"就永远返回 None（那会漏掉平移后的新位置内容）。
        assert!(mip.tile_at(1, 5, 5, &base).is_none());
        assert!(mip.tile_entry_at(1, 5, 5).unwrap().is_empty());
    }

    /// `tile_arc` 与 `tile_at` 指向同一份像素，且不触发像素复制。
    #[test]
    fn tile_arc_shares_the_same_pixels() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((0, 0), solid(A));

        let arc = mip.tile_arc(1, 0, 0, &base).expect("有内容");
        let ptr = Arc::as_ptr(&arc);
        assert_eq!(arc[0][0], A);
        // 第二次读取复用同一份 Arc（没有重新分配）
        let arc2 = mip.tile_arc(1, 0, 0, &base).expect("同上");
        assert_eq!(Arc::as_ptr(&arc2), ptr, "已重建的格子不该再分配");
        assert_eq!(Arc::strong_count(&arc2), 3, "本地两份 + 字典一份");
    }

    /// 每级只占一个 tile 尺寸：level 变大不改 tile 边长，只是覆盖更大范围。
    #[test]
    fn every_level_tile_stays_64x64() {
        let mut mip = MipMap::new();
        let mut base: Base = HashMap::new();
        base.insert((0, 0), solid(A));
        for level in 1..=MAX_MIPMAP_LEVEL {
            let t = mip.tile_at(level, 0, 0, &base).expect("level {level} 有内容");
            assert_eq!(std::mem::size_of_val(t), TILE_SIZE * TILE_SIZE * 4 * 2);
        }
        // 覆盖的基表范围是 2^level
        assert_eq!(MipMap::level_scale(1), 2);
        assert_eq!(MipMap::level_scale(MAX_MIPMAP_LEVEL), 16);
    }
}

/// 从 4 个子格重算一个父格；与 [`MipMap::ensure`] 的降采样逐字同源。
///
/// 子格缺席按全零处理（源码 `_regenerate_mipmap` 对缺席 tile 的处理），
/// 四子全空则返回 [`MipTile::empty`]。
fn rebuild_parent(
    coord: (i32, i32),
    base: &HashMap<(i32, i32), Tile>,
    prev: Option<&HashMap<(i32, i32), MipTile>>,
) -> MipTile {
    let (tx, ty) = coord;
    let mut kids: [Option<&TilePixels>; 4] = [None; 4];
    for qy in 0..2i32 {
        for qx in 0..2i32 {
            let c = (tx * 2 + qx, ty * 2 + qy);
            kids[(qy * 2 + qx) as usize] = match prev {
                None => base.get(&c).map(|t| t.pixels()),
                Some(map) => map.get(&c).and_then(|t| t.pixels()),
            };
        }
    }
    if kids.iter().all(|k| k.is_none()) {
        return MipTile::empty();
    }
    let mut dst = Arc::new(*empty_tile());
    let mut any = false;
    {
        let out = Arc::make_mut(&mut dst);
        for (i, kid) in kids.iter().enumerate() {
            let Some(src) = *kid else { continue };
            let (dx0, dy0) = ((i % 2) * HALF, (i / 2) * HALF);
            for y in 0..HALF {
                for x in 0..HALF {
                    let p00 = src[2 * y][2 * x];
                    let p01 = src[2 * y][2 * x + 1];
                    let p10 = src[2 * y + 1][2 * x];
                    let p11 = src[2 * y + 1][2 * x + 1];
                    let mut px = [0u16; 4];
                    for c in 0..4 {
                        // `pixops.cpp:31-34`：各自整除后再相加。
                        px[c] = (p00[c] as u32 / 4
                            + p01[c] as u32 / 4
                            + p10[c] as u32 / 4
                            + p11[c] as u32 / 4)
                            as u16;
                    }
                    any |= px != [0u16; 4];
                    out[dy0 + y][dx0 + x] = px;
                }
            }
        }
    }
    if any {
        MipTile {
            state: MipState::Ready(dst),
            revision: 0,
        }
    } else {
        MipTile::empty()
    }
}
