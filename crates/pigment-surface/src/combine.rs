//! 图层混合/合成模式 —— 逐位移植 `_mypaint_ref/lib/{pixops,blending,compositing,fix15}.hpp`
//! 与 `fastapprox/fast{pow,log,exp}.h`。
//!
//! | 参考源 | 本文件 |
//! |---|---|
//! | `pixops.hpp:93-117` `enum CombineMode`（22 值） | [`CombineMode`] |
//! | `fix15.hpp` 全部助手 | 本文件底部 `fix15` 模块 |
//! | `blending.hpp` 12 可分离 + 4 不可分离 BlendFunc | `blend_*` 函数 |
//! | `compositing.hpp` 7 个 CompositeFunc | `composite_*` 函数 |
//! | `blending.hpp:124` BlendNormal×SpectralWGM 特化（WGM 光谱混色） | [`tile_combine`] 内 `wgm` 分支 |
//! | `pixops.cpp:784-811` `combine_mode_info` LUT | [`tile_combine`] 的模式分派 |
//! | `lib/modes.py` 常量与 UI 字符串 | [`PASS_THROUGH_MODE`] / [`DEFAULT_MODE`] / `label()` 等 |
//!
//! 硬门禁：`tests/combine.rs` 读 `tests/fixtures/combine_vectors.bin`（由
//! `.workbuddy/tools/gen-combine-vectors.cpp` 直接编译参考头文件生成）逐位比对
//! 22 模式 × dst_has_alpha × 5 档不透明度 = 220 个 case。
//!
//! fastpow 注记：`fastpow2` 里 `cast_uint32_t(f)` 是 C 的 float→uint32 转换，
//! 参数为负时是 UB（x86 上得到 0x80000000 位型）；Rust 用 `as u32`（截断+饱和）。
//! WGM 路径可达的参数范围（p ∈ (-127, 0.05)，经金标准 fixture 实测覆盖）内
//! 两者一致；p < -126 的窄区（clipp 钳到 -126 后内层式仍为正）已被 fixture 证明无分歧。

use hokusai::tile::TILE_SIZE;

/// 单 tile 的 u16 通道数（64×64×4）。
pub const TILE_CHANNELS: usize = TILE_SIZE * TILE_SIZE * 4;

/// 组（子栈）专用的直通模式（`lib/modes.py:16`；不落盘，只作内部标记）。
pub const PASS_THROUGH_MODE: i16 = -1;

/// 默认图层模式（`lib/modes.py:28`：`_DEFAULT_MODE = CombineSpectralWGM`）。
pub const DEFAULT_MODE: CombineMode = CombineMode::SpectralWGM;

/// `lib/modes.py:20`：普通图层的合法模式 = 0..22 全部。
pub const STANDARD_MODES: [CombineMode; 22] = [
    CombineMode::Normal,
    CombineMode::Multiply,
    CombineMode::Screen,
    CombineMode::Overlay,
    CombineMode::Darken,
    CombineMode::Lighten,
    CombineMode::HardLight,
    CombineMode::SoftLight,
    CombineMode::ColorBurn,
    CombineMode::ColorDodge,
    CombineMode::Difference,
    CombineMode::Exclusion,
    CombineMode::Hue,
    CombineMode::Saturation,
    CombineMode::Color,
    CombineMode::Luminosity,
    CombineMode::Lighter,
    CombineMode::DestinationIn,
    CombineMode::DestinationOut,
    CombineMode::SourceAtop,
    CombineMode::DestinationAtop,
    CombineMode::SpectralWGM,
];

/// 图层混合/合成模式（`pixops.hpp:93-117`，索引顺序 = 源码枚举顺序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CombineMode {
    Normal = 0,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    HardLight,
    SoftLight,
    ColorBurn,
    ColorDodge,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    /// W3C "Lighter" / Porter-Duff "Plus"（`compositing.hpp:370`）。
    Lighter,
    DestinationIn,
    DestinationOut,
    SourceAtop,
    DestinationAtop,
    /// 光谱 WGM 颜料混色（MyPaint 私有，`mypaint:spectral-wgm`）。
    SpectralWGM,
}

impl CombineMode {
    /// 全部 22 个模式，按源码枚举顺序。
    pub const ALL: [CombineMode; 22] = STANDARD_MODES;

    pub fn from_index(i: u8) -> Option<CombineMode> {
        Self::ALL.get(i as usize).copied()
    }

