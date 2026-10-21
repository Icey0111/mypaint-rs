//! GPU 侧 smudge 采样内核。
//!
//! 背景：`blending_knife` 这类纯涂抹笔的 `smudge_length_log == 0`，于是
//! `recentness < threshold` 恒成立 —— **每个 dab 都要重采样画布**。采样在
//! CPU 侧做，就必须先让 GPU 队列落地，形成「一 dab 一次 submit + fence」
//! （实测 0.16ms/次，其中 0.11ms 是纯驱动往返），而且大笔刷一次 flush 还要
//! 往返几兆字节的 tile。
//!
//! 解法是把采样搬上 GPU：着色器直接读 atlas（里面已经包含排队中的 dab），
//! 算出采样色写进 storage buffer，随后的 dab pass 读它。整串 dab 一次提交，
//! 不再有 CPU 同步。
//!
//! # 为什么不是「一个工作组单线程跑完」
//!
//! 第一版就是那样写的：与 `get_color_pigment_default` 逐行对齐，结果逐位
//! 一致（`maxdiff = 2e-5`），但**慢 50–100 倍**。单线程要顺序扫过整片采样
//! 圆：r=400 是 50 万像素，实测 55ms/次，而 CPU 的并行版只要 0.5ms。
//!
//! 所以这里照抄 `pigment_surface::color_sample::get_color_pigment_parallel`
//! 的组织方式，把它搬到 GPU：
//!
//! 1. **计数**（每 band 一个线程）：统计每个 (tile, 16 行 band) 里被 mask
//!    覆盖的像素数。整块落在圆内的 band 直接按面积算，不必遍历。
//! 2. **前缀**（单线程，几百次循环）：由覆盖数推出每个 band 的 `interval`
//!    计数器初值、RNG 起始状态（用 LCG 跳进）和采样写入偏移。
//!    每个 tile 的计数器从 0 开始、band 之间连续，与参考实现一致。
//! 3. **抽样**（每 band 一个线程）：按行主序遍历、按同样的阈值抽样，把
//!    `(mask, 像素)` 写到本 band 的偏移处。
//! 4. **累加**（单线程，只有几十个采样点）：按全局顺序做 WGM 串行累加。
//!
//! 前三步把 O(像素) 的遍历摊到 `tiles × 4` 个线程上，第四步只处理真正被
//! 抽中的像素（∝ 半径，不是面积），所以整体随半径**线性**而不是平方增长。

use pigment_surface::perf;

/// 每个 tile 切成的 band 行数。
///
/// 采样必须保持 tile 行主序与 RNG 串行顺序，所以 band 是“一个线程扫一段”
/// 的唯一粒度。16 行 ⇒ 每个 tile 只有 4 个线程，大半径采样几乎跑不满 GPU；
/// 1 行 ⇒ 64 个线程，前缀 pass 仍按同一顺序把 RNG/计数器送给每个 band。
pub(crate) const BAND_ROWS: u32 = 1;
const BANDS_PER_TILE: u32 = 64 / BAND_ROWS;

const SAMPLE_WGSL: &str = r#"
struct SP {
    // sample: cx, cy, radius, paint
    c: vec4<f32>,
    // tx0, ty0, cols, rows
    g: vec4<i32>,
    // interval, threshold, rng_start, band_rows
    n: vec4<u32>,
    // samples_per_band, chain_rng, slot_base, 0
    e: vec4<u32>,
    // smudge: amount, length, length_log, radius
    s: vec4<f32>,
    // paint_mode, smudge_op_lim, eraser, mapping_active
    t: vec4<f32>,
    // base_r, base_g, base_b, sample_paint
    u: vec4<f32>,
    // bucket_index, seed_bucket, result_index, 0
    v: vec4<u32>,
    // seed bucket colour
    w: vec4<f32>,
    // seed bucket prev
    x: vec4<f32>,
    // seed recentness, 0, 0, 0
    y: vec4<f32>,
};

