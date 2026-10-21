//! 缺口检测 + 缺口闭合填充 —— `lib/fill/gap_detection.cpp` +
//! `lib/fill/gap_closing_fill.cpp` + `floodfill.py:606-857` 的移植。
//!
//! # 为什么需要它
//!
//! 普通扫描线填充是"颜色相近就传播"，线稿上 1~3 像素的小缺口会让填充
//! 漏到整张画布。源码的做法不是改阈值，而是：
//!
//! 1. **缺口检测**（[`find_gaps`]）：把 9 邻域 alpha tile 拼成一张大图，
//!    从透明像素出发沿四个对角八分圆搜索；若在 `max_gap` 距离内撞到
//!    "两侧都是不可填像素、中间一条透明缝"的结构，就把缝隙到搜索起点的
//!    连线标上**距离平方**（`gap_detection.cpp:44-90`）。
//! 2. **带距离约束的填充**（[`gc_fill`]）：BFS 传播时携带"路径上见过的
//!    最大距离"；距离突然增大超过阈值就停住，并记下**渗漏边缘**
//!    （`gap_closing_fill.cpp:170-287`）。
//! 3. **回吸**（[`gc_unseep`]）：从渗漏边缘往回擦，直到距离增长率也超过
//!    阈值（`gap_closing_fill.cpp:292-374`）。
//!
//! # 本移植的结构
//!
//! | 源码 | 本文件 |
//! |---|---|
//! | `DistanceBucket` + `dist_search` + `find_gaps` | [`find_gaps`] |
//! | `GapClosingFiller::fill` | [`gc_fill`] |
//! | `GapClosingFiller::unseep` | [`gc_unseep`] |
//! | `_GCTileHandler` + `gap_closing_fill` + `unseep` | [`gap_closing_fill`] |
//!
//! 距离 tile 是 `uint16[64*64]`，`INF_DIST = 2*N*N = 8192`
//! （`floodfill.py:33`），所有"没有缺口"的位置都是它。

use std::collections::{HashMap, HashSet, VecDeque};

use hokusai::tile::TILE_SIZE;

use super::fill::{
    AlphaTile, Edge, Filler, SeedLists, TileBoundingBox, TileSource, OPAQUE, PIXELS,
};
use super::fill_morphology::FilledTiles;

const N: i32 = TILE_SIZE as i32;

/// `INF_DIST`（`floodfill.py:33`）：缺口距离的"无穷大"。
pub const INF_DIST: u16 = (2 * N * N) as u16;

/// 缺口闭合选项（`floodfill.py:198` 的 `GapClosingOptions`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapClosingOptions {
    /// 最大缺口长度（`fill.py:985` 的 `max_gap_size`，默认 5）。
    pub max_gap_size: i32,
    /// 是否回吸渗漏（`retract_seeps`，默认 true）。
    pub retract_seeps: bool,
}

impl Default for GapClosingOptions {
    fn default() -> Self {
        Self {
            max_gap_size: 5,
            retract_seeps: true,
        }
    }
}

/// 距离 tile。
type DistTile = Box<[u16; PIXELS]>;

fn dist_at(d: &[u16; PIXELS], x: i32, y: i32) -> u16 {
    d[(y * TILE_SIZE as i32 + x) as usize]
}

/// `simple_seeds`（`gap_closing_fill.cpp:31-75`）的种子表达。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GcSeeds {
    /// 整条边都是 `INF_DIST`：用 `(反向边,)` 表达（源码的 tuple 捷径）。
    Edge(Edge),
    /// 初始种子（只有坐标，距离在第一次使用时从距离 tile 取）。
    Initial(Vec<(i32, i32)>),
    /// 带距离的像素种子。
    Pixels(Vec<(i32, i32, u16)>),
}

impl GcSeeds {
    fn is_empty(&self) -> bool {
        matches!(self, GcSeeds::Pixels(v) if v.is_empty())
    }

    /// `gc_seeds_skippable`（`floodfill.py:852-857`）。
    fn skippable(&self) -> bool {
        match self {
            GcSeeds::Edge(_) | GcSeeds::Initial(_) => true,
            GcSeeds::Pixels(v) => v.iter().any(|s| s.2 == INF_DIST),
        }
    }
}

type Overflows = [GcSeeds; 4];

fn empty_overflows() -> Overflows {
    std::array::from_fn(|_| GcSeeds::Pixels(Vec::new()))
}

/// 反向边（`edges.inverted`，`fill_common.py`）。
fn inverted(e: Edge) -> Edge {
    match e {
        Edge::North => Edge::South,
        Edge::East => Edge::West,
        Edge::South => Edge::North,
        Edge::West => Edge::East,
        Edge::None => Edge::None,
    }
}

