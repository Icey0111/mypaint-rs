//! GPU dab 光栅器 —— 把一次指针事件内的所有 dab 批量交给 compute shader。
//!
//! 设计约束：
//!
//! - **CPU 仍是权威**。事件结束时把被修改的 tile 回读进 `PigmentSurface`，
//!   撤销快照、mipmap、渲染上传、存盘全部走原来的 CPU 数据，不需要第二套
//!   显示/金字塔架构。
//! - **顺序**。dab 按引擎调用顺序逐个 dispatch（每个 dispatch 一个 compute
//!   pass，wgpu 会在 pass 之间插入屏障），后一个 dab 能看到前一个的结果。
//! - **逐位一致**。着色器重放的是 `brushmodes.rs` 的 fix15 整数混合与
//!   `spectral.rs` 的 WGM（fastlog2/fastpow2 的运算顺序一致）。不支持的
//!   blend 组合在 `queue_dab` 阶段就返回 `false`，由表面退回 CPU。
//!
//! 每个事件最多 [`ATLAS_TILES`] 块 tile；超过时整批退回 CPU（语义不变，
//! 只是这一次事件慢）。对低缩放大笔刷，一个事件的唯一 tile 数通常在
//! 几十到几百块，远低于上限。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hokusai::brushmodes::{draw_dab_default, GpuDabParams};
use hokusai::tile::TILE_SIZE;
use hokusai::{Dab, DeferredSmudge, SmudgeBucket};
use pigment_surface::{tile_bytes, GpuDabSink, PigmentSurface};

use crate::gpu_sample::{
    band_count, DeferredSample, BAND_ROWS, BUCKET_BYTES, MAX_DEFERRED, MAX_SAMPLE_TILES,
    SAMPLE_PARAM_STRIDE, SMUDGE_RESULT_BYTES,
};

const TILE_BYTES: usize = TILE_SIZE * TILE_SIZE * 4 * 2;
/// 每像素两个 u32：`r | g<<16`、`b | a<<16`。
const PIXEL_U32: usize = TILE_SIZE * TILE_SIZE * 2;
const ATLAS_TILES: usize = 2048;
const ATLAS_BYTES: u64 = (ATLAS_TILES * TILE_BYTES) as u64;
/// 每块 tile 一个 u32 脏标志（按槽位号索引）。
const FLAG_BYTES: u64 = (ATLAS_TILES * 4) as u64;
const MAX_DABS: usize = 8192;
const UNIFORM_STRIDE: u64 = 256;
const UNIFORM_SIZE: u64 = 192;
const UNIFORM_BYTES: u64 = (MAX_DABS as u64) * UNIFORM_STRIDE;
const DESC_CAPACITY: usize = 1 << 18;
const DESC_BYTES: u64 = (DESC_CAPACITY * 16) as u64;
const SMUDGE_NONE: u32 = u32::MAX;

/// 渲染帧最多提交多少个「延迟 smudge」dab；剩余留给下一帧。
fn flush_ops_budget() -> usize {
    use std::sync::OnceLock;
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("PIGMENT_GPU_FLUSH_OPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v| v > 0)
            .unwrap_or(8192)
    })
}

/// 普通光栅 dab 没有采样链，单 dab 便宜得多；快速笔画下它们会占满
/// 输入队列，所以用更大的帧批量追赶。
fn normal_ops_budget() -> usize {
    use std::sync::OnceLock;
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("PIGMENT_GPU_NORMAL_OPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v| v > 0)
            .unwrap_or(8192)
    })
}

/// 半径很小的延迟 dab（快速笔画时笔尖会被速度曲线压小）每个只有一两个
/// tile，固定 pass 开销占比高但总成本低，可以按更大的批量推进。
fn small_deferred_ops_budget() -> usize {
    use std::sync::OnceLock;
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("PIGMENT_GPU_SMALL_OPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v| v > 0)
            .unwrap_or(8192)
    })
}

const WGSL: &str = r#"
struct Params {
    // x, y, r_fringe, aspect
    a: vec4<f32>,
    // cs, sn, inv_r2, hardness
    b: vec4<f32>,
    // softness, opacity, color_a, 0
    c: vec4<f32>,
    // src_r, src_g, src_b, 0
    d: vec4<f32>,
    log_a: array<vec4<f32>, 3>,
    zero_pow: array<vec4<f32>, 3>,
    // desc_offset, desc_count, mode (0=eraser-only, 1=full paint, 2=mixed), 0
    dab_meta: vec4<f32>,
    // result index (bitcast), deferred flag, 0, 0
    smudge_idx: vec4<f32>,
};

struct DabColorResult {
    // rgb + alpha_eraser
    color: vec4<f32>,
    spec_log: array<vec4<f32>, 3>,
    spec_zero: array<vec4<f32>, 3>,
    // skip, 0, 0, 0
    flags: vec4<u32>,
};

const NONE: u32 = 0xFFFFFFFFu;

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> atlas: array<u32>;
@group(0) @binding(2) var<storage, read> tids: array<vec4<u32>>;
// 每 tile 一个「本帧真的变了」标志，回读阶段据此跳过没动的格子。
@group(0) @binding(3) var<storage, read_write> tile_dirty: array<atomic<u32>>;
// 延迟 smudge dab 的上色结果（GPU 侧算好后由本 pass 读取）。
@group(0) @binding(4) var<storage, read> smudge_results: array<DabColorResult>;
var<private> g_smudge: u32 = 0u;

// 工作组内先聚合，再由一个线程向全局写一次：整套光谱混合里
// 每像素一个全局 atomic 会把收益全吃掉。
var<workgroup> wg_hit: atomic<u32>;

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

// ---- 展开的 WGM 累积 ----
//
// 原来用 `array<f32,10>` 暂存 mix 再求和，动态下标会让 SPIR-V 把它放进
// 私有内存（每线程一张 40 字节的本地数组，可能溢出到 local memory），
// 这是 GPU 端只有峰値算力几个百分点的原因之一。下面把十项完全展开：
// 下标全是常量，常量表不再需要运行期寻址，和 CPU 的累加顺序仍逐项一致。
fn wgm_rgb(fa: f32, fb: f32, fc: f32, fac_a: f32, fac_b: f32) -> vec3<f32> {
    var t = vec3<f32>(0.0);
    t = t + vec3<f32>(TR[0], TG[0], TB[0])
        * (fastpow2(fac_a * log_a(0u)) * fastpow(spectral_of(fa, fb, fc, 0u), fac_b));
    t = t + vec3<f32>(TR[1], TG[1], TB[1])
        * (fastpow2(fac_a * log_a(1u)) * fastpow(spectral_of(fa, fb, fc, 1u), fac_b));
    t = t + vec3<f32>(TR[2], TG[2], TB[2])
        * (fastpow2(fac_a * log_a(2u)) * fastpow(spectral_of(fa, fb, fc, 2u), fac_b));
    t = t + vec3<f32>(TR[3], TG[3], TB[3])
        * (fastpow2(fac_a * log_a(3u)) * fastpow(spectral_of(fa, fb, fc, 3u), fac_b));
    t = t + vec3<f32>(TR[4], TG[4], TB[4])
        * (fastpow2(fac_a * log_a(4u)) * fastpow(spectral_of(fa, fb, fc, 4u), fac_b));
    t = t + vec3<f32>(TR[5], TG[5], TB[5])
        * (fastpow2(fac_a * log_a(5u)) * fastpow(spectral_of(fa, fb, fc, 5u), fac_b));
    t = t + vec3<f32>(TR[6], TG[6], TB[6])
        * (fastpow2(fac_a * log_a(6u)) * fastpow(spectral_of(fa, fb, fc, 6u), fac_b));
    t = t + vec3<f32>(TR[7], TG[7], TB[7])
        * (fastpow2(fac_a * log_a(7u)) * fastpow(spectral_of(fa, fb, fc, 7u), fac_b));
    t = t + vec3<f32>(TR[8], TG[8], TB[8])
        * (fastpow2(fac_a * log_a(8u)) * fastpow(spectral_of(fa, fb, fc, 8u), fac_b));
    t = t + vec3<f32>(TR[9], TG[9], TB[9])
        * (fastpow2(fac_a * log_a(9u)) * fastpow(spectral_of(fa, fb, fc, 9u), fac_b));
    let off = 0.999;
    return clamp((t - vec3<f32>(0.001)) / off, vec3<f32>(0.0), vec3<f32>(1.0));
}