@group(0) @binding(0) var<uniform> p: SP;
@group(0) @binding(1) var<storage, read> atlas: array<u32>;
// 采样 bbox 内的稠密槽位表（行主序），0xFFFFFFFF = 该 tile 不存在。
@group(0) @binding(2) var<storage, read> slots: array<u32>;
struct Bucket {
    color: vec4<f32>,
    prev: vec4<f32>,
    recentness: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

struct SmudgeResult {
    // rgb + alpha_eraser
    color: vec4<f32>,
    spec_log: array<vec4<f32>, 3>,
    spec_zero: array<vec4<f32>, 3>,
    // skip, 0, 0, 0
    flags: vec4<u32>,
};

struct SampleState {
    result: vec4<f32>,
    rng: u32,
    sample_count: u32,
    pad0: u32,
    pad1: u32,
};

const MAX_BANDS: u32 = 65536u;
const MAX_TILES: u32 = 2048u;
const OFF_COUNT: u32 = 0u;
const OFF_TILE_DRAWS: u32 = MAX_BANDS;
const OFF_TILE_START: u32 = MAX_BANDS + MAX_TILES;
const OFF_FIRST: u32 = MAX_BANDS + 2u * MAX_TILES;
const OFF_ROW_CUM: u32 = 2u * MAX_BANDS + 2u * MAX_TILES;
const OFF_BITS: u32 = 3u * MAX_BANDS + 2u * MAX_TILES;
const OFF_ROW_OFF: u32 = OFF_BITS + MAX_TILES * 128u;
const OFF_TILE_COUNT: u32 = OFF_ROW_OFF + MAX_BANDS;
const OFF_EXPANDED: u32 = OFF_TILE_COUNT + MAX_TILES;
/// 每个 tile 最多存多少个已展开采样点（半径 >= 3 时足够）。
const MAX_SAMPLES_PER_TILE: u32 = 256u;

// counts | 每 tile 总 draw 数 | 每 tile RNG 起点 |
// 每行首个非零像素 | 每行之前的覆盖像素数
@group(0) @binding(3) var<storage, read_write> bands: array<u32>;
// 最终采样色 + 链式 RNG 状态。
@group(0) @binding(4) var<storage, read_write> state: SampleState;
// 256 个 smudge bucket 的持久状态。
@group(0) @binding(5) var<storage, read_write> buckets: array<Bucket>;
// 每个被延迟的 dab 一份上色结果（含 spec 预计算），供 dab 光栅 pass 读取。
@group(0) @binding(6) var<storage, read_write> smudge_results: array<SmudgeResult>;
// 每 tile 64 行 × 64 像素的采样位图（每行 2 个 u32，低位 = 小 x）。
// 每行的两个字只由负责该行的线程写入，因此不需要原子操作。
@group(0) @binding(7) var<storage, read_write> sample_bits: array<u32>;

const NONE: u32 = 0xFFFFFFFFu;
const ONE_FIX: f32 = 32768.0;

// collect 阶段每个 tile 的工作组用它汇总每行的采样点数。
var<workgroup> row_counts: array<u32, 64>;


fn fastlog2(x: f32) -> f32 {
    let vx = bitcast<u32>(x);
    let mx = bitcast<f32>((vx & 0x007FFFFFu) | 0x3F000000u);
    let y = f32(vx) * 1.1920929e-7;
    return y - 124.22552 - 1.4980303 * mx - 1.72588 / (0.3520887 + mx);
}

fn fastpow2(q: f32) -> f32 {
    let offset = select(0.0, 1.0, q < 0.0);
    let clipp = max(q, -126.0);
    let w = i32(clipp);
    let z = clipp - f32(w) + offset;
    let inner = clipp + 121.274055 + 27.728022 / (4.8425257 - z) - 1.4901291 * z;
    return bitcast<f32>(u32(8388608.0 * inner));
}

fn fastpow(x: f32, e: f32) -> f32 {
    return fastpow2(e * fastlog2(x));
}

const SR = array<f32, 10>(
    0.009281362787953, 0.009732627042016, 0.011254252737167,
    0.015105578649573, 0.024797924177217, 0.083622585502406,
    0.977865045723212, 1.0, 0.999961046144372, 0.999999992756822);
const SG = array<f32, 10>(
    0.002854127435775, 0.003917589679914, 0.012132151699187,
    0.748259205918013, 1.0, 0.865695937531795,
    0.037477469241101, 0.022816789725717, 0.021747419446456, 0.021384940572308);
const SB = array<f32, 10>(
    0.537052150373386, 0.546646402401469, 0.575501819073983,
    0.258778829633924, 0.041709923751716, 0.012662638828324,
    0.007485593127390, 0.006766900622462, 0.006699764779016, 0.006676219883241);
const TR = array<f32, 10>(
    0.026595621243689, 0.049779426257903, 0.022449850859496,
    -0.218453689278271, -0.256894883201278, 0.445881722194840,
    0.772365886289756, 0.194498761382537, 0.014038157587820, 0.007687264480513);
const TG = array<f32, 10>(
    -0.032601672674412, -0.061021043498478, -0.052490001018404,
    0.206659098273522, 0.572496335158169, 0.317837248815438,
    -0.021216624031211, -0.019387668756117, -0.001521339050858, -0.000835181622534);
const TB = array<f32, 10>(
    0.339475473216284, 0.635401374177222, 0.771520797089589,
    0.113222640692379, -0.055251113343776, -0.048222578468680,
    -0.012966666339586, -0.001523814504223, -0.000094718948810, -0.000051604594741);

fn spectral_of(r: f32, g: f32, b: f32, i: u32) -> f32 {
    let off = 0.999;
    return SR[i] * (r * off + 0.001) + SG[i] * (g * off + 0.001) + SB[i] * (b * off + 0.001);
}

fn spectral_to_rgb(mix: array<f32, 10>) -> vec3<f32> {
    var t = vec3<f32>(0.0);
    for (var i = 0u; i < 10u; i = i + 1u) {
        t = t + vec3<f32>(TR[i], TG[i], TB[i]) * mix[i];
    }
    let off = 0.999;
    return clamp((t - vec3<f32>(0.001)) / off, vec3<f32>(0.0), vec3<f32>(1.0));
}

// MSVC 的 rand()：holdrand 是 32 位，结果只取 bits 16..30。
fn c_rand_step(state: ptr<function, u32>) -> u32 {
    let next = (*state) * 214013u + 2531011u;
    *state = next;
    return (next >> 16u) & 0x7fffu;
}

// 把 LCG 推进 `steps` 步（仿射变换 T(x) = a*x + c 的二进制幂）。
fn rng_advance(state: u32, steps: u32) -> u32 {
    var acc_a: u32 = 1u;
    var acc_c: u32 = 0u;
    var a_pow: u32 = 214013u;
    var c_pow: u32 = 2531011u;
    var n = steps;
    loop {
        if (n == 0u) { break; }
        if ((n & 1u) == 1u) {
            acc_a = acc_a * a_pow;
            acc_c = acc_c * a_pow + c_pow;
        }
        c_pow = c_pow * a_pow + c_pow;
        a_pow = a_pow * a_pow;
        n = n >> 1u;
    }
    return acc_a * state + acc_c;
}

// libmypaint 的 hardness 落降（`opa_at`），采样固定 hardness = 0.5。
fn opa_at(rr: f32, hardness: f32) -> f32 {
    if (rr > 1.0) { return 0.0; }
    if (hardness >= 1.0) { return 1.0; }
    if (hardness <= 0.0) { return 1.0 - rr; }
    if (rr <= hardness) { return 1.0 + rr * (1.0 - 1.0 / hardness); }
    return (hardness / (1.0 - hardness)) * (1.0 - rr);
}

// ---- band 几何 ----

fn tile_of(tile: u32) -> vec2<i32> {
    let cols = u32(p.g.z);
    return vec2<i32>(p.g.x + i32(tile % cols), p.g.y + i32(tile / cols));
}

// 第 `yp` 行（tile 内）在采样圆内的 x 区间；与 `fill_mask_row` 一致。
fn row_span(yl: f32, yp: i32, r_fringe: f32, xl: f32) -> vec2<i32> {
    let dy = f32(yp) + 0.5 - yl;
    let rem = r_fringe * r_fringe - dy * dy;
    if (rem < 0.0) { return vec2<i32>(1, 0); }
    let w = sqrt(rem);
    let x0 = max(i32(floor(xl - w - 1.0)), 0);
    let x1 = min(i32(floor(xl + w + 1.0)), 63);
    return vec2<i32>(x0, x1);
}

fn mask_at(xl: f32, yl: f32, xp: i32, yp: i32, inv_r2: f32) -> u32 {
    let dy = f32(yp) + 0.5 - yl;
    let dx = f32(xp) + 0.5 - xl;
    let rr = (dy * dy + dx * dx) * inv_r2;
    return u32(opa_at(rr, 0.5) * 0.5 * 32768.0);
}

// ---- 1) 每 tile 一遍：逐行找覆盖区间，算 row_cum / tile draws，清位图 ----

@compute @workgroup_size(64)
fn count_prefix_pass(@builtin(workgroup_id) wid: vec3<u32>,
                     @builtin(local_invocation_id) lid: vec3<u32>) {
    let tile = wid.x;
    let row = lid.x;
    let idx = tile * 64u + row;
    let tx = tile_of(tile);
    let ts = 64.0;
    let xl = p.c.x - f32(tx.x) * ts;
    let yl = p.c.y - f32(tx.y) * ts;
    let radius = max(p.c.z, 1.0);
    let r_fringe = radius + 1.0;
    let inv_r2 = 1.0 / (radius * radius);

    let bit_base = OFF_BITS + tile * 128u + row * 2u;
    sample_bits[bit_base] = 0u;
    sample_bits[bit_base + 1u] = 0u;

    var first: u32 = 0u;
    var count: u32 = 0u;
    let span = row_span(yl, i32(row), r_fringe, xl);
    if (span.x <= span.y) {
        // mask 沿 x 远离圆心单调下降；找到首尾非零像素后整段都非零。
        var f = span.x;
        while (f <= span.y && mask_at(xl, yl, f, i32(row), inv_r2) == 0u) {
            f = f + 1;
        }
        if (f <= span.y) {
            var l = span.y;
            while (l >= f && mask_at(xl, yl, l, i32(row), inv_r2) == 0u) {
                l = l - 1;
            }
            first = u32(f);
            count = u32(l - f + 1);
        }
    }
    bands[OFF_COUNT + idx] = count;
    bands[OFF_FIRST + idx] = first;

    // 同一工作组内先等所有行的 count 落盘，再由 0 号线程串行算 row_cum。
    storageBarrier();
    if (lid.x == 0u) {
        let interval = max(p.n.x, 1u);
        var cum: u32 = 0u;
        for (var r = 0u; r < 64u; r = r + 1u) {
            let i2 = tile * 64u + r;
            bands[OFF_ROW_CUM + i2] = cum;
            cum = cum + bands[OFF_COUNT + i2];
        }
        // counter==0 的像素不消耗 RNG；每个 tile 的 counter 从 0 开始。
        var zeros: u32 = 0u;
        if (cum > 0u) {
            zeros = 1u + (cum - 1u) / interval;
        }
        bands[OFF_TILE_DRAWS + tile] = cum - zeros;
    }
}

// ---- 2) 跨 tile 串行推进 RNG（最多 2048 次）----

@compute @workgroup_size(1)
fn tile_rng_prefix_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tiles = u32(p.g.z) * u32(p.g.w);
    // 链式模式下接着上一个 dab 的终态；非链式（独立采样）用 CPU 给的状态。
    var rng_state: u32 = select(p.n.z, state.rng, p.e.y != 0u);
    for (var t = 0u; t < tiles; t = t + 1u) {
        bands[OFF_TILE_START + t] = rng_state;
        rng_state = rng_advance(rng_state, bands[OFF_TILE_DRAWS + t]);
    }
    state.rng = rng_state;
}