/// `_GCTileHandler.OVERFLOWS`（`floodfill.py:693-699`）。
///
/// 每行按"从哪个方向进来"索引；四项是发往 `[north, east, south, west]`
/// 邻块的种子。`None` = 这条边不回弹。
fn skip_overflows(from_dir: Edge) -> Overflows {
    let table: [[Option<Edge>; 4]; 5] = [
        [None, Some(Edge::West), Some(Edge::North), Some(Edge::East)],
        [Some(Edge::South), None, Some(Edge::North), Some(Edge::East)],
        [Some(Edge::South), Some(Edge::West), None, Some(Edge::East)],
        [Some(Edge::South), Some(Edge::West), Some(Edge::North), None],
        [
            Some(Edge::South),
            Some(Edge::West),
            Some(Edge::North),
            Some(Edge::East),
        ],
    ];
    let row = match from_dir {
        Edge::North => 0,
        Edge::East => 1,
        Edge::South => 2,
        Edge::West => 3,
        Edge::None => 4,
    };
    std::array::from_fn(|i| match table[row][i] {
        Some(e) => GcSeeds::Edge(e),
        None => GcSeeds::Pixels(Vec::new()),
    })
}

// ---- lib/fill/gap_detection.cpp ----

/// 四个对角八分圆的坐标旋转（`gap_detection.cpp:92-112`）。
#[derive(Clone, Copy)]
enum Rot {
    TopRight,
    TopCentr,
    BotCentr,
    BotRight,
}

impl Rot {
    fn apply(self, x: i32, y: i32, xo: i32, yo: i32) -> (i32, i32) {
        match self {
            Rot::TopRight => (x + xo, y + yo),
            Rot::TopCentr => (x - yo, y - xo),
            Rot::BotCentr => (x - yo, y + xo),
            Rot::BotRight => (x + xo, y - yo),
        }
    }
}

/// `upd_dist`（`gap_detection.cpp:28-36`）：只允许把距离改小。
fn upd_dist(dists: &mut [u16; PIXELS], p: (i32, i32), new_dst: u16) {
    let (x, y) = p;
    if !(0..N).contains(&x) || !(0..N).contains(&y) {
        return;
    }
    let i = (y * N + x) as usize;
    if dists[i] > new_dst {
        dists[i] = new_dst;
    }
}

/// 单像素 alpha（输入大图，行主序，边长 `width`）。
fn input_at(input: &[u16], width: usize, x: i32, y: i32) -> u16 {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= width {
        return 0;
    }
    input[y as usize * width + x as usize]
}

/// `dist_search`（`gap_detection.cpp:43-90`）：在一个八分圆里找缺口。
fn dist_search(
    x: i32,
    y: i32,
    dist: i32,
    input: &[u16],
    width: usize,
    dists: &mut [u16; PIXELS],
    rot: Rot,
) -> bool {
    let offs = dist + 1;
    let rx = x - offs;
    let ry = y - offs;

    let t1 = rot.apply(x, y, 0, -1);
    let t2 = rot.apply(x, y, 1, -1);
    if input_at(input, width, t1.0, t1.1) == 0 || input_at(input, width, t2.0, t2.1) == 0 {
        return false;
    }

    let mut gap_found = false;
    for yoffs in 2..dist + 2 {
        let y_dst_sqr = (yoffs - 1) * (yoffs - 1);
        for xoffs in 0..=yoffs {
            let offs_dst = y_dst_sqr + xoffs * xoffs;
            if offs_dst >= 1 + dist * dist {
                break;
            }
            let c = rot.apply(x, y, xoffs, -yoffs);
            if input_at(input, width, c.0, c.1) != 0 {
                continue;
            }
            gap_found = true;
            // 双倍宽度的距离分配（`gap_detection.cpp:73-86`）
            let dx = xoffs as f32 / (yoffs - 1) as f32;
            let mut tx = 0.0f32;
            let mut cx = 0i32;
            for cy in 1..yoffs {
                upd_dist(dists, rot.apply(rx, ry, cx, -cy), offs_dst as u16);
                tx += dx;
                if tx.floor() as i32 > cx {
                    cx += 1;
                    upd_dist(dists, rot.apply(rx, ry, cx, -cy), offs_dst as u16);
                }
                upd_dist(dists, rot.apply(rx, ry, cx + 1, -cy), offs_dst as u16);
            }
        }
    }
    gap_found
}

/// 把 9 邻域 alpha tile 拼成 `(N+2r)^2` 的大图（`init_from_nine_grid`，
/// `fill_common.cpp:128-165`）。
fn alpha_input(grid: &[AlphaTile; 9], radius: i32) -> Vec<u16> {
    let width = (N + 2 * radius) as usize;
    let mut input = vec![0u16; width * width];
    for j in 0..width {
        for i in 0..width {
            let x = i as i32 - radius;
            let y = j as i32 - radius;
            let tx = x.div_euclid(N) + 1;
            let ty = y.div_euclid(N) + 1;
            let px = x.rem_euclid(N) as usize;
            let py = y.rem_euclid(N) as usize;
            // `radius > N` 时 9 宫格覆盖不到大图四角（源码此时 B=N-r 为负，
            // 属未定义行为）；这里把格外的像素按不可填 0 处理。
            if !(0..3).contains(&tx) || !(0..3).contains(&ty) {
                continue;
            }
            let tile = &grid[(ty * 3 + tx) as usize];
            input[j * width + i] = match tile {
                AlphaTile::Full => OPAQUE,
                AlphaTile::Uniform(a) => *a,
                AlphaTile::Data(d) => d[py * TILE_SIZE + px],
            };
        }
    }
    input
}