    /// 保存进 `.ora` 的规范名（`pixops.cpp:787-810` 的 `name` 字段）。
    pub fn ora_name(self) -> &'static str {
        match self {
            CombineMode::Normal => "svg:src-over",
            CombineMode::Multiply => "svg:multiply",
            CombineMode::Screen => "svg:screen",
            CombineMode::Overlay => "svg:overlay",
            CombineMode::Darken => "svg:darken",
            CombineMode::Lighten => "svg:lighten",
            CombineMode::HardLight => "svg:hard-light",
            CombineMode::SoftLight => "svg:soft-light",
            CombineMode::ColorBurn => "svg:color-burn",
            CombineMode::ColorDodge => "svg:color-dodge",
            CombineMode::Difference => "svg:difference",
            CombineMode::Exclusion => "svg:exclusion",
            CombineMode::Hue => "svg:hue",
            CombineMode::Saturation => "svg:saturation",
            CombineMode::Color => "svg:color",
            CombineMode::Luminosity => "svg:luminosity",
            CombineMode::Lighter => "svg:plus",
            CombineMode::DestinationIn => "svg:dst-in",
            CombineMode::DestinationOut => "svg:dst-out",
            CombineMode::SourceAtop => "svg:src-atop",
            CombineMode::DestinationAtop => "svg:dst-atop",
            CombineMode::SpectralWGM => "mypaint:spectral-wgm",
        }
    }

    /// 载入 `.ora` 时按 `composite-op` 名查模式（`lib/modes.py:190`）。
    pub fn from_ora_name(name: &str) -> Option<CombineMode> {
        Self::ALL.iter().copied().find(|m| m.ora_name() == name)
    }

    /// UI 标签（`lib/modes.py:42-184` 的 `MODE_STRINGS`，MyPaint 官方英文文案）。
    pub fn label(self) -> &'static str {
        match self {
            CombineMode::Normal => "Normal",
            CombineMode::Multiply => "Multiply",
            CombineMode::Screen => "Screen",
            CombineMode::Overlay => "Overlay",
            CombineMode::Darken => "Darken",
            CombineMode::Lighten => "Lighten",
            CombineMode::HardLight => "Hard Light",
            CombineMode::SoftLight => "Soft Light",
            CombineMode::ColorBurn => "Burn",
            CombineMode::ColorDodge => "Dodge",
            CombineMode::Difference => "Difference",
            CombineMode::Exclusion => "Exclusion",
            CombineMode::Hue => "Hue",
            CombineMode::Saturation => "Saturation",
            CombineMode::Color => "Color",
            CombineMode::Luminosity => "Luminosity",
            CombineMode::Lighter => "Plus",
            CombineMode::DestinationIn => "Destination In",
            CombineMode::DestinationOut => "Destination Out",
            CombineMode::SourceAtop => "Source Atop",
            CombineMode::DestinationAtop => "Destination Atop",
            CombineMode::SpectralWGM => "Pigment",
        }
    }

    /// UI 提示（同 `MODE_STRINGS` 第二元组项；组用 Pass-through 在
    /// `layerswindow` 侧单独拼，不在此枚举内）。
    pub fn tooltip(self) -> &'static str {
        match self {
            CombineMode::Normal => "The top layer only, without blending colors.",
            CombineMode::SpectralWGM => {
                "Similar to mixing actual pigments by upsampling to 10 spectral channels."
            }
            CombineMode::Multiply => {
                "Similar to loading two slides into a projector and projecting the combined result."
            }
            CombineMode::Screen => {
                "Like shining two separate slide projectors onto a screen simultaneously. \
                 This is the inverse of 'Multiply'."
            }
            CombineMode::Overlay => {
                "Overlays the backdrop with the top layer, preserving the backdrop's highlights \
                 and shadows. This is the inverse of 'Hard Light'."
            }
            CombineMode::Darken => "The top layer is used where it is darker than the backdrop.",
            CombineMode::Lighten => "The top layer is used where it is lighter than the backdrop.",
            CombineMode::ColorDodge => {
                "Brightens the backdrop using the top layer. The effect is similar to the \
                 photographic darkroom technique of the same name which is used for improving \
                 contrast in shadows."
            }
            CombineMode::ColorBurn => {
                "Darkens the backdrop using the top layer. The effect looks similar to the \
                 photographic darkroom technique of the same name which is used for reducing \
                 over-bright highlights."
            }
            CombineMode::HardLight => "Similar to shining a harsh spotlight onto the backdrop.",
            CombineMode::SoftLight => "Like shining a diffuse spotlight onto the backdrop.",
            CombineMode::Difference => "Subtracts the darker color from the lighter of the two.",
            CombineMode::Exclusion => "Similar to the 'Difference' mode, but lower in contrast.",
            CombineMode::Hue => {
                "Combines the hue of the top layer with the saturation and luminosity of the \
                 backdrop."
            }
            CombineMode::Saturation => {
                "Applies the saturation of the top layer's colors to the hue and luminosity of \
                 the backdrop."
            }
            CombineMode::Color => {
                "Applies the hue and saturation of the top layer to the luminosity of the \
                 backdrop."
            }
            CombineMode::Luminosity => {
                "Applies the luminosity of the top layer to the hue and saturation of the \
                 backdrop."
            }
            CombineMode::Lighter => "This layer and its backdrop are simply added together.",
            CombineMode::DestinationIn => {
                "Uses the backdrop only where this layer covers it. Everything else is ignored."
            }
            CombineMode::DestinationOut => {
                "Uses the backdrop only where this layer doesn't cover it. Everything else is \
                 ignored."
            }
            CombineMode::SourceAtop => {
                "Source which overlaps the destination, replaces the destination. Destination is \
                 placed elsewhere."
            }
            CombineMode::DestinationAtop => {
                "Destination which overlaps the source replaces the source. Source is placed \
                 elsewhere."
            }
        }
    }

    /// `composite_func::zero_alpha_has_effect`（`compositing.hpp` 各类静态常量）。
    pub fn zero_alpha_has_effect(self) -> bool {
        matches!(
            self,
            CombineMode::DestinationIn | CombineMode::DestinationAtop
        )
    }

    /// `can_decrease_alpha`。
    pub fn can_decrease_alpha(self) -> bool {
        matches!(
            self,
            CombineMode::DestinationIn | CombineMode::DestinationOut | CombineMode::DestinationAtop
        )
    }

    /// `zero_alpha_clears_backdrop`。
    pub fn zero_alpha_clears_backdrop(self) -> bool {
        matches!(
            self,
            CombineMode::DestinationIn | CombineMode::DestinationAtop
        )
    }
}