// ---- 3) 每行一个线程顺序抽样，只往位图里写 ----
//
// 每行只做一次 `rng_advance` 跳到行首，然后在行内顺序走 64 个像素——
// 与参考实现的 RNG 消耗顺序完全一致。位图让 accumulate 仍能按行主序回放。

@compute @workgroup_size(64)
fn collect_bits_pass(@builtin(workgroup_id) wid: vec3<u32>,
                     @builtin(local_invocation_id) lid: vec3<u32>) {
    let tile = wid.x;
    let row = lid.x;
    let idx = tile * 64u + row;
    let count = bands[OFF_COUNT + idx];
    let bit_base = OFF_BITS + tile * 128u + row * 2u;
    var bits0: u32 = 0u;
    var bits1: u32 = 0u;

    if (count != 0u) {
        let interval = max(p.n.x, 1u);
        let threshold = p.n.y;
        let first = bands[OFF_FIRST + idx];
        let k_row = bands[OFF_ROW_CUM + idx];
        // 行首之前消耗的 RNG 次数 = 覆盖像素数 - counter==0 的位置数。
        var zeros: u32 = 0u;
        if (k_row > 0u) {
            zeros = 1u + (k_row - 1u) / interval;
        }
        var rng = rng_advance(bands[OFF_TILE_START + tile], k_row - zeros);
        var counter = k_row % interval;

        for (var i = 0u; i < count; i = i + 1u) {
            let x = first + i;
            var sampled = counter == 0u;
            if (!sampled && interval > 1u) {
                sampled = i32(c_rand_step(&rng)) < i32(threshold);
            }
            if (sampled) {
                if (x < 32u) {
                    bits0 = bits0 | (1u << x);
                } else {
                    bits1 = bits1 | (1u << (x - 32u));
                }
            }
            counter = counter + 1u;
            if (counter == interval) { counter = 0u; }
        }
    }
    sample_bits[bit_base] = bits0;
    sample_bits[bit_base + 1u] = bits1;
    row_counts[row] = countOneBits(bits0) + countOneBits(bits1);
    workgroupBarrier();
    if (lid.x == 0u) {
        var run: u32 = 0u;
        for (var r = 0u; r < 64u; r = r + 1u) {
            bands[OFF_ROW_OFF + tile * 64u + r] = run;
            run = run + row_counts[r];
        }
        bands[OFF_TILE_COUNT + tile] = min(run, MAX_SAMPLES_PER_TILE);
    }
    workgroupBarrier();

    // 固定 per-tile 槽位：直接展开写，省掉全局列表前缀与单独写入 pass。
    var out = tile * MAX_SAMPLES_PER_TILE + bands[OFF_ROW_OFF + idx];
    var w0 = bits0;
    var w1 = bits1;
    let limit = (tile + 1u) * MAX_SAMPLES_PER_TILE;
    while (w0 != 0u) {
        let x = firstTrailingBit(w0);
        w0 = w0 & (w0 - 1u);
        if (out < limit) {
            write_expanded(out, read_pixel(tile, row, x));
        }
        out = out + 1u;
    }
    while (w1 != 0u) {
        let x = 32u + firstTrailingBit(w1);
        w1 = w1 & (w1 - 1u);
        if (out < limit) {
            write_expanded(out, read_pixel(tile, row, x));
        }
        out = out + 1u;
    }
}