/// `find_gaps`（`gap_detection.cpp:114-155` + `floodfill.py:752-761`）。
///
/// 返回 `(距离 tile, 是否找到缺口)`。距离 tile 初值全 `INF_DIST`。
pub fn find_gaps(grid: &[AlphaTile; 9], max_gap: i32) -> (DistTile, bool) {
    let radius = max_gap + 1;
    let width = (N + 2 * radius) as usize;
    let input = alpha_input(grid, radius);
    let mut dists: DistTile = Box::new([INF_DIST; PIXELS]);
    let mut gaps_found = false;
    for y in 0..(2 * radius + N - 1) {
        for x in 0..(radius + N - 1) {
            if input[y as usize * width + x as usize] != 0 {
                continue;
            }
            if y >= radius {
                gaps_found |= dist_search(x, y, max_gap, &input, width, &mut dists, Rot::TopRight);
                gaps_found |= dist_search(x, y, max_gap, &input, width, &mut dists, Rot::TopCentr);
            }
            if y < N + radius {
                gaps_found |= dist_search(x, y, max_gap, &input, width, &mut dists, Rot::BotCentr);
                gaps_found |= dist_search(x, y, max_gap, &input, width, &mut dists, Rot::BotRight);
            }
        }
    }
    (dists, gaps_found)
}

// ---- lib/fill/gap_closing_fill.cpp ----

/// BFS 队列项（`gc_coord`，`gap_closing_fill.cpp:82-91`）。
#[derive(Clone, Copy)]
struct GcCoord {
    x: i32,
    y: i32,
    distance: u16,
    is_seed: bool,
}

/// `queue_gc_seeds`（`gap_closing_fill.cpp:103-131`）。
///
/// 邻居在 tile 内就入队；到了 tile 边界且当前像素不是"收进来的种子"时，
/// 把距离写进对应边的溢出数组（避免回弹）。
fn queue_gc_seeds(
    queue: &mut VecDeque<GcCoord>,
    c: GcCoord,
    curr_dist: u16,
    north: &mut [u16; TILE_SIZE],
    east: &mut [u16; TILE_SIZE],
    south: &mut [u16; TILE_SIZE],
    west: &mut [u16; TILE_SIZE],
) {
    let (x, y) = (c.x, c.y);
    let not_seed = !c.is_seed;
    if y > 0 {
        queue.push_back(GcCoord {
            x,
            y: y - 1,
            distance: curr_dist,
            is_seed: false,
        });
    } else if not_seed {
        north[x as usize] = curr_dist;
    }
    if y < N - 1 {
        queue.push_back(GcCoord {
            x,
            y: y + 1,
            distance: curr_dist,
            is_seed: false,
        });
    } else if not_seed {
        south[x as usize] = curr_dist;
    }
    if x > 0 {
        queue.push_back(GcCoord {
            x: x - 1,
            y,
            distance: curr_dist,
            is_seed: false,
        });
    } else if not_seed {
        west[y as usize] = curr_dist;
    }
    if x < N - 1 {
        queue.push_back(GcCoord {
            x: x + 1,
            y,
            distance: curr_dist,
            is_seed: false,
        });
    } else if not_seed {
        east[y as usize] = curr_dist;
    }
}

/// `simple_seeds`（`gap_closing_fill.cpp:31-75`）。
fn simple_seeds(vals: &[u16; TILE_SIZE], e: Edge) -> GcSeeds {
    if vals.iter().all(|v| *v == INF_DIST) {
        return GcSeeds::Edge(inverted(e));
    }
    let mut out = Vec::new();
    for (i, &d) in vals.iter().enumerate() {
        if d == 0 {
            continue;
        }
        let i = i as i32;
        let (x, y) = match e {
            Edge::North => (i, N - 1),
            Edge::East => (0, i),
            Edge::South => (i, 0),
            Edge::West => (N - 1, i),
            Edge::None => (i, i),
        };
        out.push((x, y, d));
    }
    GcSeeds::Pixels(out)
}

fn alpha_at(tile: &AlphaTile, x: i32, y: i32) -> u16 {
    if !(0..N).contains(&x) || !(0..N).contains(&y) {
        return 0;
    }
    let i = (y * N + x) as usize;
    match tile {
        AlphaTile::Full => OPAQUE,
        AlphaTile::Uniform(a) => *a,
        AlphaTile::Data(d) => d[i],
    }
}