/// 把一块 fix15 预乘 tile 按指定模式混入 `dst`（`pixops.cpp:838` `tile_combine`）。
///
/// `dst_has_alpha` 对应 C++ 模板参数：背景 tile（无 alpha 通道语义）传 `false`。
/// `src_opacity` 是图层不透明度（`effective_opacity`，不可见层为 0.0）。
pub fn tile_combine(
    mode: CombineMode,
    src: &[u16],
    dst: &mut [u16],
    dst_has_alpha: bool,
    src_opacity: f32,
) {
    // 数学逐像素与 tile 尺寸无关（C++ 的 BUFSIZE 只是模板参数），按传入长度算。
    debug_assert_eq!(src.len(), dst.len());
    debug_assert_eq!(src.len() % 4, 0);
    let opac = fix15::short_clamp((src_opacity * (fix15::ONE as f32)) as u32);

    // pixops.cpp:784-811 的 LUT 分派。带专用（预乘空间快路径）特化的组合：
    //   Normal×{SourceOver, SpectralWGM, DestinationIn, DestinationOut,
    //           SourceAtop, DestinationAtop}
    // 其余走 compositing.hpp:81 的通用模板路径。
    match (mode, dst_has_alpha) {
        (CombineMode::Normal, true) => srcover_premul::<true>(src, dst, opac),
        (CombineMode::Normal, false) => srcover_premul::<false>(src, dst, opac),
        (CombineMode::SpectralWGM, true) => wgm::<true>(src, dst, opac),
        (CombineMode::SpectralWGM, false) => wgm::<false>(src, dst, opac),
        (CombineMode::DestinationIn, true) => dst_in_premul::<true>(src, dst, opac),
        (CombineMode::DestinationIn, false) => dst_in_premul::<false>(src, dst, opac),
        (CombineMode::DestinationOut, true) => dst_out_premul::<true>(src, dst, opac),
        (CombineMode::DestinationOut, false) => dst_out_premul::<false>(src, dst, opac),
        (CombineMode::SourceAtop, true) => src_atop_premul::<true>(src, dst, opac),
        (CombineMode::SourceAtop, false) => src_atop_premul::<false>(src, dst, opac),
        (CombineMode::DestinationAtop, true) => dst_atop_premul::<true>(src, dst, opac),
        (CombineMode::DestinationAtop, false) => dst_atop_premul::<false>(src, dst, opac),
        (m, dha) => generic(m, dha, src, dst, opac),
    }
}

/// 通用模板路径（`compositing.hpp:81-178`）：按模式取 Blend/Composite 函数对。
fn generic(mode: CombineMode, dst_has_alpha: bool, src: &[u16], dst: &mut [u16], opac: u16) {
    let blend: fn(u32, u32, u32, &mut u32, &mut u32, &mut u32) = match mode {
        CombineMode::Normal => blend_normal,
        CombineMode::Multiply => blend_multiply,
        CombineMode::Screen => blend_screen,
        CombineMode::Overlay => blend_overlay,
        CombineMode::Darken => blend_darken,
        CombineMode::Lighten => blend_lighten,
        CombineMode::HardLight => blend_hardlight,
        CombineMode::SoftLight => blend_softlight,
        CombineMode::ColorBurn => blend_colorburn,
        CombineMode::ColorDodge => blend_colordodge,
        CombineMode::Difference => blend_difference,
        CombineMode::Exclusion => blend_exclusion,
        CombineMode::Hue => blend_hue,
        CombineMode::Saturation => blend_saturation,
        CombineMode::Color => blend_color,
        CombineMode::Luminosity => blend_luminosity,
        // Lighter 组合用的是 Normal 混色（`pixops.cpp:805`）
        CombineMode::Lighter => blend_normal,
        _ => unreachable!("专用路径已在上层拦截"),
    };
    let composite_zero_alpha_has_effect = mode.zero_alpha_has_effect();
    let composite: fn(u32, u32, u32, u32, &mut [u16; 4]) = match mode {
        CombineMode::Lighter => composite_lighter,
        _ => composite_source_over,
    };

    // 跳过整 tile：混色不可能影响背景且不透明度为 0（compositing.hpp:97-102）
    if !composite_zero_alpha_has_effect && opac == 0 {
        return;
    }

    if dst_has_alpha {
        generic_loop::<true>(
            src,
            dst,
            opac,
            blend,
            composite,
            composite_zero_alpha_has_effect,
        );
    } else {
        generic_loop::<false>(
            src,
            dst,
            opac,
            blend,
            composite,
            composite_zero_alpha_has_effect,
        );
    }
}

fn generic_loop<const DSTALPHA: bool>(
    src: &[u16],
    dst: &mut [u16],
    opac: u16,
    blend: fn(u32, u32, u32, &mut u32, &mut u32, &mut u32),
    composite: fn(u32, u32, u32, u32, &mut [u16; 4]),
    skip_empty_src: bool,
) {
    for i in (0..src.len()).step_by(4) {
        // 源颜色去预乘（compositing.hpp:109-126）
        let as_ = src[i + 3] as u32;
        let (rs, gs, bs);
        if as_ == 0 {
            if skip_empty_src {
                continue;
            }
            rs = 0;
            gs = 0;
            bs = 0;
        } else {
            rs = fix15::short_clamp(fix15::div(src[i] as u32, as_)) as u32;
            gs = fix15::short_clamp(fix15::div(src[i + 1] as u32, as_)) as u32;
            bs = fix15::short_clamp(fix15::div(src[i + 2] as u32, as_)) as u32;
        }

        // 背景颜色去预乘（compositing.hpp:133-150）
        let (ab, mut rb, mut gb, mut bb);
        if DSTALPHA {
            ab = dst[i + 3] as u32;
            if ab == 0 {
                rb = 0;
                gb = 0;
                bb = 0;
            } else {
                rb = fix15::short_clamp(fix15::div(dst[i] as u32, ab)) as u32;
                gb = fix15::short_clamp(fix15::div(dst[i + 1] as u32, ab)) as u32;
                bb = fix15::short_clamp(fix15::div(dst[i + 2] as u32, ab)) as u32;
            }
        } else {
            ab = fix15::ONE;
            rb = dst[i] as u32;
            gb = dst[i + 1] as u32;
            bb = dst[i + 2] as u32;
        }

        // 混色（blend functor 修改的是背景色）
        blend(rs, gs, bs, &mut rb, &mut gb, &mut bb);

        // 按 DSTALPHA 把混色结果与源色按背景 alpha 插值（compositing.hpp:161-166）
        if DSTALPHA {
            let one_minus_ab = fix15::ONE - ab;
            rb = fix15::sumprods(one_minus_ab, rs, ab, rb);
            gb = fix15::sumprods(one_minus_ab, gs, ab, gb);
            bb = fix15::sumprods(one_minus_ab, bs, ab, bb);
        }

        // C++ 里 CompositeFunc 通过引用就地改写 dst 的 4 个通道
        let mut px = [dst[i], dst[i + 1], dst[i + 2], dst[i + 3]];
        composite(rb, gb, bb, fix15::mul(as_, opac as u32), &mut px);
        dst[i..i + 4].copy_from_slice(&px);
    }
}

