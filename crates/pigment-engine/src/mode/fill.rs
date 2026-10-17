//! 洪水填充 —— `gui/fill.py` + `lib/floodfill.py` + `lib/fill/{floodfill,fill_common}.cpp`
//! 的移植。
//!
//! # 源码分层（三块，本文件按同样顺序排列）
//!
//! | 参考源 | 内容 | 本文件 |
//! |---|---|---|
//! | `lib/fill/fill_common.hpp:56` | `rgba`（预乘 fix15）+ `max_diff` | [`Rgba`] |
//! | `lib/fill/floodfill.cpp:94` | 阈值函数 `pixel_fill_alpha` | [`Filler::pixel_fill_alpha`] |
//! | `lib/fill/floodfill.cpp:271` | 单 tile 四向扫描线填充 + 种子段溢出 | [`Filler::fill`] |
//! | `lib/fill/floodfill.cpp:154`/`:377` | `tile_uniformity` / `flood` | 同名方法 |
//! | `lib/fill_common.py:92` | `TileBoundingBox` | [`TileBoundingBox`] |
//! | `lib/floodfill.py:479` | `scanline_fill`（tile 队列 + 均匀 tile 捷径） | [`scanline_fill`] |
//! | `lib/floodfill.py:535` | `_TileFillSkipper` | [`TileFillSkipper`] |
//! | `lib/floodfill.py:371` | `composite`（把填充合成进目标层） | [`composite`] |
//! | `lib/floodfill.py:297` | `_flood_fill` 的编排 | [`flood_fill`] |
//! | `lib/fill/gap_detection.cpp` + `gap_closing_fill.cpp` | 缺口检测 / 闭合 / 回吸 | [`super::fill_gap`] |
//! | `gui/fill.py:40` | `FloodFillMode` | [`FillMode`] |
//! | `lib/command.py:561` | `FloodFill` 命令 | [`crate::command::FloodFill`] |
//!
//! # 算法要点（为什么这么写）
//!
//! 阈值函数**不是** 3D 欧氏距离，而是**去预乘后的四通道 Chebyshev 距离**：
//! `rgba.max_diff` 取 `max(|Δr|, |Δg|, |Δb|, |Δa|)`（`fill_common.hpp:82-88`），
//! 然后按 `dist / tolerance` 归一化（`floodfill.cpp:113`）：
//!
//! - `dist ≤ tolerance` → alpha 拉满（`32768`）；
//! - `tolerance < dist ≤ 1.5×tolerance` → 线性斜坡（边缘羽化，`floodfill.cpp:117-121`）；
//! - 更远 → 0，填充在此处停下（不传播）。
//!
//! 目标色取自**种子取样点**（可以和种子不同），且 alpha 为 0 时先把 rgb 归零
//! （`floodfill.py:103-110`）；目标色透明时距离退化成"被检像素的 alpha"
//! （`floodfill.cpp:105-106`），所以从不透明处往透明处填是"填的都是透明像素"。
//!
//! 填充以 tile 为单位：每块 tile 产出一张 alpha tile（写进 `filled` 字典），
//! **最后**才和填充色一起合成进目标层（`floodfill.py:371-476`）。因此读源与
//! 写目标是两个阶段，本移植据此把 `&PigmentSurface`（读）与
//! `&mut PigmentSurface`（写）拆开，避免同一表面的双重可变借用。
//!
//! # 工具接线（GUI 侧）
//!
//! - 动作 id：`FloodFillMode`（`gui/resources.xml:1934` 的 `GtkRadioAction`，
//!   标签 "Flood Fill"、短标签 "Fill"、图标 `mypaint-fill-symbolic`）；
//! - 快捷键：`g` ↔ `FlipFloodFillMode`（`gui/resources.xml:1944-1951`，
//!   无修饰键）；
//! - 交互：源码是 **`DragMode`**（不是 `SingleClickMode`）——按下记第一粒种子
//!   （`fill.py:161-171`）、拖动继续加种子（`:153-159`）、松开跑一次填充
//!   （`:173-178`）。所以"单击"就是只有一粒种子的拖拽；
//! - egui 适配：`Sense::click_and_drag` 会先观望点击/拖拽（点击时不发
//!   `drag_started`），所以 `shell/workspace.rs` 在判定为点击时补一对
//!   press/release —— 否则单击填充永远不触发。
//!
//! # 记账（本块未移植 / 用最保守做法的地方）
//!
//! - **src_path**（`fill.py:249` 的"取样源图层"下拉）：已接 GUI 与引擎
//!   （[`FillOptions::src_layer`] 存稳定图层 id，命令 `redo()` 时解析路径；
//!   [`LayerSource`] 按路径渲染单层/组）。
//! - **进度/取消对话框**（`fill.py:316`、`floodfill.py:116` 的 `FillHandler`）：
//!   我们的填充是同步的，没有线程与取消点；形态学/模糊同样同步。
//! - **种子叠加预览**（`fill.py:361` 的 `FloodFillOverlay`，黑白虚线）与
//!   光标族（`fill.py:61-72`）：GUI 叠加层与光标是另一块，未移植。
//! - **Gap Detection 的进度/取消**：`_GCTileHandler` 每块 tile 都可能做
//!   9 邻域缺口搜索，源码可取消；本移植同步执行，只有选项没有取消按钮。
//! - **BlendModes 管理器**（`gui/blendmodehandler.py`，决定"橡皮/锁 alpha/
//!   着色"三个修饰键的当前态）：未移植，[`FillOptions::blend_mode`] 由调用方
//!   直接给（默认 Normal），[`FillArgs::blend_parameters`] 的映射逻辑则逐行
//!   照抄 `fill.py:180-201`。
//! - **预乘色的均匀"空 tile"复用**（`_TileFillSkipper.uniform_tiles` 缓存）：
//!   纯性能优化，本移植用 [`AlphaTile::Uniform`] 表达，不需要缓存。

use std::collections::{HashMap, HashSet, VecDeque};

use hokusai::color::{hsv_to_rgb, Hsv};
use hokusai::tile::{TilePixels, TILE_SIZE};
use hokusai::{Brush, BrushSetting, TiledSurface};
use pigment_surface::combine::{self, CombineMode};
use pigment_surface::PigmentSurface;

use super::{Behavior, DragMode, InteractionMode, Key, ModeContext, PointerEvent};

/// 一块 tile 的像素数（`fill_common.py:16` 的 `N`，值 64）。
pub const PIXELS: usize = TILE_SIZE * TILE_SIZE;

/// 完全不透明的 fix15 alpha（`fill_common.py:19` 的 `_OPAQUE` = `1 << 15`）。
pub const OPAQUE: u16 = 1 << 15;

/// fix15 的 1.0（`fix15.hpp:26`）。
const ONE: u32 = combine::fix15::ONE;

/// 1.5 的 fix15 表示（`floodfill.cpp:112` 的 `onepointfive`）。
const ONE_POINT_FIVE: u32 = ONE + (ONE >> 1);

/// 空 tile（`lib/tiledsurface.py:1260` 的 `transparent_tile.rgba`）：
/// 源码里是一个共享常量，缺 tile 时只读借出它就是它。
static EMPTY_TILE: TilePixels = [[[0u16; 4]; TILE_SIZE]; TILE_SIZE];

// ---- lib/fill/fill_common.hpp ----

/// 预乘 fix15 的像素（`fill_common.hpp:56-101` 的 `rgba`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgba {
    pub red: u16,
    pub green: u16,
    pub blue: u16,
    pub alpha: u16,
}

impl Rgba {
    /// 全透明像素（源码里的 `rgba()` 默认构造）。
    pub const TRANSPARENT: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 0,
    };

    /// `rgba(chan_t r, chan_t g, chan_t b, chan_t a)`（`fill_common.hpp:74`）：
    /// 四个值原样保留（预乘空间）。
    pub const fn premultiplied(red: u16, green: u16, blue: u16, alpha: u16) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// `rgba(double r, double g, double b, chan_t a)`（`fill_common.hpp:67-73`）：
    /// 三个通道是**直通色**，乘以 alpha 后再定点化。
    ///
    /// 为什么用 f64：源码形参是 `double`，`fix15_short_clamp(r * a)` 里
    /// `r * a` 在 double 里算完再截断成 u32。填充色来自 Python float（double），
    /// 用 f32 会在大 alpha 上偏一两个 fix15 单位。
    pub fn from_straight(red: f64, green: f64, blue: f64, alpha: u16) -> Self {
        let chan = |c: f64| combine::fix15::short_clamp((c * alpha as f64) as u32);
        Self {
            red: chan(red),
            green: chan(green),
            blue: chan(blue),
            alpha,
        }
    }

    /// `max_diff`（`fill_common.hpp:82-88`）：四通道绝对差的最大值
    /// （Chebyshev 距离，**不是**欧氏距离）。
    pub fn max_diff(&self, other: &Self) -> u32 {
        let diff = |a: u16, b: u16| -> u32 {
            if a > b {
                (a - b) as u32
            } else {
                (b - a) as u32
            }
        };
        diff(self.red, other.red)
            .max(diff(self.green, other.green))
            .max(diff(self.blue, other.blue))
            .max(diff(self.alpha, other.alpha))
    }
}

/// `clamped_div`（`floodfill.cpp:62-66`）：`short_clamp(fix15_div(short_clamp(a), b))`。
#[inline]
fn clamped_div(a: u16, b: u16) -> u16 {
    let clamped = combine::fix15::short_clamp(a as u32) as u32;
    combine::fix15::short_clamp(combine::fix15::div(clamped, b as u32))
}

/// `straightened`（`floodfill.cpp:68-77`）：去预乘；全透明像素给全零。
fn straightened(px: Rgba) -> Rgba {
    if px.alpha == 0 {
        Rgba::TRANSPARENT
    } else {
        Rgba {
            red: clamped_div(px.red, px.alpha),
            green: clamped_div(px.green, px.alpha),
            blue: clamped_div(px.blue, px.alpha),
            alpha: px.alpha,
        }
    }
}

// ---- lib/fill/floodfill.cpp：阈值函数 + 单 tile 扫描线填充 ----

/// tile 的四条边（`floodfill.hpp:22-24` 的 `edges`；**顺序即数值**，
/// `queue_ranges` 与 `FULL_OVERFLOWS` 都用它当索引）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    North = 0,
    East = 1,
    South = 2,
    West = 3,
    /// `edges::none`：这批种子是"初始种子"（tile 内坐标），不是从邻边溢出的段。
    None = 4,
}

/// 一条种子段 `[start, end]`（**含端点**，`floodfill.cpp:41` 的 `Py_BuildValue("ii")`）。
pub type SeedRange = (i32, i32);

/// `Filler.fill` 的种子来源（`floodfill.cpp:294-298` 的两条分支）。
pub enum Seeds<'a> {
    /// `edges::none`：初始种子，一组 tile 内像素坐标。
    Initial(&'a [(i32, i32)]),
    /// 从某条边溢进来的种子段（按该边的方向排序）。
    Ranges {
        origin: Edge,
        ranges: &'a [SeedRange],
    },
}

/// 像素索引（行主序；C++ 侧靠 `PixelBuffer` 的 stride 走，等价）。
#[inline]
fn idx(x: i32, y: i32) -> usize {
    y as usize * TILE_SIZE + x as usize
}

#[inline]
fn rgba_of(px: [u16; 4]) -> Rgba {
    Rgba {
        red: px[0],
        green: px[1],
        blue: px[2],
        alpha: px[3],
    }
}

/// 阈值函数 + 单 tile 填充器（`floodfill.hpp:32-77` 的 `Filler`）。
///
/// 目标色在构造时定死；一次填充里 `fill` 可能被多块 tile 复用。
pub struct Filler {
    /// 去预乘后的目标色（`Filler::target_color`）。
    target_color: Rgba,
    /// 原样的预乘目标色（`tolerance == 0` 的精确比较用）。
    target_color_premultiplied: Rgba,
    /// fix15 的容差（`Filler::tolerance`）。
    tolerance: u32,
    /// 像素种子队列（`Filler::seed_queue`）。
    seed_queue: VecDeque<(i32, i32)>,
}

impl Filler {
    /// `Filler::Filler`（`floodfill.cpp:79-84`）：目标色是**取样点**的预乘像素，
    /// 容差夹到 `[0, 1]` 再定点化（截断）。
    pub fn new(target: Rgba, tolerance: f64) -> Self {
        Self {
            target_color: straightened(target),
            target_color_premultiplied: target,
            tolerance: (tolerance.clamp(0.0, 1.0) * ONE as f64) as u32,
            seed_queue: VecDeque::new(),
        }
    }

    /// 目标的去预乘色（调试用）。
    #[inline]
    pub fn target_color(&self) -> Rgba {
        self.target_color
    }

    /// 阈值函数（`floodfill.cpp:94-123`）：返回该像素**被填充后的 alpha**，
    /// 0 表示不填且不传播。
    pub fn pixel_fill_alpha(&self, px: &Rgba) -> u16 {
        // 双方全透明 → 直接算命中（floodfill.cpp:99-100）
        if (self.target_color.alpha | px.alpha) == 0 {
            return ONE as u16;
        }
        // 容差为 0：只认预乘色完全相等的像素（floodfill.cpp:101-103）
        if self.tolerance == 0 {
            return if self.target_color_premultiplied == *px {
                ONE as u16
            } else {
                0
            };
        }
        // 目标透明 → 距离退化成被检像素的 alpha（floodfill.cpp:105-106）
        let dist = if self.target_color.alpha == 0 {
            px.alpha as u32
        } else {
            self.target_color.max_diff(&straightened(*px))
        };
        // 按容差归一化后与 1.5 比较（floodfill.cpp:112-122）
        let dist = combine::fix15::div(dist, self.tolerance);
        if dist > ONE_POINT_FIVE {
            0
        } else {
            let aa = ONE_POINT_FIVE - dist;
            if aa < (ONE >> 1) {
                // 斜坡段：aa < 0.5 时翻倍（避免下溢），即 1.5→1.0 之间线性降到 0
                combine::fix15::short_clamp(combine::fix15::double(aa))
            } else {
                ONE as u16
            }
        }
    }