/// `populate_gc_queue`（`gap_closing_fill.cpp:133-168`）。
fn populate_queue(queue: &mut VecDeque<GcCoord>, seeds: &GcSeeds) {
    match seeds {
        GcSeeds::Edge(origin) => {
            let x_base = if *origin == Edge::East { N - 1 } else { 0 };
            let y_base = if *origin == Edge::South { N - 1 } else { 0 };
            let x_offs = (*origin as i32 + 1) % 2;
            let y_offs = *origin as i32 % 2;
            for i in 0..N {
                queue.push_back(GcCoord {
                    x: x_base + x_offs * i,
                    y: y_base + y_offs * i,
                    distance: INF_DIST,
                    is_seed: true,
                });
            }
        }
        GcSeeds::Initial(list) => {
            for &(x, y) in list {
                queue.push_back(GcCoord {
                    x,
                    y,
                    distance: INF_DIST,
                    is_seed: true,
                });
            }
        }
        GcSeeds::Pixels(list) => {
            for &(x, y, d) in list {
                queue.push_back(GcCoord {
                    x,
                    y,
                    distance: d,
                    is_seed: true,
                });
            }
        }
    }
}

/// `GapClosingFiller::fill`（`gap_closing_fill.cpp:170-287`）。
///
/// 返回 `(四条边的溢出种子, 渗漏边缘, 填充像素数)`。
pub fn gc_fill(
    alpha: &AlphaTile,
    distances: &[u16; PIXELS],
    dst: &mut [u16; PIXELS],
    seeds: &GcSeeds,
    bounds: (i32, i32, i32, i32),
    track_seep: bool,
) -> (Overflows, Vec<(i32, i32, u16)>, usize) {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = bounds;
    if min_x > max_x || min_y > max_y {
        return (empty_overflows(), Vec::new(), 0);
    }
    min_x = min_x.max(0);
    min_y = min_y.max(0);
    max_x = max_x.min(N - 1);
    max_y = max_y.min(N - 1);

    let mut queue: VecDeque<GcCoord> = VecDeque::new();
    populate_queue(&mut queue, seeds);
    let mut fill_edges: Vec<(i32, i32, u16)> = Vec::new();
    let mut north = [0u16; TILE_SIZE];
    let mut east = [0u16; TILE_SIZE];
    let mut south = [0u16; TILE_SIZE];
    let mut west = [0u16; TILE_SIZE];
    let mut pixels_filled = 0usize;

    while let Some(c) = queue.pop_front() {
        let (x, y) = (c.x, c.y);
        if x < min_x || x > max_x || y < min_y || y > max_y {
            continue;
        }
        let i = (y * N + x) as usize;
        let a = alpha_at(alpha, x, y);
        if dst[i] != 0 || a == 0 {
            continue;
        }
        let prev_dist = c.distance;
        let mut curr_dist = dist_at(distances, x, y);
        if prev_dist != curr_dist {
            // 未分配距离的孤立像素（`gap_closing_fill.cpp:220-257`）
            if curr_dist == INF_DIST {
                if track_seep {
                    if x > 0 && x < N - 1 {
                        let left =
                            dst[(y * N + x - 1) as usize] != 0 || alpha_at(alpha, x - 1, y) == 0;
                        let right =
                            dst[(y * N + x + 1) as usize] != 0 || alpha_at(alpha, x + 1, y) == 0;
                        if left && right {
                            dst[i] = a;
                            continue;
                        }
                    }
                    if y > 0 && y < N - 1 {
                        let up =
                            dst[((y - 1) * N + x) as usize] != 0 || alpha_at(alpha, x, y - 1) == 0;
                        let down =
                            dst[((y + 1) * N + x) as usize] != 0 || alpha_at(alpha, x, y + 1) == 0;
                        if up && down {
                            dst[i] = a;
                            continue;
                        }
                    }
                    fill_edges.push((x, y, curr_dist));
                }
                continue;
            } else if prev_dist < curr_dist
                && ((curr_dist as f32).sqrt() - (prev_dist as f32).sqrt()) > 2.0
            {
                if track_seep {
                    fill_edges.push((x, y, curr_dist));
                }
                continue;
            }
            // GC_TRACK_MIN
            if prev_dist < curr_dist {
                curr_dist = prev_dist;
            }
        }
        pixels_filled += 1;
        dst[i] = a;
        queue_gc_seeds(
            &mut queue,
            GcCoord {
                x,
                y,
                distance: curr_dist,
                is_seed: c.is_seed,
            },
            curr_dist,
            &mut north,
            &mut east,
            &mut south,
            &mut west,
        );
    }

    let fill_edges = fill_edges
        .into_iter()
        .filter(|&(x, y, _)| dst[(y * N + x) as usize] == 0)
        .collect();
    (
        [
            simple_seeds(&north, Edge::North),
            simple_seeds(&east, Edge::East),
            simple_seeds(&south, Edge::South),
            simple_seeds(&west, Edge::West),
        ],
        fill_edges,
        pixels_filled,
    )
}