// ---- 专用预乘快路径（blending.hpp 的各部分特化） ----

/// `blending.hpp:100-121`：Normal × SourceOver，预乘空间直算。
fn srcover_premul<const DSTALPHA: bool>(src: &[u16], dst: &mut [u16], opac: u16) {
    for i in (0..src.len()).step_by(4) {
        let sa = fix15::mul(src[i + 3] as u32, opac as u32);
        let one_minus_sa = fix15::ONE - sa;
        dst[i] = fix15::sumprods(src[i] as u32, opac as u32, one_minus_sa, dst[i] as u32) as u16;
        dst[i + 1] = fix15::sumprods(
            src[i + 1] as u32,
            opac as u32,
            one_minus_sa,
            dst[i + 1] as u32,
        ) as u16;
        dst[i + 2] = fix15::sumprods(
            src[i + 2] as u32,
            opac as u32,
            one_minus_sa,
            dst[i + 2] as u32,
        ) as u16;
        if DSTALPHA {
            dst[i + 3] = fix15::short_clamp(sa + fix15::mul(dst[i + 3] as u32, one_minus_sa));
        }
    }
}

/// `blending.hpp:123-194`：Normal × SpectralWGM —— 光谱 WGM 颜料混色特化。
fn wgm<const DSTALPHA: bool>(src: &[u16], dst: &mut [u16], opac: u16) {
    for i in (0..src.len()).step_by(4) {
        let sa = fix15::mul(src[i + 3] as u32, opac as u32);
        let one_minus_sa = fix15::ONE - sa;
        // 退化情形走普通 src-over：dst 无 alpha、或源全透/全不透
        if (DSTALPHA && dst[i + 3] == 0) || sa == fix15::ONE || sa == 0 {
            dst[i] =
                fix15::sumprods(src[i] as u32, opac as u32, one_minus_sa, dst[i] as u32) as u16;
            dst[i + 1] = fix15::sumprods(
                src[i + 1] as u32,
                opac as u32,
                one_minus_sa,
                dst[i + 1] as u32,
            ) as u16;
            dst[i + 2] = fix15::sumprods(
                src[i + 2] as u32,
                opac as u32,
                one_minus_sa,
                dst[i + 2] as u32,
            ) as u16;
            if DSTALPHA {
                dst[i + 3] = fix15::short_clamp(sa + fix15::mul(dst[i + 3] as u32, one_minus_sa));
            }
            continue;
        }

        // alpha 加权比（WGM 权重，和为 1.0）
        // C++ `fac_a = (float)Sa / (Sa + one_minus_Sa * dst[i+3] / (1<<15))`：
        // 分母全是 **uint32 整数运算**（乘、整除、加），仅最后转 float 相除。
        let fac_a: f32 = if DSTALPHA {
            sa as f32 / (sa + one_minus_sa * (dst[i + 3] as u32) / fix15::ONE) as f32
        } else {
            sa as f32 / (fix15::ONE as f32)
        };
        let fac_b = 1.0f32 - fac_a;

        // 背景转光谱（去预乘取反射率；低 alpha 天然低权重）
        let mut spectral_b = [0f32; 10];
        if DSTALPHA && dst[i + 3] > 0 {
            rgb_to_spectral(
                dst[i] as f32 / dst[i + 3] as f32,
                dst[i + 1] as f32 / dst[i + 3] as f32,
                dst[i + 2] as f32 / dst[i + 3] as f32,
                &mut spectral_b,
            );
        } else {
            rgb_to_spectral(
                dst[i] as f32 / (fix15::ONE as f32),
                dst[i + 1] as f32 / (fix15::ONE as f32),
                dst[i + 2] as f32 / (fix15::ONE as f32),
                &mut spectral_b,
            );
        }
        // 源转光谱（已是直色）
        let mut spectral_a = [0f32; 10];
        if src[i + 3] > 0 {
            rgb_to_spectral(
                src[i] as f32 / src[i + 3] as f32,
                src[i + 1] as f32 / src[i + 3] as f32,
                src[i + 2] as f32 / src[i + 3] as f32,
                &mut spectral_a,
            );
        } else {
            rgb_to_spectral(
                src[i] as f32 / (fix15::ONE as f32),
                src[i + 1] as f32 / (fix15::ONE as f32),
                src[i + 2] as f32 / (fix15::ONE as f32),
                &mut spectral_a,
            );
        }
        // WGM 混合两条光谱反射率
        let mut spectral_result = [0f32; 10];
        for k in 0..10 {
            spectral_result[k] = fastpow(spectral_a[k], fac_a) * fastpow(spectral_b[k], fac_b);
        }

        // 转回 RGB 并预乘
        let mut rgb_result = [0f32; 4];
        spectral_to_rgb(&spectral_result, &mut rgb_result);
        if DSTALPHA {
            rgb_result[3] =
                fix15::short_clamp(sa + fix15::mul(dst[i + 3] as u32, one_minus_sa)) as f32;
        } else {
            rgb_result[3] = fix15::ONE as f32;
        }
        // C++ 源码 `rgb_result[j] * (rgb_result[3] + 0.5)`：0.5 是 double 字面量，
        // 整个表达式按 double 求值后截断 —— 逐位一致必须用 f64。
        for j in 0..3 {
            dst[i + j] = ((rgb_result[j] as f64) * ((rgb_result[3] as f64) + 0.5)) as u16;
        }
        if DSTALPHA {
            dst[i + 3] = rgb_result[3] as u16;
        }
    }
}

