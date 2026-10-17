//! 对称绘制 —— 逐行照搬 libmypaint 的对称实现。
//!
//! | libmypaint | 本文件 |
//! |---|---|
//! | `mypaint-matrix.h:20-22` `MyPaintTransform` | [`Transform`] |
//! | `mypaint-matrix.c:20-92` 矩阵乘法/旋转/反射/平移/变换点 | [`Transform`] 的方法 |
//! | `mypaint-symmetry.h:32-39` `MyPaintSymmetryType` | [`SymmetryType`] |
//! | `mypaint-symmetry.h:49-55` `MyPaintSymmetryState` | [`SymmetryState`] |
//! | `mypaint-symmetry.h:65-72` `MyPaintSymmetryData` | [`SymmetryData`] |
//! | `mypaint-symmetry.c:31-161` 状态机 + 矩阵计算 | [`SymmetryData`] 的方法 |
//! | `mypaint-tiled-surface.c:690-800` `draw_dab` 的对称 pass | [`PigmentSurface::draw_dab_with_symmetry`] |
//!
//! # 三条照源码的设计要点
//!
//! 1. **状态有 pending / current 两份**。`set_symmetry_state` 只写 pending，
//!    矩阵在 `begin_atomic` 时才重算（[`SymmetryData::update`]）——
//!    源码注释的原话是 "Matrix recalculation should not be performed during
//!    draw operations"（`mypaint-symmetry.h:63`），调用点在
//!    `mypaint_tiled_surface_begin_atomic`（`mypaint-tiled-surface.c:97`），
//!    也就是"一笔的开头"，绝不会落在 dab 热路径里。
//! 2. **对称展开发生在表面自己的 `draw_dab` 里**（`mypaint-tiled-surface.c:690-800`），
//!    不在笔刷引擎里：一次 `draw_dab` 先画原始 dab，再按矩阵画 N-1 个镜像/旋转 dab。
//!    所以笔刷引擎（hokusai 的 `Brush::stroke_to`）完全不需要知道对称存在。
//! 3. **只有位置与 `angle` 变**，`radius` / `aspect_ratio` / 颜色 / 不透明度全部照抄
//!    —— dab 是椭圆笔尖，关于自身长轴对称，所以"镜像一个 dab"等价于"反射它的角度"。
//!    源码里 `angle` 的反射公式直接写在 `draw_dab` 的 switch 里（`:730-786`），
//!    本文件逐分支照搬，见 [`for_each_symmetry_dab`]。
//!
//! # 与源码的两处差异（记账）
//!
//! 1. **"要不要做对称 pass"的判据**。源码用 `draw_dab_internal` 的返回值
//!    `surface_modified`（`:711`），它表示"这次 dab 没被早期返回挡掉"
//!    （`:642-645`），**与像素有没有被改动无关**；注释还写明引入选区遮罩后
//!    必须改成把每个 dab 的结果 OR 起来（`:717-720`）。hokusai 的
//!    `draw_dab_default` 返回值是"有没有像素被改动"，含义更窄，直接拿来当
//!    `surface_modified` 会让"原 dab 落在空处、镜像处有内容"的场合少画。
//!    因此这里按源码逐条重写早期返回判据（[`dab_is_drawable`]），只用它决定
//!    是否展开。`softness == 1.0` 那条查不了：hokusai 的 [`Dab`] 没有
//!    `softness` 字段（引擎侧已把它折进 `hardness`/`anti_aliasing`）。
//! 2. **bbox 计数（`bbox_index` / `num_bboxes_dirtied`）没有对应物**。源码它只
//!    服务于 `end_atomic` 上报脏矩形（`mypaint-tiled-surface.c:123-148`），
//!    而本项目的 `end_atomic` 直接收集被写借出的 tile（`lib.rs` 的 `dirty`），
//!    粒度更细，不需要这套 bbox 分配逻辑。剩下的部分（矩阵 + dab 角度）
//!    逐行保留。

use std::f32::consts::PI;
use std::sync::Arc;

use hokusai::brushmodes::draw_dab_default;
use hokusai::Dab;

use crate::PigmentSurface;

/// 矩阵数组的初始容量 —— `mypaint-symmetry.c:7` 的
/// `#define DEFAULT_NUM_MATRICES 16`。
pub const DEFAULT_NUM_MATRICES: usize = 16;