    /// `tile_uniformity`（`floodfill.cpp:154-171`）：整块均匀时返回它的填充
    /// alpha，否则 `None`。`empty_tile` 对应源码的 `src_tile is _EMPTY_RGBA`。
    pub fn tile_uniformity(&self, empty_tile: bool, src: &TilePixels) -> Option<u16> {
        if empty_tile {
            return Some(self.pixel_fill_alpha(&Rgba::TRANSPARENT));
        }
        let first = src[0][0];
        let uniform = src.iter().all(|row| row.iter().all(|px| *px == first));
        if !uniform {
            return None;
        }
        Some(self.pixel_fill_alpha(&rgba_of(first)))
    }

    /// `flood`（`floodfill.cpp:377-385`）：把整块 tile 的 alpha 值算出来。
    ///
    /// 源码里这是 gap closing 的输入（`_GCTileHandler.alpha_grid`），
    /// [`super::fill_gap::alpha_grid`] 用它生成 9 邻域 alpha tile。
    pub fn flood(&self, src: &TilePixels, dst: &mut [u16; PIXELS]) {
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                dst[idx(x as i32, y as i32)] = self.pixel_fill_alpha(&rgba_of(src[y][x]));
            }
        }
    }

    /// 单 tile 的四向扫描线填充（`floodfill.cpp:271-375`）。
    ///
    /// - `src`：源 tile 像素；`dst`：本 tile 的 alpha 输出（已在别处填过的
    ///   像素非 0，会截断扫描）；
    /// - `seeds`：初始像素种子或从某边溢入的种子段；
    /// - `bounds`：`(min_x, min_y, max_x, max_y)`，bbox 不跨 tile 时是整块；
    /// - 返回四条边的溢出种子段，顺序 `[north, east, south, west]`
    ///   （`floodfill.cpp:373-374`）。
    pub fn fill(
        &mut self,
        src: &TilePixels,
        dst: &mut [u16; PIXELS],
        seeds: Seeds<'_>,
        bounds: (i32, i32, i32, i32),
    ) -> [Vec<SeedRange>; 4] {
        let (mut min_x, mut min_y, mut max_x, mut max_y) = bounds;
        let n = TILE_SIZE as i32;
        // 越界即整块不用填（floodfill.cpp:277）；随后把边界夹进 tile
        if min_x > max_x || min_y > max_y {
            return std::array::from_fn(|_| Vec::new());
        }
        min_x = min_x.max(0);
        min_y = min_y.max(0);
        max_x = max_x.min(n - 1);
        max_y = max_y.min(n - 1);

        // 收进来的种子段要先标记，稍后从溢出段里剔除（避免回弹到来源 tile）
        let mut input_seeds = [false; TILE_SIZE];
        let mut origin = Edge::None;
        match seeds {
            Seeds::Initial(list) => self.queue_seeds(list, src, dst),
            Seeds::Ranges {
                origin: o,
                ranges,
            } => {
                origin = o;
                self.queue_ranges(o, ranges, &mut input_seeds, src, dst);
            }
        }

        let mut marks_n = [false; TILE_SIZE];
        let mut marks_e = [false; TILE_SIZE];
        let mut marks_s = [false; TILE_SIZE];
        let mut marks_w = [false; TILE_SIZE];

        while let Some((x0, y)) = self.seed_queue.pop_front() {
            // 行越界就跳过（列在下面的循环条件里管）
            if y < min_y || y > max_y {
                continue;
            }
            // 先向左扫（含起点），再向右扫（从起点 +1 开始）
            for dir in 0..2 {
                let mut look_above = true;
                let mut look_below = true;
                let x_start = x0 + dir;
                let x_delta = dir * 2 - 1;
                let mut x = x_start;
                while x >= min_x && x <= max_x {
                    // 已经填过 → 本行到此为止（floodfill.cpp:330-332）
                    if dst[idx(x, y)] != 0 {
                        break;
                    }
                    let alpha = self.pixel_fill_alpha(&rgba_of(src[y as usize][x as usize]));
                    if alpha == 0 {
                        // 颜色差太多（floodfill.cpp:336-339）
                        break;
                    }
                    dst[idx(x, y)] = alpha;
                    if y > 0 {
                        look_above = self.check_enqueue(
                            x,
                            y - 1,
                            look_above,
                            &rgba_of(src[(y - 1) as usize][x as usize]),
                            dst[idx(x, y - 1)],
                        );
                    } else {
                        marks_n[x as usize] = true;
                    }
                    if y < n - 1 {
                        look_below = self.check_enqueue(
                            x,
                            y + 1,
                            look_below,
                            &rgba_of(src[(y + 1) as usize][x as usize]),
                            dst[idx(x, y + 1)],
                        );
                    } else {
                        marks_s[x as usize] = true;
                    }
                    if x == 0 {
                        marks_w[y as usize] = true;
                    } else if x == n - 1 {
                        marks_e[y as usize] = true;
                    }
                    x += x_delta;
                }
            }
        }

        // 剔除与来源边重合的溢出（floodfill.cpp:365-371）
        if origin != Edge::None {
            let edge = match origin {
                Edge::North => &mut marks_n,
                Edge::East => &mut marks_e,
                Edge::South => &mut marks_s,
                Edge::West => &mut marks_w,
                Edge::None => unreachable!("上面刚判过"),
            };
            for i in 0..TILE_SIZE {
                edge[i] = edge[i] && !input_seeds[i];
            }
        }
        [
            to_seeds(&marks_n),
            to_seeds(&marks_e),
            to_seeds(&marks_s),
            to_seeds(&marks_w),
        ]
    }

    /// `check_enqueue`（`floodfill.cpp:134-146`）。
    ///
    /// 返回值语义：**"本行还能继续往队列里加相邻像素"** —— 遇到不匹配或
    /// 已填过的像素就重新置位，同段里只入队第一个可填像素。
    fn check_enqueue(
        &mut self,
        x: i32,
        y: i32,
        check: bool,
        src_px: &Rgba,
        dst_px: u16,
    ) -> bool {
        if dst_px != 0 {
            return true;
        }
        let matches = self.pixel_fill_alpha(src_px) > 0;
        if matches && check {
            self.seed_queue.push_back((x, y));
            return false;
        }
        !matches
    }

    /// `queue_seeds`（`floodfill.cpp:173-188`）：初始种子入队前先过一遍阈值。
    fn queue_seeds(&mut self, seeds: &[(i32, i32)], src: &TilePixels, dst: &[u16; PIXELS]) {
        for &(x, y) in seeds {
            // 边界防卫：`seeds_by_tile` 给出的坐标必在 0..N-1（Python 用 `%`），
            // 这里只是 Rust 的数组越界保护，不改变任何合法输入的行为。
            if !in_tile(x, y) {
                continue;
            }
            if dst[idx(x, y)] == 0
                && self.pixel_fill_alpha(&rgba_of(src[y as usize][x as usize])) > 0
            {
                self.seed_queue.push_back((x, y));
            }
        }
    }

    /// `queue_ranges`（`floodfill.cpp:198-253`）：把一条边上的种子段转成
    /// 像素种子（同段里只入队第一个可填像素），并标记"这些是收进来的种子"。
    fn queue_ranges(
        &mut self,
        origin: Edge,
        ranges: &[SeedRange],
        marks: &mut [bool; TILE_SIZE],
        src: &TilePixels,
        dst: &[u16; PIXELS],
    ) {
        let n = TILE_SIZE as i32;
        // 边的方向决定扫描轴（floodfill.cpp:206-211）
        let x_base = if origin == Edge::East { n - 1 } else { 0 };
        let y_base = if origin == Edge::South { n - 1 } else { 0 };
        let x_offs = (origin as i32 + 1) % 2;
        let y_offs = origin as i32 % 2;
        for &(seg_start, seg_end) in ranges {
            let mut contiguous = false;
            let mut k = seg_start;
            let mut x = x_base + x_offs * k;
            let mut y = y_base + y_offs * k;
            while k <= seg_end {
                if (0..n).contains(&k) {
                    marks[k as usize] = true;
                }
                // 边界防卫同 `queue_seeds`
                if in_tile(x, y) {
                    if dst[idx(x, y)] == 0
                        && self.pixel_fill_alpha(&rgba_of(src[y as usize][x as usize])) > 0
                    {
                        if !contiguous {
                            self.seed_queue.push_back((x, y));
                            contiguous = true;
                        }
                    } else {
                        contiguous = false;
                    }
                } else {
                    contiguous = false;
                }
                k += 1;
                x += x_offs;
                y += y_offs;
            }
        }
    }
}

/// 坐标是否落在 tile 内。
#[inline]
fn in_tile(x: i32, y: i32) -> bool {
    let n = TILE_SIZE as i32;
    (0..n).contains(&x) && (0..n).contains(&y)
}

/// `to_seeds`（`floodfill.cpp:22-60`）：布尔边数组 → 连续段的 `[(start, end)]`。
fn to_seeds(edge: &[bool; TILE_SIZE]) -> Vec<SeedRange> {
    let mut out = Vec::new();
    let mut contiguous = false;
    let mut seg_start = 0i32;
    let mut seg_end = 0i32;
    for (c, &on) in edge.iter().enumerate() {
        if on {
            if !contiguous {
                seg_start = c as i32;
                seg_end = c as i32;
                contiguous = true;
            } else {
                seg_end += 1;
            }
        } else if contiguous {
            out.push((seg_start, seg_end));
            contiguous = false;
        }
    }
    if contiguous {
        out.push((seg_start, seg_end));
    }
    out
}

// ---- lib/floodfill.py：tile 队列驱动 ----

/// 只读 tile 源（`floodfill.py:283` 的 "anything supporting readonly
/// tile_request()"；缺 tile 等价源码里共享的空 tile）。
///
/// # 为什么返回 [`TileRef`] 而不是 `&TilePixels`
///
/// 普通填充的源是一张真实表面，可以零拷贝借出；`sample merged` 的源是
/// **图层栈的合成结果**（`command.py:594-595` 的 `src_layer = layers`），
/// 每块 tile 都是现算出来的，没有可借的存储。`TileRef` 让两种源共用同一套
/// 扫描线代码：表面借出、合成结果持有。
pub trait TileSource {
    fn tile(&self, tx: i32, ty: i32) -> Option<TileRef<'_>>;
}

/// 一次 tile 读取的结果：借自表面，或合成出来的临时 tile。
pub enum TileRef<'a> {
    Borrowed(&'a TilePixels),
    Owned(Box<TilePixels>),
}

impl std::ops::Deref for TileRef<'_> {
    type Target = TilePixels;

    fn deref(&self) -> &TilePixels {
        match self {
            TileRef::Borrowed(t) => t,
            TileRef::Owned(t) => t,
        }
    }
}

impl TileSource for PigmentSurface {
    fn tile(&self, tx: i32, ty: i32) -> Option<TileRef<'_>> {
        PigmentSurface::tile(self, tx, ty).map(TileRef::Borrowed)
    }
}

/// 把扁平的 fix15 RGBA 缓冲转成一块 tile（合成结果的落点）。
fn owned_tile(flat: &[u16]) -> Box<TilePixels> {
    let mut tile = hokusai::tile::empty_tile();
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let o = (y * TILE_SIZE + x) * 4;
            tile[y][x] = [flat[o], flat[o + 1], flat[o + 2], flat[o + 3]];
        }
    }
    tile
}

/// `sample merged` 的源：整份图层栈的合成结果（`command.py:594-595`）。
///
/// 对应源码 `layers.flood_fill()`（`group.py:438-457`）：根栈本身就是
/// "可 tile 访问的渲染"，含背景（`tree.py:640-651` 的
/// `get_tile_accessible_layer_rendering` 对根栈走标准 spec）。
pub struct MergedSource<'a> {
    stack: &'a pigment_surface::layer::LayerStack,
}

impl<'a> MergedSource<'a> {
    pub fn new(stack: &'a pigment_surface::layer::LayerStack) -> Self {
        Self { stack }
    }
}

impl TileSource for MergedSource<'_> {
    fn tile(&self, tx: i32, ty: i32) -> Option<TileRef<'_>> {
        let mut flat = vec![0u16; TILE_SIZE * TILE_SIZE * 4];
        if !self.stack.composite_tile(tx, ty, &mut flat) {
            return None;
        }
        Some(TileRef::Owned(owned_tile(&flat)))
    }
}

/// 单个图层/组作为填充源的渲染（`group.py:455` 的
/// `get_tile_accessible_layer_rendering(self)`）。
///
/// 用于"当前层不可填 → 新建层再填"时源仍是原图层的情形：原图层可能是一个
/// 组，没有自己的 surface。记账：祖先组的可见性/不透明度不参与渲染
/// （源码 solo 渲染会带祖先的 PUSH/POP，本移植只渲染该层自身）。
pub struct LayerSource<'a> {
    stack: &'a pigment_surface::layer::LayerStack,
    path: Vec<usize>,
}

impl<'a> LayerSource<'a> {
    pub fn new(stack: &'a pigment_surface::layer::LayerStack, path: &[usize]) -> Self {
        Self {
            stack,
            path: path.to_vec(),
        }
    }
}

impl TileSource for LayerSource<'_> {
    fn tile(&self, tx: i32, ty: i32) -> Option<TileRef<'_>> {
        let mut flat = vec![0u16; TILE_SIZE * TILE_SIZE * 4];
        if !self.stack.composite_layer_tile(&self.path, tx, ty, &mut flat) {
            return None;
        }
        Some(TileRef::Owned(owned_tile(&flat)))
    }
}