/// `blending.hpp:196-216`：Normal × DestinationIn。
fn dst_in_premul<const DSTALPHA: bool>(src: &[u16], dst: &mut [u16], opac: u16) {
    for i in (0..src.len()).step_by(4) {
        let sa = fix15::mul(src[i + 3] as u32, opac as u32);
        dst[i] = fix15::mul(dst[i] as u32, sa) as u16;
        dst[i + 1] = fix15::mul(dst[i + 1] as u32, sa) as u16;
        dst[i + 2] = fix15::mul(dst[i + 2] as u32, sa) as u16;
        if DSTALPHA {
            dst[i + 3] = fix15::mul(sa, dst[i + 3] as u32) as u16;
        }
    }
}

/// `blending.hpp:218-238`：Normal × DestinationOut。
fn dst_out_premul<const DSTALPHA: bool>(src: &[u16], dst: &mut [u16], opac: u16) {
    for i in (0..src.len()).step_by(4) {
        let one_minus_sa = fix15::ONE - fix15::mul(src[i + 3] as u32, opac as u32);
        dst[i] = fix15::mul(dst[i] as u32, one_minus_sa) as u16;
        dst[i + 1] = fix15::mul(dst[i + 1] as u32, one_minus_sa) as u16;
        dst[i + 2] = fix15::mul(dst[i + 2] as u32, one_minus_sa) as u16;
        if DSTALPHA {
            dst[i + 3] = fix15::mul(one_minus_sa, dst[i + 3] as u32) as u16;
        }
    }
}

/// `blending.hpp:240-272`：Normal × SourceAtop。
fn src_atop_premul<const DSTALPHA: bool>(src: &[u16], dst: &mut [u16], opac: u16) {
    for i in (0..src.len()).step_by(4) {
        let as_ = fix15::mul(src[i + 3] as u32, opac as u32);
        let ab = dst[i + 3] as u32;
        let one_minus_as = fix15::ONE - as_;
        // W3C: co = as*Cs*ab + ab*Cb*(1-as)；输出 alpha 不变
        dst[i] = fix15::sumprods(
            fix15::mul(src[i] as u32, opac as u32),
            ab,
            dst[i] as u32,
            one_minus_as,
        ) as u16;
        dst[i + 1] = fix15::sumprods(
            fix15::mul(src[i + 1] as u32, opac as u32),
            ab,
            dst[i + 1] as u32,
            one_minus_as,
        ) as u16;
        dst[i + 2] = fix15::sumprods(
            fix15::mul(src[i + 2] as u32, opac as u32),
            ab,
            dst[i + 2] as u32,
            one_minus_as,
        ) as u16;
    }
}

/// `blending.hpp:274-307`：Normal × DestinationAtop。
fn dst_atop_premul<const DSTALPHA: bool>(src: &[u16], dst: &mut [u16], opac: u16) {
    for i in (0..src.len()).step_by(4) {
        let as_ = fix15::mul(src[i + 3] as u32, opac as u32);
        let ab = dst[i + 3] as u32;
        let one_minus_ab = fix15::ONE - ab;
        // W3C: co = as*Cs*(1-ab) + ab*Cb*as；ao = as
        dst[i] = fix15::sumprods(
            fix15::mul(src[i] as u32, opac as u32),
            one_minus_ab,
            dst[i] as u32,
            as_,
        ) as u16;
        dst[i + 1] = fix15::sumprods(
            fix15::mul(src[i + 1] as u32, opac as u32),
            one_minus_ab,
            dst[i + 1] as u32,
            as_,
        ) as u16;
        dst[i + 2] = fix15::sumprods(
            fix15::mul(src[i + 2] as u32, opac as u32),
            one_minus_ab,
            dst[i + 2] as u32,
            as_,
        ) as u16;
        if DSTALPHA {
            dst[i + 3] = as_ as u16;
        }
    }
}

// ---- CompositeFunc（compositing.hpp）----
// 输入 RGB 是未预乘的混色结果；写出的是预乘值。
// `as_` 已含图层不透明度（fix15_mul(as, src_opacity)）。

fn composite_source_over(rs: u32, gs: u32, bs: u32, as_: u32, px: &mut [u16; 4]) {
    // compositing.hpp:206-226（rb/gb/bb/ab 即 px[0..4]）
    let [rb, gb, bb, ab] = px;
    let j = fix15::ONE - as_;
    let k = fix15::mul(*ab as u32, j);
    *rb = fix15::short_clamp(fix15::sumprods(as_, rs, j, *rb as u32));
    *gb = fix15::short_clamp(fix15::sumprods(as_, gs, j, *gb as u32));
    *bb = fix15::short_clamp(fix15::sumprods(as_, bs, j, *bb as u32));
    *ab = fix15::short_clamp(as_ + k);
}

fn composite_lighter(rs: u32, gs: u32, bs: u32, as_: u32, px: &mut [u16; 4]) {
    // compositing.hpp:374-391（Porter-Duff Plus）
    let [rb, gb, bb, ab] = px;
    *rb = fix15::short_clamp(fix15::mul(rs, as_) + *rb as u32);
    *gb = fix15::short_clamp(fix15::mul(gs, as_) + *gb as u32);
    *bb = fix15::short_clamp(fix15::mul(bs, as_) + *bb as u32);
    *ab = fix15::short_clamp(*ab as u32 + as_);
}

// ---- BlendFunc（blending.hpp）----
// 全部修改 dst（背景色），输入输出均为未预乘 fix15。

fn blend_normal(_sr: u32, _sg: u32, _sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    // 原样保留背景（blending.hpp:87-98 把 src 赋给 dst —— 但通用路径随后
    // 又把背景色与源色按 DSTALPHA 混合，直接沿用 C++ 语义：dst = src）
    *dr = _sr;
    *dg = _sg;
    *db = _sb;
}

fn blend_multiply(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    *dr = fix15::mul(sr, *dr);
    *dg = fix15::mul(sg, *dg);
    *db = fix15::mul(sb, *db);
}

fn blend_screen(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    *dr = *dr + sr - fix15::mul(*dr, sr);
    *dg = *dg + sg - fix15::mul(*dg, sg);
    *db = *db + sb - fix15::mul(*db, sb);
}