// ---- 3b) 把每个 tile 的采样点数前缀成全局列表偏移 ----


// ---- 3c) 按行主序把采样点展开写入全局列表 ----
//
// 每个条目 16 个 u32：(a, mask, spectrum[10], straight rgb[3])。
// 随后的 accumulate 只顺序读这块内存，不再碰 atlas/几何。

fn write_expanded(out: u32, s: SampleData) {
    let base = OFF_EXPANDED + out * 16u;
    let a = f32(s.m) * f32(s.pa) / 1073741824.0;
    bands[base] = bitcast<u32>(a);
    bands[base + 1u] = bitcast<u32>(f32(s.m));
    if (s.pa > 0u) {
        let inv_pa = 1.0 / f32(s.pa);
        let fa = f32(s.pr) * inv_pa;
        let fb = f32(s.pg) * inv_pa;
        let fc = f32(s.pb) * inv_pa;
        for (var c = 0u; c < 10u; c = c + 1u) {
            bands[base + 2u + c] = bitcast<u32>(spectral_of(fa, fb, fc, c));
        }
        bands[base + 12u] = bitcast<u32>(fa);
        bands[base + 13u] = bitcast<u32>(fb);
        bands[base + 14u] = bitcast<u32>(fc);
    } else {
        for (var c = 0u; c < 10u; c = c + 1u) {
            bands[base + 2u + c] = bitcast<u32>(spectral_of(0.0, 0.0, 0.0, c));
        }
        bands[base + 12u] = 0u;
        bands[base + 13u] = 0u;
        bands[base + 14u] = 0u;
    }
}


// ---- 4) 并行 WGM：带权几何平均与顺序无关，两阶段归约 ----

struct SampleData {
    m: u32,
    pr: u32,
    pg: u32,
    pb: u32,
    pa: u32,
};

fn read_pixel(tile: u32, row: u32, x: u32) -> SampleData {
    let tx = tile_of(tile);
    let ts = 64.0;
    let xl = p.c.x - f32(tx.x) * ts;
    let yl = p.c.y - f32(tx.y) * ts;
    let radius = max(p.c.z, 1.0);
    let inv_r2 = 1.0 / (radius * radius);
    var out: SampleData;
    out.m = mask_at(xl, yl, i32(x), i32(row), inv_r2);
    out.pr = 0u;
    out.pg = 0u;
    out.pb = 0u;
    out.pa = 0u;
    let slot = slots[p.e.z + tile];
    if (slot != NONE) {
        let idx = slot * 8192u + (row * 64u + x) * 2u;
        let rg = atlas[idx];
        let ba = atlas[idx + 1u];
        out.pr = rg & 0xffffu;
        out.pg = rg >> 16u;
        out.pb = ba & 0xffffu;
        out.pa = ba >> 16u;
    }
    return out;
}

// 单线程按顺序重放 running-WGM；数据已由 list_write 展开成连续内存，
// 保留参考实现的 fastpow 逐步误差，但不再做几何/atlas 的随机读。