/// 对称类型 —— `MyPaintSymmetryType`（`mypaint-symmetry.h:32-39`）。
///
/// 判别值与 C 枚举、与 MyPaint 侧 `lib/tiledsurface.hpp:24-31` 的
/// `SymmetryType` 完全一致；Python 的选项面板把类型号直接当下标用
/// （`lib/tiledsurface.py:45`：`SYMMETRY_TYPES = tuple(range(NumSymmetryTypes))`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymmetryType {
    /// 跨 y 轴镜像（`MYPAINT_SYMMETRY_TYPE_VERTICAL`）。
    Vertical = 0,
    /// 跨 x 轴镜像（`MYPAINT_SYMMETRY_TYPE_HORIZONTAL`）。
    Horizontal = 1,
    /// 水平 + 垂直两条轴（`VERTHORZ`，SNOWFLAKE 的特例）。
    VertHorz = 2,
    /// 绕中心旋转 N 份（`ROTATIONAL`）。
    Rotational = 3,
    /// 旋转 + 每条轴镜像（`SNOWFLAKE`）。
    Snowflake = 4,
}

impl SymmetryType {
    /// 全部类型（顺序 = C 枚举顺序 = 选项面板的条目顺序）。
    pub const ALL: [SymmetryType; 5] = [
        SymmetryType::Vertical,
        SymmetryType::Horizontal,
        SymmetryType::VertHorz,
        SymmetryType::Rotational,
        SymmetryType::Snowflake,
    ];

    /// 从源码里的整数还原（越界返回 `None`，对应 C 的 `default:` 分支）。
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(SymmetryType::Vertical),
            1 => Some(SymmetryType::Horizontal),
            2 => Some(SymmetryType::VertHorz),
            3 => Some(SymmetryType::Rotational),
            4 => Some(SymmetryType::Snowflake),
            _ => None,
        }
    }

    /// 源码里的整数值（与 C 枚举同一套数字）。
    #[inline]
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// 对称的基础参数 —— `MyPaintSymmetryState`（`mypaint-symmetry.h:49-55`）。
///
/// 这里的字段名与 C 结构体一一对应；`num_lines` 在 C 里是 `float`（`:54`），
/// 由 `mypaint_symmetry_set_pending` 用 `MAX(2, rot_symmetry_lines)` 写入
/// （`mypaint-symmetry.c:157`），所以实际值总是 ≥ 2 的整数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SymmetryState {
    /// 对称类型。
    pub type_: SymmetryType,
    /// 对称中心的 x（模型坐标）。
    pub center_x: f32,
    /// 对称中心的 y（模型坐标）。
    pub center_y: f32,
    /// 对称线的角度（度）。
    pub angle: f32,
    /// 旋转对称的线数（C 里是 float，见上）。
    pub num_lines: f32,
}

impl Default for SymmetryState {
    /// `symmetry_state_default()`（`mypaint-symmetry.c:58-64`）：
    /// 垂直镜像、中心在原点、角度 0、2 条线。
    fn default() -> Self {
        Self {
            type_: SymmetryType::Vertical,
            center_x: 0.0,
            center_y: 0.0,
            angle: 0.0,
            num_lines: 2.0,
        }
    }
}

/// 3×3 变换矩阵 —— `MyPaintTransform`（`mypaint-matrix.h:20-22`）。
///
/// C 侧用**行向量**约定：`mypaint_transform_point` 算的是
/// `x' = rows[0][0]*x + rows[0][1]*y + rows[0][2]`（`mypaint-matrix.c:88-92`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub rows: [[f32; 3]; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self::UNIT
    }
}

impl Transform {
    /// 单位矩阵 —— `mypaint_transform_unit()`（`mypaint-matrix.c:35-40`）。
    pub const UNIT: Transform = Transform {
        rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    /// 矩阵乘 —— `mypaint_matrix_multiply`（`mypaint-matrix.c:20-33`）。
    ///
    /// 逐字照搬那个双重循环。在行向量约定下它算的是"先 `m1` 后 `m2`"，
    /// 所以 [`Transform::reflect`] 之类写成 `multiply(self, factor)` 与 C 的
    /// `mypaint_matrix_multiply(transform, factor)` 语义完全相同。
    pub fn multiply(m1: Transform, m2: Transform) -> Transform {
        let mut out = Transform::UNIT;
        for row in 0..3 {
            for col in 0..3 {
                out.rows[row][col] = m1.rows[0][col] * m2.rows[row][0]
                    + m1.rows[1][col] * m2.rows[row][1]
                    + m1.rows[2][col] * m2.rows[row][2];
            }
        }
        out
    }

    /// 顺时针旋转（弧度）—— `mypaint_transform_rotate_cw`
    /// （`mypaint-matrix.c:42-48`）。
    pub fn rotate_cw(self, angle_radians: f32) -> Transform {
        let a = angle_radians;
        let factor = Transform {
            rows: [
                [a.cos(), a.sin(), 0.0],
                [-a.sin(), a.cos(), 0.0],
                [0.0, 0.0, 1.0],
            ],
        };
        Transform::multiply(self, factor)
    }

    /// 关于过原点、方向角为 `angle_radians` 的直线反射 ——
    /// `mypaint_transform_reflect`（`mypaint-matrix.c:62-73`）。
    pub fn reflect(self, angle_radians: f32) -> Transform {
        let x = angle_radians.cos();
        let y = angle_radians.sin();
        let factor = Transform {
            rows: [
                [x * x - y * y, 2.0 * x * y, 0.0],
                [2.0 * x * y, y * y - x * x, 0.0],
                [0.0, 0.0, 1.0],
            ],
        };
        Transform::multiply(self, factor)
    }

    /// 平移 —— `mypaint_transform_translate`（`mypaint-matrix.c:76-85`）。
    pub fn translate(self, x: f32, y: f32) -> Transform {
        let factor = Transform {
            rows: [[1.0, 0.0, x], [0.0, 1.0, y], [0.0, 0.0, 1.0]],
        };
        Transform::multiply(self, factor)
    }

    /// 变换一个点 —— `mypaint_transform_point`（`mypaint-matrix.c:87-92`）。
    pub fn transform_point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.rows[0][0] * x + self.rows[0][1] * y + self.rows[0][2],
            self.rows[1][0] * x + self.rows[1][1] * y + self.rows[1][2],
        )
    }
}