/// `starting_coordinates`（`floodfill.py:80-84`）：模型坐标 → tile 坐标 +
/// tile 内坐标。Python 用 `//` / `%`（向下取整、结果非负），故用
/// `div_euclid` / `rem_euclid`。
pub fn starting_coordinates(x: i32, y: i32) -> (i32, i32, i32, i32) {
    let n = TILE_SIZE as i32;
    (x.div_euclid(n), y.div_euclid(n), x.rem_euclid(n), y.rem_euclid(n))
}

/// 按 tile 分组的种子：`(tile 坐标, tile 内像素坐标列表)`，组序 = 首次出现序。
///
/// 对应 `floodfill.py:326` 的 `seed_lists`（`dict` 的 items）。
pub type SeedLists = Vec<((i32, i32), Vec<(i32, i32)>)>;

/// `seeds_by_tile`（`floodfill.py:87-100`）：按 tile 分组，组序 = 首次出现顺序
/// （Python dict 的插入序；顺序会影响 tile 访问次序，但不影响填出来的内容）。
pub fn seeds_by_tile(seeds: &[(i32, i32)]) -> SeedLists {
    let mut order: Vec<(i32, i32)> = Vec::new();
    let mut index: HashMap<(i32, i32), usize> = HashMap::new();
    let mut lists: Vec<Vec<(i32, i32)>> = Vec::new();
    for &(x, y) in seeds {
        let (tx, ty, px, py) = starting_coordinates(x, y);
        let key = (tx, ty);
        let i = match index.get(&key) {
            Some(&i) => i,
            None => {
                let i = order.len();
                index.insert(key, i);
                order.push(key);
                lists.push(Vec::new());
                i
            }
        };
        lists[i].push((px, py));
    }
    order.into_iter().zip(lists).collect()
}

/// `get_target_color`（`floodfill.py:103-110`）：取样点的预乘像素；
/// 全透明时 rgb 归零（源码如此，这样"透明目标色"只有一个）。
pub fn get_target_color(
    src: &impl TileSource,
    tx: i32,
    ty: i32,
    px: i32,
    py: i32,
) -> Rgba {
    let mut pixel = src
        .tile(tx, ty)
        .map(|t| t[py as usize][px as usize])
        .unwrap_or([0u16; 4]);
    if pixel[3] == 0 {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
    }
    rgba_of(pixel)
}

/// tile 包围盒（`fill_common.py:92-173` 的 `TileBoundingBox`）。
///
/// 像素 bbox 可能不落在 tile 边界上，于是每块 tile 的有效像素范围不同
/// （`tile_bounds`），并且"跨越 bbox 边"的 tile 不能走均匀捷径
/// （`crossing`）。
pub struct TileBoundingBox {
    min_tx: i32,
    min_ty: i32,
    max_tx: i32,
    max_ty: i32,
    min_px: i32,
    min_py: i32,
    max_px: i32,
    max_py: i32,
    no_tile_crossing: bool,
}

impl TileBoundingBox {
    pub fn new(bbox: (i32, i32, i32, i32)) -> Self {
        let n = TILE_SIZE as i32;
        let (bbx, bby, bbw, bbh) = bbox;
        let bb_rx = bbx + bbw - 1;
        let bb_ry = bby + bbh - 1;
        let min_px = bbx.rem_euclid(n);
        let min_py = bby.rem_euclid(n);
        let max_px = bb_rx.rem_euclid(n);
        let max_py = bb_ry.rem_euclid(n);
        Self {
            min_tx: bbx.div_euclid(n),
            min_ty: bby.div_euclid(n),
            max_tx: bb_rx.div_euclid(n),
            max_ty: bb_ry.div_euclid(n),
            min_px,
            min_py,
            max_px,
            max_py,
            no_tile_crossing: (min_px, min_py, max_px, max_py) == (0, 0, n - 1, n - 1),
        }
    }

    /// 该 tile 内可填的像素范围 `(min_x, min_y, max_x, max_y)`。
    pub fn tile_bounds(&self, tc: (i32, i32)) -> (i32, i32, i32, i32) {
        let n = TILE_SIZE as i32;
        if self.no_tile_crossing {
            return (0, 0, n - 1, n - 1);
        }
        let (tx, ty) = tc;
        let min_x = if tx == self.min_tx { self.min_px } else { 0 };
        let min_y = if ty == self.min_ty { self.min_py } else { 0 };
        let max_x = if tx == self.max_tx { self.max_px } else { n - 1 };
        let max_y = if ty == self.max_ty { self.max_py } else { n - 1 };
        (min_x, min_y, max_x, max_y)
    }

    /// tile 是否整块落在 bbox 之外。
    pub fn outside(&self, tc: (i32, i32)) -> bool {
        let (tx, ty) = tc;
        tx < self.min_tx || tx > self.max_tx || ty < self.min_ty || ty > self.max_ty
    }

    /// tile 是否跨过 bbox 的某条边（跨边的 tile 不能走均匀捷径）。
    pub fn crossing(&self, tc: (i32, i32)) -> bool {
        if self.no_tile_crossing {
            return false;
        }
        let n = TILE_SIZE as i32;
        let (tx, ty) = tc;
        (tx == self.min_tx && self.min_px != 0)
            || (ty == self.min_ty && self.min_py != 0)
            || (tx == self.max_tx && self.max_px != n - 1)
            || (ty == self.max_ty && self.max_py != n - 1)
    }
}

/// 填充结果里的一块 alpha tile（`floodfill.py:498` 的 `filled` 字典值）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AlphaTile {
    /// 整块 alpha 都是 `OPAQUE`（源码的 `_FULL_TILE` 常量）。
    Full,
    /// 整块同一个非不透明 alpha（`_TileFillSkipper.uniform_tile(alpha)`）。
    Uniform(u16),
    /// 逐像素 alpha（行主序 `[y * N + x]`）。
    Data(Box<[u16; PIXELS]>),
}

impl AlphaTile {
    fn new_data() -> Self {
        Self::Data(Box::new([0u16; PIXELS]))
    }

    /// 取逐像素数据；常量 tile 就地展开（内容等价，只是失去"常量"这层优化）。
    ///
    /// 正常流程里不会对 `Full`/`Uniform` 走到这里（它们一律进 `final`，
    /// 不会重跑填充），这里只是让 `filled` 的类型在借出时没有分支。
    pub(crate) fn as_data_mut(&mut self) -> &mut [u16; PIXELS] {
        match self {
            AlphaTile::Data(data) => return data.as_mut(),
            AlphaTile::Full => *self = AlphaTile::Data(Box::new([OPAQUE; PIXELS])),
            AlphaTile::Uniform(alpha) => {
                let alpha = *alpha;
                *self = AlphaTile::Data(Box::new([alpha; PIXELS]));
            }
        }
        match self {
            AlphaTile::Data(data) => data.as_mut(),
            _ => unreachable!("上面刚展开成 Data"),
        }
    }
}

/// tile 队列里的一项（`floodfill.py:504-505` 的 `(tile_coord, seeds, from_dir)`）。
struct QueuedTile {
    coord: (i32, i32),
    seeds: QueuedSeeds,
    from_dir: Edge,
}

/// 队列里的种子（初始像素坐标 或 从某边溢入的段）。
enum QueuedSeeds {
    Pixels(Vec<(i32, i32)>),
    Ranges(Vec<SeedRange>),
}

impl QueuedSeeds {
    /// 把队列表述转成 `Filler::fill` 的入参。
    fn as_seeds(&self, origin: Edge) -> Seeds<'_> {
        match self {
            QueuedSeeds::Pixels(p) => Seeds::Initial(p),
            QueuedSeeds::Ranges(r) => Seeds::Ranges {
                origin,
                ranges: r,
            },
        }
    }
}

/// 均匀 tile 的捷径（`floodfill.py:535-603` 的 `_TileFillSkipper`）。
///
/// `filler` 是**可变**借用：源码里 skipper 与填充循环共享同一个 `Filler`
/// 实例（`floodfill.py:507`），它的像素种子队列要跨 tile 复用。
struct TileFillSkipper<'a> {
    /// 已处理完的 tile（含"均匀且不可填"的）——`tfs.final`。
    final_tiles: HashSet<(i32, i32)>,
    tiles_bbox: &'a TileBoundingBox,
    filler: &'a mut Filler,
}

impl TileFillSkipper<'_> {
    /// `_TileFillSkipper.check`（`floodfill.py:570-603`）。
    ///
    /// 返回 `Some(overflows)` 表示这块 tile 已经处理完（整块填了 / 整块跳过），
    /// 值为四条边的溢出段；返回 `None` 表示必须跑真正的填充。
    fn check(
        &mut self,
        src: &impl TileSource,
        filled: &mut HashMap<(i32, i32), AlphaTile>,
        coord: (i32, i32),
        from_dir: Edge,
    ) -> Option<[Vec<SeedRange>; 4]> {
        if filled.contains_key(&coord) || self.tiles_bbox.crossing(coord) {
            return None;
        }
        let (is_empty, tile) = match src.tile(coord.0, coord.1) {
            Some(t) => (false, t),
            None => (true, TileRef::Borrowed(&EMPTY_TILE)),
        };
        let alpha = self.filler.tile_uniformity(is_empty, &tile)?;
        // 均匀 tile 不会再进填充循环（floodfill.py:596）
        self.final_tiles.insert(coord);
        if alpha == 0 {
            // 不可填：什么也不产出，也不往外溢出
            return Some(std::array::from_fn(|_| Vec::new()));
        }
        if alpha == OPAQUE {
            filled.insert(coord, AlphaTile::Full);
        } else {
            filled.insert(coord, AlphaTile::Uniform(alpha));
        }
        Some(full_overflows(from_dir))
    }
}

/// `_TileFillSkipper.FULL_OVERFLOWS`（`floodfill.py:538-549`）：整块均匀且可填
/// 的 tile，四条边都整条溢出，**除了种子进来的那条边**（否则会回弹）。
fn full_overflows(from_dir: Edge) -> [Vec<SeedRange>; 4] {
    let full = vec![(0, TILE_SIZE as i32 - 1)];
    let skip = if from_dir == Edge::None {
        None
    } else {
        Some(from_dir as usize)
    };
    std::array::from_fn(|i| {
        if Some(i) == skip {
            Vec::new()
        } else {
            full.clone()
        }
    })
}

/// `orthogonal`（`fill_common.py:79-89`）：北、东、南、西。
fn orthogonal(coord: (i32, i32)) -> [(i32, i32); 4] {
    let (tx, ty) = coord;
    [(tx, ty - 1), (tx + 1, ty), (tx, ty + 1), (tx - 1, ty)]
}

/// 从邻边进来的方向（`floodfill.py:500` 的 `inv_edges`）。
const INV_EDGES: [Edge; 4] = [Edge::South, Edge::West, Edge::North, Edge::East];

/// `enqueue_overflows`（`floodfill.py:57-77`）：把四条边的溢出段换成队列项。
fn enqueue_overflows(
    queue: &mut VecDeque<QueuedTile>,
    coord: (i32, i32),
    seeds: [Vec<SeedRange>; 4],
    tiles_bbox: &TileBoundingBox,
) {
    for (i, edge_coord) in orthogonal(coord).into_iter().enumerate() {
        let edge_seeds = &seeds[i];
        if !edge_seeds.is_empty() && !tiles_bbox.outside(edge_coord) {
            queue.push_back(QueuedTile {
                coord: edge_coord,
                seeds: QueuedSeeds::Ranges(edge_seeds.clone()),
                from_dir: INV_EDGES[i],
            });
        }
    }
}

/// `scanline_fill`（`floodfill.py:479-532`）：按 tile 跑扫描线填充，返回
/// `coord -> alpha tile`。
pub fn scanline_fill(
    src: &impl TileSource,
    filler: &mut Filler,
    seed_lists: &SeedLists,
    tiles_bbox: &TileBoundingBox,
) -> HashMap<(i32, i32), AlphaTile> {
    let mut filled: HashMap<(i32, i32), AlphaTile> = HashMap::new();
    let mut queue: VecDeque<QueuedTile> = seed_lists
        .iter()
        .map(|(coord, seeds)| QueuedTile {
            coord: *coord,
            seeds: QueuedSeeds::Pixels(seeds.clone()),
            // 初始种子：`myplib.edges.none`（floodfill.py:505）
            from_dir: Edge::None,
        })
        .collect();
    let mut skipper = TileFillSkipper {
        final_tiles: HashSet::new(),
        tiles_bbox,
        filler,
    };

    while let Some(item) = queue.pop_front() {
        // 这块已经整块处理过了（floodfill.py:511-513）
        if skipper.final_tiles.contains(&item.coord) {
            continue;
        }
        let overflows = skipper.check(src, &mut filled, item.coord, item.from_dir);
        let overflows = match overflows {
            Some(o) => o,
            None => {
                // 真正跑一块 tile 的填充（floodfill.py:519-528）
                let entry = filled
                    .entry(item.coord)
                    .or_insert_with(AlphaTile::new_data);
                let dst = entry.as_data_mut();
                let src_tile = src
                    .tile(item.coord.0, item.coord.1)
                    .unwrap_or(TileRef::Borrowed(&EMPTY_TILE));
                skipper.filler.fill(
                    &src_tile,
                    dst,
                    item.seeds.as_seeds(item.from_dir),
                    tiles_bbox.tile_bounds(item.coord),
                )
            }
        };
        enqueue_overflows(&mut queue, item.coord, overflows, tiles_bbox);
    }
    filled
}

// ---- 填充参数（lib/floodfill.py:198 的 FloodFillArguments） ----

/// 填充的混合模式修饰键（`gui/blendmodehandler.py:53-56` 的 `BlendModes`）。
///
/// 数值顺序照源码：`NORMAL = 0, ERASE = 1, LOCK_ALPHA = 2, COLORIZE = 3`。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FillBlendMode {
    #[default]
    Normal = 0,
    Erase = 1,
    LockAlpha = 2,
    Colorize = 3,
}

/// `_blend_parameters` 的结果：合成模式 + 是否锁 alpha（`fill.py:180-201`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FillBlendParams {
    pub mode: CombineMode,
    pub lock_alpha: bool,
}