@compute @workgroup_size(1)
fn accumulate_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let paint = p.c.w;
    let tiles = u32(p.g.z) * u32(p.g.w);
    var sum_weight = 0.0;
    var sum_a = 0.0;
    // libmypaint runs get_color_pixels_accumulate per tile with the running
    // `sum_r/g/b`: each call re-derives its spectral accumulator from those
    // sums and writes the mixed result back. Keep the same boundary; carrying
    // one spectral accumulator across tiles changes multi-tile samples.
    var sum_r = 0.0;
    var sum_g = 0.0;
    var sum_b = 0.0;

    for (var t = 0u; t < tiles; t = t + 1u) {
      var spec: array<f32, 10>;
      for (var c = 0u; c < 10u; c = c + 1u) {
          spec[c] = 0.0;
      }
      if (paint > 0.0) {
          for (var c = 0u; c < 10u; c = c + 1u) {
              spec[c] = spectral_of(sum_r, sum_g, sum_b, c);
          }
      }
      var avg_rgb = vec3<f32>(sum_r, sum_g, sum_b);
      let n = min(bands[OFF_TILE_COUNT + t], MAX_SAMPLES_PER_TILE);
      for (var s = 0u; s < n; s = s + 1u) {
        let base = OFF_EXPANDED + (t * MAX_SAMPLES_PER_TILE + s) * 16u;
        let a = bitcast<f32>(bands[base]);
        let m = bitcast<f32>(bands[base + 1u]);
        sum_weight = sum_weight + m / 32768.0;
        let alpha_sums = a + sum_a;
        var fac_a = 1.0;
        var fac_b = 1.0;
        if (alpha_sums > 0.0) {
            fac_a = a / alpha_sums;
            fac_b = 1.0 - fac_a;
        }
        if (paint > 0.0 && a > 0.0) {
            for (var c = 0u; c < 10u; c = c + 1u) {
                let se = bitcast<f32>(bands[base + 2u + c]);
                spec[c] = fastpow(se, fac_a) * fastpow(spec[c], fac_b);
            }
        }
        if (paint < 1.0 && a > 0.0) {
            let rgb = vec3<f32>(
                bitcast<f32>(bands[base + 12u]),
                bitcast<f32>(bands[base + 13u]),
                bitcast<f32>(bands[base + 14u]));
            avg_rgb = rgb * fac_a + avg_rgb * fac_b;
        }
        sum_a = sum_a + a;
      }
      let spec_rgb = spectral_to_rgb(spec);
      sum_r = spec_rgb.x * paint + (1.0 - paint) * avg_rgb.x;
      sum_g = spec_rgb.y * paint + (1.0 - paint) * avg_rgb.y;
      sum_b = spec_rgb.z * paint + (1.0 - paint) * avg_rgb.z;
    }

    if (sum_weight <= 0.0) {
        state.result = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        return;
    }
    let a_out = clamp(sum_a / sum_weight, 0.0, 1.0);
    if (sum_a <= 0.0) {
        state.result = vec4<f32>(0.0, 1.0, 0.0, a_out);
        return;
    }
    state.result = vec4<f32>(
        clamp(sum_r, 0.0, 1.0),
        clamp(sum_g, 0.0, 1.0),
        clamp(sum_b, 0.0, 1.0),
        a_out,
    );
}

// ---- 5) smudge bucket + apply_smudge（每个 dab 一个线程）----

fn spec_arr(r: f32, g: f32, b: f32) -> array<f32, 10> {
    var out: array<f32, 10>;
    for (var i = 0u; i < 10u; i = i + 1u) {
        out[i] = spectral_of(r, g, b, i);
    }
    return out;
}

// `spectral::mix_colors`（helpers.c）：这里必须用真实 pow，而不是 fastpow。
fn mix_colors_wgm(a: vec4<f32>, b: vec4<f32>, fac: f32, paint_mode: f32) -> vec4<f32> {
    let opa_a = fac;
    let opa_b = 1.0 - opa_a;
    let out_a = clamp(opa_a * a.a + opa_b * b.a, 0.0, 1.0);
    var sfac_a = 0.0;
    if (a.a != 0.0) {
        sfac_a = opa_a * a.a / (a.a + b.a * opa_b);
    }
    let sfac_b = 1.0 - sfac_a;
    var result = vec3<f32>(0.0);
    if (paint_mode > 0.0) {
        let sa = spec_arr(a.r, a.g, a.b);
        let sb = spec_arr(b.r, b.g, b.b);
        var mix: array<f32, 10>;
        for (var i = 0u; i < 10u; i = i + 1u) {
            mix[i] = pow(sa[i], sfac_a) * pow(sb[i], sfac_b);
        }
        result = spectral_to_rgb(mix);
    }
    if (paint_mode < 1.0) {
        let lin = vec3<f32>(
            a.r * opa_a + b.r * opa_b,
            a.g * opa_a + b.g * opa_b,
            a.b * opa_a + b.b * opa_b);
        result = result * paint_mode + (1.0 - paint_mode) * lin;
    }
    return vec4<f32>(result, out_a);
}

@compute @workgroup_size(1)
fn reset_buckets_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = 0u; i < 256u; i = i + 1u) {
        buckets[i].color = vec4<f32>(0.0);
        buckets[i].prev = vec4<f32>(0.0);
        buckets[i].recentness = 0.0;
    }
}