/// 对称运行数据 —— `MyPaintSymmetryData`（`mypaint-symmetry.h:65-72`）。
///
/// 字段与 C 结构体一一对应，只有 `symmetry_matrices` + `num_symmetry_matrices`
/// 合并成 [`SymmetryData::matrices`]（`Vec` 的 `len()` 就是 C 的
/// `num_symmetry_matrices`，`Arc` 让 `draw_dab` 热路径能"一边拿着矩阵一边
/// 可变借用表面"，见 [`PigmentSurface::draw_dab_with_symmetry`]）。
#[derive(Clone, Debug)]
pub struct SymmetryData {
    /// 当前生效的状态（矩阵就是按它算的）。
    pub state_current: SymmetryState,
    /// 待生效的状态（`set_symmetry_state` 写这里）。
    pub state_pending: SymmetryState,
    /// 是否有待重算的改动（`mypaint-symmetry.h:68`）。
    pub pending_changes: bool,
    /// 对称是否开启（`mypaint-symmetry.h:69`）。注意它**不在** state 里：
    /// 源码允许"改了参数但没开"。
    pub active: bool,
    /// 对称变换矩阵（C 的 `symmetry_matrices` / `num_symmetry_matrices`）。
    pub matrices: Arc<Vec<Transform>>,
}

impl Default for SymmetryData {
    /// `mypaint_default_symmetry_data()`（`mypaint-symmetry.c:123-138`）。
    ///
    /// 源码把 `state_current.type` 初始化成 `-1`（无效类型），于是
    /// `mypaint_update_symmetry_state` 必定重算一遍，把默认状态的矩阵算好。
    /// Rust 的 [`SymmetryType`] 没有"无效值"，所以这里直接调 [`Self::recompute`]
    /// 算出同一份结果。
    fn default() -> Self {
        let mut data = Self {
            state_current: SymmetryState::default(),
            state_pending: SymmetryState::default(),
            pending_changes: true,
            active: false,
            matrices: Arc::new(Vec::new()),
        };
        data.recompute();
        data
    }
}

impl SymmetryData {
    /// 设置待生效的对称状态 —— `mypaint_symmetry_set_pending`
    /// （`mypaint-symmetry.c:148-161`）。
    ///
    /// 与源码一致：**只写 pending**，矩阵下次 `begin_atomic` 才算。
    /// 参数顺序也照源码（`angle` 在 `type` 之前）。
    pub fn set_pending(
        &mut self,
        active: bool,
        center_x: f32,
        center_y: f32,
        symmetry_angle: f32,
        symmetry_type: SymmetryType,
        rot_symmetry_lines: i32,
    ) {
        self.active = active;
        self.state_pending.center_x = center_x;
        self.state_pending.center_y = center_y;
        self.state_pending.type_ = symmetry_type;
        // C: `data->state_pending.num_lines = MAX(2, rot_symmetry_lines);`（`:157`）
        self.state_pending.num_lines = 2.0f32.max(rot_symmetry_lines as f32);
        self.state_pending.angle = symmetry_angle;
        self.pending_changes = true;
    }

    /// 必要时重算矩阵 —— `mypaint_update_symmetry_state`
    /// （`mypaint-symmetry.c:66-121`）。
    pub fn update(&mut self) {
        // C: `if (!self->pending_changes || symmetry_states_equal(...)) return;`
        if !self.pending_changes || self.state_current == self.state_pending {
            return;
        }
        self.recompute();
    }