/// `GapClosingFiller::unseep`（`gap_closing_fill.cpp:292-374`）。
pub fn gc_unseep(
    distances: &[u16; PIXELS],
    dst: &mut [u16; PIXELS],
    seeds: &[(i32, i32, u16)],
    initial: bool,
) -> (Overflows, usize) {
    let mut queue: VecDeque<GcCoord> = VecDeque::new();
    for &(x, y, d) in seeds {
        let filled = dst[(y * N + x) as usize] != 0;
        // `initial ^ (dst != 0)`：初始种子只擦"已经填上的"，
        // 后续种子只擦"还没填的"。
        if initial != filled {
            dst[(y * N + x) as usize] = OPAQUE;
            queue.push_back(GcCoord {
                x,
                y,
                distance: d,
                is_seed: true,
            });
        }
    }
    let mut north = [0u16; TILE_SIZE];
    let mut east = [0u16; TILE_SIZE];
    let mut south = [0u16; TILE_SIZE];
    let mut west = [0u16; TILE_SIZE];
    let mut pixels_erased = 0usize;

    while let Some(c) = queue.pop_front() {
        let (x, y) = (c.x, c.y);
        let i = (y * N + x) as usize;
        if dst[i] == 0 {
            continue;
        }
        let prev_dist = c.distance;
        let mut curr_dist = dist_at(distances, x, y);
        pixels_erased += 1;
        dst[i] = 0;
        if curr_dist == INF_DIST && !c.is_seed {
            continue;
        }
        if prev_dist != curr_dist {
            if curr_dist == INF_DIST
                || (prev_dist < curr_dist
                    && ((curr_dist as f32).sqrt() - (prev_dist as f32).sqrt()) > 1.0)
            {
                continue;
            }
            if prev_dist < curr_dist {
                curr_dist = prev_dist;
            }
        }
        queue_gc_seeds(
            &mut queue,
            GcCoord {
                x,
                y,
                distance: curr_dist,
                is_seed: c.is_seed,
            },
            curr_dist,
            &mut north,
            &mut east,
            &mut south,
            &mut west,
        );
    }

    (
        [
            simple_seeds(&north, Edge::North),
            simple_seeds(&east, Edge::East),
            simple_seeds(&south, Edge::South),
            simple_seeds(&west, Edge::West),
        ],
        pixels_erased,
    )
}

// ---- floodfill.py:606 gap_closing_fill ----

/// 队列项：`(tile 坐标, 种子, 从哪个方向进来)`。
struct SeedEntry {
    coord: (i32, i32),
    seeds: GcSeeds,
    from_dir: Edge,
}

/// `enqueue_overflows`（`floodfill.py:57-77`）的 gap-closing 版。
/// 9 邻域坐标（`fill_common.py:74` 的 `nine_grid`，行主序）。
fn nine_grid(coord: (i32, i32)) -> [(i32, i32); 9] {
    let (tx, ty) = coord;
    [
        (tx - 1, ty - 1),
        (tx, ty - 1),
        (tx + 1, ty - 1),
        (tx - 1, ty),
        (tx, ty),
        (tx + 1, ty),
        (tx - 1, ty + 1),
        (tx, ty + 1),
        (tx + 1, ty + 1),
    ]
}

/// 北、东、南、西（`fill_common.py:79-89`）。
fn orthogonal(coord: (i32, i32)) -> [(i32, i32); 4] {
    let (tx, ty) = coord;
    [(tx, ty - 1), (tx + 1, ty), (tx, ty + 1), (tx - 1, ty)]
}

/// 从邻边进来的方向（`floodfill.py:850` 的 `inv_edges`）。
const INV_EDGES: [Edge; 4] = [Edge::South, Edge::West, Edge::North, Edge::East];

fn enqueue_overflows(
    queue: &mut VecDeque<SeedEntry>,
    coord: (i32, i32),
    seeds: Overflows,
    bbox: &TileBoundingBox,
) {
    for (i, edge_coord) in orthogonal(coord).into_iter().enumerate() {
        if !seeds[i].is_empty() && !bbox.outside(edge_coord) {
            queue.push_back(SeedEntry {
                coord: edge_coord,
                seeds: seeds[i].clone(),
                from_dir: INV_EDGES[i],
            });
        }
    }
}

/// 回吸阶段的溢出（`floodfill.py:820`）：`is_initial=False`。
fn enqueue_unseep_overflows(
    queue: &mut VecDeque<((i32, i32), Vec<(i32, i32, u16)>, bool)>,
    coord: (i32, i32),
    seeds: Overflows,
    bbox: &TileBoundingBox,
) {
    for (i, edge_coord) in orthogonal(coord).into_iter().enumerate() {
        if seeds[i].is_empty() || bbox.outside(edge_coord) {
            continue;
        }
        let list = match &seeds[i] {
            GcSeeds::Pixels(v) => v.clone(),
            GcSeeds::Edge(origin) => {
                let x_base = if *origin == Edge::East { N - 1 } else { 0 };
                let y_base = if *origin == Edge::South { N - 1 } else { 0 };
                let x_offs = (*origin as i32 + 1) % 2;
                let y_offs = *origin as i32 % 2;
                (0..N)
                    .map(|k| (x_base + x_offs * k, y_base + y_offs * k, INF_DIST))
                    .collect()
            }
            GcSeeds::Initial(v) => v.iter().map(|&(x, y)| (x, y, INF_DIST)).collect(),
        };
        queue.push_back((edge_coord, list, false));
    }
}