/// 一次填充的全部参数（`floodfill.py:198-275` 的 `FloodFillArguments`）。
///
/// 与源码的差异：
/// - `mode` + `lock_alpha` 不直接存，由 [`FillArgs::blend_parameters`] 从
///   [`FillArgs::comp_mode`] + [`FillArgs::blend_mode`] 现算（与源码每次
///   `fill()` 现算一致）。
#[derive(Clone, Debug)]
pub struct FillArgs {
    /// 目标色的取样像素（`fill.py:226` 的 `target_pos`；可与种子不同）。
    pub target_pos: (i32, i32),
    /// 种子像素集合（模型坐标；`fill.py:225` 的 `seeds`）。
    pub seeds: Vec<(i32, i32)>,
    /// 填充色，**线性** RGB 0..1（`fill.py:217-220`：`get_rgb()` 再乘 eotf）。
    pub color: [f64; 3],
    /// 容差 0..1（`fill.py:234`）。
    pub tolerance: f32,
    /// 填充后膨胀/腐蚀的半径（`fill.py:235` 的 `opts.offset`，正膨胀负腐蚀）。
    pub offset: i32,
    /// 填充后高斯模糊的半径（`fill.py:236` 的 `opts.feather`）。
    pub feather: i32,
    /// 画框是否启用（`command.py:582` 的 `fill_args.framed = doc.get_frame_enabled()`）。
    ///
    /// 只影响"膨胀/模糊之后是否按 bbox 裁剪结果"（`floodfill.py:346`）。
    pub framed: bool,
    /// 缺口闭合选项（`fill.py:237` 的 `opts.gap_closing_options`）；
    /// `None` = 普通扫描线填充。
    pub gap_closing: Option<super::fill_gap::GapClosingOptions>,
    /// 目标合成模式（`fill.py:228` 的 `opts.blend_mode`，默认
    /// `DEFAULT_BLEND_MODE = 0` = CombineNormal）。
    pub comp_mode: CombineMode,
    /// 混合模式修饰键当前态（`fill.py:228` 的 `self.bm.active_mode.mode_type`）。
    pub blend_mode: FillBlendMode,
    /// 填充不透明度（`fill.py:240`，默认 1.0）。
    pub opacity: f32,
    /// 填充的像素包围盒（`document.py:1089` 的 `fill_args.bbox`）。
    pub bbox: (i32, i32, i32, i32),
}

impl FillArgs {
    /// `_blend_parameters`（`fill.py:180-201`），逐行照抄。
    pub fn blend_parameters(&self) -> FillBlendParams {
        let mut lock_alpha = false;
        let mut mode = self.comp_mode;
        match self.blend_mode {
            FillBlendMode::LockAlpha => {
                // 锁 alpha：CombineNormal 可以一步合成（改 SourceAtop）
                if mode == CombineMode::Normal {
                    mode = CombineMode::SourceAtop;
                }
                lock_alpha = true;
            }
            // 擦除：优先于其他合成模式
            FillBlendMode::Erase => mode = CombineMode::DestinationOut,
            // 着色：非光谱的 Color 混色 + 锁 alpha
            FillBlendMode::Colorize => {
                mode = CombineMode::Color;
                lock_alpha = true;
            }
            FillBlendMode::Normal => {}
        }
        FillBlendParams { mode, lock_alpha }
    }

    /// `skip_empty_dst`（`floodfill.py:258-264`）：目标 tile 为空时合成无事可做。
    pub fn skip_empty_dst(&self) -> bool {
        let p = self.blend_parameters();
        p.lock_alpha
            || matches!(
                p.mode,
                CombineMode::SourceAtop | CombineMode::DestinationOut | CombineMode::DestinationIn
            )
    }

    /// `no_op`（`floodfill.py:266-275`）：锁 alpha 遇上"会降低背景 alpha"的
    /// 模式时，合成不可能产生任何可见变化 → 整个填充直接不做。
    ///
    /// `MODES_DECREASING_BACKDROP_ALPHA`（`lib/modes.py:197`）由
    /// `combine_mode_info(...)["can_decrease_alpha"]` 生成，与
    /// [`CombineMode::can_decrease_alpha`] 是同一判据。
    pub fn no_op(&self) -> bool {
        let p = self.blend_parameters();
        p.lock_alpha && p.mode.can_decrease_alpha()
    }
}

// ---- lib/floodfill.py:297 `_flood_fill` ----

/// 主填充入口（`_flood_fill`，`floodfill.py:297-348`）。
///
/// `src` 是采样源（只读），`dst` 是目标表面（可写）。返回实际合成过的 tile 数
/// （源码没有这个返回值，这里只为测试/状态方便；内容不受影响）。
///
/// 编排与源码一致：扫描线 → 膨胀/腐蚀 → 高斯模糊 → 合成。`handler`
/// （进度/取消）未移植，填充是同步的。
pub fn flood_fill(src: &impl TileSource, args: &FillArgs, dst: &mut PigmentSurface) -> usize {
    let (_, _, width, height) = args.bbox;
    if width <= 0 || height <= 0 || args.no_op() {
        return 0;
    }
    let tiles_bbox = TileBoundingBox::new(args.bbox);

    // `floodfill.py:318-321` 的安全夹紧
    let offset = args.offset.clamp(-(TILE_SIZE as i32), TILE_SIZE as i32);
    let feather = args.feather.clamp(0, TILE_SIZE as i32);

    // 目标色取自取样点（floodfill.py:324）
    let (tx, ty, px, py) = starting_coordinates(args.target_pos.0, args.target_pos.1);
    let target = get_target_color(src, tx, ty, px, py);
    let mut filler = Filler::new(target, args.tolerance as f64);
    let seed_lists = seeds_by_tile(&args.seeds);

    let mut filled = if let Some(gc) = args.gap_closing {
        super::fill_gap::gap_closing_fill(src, &filler, &seed_lists, &tiles_bbox, gc)
    } else {
        scanline_fill(src, &mut filler, &seed_lists, &tiles_bbox)
    };
    // Dilate/Erode（floodfill.py:336-338）
    if offset != 0 {
        filled = super::fill_morphology::morph(offset, filled);
    }
    // Feather（floodfill.py:340-342）
    if feather != 0 {
        filled = super::fill_morphology::blur(feather, filled);
    }
    composite(dst, args, &filled, &tiles_bbox)
}

/// 就地在单个表面里填充（`lib/layer/data.py:1360` 的 src==dst 情形）。
///
/// 两阶段：先只读扫描出 `filled`（alpha tile 表），再合成写回 —— 与 Python
/// 侧"先 scanline_fill 再 composite"的结构一致，也避开同一表面的双重借用。
pub fn flood_fill_surface(surface: &mut PigmentSurface, args: &FillArgs) -> usize {
    let (_, _, width, height) = args.bbox;
    if width <= 0 || height <= 0 || args.no_op() {
        return 0;
    }
    let tiles_bbox = TileBoundingBox::new(args.bbox);
    let offset = args.offset.clamp(-(TILE_SIZE as i32), TILE_SIZE as i32);
    let feather = args.feather.clamp(0, TILE_SIZE as i32);
    let (tx, ty, px, py) = starting_coordinates(args.target_pos.0, args.target_pos.1);
    let target = get_target_color(&*surface, tx, ty, px, py);
    let mut filler = Filler::new(target, args.tolerance as f64);
    let seed_lists = seeds_by_tile(&args.seeds);
    let mut filled = if let Some(gc) = args.gap_closing {
        super::fill_gap::gap_closing_fill(&*surface, &filler, &seed_lists, &tiles_bbox, gc)
    } else {
        scanline_fill(&*surface, &mut filler, &seed_lists, &tiles_bbox)
    };
    if offset != 0 {
        filled = super::fill_morphology::morph(offset, filled);
    }
    if feather != 0 {
        filled = super::fill_morphology::blur(feather, filled);
    }
    composite(surface, args, &filled, &tiles_bbox)
}

/// `rgba_tile_from_alpha_tile`（`floodfill.cpp:387-407`）：把 alpha tile 变成
/// 预乘了填充色的 rgba tile（行主序，`[y * N + x] * 4`）。
///
/// `bounds = (min_x, min_y, max_x, max_y)` 是源码的四个裁剪参数：只写这块
/// 矩形，其余保持 0。bbox 不跨界时是整块 `(0,0,N-1,N-1)`；膨胀/模糊之后
/// 跨界的 tile 用 bbox 的 `tile_bounds`。未写区域在源码里也是 0。
fn rgba_tile_from_alpha(
    alpha: &[u16],
    fill: [f64; 3],
    bounds: (i32, i32, i32, i32),
) -> Vec<u16> {
    let (min_x, min_y, max_x, max_y) = bounds;
    let mut out = vec![0u16; PIXELS * 4];
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let i = (y as usize) * TILE_SIZE + x as usize;
            let a = alpha[i];
            let px = Rgba::from_straight(fill[0], fill[1], fill[2], a);
            out[i * 4] = px.red;
            out[i * 4 + 1] = px.green;
            out[i * 4 + 2] = px.blue;
            out[i * 4 + 3] = px.alpha;
        }
    }
    out
}

/// 整块同 alpha 的 rgba tile（对应 `_FULL_TILE` 版与均匀 tile 版）。
fn rgba_tile_uniform(alpha: u16, fill: [f64; 3]) -> Vec<u16> {
    let n = TILE_SIZE as i32;
    rgba_tile_from_alpha(&vec![alpha; PIXELS], fill, (0, 0, n - 1, n - 1))
}

/// `composite`（`floodfill.py:371-476`）：把填充结果合成进目标表面。
fn composite(
    dst: &mut PigmentSurface,
    args: &FillArgs,
    filled: &HashMap<(i32, i32), AlphaTile>,
    tiles_bbox: &TileBoundingBox,
) -> usize {
    let params = args.blend_parameters();
    let skip_empty_dst = args.skip_empty_dst();
    let full_rgba = rgba_tile_uniform(OPAQUE, args.color);
    // `floodfill.py:346`：只有"画框启用 + 膨胀/模糊"时才按 bbox 裁剪结果。
    let trim_result = args.framed && (args.offset > 0 || args.feather != 0);
    let n = TILE_SIZE as i32;

    // 坐标排序只为确定性（源码顺序来自 dict；合成结果与顺序无关）
    let mut coords: Vec<(i32, i32)> = filled.keys().copied().collect();
    coords.sort_unstable();

    let mut count = 0;
    for coord in coords {
        let tile = &filled[&coord];
        // 膨胀/模糊会把 tile 撑出 bbox；只有画框启用时才裁掉
        // （`floodfill.py:402-405`）。
        if trim_result && tiles_bbox.outside(coord) {
            continue;
        }
        // 擦除/锁 alpha 时空目标 tile 无事可做（floodfill.py:407-410）。
        //
        // 判据差异（记账）：源码用 `tile_coord not in dst.get_tiles()`，MyPaint
        // 会把"写空"的 tile 从 tiledict 里摘掉；我们的表面保留已分配的 tile，
        // 所以这里可能把一块全零 tile 当成"非空"多做一次合成 —— 结果不变
        // （空目标上做 DestinationOut/SourceAtop 本来就不改像素），只是慢一点。
        if skip_empty_dst && dst.tile(coord.0, coord.1).is_none() {
            continue;
        }
        // 跨界 tile 不能整块拷贝/整块清空（`floodfill.py:419` 的 cut_off）。
        let cut_off = trim_result && tiles_bbox.crossing(coord);
        let full_inner = matches!(tile, AlphaTile::Full) && !cut_off;

        let dst_tile = dst.tile_request_start(coord.0, coord.1);
        let dst_flat = dst_tile.as_flattened_mut().as_flattened_mut();
        if full_inner {
            // 直接拷贝/清空/跳过三条快捷路径（`floodfill.py:421-431`）。
            if params.mode == CombineMode::Normal && args.opacity == 1.0 {
                dst_flat.copy_from_slice(&full_rgba);
                dst.tile_request_end(coord.0, coord.1);
                count += 1;
                continue;
            }
            if params.mode == CombineMode::DestinationOut && args.opacity == 1.0 {
                dst_flat.fill(0);
                dst.tile_request_end(coord.0, coord.1);
                count += 1;
                continue;
            }
            if params.mode == CombineMode::DestinationIn && args.opacity == 1.0 {
                dst.tile_request_end(coord.0, coord.1);
                continue;
            }
        }
        let bounds = if trim_result {
            tiles_bbox.tile_bounds(coord)
        } else {
            (0, 0, n - 1, n - 1)
        };
        let src_rgba = match tile {
            AlphaTile::Full => full_rgba.clone(),
            AlphaTile::Uniform(alpha) => {
                let data = vec![*alpha; PIXELS];
                rgba_tile_from_alpha(&data, args.color, bounds)
            }
            AlphaTile::Data(data) => rgba_tile_from_alpha(data.as_ref(), args.color, bounds),
        };
        if params.lock_alpha && params.mode != CombineMode::SourceAtop {
            // 锁 alpha 且模式不是 SourceAtop：先合成一次，再用保留的 dst 副本
            // 以 DestinationAtop 把结果掩回原 alpha（floodfill.py:441-447）
            let mask = dst_flat.to_vec();
            combine::tile_combine(params.mode, &src_rgba, dst_flat, true, args.opacity);
            combine::tile_combine(CombineMode::DestinationAtop, &mask, dst_flat, true, 1.0);
        } else {
            combine::tile_combine(params.mode, &src_rgba, dst_flat, true, args.opacity);
        }
        dst.tile_request_end(coord.0, coord.1);
        count += 1;
    }

    // DestinationIn / DestinationAtop：没被填到的目标 tile 必须清空
    // （floodfill.py:451-459）。
    //
    // 记账：源码是 `dst_tiles.pop(tile_coord)`（直接删 tile）；我们的表面
    // 没有"删 tile"的公共入口（GPU 镜像靠 revision 自愈），所以改写全零像素。
    // 内容语义与删 tile 相同（空 tile），区别只在内存占用与 tile_count。
    if params.mode == CombineMode::DestinationIn || params.mode == CombineMode::DestinationAtop {
        let untouched: Vec<(i32, i32)> = dst
            .tiles()
            .map(|(coord, _)| coord)
            .filter(|coord| !filled.contains_key(coord))
            .collect();
        for coord in untouched {
            let dst_tile = dst.tile_request_start(coord.0, coord.1);
            dst_tile
                .as_flattened_mut()
                .as_flattened_mut()
                .fill(0);
            dst.tile_request_end(coord.0, coord.1);
            count += 1;
        }
    }
    count
}