fn blend_overlay(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    overlay_channel(sr, dr);
    overlay_channel(sg, dg);
    overlay_channel(sb, db);
}

fn overlay_channel(cs: u32, cb: &mut u32) {
    // blending.hpp:350-360
    let two_cb = fix15::double(*cb);
    if two_cb <= fix15::ONE {
        *cb = fix15::mul(cs, two_cb);
    } else {
        let tmp = two_cb - fix15::ONE;
        *cb = cs + tmp - fix15::mul(cs, tmp);
    }
}

fn blend_darken(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    if sr < *dr {
        *dr = sr;
    }
    if sg < *dg {
        *dg = sg;
    }
    if sb < *db {
        *db = sb;
    }
}

fn blend_lighten(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    if sr > *dr {
        *dr = sr;
    }
    if sg > *dg {
        *dg = sg;
    }
    if sb > *db {
        *db = sb;
    }
}

fn blend_hardlight(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    hardlight_channel(sr, dr);
    hardlight_channel(sg, dg);
    hardlight_channel(sb, db);
}

fn hardlight_channel(cs: u32, cb: &mut u32) {
    // blending.hpp:412-423
    let two_cs = fix15::double(cs);
    if two_cs <= fix15::ONE {
        *cb = fix15::mul(*cb, two_cs);
    } else {
        let tmp = two_cs - fix15::ONE;
        *cb = *cb + tmp - fix15::mul(*cb, tmp);
    }
}

fn blend_colordodge(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    colordodge_channel(sr, dr);
    colordodge_channel(sg, dg);
    colordodge_channel(sb, db);
}

fn colordodge_channel(cs: u32, cb: &mut u32) {
    // blending.hpp:441-451
    if cs < fix15::ONE {
        let tmp = fix15::div(*cb, fix15::ONE - cs);
        if tmp < fix15::ONE {
            *cb = tmp;
            return;
        }
    }
    *cb = fix15::ONE;
}

fn blend_colorburn(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    colorburn_channel(sr, dr);
    colorburn_channel(sg, dg);
    colorburn_channel(sb, db);
}

fn colorburn_channel(cs: u32, cb: &mut u32) {
    // blending.hpp:471-480
    if cs > 0 {
        let tmp = fix15::div(fix15::ONE - *cb, cs);
        if tmp < fix15::ONE {
            *cb = fix15::ONE - tmp;
            return;
        }
    }
    *cb = 0;
}

fn blend_softlight(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    softlight_channel(sr, dr);
    softlight_channel(sg, dg);
    softlight_channel(sb, db);
}

fn softlight_channel(cs: u32, cb: &mut u32) {
    // blending.hpp:499-530
    let two_cs = fix15::double(cs);
    let b;
    if two_cs <= fix15::ONE {
        let mut bb = fix15::ONE - fix15::mul(fix15::ONE - two_cs, fix15::ONE - *cb);
        bb = fix15::mul(bb, *cb);
        b = bb;
    } else {
        let d;
        let four_cb = *cb << 2;
        if four_cb <= fix15::ONE {
            let cb_squared = fix15::mul(*cb, *cb);
            // D = 4Cb + 16Cb³ - 12Cb²（0 ≤ Cb ≤ 0.25 时恒 ≥ Cb）
            d = four_cb + 16 * fix15::mul(cb_squared, *cb) - 12 * cb_squared;
        } else {
            d = fix15::sqrt(*cb);
        }
        b = *cb + fix15::mul(2 * cs - fix15::ONE, d - *cb);
    }
    *cb = b;
}

fn blend_difference(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    difference_channel(sr, dr);
    difference_channel(sg, dg);
    difference_channel(sb, db);
}

fn difference_channel(cs: u32, cb: &mut u32) {
    // blending.hpp:549-556
    if cs >= *cb {
        *cb = cs - *cb;
    } else {
        *cb = *cb - cs;
    }
}

fn blend_exclusion(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    exclusion_channel(sr, dr);
    exclusion_channel(sg, dg);
    exclusion_channel(sb, db);
}

fn exclusion_channel(cs: u32, cb: &mut u32) {
    // blending.hpp:574-577
    *cb = *cb + cs - fix15::double(fix15::mul(*cb, cs));
}

// ---- 不可分离模式（blending.hpp:592-696，ufix15_t = i32）----

const BLENDING_LUM_R_COEFF: i32 = (0.3 * (fix15::ONE as f64)) as i32; // 9830
const BLENDING_LUM_G_COEFF: i32 = (0.59 * (fix15::ONE as f64)) as i32; // 19333
const BLENDING_LUM_B_COEFF: i32 = (0.11 * (fix15::ONE as f64)) as i32; // 3604

fn nonsep_lum(r: i32, g: i32, b: i32) -> i32 {
    (r * BLENDING_LUM_R_COEFF + g * BLENDING_LUM_G_COEFF + b * BLENDING_LUM_B_COEFF)
        / fix15::ONE as i32
}

fn nonsep_clipcolor(r: &mut i32, g: &mut i32, b: &mut i32) {
    // blending.hpp:617-638
    let lum = nonsep_lum(*r, *g, *b);
    let cmin = if *r < *g { (*r).min(*b) } else { (*g).min(*b) };
    let cmax = if *r > *g { (*r).max(*b) } else { (*g).max(*b) };
    if cmin < 0 {
        let lum_minus_cmin = lum - cmin;
        *r = lum + ((*r - lum) * lum) / lum_minus_cmin;
        *g = lum + ((*g - lum) * lum) / lum_minus_cmin;
        *b = lum + ((*b - lum) * lum) / lum_minus_cmin;
    }
    if cmax > fix15::ONE as i32 {
        let one_minus_lum = fix15::ONE as i32 - lum;
        let cmax_minus_lum = cmax - lum;
        *r = lum + ((*r - lum) * one_minus_lum) / cmax_minus_lum;
        *g = lum + ((*g - lum) * one_minus_lum) / cmax_minus_lum;
        *b = lum + ((*b - lum) * one_minus_lum) / cmax_minus_lum;
    }
}