    /// [`Self::update`] 去掉"要不要算"的判断那一段（源码里同一函数的函数体）。
    fn recompute(&mut self) {
        let symm = self.state_pending;
        let required = num_matrices_required(&symm);
        let cx = symm.center_x;
        let cy = symm.center_y;
        // C: `float angle = symm.angle * (M_PI / 180.0);`（`:83`）
        let angle = symm.angle * (PI / 180.0);
        // C: `float rot_angle = (2.0 * M_PI) / symm.num_lines;`（`:84`）
        let rot_angle = (2.0 * PI) / symm.num_lines;
        // C 的循环变量是 int、上界是 float（`i < symm.num_lines`）；num_lines
        // 恒为 ≥2 的整数（见 `set_pending`），所以截断成 usize 是等价的。
        let lines = symm.num_lines.max(0.0) as usize;
        // C: `const MyPaintTransform m = mypaint_transform_translate(
        //         mypaint_transform_unit(), -cx, -cy);`（`:86`）
        let m = Transform::UNIT.translate(-cx, -cy);
        // 源码的矩阵数组是"容量 ≥ required"，只有前 required 个会被写/读；
        // 容量下限是 DEFAULT_NUM_MATRICES（`:74-77` 的扩容分支）。
        let mut matrices = vec![Transform::UNIT; required.max(DEFAULT_NUM_MATRICES)];

        match symm.type_ {
            // 垂直/水平镜像（`:88-94`）：一个反射矩阵，垂直轴等于把角度再加 90°。
            SymmetryType::Vertical | SymmetryType::Horizontal => {
                let mut a = angle;
                if symm.type_ == SymmetryType::Vertical {
                    a += PI / 2.0;
                }
                matrices[0] = m.reflect(-a);
            }
            // 水平 + 垂直（`:95-100`）：三次反射串起来。
            SymmetryType::VertHorz => {
                let v_angle = angle + PI / 2.0;
                matrices[0] = m.reflect(-angle);
                matrices[1] = matrices[0].reflect(-v_angle);
                matrices[2] = matrices[1].reflect(-angle);
            }
            // 雪花（`:101-107`）**故意不 break**，继续走下面的旋转分支（`:108`）：
            // 先写 num_lines 个"旋转 + 反射"到 `num_lines-1` 起始的位置，
            // 再写前 num_lines-1 个纯旋转，总数正好 2*num_lines-1。
            SymmetryType::Snowflake => {
                let base_idx = lines.saturating_sub(1);
                for i in 0..lines {
                    matrices[base_idx + i] = m
                        .rotate_cw(rot_angle * i as f32)
                        .reflect(-(i as f32) * rot_angle - angle);
                }
                for i in 1..lines {
                    matrices[i - 1] = m.rotate_cw(rot_angle * i as f32);
                }
            }
            // 旋转（`:108-112`）：num_lines-1 个顺时针旋转矩阵。
            SymmetryType::Rotational => {
                for i in 1..lines {
                    matrices[i - 1] = m.rotate_cw(rot_angle * i as f32);
                }
            }
        }

        // `:117-119`：把所有矩阵从"以中心为原点"平移回去。
        for t in matrices.iter_mut().take(required) {
            *t = t.translate(cx, cy);
        }

        // C 里 `state_current` 在算之前就赋了（`:79`），保持一致。
        self.state_current = symm;
        self.matrices = Arc::new(matrices);
        self.pending_changes = false;
    }
}

/// 一个对称矩阵要对应几个 dab —— `num_matrices_required`
/// （`mypaint-symmetry.c:38-54`）。
fn num_matrices_required(state: &SymmetryState) -> usize {
    // C 里 num_lines 是 float，`state->num_lines - 1` 之后隐式转 int；
    // 恒为整数（见 `set_pending`），截断即可。
    let lines = state.num_lines.max(0.0) as usize;
    match state.type_ {
        SymmetryType::Vertical | SymmetryType::Horizontal => 1,
        SymmetryType::VertHorz => 3,
        SymmetryType::Rotational => lines.saturating_sub(1),
        SymmetryType::Snowflake => lines.saturating_mul(2).saturating_sub(1),
    }
}

/// 一个展开出来的对称 dab 的几何（位置 + 角度）。
///
/// 这是 [`for_each_symmetry_dab`] 的输出；其余字段（半径、长宽比、颜色、
/// 不透明度……）照抄原 dab —— 见源码 `DDI(x_out, y_out, angle, bb_idx)`
/// 那个宏（`mypaint-tiled-surface.c:705-708`）：它只替换 `x`/`y`/`angle`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SymmetricDab {
    pub x: f32,
    pub y: f32,
    /// 单位：度（与输入的 [`Dab::angle`] 一致）。
    pub angle: f32,
}