/// 9 邻域 alpha tile + 是否全为 `Full`（`_GCTileHandler.alpha_grid`，
/// `floodfill.py:763-794`）。
fn alpha_grid(
    src: &impl TileSource,
    coord: (i32, i32),
    filler: &Filler,
    cache: &mut HashMap<(i32, i32), AlphaTile>,
) -> ([AlphaTile; 9], bool) {
    let mut grid: [AlphaTile; 9] = std::array::from_fn(|_| AlphaTile::Uniform(0));
    let mut all_full = true;
    for (i, ntc) in nine_grid(coord).into_iter().enumerate() {
        if !cache.contains_key(&ntc) {
            let src_tile = src.tile(ntc.0, ntc.1);
            let empty = hokusai::tile::empty_tile();
            let (is_empty, tile_ref) = match &src_tile {
                Some(t) => (false, &**t),
                None => (true, &*empty),
            };
            let tile = match filler.tile_uniformity(is_empty, tile_ref) {
                Some(OPAQUE) => AlphaTile::Full,
                Some(0) => AlphaTile::Uniform(0),
                Some(a) => AlphaTile::Uniform(a),
                None => {
                    let mut data = Box::new([0u16; PIXELS]);
                    filler.flood(tile_ref, data.as_mut());
                    AlphaTile::Data(data)
                }
            };
            cache.insert(ntc, tile);
        }
        let t = cache[&ntc].clone();
        all_full &= matches!(t, AlphaTile::Full);
        grid[i] = t;
    }
    (grid, all_full)
}

/// `_GCTileHandler.get_gc_data`（`floodfill.py:711-750`）。
#[allow(clippy::too_many_arguments)]
fn gc_tile_data(
    src: &impl TileSource,
    coord: (i32, i32),
    seeds: &GcSeeds,
    from_dir: Edge,
    filler: &Filler,
    bbox: &TileBoundingBox,
    max_gap: i32,
    alpha_cache: &mut HashMap<(i32, i32), AlphaTile>,
    distances: &mut HashMap<(i32, i32), DistTile>,
    final_set: &mut HashSet<(i32, i32)>,
) -> (AlphaTile, DistTile, Option<Overflows>) {
    if !distances.contains_key(&coord) {
        let (grid, all_full) = alpha_grid(src, coord, filler, alpha_cache);
        let (dist, gaps) = if all_full {
            (Box::new([INF_DIST; PIXELS]), false)
        } else {
            find_gaps(&grid, max_gap)
        };
        if all_full || !gaps {
            distances.insert(coord, Box::new([INF_DIST; PIXELS]));
            // `floodfill.py:733-744`：整块可跳过的三种条件
            let can_skip = (all_full || matches!(&grid[0], AlphaTile::Full))
                && !bbox.crossing(coord)
                && seeds.skippable();
            if can_skip {
                final_set.insert(coord);
                let from = if matches!(seeds, GcSeeds::Initial(_)) {
                    Edge::None
                } else {
                    from_dir
                };
                return (
                    AlphaTile::Full,
                    Box::new([INF_DIST; PIXELS]),
                    Some(skip_overflows(from)),
                );
            }
        } else {
            distances.insert(coord, dist);
        }
    }
    let alpha = alpha_cache[&coord].clone();
    let dist = distances[&coord].clone();
    (alpha, dist, None)
}

/// `complement_gc_seeds`（`floodfill.py:829-849`）。
fn complement_seeds(seeds: &GcSeeds, dist: &[u16; PIXELS]) -> (GcSeeds, bool) {
    match seeds {
        GcSeeds::Initial(list) => {
            let mut any_not_max = false;
            let out = list
                .iter()
                .map(|&(x, y)| {
                    let d = dist_at(dist, x, y);
                    if d < INF_DIST {
                        any_not_max = true;
                    }
                    (x, y, d)
                })
                .collect();
            (GcSeeds::Pixels(out), any_not_max)
        }
        other => (other.clone(), false),
    }
}