fn do_smudge(sample_color: vec4<f32>) {
    let bindex = p.v.x;
    var bucket = buckets[bindex];
    if (p.v.y != 0u) {
        bucket.color = p.w;
        bucket.prev = p.x;
        bucket.recentness = p.y.x;
    }
    var skip = false;
    if (p.t.w != 0.0) {
        let length = clamp(p.s.y, 0.0, 1.0);
        if (length < 1.0) {
            var update_factor = max(length, 0.01);
            let recentness = bucket.recentness * update_factor;
            bucket.recentness = recentness;
            let threshold = min(pow(0.5 * update_factor, p.s.z), 1.0) + 1e-16;
            var sample = bucket.prev;
            if (recentness < threshold && p.e.w != 0u) {
                if (recentness == 0.0) {
                    update_factor = 0.0;
                }
                bucket.recentness = 1.0;
                let s = sample_color;
                let lim = p.t.y;
                if ((lim > 0.0 && s.a < lim) || (lim < 0.0 && s.a > -lim)) {
                    skip = true;
                } else {
                    bucket.prev = s;
                }
                sample = s;
            }
            if (!skip) {
                let fac = update_factor;
                let pm = clamp(p.t.x, 0.0, 1.0);
                if (pm > 0.0) {
                    if (sample.a > 0.01) {
                        bucket.color = mix_colors_wgm(bucket.color, sample, fac, pm);
                    } else {
                        bucket.color.a = (bucket.color.a + sample.a) * 0.5;
                    }
                } else {
                    let fac_new = (1.0 - fac) * sample.a;
                    bucket.color = vec4<f32>(
                        bucket.color.r * fac + sample.r * fac_new,
                        bucket.color.g * fac + sample.g * fac_new,
                        bucket.color.b * fac + sample.b * fac_new,
                        clamp(bucket.color.a * fac + (1.0 - fac) * sample.a, 0.0, 1.0));
                }
            }
            buckets[bindex] = bucket;
        }
    }

    // apply_smudge
    let amt = clamp(p.s.x, 0.0, 1.0);
    let paint_mode = clamp(p.t.x, 0.0, 1.0);
    let eraser_target = clamp((1.0 - amt) + amt * bucket.color.a, 0.0, 1.0);
    var mixed = p.u.xyz;
    if (amt > 0.0 && eraser_target > 0.0) {
        if (paint_mode > 0.0) {
            mixed = mix_colors_wgm(bucket.color, vec4<f32>(p.u.xyz, 1.0), amt, paint_mode).rgb;
            mixed = clamp(mixed, vec3<f32>(0.0), vec3<f32>(1.0));
        } else {
            let cf = 1.0 - amt;
            mixed = clamp(
                (amt * bucket.color.rgb + cf * p.u.xyz) / eraser_target,
                vec3<f32>(0.0),
                vec3<f32>(1.0));
        }
    }
    let alpha_eraser = clamp(eraser_target * (1.0 - clamp(p.t.z, 0.0, 1.0)), 0.0, 1.0);
    let src = vec3<u32>(
        u32(clamp(mixed.x, 0.0, 1.0) * ONE_FIX),
        u32(clamp(mixed.y, 0.0, 1.0) * ONE_FIX),
        u32(clamp(mixed.z, 0.0, 1.0) * ONE_FIX));
    var res: SmudgeResult;
    res.color = vec4<f32>(mixed, alpha_eraser);
    for (var i = 0u; i < 10u; i = i + 1u) {
        let spec = spectral_of(
            f32(src.x) / ONE_FIX,
            f32(src.y) / ONE_FIX,
            f32(src.z) / ONE_FIX,
            i);
        res.spec_log[i / 4u][i % 4u] = fastlog2(spec);
        res.spec_zero[i / 4u][i % 4u] = 1.0;
    }
    res.flags = vec4<u32>(select(0u, 1u, skip), 0u, 0u, 0u);
    buckets[bindex] = bucket;
    smudge_results[p.v.z] = res;
}

@compute @workgroup_size(1)
fn smudge_from_state_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    do_smudge(state.result);
}


"#;

/// 一次 flush 里最多允许多少个延迟 smudge dab。
pub(crate) const MAX_DEFERRED: usize = 4096;
/// 采样 bbox 的最大 tile 数（半径上限 1000 → 32×32）。
pub(crate) const MAX_SAMPLE_TILES: u32 = 32 * 32;
/// 采样 uniform 的 wgpu 动态偏移对齐。
pub(crate) const SAMPLE_PARAM_STRIDE: u64 = 256;
/// 第 0 个 slot 留给独立的 `sample()` 调试入口。
pub(crate) const SAMPLE_PARAM_SLOTS: usize = MAX_DEFERRED + 1;
pub(crate) const SAMPLE_SLOT_WORDS: usize = SAMPLE_PARAM_SLOTS * MAX_SAMPLE_TILES as usize;
pub(crate) const BUCKET_BYTES: u64 = 256 * 48;
pub(crate) const SMUDGE_RESULT_BYTES: u64 = (MAX_DEFERRED + 1) as u64 * 128;
pub(crate) const SAMPLE_PARAM_BYTES: u64 = SAMPLE_PARAM_SLOTS as u64 * SAMPLE_PARAM_STRIDE;
/// Shader `MAX_BANDS`（半径 1000 的 bbox = 1024 tile × 64 行）。
const MAX_BANDS: usize = 65536;
const MAX_TILES: usize = 2048;
const MAX_SAMPLES_PER_TILE: usize = 256;
const BAND_STATE_BYTES: u64 = ((MAX_BANDS * 4
    + MAX_TILES * 132
    + MAX_SAMPLE_TILES as usize * MAX_SAMPLES_PER_TILE * 16)
    * 4) as u64;
const BITS_BYTES: u64 = (MAX_TILES * 128 * 4) as u64;
const SAMPLE_STATE_BYTES: u64 = 32;

/// 一个延迟 smudge dab 的完整 GPU uniform 载荷。
#[derive(Clone, Copy, Debug)]
pub(crate) struct DeferredSample {
    pub cx: f32,
    pub cy: f32,
    pub radius: f32,
    pub paint: f32,
    pub tx0: i32,
    pub ty0: i32,
    pub cols: u32,
    pub rows: u32,
    pub cap: u32,
    pub chain_rng: bool,
    pub has_sample: bool,
    pub rng_start: u32,
    pub slot_base: u32,
    pub smudge_amt: f32,
    pub smudge_length: f32,
    pub smudge_length_log: f32,
    pub smudge_radius: f32,
    pub paint_mode: f32,
    pub smudge_op_lim: f32,
    pub eraser: f32,
    pub mapping_active: bool,
    pub base_color: [f32; 3],
    pub sample_paint: f32,
    pub bucket_index: u32,
    pub seed_bucket: bool,
    pub run_smudge: bool,
    pub result_index: u32,
    pub bucket_color: [f32; 4],
    pub bucket_prev: [f32; 4],
    pub recentness: f32,
}