/// 一次 dab 在对称状态下要额外画哪些 dab —— `mypaint-tiled-surface.c:715-796`。
///
/// 只遍历**额外的**那些（不含原始 dab），顺序与源码 switch 的 DDI 调用顺序一致。
/// 每种类型的角度公式都是逐字抄的（见各分支注释）。
///
/// 调用者要保证 `matrices` 至少是 `num_matrices_required(state)` 长度
/// （[`SymmetryData::update`] 之后自然成立）；不足时按源码"读越界"的场合
/// 保守跳过对应项。
pub fn for_each_symmetry_dab(
    state: SymmetryState,
    matrices: &[Transform],
    dab: &Dab,
    mut f: impl FnMut(SymmetricDab),
) {
    if matrices.is_empty() {
        return;
    }
    // C: `const float rot_angle = 360.0 / symm.num_lines;`（`:725`）
    // （`:724` 的 `num_bboxes` 只服务于脏矩形分配，本项目没有对应物 —— 见模块文档）
    let rot_angle = 360.0f32 / state.num_lines;
    let lines = state.num_lines.max(0.0) as usize;

    match state.type_ {
        // 垂直镜像（`:730-735`）
        SymmetryType::Vertical => {
            let (x, y) = matrices[0].transform_point(dab.x, dab.y);
            f(SymmetricDab {
                x,
                y,
                angle: -2.0 * (90.0 + state.angle) - dab.angle,
            });
        }
        // 水平镜像（`:736-741`）
        SymmetryType::Horizontal => {
            let (x, y) = matrices[0].transform_point(dab.x, dab.y);
            f(SymmetricDab {
                x,
                y,
                angle: -2.0 * state.angle - dab.angle,
            });
        }
        // 水平 + 垂直（`:742-754`）：先跨水平线、再跨垂直线（对角）、最后跨回水平线。
        SymmetryType::VertHorz => {
            let mirror_angle = -2.0 * state.angle - dab.angle;
            let (x, y) = matrices[0].transform_point(dab.x, dab.y);
            f(SymmetricDab {
                x,
                y,
                angle: mirror_angle,
            });
            let (x, y) = matrices[1].transform_point(dab.x, dab.y);
            f(SymmetricDab {
                x,
                y,
                angle: dab.angle,
            });
            let (x, y) = matrices[2].transform_point(dab.x, dab.y);
            f(SymmetricDab {
                x,
                y,
                angle: mirror_angle,
            });
        }
        // 雪花（`:755-772`）：先画 num_lines 个"旋转 + 反射"的 dab，
        // 然后注释里写着 "fall through to rotational to finish the process"，
        // 也就是继续画 num_lines-1 个纯旋转 dab。
        SymmetryType::Snowflake => {
            let base_idx = lines.saturating_sub(1);
            let base_angle = -2.0 * state.angle - dab.angle;
            for dab_count in 0..lines {
                let Some(t) = matrices.get(base_idx + dab_count) else {
                    continue;
                };
                let (x, y) = t.transform_point(dab.x, dab.y);
                f(SymmetricDab {
                    x,
                    y,
                    // C: `base_angle - dab_count * rot_angle`
                    angle: base_angle - dab_count as f32 * rot_angle,
                });
            }
            for dab_count in 1..lines {
                let Some(t) = matrices.get(dab_count - 1) else {
                    continue;
                };
                let (x, y) = t.transform_point(dab.x, dab.y);
                f(SymmetricDab {
                    x,
                    y,
                    // C: `angle - dab_count * rot_angle`
                    angle: dab.angle - dab_count as f32 * rot_angle,
                });
            }
        }
        // 旋转（`:773-791`）：从 1 数到 num_lines-1（第 0 份由原始 pass 画了）。
        SymmetryType::Rotational => {
            for dab_count in 1..lines {
                let Some(t) = matrices.get(dab_count - 1) else {
                    continue;
                };
                let (x, y) = t.transform_point(dab.x, dab.y);
                f(SymmetricDab {
                    x,
                    y,
                    angle: dab.angle - dab_count as f32 * rot_angle,
                });
            }
        }
    }
}

/// 便利包装：把对称 dab 收进 `Vec`（测试与调试用；热路径走
/// [`for_each_symmetry_dab`] 以避免每次分配）。
pub fn symmetry_dabs(state: SymmetryState, matrices: &[Transform], dab: &Dab) -> Vec<SymmetricDab> {
    let mut out = Vec::new();
    for_each_symmetry_dab(state, matrices, dab, |d| out.push(d));
    out
}