// ---- gui/fill.py：FloodFillMode ----

/// 填充命令的展示名（`lib/command.py:564` 的 `display_name = _("Flood Fill")`）。
pub const DISPLAY_NAME: &str = "Flood Fill";

/// 填充模式的单选动作名（`fill.py:45` 的 `ACTION_NAME`）。
pub const ACTION_NAME: &str = "FloodFillMode";

/// 缺口检测开启时的**光标动作名**（`fill.py:46` 的 `GC_ACTION_NAME`）。
///
/// 源码只为它注册了一个 `GtkAction`（`resources.xml:1952-1957`），没有
/// activate 处理器 —— 用途是让 `app.cursors.get_action_cursor()` 取到
/// `mypaint-fill-gc-symbolic` 那枚光标。它**不是**模式类，所以不参与
/// `ModeRegistry.get_action_names()`；本移植把它作为
/// [`FillMode::cursor_action_name`] 的返回值保留，供以后的自绘光标使用。
/// 记账：本应用目前不画笔刷/工具光标（`cursor.*` 整块未移植），所以这个
/// id 还没有消费方。
pub const GC_ACTION_NAME: &str = "FloodFillGCMode";

/// 填充模式的可调项（`gui/fill.py:433` 的 `FloodFillOptionsWidget`）。
///
/// 默认值照源码类常量（`fill.py:450-462`）。
#[derive(Clone, Debug)]
pub struct FillOptions {
    /// 容差（`DEFAULT_TOLERANCE = 0.05`）。
    pub tolerance: f32,
    /// 不透明度（`DEFAULT_OPACITY = 1.0`）。
    pub opacity: f32,
    /// 目标合成模式（`DEFAULT_BLEND_MODE = 0` → CombineNormal）。
    pub comp_mode: CombineMode,
    /// 混合模式修饰键当前态（`BlendModes.NORMAL`）。
    pub blend_mode: FillBlendMode,
    /// 从所有可见层的临时合成里取样（`DEFAULT_SAMPLE_MERGED = False`）。
    pub sample_merged: bool,
    /// 取样源图层 id（`fill.py:965-970` 的 `src_path`；`None` = 当前层）。
    ///
    /// 源码存的是图层对象路径，并在插层/删层/撤销时修正；这里用稳定 id，
    /// 到填充命令 `redo()` 时再解析成路径，插层后不会指错层。
    pub src_layer: Option<u64>,
    /// 把填充限制在当前视口内（`DEFAULT_LIM_TO_VIEW = False`）。
    pub limit_to_view: bool,
    /// 这一次填充写到新层（`DEFAULT_MAKE_NEW_LAYER = False`，用完自动复位）。
    pub make_new_layer: bool,
    /// 膨胀（正）/腐蚀（负）像素数（`DEFAULT_OFFSET = 0`）。
    pub offset: i32,
    /// 高斯模糊半径（`DEFAULT_FEATHER = 0`）。
    pub feather: i32,
    /// 是否启用缺口检测（`DEFAULT_GAP_CLOSING = False`）。
    pub gap_closing: bool,
    /// 最大缺口长度（`DEFAULT_GAP_SIZE = 5`）。
    pub gap_size: i32,
    /// 是否回吸渗漏（`DEFAULT_RETRACT_SEEPS = True`）。
    pub retract_seeps: bool,
}

impl Default for FillOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.05,
            opacity: 1.0,
            comp_mode: CombineMode::Normal,
            blend_mode: FillBlendMode::Normal,
            sample_merged: false,
            src_layer: None,
            limit_to_view: false,
            make_new_layer: false,
            offset: 0,
            feather: 0,
            gap_closing: false,
            gap_size: 5,
            retract_seeps: true,
        }
    }
}

/// 填充模式（`gui/fill.py:40` 的 `FloodFillMode`）。
///
/// 源码是 `ScrollableModeMixin + DragMode`：滚轮管视图，按下→拖动收集种子→
/// 松开时一次性填充。`SPRING_LOADED = False`（`fill.py:48`），所以不会因为
/// 松开修饰键而出栈。
pub struct FillMode {
    /// 拖拽基础设施（`DragMode`：记录起点按钮/修饰键）。
    pub drag: DragMode,
    /// 填充选项。
    pub options: FillOptions,
    /// 种子像素集合（`fill.py:142` 的 `_seed_pixels`；用 Vec 保插入序，
    /// 语义等价于 set）。
    seeds: Vec<(i32, i32)>,
    /// 目标色取样位置（`fill.py:144` 的 `_target_pos`）。
    target_pos: Option<(i32, i32)>,
    /// 当前位置能否填充（`fill.py:81` 的 `_fill_permitted`）。
    fill_permitted: bool,
    /// 最近一次指针位置（`fill.py:82-83` 的 `_x`/`_y`，模型坐标）。
    last_pos: Option<(f32, f32)>,
    /// 最近一次指针事件所属视口的模型包围盒（`tdw.get_corners_model_coords()`
    /// + `helpers.rotated_rectangle_bbox`，`fill.py:222-224`）。
    view_bbox: Option<(i32, i32, i32, i32)>,
}

impl Default for FillMode {
    fn default() -> Self {
        Self::new()
    }
}

impl FillMode {
    pub fn new() -> Self {
        Self {
            drag: DragMode {
                // `SPRING_LOADED = False`（`fill.py:48`）
                spring_loaded: false,
                ..DragMode::default()
            },
            options: FillOptions::default(),
            seeds: Vec::new(),
            target_pos: None,
            fill_permitted: true,
            last_pos: None,
            view_bbox: None,
        }
    }

    /// 当前应使用的光标动作名 —— `fill.py:93-97` 的 `cursor` 属性。
    ///
    /// 缺口检测开着时返回 [`GC_ACTION_NAME`]，否则返回 [`ACTION_NAME`]。
    /// 源码用这个 id 去取两枚不同的光标图；本移植还没有光标绘制，先保留
    /// 语义与 id，供 `cursor.*` 移植时直接接上。
    pub fn cursor_action_name(&self) -> &'static str {
        if self.options.gap_closing {
            GC_ACTION_NAME
        } else {
            ACTION_NAME
        }
    }

    /// 已收集的种子（测试/调试用）。
    #[inline]
    pub fn seeds(&self) -> &[(i32, i32)] {
        &self.seeds
    }

    /// 把模型坐标截成整数像素（`fill.py:156` 的 `int(x)`：向零截断）。
    #[inline]
    fn seed_pixel(x: f32, y: f32) -> (i32, i32) {
        (x as i32, y as i32)
    }

    /// `_update_ui` 里的可填判定（`fill.py:279-286`）。
    ///
    /// 目标层可填 = `visible and not locked`（**只看本层**，与
    /// `target_layer.visible and not target_layer.locked` 一致；组也算可填，
    /// 因为命令会自动新建一层）。勾了 "New Layer" 时目标层不存在，
    /// 源码直接 `permitted = True`。画框启用时，点还必须落在画框里。
    fn permitted_at(&self, ctx: &ModeContext<'_>, x: f32, y: f32) -> bool {
        let permitted = self.options.make_new_layer
            || ctx.doc.layers().current_fill_permitted();
        if !permitted {
            return false;
        }
        if ctx.doc.get_frame_enabled() {
            let (fx, fy, fw, fh) = ctx.doc.get_frame();
            let (fx2, fy2) = (fx + fw, fy + fh);
            if !(fx as f32 <= x && x < fx2 as f32 && fy as f32 <= y && y < fy2 as f32) {
                return false;
            }
        }
        true
    }

    /// `drag_start_cb`（`fill.py:161-171`）：建种子集合、记取样点、首个种子。
    fn begin_drag(&mut self, ctx: &mut ModeContext<'_>, start_x: f32, start_y: f32) {
        self.seeds.clear();
        // 取样点：源码存的是**浮点**模型坐标（`fill.py:167`），到
        // `starting_coordinates` 里才 `int(x // N)` / `int(x % N)`（向下取整）。
        // 先 floor 再整除，与"先 `//`/`%` 再 int 截断"在负数上等价
        // （如 -10.7 → tile -1 / 像素 53），所以这里用 floor。
        self.target_pos = Some((start_x.floor() as i32, start_y.floor() as i32));
        self.fill_permitted = self.permitted_at(ctx, start_x, start_y);
        if self.fill_permitted {
            // 种子是 `int(x)`（向零截断，`fill.py:156`）
            self.seeds.push(Self::seed_pixel(start_x, start_y));
        }
    }

    /// `drag_update_cb`（`fill.py:153-159`）：把经过的像素不断加进种子集合。
    fn extend_drag(&mut self, ctx: &mut ModeContext<'_>, x: f32, y: f32) {
        self.last_pos = Some((x, y));
        self.fill_permitted = self.permitted_at(ctx, x, y);
        let seed = Self::seed_pixel(x, y);
        if self.fill_permitted && !self.seeds.contains(&seed) {
            self.seeds.push(seed);
        }
    }

    /// 松开：跑一次填充（`drag_stop_cb`，`fill.py:173-178` + `fill()`，`:203`）。
    ///
    /// 返回是否真的产生了填充命令。
    fn end_drag(&mut self, ctx: &mut ModeContext<'_>) -> bool {
        let seeds = std::mem::take(&mut self.seeds);
        if seeds.is_empty() {
            return false;
        }
        // `fill.py:213-216`：勾了 "New Layer" 或当前层 `get_fillable()` False
        // （组/锁定层）时，命令会先建一层再填。隐藏层在 `_fill_permitted`
        // 那一步就收不到种子，与源码一致。
        let make_new_layer =
            self.options.make_new_layer || !ctx.doc.layers().current_is_fillable();
        let color = current_color_linear(ctx.brush);
        // 包围盒（document.py:1064-1089）：必须在拿可变 surface 之前算好
        let view_bbox = if self.options.limit_to_view {
            self.view_bbox
        } else {
            None
        };
        let bbox = ctx.doc.fill_bbox(&seeds, view_bbox);
        let args = FillArgs {
            target_pos: self.target_pos.unwrap_or(seeds[0]),
            seeds,
            color,
            tolerance: self.options.tolerance,
            offset: self.options.offset,
            feather: self.options.feather,
            framed: ctx.doc.get_frame_enabled(),
            gap_closing: if self.options.gap_closing {
                Some(super::fill_gap::GapClosingOptions {
                    max_gap_size: self.options.gap_size,
                    retract_seeps: self.options.retract_seeps,
                })
            } else {
                None
            },
            comp_mode: self.options.comp_mode,
            blend_mode: self.options.blend_mode,
            opacity: self.options.opacity,
            bbox,
        };
        // 源码 `src_path=None` 表示"当前层"（`command.py:596-599`）。命令要拿
        // 稳定 id，而不是此刻的路径：勾了 "New Layer (once)" 时新层会插在
        // 当前层位置、把原层往下挤，路径会指错。
        let src_layer = self
            .options
            .src_layer
            .unwrap_or_else(|| ctx.current_layer_id());
        let cmd = crate::command::FloodFill::new(
            ctx.current_layer_id(),
            src_layer,
            args,
            self.options.sample_merged,
            make_new_layer,
        );
        let ok = ctx.doc.do_command(Box::new(cmd));
        // `fill.py:254`：`opts.make_new_layer = False`（一次性开关）
        self.options.make_new_layer = false;
        ok
    }
}

/// 当前画笔颜色，线性 RGB（`fill.py:211-220`）。
///
/// 源码取 `doc.app.brush_color_manager.get_color().get_rgb()`，再按 eotf
/// （默认 2.2）转线性。我们的"当前颜色"就是笔刷的 `color_h/s/v`（与绘制
/// 同一个入口，`derive_active` 写入），而 hokusai 的 `hsv_to_rgb` 直接在
/// **线性** RGB 上工作（libmypaint 的约定），终点与源码的
/// `sRGB → eotf` 一致。
pub fn current_color_linear(brush: &Brush) -> [f64; 3] {
    let rgb = hsv_to_rgb(Hsv {
        h: brush.get(BrushSetting::ColorH).base_value,
        s: brush.get(BrushSetting::ColorS).base_value,
        v: brush.get(BrushSetting::ColorV).base_value,
    });
    [rgb.r as f64, rgb.g as f64, rgb.b as f64]
}