/// 缺口闭合填充的主循环（`floodfill.py:606-683`）。
pub fn gap_closing_fill(
    src: &impl TileSource,
    filler: &Filler,
    seed_lists: &SeedLists,
    bbox: &TileBoundingBox,
    options: GapClosingOptions,
) -> FilledTiles {
    // 源码夹到 1..TILE_SIZE，但 64 会让 `radius=65 > N`、9 宫格拼图越界
    // （C++ 的 `B = N - r` 变负，属未定义行为）；这里用 63 作实际上限。
    let max_gap = options.max_gap_size.clamp(1, N - 1);
    let mut filled: FilledTiles = HashMap::new();
    let mut final_set: HashSet<(i32, i32)> = HashSet::new();
    let mut alpha_cache: HashMap<(i32, i32), AlphaTile> = HashMap::new();
    let mut distances: HashMap<(i32, i32), DistTile> = HashMap::new();
    let mut seed_queue: VecDeque<SeedEntry> = seed_lists
        .iter()
        .map(|(coord, seeds)| SeedEntry {
            coord: *coord,
            seeds: GcSeeds::Initial(seeds.clone()),
            from_dir: Edge::None,
        })
        .collect();
    let mut unseep_queue: VecDeque<((i32, i32), Vec<(i32, i32, u16)>, bool)> = VecDeque::new();
    let mut total_px: i64 = 0;
    let mut skip_unseeping = false;

    while let Some(item) = seed_queue.pop_front() {
        if final_set.contains(&item.coord) {
            continue;
        }
        let (alpha_t, dist_t, skip) = gc_tile_data(
            src,
            item.coord,
            &item.seeds,
            item.from_dir,
            filler,
            bbox,
            max_gap,
            &mut alpha_cache,
            &mut distances,
            &mut final_set,
        );
        let overflows = if let Some(ov) = skip {
            filled.insert(item.coord, AlphaTile::Full);
            ov
        } else {
            let (seeds, any_not_max) = complement_seeds(&item.seeds, &dist_t);
            if any_not_max {
                skip_unseeping = true;
            }
            let bounds = bbox.tile_bounds(item.coord);
            let entry = filled
                .entry(item.coord)
                .or_insert_with(|| AlphaTile::Data(Box::new([0u16; PIXELS])));
            let dst = entry.as_data_mut();
            let (ov, fill_edges, px_f) = gc_fill(
                &alpha_t,
                &dist_t,
                dst,
                &seeds,
                bounds,
                options.retract_seeps,
            );
            if px_f == PIXELS {
                final_set.insert(item.coord);
            }
            total_px += px_f as i64;
            if !skip_unseeping && !fill_edges.is_empty() {
                unseep_queue.push_back((item.coord, fill_edges, true));
            }
            ov
        };
        enqueue_overflows(&mut seed_queue, item.coord, overflows, bbox);
    }

    // `floodfill.py:797-826`：回吸
    if !skip_unseeping {
        let mut backup: HashMap<(i32, i32), AlphaTile> = HashMap::new();
        while let Some((coord, seeds, initial)) = unseep_queue.pop_front() {
            if !distances.contains_key(&coord) || !filled.contains_key(&coord) {
                continue;
            }
            if !backup.contains_key(&coord) {
                let tile = filled[&coord].clone();
                backup.insert(coord, tile);
                // Full 要换成可写的满 alpha tile（源码 `new_full_tile(1<<15)`）
                if matches!(filled[&coord], AlphaTile::Full) {
                    filled.insert(coord, AlphaTile::Data(Box::new([OPAQUE; PIXELS])));
                }
            }
            let dist = distances[&coord].clone();
            let entry = filled.get_mut(&coord).expect("刚检查过");
            let dst = entry.as_data_mut();
            let (ov, erased) = gc_unseep(&dist, dst, &seeds, initial);
            total_px -= erased as i64;
            enqueue_unseep_overflows(&mut unseep_queue, coord, ov, bbox);
        }
        if total_px <= 0 {
            for (coord, tile) in backup {
                filled.insert(coord, tile);
            }
        }
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    use hokusai::tile::TILE_SIZE as TS;

    fn alpha_tile(f: impl Fn(usize, usize) -> u16) -> AlphaTile {
        let mut d = Box::new([0u16; PIXELS]);
        for y in 0..TS {
            for x in 0..TS {
                d[y * TS + x] = f(x, y);
            }
        }
        AlphaTile::Data(d)
    }

    fn grid_of(center: AlphaTile) -> [AlphaTile; 9] {
        let mut g: [AlphaTile; 9] = std::array::from_fn(|_| AlphaTile::Uniform(0));
        g[4] = center;
        g
    }

    /// 一条 1px 的水平墙（alpha 0 = 不可填）+ 中间一个 2px 缺口（可填）：
    /// 缺口检测必须返回 true，且缺口附近被写上非 INF 距离。
    #[test]
    fn find_gaps_detects_a_small_hole_in_a_wall() {
        let center = alpha_tile(|x, y| {
            if y == 32 && !(30..32).contains(&x) {
                0
            } else {
                OPAQUE
            }
        });
        let (dist, gaps) = find_gaps(&grid_of(center), 5);
        assert!(gaps, "2px 的缺口应被检测到");
        let any_marked = dist.iter().any(|&d| d != INF_DIST);
        assert!(any_marked, "缺口附近应有被标记的距离");
    }

    /// 没有缺口的实心墙：不应报缺口。墙要铺满同一行的三块 tile，
    /// 否则 tile 边界会被当成"墙的尽头"（真实填充时邻块也有墙）。
    #[test]
    fn find_gaps_reports_nothing_for_a_solid_wall() {
        let wall = || alpha_tile(|_x, y| if y == 32 { 0 } else { OPAQUE });
        let mut grid: [AlphaTile; 9] = std::array::from_fn(|_| AlphaTile::Full);
        grid[3] = wall();
        grid[4] = wall();
        grid[5] = wall();
        let (_dist, gaps) = find_gaps(&grid, 5);
        assert!(!gaps, "实心墙不该被当成缺口");
    }

    /// 端到端：一堵 1px 墙 + 2px 缺口。普通填充会漏到墙下，
    /// 缺口闭合填充应停在墙上方。
    #[test]
    fn gap_closing_keeps_the_fill_above_a_2px_hole() {
        use crate::mode::fill::{flood_fill, FillArgs};
        use hokusai::TiledSurface;
        use pigment_surface::combine::CombineMode;
        use pigment_surface::PigmentSurface;

        let mut src = PigmentSurface::new();
        {
            let tile = src.tile_request_start(0, 0);
            for (y, row) in tile.iter_mut().enumerate() {
                for (x, px) in row.iter_mut().enumerate() {
                    let barrier = y == 32 && !(30..32).contains(&x);
                    *px = if barrier {
                        [0, 0, 0, 0]
                    } else {
                        [1000, 1000, 1000, OPAQUE]
                    };
                }
            }
            src.tile_request_end(0, 0);
        }
        let args = FillArgs {
            target_pos: (5, 5),
            seeds: vec![(5, 5)],
            color: [1.0, 0.0, 0.0],
            tolerance: 0.05,
            offset: 0,
            feather: 0,
            framed: false,
            gap_closing: None,
            comp_mode: CombineMode::Normal,
            blend_mode: crate::mode::fill::FillBlendMode::Normal,
            opacity: 1.0,
            bbox: (0, 0, N, N),
        };
        let mut leak = PigmentSurface::new();
        flood_fill(&src, &args, &mut leak);
        assert!(
            leak.tile(0, 0).unwrap()[50][5][3] > 0,
            "普通填充应穿过缺口漏到墙下"
        );

        let mut closed = PigmentSurface::new();
        let mut gc = args.clone();
        gc.gap_closing = Some(GapClosingOptions {
            max_gap_size: 5,
            retract_seeps: true,
        });
        flood_fill(&src, &gc, &mut closed);
        let below = closed.tile(0, 0).map(|t| t[50][5][3]).unwrap_or(0);
        assert_eq!(below, 0, "缺口闭合后不该漏到墙下");
        assert!(closed.tile(0, 0).unwrap()[10][5][3] > 0, "墙上方仍应被填上");
    }

    /// `dist_search` 的四个八分圆方向都要在缺口两侧对称工作。
    #[test]
    fn gap_distances_are_symmetric_around_the_hole() {
        let center = alpha_tile(|x, y| {
            if y == 32 && !(30..32).contains(&x) {
                0
            } else {
                OPAQUE
            }
        });
        let (dist, _) = find_gaps(&grid_of(center), 5);
        // 墙的上方和下方都应出现被标记的距离（四个八分圆都工作）
        let mut above = false;
        let mut below = false;
        for y in 0..TS as i32 {
            for x in 0..TS as i32 {
                if dist_at(&dist, x, y) < INF_DIST {
                    if y < 32 {
                        above = true;
                    } else if y > 32 {
                        below = true;
                    }
                }
            }
        }
        assert!(above, "墙上方应有缺口距离");
        assert!(below, "墙下方应有缺口距离");
    }

    /// `simple_seeds` 的整边捷径：全 INF 的边返回反向边，否则返回像素列表。
    #[test]
    fn simple_seeds_uses_the_edge_shortcut_only_for_all_inf() {
        let all = [INF_DIST; TILE_SIZE];
        assert_eq!(simple_seeds(&all, Edge::North), GcSeeds::Edge(Edge::South));
        let mut mixed = all;
        mixed[3] = 42;
        match simple_seeds(&mixed, Edge::East) {
            GcSeeds::Pixels(v) => {
                assert_eq!(v.len(), TILE_SIZE, "其余 63 个 INF 也要作为种子");
                assert!(v.contains(&(0, 3, 42)));
            }
            _ => panic!("应是像素列表"),
        }
    }

    /// `gc_fill` 的距离约束：距离从 0 跳到 INF 时不应继续填。
    #[test]
    fn gc_fill_stops_at_a_distance_jump() {
        let alpha = AlphaTile::Full;
        let mut dist: DistTile = Box::new([0u16; PIXELS]);
        // 右半边没有距离数据（INF）
        for y in 0..TS {
            for x in 32..TS {
                dist[y * TS + x] = INF_DIST;
            }
        }
        let mut dst = Box::new([0u16; PIXELS]);
        let seeds = GcSeeds::Pixels(vec![(5, 5, 0)]);
        let (_ov, edges, px) = gc_fill(&alpha, &dist, &mut dst, &seeds, (0, 0, 63, 63), true);
        assert!(px > 0);
        assert_eq!(dst[(5 * TS + 5) as usize], OPAQUE);
        assert_eq!(dst[(5 * TS + 40) as usize], 0, "距离跳变处必须停下");
        assert!(!edges.is_empty(), "停下处应记录渗漏边缘（供回吸）");
    }
}