/// 源码 `draw_dab_internal` 的早期返回判据（`mypaint-tiled-surface.c:642-645`）：
///
/// ```c
/// if (op->radius < 0.1f) return FALSE;   // 小于 0.1 像素的 dab 不画
/// if (op->hardness == 0.0f) return FALSE;
/// if (op->softness == 1.0f) return FALSE;
/// if (op->opaque == 0.0f) return FALSE;
/// ```
///
/// 它同时是 `draw_dab` 里决定"要不要做对称 pass"的那个 `surface_modified`
/// （`:711`）。`softness` 那条查不了：hokusai 的 [`Dab`] 没有这个字段。
fn dab_is_drawable(dab: &Dab) -> bool {
    dab.radius >= 0.1 && dab.hardness != 0.0 && dab.opaque != 0.0
}

impl PigmentSurface {
    /// 设置对称状态 —— `mypaint_tiled_surface_set_symmetry_state`
    /// （`mypaint-tiled-surface.c:192-201`）。
    ///
    /// 参数顺序照 C++ 包装层 `TiledSurface::set_symmetry_state`
    /// （`lib/tiledsurface.hpp:57-68`）与 Python 侧 `data.py:505-511` 的调用：
    /// `(active, center_x, center_y, symmetry_type, rot_symmetry_lines, angle)`。
    ///
    /// 只写 pending，矩阵在下一次 [`PigmentSurface::begin_atomic`] 时才算
    /// （源码注释见模块文档第 1 条）。
    pub fn set_symmetry_state(
        &mut self,
        active: bool,
        center_x: f32,
        center_y: f32,
        symmetry_type: SymmetryType,
        rot_symmetry_lines: i32,
        symmetry_angle: f32,
    ) {
        self.symmetry.set_pending(
            active,
            center_x,
            center_y,
            symmetry_angle,
            symmetry_type,
            rot_symmetry_lines,
        );
    }

    /// 当前生效的对称参数（C 的 `symmetry_data.state_current`）。
    #[inline]
    pub fn symmetry_state(&self) -> SymmetryState {
        self.symmetry.state_current
    }

    /// 待生效的对称参数（C 的 `symmetry_data.state_pending`）。
    #[inline]
    pub fn symmetry_pending(&self) -> SymmetryState {
        self.symmetry.state_pending
    }

    /// 对称是否开启（C 的 `symmetry_data.active`）。
    #[inline]
    pub fn symmetry_active(&self) -> bool {
        self.symmetry.active
    }

    /// 已算好的矩阵数（C 的 `num_symmetry_matrices`，是容量而非"用到的个数"）。
    #[inline]
    pub fn symmetry_matrix_count(&self) -> usize {
        self.symmetry.matrices.len()
    }

    /// 对称矩阵（只读；测试与诊断用）。
    #[inline]
    pub fn symmetry_matrices(&self) -> &[Transform] {
        &self.symmetry.matrices
    }