impl DeferredSample {
    pub(crate) fn bytes(&self) -> [u8; 176] {
        let mut ub = [0u8; 176];
        let mut u32_at = |at: usize, v: u32| {
            ub[at..at + 4].copy_from_slice(&v.to_ne_bytes());
        };
        macro_rules! f32_at {
            ($at:expr, $v:expr) => {
                u32_at($at, ($v as f32).to_bits())
            };
        }
        let radius = self.radius.max(1.0);
        let interval = if radius <= 2.0 {
            1u32
        } else {
            (radius * 7.0) as u32
        };
        let threshold = ((1.0 / (7.0 * radius)) * 32767.0) as u32;
        f32_at!(0, self.cx);
        f32_at!(4, self.cy);
        f32_at!(8, radius);
        f32_at!(12, self.paint);
        u32_at(16, self.tx0 as u32);
        u32_at(20, self.ty0 as u32);
        u32_at(24, self.cols);
        u32_at(28, self.rows);
        u32_at(32, interval);
        u32_at(36, threshold);
        u32_at(40, self.rng_start);
        u32_at(44, BAND_ROWS);
        u32_at(48, self.cap);
        u32_at(52, self.chain_rng as u32);
        u32_at(56, self.slot_base);
        u32_at(60, self.has_sample as u32);
        f32_at!(64, self.smudge_amt);
        f32_at!(68, self.smudge_length);
        f32_at!(72, self.smudge_length_log);
        f32_at!(76, self.smudge_radius);
        f32_at!(80, self.paint_mode);
        f32_at!(84, self.smudge_op_lim);
        f32_at!(88, self.eraser);
        f32_at!(92, self.mapping_active as u32 as f32);
        f32_at!(96, self.base_color[0]);
        f32_at!(100, self.base_color[1]);
        f32_at!(104, self.base_color[2]);
        f32_at!(108, self.sample_paint);
        u32_at(112, self.bucket_index);
        u32_at(116, self.seed_bucket as u32);
        u32_at(120, self.result_index);
        u32_at(124, self.run_smudge as u32);
        for (i, v) in self.bucket_color.iter().enumerate() {
            f32_at!(128 + i * 4, *v);
        }
        for (i, v) in self.bucket_prev.iter().enumerate() {
            f32_at!(144 + i * 4, *v);
        }
        f32_at!(160, self.recentness);
        ub
    }
}

/// 采样内核。绑定一次 atlas 之后可以反复调用。
pub struct GpuSampler {
    count_prefix: wgpu::ComputePipeline,
    tile_rng_prefix: wgpu::ComputePipeline,
    collect_bits: wgpu::ComputePipeline,
    accumulate: wgpu::ComputePipeline,
    reset_buckets: wgpu::ComputePipeline,
    smudge: wgpu::ComputePipeline,
    /// 保留一份 atlas 句柄，重建绑定组时要绑回去（`wgpu::Buffer` 是引用计数）。
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    slots: wgpu::Buffer,
    state: wgpu::Buffer,
    out_readback: wgpu::Buffer,
    buckets: wgpu::Buffer,
}

/// 一次采样要占用的 band 数（`cols * rows * BANDS_PER_TILE`）。
pub(crate) fn band_count(cols: u32, rows: u32) -> u32 {
    cols * rows * BANDS_PER_TILE
}

