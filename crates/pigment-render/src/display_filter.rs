//! 显示过滤器 —— `gui/displayfilter.py` 的 Rust 版。
//!
//! 这些效果只作用于**显示用** RGBA8 像素，不改文档、不写盘。源码在
//! `TiledDrawWidget` 的渲染结果上做 NumPy 逐像素变换（`displayfilter.py:46-84`）；
//! 这里放在 CPU 合成 tile 之后、GPU 上传之前。

/// `Visualize` 六种显示过滤（`gui/menu.xml` 的 DisplayFilter 子菜单）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayFilter {
    #[default]
    None,
    /// `luma_only`（`displayfilter.py:46-49`）
    LumaOnly,
    /// `invert_colors`（`:52-54`）
    InvertColors,
    /// `sim_deuteranopia`（`:57-64`）
    SimDeuteranopia,
    /// `sim_protanopia`（`:67-74`）
    SimProtanopia,
    /// `sim_tritanopia`（`:77-84`）
    SimTritanopia,
}

impl DisplayFilter {
    pub fn from_action_id(id: &str) -> Option<Self> {
        match id {
            "DisplayFilterNone" => Some(Self::None),
            "DisplayFilterLumaOnly" => Some(Self::LumaOnly),
            "DisplayFilterInvertColors" => Some(Self::InvertColors),
            "DisplayFilterSimDeuteranopia" => Some(Self::SimDeuteranopia),
            "DisplayFilterSimProtanopia" => Some(Self::SimProtanopia),
            "DisplayFilterSimTritanopia" => Some(Self::SimTritanopia),
            _ => None,
        }
    }

    /// 就地过滤一整块 RGBA8 tile；alpha 不变。
    pub fn apply(self, pixels: &mut [u8]) {
        match self {
            Self::None => {}
            Self::LumaOnly => {
                for px in pixels.chunks_exact_mut(4) {
                    let luma =
                        (px[0] as f32 * 0.2126 + px[1] as f32 * 0.7152 + px[2] as f32 * 0.0722)
                            .clamp(0.0, 255.0) as u8;
                    px[0] = luma;
                    px[1] = luma;
                    px[2] = luma;
                }
            }
            Self::InvertColors => {
                for px in pixels.chunks_exact_mut(4) {
                    px[0] = 255 - px[0];
                    px[1] = 255 - px[1];
                    px[2] = 255 - px[2];
                }
            }
            Self::SimDeuteranopia => apply_matrix(
                pixels,
                [0.43, 0.72, -0.15],
                [0.34, 0.57, 0.09],
                [-0.02, 0.03, 1.0],
            ),
            Self::SimProtanopia => apply_matrix(
                pixels,
                [0.2, 0.99, -0.19],
                [0.16, 0.79, 0.04],
                [0.01, 0.01, 1.0],
            ),
            Self::SimTritanopia => apply_matrix(
                pixels,
                [0.972, 0.112, -0.084],
                [0.022, 0.818, 0.160],
                [-0.063, 0.881, 0.182],
            ),
        }
    }
}

fn apply_matrix(pixels: &mut [u8], r: [f32; 3], g: [f32; 3], b: [f32; 3]) {
    for px in pixels.chunks_exact_mut(4) {
        let (sr, sg, sb) = (px[0] as f32, px[1] as f32, px[2] as f32);
        px[0] = (sr * r[0] + sg * r[1] + sb * r[2]).clamp(0.0, 255.0) as u8;
        px[1] = (sr * g[0] + sg * g[1] + sb * g[2]).clamp(0.0, 255.0) as u8;
        px[2] = (sr * b[0] + sg * b[1] + sb * b[2]).clamp(0.0, 255.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luma_and_invert_match_source_formulas() {
        let mut px = [255, 0, 0, 128, 0, 255, 0, 255];
        DisplayFilter::LumaOnly.apply(&mut px);
        assert_eq!(&px[0..3], &[54, 54, 54], "0.2126*255 截断");
        assert_eq!(px[3], 128, "alpha 不变");
        assert_eq!(&px[4..7], &[182, 182, 182], "0.7152*255 截断");

        let mut px = [10, 20, 30, 255];
        DisplayFilter::InvertColors.apply(&mut px);
        assert_eq!(px, [245, 235, 225, 255]);
    }

    #[test]
    fn color_blindness_matrices_match_source() {
        // displayfilter.py:30-40，纯红/纯绿/纯蓝各跑一遍
        let mut red = [255, 0, 0, 255];
        DisplayFilter::SimDeuteranopia.apply(&mut red);
        assert_eq!(red, [109, 86, 0, 255]);

        let mut green = [0, 255, 0, 255];
        DisplayFilter::SimProtanopia.apply(&mut green);
        assert_eq!(green, [252, 201, 2, 255]);

        let mut blue = [0, 0, 255, 255];
        DisplayFilter::SimTritanopia.apply(&mut blue);
        assert_eq!(blue, [0, 40, 46, 255]);
    }

    #[test]
    fn action_ids_round_trip() {
        for (id, expected) in [
            ("DisplayFilterNone", DisplayFilter::None),
            ("DisplayFilterLumaOnly", DisplayFilter::LumaOnly),
            ("DisplayFilterInvertColors", DisplayFilter::InvertColors),
            (
                "DisplayFilterSimDeuteranopia",
                DisplayFilter::SimDeuteranopia,
            ),
            ("DisplayFilterSimProtanopia", DisplayFilter::SimProtanopia),
            ("DisplayFilterSimTritanopia", DisplayFilter::SimTritanopia),
        ] {
            assert_eq!(DisplayFilter::from_action_id(id), Some(expected), "{id}");
        }
        assert_eq!(DisplayFilter::default(), DisplayFilter::None);
    }
}