    /// 一次 `draw_dab`：原始 dab + 对称 pass（`mypaint-tiled-surface.c:690-800`）。
    ///
    /// 返回值：源码只返回基础 pass 的 `surface_modified`（`:711,798`），
    /// 但 hokusai 的 `TiledSurface::draw_dab` 契约是"是否改动了像素"，
    /// 所以这里把两个 pass 的结果 OR 起来 —— 源码注释也说明引入选区遮罩后
    /// 必须这样改（`:717-720`）。
    pub(crate) fn draw_dab_with_symmetry(&mut self, dab: &Dab) -> bool {
        // 基础 pass（`:711` `gboolean surface_modified = DDI(x, y, angle, 0);`）
        let base_modified = draw_dab_default(self, dab);

        // 对称 pass（`:715-796`）。`surface_modified` 用源码自己的判据
        // （见 [`dab_is_drawable`]），不是 `base_modified`。
        let drawable = dab_is_drawable(dab);
        let mut symmetry_modified = false;
        if drawable && self.symmetry.active && !self.symmetry.matrices.is_empty() {
            // 矩阵是 `Arc`：拷一份引用计数就能"一边读矩阵、一边可变借用表面"。
            let state = self.symmetry.state_current;
            let matrices = Arc::clone(&self.symmetry.matrices);
            let base = *dab;
            for_each_symmetry_dab(state, &matrices, dab, |d| {
                // 照 DDI 宏：只有 x/y/angle 变，其余字段原样
                // （`mypaint-tiled-surface.c:705-708`）。
                let mirrored = Dab {
                    x: d.x,
                    y: d.y,
                    angle: d.angle,
                    ..base
                };
                symmetry_modified |= draw_dab_default(self, &mirrored);
            });
        }

        base_modified || symmetry_modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hokusai::color::RgbaF32;
    use hokusai::TiledSurface;

    /// 一个几何参数都明确的测试 dab（颜色无关紧要，这里取纯红）。
    pub(super) fn test_dab(x: f32, y: f32, angle: f32) -> Dab {
        Dab {
            x,
            y,
            radius: 6.0,
            color: RgbaF32 {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            opaque: 1.0,
            hardness: 0.9,
            alpha_eraser: 0.0,
            aspect_ratio: 1.0,
            angle,
            lock_alpha: 0.0,
            colorize: 0.0,
            posterize: 0.0,
            posterize_num: 0.0,
            paint: 1.0,
            anti_aliasing: 0.0,
        }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// 默认数据照 `mypaint_default_symmetry_data`：垂直轴、原点、2 条线、未激活，
    /// 且矩阵已经算好一份（16 个槽位）。
    #[test]
    fn default_data_matches_source() {
        let data = SymmetryData::default();
        assert_eq!(data.state_current, SymmetryState::default());
        assert_eq!(data.state_pending, SymmetryState::default());
        assert!(!data.active);
        assert!(!data.pending_changes);
        assert_eq!(data.matrices.len(), DEFAULT_NUM_MATRICES);

        // 垂直轴（过原点）镜像：x → -x，y 不变
        let (x, y) = data.matrices[0].transform_point(30.0, 7.0);
        assert!(approx(x, -30.0) && approx(y, 7.0));
    }

    /// `num_matrices_required`（`mypaint-symmetry.c:38-54`）：1 / 1 / 3 / n-1 / 2n-1。
    #[test]
    fn num_matrices_required_matches_source() {
        let base = SymmetryState::default();
        let of = |type_: SymmetryType, lines: f32| {
            num_matrices_required(&SymmetryState {
                type_,
                num_lines: lines,
                ..base
            })
        };
        assert_eq!(of(SymmetryType::Vertical, 2.0), 1);
        assert_eq!(of(SymmetryType::Horizontal, 2.0), 1);
        assert_eq!(of(SymmetryType::VertHorz, 2.0), 3);
        assert_eq!(of(SymmetryType::Rotational, 5.0), 4);
        assert_eq!(of(SymmetryType::Snowflake, 5.0), 9);
    }

    /// `set_pending` 只写 pending、并把线数夹到 ≥2（`mypaint-symmetry.c:148-161`）。
    #[test]
    fn set_pending_writes_pending_and_clamps_lines() {
        let mut data = SymmetryData::default();
        data.set_pending(true, 10.0, 20.0, 30.0, SymmetryType::Rotational, 1);
        assert!(data.active);
        assert!(data.pending_changes);
        assert_eq!(data.state_pending.num_lines, 2.0, "MAX(2, 1)");
        assert_eq!(data.state_pending.center_x, 10.0);
        assert_eq!(data.state_pending.angle, 30.0);
        // 还没 update：current 仍是默认值
        assert_eq!(data.state_current, SymmetryState::default());
    }

    /// 只有 `pending_changes` 且状态真的变了才重算（`:70`）；重复设同一份状态
    /// 不该重算矩阵。
    #[test]
    fn update_is_skipped_when_state_is_unchanged() {
        let mut data = SymmetryData::default();
        data.set_pending(true, 5.0, 5.0, 0.0, SymmetryType::Horizontal, 2);
        data.update();
        assert!(!data.pending_changes);
        assert_eq!(data.state_current.center_x, 5.0);
        let ptr = Arc::as_ptr(&data.matrices);

        // 再设一次一模一样的：pending_changes 又是 true，但状态相等 → 不重算
        data.set_pending(true, 5.0, 5.0, 0.0, SymmetryType::Horizontal, 2);
        data.update();
        assert_eq!(Arc::as_ptr(&data.matrices), ptr, "状态没变就该复用同一份矩阵");
        // pending_changes 保持 true（源码这里提前 return，没清标志）
        assert!(data.pending_changes);
    }

    /// 水平镜像的角度公式 `-2*angle - dab_angle`（`:738`），位置关于 y=cy 对称。
    #[test]
    fn horizontal_mirror_formula() {
        let mut data = SymmetryData::default();
        data.set_pending(true, 10.0, 50.0, 0.0, SymmetryType::Horizontal, 2);
        data.update();
        let dab = test_dab(30.0, 20.0, 25.0);
        let out = symmetry_dabs(data.state_current, &data.matrices, &dab);
        assert_eq!(out.len(), 1);
        assert!(approx(out[0].x, 30.0), "水平镜像不动 x：{}", out[0].x);
        assert!(approx(out[0].y, 80.0), "y 关于 y=50 对称：{}", out[0].y);
        assert!(approx(out[0].angle, -25.0), "角度：{}", out[0].angle);
    }

    /// 垂直镜像的角度公式 `-2*(90+angle) - dab_angle`（`:732`），位置关于 x=cx 对称。
    #[test]
    fn vertical_mirror_formula() {
        let mut data = SymmetryData::default();
        data.set_pending(true, 10.0, 50.0, 0.0, SymmetryType::Vertical, 2);
        data.update();
        let dab = test_dab(30.0, 20.0, 25.0);
        let out = symmetry_dabs(data.state_current, &data.matrices, &dab);
        assert_eq!(out.len(), 1);
        assert!(approx(out[0].x, -10.0), "x 关于 x=10 对称：{}", out[0].x);
        assert!(approx(out[0].y, 20.0), "垂直镜像不动 y：{}", out[0].y);
        // -2*(90+0) - 25 = -205
        assert!(approx(out[0].angle, -205.0), "角度：{}", out[0].angle);
        // 椭圆笔尖每 180° 自重合，所以 -205° 与 -25° 是同一个朝向
        assert!(approx((out[0].angle + 25.0).rem_euclid(180.0), 0.0));
    }

    /// 旋转对称展开成 N-1 个 dab，位置绕中心均匀分布（`:773-791`）。
    #[test]
    fn rotational_expands_to_n_minus_one() {
        let mut data = SymmetryData::default();
        data.set_pending(true, 100.0, 100.0, 0.0, SymmetryType::Rotational, 4);
        data.update();
        let dab = test_dab(120.0, 100.0, 0.0); // 中心右侧 20px
        let out = symmetry_dabs(data.state_current, &data.matrices, &dab);
        assert_eq!(out.len(), 3, "4 条线 → 额外 3 个 dab");
        // 顺时针旋转（模型坐标 y 向下）：右 → 上 → 左 → 下
        let expect = [(100.0, 80.0), (80.0, 100.0), (100.0, 120.0)];
        for (i, (ex, ey)) in expect.iter().enumerate() {
            assert!(
                approx(out[i].x, *ex) && approx(out[i].y, *ey),
                "第 {i} 个 dab 应在 ({ex},{ey})，实得 ({},{})",
                out[i].x,
                out[i].y
            );
            // 角度每份减 360/N = 90
            assert!(approx(out[i].angle, -90.0 * (i as f32 + 1.0)));
        }
    }

    /// 雪花 = num_lines 个反射 + num_lines-1 个旋转（`:101-112` 的 fall-through）。
    #[test]
    fn snowflake_expands_to_two_n_minus_one() {
        let mut data = SymmetryData::default();
        data.set_pending(true, 0.0, 0.0, 0.0, SymmetryType::Snowflake, 3);
        data.update();
        assert_eq!(num_matrices_required(&data.state_current), 5);
        let dab = test_dab(10.0, 0.0, 0.0);
        let out = symmetry_dabs(data.state_current, &data.matrices, &dab);
        assert_eq!(out.len(), 5, "3 条线 → 3 反射 + 2 旋转");
        // 前 3 个是反射（第 0 个就是跨 x 轴镜像：y → -y）
        assert!(approx(out[0].x, 10.0) && approx(out[0].y, 0.0));
        // 后 2 个是 120° / 240° 旋转
        assert!(approx(out[3].x, -5.0) && approx(out[3].y, -8.660254), "{:?}", out[3]);
        assert!(approx(out[4].x, -5.0) && approx(out[4].y, 8.660254), "{:?}", out[4]);
    }

    /// `symmetry_angle` 把轴整体转过去：90° 让垂直镜像变成水平镜像。
    #[test]
    fn symmetry_angle_rotates_the_axis() {
        let mut data = SymmetryData::default();
        data.set_pending(true, 0.0, 0.0, 90.0, SymmetryType::Vertical, 2);
        data.update();
        let dab = test_dab(0.0, 10.0, 0.0);
        let out = symmetry_dabs(data.state_current, &data.matrices, &dab);
        // 轴转 90° 后垂直镜像 = 原来的水平镜像：y → -y
        assert!(approx(out[0].x, 0.0) && approx(out[0].y, -10.0), "{:?}", out[0]);
    }

    /// 表面 API：`set_symmetry_state` 在 `begin_atomic` 之前不改变 current，
    /// 之后才生效（`mypaint-tiled-surface.c:94-99`）。
    #[test]
    fn surface_applies_pending_state_at_begin_atomic() {
        let mut surface = PigmentSurface::new();
        assert!(!surface.symmetry_active());
        surface.set_symmetry_state(true, 40.0, 0.0, SymmetryType::Horizontal, 3, 0.0);
        assert!(surface.symmetry_active());
        assert_eq!(surface.symmetry_state().center_x, 0.0, "还没 begin_atomic");
        surface.begin_atomic();
        assert_eq!(surface.symmetry_state().center_x, 40.0);
        assert_eq!(surface.symmetry_matrix_count(), DEFAULT_NUM_MATRICES);
        surface.end_atomic();
    }
}