// 纯涂抹路径：源色权重恒为 0，`fac_a == 0`，只需 `spec_a_zero_pow` 那一侧。
fn wgm_eraser_rgb(fa: f32, fb: f32, fc: f32) -> vec3<f32> {
    var t = vec3<f32>(0.0);
    t = t + vec3<f32>(TR[0], TG[0], TB[0]) * (zero_pow(0u) * fastpow(spectral_of(fa, fb, fc, 0u), 1.0));
    t = t + vec3<f32>(TR[1], TG[1], TB[1]) * (zero_pow(1u) * fastpow(spectral_of(fa, fb, fc, 1u), 1.0));
    t = t + vec3<f32>(TR[2], TG[2], TB[2]) * (zero_pow(2u) * fastpow(spectral_of(fa, fb, fc, 2u), 1.0));
    t = t + vec3<f32>(TR[3], TG[3], TB[3]) * (zero_pow(3u) * fastpow(spectral_of(fa, fb, fc, 3u), 1.0));
    t = t + vec3<f32>(TR[4], TG[4], TB[4]) * (zero_pow(4u) * fastpow(spectral_of(fa, fb, fc, 4u), 1.0));
    t = t + vec3<f32>(TR[5], TG[5], TB[5]) * (zero_pow(5u) * fastpow(spectral_of(fa, fb, fc, 5u), 1.0));
    t = t + vec3<f32>(TR[6], TG[6], TB[6]) * (zero_pow(6u) * fastpow(spectral_of(fa, fb, fc, 6u), 1.0));
    t = t + vec3<f32>(TR[7], TG[7], TB[7]) * (zero_pow(7u) * fastpow(spectral_of(fa, fb, fc, 7u), 1.0));
    t = t + vec3<f32>(TR[8], TG[8], TB[8]) * (zero_pow(8u) * fastpow(spectral_of(fa, fb, fc, 8u), 1.0));
    t = t + vec3<f32>(TR[9], TG[9], TB[9]) * (zero_pow(9u) * fastpow(spectral_of(fa, fb, fc, 9u), 1.0));
    let off = 0.999;
    return clamp((t - vec3<f32>(0.001)) / off, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn spectral_blend_factor(x: f32) -> f32 {
    let b = x * 8.0 - 3.0;
    return clamp(0.5 + b / (1.0 + abs(b) * 1.65), 0.0, 1.0);
}

fn log_a(i: u32) -> f32 {
    if (g_smudge != NONE) {
        return smudge_results[g_smudge].spec_log[i / 4u][i % 4u];
    }
    return p.log_a[i / 4u][i % 4u];
}

fn zero_pow(i: u32) -> f32 {
    if (g_smudge != NONE) {
        return smudge_results[g_smudge].spec_zero[i / 4u][i % 4u];
    }
    return p.zero_pow[i / 4u][i % 4u];
}

@compute @workgroup_size(16, 16)
fn main(@builtin(workgroup_id) wid: vec3<u32>,
        @builtin(local_invocation_id) lid: vec3<u32>) {
    let off = u32(p.dab_meta.x);
    // 一个 tile 是 64×64，workgroup 只有 16×16：x 方向 4 个块、y 方向 4 个块。
    let tid = tids[off + wid.x / 4u];
    let base = tid.x;
    // `tids.w` 是这块 tile 在 atlas 里的槽位号（脏标志的下标）。
    let slot = tid.w;
    let lx = (wid.x % 4u) * 16u + lid.x;
    let ly = wid.y * 16u + lid.y;

    g_smudge = bitcast<u32>(p.smudge_idx.x);
    let deferred = g_smudge != NONE;
    var src_r = u32(p.d.x);
    var src_g = u32(p.d.y);
    var src_b = u32(p.d.z);
    var color_a = u32(p.c.z);
    var mode = u32(p.dab_meta.z);
    var skip = false;
    if (deferred) {
        let r = smudge_results[g_smudge];
        src_r = u32(clamp(r.color.x, 0.0, 1.0) * 32768.0);
        src_g = u32(clamp(r.color.y, 0.0, 1.0) * 32768.0);
        src_b = u32(clamp(r.color.z, 0.0, 1.0) * 32768.0);
        color_a = u32(clamp(r.color.w, 0.0, 1.0) * 32768.0);
        skip = r.flags.x != 0u;
        if (color_a == 0u) {
            mode = 0u;
        } else if (color_a == 32768u) {
            mode = 1u;
        } else {
            mode = 2u;
        }
    }

    if (lid.x == 0u && lid.y == 0u) {
        atomicStore(&wg_hit, 0u);
    }
    workgroupBarrier();

    var changed = false;

    // ---- libmypaint draw_dab 的 mask（prepare_dab 的几何）----
    let wx = bitcast<f32>(tid.y) + f32(lx) + 0.5;
    let wy = bitcast<f32>(tid.z) + f32(ly) + 0.5;
    let yy = wy - p.a.y;
    let xx = wx - p.a.x;
    let yyr = (yy * p.b.x - xx * p.b.y) * p.a.w;
    let xxr = yy * p.b.y + xx * p.b.x;
    let rr = (yyr * yyr + xxr * xxr) * p.b.z;
    let hardness = p.b.w;
    var mf: f32 = 0.0;
    if rr <= 1.0 {
        if hardness >= 1.0 {
            mf = 1.0;
        } else if hardness <= 0.0 {
            mf = 1.0 - rr;
        } else if rr <= hardness {
            mf = 1.0 + rr * (1.0 - 1.0 / hardness);
        } else {
            mf = (hardness / (1.0 - hardness)) * (1.0 - rr);
        }
        mf = mf * (1.0 - p.c.x);
    }
    let mask = u32(mf * 32768.0);
    let idx = base + (ly * 64u + lx) * 2u;

    // 注意：这里**不能** early-return —— 后面还有 `workgroupBarrier()`，
    // 分支里返回会让屏障发散。整段包在 if 里，屏蔽掉没有覆盖的像素。
    if (!skip && mask != 0u) {
        let rg = atlas[idx];
        let ba = atlas[idx + 1u];
        let r0 = rg & 0xffffu;
        let g0 = rg >> 16u;
        let b0 = ba & 0xffffu;
        let a0 = ba >> 16u;

        let ONE = 32768u;
        let opacity = u32(p.c.y);
        let opa_a = mask * opacity / ONE;
        let opa_b = ONE - opa_a;

        var out_r: u32;
        var out_g: u32;
        var out_b: u32;
        var out_a: u32;

        if mode == 0u {
            // blend_eraser_only_paint：纯涂抹（color_a == 0）。
            let opa_out = opa_b * a0 / ONE;
            let sf = spectral_blend_factor(f32(a0) / 32768.0);
            let af = 1.0 - sf;
            var add = vec3<u32>(0u);
            if af != 0.0 {
                add = vec3<u32>(opa_b * r0 / ONE, opa_b * g0 / ONE, opa_b * b0 / ONE);
            }
            var rgb = add;
            if sf != 0.0 && a0 != 0u {
                let inv_a = 1.0 / f32(a0);
                let fa = f32(r0) * inv_a;
                let fb = f32(g0) * inv_a;
                let fc = f32(b0) * inv_a;
                let res = wgm_eraser_rgb(fa, fb, fc);
                let oo = f32(opa_out);
                rgb = vec3<u32>(
                    u32(af * f32(add.x) + sf * res.x * oo),
                    u32(af * f32(add.y) + sf * res.y * oo),
                    u32(af * f32(add.z) + sf * res.z * oo));
            }
            out_r = rgb.x;
            out_g = rgb.y;
            out_b = rgb.z;
            out_a = opa_out;
        } else if mode == 1u {
            // blend_normal_paint：color_a == 32768。
            let opacity2 = max(opacity, 150u);
            let oa = mask * opacity2 / ONE;
            let ob = ONE - oa;
            let a_out = oa + ob * a0 / ONE;
            if a0 == 0u {
                out_r = (oa * src_r + ob * r0) / ONE;
                out_g = (oa * src_g + ob * g0) / ONE;
                out_b = (oa * src_b + ob * b0) / ONE;
            } else {
                let fac_a = f32(oa) / f32(a_out);
                let fac_b = 1.0 - fac_a;
                let inv_a = 1.0 / f32(a0);
                let fa = f32(r0) * inv_a;
                let fb = f32(g0) * inv_a;
                let fc = f32(b0) * inv_a;
                let res = wgm_rgb(fa, fb, fc, fac_a, fac_b);
                let ao = f32(a_out);
                out_r = u32(res.x * ao + 0.5);
                out_g = u32(res.y * ao + 0.5);
                out_b = u32(res.z * ao + 0.5);
            }
            out_a = a_out;
        } else {
            // blend_normal_and_eraser_paint：0 < color_a < 1（涂抹笔的边缘）。
            let opa_a2 = opa_a * color_a / ONE;
            let opa_out = opa_a2 + opa_b * a0 / ONE;
            let sf = spectral_blend_factor(f32(a0) / 32768.0);
            let af = 1.0 - sf;
            var rgb = vec3<u32>(0u);
            if af != 0.0 {
                rgb = vec3<u32>(
                    (opa_a2 * src_r + opa_b * r0) / ONE,
                    (opa_a2 * src_g + opa_b * g0) / ONE,
                    (opa_a2 * src_b + opa_b * b0) / ONE);
            }
            if sf != 0.0 && a0 != 0u {
                let inv_a = 1.0 / f32(a0);
                let fa = f32(r0) * inv_a;
                let fb = f32(g0) * inv_a;
                let fc = f32(b0) * inv_a;
                let a_sum = opa_a + opa_b * a0 / ONE;
                var fac_a = f32(opa_a) / f32(a_sum);
                fac_a = fac_a * (f32(color_a) / 32768.0);
                let fac_b = 1.0 - fac_a;
                let res = wgm_rgb(fa, fb, fc, fac_a, fac_b);
                let oo = f32(opa_out);
                rgb = vec3<u32>(
                    u32(af * f32(rgb.x) + sf * res.x * oo),
                    u32(af * f32(rgb.y) + sf * res.y * oo),
                    u32(af * f32(rgb.z) + sf * res.z * oo));
            }
            out_r = rgb.x;
            out_g = rgb.y;
            out_b = rgb.z;
            out_a = opa_out;
        }

        changed = out_r != r0 || out_g != g0 || out_b != b0 || out_a != a0;
        atlas[idx] = out_r | (out_g << 16u);
        atlas[idx + 1u] = out_b | (out_a << 16u);
    }

    if changed {
        atomicOr(&wg_hit, 1u);
    }
    workgroupBarrier();
    if (lid.x == 0u && lid.y == 0u && atomicLoad(&wg_hit) != 0u) {
        atomicOr(&tile_dirty[slot], 1u);
    }
}
"#;

struct GpuBuffers {
    bind_group: wgpu::BindGroup,
    atlas: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    descs: wgpu::Buffer,
    staging: wgpu::Buffer,
    /// 每槽位「本帧是否真的改了像素」标志。
    flags: wgpu::Buffer,
    /// smudge 采样内核（绑定了 `atlas`，创建后常驻）。
    sampler: Option<crate::gpu_sample::GpuSampler>,
    /// 延迟 smudge 的 bucket 状态（与采样内核共享）。
    buckets: wgpu::Buffer,
    /// 延迟 smudge 的每 dab 上色结果（dab pass 读取）。
    smudge_results: wgpu::Buffer,
    bucket_readback: wgpu::Buffer,
    rng_readback: wgpu::Buffer,
    /// 标志的回读副本（很小，单独一份 MAP_READ）。
    flag_readback: wgpu::Buffer,
    /// 当前占用 atlas 驻留槽位的 sink；切换时 epoch 自增。
    owner: u64,
    epoch: u64,
}

pub struct GpuDabEngine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    buffers: Mutex<GpuBuffers>,
    perf: bool,
    perf_upload_ns: std::sync::atomic::AtomicU64,
    perf_submit_ns: std::sync::atomic::AtomicU64,
    perf_wait_ns: std::sync::atomic::AtomicU64,
    perf_apply_ns: std::sync::atomic::AtomicU64,
    perf_flushes: std::sync::atomic::AtomicU64,
    next_sink: std::sync::atomic::AtomicU64,
}

impl GpuDabEngine {
    /// 用 GPU 上的当前像素做一次 smudge 采样（`get_color_pigment` 的 GPU 版）。
    ///
    /// `items` 是驻留表里的 `(tile 坐标, 槽位)`：只有存在的 tile 参与采样，
    /// 缺席的按全透明处理（与 CPU 侧 `tile_lookup` 返回 `None` 一致）。
    ///
    /// 这一版会把结果回读，用于和 CPU 逐位对照；融合进 dab 链之后不再回读，
    /// 采样色直接写进 storage buffer 供下一个 dispatch 使用。
    #[allow(clippy::too_many_arguments)]
    pub fn sample_disc(
        &self,
        items: &[((i32, i32), u32)],
        cx: f32,
        cy: f32,
        radius: f32,
        paint: f32,
        rng: u32,
    ) -> Option<[f32; 4]> {
        let radius = radius.max(1.0);
        let rf = radius + 1.0;
        let ts = TILE_SIZE as i32;
        let tx0 = ((cx - rf).floor() as i32).div_euclid(ts);
        let tx1 = ((cx + rf).floor() as i32).div_euclid(ts);
        let ty0 = ((cy - rf).floor() as i32).div_euclid(ts);
        let ty1 = ((cy + rf).floor() as i32).div_euclid(ts);
        let cols = (tx1 - tx0 + 1) as u32;
        let rows = (ty1 - ty0 + 1) as u32;
        let mut dense = vec![u32::MAX; (cols as usize) * (rows as usize)];
        for &((tx, ty), slot) in items {
            if tx >= tx0 && tx <= tx1 && ty >= ty0 && ty <= ty1 {
                let idx = (ty - ty0) as usize * cols as usize + (tx - tx0) as usize;
                dense[idx] = slot;
            }
        }
        let mut bufs = self.buffers.lock().ok()?;
        let sampler = bufs.sampler.as_mut()?;
        let t = std::time::Instant::now();
        let res = sampler.sample(
            &self.device,
            &self.queue,
            &dense,
            tx0,
            ty0,
            cols,
            rows,
            cx,
            cy,
            radius,
            paint,
            rng,
        );
        crate::gpu_sample::note_sample_ns(t.elapsed().as_nanos() as u64);
        Some(res)
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Arc<Self>> {
        let limits = device.limits();
        if limits.max_storage_buffer_binding_size < ATLAS_BYTES {
            eprintln!(
                "GPU dab: storage binding {} < atlas {}",
                limits.max_storage_buffer_binding_size, ATLAS_BYTES
            );
            return None;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu dab shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu dab layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(ATLAS_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(DESC_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(FLAG_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(SMUDGE_RESULT_BYTES),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu dab pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gpu dab pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let make = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let atlas = make(
            "gpu dab atlas",
            ATLAS_BYTES,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );
        let uniforms = make(
            "gpu dab uniforms",
            UNIFORM_BYTES,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let descs = make(
            "gpu dab descs",
            DESC_BYTES,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let staging = make(
            "gpu dab staging",
            ATLAS_BYTES,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let flags = make(
            "gpu dab dirty flags",
            FLAG_BYTES,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let flag_readback = make(
            "gpu dab dirty flags readback",
            FLAG_BYTES,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let smudge_rw = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        let buckets = make("gpu smudge buckets", BUCKET_BYTES, smudge_rw);
        let smudge_results = make("gpu smudge results", SMUDGE_RESULT_BYTES, smudge_rw);
        let bucket_readback = make(
            "gpu smudge buckets readback",
            BUCKET_BYTES,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let rng_readback = make(
            "gpu smudge rng readback",
            4,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu dab bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniforms,
                        offset: 0,
                        size: std::num::NonZeroU64::new(UNIFORM_SIZE),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: atlas.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: descs.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: flags.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: smudge_results.as_entire_binding(),
                },
            ],
        });
        eprintln!(
            "gpu dab rasterizer: {} slots ({:.0} MiB), batched compute",
            ATLAS_TILES,
            ATLAS_BYTES as f64 / (1024.0 * 1024.0)
        );
        let mut buffers = GpuBuffers {
            bind_group,
            atlas,
            uniforms,
            descs,
            staging,
            flags,
            sampler: None,
            buckets,
            smudge_results,
            bucket_readback,
            rng_readback,
            flag_readback,
            owner: 0,
            epoch: 0,
        };
        buffers.sampler = Some(crate::gpu_sample::GpuSampler::new(
            device,
            &buffers.atlas,
            &buffers.buckets,
            &buffers.smudge_results,
        ));
        Some(Arc::new(Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            buffers: Mutex::new(buffers),
            perf: std::env::var_os("PIGMENT_GPU_PERF").is_some(),
            perf_upload_ns: std::sync::atomic::AtomicU64::new(0),
            perf_submit_ns: std::sync::atomic::AtomicU64::new(0),
            perf_wait_ns: std::sync::atomic::AtomicU64::new(0),
            perf_apply_ns: std::sync::atomic::AtomicU64::new(0),
            perf_flushes: std::sync::atomic::AtomicU64::new(0),
            next_sink: std::sync::atomic::AtomicU64::new(0),
        }))
    }
}

struct QueuedDab {
    dab: Dab,
    params: GpuDabParams,
    tiles: Vec<(i32, i32)>,
}

struct QueuedDeferred {
    dab: Dab,
    /// `None` for degenerate dabs that only update the smudge bucket.
    params: Option<GpuDabParams>,
    tiles: Vec<(i32, i32)>,
    req: DeferredSmudge,
    /// Seed the device bucket from `req.bucket` before updating it.
    seed: bool,
    /// Clear the whole device bucket array before this op (brush reset).
    reset_buckets: bool,
}

enum QueuedOp {
    Raster(QueuedDab),
    Deferred(QueuedDeferred),
}

struct Resident {
    slot: u32,
    rev: Option<u64>,
}

/// 一块表面上的待执行队列 + 显存驻留表。
///
/// 驻留表跨事件保留：事件结束后 CPU 与显存逐字节一致，下一次事件只上传
/// 新出现的 tile。切到别的表面时 `epoch` 变化，驻留表整份作废。
pub struct SurfaceGpuSink {
    engine: Arc<GpuDabEngine>,
    pending: Vec<QueuedOp>,
    id: u64,
    epoch: u64,
    resident: HashMap<(i32, i32), Resident>,
    /// atlas 的高水位槽位数（槽位本身没有坐标，坐标在 `resident` 里）。
    next_slot: u32,
    /// 上一次回读里「真的变了」的比例，用来决定这一帧走单段还是两段回读。
    ///
    /// 两段回读（先读脏标志、只拷脏 tile）在软笔尖上能省掉一大半 PCIe
    /// 流量，但硬笔刷/刮刀几乎块块都变，多出来的一次 submit+fence 反而
    /// 是净亏。初始按单段算。
    dirty_ratio: f32,
    /// 被淘汰后空出来的槽位，下次分配优先复用（不挤占 atlas）。
    free_slots: Vec<u32>,
    /// 设备侧 bucket 属于哪个 reset epoch。
    smudge_epoch: u64,
    /// 哪些 bucket 已在设备上初始化过（第一次使用要播种）。
    bucket_valid: [bool; 256],
    /// 下一次延迟 dab 前是否要清空整张 bucket 表。
    reset_buckets_next: bool,
    /// 设备 bucket 比 CPU 镜像新（需要 sync 才能回到 CPU 采样）。
    smudge_dirty: bool,
    /// 最近一次 flush 回读到的 bucket 状态。
    cached_buckets: Option<Vec<SmudgeBucket>>,
    cached_epoch: u64,
    /// 下一次 `flush` 最多处理多少个 op；`usize::MAX` = 全部。
    flush_limit: usize,
    /// 已提交但还没回读到 CPU 的 tile（采样 flush 只等 GPU，不回读）。
    pending_readback: std::collections::HashSet<(i32, i32)>,
    /// 本次 flush 只提交、不回读 tile。
    skip_readback: bool,
}

impl SurfaceGpuSink {
    pub fn new(engine: Arc<GpuDabEngine>) -> Self {
        let id = engine
            .next_sink
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        Self {
            engine,
            pending: Vec::new(),
            id,
            epoch: 0,
            resident: HashMap::new(),
            next_slot: 0,
            dirty_ratio: 1.0,
            free_slots: Vec::new(),
            smudge_epoch: 0,
            bucket_valid: [false; 256],
            reset_buckets_next: false,
            smudge_dirty: false,
            cached_buckets: None,
            cached_epoch: 0,
            flush_limit: usize::MAX,
            pending_readback: std::collections::HashSet::new(),
            skip_readback: false,
        }
    }

    /// 让采样 bbox 内 **CPU 有内容** 的 tile 驻留在显存。
    ///
    /// 缺席的 tile 在 GPU 采样里按全透明处理，与 `tile_lookup` 返回 `None`
    /// 完全一致，因此不再为了"补零"上传整片空白（r=500 的 bbox 有上千格）。
    /// 返回 `false` 表示显存装不下，调用方退回 CPU 采样。
    fn ensure_sample_tiles(&mut self, surface: &PigmentSurface, x: f32, y: f32, radius: f32) -> bool {
        let rf = radius.max(1.0) + 1.0;
        let ts = TILE_SIZE as i32;
        let tx0 = ((x - rf).floor() as i32).div_euclid(ts);
        let tx1 = ((x + rf).floor() as i32).div_euclid(ts);
        let ty0 = ((y - rf).floor() as i32).div_euclid(ts);
        let ty1 = ((y + rf).floor() as i32).div_euclid(ts);
        let mut missing: Vec<(i32, i32)> = Vec::new();
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                if !self.resident.contains_key(&(tx, ty)) && surface.tile(tx, ty).is_some() {
                    missing.push((tx, ty));
                }
            }
        }
        if missing.is_empty() {
            return true;
        }
        if self.resident.len() + missing.len() > ATLAS_TILES {
            return false;
        }
        let available = self.free_slots.len()
            + ATLAS_TILES.saturating_sub(self.next_slot as usize);
        if missing.len() > available {
            return false;
        }
        let engine = Arc::clone(&self.engine);
        let mut bufs = engine.buffers.lock().unwrap();
        if bufs.owner != self.id {
            bufs.owner = self.id;
            bufs.epoch += 1;
        }
        if self.epoch != bufs.epoch {
            let epoch = bufs.epoch;
            self.reset_residency(epoch);
        }
        for c in missing {
            let slot = match self.free_slots.pop() {
                Some(slot) => slot,
                None => {
                    let slot = self.next_slot;
                    self.next_slot += 1;
                    slot
                }
            };
            let rev = Self::current_rev(surface, c);
            self.resident.insert(c, Resident { slot, rev });
            let offset = (slot as usize * TILE_BYTES) as u64;
            if let Some(px) = surface.tile(c.0, c.1) {
                engine.queue.write_buffer(&bufs.atlas, offset, tile_bytes(px));
            }
        }
        true
    }

    fn flush_cpu(&mut self, surface: &mut PigmentSurface) {
        for q in self.pending.drain(..) {
            match q {
                // Deferred ops have already consumed the CPU sample RNG on
                // the device; replaying their placeholder colour is only a
                // last-resort path (atlas exhaustion), never the normal one.
                QueuedOp::Raster(q) => {
                    draw_dab_default(surface, &q.dab);
                }
                QueuedOp::Deferred(q) => {
                    draw_dab_default(surface, &q.dab);
                }
            }
        }
    }

    fn current_rev(surface: &PigmentSurface, c: (i32, i32)) -> Option<u64> {
        surface.tile_entry(c.0, c.1).map(|t| t.revision())
    }

    fn reset_residency(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.resident.clear();
        self.next_slot = 0;
        self.free_slots.clear();
    }

    /// Drop device smudge state after a fallback that could not replay it.
    fn abandon_deferred_state(&mut self) {
        self.smudge_dirty = false;
        self.cached_buckets = None;
        self.bucket_valid = [false; 256];
        self.reset_buckets_next = true;
    }
}

impl GpuDabSink for SurfaceGpuSink {
    fn sample(
        &mut self,
        surface: &PigmentSurface,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> Option<[f32; 4]> {
        // 采样范围内、**还没驻留**的 tile 先从 CPU 补传一次。
        //
        // 只上传不驻留的格子是安全的：任何排队中的 dab 都会在 `queue_dab`
        // 时把自己的 tile 变成驻留（并带回最新 revision），所以"不驻留"
        // 等价于"这里没有显存侧的新内容"，CPU 像素即最新。
        let _ = self.ensure_sample_tiles(surface, x, y, radius);

        // 采样要看到已排队的 dab，但那些像素只在显存里 —— 由内核直接读 atlas。
        let items: Vec<((i32, i32), u32)> =
            self.resident.iter().map(|(c, r)| (*c, r.slot)).collect();
        let rng = hokusai::brushmodes::color_rng_state() as u32;
        self.engine.sample_disc(&items, x, y, radius, paint, rng)
    }

    fn queue_dab(&mut self, dab: &Dab, params: &GpuDabParams, tiles: &[(i32, i32)]) -> bool {
        if tiles.is_empty() || self.pending.len() >= MAX_DABS {
            return false;
        }
        self.pending.push(QueuedOp::Raster(QueuedDab {
            dab: *dab,
            params: *params,
            tiles: tiles.to_vec(),
        }));
        true
    }

    fn defer_smudge(
        &mut self,
        surface: &PigmentSurface,
        dab: &Dab,
        params: Option<&GpuDabParams>,
        tiles: &[(i32, i32)],
        req: &DeferredSmudge,
    ) -> bool {
        if self.pending.len() >= MAX_DABS {
            return false;
        }
        let deferred_count = self
            .pending
            .iter()
            .filter(|q| matches!(q, QueuedOp::Deferred(_)))
            .count();
        if deferred_count >= MAX_DEFERRED {
            return false;
        }
        if req.bucket_index >= 256 {
            return false;
        }
        if std::env::var_os("PIGMENT_TRACE_SMUDGE").is_some() {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            if n < 8 {
                let rf = req.sample_radius.max(1.0) + 1.0;
                let ts = TILE_SIZE as i32;
                let cols = (((req.sample_x + rf).floor() as i32).div_euclid(ts)
                    - ((req.sample_x - rf).floor() as i32).div_euclid(ts)
                    + 1)
                    .max(0);
                let rows = (((req.sample_y + rf).floor() as i32).div_euclid(ts)
                    - ((req.sample_y - rf).floor() as i32).div_euclid(ts)
                    + 1)
                    .max(0);
                eprintln!(
                    "defer[{n}] sample r={:.1} bbox={}x{} tiles={} sm_amt={:.2} len={:.2} upd={}",
                    req.sample_radius,
                    cols,
                    rows,
                    cols * rows,
                    req.smudge_amt,
                    req.smudge_length,
                    req.update_active
                );
            }
        }
        // 采样内核只实现了 pigment 路径；legacy smudge 留给 CPU。
        if req.update_active && req.resample && req.sample_paint <= 0.0 {
            return false;
        }
        // 每 tile 固定 256 个展开槽位；极小半径会超过这个上界。
        if req.update_active && req.resample && req.sample_radius < 3.0 {
            return false;
        }
        if req.resample {
            let rf = req.sample_radius.max(1.0) + 1.0;
            let ts = TILE_SIZE as i32;
            let tx0 = ((req.sample_x - rf).floor() as i32).div_euclid(ts);
            let tx1 = ((req.sample_x + rf).floor() as i32).div_euclid(ts);
            let ty0 = ((req.sample_y - rf).floor() as i32).div_euclid(ts);
            let ty1 = ((req.sample_y + rf).floor() as i32).div_euclid(ts);
            let cols = (tx1 - tx0 + 1).max(0) as u32;
            let rows = (ty1 - ty0 + 1).max(0) as u32;
            if cols.saturating_mul(rows) > MAX_SAMPLE_TILES {
                return false;
            }
            if !self.ensure_sample_tiles(surface, req.sample_x, req.sample_y, req.sample_radius) {
                return false;
            }
        }
        let mut reset_buckets = self.reset_buckets_next;
        if req.reset_epoch != self.smudge_epoch {
            self.smudge_epoch = req.reset_epoch;
            self.bucket_valid = [false; 256];
            self.reset_buckets_next = false;
            reset_buckets = true;
        }
        let seed = !self.bucket_valid[req.bucket_index as usize];
        self.bucket_valid[req.bucket_index as usize] = true;
        self.smudge_dirty = true;
        self.pending.push(QueuedOp::Deferred(QueuedDeferred {
            dab: *dab,
            params: params.copied(),
            tiles: tiles.to_vec(),
            req: *req,
            seed,
            reset_buckets,
        }));
        true
    }

    fn deferred_smudge_dirty(&self) -> bool {
        self.smudge_dirty
    }

    fn pending_ops(&self) -> usize {
        self.pending.len()
    }

    fn sync_deferred_smudge(
        &mut self,
        surface: &mut PigmentSurface,
        reset_epoch: u64,
        buckets: &mut [SmudgeBucket],
    ) {
        self.flush(surface);
        if reset_epoch != self.smudge_epoch {
            self.smudge_epoch = reset_epoch;
            self.bucket_valid = [false; 256];
            self.reset_buckets_next = true;
            self.smudge_dirty = false;
            return;
        }
        if self.smudge_dirty {
            if let Some(cached) = &self.cached_buckets {
                if self.cached_epoch == reset_epoch {
                    let n = buckets.len().min(cached.len());
                    buckets[..n].copy_from_slice(&cached[..n]);
                }
            }
            self.smudge_dirty = false;
        }
    }

    fn flush_for_sample(&mut self, surface: &mut PigmentSurface) {
        self.flush_limit = usize::MAX;
        self.skip_readback = true;
        self.flush(surface);
    }

    fn flush_budgeted(&mut self, surface: &mut PigmentSurface) {
        let mut any_deferred = false;
        let mut max_sample_radius = 0.0f32;
        for q in &self.pending {
            if let QueuedOp::Deferred(q) = q {
                any_deferred = true;
                max_sample_radius = max_sample_radius.max(q.req.sample_radius);
            }
        }
        self.flush_limit = if any_deferred {
            if max_sample_radius < 24.0 {
                small_deferred_ops_budget()
            } else {
                flush_ops_budget()
            }
        } else {
            normal_ops_budget()
        };
        self.flush(surface);
    }

    fn flush(&mut self, surface: &mut PigmentSurface) {
        let limit = std::mem::replace(&mut self.flush_limit, usize::MAX);
        let skip_readback = std::mem::replace(&mut self.skip_readback, false);
        if self.pending.is_empty() && self.pending_readback.is_empty() {
            return;
        }
        // 只处理队列前缀；后面的 dab 留在 pending 里，下一帧继续。
        // 这样每个事件的 dab 顺序、采样依赖都不变，只是分帧落地。
        let mut rest = if self.pending.len() > limit {
            self.pending.split_off(limit)
        } else {
            Vec::new()
        };
        let has_deferred = self
            .pending
            .iter()
            .any(|q| matches!(q, QueuedOp::Deferred(_)));
        let mut desc_total = 0usize;
        let mut deferred_total = 0usize;
        // 需要上传/驻留：光栅 tile + 采样 bbox 里"有内容"的 tile。
        let mut needed: Vec<(i32, i32)> = Vec::new();
        let mut needed_set: std::collections::HashSet<(i32, i32)> =
            std::collections::HashSet::new();
        // 真正会被 GPU 改动、需要回读的 tile。
        let mut raster_tiles: Vec<(i32, i32)> = Vec::new();
        let mut raster_set: std::collections::HashSet<(i32, i32)> =
            std::collections::HashSet::new();
        for op in &self.pending {
            match op {
                QueuedOp::Raster(q) => {
                    desc_total += q.tiles.len();
                    for &c in &q.tiles {
                        if needed_set.insert(c) {
                            needed.push(c);
                        }
                        if raster_set.insert(c) {
                            raster_tiles.push(c);
                        }
                    }
                }
                QueuedOp::Deferred(q) => {
                    deferred_total += 1;
                    if q.params.is_some() {
                        desc_total += q.tiles.len();
                    }
                    for &c in &q.tiles {
                        if needed_set.insert(c) {
                            needed.push(c);
                        }
                        if q.params.is_some() && raster_set.insert(c) {
                            raster_tiles.push(c);
                        }
                    }
                    // 采样 bbox：只在显存或 CPU 里真的有这块 tile 时才纳入
                    // 驻留集合；缺席的格子由采样 kernel 按透明处理。
                    if q.req.resample {
                        let rf = q.req.sample_radius.max(1.0) + 1.0;
                        let ts = TILE_SIZE as i32;
                        let tx0 = ((q.req.sample_x - rf).floor() as i32).div_euclid(ts);
                        let tx1 = ((q.req.sample_x + rf).floor() as i32).div_euclid(ts);
                        let ty0 = ((q.req.sample_y - rf).floor() as i32).div_euclid(ts);
                        let ty1 = ((q.req.sample_y + rf).floor() as i32).div_euclid(ts);
                        for ty in ty0..=ty1 {
                            for tx in tx0..=tx1 {
                                let c = (tx, ty);
                                if (self.resident.contains_key(&c) || surface.tile(tx, ty).is_some())
                                    && needed_set.insert(c)
                                {
                                    needed.push(c);
                                }
                            }
                        }
                    }
                }
            }
        }
        let batch_raster_tiles = raster_tiles.clone();
        // 采样 flush 只提交不回读；这些 tile 必须在下一次完整 flush 时补读。
        for &c in &self.pending_readback {
            if !self.resident.contains_key(&c) {
                continue;
            }
            if needed_set.insert(c) {
                needed.push(c);
            }
            if raster_set.insert(c) {
                raster_tiles.push(c);
            }
        }
        if desc_total > DESC_CAPACITY || needed.len() > ATLAS_TILES {
            if has_deferred {
                self.abandon_deferred_state();
            }
            self.flush_cpu(surface);
            self.pending.append(&mut rest);
            return;
        }

        let engine = Arc::clone(&self.engine);
        let mut bufs = engine.buffers.lock().unwrap();
        if bufs.owner != self.id {
            bufs.owner = self.id;
            bufs.epoch += 1;
        }
        if self.epoch != bufs.epoch {
            let epoch = bufs.epoch;
            self.reset_residency(epoch);
        }

        // ---- 找出需要上传的 tile；必要时清空驻留表 ----
        let mut uploads: Vec<((i32, i32), Option<u64>)> = Vec::new();
        for &c in &needed {
            let rev = Self::current_rev(surface, c);
            if matches!(self.resident.get(&c), Some(r) if r.rev == rev) {
                continue;
            }
            // 纯采样 tile 在 CPU 里没有内容时保持缺席：绝不为了补零而上传
            // （大半径 bbox 有上千格，会白烧几十 MiB 带宽）。
            if !raster_set.contains(&c) && surface.tile(c.0, c.1).is_none() {
                continue;
            }
            uploads.push((c, rev));
        }
        // atlas 装不下：只淘汰**本事件用不到**的旧槽位，而不是整表作废。
        if self.resident.len() + uploads.len() > ATLAS_TILES {
            let mut victims: Vec<(i32, i32)> = self
                .resident
                .keys()
                .copied()
                .filter(|c| !needed_set.contains(c))
                .collect();
            let over = self.resident.len() + uploads.len() - ATLAS_TILES;
            if victims.len() >= over {
                victims.sort_unstable();
                victims.truncate(over);
                for c in victims {
                    if let Some(r) = self.resident.remove(&c) {
                        self.free_slots.push(r.slot);
                    }
                }
            } else {
                let epoch = bufs.epoch;
                self.reset_residency(epoch);
                uploads = needed
                    .iter()
                    .filter(|&&c| raster_set.contains(&c) || surface.tile(c.0, c.1).is_some())
                    .map(|&c| (c, Self::current_rev(surface, c)))
                    .collect();
            }
        }

        // Defensive: never hand out a slot past the atlas end.  The eviction
        // logic above normally guarantees this, but a stale resident table
        // (e.g. after a surface switch) must not turn into an OOB upload.
        let available =
            self.free_slots.len() + ATLAS_TILES.saturating_sub(self.next_slot as usize);
        if uploads.len() > available {
            let epoch = bufs.epoch;
            self.reset_residency(epoch);
            uploads = needed
                .iter()
                .filter(|&&c| raster_set.contains(&c) || surface.tile(c.0, c.1).is_some())
                .map(|&c| (c, Self::current_rev(surface, c)))
                .collect();
            if uploads.len() > ATLAS_TILES {
                drop(bufs);
                self.flush_cpu(surface);
                self.pending.append(&mut rest);
                return;
            }
        }

        let t_upload = std::time::Instant::now();
        // ---- 分配槽位并上传 ----
        let mut zero = vec![0u8; TILE_BYTES];
        for &(c, rev) in &uploads {
            let slot = match self.free_slots.pop() {
                Some(slot) => slot,
                None => {
                    let slot = self.next_slot;
                    self.next_slot += 1;
                    slot
                }
            };
            self.resident.insert(c, Resident { slot, rev });
            let offset = (slot as usize * TILE_BYTES) as u64;
            match surface.tile(c.0, c.1) {
                Some(px) => self
                    .engine
                    .queue
                    .write_buffer(&bufs.atlas, offset, tile_bytes(px)),
                None => self.engine.queue.write_buffer(&bufs.atlas, offset, &zero),
            }
        }
        zero.clear();

        // ---- 每个 op 的参数 + tile 描述符 ----
        let mut descs: Vec<u32> = Vec::with_capacity(desc_total * 4);
        let mut uniform_bytes = vec![0u8; self.pending.len() * UNIFORM_STRIDE as usize];
        let mut result_index_of = vec![SMUDGE_NONE; self.pending.len()];
        for (i, op) in self.pending.iter().enumerate() {
            let (params, tiles, deferred) = match op {
                QueuedOp::Raster(q) => (Some(&q.params), q.tiles.as_slice(), false),
                QueuedOp::Deferred(q) => (q.params.as_ref(), q.tiles.as_slice(), true),
            };
            let desc_off = descs.len() / 4;
            if params.is_some() {
                for &c in tiles {
                    let slot = self.resident[&c].slot;
                    descs.push(slot * PIXEL_U32 as u32);
                    // 世界坐标可为负：传 f32 的位模式，着色器再 bitcast 回来。
                    descs.push(((c.0 * TILE_SIZE as i32) as f32).to_bits());
                    descs.push(((c.1 * TILE_SIZE as i32) as f32).to_bits());
                    // 槽位号：着色器用它写「这块 tile 本帧变了」的标志。
                    descs.push(slot);
                }
            }
            let u = &mut uniform_bytes
                [i * UNIFORM_STRIDE as usize..(i + 1) * UNIFORM_STRIDE as usize];
            let mut f = |at: usize, v: f32| {
                u[at..at + 4].copy_from_slice(&v.to_ne_bytes());
            };
            if let Some(p) = params {
                f(0, p.x);
                f(4, p.y);
                f(8, p.r_fringe);
                f(12, p.aspect);
                f(16, p.cs);
                f(20, p.sn);
                f(24, p.inv_r2);
                f(28, p.hardness);
                f(32, p.softness);
                f(36, p.opacity as f32);
                f(40, p.color_a as f32);
                f(48, p.src_r as f32);
                f(52, p.src_g as f32);
                f(56, p.src_b as f32);
                for k in 0..10 {
                    f(64 + k * 4, p.spec_a_log[k]);
                    f(112 + k * 4, p.spec_a_zero_pow[k]);
                }
                let mode = if p.color_a == 0 {
                    0.0
                } else if p.color_a == 32768 {
                    1.0
                } else {
                    2.0
                };
                f(168, mode);
            }
            f(160, desc_off as f32);
            f(164, tiles.len() as f32);
            let _ = deferred;
            // Smudge result index is assigned below; fill it after the loop.
        }

        // ---- 延迟 smudge：采样参数、槽位表与结果索引 ----
        let mut sample_bytes =
            vec![0u8; (deferred_total + 1) * SAMPLE_PARAM_STRIDE as usize];
        // slot 0 留给独立 `sample()` 调试入口。
        let mut slot_data: Vec<u32> = vec![u32::MAX; MAX_SAMPLE_TILES as usize];
        let cpu_rng = hokusai::brushmodes::color_rng_state() as u32;
        let mut sample_seen = false;
        let mut any_sample = false;
        let mut max_bands = 1u32;
        let mut max_samples = 1u32;
        let mut deferred_ord = 0usize;
        for (i, op) in self.pending.iter().enumerate() {
            let QueuedOp::Deferred(q) = op else {
                continue;
            };
            deferred_ord += 1;
            result_index_of[i] = deferred_ord as u32;
            let req = &q.req;
            let will_sample = req.update_active && req.resample;
            let (tx0, ty0, cols, rows, slot_base, radius, cap, bands) = if will_sample {
                let rf = req.sample_radius.max(1.0) + 1.0;
                let ts = TILE_SIZE as i32;
                let tx0 = ((req.sample_x - rf).floor() as i32).div_euclid(ts);
                let tx1 = ((req.sample_x + rf).floor() as i32).div_euclid(ts);
                let ty0 = ((req.sample_y - rf).floor() as i32).div_euclid(ts);
                let ty1 = ((req.sample_y + rf).floor() as i32).div_euclid(ts);
                let cols = (tx1 - tx0 + 1).max(0) as u32;
                let rows = (ty1 - ty0 + 1).max(0) as u32;
                let slot_base = slot_data.len() as u32;
                for ty in ty0..=ty1 {
                    for tx in tx0..=tx1 {
                        slot_data.push(
                            self.resident
                                .get(&(tx, ty))
                                .map(|r| r.slot)
                                .unwrap_or(u32::MAX),
                        );
                    }
                }
                let radius = req.sample_radius.max(1.0);
                let interval = if radius <= 2.0 {
                    1u32
                } else {
                    (radius * 7.0) as u32
                };
                let cap = if interval <= 1 {
                    BAND_ROWS * 64
                } else {
                    (BAND_ROWS * 64) / interval + 64
                };
                let bands = band_count(cols, rows);
                max_bands = max_bands.max(bands);
                max_samples = max_samples.max(bands * cap);
                (tx0, ty0, cols, rows, slot_base, radius, cap, bands)
            } else {
                (0, 0, 1, 1, 0, 0.0, 0, 1)
            };
            let _ = bands;
            let chain_rng = if will_sample {
                let seen = sample_seen;
                sample_seen = true;
                any_sample = true;
                seen
            } else {
                false
            };
            let ds = DeferredSample {
                cx: req.sample_x,
                cy: req.sample_y,
                radius,
                paint: req.sample_paint,
                tx0,
                ty0,
                cols,
                rows,
                cap,
                chain_rng,
                has_sample: req.update_active && req.resample,
                rng_start: cpu_rng,
                slot_base,
                smudge_amt: req.smudge_amt,
                smudge_length: req.smudge_length,
                smudge_length_log: req.smudge_length_log,
                smudge_radius: req.sample_radius,
                paint_mode: req.paint_mode,
                smudge_op_lim: req.smudge_op_lim,
                eraser: req.eraser,
                mapping_active: true,
                base_color: req.base_color,
                sample_paint: req.sample_paint,
                bucket_index: req.bucket_index,
                seed_bucket: q.seed,
                run_smudge: true,
                result_index: deferred_ord as u32,
                bucket_color: req.bucket.color,
                bucket_prev: req.bucket.prev,
                recentness: req.bucket.recentness,
            };
            let off = deferred_ord * SAMPLE_PARAM_STRIDE as usize;
            sample_bytes[off..off + 176].copy_from_slice(&ds.bytes());
        }

        // 回填 deferred 的 smudge_idx（结果索引）与 flag。
        for (i, op) in self.pending.iter().enumerate() {
            if !matches!(op, QueuedOp::Deferred(_)) {
                continue;
            }
            let u = &mut uniform_bytes
                [i * UNIFORM_STRIDE as usize..(i + 1) * UNIFORM_STRIDE as usize];
            let idx = result_index_of[i].to_ne_bytes();
            u[176..180].copy_from_slice(&idx);
            u[180..184].copy_from_slice(&1u32.to_ne_bytes());
        }
        for (i, op) in self.pending.iter().enumerate() {
            if matches!(op, QueuedOp::Deferred(_)) {
                continue;
            }
            let u = &mut uniform_bytes
                [i * UNIFORM_STRIDE as usize..(i + 1) * UNIFORM_STRIDE as usize];
            u[176..180].copy_from_slice(&SMUDGE_NONE.to_ne_bytes());
        }

        if !descs.is_empty() {
            self.engine
                .queue
                .write_buffer(&bufs.descs, 0, bytemuck_u32(&descs));
        }
        if !uniform_bytes.is_empty() {
            self.engine
                .queue
                .write_buffer(&bufs.uniforms, 0, &uniform_bytes);
        }
        if deferred_total > 0 {
            let sampler = bufs.sampler.as_mut().expect("sampler");
            sampler.ensure_capacity(&self.engine.device, max_bands, max_samples);
            sampler.write_params(&self.engine.queue, 0, &sample_bytes);
            self.engine
                .queue
                .write_buffer(sampler.slots_buffer(), 0, bytemuck_u32(&slot_data));
        }

        if self.engine.perf {
            self.engine.perf_upload_ns.fetch_add(
                t_upload.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        // 清空脏标志（全表只有 8 KiB，直接写）。
        let flag_zeros = vec![0u8; FLAG_BYTES as usize];
        self.engine
            .queue
            .write_buffer(&bufs.flags, 0, &flag_zeros);

        // 脏比例高的笔刷（硬笔尖、刮刀）走单段：多一次同步是净亏。
        let two_phase =
            !raster_tiles.is_empty() && self.pending_readback.is_empty() && self.dirty_ratio < 0.75;

        // ---- 按队列顺序编码：光栅 / 采样+smudge+光栅 ----
        let t_submit = std::time::Instant::now();
        let mut encoder = self
            .engine
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu dab"),
            });
        {
            let sampler = bufs.sampler.as_ref().expect("sampler");
            for (i, op) in self.pending.iter().enumerate() {
                match op {
                    QueuedOp::Raster(q) => {
                        if q.tiles.is_empty() {
                            continue;
                        }
                        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some("gpu dab pass"),
                            timestamp_writes: None,
                        });
                        pass.set_pipeline(&self.engine.pipeline);
                        pass.set_bind_group(
                            0,
                            &bufs.bind_group,
                            &[(i as u64 * UNIFORM_STRIDE) as u32],
                        );
                        pass.dispatch_workgroups(q.tiles.len() as u32 * 4, 4, 1);
                    }
                    QueuedOp::Deferred(q) => {
                        let param_offset = result_index_of[i] as u64 * SAMPLE_PARAM_STRIDE;
                        if q.reset_buckets {
                            sampler.encode_reset_buckets(&mut encoder, param_offset);
                        }
                        if q.req.update_active
                            && q.req.resample
                            && std::env::var_os("PIGMENT_NO_GPU_SAMPLE").is_none()
                        {
                            let rf = q.req.sample_radius.max(1.0) + 1.0;
                            let ts = TILE_SIZE as i32;
                            let cols = (((q.req.sample_x + rf).floor() as i32).div_euclid(ts)
                                - ((q.req.sample_x - rf).floor() as i32).div_euclid(ts)
                                + 1)
                                .max(0) as u32;
                            let rows = (((q.req.sample_y + rf).floor() as i32).div_euclid(ts)
                                - ((q.req.sample_y - rf).floor() as i32).div_euclid(ts)
                                + 1)
                                .max(0) as u32;
                            if std::env::var_os("PIGMENT_TRACE_SMUDGE").is_some() {
                                use std::sync::atomic::{AtomicU64, Ordering};
                                static N: AtomicU64 = AtomicU64::new(0);
                                let n = N.fetch_add(1, Ordering::Relaxed);
                                if n < 8 {
                                    eprintln!(
                                        "encode sample[{n}] bands={} bbox={}x{} idx={}",
                                        band_count(cols, rows),
                                        cols,
                                        rows,
                                        result_index_of[i]
                                    );
                                }
                            }
                            sampler.encode_sample(
                                &mut encoder,
                                band_count(cols, rows),
                                param_offset,
                            );
                        }
                        if std::env::var_os("PIGMENT_NO_SMUDGE_PASS").is_none() {
                            sampler.encode_smudge(&mut encoder, param_offset);
                        }
                        if q.params.is_some()
                            && !q.tiles.is_empty()
                            && std::env::var_os("PIGMENT_NO_GPU_RASTER").is_none()
                        {
                            let mut pass =
                                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                                    label: Some("gpu dab pass"),
                                    timestamp_writes: None,
                                });
                            pass.set_pipeline(&self.engine.pipeline);
                            pass.set_bind_group(
                                0,
                                &bufs.bind_group,
                                &[(i as u64 * UNIFORM_STRIDE) as u32],
                            );
                            pass.dispatch_workgroups(q.tiles.len() as u32 * 4, 4, 1);
                        }
                    }
                }
            }
        }
        if !skip_readback {
            if two_phase {
                // 只回读「哪些 tile 变了」这张小表（8 KiB），tile 本体等看到标志再拷。
                encoder.copy_buffer_to_buffer(&bufs.flags, 0, &bufs.flag_readback, 0, FLAG_BYTES);
            } else {
                // 槽位跨事件保留，按 tile 单独拷到紧凑 staging。
                for (i, &c) in raster_tiles.iter().enumerate() {
                    let slot = self.resident[&c].slot;
                    encoder.copy_buffer_to_buffer(
                        &bufs.atlas,
                        (slot as usize * TILE_BYTES) as u64,
                        &bufs.staging,
                        (i * TILE_BYTES) as u64,
                        TILE_BYTES as u64,
                    );
                }
            }
            if deferred_total > 0 {
                encoder.copy_buffer_to_buffer(
                    &bufs.buckets,
                    0,
                    &bufs.bucket_readback,
                    0,
                    BUCKET_BYTES,
                );
                if any_sample {
                    let state_buf =
                        bufs.sampler.as_ref().expect("sampler").state_buffer();
                    encoder.copy_buffer_to_buffer(state_buf, 16, &bufs.rng_readback, 0, 4);
                }
            }
        }
        self.engine.queue.submit(Some(encoder.finish()));
        if skip_readback {
            // GPU 采样会随后提交并等待，队列顺序保证它看到这批 dab；
            // tile 回读留到帧末一次完成。
            for c in batch_raster_tiles {
                self.pending_readback.insert(c);
            }
            self.pending.clear();
            self.pending.append(&mut rest);
            return;
        }
        let t_wait = std::time::Instant::now();

        // 先挂上状态回读的 map，后面的 poll 会一并完成。
        let bucket_map = (deferred_total > 0).then(|| bufs.bucket_readback.slice(..BUCKET_BYTES));
        if let Some(slice) = &bucket_map {
            slice.map_async(wgpu::MapMode::Read, |_| {});
        }
        let rng_map = any_sample.then(|| bufs.rng_readback.slice(..4));
        if let Some(slice) = &rng_map {
            slice.map_async(wgpu::MapMode::Read, |_| {});
        }

        // 第一次 GPU 同步完成的时刻（诊断用：区分"等 GPU"与"CPU 落地"）。
        let mut t_first_sync: Option<std::time::Instant> = None;
        let copy_list: Vec<(i32, i32)> = if two_phase {
            // ---- 第二阶段：等 compute 完成，读脏标志 ----
            let flag_slice = bufs.flag_readback.slice(0..FLAG_BYTES);
            flag_slice.map_async(wgpu::MapMode::Read, |_| {});
            self.engine
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .ok();
            let mut dirty_slots: std::collections::HashSet<u32> = std::collections::HashSet::new();
            if let Ok(data) = flag_slice.get_mapped_range() {
                for (i, chunk) in data.chunks_exact(4).enumerate() {
                    if chunk != [0u8; 4] {
                        dirty_slots.insert(i as u32);
                    }
                }
            }
            bufs.flag_readback.unmap();
            t_first_sync = Some(std::time::Instant::now());
            let list: Vec<(i32, i32)> = raster_tiles
                .iter()
                .copied()
                .filter(|c| {
                    self.resident
                        .get(c)
                        .is_some_and(|r| dirty_slots.contains(&r.slot))
                })
                .collect();
            if !list.is_empty() {
                let mut enc2 = self
                    .engine
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("gpu dab readback"),
                    });
                for (i, &c) in list.iter().enumerate() {
                    let slot = self.resident[&c].slot;
                    enc2.copy_buffer_to_buffer(
                        &bufs.atlas,
                        (slot as usize * TILE_BYTES) as u64,
                        &bufs.staging,
                        (i * TILE_BYTES) as u64,
                        TILE_BYTES as u64,
                    );
                }
                self.engine.queue.submit(Some(enc2.finish()));
            }
            list
        } else {
            raster_tiles.clone()
        };

        // ---- 统一回读：staging → 表面 ----
        let t_apply = std::time::Instant::now();
        let mut written_tiles = 0usize;
        let mut batch: Vec<((i32, i32), &[u8])> = Vec::with_capacity(copy_list.len());
        if !copy_list.is_empty() {
            let used_bytes = (copy_list.len() * TILE_BYTES) as u64;
            let slice = bufs.staging.slice(0..used_bytes);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            self.engine
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .ok();
            if t_first_sync.is_none() {
                t_first_sync = Some(std::time::Instant::now());
            }
            let zero = vec![0u8; TILE_BYTES];
            if let Ok(data) = slice.get_mapped_range() {
                for (i, &c) in copy_list.iter().enumerate() {
                    let s = i * TILE_BYTES;
                    let bytes = &data[s..s + TILE_BYTES];
                    // 标志只说明"可能变了"（后一个 dab 可能又改回原值），
                    // 逐字节再比一次，落地量与像素语义都保持原样。
                    let same = surface
                        .tile(c.0, c.1)
                        .is_some_and(|px| tile_bytes(px) == bytes);
                    if same {
                        continue;
                    }
                    if surface.tile(c.0, c.1).is_none() && bytes == &zero[..] {
                        continue;
                    }
                    batch.push((c, bytes));
                }
                written_tiles = batch.len();
                pigment_surface::perf::add(
                    &pigment_surface::perf::FLUSH_TILES,
                    written_tiles as u64,
                );
                // 用"真正落地了多少块"更新比例：单段路径下 `copy_list`
                // 恒等于 `raster_tiles`，拿它算会让比例永远停在 1.0。
                self.dirty_ratio =
                    written_tiles as f32 / raster_tiles.len().max(1) as f32;
                surface.write_gpu_tiles(&batch);
                for &(c, _) in &batch {
                    let rev = Self::current_rev(surface, c);
                    if let Some(r) = self.resident.get_mut(&c) {
                        r.rev = rev;
                    }
                }
            }
            bufs.staging.unmap();
        } else if deferred_total > 0 {
            self.engine
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .ok();
        }

        // ---- 设备 smudge 状态回读 ----
        if let Some(slice) = &bucket_map {
            if let Ok(data) = slice.get_mapped_range() {
                let mut buckets = vec![SmudgeBucket::default(); 256];
                for (i, b) in buckets.iter_mut().enumerate() {
                    let base = i * 48;
                    let rd = |at: usize| {
                        let mut v = [0u8; 4];
                        v.copy_from_slice(&data[base + at..base + at + 4]);
                        f32::from_ne_bytes(v)
                    };
                    b.color = [rd(0), rd(4), rd(8), rd(12)];
                    b.prev = [rd(16), rd(20), rd(24), rd(28)];
                    b.recentness = rd(32);
                }
                self.cached_buckets = Some(buckets);
                self.cached_epoch = self.smudge_epoch;
            }
            bufs.bucket_readback.unmap();
        }
        if let Some(slice) = &rng_map {
            if let Ok(data) = slice.get_mapped_range() {
                let mut v = [0u8; 4];
                v.copy_from_slice(&data[..4]);
                hokusai::brushmodes::set_color_rng_state(u32::from_ne_bytes(v) as u64);
            }
            bufs.rng_readback.unmap();
        }

        let _ = t_wait;
        if self.engine.perf {
            use std::sync::atomic::Ordering;
            self.engine.perf_submit_ns.fetch_add(
                t_wait.duration_since(t_submit).as_nanos() as u64,
                Ordering::Relaxed,
            );
            let sync_at = t_first_sync.unwrap_or(t_apply);
            let wait_ns = sync_at.duration_since(t_wait).as_nanos() as u64;
            let apply_ns = t_apply.elapsed().as_nanos() as u64;
            self.engine.perf_wait_ns.fetch_add(wait_ns, Ordering::Relaxed);
            self.engine.perf_apply_ns.fetch_add(apply_ns, Ordering::Relaxed);
            let n = self.engine.perf_flushes.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 5 == 0 {
                let f = n as f64;
                eprintln!(
                    "gpu-flush n={n} tiles={} upload_tiles={} dabs={} deferred={} written={} upload={:.2}ms submit={:.2}ms wait={:.2}ms apply={:.2}ms",
                    raster_tiles.len(),
                    uploads.len(),
                    self.pending.len(),
                    deferred_total,
                    written_tiles,
                    self.engine.perf_upload_ns.load(Ordering::Relaxed) as f64 / f / 1e6,
                    self.engine.perf_submit_ns.load(Ordering::Relaxed) as f64 / f / 1e6,
                    self.engine.perf_wait_ns.load(Ordering::Relaxed) as f64 / f / 1e6,
                    self.engine.perf_apply_ns.load(Ordering::Relaxed) as f64 / f / 1e6,
                );
            }
        }
        self.pending.clear();
        self.pending.append(&mut rest);
        self.pending_readback.clear();
    }
}

fn bytemuck_u32(v: &[u32]) -> &[u8] {
    // SAFETY: u32 → u8 的对齐只会变宽到 1，长度按 4 倍计算。
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), v.len() * 4) }
}