impl InteractionMode for FillMode {
    /// `get_name()`（`fill.py:120-127`）。
    fn name(&self) -> &'static str {
        "Flood Fill"
    }

    fn fill_options(&self) -> Option<FillOptions> {
        Some(self.options.clone())
    }

    fn set_fill_options(&mut self, options: FillOptions) {
        self.options = options;
    }

    /// `get_usage()`（`fill.py:129-130`）。
    fn usage(&self) -> &str {
        "Fill areas with color"
    }

    /// `ACTION_NAME`（`fill.py:45`）。
    fn action_name(&self) -> Option<&'static str> {
        Some(ACTION_NAME)
    }

    /// `fill.py:93-97` 的 `cursor` 属性：缺口检测开/关对应两个动作 id。
    fn cursor_action_name(&self) -> Option<&'static str> {
        Some(self.cursor_action_name())
    }

    /// `pointer_behavior = Behavior.PAINT_NOBRUSH`（`fill.py:76`）。
    fn pointer_behavior(&self) -> Behavior {
        Behavior::PAINT_NOBRUSH
    }

    /// `scroll_behavior = Behavior.CHANGE_VIEW`（`fill.py:77`）。
    fn scroll_behavior(&self) -> Behavior {
        Behavior::CHANGE_VIEW
    }

    fn enter(&mut self, ctx: &mut ModeContext<'_>) {
        // `FloodFillMode.enter`（fill.py:104-111）：重置拖拽状态并刷新许可
        self.drag = DragMode {
            spring_loaded: false,
            ..DragMode::default()
        };
        self.seeds.clear();
        self.target_pos = None;
        self.last_pos = None;
        self.view_bbox = None;
        self.fill_permitted =
            self.options.make_new_layer || ctx.doc.layers().current_fill_permitted();
    }

    fn leave(&mut self, _ctx: &mut ModeContext<'_>) {
        // `DragMode.leave`（mode.py:1204）→ `_stop_drag()`
        self.drag.drag.stop();
        self.seeds.clear();
        self.target_pos = None;
        self.fill_permitted = false;
    }

    fn button_press(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        // 我们的画布只把左键喂给模式（与 line.rs 一致）
        if ev.button != 1 {
            return false;
        }
        self.drag.enter(ev.modifiers);
        self.drag.drag.start(ev.x, ev.y);
        self.drag.drag.start_button = Some(ev.button);
        self.view_bbox = ev.view_bbox;
        self.begin_drag(ctx, ev.x, ev.y);
        true
    }

    fn motion(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        // `motion_notify_cb`（fill.py:256-263）：不在拖拽中也记位置（刷光标）
        self.view_bbox = ev.view_bbox;
        self.last_pos = Some((ev.x, ev.y));
        if self.drag.drag.in_drag {
            self.extend_drag(ctx, ev.x, ev.y);
            return true;
        }
        false
    }

    fn button_release(&mut self, ctx: &mut ModeContext<'_>, ev: &PointerEvent) -> bool {
        if !self.drag.drag.in_drag || ev.button != self.drag.drag.start_button.unwrap_or(1) {
            return false;
        }
        self.drag.drag.stop();
        self.end_drag(ctx);
        true
    }

    fn key_press(&mut self, _ctx: &mut ModeContext<'_>, _key: Key) -> bool {
        // `DragMode.key_press_cb`（mode.py:1258-1266）：拖拽中一律吃掉按键，
        // 免得 Esc 之类的键在拖拽中途动模式栈。
        self.drag.drag.in_drag
    }

    fn process_pending(&mut self, _ctx: &mut ModeContext<'_>) -> bool {
        // 填充是同步的，没有排队输入（源码的排队只用于拖拽预览叠加层）
        false
    }

    // ---- 框架的 drag_* 钩子（GUI 亦可直接调，等价于 button_* 内部流程）----

    fn drag_start(&mut self, ctx: &mut ModeContext<'_>) {
        if let (Some(x), Some(y)) = (self.drag.drag.start_x, self.drag.drag.start_y) {
            self.begin_drag(ctx, x, y);
        }
    }

    fn drag_update(&mut self, ctx: &mut ModeContext<'_>, _dx: f32, _dy: f32) {
        if let Some((x, y)) = self.last_pos {
            self.extend_drag(ctx, x, y);
        }
    }

    fn drag_stop(&mut self, ctx: &mut ModeContext<'_>) {
        self.end_drag(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn options_roundtrip_through_the_mode_trait() {
        let mut mode = FillMode::new();
        let mut o = mode.fill_options().expect("填充模式应有选项");
        assert_eq!(o.tolerance, 0.05);
        o.tolerance = 0.42;
        o.opacity = 0.5;
        o.comp_mode = CombineMode::Multiply;
        mode.set_fill_options(o);
        let got = mode.fill_options().unwrap();
        assert_eq!(got.tolerance, 0.42);
        assert_eq!(got.opacity, 0.5);
        assert_eq!(got.comp_mode, CombineMode::Multiply);
    }

    // ---- 测试脚手架：内存里的只读 tile 源 ----

    /// 一块"小型画布"：tile → 像素。
    struct TestSource {
        tiles: HashMap<(i32, i32), Box<TilePixels>>,
    }

    impl TestSource {
        fn new() -> Self {
            Self {
                tiles: HashMap::new(),
            }
        }

        /// 用闭包填一块 tile（`f(x, y)` 给像素）。
        fn with_tile(mut self, tx: i32, ty: i32, f: impl Fn(usize, usize) -> [u16; 4]) -> Self {
            let mut tile = hokusai::tile::empty_tile();
            for (y, row) in tile.iter_mut().enumerate() {
                for (x, px) in row.iter_mut().enumerate() {
                    *px = f(x, y);
                }
            }
            self.tiles.insert((tx, ty), tile);
            self
        }
    }

    impl TileSource for TestSource {
        fn tile(&self, tx: i32, ty: i32) -> Option<TileRef<'_>> {
            self.tiles.get(&(tx, ty)).map(|t| TileRef::Borrowed(t.as_ref()))
        }
    }

    /// 不透明像素（alpha = 32768）。
    fn opaque(r: u16, g: u16, b: u16) -> [u16; 4] {
        [r, g, b, OPAQUE]
    }

    /// 读目标表面的像素。
    fn dst_px(surface: &PigmentSurface, x: i32, y: i32) -> [u16; 4] {
        let (tx, ty, px, py) = starting_coordinates(x, y);
        surface
            .tile(tx, ty)
            .map(|t| t[py as usize][px as usize])
            .unwrap_or([0; 4])
    }

    /// 一个只填一块 tile 的参数集（红色）。
    fn args_over(bbox: (i32, i32, i32, i32), seeds: Vec<(i32, i32)>) -> FillArgs {
        FillArgs {
            target_pos: seeds[0],
            seeds,
            color: [1.0, 0.0, 0.0],
            tolerance: 0.05,
            offset: 0,
            feather: 0,
            framed: false,
            gap_closing: None,
            comp_mode: CombineMode::Normal,
            blend_mode: FillBlendMode::Normal,
            opacity: 1.0,
            bbox,
        }
    }

    // ---- 阈值函数 ----

    /// 阈值曲线：`dist ≤ tol` 拉满，`tol < dist ≤ 1.5 tol` 线性斜坡，更远为 0。
    ///
    /// 这三段直接对应 `floodfill.cpp:112-122`。
    #[test]
    fn threshold_curve_matches_source() {
        let target = Rgba::premultiplied(1000, 1000, 1000, OPAQUE);
        let tolerance = 0.05f64;
        let filler = Filler::new(target, tolerance);
        let tol = (tolerance * ONE as f64) as u32; // 1638

        // 构造时就把目标色去预乘；不透明像素的直通色 = 预乘色，所以恒等
        assert_eq!(filler.target_color(), target);

        // 完全同色 → 拉满
        assert_eq!(filler.pixel_fill_alpha(&target), OPAQUE);
        // 恰好等于容差 → 仍是满 alpha（斜坡从 dist == tol 之后才开始）
        let at_tol = Rgba::premultiplied(1000 + tol as u16, 1000, 1000, OPAQUE);
        assert_eq!(filler.pixel_fill_alpha(&at_tol), OPAQUE);
        // 1.25 倍容差 → 半程斜坡，约为 0.5
        let mid = Rgba::premultiplied(1000 + (tol as u16 * 5 / 4), 1000, 1000, OPAQUE);
        let alpha = filler.pixel_fill_alpha(&mid);
        assert!(
            (alpha as i32 - OPAQUE as i32 / 2).abs() <= 64,
            "1.25 倍容差应落在半程（实得 {alpha}）"
        );
        // 1.5 倍容差之外 → 0
        let beyond = Rgba::premultiplied(1000 + (tol as u16 * 2), 1000, 1000, OPAQUE);
        assert_eq!(filler.pixel_fill_alpha(&beyond), 0);
        // 斜坡单调递减
        let mut last = OPAQUE as i32 + 1;
        for step in 1..=10u16 {
            let d = tol as u16 + step * (tol as u16 / 10);
            let a = filler.pixel_fill_alpha(&Rgba::premultiplied(1000 + d, 1000, 1000, OPAQUE));
            assert!((a as i32) <= last, "斜坡必须单调不增");
            last = a as i32;
        }

        // 双方全透明 → 命中（`floodfill.cpp:99-100`）
        let transparent = Filler::new(Rgba::TRANSPARENT, tolerance);
        assert_eq!(
            transparent.pixel_fill_alpha(&Rgba::TRANSPARENT),
            OPAQUE,
            "全透明对全透明应直接命中"
        );

        // 目标透明 → 距离 = 被检像素 alpha（`floodfill.cpp:105-106`）
        let from_transparent = Filler::new(Rgba::TRANSPARENT, 0.5);
        assert_eq!(
            from_transparent.pixel_fill_alpha(&Rgba::premultiplied(5000, 5000, 5000, OPAQUE)),
            0,
            "不透明像素离透明目标太远"
        );
        assert_eq!(
            from_transparent.pixel_fill_alpha(&Rgba::premultiplied(0, 0, 0, 1000)),
            OPAQUE,
            "低 alpha 像素在容差内"
        );

        // 容差 0：只认预乘色完全相等（`floodfill.cpp:101-103`）
        let exact = Filler::new(target, 0.0);
        assert_eq!(exact.pixel_fill_alpha(&target), OPAQUE);
        assert_eq!(
            exact.pixel_fill_alpha(&Rgba::premultiplied(1001, 1000, 1000, OPAQUE)),
            0
        );
    }

    /// `tile_uniformity` / `flood`：均匀判定与整块 alpha。
    #[test]
    fn uniformity_and_flood_match_source() {
        let mut uniform = hokusai::tile::empty_tile();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                uniform[y][x] = opaque(1000, 1000, 1000);
            }
        }
        let filler = Filler::new(Rgba::premultiplied(1000, 1000, 1000, OPAQUE), 0.05);
        assert_eq!(filler.tile_uniformity(false, &uniform), Some(OPAQUE));
        // 空 tile 走的是"透明像素"分支，结果与逐像素算出来的一致
        assert_eq!(
            filler.tile_uniformity(true, &EMPTY_TILE),
            Some(filler.pixel_fill_alpha(&Rgba::TRANSPARENT))
        );

        // 一个像素不同 → 不再均匀
        let mut mixed = uniform.clone();
        mixed[10][10] = opaque(5000, 0, 0);
        assert_eq!(filler.tile_uniformity(false, &mixed), None);

        // flood 逐像素给出 alpha
        let mut alpha = [0u16; PIXELS];
        filler.flood(&mixed, &mut alpha);
        assert_eq!(alpha[idx(0, 0)], OPAQUE, "同色像素满 alpha");
        assert_eq!(alpha[idx(10, 10)], 0, "越界颜色为 0");
    }

    // ---- 单 tile / 多 tile 填充 ----

    /// 整块均匀的源 tile 走捷径：`filled` 里是 `Full`，目标整块被填色。
    #[test]
    fn uniform_tile_shortcut_fills_the_whole_tile() {
        let src = TestSource::new().with_tile(0, 0, |_, _| opaque(1000, 1000, 1000));
        let args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(5, 5)]);
        let mut dst = PigmentSurface::new();
        let n = flood_fill(&src, &args, &mut dst);
        assert_eq!(n, 1, "只该合成一块 tile");

        // 四角都应是填充色的不透明像素
        for (x, y) in [(0, 0), (63, 0), (0, 63), (63, 63)] {
            let px = dst_px(&dst, x, y);
            assert_eq!(px[3], OPAQUE, "({x},{y}) 应被填成不透明");
            assert_eq!(px[0], OPAQUE, "红色通道应为满值（fill color = 1.0）");
            assert_eq!((px[1], px[2]), (0, 0));
        }
    }

    /// 渐变阈值：`x*100` 的红色通道，填充在第 25 列停下（斜坡段是半透明）。
    ///
    /// 期望（按 `pixel_fill_alpha` 的三段）：`x ≤ 16` 满、`17..=24` 斜坡、
    /// `x ≥ 25` 完全不填。
    #[test]
    fn gradient_threshold_gives_a_soft_edge() {
        let src = TestSource::new().with_tile(0, 0, |x, y| {
            let _ = y;
            opaque((x as u16) * 100, 0, 0)
        });
        let mut args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(0, 0)]);
        // 目标色取 (0,0)：红 0
        args.target_pos = (0, 0);
        let mut dst = PigmentSurface::new();
        flood_fill(&src, &args, &mut dst);

        for x in 0..=16i32 {
            assert_eq!(dst_px(&dst, x, 0)[3], OPAQUE, "x={x} 在容差内应满 alpha");
        }
        let mut prev = OPAQUE as i32;
        for x in 17..=24i32 {
            let a = dst_px(&dst, x, 0)[3] as i32;
            assert!(a > 0 && a < OPAQUE as i32, "x={x} 应是斜坡（实得 {a}）");
            assert!(a < prev, "斜坡应单调下降（x={x}）");
            prev = a;
        }
        for x in 25..32i32 {
            assert_eq!(dst_px(&dst, x, 0)[3], 0, "x={x} 超出 1.5 倍容差，不该填");
        }
        // 填充是四连通的：同一列往下都被填（第 0 列红色恒为 0，一直在容差内），
        // 而右侧 x≥25 的整列都没动。
        assert_eq!(dst_px(&dst, 0, 40)[3], OPAQUE, "同列下方应连通填上");
        assert_eq!(dst_px(&dst, 40, 40)[3], 0, "斜坡之外的列不该动");
    }

    /// 跨 tile：均匀 tile 通过溢出段把填充传给邻块，邻块在颜色边界处停下。
    ///
    /// 这条同时验证 `queue_ranges`（从西边进来的整段种子）与
    /// `input_seeds` 过滤（不把种子原样回弹给来源 tile）。
    #[test]
    fn fill_crosses_the_tile_edge_and_stops_at_a_colour_boundary() {
        let src = TestSource::new()
            .with_tile(0, 0, |_, _| opaque(1000, 0, 0))
            .with_tile(1, 0, |x, _| {
                if x < 32 {
                    opaque(1000, 0, 0)
                } else {
                    // 与目标色差 20000 fix15，远超 1.5 倍容差（≈2457）
                    opaque(0, 20000, 0)
                }
            });
        let args = args_over(
            (0, 0, 2 * TILE_SIZE as i32, TILE_SIZE as i32),
            vec![(5, 5)],
        );
        let mut dst = PigmentSurface::new();
        let n = flood_fill(&src, &args, &mut dst);
        assert_eq!(n, 2, "两块 tile 都该被填");

        // 左块整块填满
        assert_eq!(dst_px(&dst, 63, 0)[3], OPAQUE);
        // 右块：x<32（全局 x < 96）填，x≥32 不填
        assert_eq!(dst_px(&dst, 64 + 31, 10)[3], OPAQUE, "右块左半边应填上");
        assert_eq!(dst_px(&dst, 64 + 32, 10)[3], 0, "颜色边界处必须停下");
        assert_eq!(dst_px(&dst, 127, 63)[3], 0);
    }

    /// 起点落在透明处：只填透明连通区，不碰不透明像素。
    #[test]
    fn transparent_seed_fills_only_the_transparent_region() {
        let src = TestSource::new().with_tile(0, 0, |x, y| {
            // 中间一块 20×20 的不透明蓝
            if (20..40).contains(&x) && (20..40).contains(&y) {
                opaque(0, 0, 20000)
            } else {
                [0, 0, 0, 0]
            }
        });
        let args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(2, 2)]);
        let mut dst = PigmentSurface::new();
        flood_fill(&src, &args, &mut dst);

        assert_eq!(dst_px(&dst, 2, 2)[3], OPAQUE, "透明区应被填满");
        assert_eq!(dst_px(&dst, 63, 63)[3], OPAQUE, "透明区延伸到角上");
        assert_eq!(dst_px(&dst, 25, 25)[3], 0, "不透明方块不该被填");
    }

    /// 种子像素本身在容差之外（取样点和种子不是同一个像素）→ 什么也不填。
    #[test]
    fn seed_outside_tolerance_fills_nothing() {
        let src = TestSource::new()
            .with_tile(0, 0, |_, _| opaque(0, 0, 0))
            .with_tile(1, 0, |_, _| opaque(3000, 0, 0));
        let mut args = args_over(
            (0, 0, 2 * TILE_SIZE as i32, TILE_SIZE as i32),
            vec![(64 + 5, 5)],
        );
        // 取样点指向左块（纯黑），种子在右块（红 3000，远超 1.5 倍容差）
        args.target_pos = (5, 5);
        let mut dst = PigmentSurface::new();
        assert_eq!(flood_fill(&src, &args, &mut dst), 0, "不该有任何 tile 被填");
        assert_eq!(dst.tile_count(), 0);
    }

    /// 扫描线填充的健全性：结果必须**正好**是"可填像素的四连通分量"。
    ///
    /// tile 里撒一批孤立的"洞"（不可填像素），其余都可填 —— 这样连通分量一定
    /// 很大且必须绕开洞，然后把 [`Filler::fill`] 的产出与朴素 BFS 逐像素对比。
    /// 这条能抓住 `check_enqueue` 的 `look_above`/`look_below` 连续段复用、
    /// 左右双向扫描、tile 边界溢出这些最容易写错的地方。
    #[test]
    fn scanline_fill_matches_a_naive_connected_component() {
        // 逐像素的哈希（不用顺序生成器：LCG/xorshift 的相邻输出在二维铺开后
        // 会形成条纹，连通分量反而很小）
        let hash = |i: usize| -> u32 {
            let mut v = (i as u32).wrapping_mul(0x9E37_79B9);
            v ^= v >> 15;
            v = v.wrapping_mul(0x85EB_CA6B);
            v ^= v >> 13;
            v
        };
        let mut tile = hokusai::tile::empty_tile();
        for i in 0..PIXELS {
            // 约 1/8 的孤立洞：可填色（灰 1000）与不可填色（红 20000）差得远超容差
            tile[i / TILE_SIZE][i % TILE_SIZE] = if hash(i) % 8 == 0 {
                opaque(20000, 0, 0)
            } else {
                opaque(1000, 1000, 1000)
            };
        }

        // 选一个可填像素当种子（避开洞）
        let (sx, sy) = (20usize, 20usize);
        let seed = (sx as i32, sy as i32);
        assert_ne!(
            tile[sy][sx],
            opaque(20000, 0, 0),
            "种子不该正好落在洞里（换个坐标或调哈希）"
        );

        let mut filler = Filler::new(rgba_of(tile[sy][sx]), 0.05);
        let mut dst = [0u16; PIXELS];
        filler.fill(
            &tile,
            &mut dst,
            Seeds::Initial(&[seed]),
            (0, 0, TILE_SIZE as i32 - 1, TILE_SIZE as i32 - 1),
        );

        // 朴素四连通 BFS
        let mut expected = [false; PIXELS];
        let mut stack = vec![seed];
        while let Some((x, y)) = stack.pop() {
            if !in_tile(x, y) {
                continue;
            }
            let i = idx(x, y);
            if expected[i] {
                continue;
            }
            if filler.pixel_fill_alpha(&rgba_of(tile[y as usize][x as usize])) == 0 {
                continue;
            }
            expected[i] = true;
            stack.push((x + 1, y));
            stack.push((x - 1, y));
            stack.push((x, y + 1));
            stack.push((x, y - 1));
        }

        let mut filled_count = 0usize;
        for i in 0..PIXELS {
            let (x, y) = ((i % TILE_SIZE) as i32, (i / TILE_SIZE) as i32);
            let want = filler.pixel_fill_alpha(&rgba_of(tile[y as usize][x as usize]));
            assert_eq!(
                dst[i] != 0,
                expected[i],
                "像素 ({x},{y}) 的填充状态与连通分量不一致"
            );
            if expected[i] {
                assert_eq!(dst[i], want, "像素 ({x},{y}) 的 alpha 值不对");
                filled_count += 1;
            }
        }
        // 稀疏的孤立洞挡不住四连通传播 → 分量应该很大，但绝不是整块
        assert!(
            filled_count > PIXELS / 2 && filled_count < PIXELS,
            "连通分量大小异常：{filled_count}"
        );
    }

    /// bbox 限制：只有 bbox 覆盖的 10×10 像素被填，其余不动。
    #[test]
    fn bbox_limits_the_filled_area() {
        let src = TestSource::new().with_tile(0, 0, |_, _| opaque(1000, 1000, 1000));
        let args = args_over((0, 0, 10, 10), vec![(5, 5)]);
        let mut dst = PigmentSurface::new();
        flood_fill(&src, &args, &mut dst);

        assert_eq!(dst_px(&dst, 9, 9)[3], OPAQUE, "bbox 内应填满");
        assert_eq!(dst_px(&dst, 10, 5)[3], 0, "bbox 外（同一 tile）不该填");
        assert_eq!(dst_px(&dst, 5, 10)[3], 0);
    }

    /// `no_op`：锁 alpha + 会降背景 alpha 的模式 → 整个填充不做。
    #[test]
    fn no_op_combination_fills_nothing() {
        let src = TestSource::new().with_tile(0, 0, |_, _| opaque(1000, 1000, 1000));
        let mut args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(5, 5)]);
        args.blend_mode = FillBlendMode::LockAlpha;
        args.comp_mode = CombineMode::DestinationOut;
        assert!(args.no_op(), "锁 alpha + DestinationOut 应是空操作");
        let mut dst = PigmentSurface::new();
        assert_eq!(flood_fill(&src, &args, &mut dst), 0);
        assert_eq!(dst.tile_count(), 0);

        // 普通模式（Normal + Normal 合成）不受影响
        args.blend_mode = FillBlendMode::Normal;
        args.comp_mode = CombineMode::Normal;
        assert!(!args.no_op());
        assert!(flood_fill(&src, &args, &mut dst) > 0);
    }

    // ---- FillMode 端到端 ----

    /// 造一个带一块涂色区域的文档 + 模式上下文。
    fn doc_with_paint() -> Document {
        let mut doc = Document::new();
        {
            let surface = doc.surface_mut();
            let tile = surface.tile_request_start(0, 0);
            for row in tile.iter_mut() {
                for px in row.iter_mut() {
                    // 整块不透明黑（fix15 满 alpha 的"纸"）
                    *px = opaque(0, 0, 0);
                }
            }
            surface.tile_request_end(0, 0);
        }
        doc
    }

    /// 缺口检测开关切换光标动作 id（`fill.py:93-97`）。
    #[test]
    fn cursor_action_follows_gap_closing() {
        let mut mode = FillMode::new();
        assert_eq!(mode.cursor_action_name(), ACTION_NAME);
        assert_eq!(
            InteractionMode::cursor_action_name(&mode),
            Some(ACTION_NAME)
        );
        mode.options.gap_closing = true;
        assert_eq!(mode.cursor_action_name(), GC_ACTION_NAME);
        assert_eq!(GC_ACTION_NAME, "FloodFillGCMode");
        assert_eq!(
            InteractionMode::cursor_action_name(&mode),
            Some("FloodFillGCMode")
        );
    }

    /// 单击（按下 → 松开）在空白层上填出一块，且**一条**命令入栈、可撤销。
    #[test]
    fn click_fills_and_undo_restores_the_layer() {
        let mut doc = Document::new();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let before = doc.command_stack.undo_len();
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            // 取色通道（B3-4）：填充模式不读不写，给中性值/空出口
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FillMode::new();

        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        assert!(InteractionMode::button_press(&mut mode, &mut ctx, &press));
        let release = PointerEvent::button(10.0, 10.0, 16.0, 1);
        assert!(InteractionMode::button_release(&mut mode, &mut ctx, &release));

        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before + 1,
            "一次填充只该入栈一条 FloodFill 命令"
        );
        assert_eq!(
            ctx.doc.get_last_command().map(|c| c.display_name()),
            Some("Flood Fill".to_string()),
            "展示名照源码 `display_name = _(\"Flood Fill\")`"
        );
        let surface = ctx.doc.layers().current().surface().expect("绘画层");
        assert_eq!(surface.tile_count(), 1, "空白层上应填出一块 tile");
        // 填充色来自画笔颜色（默认 h/s/v → 某个不透明色），alpha 必须满
        let px = dst_px(surface, 10, 10);
        assert_eq!(px[3], OPAQUE, "填充结果应是不透明的");

        // 撤销 → 回到空白
        ctx.doc.undo();
        let surface = ctx.doc.layers().current().surface().expect("绘画层");
        assert_eq!(surface.tile_count(), 0, "撤销应清掉这次填充");
        // 重做 → 又出现
        ctx.doc.redo();
        let surface = ctx.doc.layers().current().surface().expect("绘画层");
        assert_eq!(surface.tile_count(), 1, "重做应重新填上");
    }

    /// 组上单击：`get_fillable()` False → 自动新建一层再填
    /// （`fill.py:213-216`、`command.py:601-616`），撤销会把新层删掉。
    #[test]
    fn fill_on_a_group_creates_a_new_layer_and_undo_removes_it() {
        use pigment_surface::layer::Layer;

        let mut doc = Document::new();
        let mut group = Layer::new_group();
        group.name = Some("G".into());
        doc.layers_mut().insert(0, group);
        doc.select_layer(&[0]);
        assert!(doc.layers().paintable_at(&[0]).is_none());
        let group_path = doc.current_path().to_vec();
        // `select_layer` 自己会入一条自动撤销命令，先记基线
        let before = doc.command_stack.undo_len();
        let layers_before = doc.layers().len();

        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            // 取色通道（B3-4）：填充模式不读不写，给中性值/空出口
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FillMode::new();
        let press = PointerEvent::button(10.0, 10.0, 0.0, 1);
        InteractionMode::button_press(&mut mode, &mut ctx, &press);
        assert!(!mode.seeds().is_empty(), "组可见且未锁，应收集种子");
        let release = PointerEvent::button(10.0, 10.0, 16.0, 1);
        InteractionMode::button_release(&mut mode, &mut ctx, &release);

        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before + 1,
            "组上填充应产生一条命令"
        );
        assert_eq!(
            ctx.doc.layers().len(),
            layers_before + 1,
            "应自动新建一层"
        );
        // 当前层切到了新的绘画层，且填上了内容
        let surface = ctx.doc.layers().current().surface().expect("新绘画层");
        assert!(surface.tile_count() > 0, "新层应有填充结果");
        // 撤回：层数恢复、当前层回到组
        ctx.doc.undo();
        assert_eq!(ctx.doc.layers().len(), layers_before, "撤销应删掉新层");
        assert_eq!(ctx.doc.current_path(), group_path.as_slice(), "当前层应恢复");
        assert!(ctx.doc.layers().current().is_group());
    }

    /// 拖动收集多个种子；松开时按种子集合一次填充。
    #[test]
    fn dragging_collects_seeds_and_fills_once() {
        let mut doc = doc_with_paint();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        let before = doc.command_stack.undo_len();
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            // 取色通道（B3-4）：填充模式不读不写，给中性值/空出口
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FillMode::new();
        InteractionMode::button_press(&mut mode, &mut ctx, &PointerEvent::button(2.0, 2.0, 0.0, 1));
        for i in 1..4 {
            let ev = PointerEvent::motion(2.0 + i as f32 * 8.0, 2.0, i as f64 * 8.0);
            assert!(InteractionMode::motion(&mut mode, &mut ctx, &ev));
        }
        assert_eq!(mode.seeds().len(), 4, "四个不同像素应产生四个种子");
        InteractionMode::button_release(
            &mut mode,
            &mut ctx,
            &PointerEvent::button(26.0, 2.0, 32.0, 1),
        );
        assert_eq!(
            ctx.doc.command_stack.undo_len(),
            before + 1,
            "整段拖动只该产生一条命令"
        );
        assert!(mode.seeds().is_empty(), "填充后种子应清空");
    }

    /// `sample merged` 的源是整份图层栈的合成（`command.py:594-595`），
    /// 不是目标层自身：目标层为空时也能按**别的层**的颜色决定填充范围。
    #[test]
    fn sample_merged_reads_other_layers() {
        let mut doc = Document::new();
        doc.layers_mut().set_background_visible(false);
        // 当前层：左半边不透明红，右半边透明
        {
            let surface = doc.surface_mut();
            let tile = surface.tile_request_start(0, 0);
            for row in tile.iter_mut() {
                for (x, px) in row.iter_mut().enumerate() {
                    *px = if x < 32 { opaque(20000, 0, 0) } else { [0; 4] };
                }
            }
            surface.tile_request_end(0, 0);
        }
        let mut dst = PigmentSurface::new();
        let args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(2, 2)]);
        let src = MergedSource::new(doc.layers());
        flood_fill(&src, &args, &mut dst);
        assert_eq!(dst_px(&dst, 5, 5)[3], OPAQUE, "不透明红区应被填");
        assert_eq!(dst_px(&dst, 40, 5)[3], 0, "透明区不该被填");
        assert_eq!(dst_px(&dst, 5, 5)[0], OPAQUE);
    }

    /// 单层源渲染（`LayerSource`）：只有该层的像素参与，背景不参与。
    #[test]
    fn layer_source_renders_only_the_requested_layer() {
        let mut doc = Document::new();
        doc.layers_mut().set_background_visible(false);
        {
            let surface = doc.surface_mut();
            let tile = surface.tile_request_start(0, 0);
            for row in tile.iter_mut() {
                for px in row.iter_mut() {
                    *px = opaque(0, 0, 12000);
                }
            }
            surface.tile_request_end(0, 0);
        }
        let src = LayerSource::new(doc.layers(), doc.current_path());
        let px = src
            .tile(0, 0)
            .map(|t| t[3][3])
            .expect("该层应有 tile");
        assert_eq!(px, opaque(0, 0, 12000));
        assert!(src.tile(1, 0).is_none(), "没有内容的 tile 应返回 None");
    }

    /// `offset > 0` 膨胀：不启用画框时结果会扩到 bbox 之外的邻 tile；
    /// 启用画框时按 bbox 裁剪（`floodfill.py:344-348`、`:402-405`）。
    #[test]
    fn offset_grows_the_fill_and_frame_trims_it() {
        let src = TestSource::new().with_tile(0, 0, |_, _| opaque(1000, 1000, 1000));
        let mut args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(5, 5)]);
        args.offset = 2;
        let mut free = PigmentSurface::new();
        flood_fill(&src, &args, &mut free);
        assert!(
            dst_px(&free, -1, 10)[3] > 0,
            "不裁剪时膨胀应越过 bbox 进入左邻块"
        );
        assert!(free.tile_count() > 1);

        args.framed = true;
        let mut framed = PigmentSurface::new();
        flood_fill(&src, &args, &mut framed);
        assert_eq!(framed.tile_count(), 1, "画框启用时邻块应被裁掉");
        assert_eq!(dst_px(&framed, 63, 10)[3], OPAQUE, "bbox 内仍是满 alpha");
        assert_eq!(dst_px(&framed, 64, 10)[3], 0, "bbox 外不该写");
    }

    /// `offset < 0` 腐蚀：满 tile 的填充边缘被吃掉，结果 alpha 变小。
    #[test]
    fn negative_offset_erodes_the_fill() {
        let src = TestSource::new().with_tile(0, 0, |_, _| opaque(1000, 1000, 1000));
        let mut args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(5, 5)]);
        args.offset = -3;
        let mut dst = PigmentSurface::new();
        flood_fill(&src, &args, &mut dst);
        assert_eq!(dst_px(&dst, 32, 32)[3], OPAQUE, "内部仍满");
        assert_eq!(dst_px(&dst, 0, 32)[3], 0, "边缘被腐蚀掉");
        assert_eq!(dst_px(&dst, 63, 32)[3], 0);
    }

    /// `feather > 0` 高斯模糊：硬边出现半透明过渡带。
    #[test]
    fn feather_softens_the_fill_edge() {
        let src = TestSource::new().with_tile(0, 0, |_, _| opaque(1000, 1000, 1000));
        let mut args = args_over((0, 0, TILE_SIZE as i32, TILE_SIZE as i32), vec![(5, 5)]);
        args.feather = 4;
        let mut dst = PigmentSurface::new();
        flood_fill(&src, &args, &mut dst);
        let edge = dst_px(&dst, 63, 32)[3];
        assert!(edge > 0 && edge < OPAQUE, "边缘应被模糊（实得 {edge}）");
        assert!(dst_px(&dst, 32, 32)[3] > (OPAQUE as u32 * 9 / 10) as u16);
    }

    /// "New Layer (once)"：填充写到新层，原层不动；撤销删层并回到原层。
    #[test]
    fn make_new_layer_option_writes_to_a_fresh_layer() {
        let mut doc = doc_with_paint();
        // 换个非黑填充色，才能区分"新层内容"和原层的黑
        let mut brush = crate::brushlib::default_brush();
        brush.get_mut(BrushSetting::ColorS).base_value = 0.0;
        brush.get_mut(BrushSetting::ColorV).base_value = 1.0;
        let mut state = crate::hokusai::BrushState::default();
        let before_layers = doc.layers().len();
        let before_undo = doc.command_stack.undo_len();
        {
            let mut ctx = ModeContext {
                doc: &mut doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: [0.0, 0.0, 0.0],
                picked_color: None,
            };
            let mut mode = FillMode::new();
            mode.options.make_new_layer = true;
            InteractionMode::button_press(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(10.0, 10.0, 0.0, 1),
            );
            InteractionMode::button_release(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(10.0, 10.0, 16.0, 1),
            );
            assert!(!mode.options.make_new_layer, "一次性开关应自动复位");
            assert_eq!(ctx.doc.layers().len(), before_layers + 1);
            assert_eq!(ctx.doc.command_stack.undo_len(), before_undo + 1);
            // 原层仍是整块黑，新层是填充色
            let old = ctx.doc.layers().layer_at(&[1]).unwrap().surface().unwrap();
            assert_eq!(dst_px(old, 10, 10), opaque(0, 0, 0));
            let new = ctx.doc.layers().current().surface().unwrap();
            assert_eq!(dst_px(new, 10, 10)[3], OPAQUE);
            assert_ne!(dst_px(new, 10, 10), opaque(0, 0, 0));
            ctx.doc.undo();
            assert_eq!(ctx.doc.layers().len(), before_layers);
        }
    }

    /// "Source" 下拉：显式指定取样源图层（`fill.py:965-970` 的 `src_path`）。
    ///
    /// 目标层是空的，源层只有左半边不透明；填充范围必须按**源层**算，
    /// 结果写进目标层。
    #[test]
    fn explicit_source_layer_controls_the_filled_region() {
        use pigment_surface::layer::Layer;

        let mut doc = Document::new();
        doc.layers_mut().set_background_visible(false);
        // 当前层（源）：左半边不透明黑，右半边透明
        {
            let surface = doc.surface_mut();
            let tile = surface.tile_request_start(0, 0);
            for row in tile.iter_mut() {
                for (x, px) in row.iter_mut().enumerate() {
                    *px = if x < 32 { opaque(0, 0, 0) } else { [0; 4] };
                }
            }
            surface.tile_request_end(0, 0);
        }
        let src_id = doc.layers().current_id();
        // 目标层：上方空白层
        let top = Layer::new();
        let top_id = top.id;
        doc.layers_mut().insert(0, top);
        assert!(doc.layers_mut().set_current_by_id(top_id));

        let mut brush = crate::brushlib::default_brush();
        brush.get_mut(BrushSetting::ColorS).base_value = 0.0;
        brush.get_mut(BrushSetting::ColorV).base_value = 1.0;
        let mut state = crate::hokusai::BrushState::default();
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FillMode::new();
        mode.options.src_layer = Some(src_id);
        InteractionMode::button_press(&mut mode, &mut ctx, &PointerEvent::button(2.0, 2.0, 0.0, 1));
        InteractionMode::button_release(
            &mut mode,
            &mut ctx,
            &PointerEvent::button(2.0, 2.0, 16.0, 1),
        );

        let target = ctx.doc.layers().layer_at(&[0]).unwrap().surface().unwrap();
        assert_eq!(
            dst_px(target, 10, 10)[3],
            OPAQUE,
            "源层不透明区应决定填充范围"
        );
        assert_eq!(
            dst_px(target, 40, 10)[3],
            0,
            "源层透明区不该被填（目标层自身是空的）"
        );
        // 源层原样不动
        let src = ctx.doc.layers().layer_at(&[1]).unwrap().surface().unwrap();
        assert_eq!(dst_px(src, 10, 10), opaque(0, 0, 0));
        assert_eq!(dst_px(src, 40, 10), [0; 4]);
        assert_eq!(ctx.doc.current_path(), &[0]);
    }

    /// "New Layer (once)" 时取样源仍是**填充前的当前层**：新层插在原层
    /// 位置会把原层路径往后挤，命令必须按 id 重新解析路径
    /// （`command.py:596-599` 持图层对象引用）。
    #[test]
    fn new_layer_fill_keeps_the_original_layer_as_source() {
        let mut doc = Document::new();
        doc.layers_mut().set_background_visible(false);
        {
            let surface = doc.surface_mut();
            let tile = surface.tile_request_start(0, 0);
            for row in tile.iter_mut() {
                for (x, px) in row.iter_mut().enumerate() {
                    *px = if x < 32 { opaque(0, 0, 0) } else { [0; 4] };
                }
            }
            surface.tile_request_end(0, 0);
        }
        let before_layers = doc.layers().len();
        let mut brush = crate::brushlib::default_brush();
        brush.get_mut(BrushSetting::ColorS).base_value = 0.0;
        brush.get_mut(BrushSetting::ColorV).base_value = 1.0;
        let mut state = crate::hokusai::BrushState::default();
        let mut ctx = ModeContext {
            doc: &mut doc,
            brush: &brush,
            brush_name: "test",
            brush_state: &mut state,
            fakepressure: 0.5,
            fakerotation: 0.5,
            hide_cursor_while_painting: false,
            status: None,
            color: [0.0, 0.0, 0.0],
            picked_color: None,
        };
        let mut mode = FillMode::new();
        mode.options.make_new_layer = true;
        InteractionMode::button_press(&mut mode, &mut ctx, &PointerEvent::button(2.0, 2.0, 0.0, 1));
        InteractionMode::button_release(
            &mut mode,
            &mut ctx,
            &PointerEvent::button(2.0, 2.0, 16.0, 1),
        );

        assert_eq!(ctx.doc.layers().len(), before_layers + 1);
        let new = ctx.doc.layers().current().surface().expect("新绘画层");
        assert_eq!(
            dst_px(new, 10, 10)[3],
            OPAQUE,
            "新层应填满源层的不透明左半边"
        );
        assert_eq!(
            dst_px(new, 40, 10)[3],
            0,
            "源层右半边透明，新层也不该填（旧实现会错取空的新层当源）"
        );
    }

    /// 锁定层：`_fill_permitted` = `visible and not locked` → 默认收不到种子；
    /// 勾 "New Layer" 后目标层为 None，permitted=True，才会新建一层
    /// （`fill.py:275-281`）。
    #[test]
    fn locked_layer_needs_the_new_layer_option() {
        let mut doc = doc_with_paint();
        doc.layers_mut().current_mut().locked = true;
        let before_layers = doc.layers().len();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        {
            let mut ctx = ModeContext {
                doc: &mut doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: [0.0, 0.0, 0.0],
                picked_color: None,
            };
            let mut mode = FillMode::new();
            InteractionMode::button_press(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(10.0, 10.0, 0.0, 1),
            );
            assert!(mode.seeds().is_empty(), "锁定层默认不该收集种子");
            InteractionMode::button_release(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(10.0, 10.0, 16.0, 1),
            );
            assert_eq!(ctx.doc.layers().len(), before_layers, "不该新建层");

            mode.options.make_new_layer = true;
            InteractionMode::button_press(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(10.0, 10.0, 32.0, 1),
            );
            assert!(!mode.seeds().is_empty(), "勾了 New Layer 后允许收集种子");
            InteractionMode::button_release(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(10.0, 10.0, 48.0, 1),
            );
            assert_eq!(
                ctx.doc.layers().len(),
                before_layers + 1,
                "应新建一层"
            );
        }
    }

    /// 橡皮填充（`BlendModes.ERASE` → DestinationOut）：把已有内容的 alpha 擦掉。
    #[test]
    fn eraser_fill_clears_existing_pixels() {
        let mut doc = doc_with_paint();
        let brush = crate::brushlib::default_brush();
        let mut state = crate::hokusai::BrushState::default();
        {
            let mut ctx = ModeContext {
                doc: &mut doc,
                brush: &brush,
                brush_name: "test",
                brush_state: &mut state,
                fakepressure: 0.5,
                fakerotation: 0.5,
                hide_cursor_while_painting: false,
                status: None,
                color: [0.0, 0.0, 0.0],
                picked_color: None,
            };
            let mut mode = FillMode::new();
            mode.options.blend_mode = FillBlendMode::Erase;
            InteractionMode::button_press(&mut mode, &mut ctx, &PointerEvent::button(8.0, 8.0, 0.0, 1));
            InteractionMode::button_release(
                &mut mode,
                &mut ctx,
                &PointerEvent::button(8.0, 8.0, 16.0, 1),
            );
            let surface = ctx.doc.layers().current().surface().expect("绘画层");
            assert_eq!(
                dst_px(surface, 8, 8)[3],
                0,
                "橡皮填充应把这块区域的 alpha 清成 0"
            );
        }
        // 撤销能恢复内容
        doc.undo();
        let surface = doc.layers().current().surface().expect("绘画层");
        assert_eq!(dst_px(surface, 8, 8)[3], OPAQUE, "撤销应恢复原来的不透明像素");
    }
}