impl GpuSampler {
    pub fn new(
        device: &wgpu::Device,
        atlas: &wgpu::Buffer,
        buckets: &wgpu::Buffer,
        smudge_results: &wgpu::Buffer,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu sample"),
            source: wgpu::ShaderSource::Wgsl(SAMPLE_WGSL.into()),
        });
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu sample layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(176),
                    },
                    count: None,
                },
                storage(1, true),
                storage(2, true),
                storage(3, false),
                storage(4, false),
                storage(5, false),
                storage(6, false),
                storage(7, false),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu sample pipeline layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let make_pipeline = |label: &str, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let count_prefix = make_pipeline("gpu sample count prefix", "count_prefix_pass");
        let tile_rng_prefix =
            make_pipeline("gpu sample tile rng prefix", "tile_rng_prefix_pass");
        let collect_bits = make_pipeline("gpu sample collect bits", "collect_bits_pass");
        let accumulate = make_pipeline("gpu sample accumulate", "accumulate_pass");
        let reset_buckets = make_pipeline("gpu sample reset buckets", "reset_buckets_pass");
        let smudge =
            make_pipeline("gpu sample smudge", "smudge_from_state_pass");

        let make_buf = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let params = make_buf(
            "gpu sample params",
            SAMPLE_PARAM_BYTES,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let rw = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        let slots = make_buf(
            "gpu sample slots",
            (SAMPLE_SLOT_WORDS as u64) * 4,
            rw,
        );
        let bands = make_buf("gpu sample bands", BAND_STATE_BYTES, rw);
        let sample_bits = make_buf("gpu sample bits", BITS_BYTES, rw);
        let state = make_buf(
            "gpu sample state",
            SAMPLE_STATE_BYTES,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let out_readback = make_buf(
            "gpu sample readback",
            32,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu sample bind group"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params,
                        offset: 0,
                        size: std::num::NonZeroU64::new(176),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: atlas.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: slots.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: bands.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: state.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: buckets.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: smudge_results.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: sample_bits.as_entire_binding(),
                },
            ],
        });
        Self {
            count_prefix,
            tile_rng_prefix,
            collect_bits,
            accumulate,
            reset_buckets,
            smudge,
            bind_group,
            params,
            slots,
            state,
            out_readback,
            buckets: buckets.clone(),
        }
    }

    /// 把 4 个采样 dispatch 录进 `encoder`（不起提交）。
    pub fn encode_sample(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        bands: u32,
        param_offset: u64,
    ) {
        let tiles = (bands / BANDS_PER_TILE).max(1);
        for (pipeline, groups) in [
            (&self.count_prefix, tiles),
            (&self.tile_rng_prefix, 1),
            (&self.collect_bits, tiles),
            (&self.accumulate, 1),
        ] {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gpu sample"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.bind_group, &[param_offset as u32]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
    }

    /// 清空设备侧 bucket 表（brush reset）。
    pub fn encode_reset_buckets(&self, encoder: &mut wgpu::CommandEncoder, param_offset: u64) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gpu sample reset buckets"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.reset_buckets);
        pass.set_bind_group(0, &self.bind_group, &[param_offset as u32]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    /// 把 smudge bucket / apply_smudge / spec 预计算录进 `encoder`。
    pub fn encode_smudge(&self, encoder: &mut wgpu::CommandEncoder, param_offset: u64) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gpu sample smudge"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.smudge);
        pass.set_bind_group(0, &self.bind_group, &[param_offset as u32]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    pub fn params_buffer(&self) -> &wgpu::Buffer {
        &self.params
    }

    pub fn slots_buffer(&self) -> &wgpu::Buffer {
        &self.slots
    }

    /// `SampleState` 缓冲：前 16 字节是采样色，offset 16 是链式 RNG 状态。
    pub fn state_buffer(&self) -> &wgpu::Buffer {
        &self.state
    }

    pub fn bucket_buffer(&self) -> &wgpu::Buffer {
        &self.buckets
    }

    pub fn write_params(&self, queue: &wgpu::Queue, offset: u64, bytes: &[u8]) {
        queue.write_buffer(&self.params, offset, bytes);
    }

    /// band 表按固定上限分配；这里仅校验不会越界。
    pub fn ensure_capacity(&mut self, _device: &wgpu::Device, bands: u32, _samples: u32) {
        debug_assert!(bands as usize <= MAX_BANDS);
    }

    /// 按 `get_color_pigment` 的语义采样一次，返回 `[r, g, b, a]`。
    ///
    /// `slots` 是 bbox 内的稠密槽位表（行主序，缺失填 `u32::MAX`），
    /// 与 `tx0 / ty0 / cols / rows` 一起描述采样范围。
    ///
    /// 这是**对照/调试入口**：它会把结果回读（一次同步）。融合进 dab 链之后
    /// 不再需要回读，采样结果直接进 storage buffer。
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slots: &[u32],
        tx0: i32,
        ty0: i32,
        cols: u32,
        rows: u32,
        cx: f32,
        cy: f32,
        radius: f32,
        paint: f32,
        rng_state: u32,
    ) -> [f32; 4] {
        let bands = band_count(cols, rows).max(1);
        let radius = radius.max(1.0);
        let interval = if radius <= 2.0 {
            1u32
        } else {
            (radius * 7.0) as u32
        };
        // 每 band 的采样槽位数：期望覆盖数 / interval，再留 64 的余量吸收
        // 二项分布的波动（`samples` 是环形写入的上界，超过会被截断）。
        let cap = if interval <= 1 {
            BAND_ROWS * 64
        } else {
            (BAND_ROWS * 64) / interval + 64
        };
        self.ensure_capacity(device, bands, bands * cap);
        // 调试入口固定用 slot 0；延迟链从 slot 1 开始。
        queue.write_buffer(&self.slots, 0, bytemuck_u32(slots));

        let p = DeferredSample {
            cx,
            cy,
            radius,
            paint,
            tx0,
            ty0,
            cols,
            rows,
            cap,
            chain_rng: false,
            has_sample: true,
            rng_start: rng_state,
            slot_base: 0,
            smudge_amt: 0.0,
            smudge_length: 1.0,
            smudge_length_log: 0.0,
            smudge_radius: radius,
            paint_mode: 1.0,
            smudge_op_lim: 0.0,
            eraser: 0.0,
            mapping_active: false,
            base_color: [0.0; 3],
            sample_paint: paint,
            bucket_index: 0,
            seed_bucket: false,
            run_smudge: false,
            result_index: 0,
            bucket_color: [0.0; 4],
            bucket_prev: [0.0; 4],
            recentness: 0.0,
        };
        queue.write_buffer(&self.params, 0, &p.bytes());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu sample"),
        });
        self.encode_sample(&mut encoder, bands, 0);
        encoder.copy_buffer_to_buffer(&self.state, 0, &self.out_readback, 0, 32);
        queue.submit(Some(encoder.finish()));

        // 诊断/基准用：只提交不等结果，用来量"链内"成本（无回读往返）。
        if std::env::var_os("PIGMENT_SAMPLE_NOSYNC").is_some() {
            return [0.0; 4];
        }

        let rb = self.out_readback.slice(..32);
        rb.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok();
        let mut res = [0.0f32; 4];
        if let Ok(data) = rb.get_mapped_range() {
            for i in 0..4 {
                let mut b = [0u8; 4];
                b.copy_from_slice(&data[i * 4..i * 4 + 4]);
                res[i] = f32::from_ne_bytes(b);
            }
            // Keep the CPU C-rand mirror in step with the device sample.
            let mut b = [0u8; 4];
            b.copy_from_slice(&data[16..20]);
            hokusai::brushmodes::set_color_rng_state(u32::from_ne_bytes(b) as u64);
        }
        self.out_readback.unmap();
        res
    }

}

fn bytemuck_u32(v: &[u32]) -> &[u8] {
    // SAFETY: u32 无填充、对齐要求高于 u8。
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

/// 记录一次采样耗时（诊断用）。
pub fn note_sample_ns(ns: u64) {
    perf::add(&perf::SAMPLE_NS, ns);
    perf::add(&perf::SAMPLE_CALLS, 1);
}