fn nonsep_setlum(r: &mut i32, g: &mut i32, b: &mut i32, lum: i32) {
    let diff = lum - nonsep_lum(*r, *g, *b);
    *r += diff;
    *g += diff;
    *b += diff;
    nonsep_clipcolor(r, g, b);
}

fn nonsep_sat(r: i32, g: i32, b: i32) -> i32 {
    let cmax = if r > g { r.max(b) } else { g.max(b) };
    let cmin = if r < g { r.min(b) } else { g.min(b) };
    cmax - cmin
}

fn nonsep_setsat(r: &mut i32, g: &mut i32, b: &mut i32, s: i32) {
    // blending.hpp:666-696：指针排序等价于 (值, 原变量) 成对交换。
    // C 里 top/mid/bot 初始分别指向 b/g/r。
    let mut top = (*b, 2usize);
    let mut mid = (*g, 1usize);
    let mut bot = (*r, 0usize);
    if top.0 < mid.0 {
        std::mem::swap(&mut top, &mut mid);
    }
    if top.0 < bot.0 {
        std::mem::swap(&mut top, &mut bot);
    }
    if mid.0 < bot.0 {
        std::mem::swap(&mut mid, &mut bot);
    }
    if top.0 > bot.0 {
        // C: 先乘后除两条语句（(mid-bot)*s ≤ 2^30，i32 安全）
        mid.0 = (mid.0 - bot.0) * s;
        mid.0 /= top.0 - bot.0;
        top.0 = s;
    } else {
        top.0 = 0;
        mid.0 = 0;
    }
    bot.0 = 0;
    // C 通过排序后的指针写回各原变量（top 未必还是 b）——按原始索引写回
    let mut out = [0i32; 3];
    out[top.1] = top.0;
    out[mid.1] = mid.0;
    out[bot.1] = bot.0;
    *r = out[0];
    *g = out[1];
    *b = out[2];
}

fn blend_hue(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    // blending.hpp:701-727
    let dst_lum = nonsep_lum(*dr as i32, *dg as i32, *db as i32);
    let dst_sat = nonsep_sat(*dr as i32, *dg as i32, *db as i32);
    let mut r = sr as i32;
    let mut g = sg as i32;
    let mut b = sb as i32;
    nonsep_setsat(&mut r, &mut g, &mut b, dst_sat);
    nonsep_setlum(&mut r, &mut g, &mut b, dst_lum);
    *dr = r as u32;
    *dg = g as u32;
    *db = b as u32;
}

fn blend_saturation(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    // blending.hpp:732-758
    let dst_lum = nonsep_lum(*dr as i32, *dg as i32, *db as i32);
    let src_sat = nonsep_sat(sr as i32, sg as i32, sb as i32);
    let mut r = *dr as i32;
    let mut g = *dg as i32;
    let mut b = *db as i32;
    nonsep_setsat(&mut r, &mut g, &mut b, src_sat);
    nonsep_setlum(&mut r, &mut g, &mut b, dst_lum);
    *dr = r as u32;
    *dg = g as u32;
    *db = b as u32;
}

fn blend_color(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    // blending.hpp:763-787
    let mut r = sr as i32;
    let mut g = sg as i32;
    let mut b = sb as i32;
    nonsep_setlum(
        &mut r,
        &mut g,
        &mut b,
        nonsep_lum(*dr as i32, *dg as i32, *db as i32),
    );
    *dr = r as u32;
    *dg = g as u32;
    *db = b as u32;
}

fn blend_luminosity(sr: u32, sg: u32, sb: u32, dr: &mut u32, dg: &mut u32, db: &mut u32) {
    // blending.hpp:792-816
    let mut r = *dr as i32;
    let mut g = *dg as i32;
    let mut b = *db as i32;
    nonsep_setlum(
        &mut r,
        &mut g,
        &mut b,
        nonsep_lum(sr as i32, sg as i32, sb as i32),
    );
    *dr = r as u32;
    *dg = g as u32;
    *db = b as u32;
}

// ---- 光谱 upsampling（blending.hpp:21-81）----

/// WGM 数值稳定下限（`blending.hpp:15` `WGM_EPSILON`）。
/// C 源码里是 `#define WGM_EPSILON 0.001` —— **double 字面量**，参与运算时
/// 会把 float 提升 double；双精度版本另行保留。
const WGM_EPSILON_F64: f64 = 0.001;

/// RGB→光谱上采样矩阵（`blending.hpp:21-29`，10 通道）。
const T_MATRIX_SMALL: [[f32; 10]; 3] = [
    [
        0.026595621243689,
        0.049779426257903,
        0.022449850859496,
        -0.218453689278271,
        -0.256894883201278,
        0.445881722194840,
        0.772365886289756,
        0.194498761382537,
        0.014038157587820,
        0.007687264480513,
    ],
    [
        -0.032601672674412,
        -0.061021043498478,
        -0.052490001018404,
        0.206659098273522,
        0.572496335158169,
        0.317837248815438,
        -0.021216624031211,
        -0.019387668756117,
        -0.001521339050858,
        -0.000835181622534,
    ],
    [
        0.339475473216284,
        0.635401374177222,
        0.771520797089589,
        0.113222640692379,
        -0.055251113343776,
        -0.048222578468680,
        -0.012966666339586,
        -0.001523814504223,
        -0.000094718948810,
        -0.000051604594741,
    ],
];

const SPECTRAL_R_SMALL: [f32; 10] = [
    0.009281362787953,
    0.009732627042016,
    0.011254252737167,
    0.015105578649573,
    0.024797924177217,
    0.083622585502406,
    0.977865045723212,
    1.000000000000000,
    0.999961046144372,
    0.999999992756822,
];

const SPECTRAL_G_SMALL: [f32; 10] = [
    0.002854127435775,
    0.003917589679914,
    0.012132151699187,
    0.748259205918013,
    1.000000000000000,
    0.865695937531795,
    0.037477469241101,
    0.022816789725717,
    0.021747419446456,
    0.021384940572308,
];

const SPECTRAL_B_SMALL: [f32; 10] = [
    0.537052150373386,
    0.546646402401469,
    0.575501819073983,
    0.258778829633924,
    0.041709923751716,
    0.012662638828324,
    0.007485593127390,
    0.006766900622462,
    0.006699764779016,
    0.006676219883241,
];

fn rgb_to_spectral(r: f32, g: f32, b: f32, spectral: &mut [f32; 10]) {
    // blending.hpp:44-68。WGM_EPSILON（0.001）与字面量 1.0 是 double：
    // offset 在 double 域算完舍入为 float；`r*offset + 0.001` 提升为 double
    // 求和后舍回 float —— 两处舍入都必须复刻。
    let offset = (1.0 - WGM_EPSILON_F64) as f32;
    let r = ((r * offset) as f64 + WGM_EPSILON_F64) as f32;
    let g = ((g * offset) as f64 + WGM_EPSILON_F64) as f32;
    let b = ((b * offset) as f64 + WGM_EPSILON_F64) as f32;
    for k in 0..10 {
        spectral[k] += SPECTRAL_R_SMALL[k] * r + SPECTRAL_G_SMALL[k] * g + SPECTRAL_B_SMALL[k] * b;
    }
}

fn spectral_to_rgb(spectral: &[f32; 10], rgb: &mut [f32; 4]) {
    // blending.hpp:70-81（调用方保证 rgb[0..3] 为 0）。
    // 末段 `(rgb - eps) / offset` 的 eps 是 double 字面量 → double 域求值
    // 再舍回 float。
    let offset = (1.0 - WGM_EPSILON_F64) as f32;
    for k in 0..10 {
        rgb[0] += T_MATRIX_SMALL[0][k] * spectral[k];
        rgb[1] += T_MATRIX_SMALL[1][k] * spectral[k];
        rgb[2] += T_MATRIX_SMALL[2][k] * spectral[k];
    }
    for v in rgb.iter_mut().take(3) {
        *v = (((*v as f64 - WGM_EPSILON_F64) / (offset as f64)).clamp(0.0, 1.0)) as f32;
    }
}

// ---- fastapprox（fastpow.h / fastexp.h / fastlog.h）----

/// `fastexp.h:52` `fastpow2`。
fn fastpow2(p: f32) -> f32 {
    let offset = if p < 0.0 { 1.0f32 } else { 0.0f32 };
    let clipp = if p < -126.0 { -126.0f32 } else { p };
    // C 的 `int w = clipp`：向零截断
    let w = clipp as i32;
    let z = clipp - w as f32 + offset;
    // cast_uint32_t：见文件头注记（WGM 可达范围内参数恒正）
    let bits = ((1u32 << 23) as f32
        * (clipp + 121.2740575f32 + 27.7280233f32 / (4.84252568f32 - z) - 1.49012907f32 * z))
        as u32;
    f32::from_bits(bits)
}

/// `fastlog.h:48` `fastlog2`。
fn fastlog2(x: f32) -> f32 {
    let vx = x.to_bits();
    // C 的 union 复用：mx 是「位型重解释」，不是整数值转换
    let mx = f32::from_bits((vx & 0x007F_FFFF) | 0x3F00_0000);
    // float y = vx.i（u32 → float 整数值转换）再乘 2^-23
    let y = vx as f32 * 1.192_092_895_507_812_5e-7f32;
    y - 124.225_514_99f32 - 1.498_030_302f32 * mx - 1.725_879_99f32 / (0.352_088_706_8f32 + mx)
}

/// `fastpow.h:50` `fastpow`。
fn fastpow(x: f32, p: f32) -> f32 {
    fastpow2(p * fastlog2(x))
}

// ---- fix15（fix15.hpp 全量移植；hokusai::fix15 只覆盖了其中一部分）----

pub mod fix15 {
    /// 1.0 的定点表示（`fix15.hpp:26`）。
    pub const ONE: u32 = 1 << 15;

    /// `fix15_short_clamp`（`fix15.hpp:37-41`）。
    #[inline]
    pub const fn short_clamp(n: u32) -> u16 {
        if n > ONE {
            ONE as u16
        } else {
            n as u16
        }
    }

    /// `fix15_mul`（`fix15.hpp:46-50`）。
    #[inline]
    pub const fn mul(a: u32, b: u32) -> u32 {
        (a * b) >> 15
    }

    /// `fix15_sumprods`（`fix15.hpp:53-58`）。
    #[inline]
    pub const fn sumprods(a1: u32, a2: u32, b1: u32, b2: u32) -> u32 {
        ((a1 * a2) + (b1 * b2)) >> 15
    }

    /// `fix15_double`（`fix15.hpp:61-63`）。
    #[inline]
    pub const fn double(n: u32) -> u32 {
        n << 1
    }

    /// `fix15_div`（`fix15.hpp:73-77`）。
    #[inline]
    pub fn div(a: u32, b: u32) -> u32 {
        (a << 15) / b
    }

    /// `fix15_sqrt`（`fix15.hpp:104-140`）：巴比伦法 + 初始近似表，逐位一致。
    pub fn sqrt(x: u32) -> u32 {
        const APPROX16: [u16; 16] = [
            16383, 23169, 28376, 32767, 36634, 40131, 43346, 46339, 49151, 51809, 54338, 56754,
            59072, 61302, 63453, 65535,
        ];
        if x == 0 || x == ONE {
            return x;
        }
        let s = (x << 1) as u32;
        let fracbits = 15 + 1;
        let mut n = APPROX16[(s >> 12) as usize] as u32;
        for _ in 0..15 {
            let n_old = n;
            n += (s << fracbits) / n;
            n >>= 1;
            if n == n_old || (n > n_old && n - 1 == n_old) || (n < n_old && n + 1 == n_old) {
                break;
            }
        }
        n >> 1
    }
}
