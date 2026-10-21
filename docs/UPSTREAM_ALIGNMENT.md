# MyPaint 上游对齐移植事实报告

> 快照日期：2026-10-21
> 目标：以 100% 移植为目标的逐文件、逐代码功能对齐台账。
> 内容约束：本文件只记录**当前代码快照的对齐事实**。不记录修复轮次、迭代结论、性能数据、待办优先级或开发过程。
> 判定约束：`对应` 只表示找到了承担同一职责的本地实现，不等于逐位等价；逐位等价需由测试或逐像素对照单独证明。

目录：[基线](#0-基线) · [判定口径](#1-判定口径) · [总体结构](#2-移植总体结构) · [libmypaint](#3-libmypaint-逐文件--逐函数) · [lib/ Python](#4-mypaint-libpython逐文件) · [lib/ C/C++](#5-mypaint-lib-原生-cc-扩展逐文件) · [gui/](#6-mypaint-guipythongtk逐文件) · [资源](#7-上游数据与资源) · [不等价事实](#8-当前不等价事实) · [验证](#9-验证) · [统计](#10-覆盖统计)

## 0. 基线

| 对象 | 本地路径 | 版本 / 提交 | 工作树状态 |
|---|---|---|---|
| 上游 MyPaint | `_mypaint_ref` | `fb6f7d6b12238d125294496b105f5f36e658bc61`，`v2.1.0-alpha-627-gfb6f7d6b` | 干净 |
| 上游 libmypaint | `_libmypaint_ref` | `d5a88fbe6649d5ec776bc42ec8c1f4bb29d7fd7f`（master） | 干净 |
| 上游画笔包 | `mypaint-brushes` | `08da4a48a4b63f2e0f3303942d7c21da8479ea9f`，196 支 `.myb` | 干净 |
| 移植引擎 fork | `hokusai` | HEAD `f7e998173c0e7427b95afe0b6947e3103da60f00`（v0.3.0） | 有未提交修改 |
| 移植应用 | `pigment` | HEAD `d9d7c7bfb5b068dbbd2afd3ffb8820289ab0ce58` | 有未提交修改 |

本报告描述的是 `pigment` 与 `hokusai` 的**工作树当前内容**，不是上述提交本身。上游引用均以两个 `_ref` 目录的当前检出为准。

## 1. 判定口径

| 状态 | 含义 |
|---|---|
| 对应 | 上游符号或行为在本地有明确落点；未发现该符号承载的功能缺失。 |
| 部分 | 本地有实现，但上游文件内至少一个符号/行为没有落点；缺失项逐条列出。 |
| 缺失 | 上游符号承载的可执行功能在本地没有找到落点；附检索证据。 |
| 结构性替代 | 上游代码属于 GUI 工具包、Python 运行时、SWIG/GLib 绑定或语言兼容层；本地由 egui/Rust/std 直接承担，不存在 1:1 符号。 |
| 不适用 | 上游的可选后端、示例、构建/测试辅助文件，不属于应用功能。 |

粒度：

- `libmypaint`：按头文件公开 API + 承载语义的内部函数逐条对照。
- MyPaint `lib/` Python：按文件、类、顶层函数对照；核心类（`Document`、`BrushInfo`、`Stroke`、`Palette`、`StrokeShape`、图层类）补关键方法。
- MyPaint `lib/` C/C++：按文件、类/函数对照。
- MyPaint `gui/` Python：按文件和行为单元（类/顶层函数）对照。GTK 控件方法不逐方法对照，因为本地 UI 栈是 egui，控件层不存在一一对应；用户可见行为仍逐项记录。

证据写法：本地路径均相对 `(workspace root)`；“本地落点”列给出文件、符号或行号；缺失项给出实际检索词。

## 2. 移植总体结构

| 本地 crate / 目录 | 职责 | 主要上游来源 |
|---|---|---|
| `hokusai/crates/hokusai-core` | 笔刷求值、dab 光栅化、tile surface 抽象、RNG、光谱混色、对称 | `libmypaint` 核心 |
| `hokusai/crates/hokusai-brush` | `.myb` v3 JSON 读写与默认值 | `libmypaint` + MyPaint `lib/brush.py` 的序列化契约 |
| `hokusai/crates/hokusai-compat` | 笔画快照回归（golden 由 hokusai 自身生成，见 9.1） | `libmypaint` 测试工具 |
| `hokusai/crates/hokusai-tile-mem` | 内存 tile 后端 | `mypaint-fixed-tiled-surface.c` |
| `pigment/crates/pigment-surface` | 无限画布 tile、图层合成、mipmap、strokemap、对称、PNG、矢量层 | `lib/tiledsurface.py`、`lib/layer/*`、`lib/pixops.cpp`、`lib/fill/*`、`mypaint-tiled-surface.c` |
| `pigment/crates/pigment-engine` | 文档模型、命令栈、模式、颜色、文件 I/O、自动保存 | `lib/document.py`、`lib/command.py`、`gui/mode.py` 及模式文件、`lib/color.py`、`lib/palette.py` |
| `pigment/crates/pigment-render` | 与 GUI 无关的 wgpu 画布、mipmap、GPU dab、显示滤镜 | `gui/tileddrawwidget.py`、`gui/displayfilter.py` 的渲染职责 |
| `pigment/crates/pigment-app` | egui/winit 外壳、面板、对话框、输入、动作 | 整个 `gui/` |
| `pigment/crates/tablet-probe` | 数位板输入探测 | `gui/inputtestwindow.py` 的输入诊断职责 |

---

## 3. libmypaint 逐文件 / 逐函数

### 3.1 文件映射总表

| 上游文件 | 本地落点 | 状态 | C 门禁验收 |
|---|---|---|---|
| `mypaint-brush.c` / `.h` | `hokusai-core/src/{brush,state,stroke,evaluator}.rs` | 对应 | 逐位一致\* |
| `mypaint-brush-settings.c` / `.h` + `brushsettings.json` | `hokusai-core/src/{setting,input}.rs`、`hokusai-brush/src/lib.rs` | 对应 | 逐位一致\* |
| `mypaint-mapping.c` / `.h` | `hokusai-core/src/mapping.rs` | 对应 | 逐位一致\* |
| `brushmodes.c` / `.h` | `hokusai-core/src/brushmodes.rs`、`spectral.rs` | 对应 | 逐位一致\*（CPU） |
| `helpers.c` / `.h` | `hokusai-core/src/{color,spectral,stroke,rng}.rs` | 对应 | 部分覆盖（门禁用到的路径） |
| `rng-double.c` / `.h` | `hokusai-core/src/rng.rs` | 对应 | 逐位一致\* |
| `mypaint-symmetry.c` / `.h` | `pigment-surface/src/symmetry.rs` | 对应 | 未覆盖 |
| `mypaint-tiled-surface.c` / `.h` | `hokusai-core/src/surface.rs`、`pigment-surface/src/lib.rs` | 对应 | 部分覆盖（dab/采样 CPU 路径） |
| `mypaint-surface.c` / `.h` | `hokusai-core/src/surface.rs`、`pigment-surface/src/lib.rs` | 对应 | 部分覆盖（draw_dab/get_color） |
| `mypaint-fixed-tiled-surface.c` / `.h` | `hokusai-tile-mem/src/lib.rs`、`pigment-surface/src/lib.rs` | 对应 | 未覆盖 |
| `tilemap.c` / `.h` | `pigment-surface/src/lib.rs` 的 tile 表 + `mipmap.rs` | 结构性替代 | 间接覆盖（CPU 合成结果） |
| `fifo.c` / `.h` | `pigment-surface/src/lib.rs` 的 `queue_dab` / `flush`；Rust `VecDeque` | 结构性替代 | 间接覆盖（CPU 合成结果） |
| `operationqueue.c` / `.h` | `pigment-surface/src/lib.rs` 的 `pending_ops` / `flush` / GPU sink | 结构性替代 | 间接覆盖（CPU 合成结果） |
| `mypaint-matrix.c` / `.h` | `pigment-surface/src/symmetry.rs` 的 `Transform` | 对应 | 未覆盖 |
| `mypaint-rectangle.c` / `.h` | 元组 `(i32,i32,i32,i32)` + `pigment-engine/src/document.rs:1595 expand_to_include_point` | 结构性替代 | 未覆盖 |
| `mypaint.c` / `mypaint-config.h` | `Cargo.toml` 版本与 feature | 结构性替代 | 不适用 |
| `fastapprox/*.h` | `hokusai-core/src/spectral.rs` 的 `fastlog2` / `fastpow2` / `fastpow` | 对应 | 逐位一致\* |
| `gegl/*` | 无 | 不适用（GEGL 后端不在移植目标内） | 不适用 |
| `glib/*` | 无 | 不适用（GObject 包装层由 Rust API 取代） | 不适用 |
| `tests/*`、`examples/*` | `hokusai-compat/tests`、`pigment-* /tests` | 不适用（测试载体不同） | 不适用 |

\* “逐位一致”指在 `hokusai-compat/cref` 的 C 差分门禁中，196 支官方笔刷有 194 支、9/9 fixture 的最终 fix15 画布逐位相同；逐事件状态在抽检笔刷上一致。剩余 2 支及覆盖边界见第 9.2、9.5 节。

### 3.2 `mypaint-brush.c` / `mypaint-brush.h`

| 上游符号 | 上游位置 | 本地落点 | 状态 | 事实 |
|---|---|---|---|---|
| `mypaint_brush_new` | `mypaint-brush.h` | `hokusai-core/src/brush.rs:33 Brush::new` | 对应 | `Brush` 为纯值类型。 |
| `mypaint_brush_new_with_buckets` | `mypaint-brush.c:197` | `hokusai-core/src/state.rs:19 NUM_SMUDGE_BUCKETS`（固定 256） | 部分 | 本地没有可变 bucket 数量参数；MyPaint 自身固定 256（`lib/brush.hpp:26-27`）。 |
| `mypaint_brush_ref` / `unref` | `mypaint-brush.c` | Rust 所有权 | 结构性替代 | 无引用计数 API。 |
| `mypaint_brush_from_defaults` | `mypaint-brush.c` | `hokusai-brush/src/lib.rs:39 apply_libmypaint_defaults`、`pigment-engine/src/brushlib.rs:45 default_brush` | 对应 | 缺省设置与 `opaque_multiply` 压力曲线均安装。 |
| `mypaint_brush_from_string` | `mypaint-brush.c` | `hokusai-brush/src/lib.rs:125 from_str` | 对应 | 只接受 v3 JSON；v2 旧格式无落点。 |
| `mypaint_brush_get_base_value` | `mypaint-brush.c` | `hokusai-core/src/brush.rs:76` | 对应 | |
| `mypaint_brush_set_base_value` | `mypaint-brush.c` | `hokusai-core/src/brush.rs` 的 `set_base_value` | 对应 | |
| `mypaint_brush_is_constant` | `mypaint-brush.c` | `hokusai-core/src/brush.rs:86` | 对应 | |
| `mypaint_brush_get_inputs_used_n` | `mypaint-brush.c` | `hokusai-core/src/brush.rs:91` | 对应 | |
| `mypaint_brush_get_mapping_n` | `mypaint-brush.c` | `hokusai-core/src/brush.rs:100` | 对应 | |
| `mypaint_brush_set_mapping_n` | `mypaint-brush.c` | `hokusai-core/src/brush.rs:112` | 对应 | |
| `mypaint_brush_get_mapping_point` | `mypaint-brush.c` | `hokusai-core/src/brush.rs:133` | 对应 | |
| `mypaint_brush_set_mapping_point` | `mypaint-brush.c` | `hokusai-core/src/brush.rs:149` | 对应 | |
| `mypaint_brush_get_total_stroke_painting_time` | `mypaint-brush.c` | `hokusai-core/src/state.rs:113 stroke_total_painting_time` | 对应 | 以字段承载；`pigment-engine/src/stroke.rs:222` 读取。 |
| `mypaint_brush_get_state` | `mypaint-brush.c` | `hokusai-core/src/state.rs:320 get_state`（44 项 `BrushStateField`） | 对应 | 字段枚举顺序有测试 `state_enum_matches_libmypaint_count_and_order`。 |
| `mypaint_brush_set_state` | `mypaint-brush.c` | `hokusai-core/src/state.rs:377 set_state` | 对应 | |
| `mypaint_brush_get_smudge_bucket_state` | `mypaint-brush.c` | `hokusai-core/src/state.rs:465` | 对应 | 返回 `([f32;4],[f32;4],f32)`。 |
| `mypaint_brush_set_smudge_bucket_state` | `mypaint-brush.c` | `hokusai-core/src/state.rs:441` | 对应 | |
| `mypaint_brush_get_min_smudge_bucket_used` | `mypaint-brush.c:517` | `hokusai-core/src/state.rs:472` | 对应 | |
| `mypaint_brush_get_max_smudge_bucket_used` | `mypaint-brush.c:529` | `hokusai-core/src/state.rs:477` | 对应 | |
| `mypaint_brush_new_stroke` | `mypaint-brush.c` | `hokusai-core/src/state.rs:314 new_stroke` | 对应 | 只复位两个计时器，保留位置/RNG/bucket。 |
| `mypaint_brush_reset` | `mypaint-brush.c:304`（内部 `brush_reset` 在 `:145`） | `hokusai-core/src/state.rs:303 reset` | 对应 | 清状态与 smudge bucket，保留 RNG 流。 |
| `mypaint_brush_stroke_to` | `mypaint-brush.c:1300+` | `hokusai-core/src/stroke.rs:36 stroke_to`、`:62 stroke_to_full` | 对应 | 完整输入走 `stroke_to_full`（含 viewzoom/viewrotation/barrel/linear）。 |
| `mypaint_brush_set_print_inputs` | `mypaint-brush.c:667` | `hokusai-core/src/stroke.rs:1673 trace_inputs`（环境变量）+ `pigment-app/src/app.rs:3020` | 部分 | 调试输出存在，但不是 brush 对象上的开关 API。 |
| `settings_base_values_have_changed`（内部） | `mypaint-brush.c:139` | `hokusai-core/src/evaluator.rs:68 evaluate` 每事件重算 | 结构性替代 | 本地无脏标记，求值器每次从 `Brush` 重新计算全部设置。 |
| `update_states_and_setting_values`（内部） | `mypaint-brush.c:600-824` | `hokusai-core/src/stroke.rs` 事件主循环（约 `:542` 起） | 对应 | 输入归一化、曲线求和、STATE 写回均在同一循环内。 |
| `count_dabs_to`（内部） | `mypaint-brush.c:1279` | `hokusai-core/src/stroke.rs:1630 count_dabs_to` | 对应 | 直接读 `STATE(DABS_PER_*)`，保留 `isnan/<0 → 0`。 |
| `directional_offsets`（内部） | `mypaint-brush.c:614-657` | `hokusai-core/src/stroke.rs:1688 directional_offsets` | 对应 | |
| `update_smudge_color`（内部） | `mypaint-brush.c:854` | `hokusai-core/src/stroke.rs:1221 update_smudge_bucket_cpu` + `build_dab` | 对应 | CPU 路径；GPU 延迟采样见第 8 节。 |
| `prepare_and_draw_dab`（内部） | `mypaint-brush.c:1042` | `hokusai-core/src/stroke.rs` 的 `build_dab` / `draw_dab`（约 `:899-1004`） | 对应 | |

### 3.3 `mypaint-brush-settings.c` / `.h` + `brushsettings.json`

| 上游符号/数据 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| 65 个 setting 的 `id`/`name`/`default`/`min`/`max` | `hokusai-core/src/setting.rs`（`BrushSetting::ALL`，65 项） | 对应 | 测试 `num_settings_matches_all`、`roundtrip_all_cnames`。 |
| 18 个 input 的 `id`/`name`/`hard_min`/`soft_min`/`soft_max`/`hard_max` | `hokusai-core/src/input.rs`（18 项） | 对应 | `.myb` 键名逐字一致。 |
| `mypaint_brush_setting_info` 等内省结构 | 构建期生成的画笔元数据 + `setting.rs` | 结构性替代 | 没有 C 结构体 API；名称/顺序由枚举提供。 |
| `brushsettings.json` 文件本身 | 编译进二进制的 `BrushSetting` 枚举与默认值 | 对应 | 运行时不再读 JSON；数据等价由枚举测试覆盖。 |

### 3.4 `mypaint-mapping.c` / `.h`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `mypaint_mapping_new` / `free` | `hokusai-core/src/{brush,mapping}.rs` 的 `InputMapping` | 结构性替代 | Rust 值类型 + `Drop`。 |
| `mypaint_mapping_get_base_value` / `set_base_value` | `hokusai-core/src/brush.rs:76` 与 `set_base_value` | 对应 | |
| `mypaint_mapping_get_n` / `set_n` | `hokusai-core/src/brush.rs:100/112` | 对应 | |
| `mypaint_mapping_get_point` / `set_point` | `hokusai-core/src/brush.rs:133/149` | 对应 | |
| `mypaint_mapping_is_constant` | `hokusai-core/src/brush.rs:86` | 对应 | |
| `mypaint_mapping_calculate` | `hokusai-core/src/mapping.rs:27 eval` + `evaluator.rs:68` | 对应 | `base + Σ mapping`；`eval` 为分段线性。 |
| `mypaint_mapping_calculate_single_input` | `hokusai-core/src/mapping.rs:27 eval` | 对应 | 单输入路径即单条 `eval`。 |

### 3.5 `brushmodes.c` / `brushmodes.h`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `draw_dab_pixels_BlendMode_Normal` | `hokusai-core/src/brushmodes.rs:752 blend_normal` | 对应 | fix15、预乘 alpha。 |
| `draw_dab_pixels_BlendMode_Normal_Paint` | `brushmodes.rs:978 blend_normal_paint` | 对应 | 光谱 pigment 路径。 |
| `draw_dab_pixels_BlendMode_Posterize` | `brushmodes.rs:946 blend_posterize` | 对应 | |
| `draw_dab_pixels_BlendMode_Normal_and_Eraser` | `brushmodes.rs:773 blend_normal_and_eraser` | 对应 | |
| `draw_dab_pixels_BlendMode_Normal_and_Eraser_Paint` | `brushmodes.rs:1211 blend_normal_and_eraser_paint` | 对应 | |
| `draw_dab_pixels_BlendMode_LockAlpha` | `brushmodes.rs:795 blend_lock_alpha` | 对应 | |
| `draw_dab_pixels_BlendMode_LockAlpha_Paint` | `brushmodes.rs:816 blend_lock_alpha_paint` | 对应 | 不回写 alpha。 |
| `spectral_blend_factor` | `spectral.rs:234` | 对应 | |
| `get_color_pixels_legacy` | `brushmodes.rs:1709 get_color_default` | 对应 | 固定 `softness=0.5`（`brushmodes.rs:1389`）。 |
| `get_color_pixels_accumulate` | `brushmodes.rs:1788 get_color_pigment_default` | 对应 | 稀疏采样 + `c_rand` 镜像。 |
| `draw_dab_pixels_BlendMode_Color` | `brushmodes.rs:863 blend_color` | 对应 | 去预乘 → 保留目标亮度 → 重新预乘 → Normal 合成。 |

### 3.6 `helpers.c` / `helpers.h`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `rand_gauss` | `hokusai-core/src/rng.rs` 的 `next_gauss` | 对应 | 同 Box-Muller 常量。 |
| `mod_arith` | `hokusai-core/src/stroke.rs:1794 mod_arith_c` | 对应 | `a - N*floor(a/N)`。 |
| `smallest_angular_difference` | `hokusai-core/src/stroke.rs:1801` | 对应 | |
| `mix_colors` | `hokusai-core/src/spectral.rs:188 mix_colors` | 对应 | |
| `rgb_to_spectral` | `spectral.rs:152` | 对应 | 10 波段表。 |
| `spectral_to_rgb` | `spectral.rs:168` | 对应 | |
| `rgb_to_hsv_float` / `hsv_to_rgb_float` | `hokusai-core/src/color.rs:56/124` | 对应 | |
| `rgb_to_hsl_float` / `hsl_to_rgb_float` | `color.rs:74/98` | 对应 | |
| `rgb_to_hcy_float` / `hcy_to_rgb_float` | `pigment-engine/src/color/space.rs:121/159`、`mode/pick.rs:105/97` | 对应 | HCY 的完整轮子数学在 `color/hcywheel.rs`。 |

### 3.7 `rng-double.c` / `.h`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `rng_double_new` / `free` | `hokusai-core/src/rng.rs` 的 `BrushRng` | 结构性替代 | Rust 值类型。 |
| `rng_double_set_seed` | `rng.rs` 的 `set_seed` | 对应 | 默认种子 1000。 |
| `rng_double_next` | `rng.rs` 的 `next_unit_f64` | 对应 | KK=10 / LL=7 / TT=7。 |
| `rng_double_get_array` | `rng.rs` 的 `get_array` | 对应 | |
| `rng_double_next` 的 `rand()` 镜像 | `brushmodes.rs:1459 reset_get_color_rng`、`c_rand` | 对应 | Windows/MSVC 与 Park–Miller 分支；见第 8 节平台差异。 |

### 3.8 `mypaint-symmetry.c` / `.h`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `symmetry_state_default` / `default_symmetry_data` | `pigment-surface/src/symmetry.rs:173` 与 `Transform::default` | 对应 | |
| `mypaint_symmetry_set_pending` | `symmetry.rs` 的 `set_pending` | 对应 | 写入 pending，不立即改 current。 |
| `mypaint_update_symmetry_state` | `symmetry.rs:336` 的 `update` / `recompute` | 对应 | 矩阵重算与去重条件一致。 |
| `mypaint_tiled_surface_set_symmetry_state` | `symmetry.rs:587` | 对应 | 在 `begin_atomic` 应用。 |
| 矩阵计数（None/Vertical/Horizontal/Rotational/Snowflake） | `symmetry.rs` 的 `num_matrices_required` | 对应 | 有逐公式测试。 |

### 3.9 `mypaint-tiled-surface.c` / `.h`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `mypaint_tiled_surface_init` / `destroy` | `pigment-surface/src/lib.rs` 的 `PigmentSurface` 构造/`Drop` | 结构性替代 | |
| `mypaint_tiled_surface_begin_atomic` | `pigment-surface/src/lib.rs:1503` | 对应 | 应用 pending 对称状态。 |
| `mypaint_tiled_surface_end_atomic` | `lib.rs:1513` | 对应 | 返回脏 tile 列表。 |
| `mypaint_tiled_surface_tile_request_start` | `lib.rs:1460` | 对应 | 返回 `&mut TilePixels`。 |
| `mypaint_tiled_surface_tile_request_end` | `lib.rs:1482` | 对应 | |
| `mypaint_tiled_surface_draw_dab` | `hokusai-core/src/surface.rs:127` + `brushmodes.rs:743 draw_dab_default` | 对应 | 默认实现被所有后端复用。 |
| `mypaint_tiled_surface_get_color` | `surface.rs:160/170` + `brushmodes.rs:1709/1788` | 对应 | |
| `mypaint_surface_get_alpha` | `pigment-surface/src/lib.rs:921 average_alpha` | 对应 | 圆形邻域平均。 |
| `mypaint_surface_save_png` | `pigment-surface/src/lib.rs:1209 save_as_png` + `png.rs` | 对应 | sRGB/gAMA/cHRM 辅助块。 |
| `mypaint_tile_request_init` | 无独立结构 | 结构性替代 | Rust 直接返回 tile 借用。 |

### 3.10 `mypaint-fixed-tiled-surface.c`、`mypaint-surface.c`、`mypaint.c`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `mypaint_fixed_tiled_surface_init` | `hokusai-tile-mem/src/lib.rs` 的 `MemSurface` | 对应 | 测试/参考后端。 |
| `reset_null_tile` | `hokusai-tile-mem` 的零 tile 处理 | 对应 | |
| `free_simple_tiledsurf` | Rust `Drop` | 结构性替代 | |
| `mypaint_surface_init` / `ref` / `unref` | `hokusai-core/src/surface.rs` 的 `TiledSurface` trait | 结构性替代 | |
| `mypaint_surface_begin_atomic` / `end_atomic` | `surface.rs:73/75` | 对应 | 默认空实现，由后端覆盖。 |
| `mypaint_surface_draw_dab` | `surface.rs:127` | 对应 | |
| `mypaint_surface_get_color` | `surface.rs:160` | 对应 | |
| `mypaint_surface_get_alpha` | `pigment-surface/src/lib.rs:921` | 对应 | |
| `mypaint_surface_save_png` | `pigment-surface/src/lib.rs:1209` | 对应 | |
| `mypaint.c` 版本宏 | `Cargo.toml` / `env!("CARGO_PKG_VERSION")` | 结构性替代 | |

### 3.11 `tilemap.c`、`fifo.c`、`operationqueue.c`、`mypaint-matrix.c`、`mypaint-rectangle.c`

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `tile_map_new` / `get` / `contains` / `free` | `pigment-surface/src/lib.rs` 的 tile `HashMap` + `mipmap.rs` | 结构性替代 | 没有独立 tilemap 类型。 |
| `tile_map_copy_to` | `lib.rs` 的 `clone` / snapshot | 对应 | |
| `fifo_new` / `push` / `pop` / `peek_*` | `lib.rs` 的 `queue_dab` / `pending_ops` + Rust `VecDeque` | 结构性替代 | |
| `operationqueue` 初始化/提交/取出 | `lib.rs` 的 `queue_dab`/`flush`/`flush_budgeted` | 结构性替代 | 同步 flush + 可预算 flush。 |
| `mypaint_transform_unit` / `translate` / `rotate_*` / `reflect` / `point` | `pigment-surface/src/symmetry.rs` 的 `Transform` | 对应 | |
| `mypaint_rectangle_copy` / `expand_to_include_*` | `pigment-engine/src/document.rs:1595` + 元组矩形 | 结构性替代 | 没有 C 矩形类型。 |
| `memdup` | Rust `to_vec` / `clone` | 结构性替代 | |

### 3.12 libmypaint 附注

- `gegl/`、`glib/`、`tests/`、`examples/` 不是应用运行路径；本地没有对应文件，按“不适用”处理。
- `fastapprox` 的公式被内联进 `hokusai-core/src/spectral.rs`，不是逐头文件移植。
- libmypaint 的异步 operation queue 在本地被同步 tile 写 + 可预算 flush + GPU sink 取代；调用时序不同，像素结果由 `hokusai-compat` 快照与 `pigment-surface/tests/parity.rs` 约束。

---

## 4. MyPaint `lib/`（Python）逐文件

### 4.1 文件状态总表

| 上游文件 | 本地落点 | 状态 |
|---|---|---|
| `lib/alg.py` | `pigment-engine/src/color/hcywheel.rs`、`pigment-app/src/shell/overlays.rs` | 部分 |
| `lib/autosave.py` | `pigment-engine/src/autosave.rs`、`pigment-app/src/app.rs` | 对应 |
| `lib/brush.py` | `hokusai-core/src/{brush,stroke}.rs`、`hokusai-brush`、`pigment-engine/src/brushlib.rs` | 部分 |
| `lib/brushes_migrate_json.py` | 无 | 不适用 |
| `lib/brushsettings.py` | `hokusai-core/src/{setting,input}.rs` + 构建期画笔元数据 | 对应 |
| `lib/cache.py` | `pigment-surface/src/{lib,mipmap}.rs` 的缓存结构 | 结构性替代 |
| `lib/color.py` | `pigment-engine/src/color/{model,space}.rs` | 对应 |
| `lib/command.py` | `pigment-engine/src/command.rs` | 对应 |
| `lib/document.py` | `pigment-engine/src/{document,fileio,autosave}.rs` | 对应 |
| `lib/eotf.py` | `pigment-engine/src/compat.rs`、`pigment-surface/src/color.rs` | 对应 |
| `lib/errors.py` | Rust `Result` + 各模块错误枚举 | 结构性替代 |
| `lib/feedback.py` | 无进度对象；操作为同步调用 | 缺失 |
| `lib/fileutils.py` | `pigment-engine/src/{prefs,scrap,fileio}.rs`、`pigment-app/src/native_dialog.rs` | 部分 |
| `lib/fill_common.py` | `pigment-engine/src/mode/{fill,fill_gap}.rs` | 对应 |
| `lib/floodfill.py` | `pigment-engine/src/mode/{fill,fill_gap}.rs` | 对应 |
| `lib/gettext.py` | `pigment-app/src/i18n.rs` 的 `.mo` 读取 | 结构性替代 |
| `lib/gettext_setup.py` | `pigment-app/src/i18n.rs` | 结构性替代 |
| `lib/gibindings.py` | 无 | 不适用 |
| `lib/glib.py` | `pigment-engine/src/prefs.rs` 的目录解析 | 结构性替代 |
| `lib/helpers.py` | `pigment-surface/src/{vector,thumb,png}.rs`、`pigment-engine/src/color/*` | 部分 |
| `lib/i18n.py` | `pigment-app/src/i18n.rs` | 对应 |
| `lib/idletask.py` | rayon / 线程 + 各模块队列 | 结构性替代 |
| `lib/layer/core.py` | `pigment-surface/src/layer.rs` 的 `Layer` | 对应 |
| `lib/layer/data.py` | `pigment-surface/src/layer.rs` 的 `Layer` / `LayerContent` / `vector.rs` | 对应 |
| `lib/layer/error.py` | Rust `Result` | 结构性替代 |
| `lib/layer/group.py` | `pigment-surface/src/layer.rs:995 LayerStack` | 对应 |
| `lib/layer/rendering.py` | `pigment-surface/src/layer.rs` 的直接合成 | 结构性替代 |
| `lib/layer/test.py` | 无（测试辅助） | 不适用 |
| `lib/layer/tree.py` | `pigment-surface/src/layer.rs:995 LayerStack` | 对应 |
| `lib/layervis.py` | `pigment-engine/src/layer_views.rs` + `command.rs` | 部分 |
| `lib/localecodes.py` | `pigment-app/src/shell/locales_generated.rs` | 对应 |
| `lib/meta.py` | `pigment-engine/src/fileio.rs:156 Compatibility` | 对应 |
| `lib/modes.py` | `pigment-engine/src/compat.rs` | 对应 |
| `lib/morphology.py` | `pigment-engine/src/mode/fill_morphology.rs` | 对应 |
| `lib/naming.py` | `pigment-engine/src/layer_views.rs` 的 `make_unique_name` | 对应 |
| `lib/observable.py` | 无观察者框架；状态直接读写 | 结构性替代 |
| `lib/palette.py` | `pigment-engine/src/color/palette.rs` | 对应 |
| `lib/pixbuf.py` | `pigment-surface/src/png.rs`、`pigment-engine/src/fileio.rs` | 部分 |
| `lib/pixbufsurface.py` | `pigment-surface/src/{vector,color}.rs` | 部分 |
| `lib/pycompat.py` | 无 | 不适用 |
| `lib/stroke.py` | `pigment-engine/src/stroke.rs` | 对应 |
| `lib/strokemap.py` | `pigment-surface/src/strokemap.rs` | 对应 |
| `lib/surface.py` | `pigment-surface/src/{lib,png}.rs`、`hokusai-core/src/surface.rs` | 对应 |
| `lib/tiledsurface.py` | `pigment-surface/src/{lib,layer,symmetry,mipmap}.rs` | 对应 |
| `lib/validation.py` | 各模块的类型化解析 | 结构性替代 |
| `lib/xml.py` | `pigment-engine/src/fileio.rs`、`accelmap.rs` | 对应 |
| `lib/__init__.py` | 无 | 不适用 |
| `lib/layer/__init__.py` | 无（Rust 模块直接导出） | 结构性替代 |

### 4.2 逐文件明细

#### `lib/alg.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `convex_hull` | `pigment-engine/src/color/hcywheel.rs:221` | 对应 |
| `det` | `hcywheel.rs:200` | 对应 |
| `poly_area` | `hcywheel.rs:286` | 对应 |
| `point_in_convex_poly` | `hcywheel.rs:314`、`shell/overlays.rs:631` | 对应 |
| `nearest_point_in_segment` | `hcywheel.rs:356` | 对应 |
| `intersection_of_vector_and_poly` | `shell/overlays.rs:686` | 对应 |
| `intersection_of_vectors` | `shell/overlays.rs:662` | 对应 |
| `pairwise` | `hcywheel.rs:184`、`shell/overlays.rs:626` | 对应 |
| `poly_centroid` | 无 | 缺失 |
| `nearest_point_on_segment` | 无 | 缺失 |
| `nearest_point_in_poly` | 无 | 缺失 |
| `nearest_point_on_line` | 无 | 缺失 |
| `_nearest_point` | 无 | 缺失 |
| `_intersects` | 无 | 缺失 |
| `intersection_of_segments` | 无 | 缺失 |
| `LineType` | `shell/overlays.rs:613` 的同名枚举 | 对应 |

缺失项的检索依据：在 `pigment/crates` 与 `hokusai/crates` 按上述符号名检索，无结果。缺失集中在 HCY 遮罩几何；本地颜色器界面也未提供遮罩编辑（见 `gui/colors/hcywheel.py`）。

#### `lib/autosave.py`

**状态：对应**

`Autosaveable` 的 `autosave_dirty` / `autosave_uuid` / `queue_autosave` 是 Python mixin。本地没有同名 trait，行为落在：

- `pigment-engine/src/autosave.rs`：缓存根目录、`AutosaveInfo`、元数据、清理；
- `pigment-app/src/app.rs`：`AutosaveState` / `AutosaveJob` 与定时写盘；
- `pigment-engine/src/document.rs`：`queue_autosave` 等文档级入口。

#### `lib/brush.py`

**状态：部分**

| 上游符号/方法 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `BrushInfo.to_json` / `from_json` / `load_from_string` / `save_to_string` | `hokusai-brush/src/lib.rs:125 from_str`、`:191 to_string_pretty` | 对应 | 输出全部 65 个设置，显式 0 保留。 |
| `BrushInfo.set_base_value` / `get_base_value` / `set_points` / `get_points` | `hokusai-core/src/brush.rs:76-160` | 对应 | |
| `BrushInfo.get/set_setting` | `hokusai-core/src/brush.rs:54 set` / `get` | 对应 | |
| `BrushInfo.get_visual_radius` / `brush_visual_radius` | `pigment-engine/src/mode/brushresize.rs:107` | 对应 | |
| `BrushInfo.get_color_hsv` / `set_color_hsv` / `get_color_rgb` / `set_color_rgb` | `PigmentApp::color_hsv/set_color_hsv`（`app.rs:3757/3777`）+ `derive_active`（`app.rs:3513-3518` 写入 `BrushSetting::ColorH/S/V`） | 对应 | 颜色由 `color_manager` 持有，派生工作笔刷时写入 `color_h/s/v`；`restore_color` 在 `brushlib.rs`。 |
| `BrushInfo.is_eraser` / `is_alpha_locked` / `is_colorize` | `pigment-app/src/blend_modes.rs` 的 `BrushBlendMode` + `brushlib.rs` setting 名 | 对应 | |
| `BrushInfo.begin_atomic` / `add_pending_update` / `end_atomic` | 无 | 结构性替代 | 本地直接改 `Brush`，无待提交更新批次。 |
| `Brush._update_setting_from_brushinfo` / `_transform_brush_color` | `hokusai-core/src/stroke.rs` + `pigment-engine/src/compat.rs` / `pigment-surface/src/color.rs` 的 EOTF 链 | 对应 | 线性/非线性颜色链在引擎与显示编码两处实现。 |
| `Brush.stroke_to` | `hokusai-core/src/stroke.rs:62 stroke_to_full` | 对应 | 6 个基础输入之外还有 viewzoom/viewrotation/barrel/linear。 |
| `_load_old_format` / `_oldfmt_parse_value` / `_oldfmt_parse_points_v1/v2` / `_oldfmt_transform_y` / `brushinfo_unquote` | 无 | 缺失 | `.myb` v2 旧文本格式不支持；只读 v3 JSON。 |
| `ParseError` / `Obsolete` | `hokusai-brush::Error`、`pigment-engine::brushlib::BrushLoadError` | 结构性替代 | 错误类型不同。 |

#### `lib/brushes_migrate_json.py`

**状态：不适用**

一次性迁移脚本（把旧画笔目录迁到 JSON）。本地只消费 v3 `.myb`，没有迁移入口；该文件不属于运行时功能。

#### `lib/brushsettings.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `BrushInputInfo` | `hokusai-core/src/input.rs` 的 `BrushInput` + `cname/index` | 对应 |
| `BrushSettingInfo` | `hokusai-core/src/setting.rs` 的 `BrushSetting` + `cname/index` | 对应 |
| 65 setting / 18 input 的名称、范围、默认值 | `setting.rs` / `input.rs` / `hokusai-brush/src/lib.rs:39` | 对应 |

#### `lib/cache.py`

**状态：结构性替代**

上游 `LRUCache` 用于 `RootLayerStack` 的渲染 tile 缓存（`lib/layer/tree.py:124`）。本地没有 LRU 类型；渲染缓存由：

- `pigment-surface/src/mipmap.rs` 的多级金字塔（带 revision / dirty）；
- `pigment-surface/src/layer.rs` 的 `tile_composite_revision` 系列缓存；

承担。两者不是同一数据结构，缓存键与淘汰策略不同。

#### `lib/color.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `UIColor` | `pigment-engine/src/color/model.rs:83` | 对应 |
| `RGBColor` / `HSVColor` / `HCYColor` / `YCbCrColor` | `color/model.rs` 的对应结构 | 对应 |
| `RGB_to_YCbCr_BT601` / `YCbCr_to_RGB_BT601` | `color/space.rs` | 对应 |
| `RGB_to_HCY` / `HCY_to_RGB` | `color/space.rs`、`color/hcywheel.rs` | 对应 |
| `interpolate` | `color/model.rs` 的插值方法 | 对应 |
| `to_hex_str` / `new_from_hex_str` | `color/model.rs` | 对应 |
| `to_fill_pixel` | `pigment-surface/src/color.rs` 的 fix15/编码路径 | 对应 |

`pigment-engine/tests/color_vectors.rs`、`color_doctests.rs` 用上游计算值做向量门禁。

#### `lib/command.py`

**状态：对应**

| 上游类 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `CommandStack` | `pigment-engine/src/command.rs:164` | 对应 | |
| `Command` | `command.rs:113` | 对应 | |
| `Brushwork` | `command.rs:329` | 对应 | |
| `FloodFill` | `command.rs:645` | 对应 | |
| `TrimLayer` | `command.rs:1636 MoveLayer::trim` | 对应 | 复用 `MoveLayer` 的前后快照，标签为 `Trim Layer`；没有独立 `TrimLayer` 类型。 |
| `UniqLayer` | `command.rs:1896` | 对应 | |
| `RefactorGroup` | `command.rs:1936` | 对应 | |
| `ClearLayer` | `command.rs:1780` | 对应 | |
| `LoadLayer` | `command.rs:865` | 对应 | |
| `NewLayerMergedFromVisible` | `pigment-engine/src/document.rs:690 new_layer_from_visible` | 对应 | 无独立命令类；生成 `LoadLayer` 命令，可撤销。 |
| `MergeVisibleLayers` | `command.rs:1866` | 对应 | |
| `MergeLayerDown` | `command.rs:1539` | 对应 | |
| `NormalizeLayerMode` | `command.rs:1822` | 对应 | |
| `AddLayer` | `command.rs:807` | 对应 | |
| `RemoveLayer` | `command.rs:895` | 对应 | |
| `SelectLayer` | `command.rs:944` | 对应 | |
| `MoveLayer` | `command.rs:1588` | 对应 | |
| `DuplicateLayer` | `command.rs:976` | 对应 | |
| `BubbleLayerUp` / `BubbleLayerDown` | `command.rs:1029 BubbleLayer` | 对应 | 一个类型带方向。 |
| `RestackLayer` | `command.rs:2100` | 对应 | |
| `RenameLayer` | `command.rs:1251` | 对应 | |
| `SetLayerVisibility` | `command.rs:1309` | 对应 | |
| `SetLayerLocked` | `command.rs:1369` | 对应 | |
| `SetLayerOpacity` | `command.rs:1424` | 对应 | |
| `SetLayerMode` | `command.rs:1481` | 对应 | |
| `SetFrameEnabled` | `pigment-engine/src/document.rs:924 set_frame_enabled` | 部分 | 只改 `frame_enabled` 状态；源码的用户触发切换会入撤销栈，本地没有对应命令。 |
| `UpdateFrame` | `command.rs:1976` | 对应 | |
| `ExternalLayerEdit` | `command.rs:1690` | 对应 | |
| `LayerView` 相关五个命令 | `command.rs:1075-1218` | 对应 | 见 `lib/layervis.py`。 |

#### `lib/document.py`

**状态：对应**

| 上游类/函数 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `Document` | `pigment-engine/src/document.rs:122` | 对应 | 图层栈、当前层、分辨率、画框、对称、自动保存、撤销。 |
| `AutosaveInfo` | `pigment-engine/src/autosave.rs:221` | 对应 | |
| `_save_layers_to_new_orazip` | `pigment-engine/src/fileio.rs` 的 ORA 写路径 | 对应 | |
| `get_app_cache_root` | `autosave.rs:125 app_cache_root` | 对应 | |
| `get_available_autosaves` | `autosave.rs` / `pigment-app` 恢复列表 | 对应 | |
| `_get_path_mtime` | `autosave.rs` 的元数据 mtime | 对应 | |
| `save` / `load` / `save_ora` / `load_ora` | `pigment-engine/src/fileio.rs` + `document.rs` | 对应 | |
| `save_png` / `load_png` / `save_jpg` | `fileio.rs:536 save_png`、`load_png`；JPEG 由 `image` crate | 对应 | |
| `import_layers` | `pigment-app` 文件导入 + `fileio` | 对应 | |
| `render_thumbnail` | `pigment-surface/src/thumb.rs` | 对应 | |
| `layer_view_manager` | `pigment-engine/src/layer_views.rs` | 对应 | |
| `resume_from_autosave` | `pigment-app/src/app.rs` 恢复流程 | 对应 | |

#### `lib/eotf.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `set_eotf` / `eotf` / `set_base_eotf` / `base_eotf` | `pigment-engine/src/compat.rs:72` 的 `CompatMode::eotf` + `CompatSettings` | 对应 |
| OETF/EOTF 像素编码 | `pigment-surface/src/color.rs` 的 `eotf_encode` / `eotf_decode` / LUT | 对应 |

本地 EOTF 是模式字段（1.0/2.2），不是全局可写函数。

#### `lib/errors.py`

**状态：结构性替代**

`FileHandlingError` / `AllocationError` 没有同名类型。文件 I/O 用 `pigment-engine/src/fileio.rs:106 FileError`，其余用各模块 `Result`/错误枚举。

#### `lib/feedback.py`

**状态：缺失**

上游 `Progress` 用于文档保存/加载的进度回调（`lib/document.py:1591/1649/2058`），填充处理器另有自己的进度字符串。本地：

- `pigment-engine` 的保存/加载/填充是同步函数，返回 `Result`，没有进度对象；
- `pigment-app/src/app.rs` 只有状态栏文本与忙碌光标，没有分项进度。

检索 `Progress`、`fraction`、`set_stage` 在 `pigment/crates` 无对应实现。

#### `lib/fileutils.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `expanduser_unicode` | `pigment-engine/src/scrap.rs:34 expand_user` | 部分 | 只处理常用 `~`/`~user` 前缀。 |
| `via_tempfile` | `pigment-engine/src/fileio.rs` 的直接写 + 临时文件路径 | 部分 | 无统一“写临时再原子替换”封装。 |
| `replace` | `std::fs::rename` 等直接调用 | 结构性替代 | |
| `startfile` | `pigment-app` 的 `open_external_url` / 外部编辑器启动 | 部分 | 平台路径不同。 |
| `safename` | 无同名实现 | 缺失 | 文件命名清洗在保存对话框中按扩展名处理，没有通用 `safename`。 |

#### `lib/fill_common.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `TileBoundingBox` | `pigment-engine/src/mode/fill.rs:720` | 对应 |
| `nine_grid` | `pigment-engine/src/mode/fill_gap.rs:639` | 对应 |
| `orthogonal` | `fill.rs:921`、`fill_gap.rs:655` | 对应 |
| `adjacent` | `fill_gap.rs` 的邻接遍历 | 对应 |
| `new_full_tile` | 填充 tile 初始化路径 | 对应 |

#### `lib/floodfill.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `GapClosingOptions` | `pigment-engine/src/mode/fill_gap.rs:47` | 对应 |
| `starting_coordinates` | `fill.rs:661` | 对应 |
| `seeds_by_tile` | `fill.rs:678` | 对应 |
| `get_target_color` | `fill.rs:702` | 对应 |
| `enqueue_overflows` | `fill.rs:930`、`fill_gap.rs:663` | 对应 |
| `flood_fill` | `fill.rs:1127` | 对应 |
| `composite` | `fill.rs:1220` | 对应 |
| `scanline_fill` | `fill.rs:950` | 对应 |
| `gap_closing_fill` | `fill_gap.rs:816` | 对应 |
| `FillHandler` / `FloodFillArguments` / `_GCTileHandler` / `_TileFillSkipper` | 无同名类型；状态由 `fill.rs` / `fill_gap.rs` 内部结构承载 | 结构性替代 |
| `update_bbox` / `unseep` / `complement_gc_seeds` / `gc_seeds_skippable` | 无同名函数（按符号名检索无结果；对应阶段可能内联，未核实） | 缺失 |

#### `lib/gettext.py` / `lib/gettext_setup.py`

**状态：结构性替代**

本地不嵌入 Python gettext。`pigment-app/src/i18n.rs` 直接读取安装目录的 `.mo`、内置 `zh_CN` 表，并提供 `tr` / `tr_fmt`。`C_` 上下文函数、locale 调试与 gettext 安装流程没有同名 API。

#### `lib/gibindings.py`

**状态：不适用**

PyGObject 绑定延迟导入工具。本地没有 Python 运行时。

#### `lib/glib.py`

**状态：结构性替代**

上游封装 GLib 的用户目录/URI/文件名编码。本地：

- `pigment-engine/src/prefs.rs:156 user_config_dir`、`:175 documents_dir`、`:206 config_dir`；
- `pigment-engine/src/autosave.rs:97 user_cache_dir`；
- `pigment-engine/src/scratchpad.rs:103 user_data_dir`。

URI 编解码没有同名实现。

#### `lib/helpers.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `rotated_rectangle_bbox` | `pigment-surface/src/vector.rs:138` | 对应 |
| `clamp` | `pigment-engine/src/color/tools.rs:113`、`shell/overlays.rs` | 对应 |
| `rgb_to_hsv` / `hsv_to_rgb` | `pigment-engine/src/color/space.rs:34/62`、`mode/pick.rs:87/92` | 对应 |
| `zipfile_writestr` | `pigment-engine/src/zip.rs` | 对应 |
| `scale_proportionally` / `pixbuf_thumbnail` | `pigment-surface/src/thumb.rs` 的预览几何/降采样 | 部分 |
| `Rect` | 无同名类型；使用 `(i32,i32,i32,i32)` 元组 | 结构性替代 |
| `gdkpixbuf2numpy` | 无 | 缺失（GdkPixbuf 专用） |
| `freedesktop_thumbnail` | 无 | 缺失（缩略图缓存协议） |
| `get_pixbuf` | 无 | 缺失（GdkPixbuf 专用） |
| `transform_hsv` | 无同名函数 | 缺失 |
| `run_garbage_collector` / `record_memory_leak_status` | 无 | 结构性替代（Rust 无 Python GC） |
| `utf8` / `casefold` / `grouper` / `fmt_time_period_abbr` | 无同名函数 | 部分（Rust 标准库/局部实现替代，未逐一核实） |

#### `lib/i18n.py`

**状态：对应**

本地 `pigment-app/src/i18n.rs` 覆盖：系统 locale 识别、语言代码选择、内置中文表、`.mo` 读取、`LANG`/`LANGUAGE` 环境处理。`osx_locale_id_to_lang` 在非 macOS 构建中无对应分支。

#### `lib/idletask.py`

**状态：结构性替代**

上游 `Processor` 是 GTK 空闲回调的低优先级任务队列。本地没有同名队列；后台工作由 rayon 线程池、`pigment-surface` 的并行 tile 处理，以及应用层 `AutosaveJob` / 恢复轮询承担。

#### `lib/layer/core.py`

**状态：对应**

| 上游符号/方法 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `LayerBase` 属性（name/opacity/visible/locked/mode） | `pigment-surface/src/layer.rs:400 Layer` 字段 | 对应 | |
| `LayerBase.get_bbox` / `is_empty` / `effective_opacity` | `layer.rs` 的 `data_bbox` / `is_empty` / `effective_opacity` | 对应 | |
| `LayerBase.get_render_ops` | 无；直接合成 | 结构性替代 | |
| `LayerBase.save_to_openraster` | `pigment-engine/src/fileio.rs` 的 ORA 写路径 | 对应 | |
| `LayerBase.thumbnail` / `render_thumbnail` | `pigment-surface/src/thumb.rs` | 对应 | |
| `LayerBase.set_symmetry_state` | `document.rs` → `surface.set_symmetry_state` | 对应 | |
| `LayerBaseSnapshot` / `restore_to_layer` | `pigment-surface/src/lib.rs:161 SurfaceSnapshot` + `load_snapshot` | 对应 | |
| `ExternallyEditable` | `pigment-app/src/app.rs` 的外部编辑流程 | 部分 | 无同名 mixin。 |
| `_StubLayerBase` | 无 | 不适用 | 测试桩。 |

#### `lib/layer/data.py`

**状态：对应**

本地把上游的图层类层次压成 `Layer` + `LayerContent::Painting/Group` + `Layer.vector`：

| 上游类 | 本地落点 | 状态 |
|---|---|---|
| `SurfaceBackedLayer` | `layer.rs` 的 `Layer` + `LayerContent::Painting(PigmentSurface)` | 对应 |
| `SimplePaintingLayer` / `PaintingLayer` / `StrokemappedPaintingLayer` | 同一 `Layer`；笔画信息在 `pigment-surface/src/strokemap.rs` | 对应 |
| `FileBackedLayer` | `pigment-engine/src/fileio.rs` 的 ORA 成员读写 + `pigment-surface` 表面 | 部分 |
| `BackgroundLayer` | `pigment-surface/src/layer.rs` 的 `BackgroundImage` + `LayerStack` 背景字段 | 对应 |
| `VectorLayer` | `pigment-surface/src/vector.rs` 的 `VectorLayerData` | 对应 |
| `FallbackBitmapLayer` / `FallbackDataLayer` | 无同名类（`layer.rs:30` 注释明确不移植） | 缺失 |
| `SurfaceBackedLayerMove` / `LayerStackMove` / `LayerMoveMode` | `pigment-engine/src/mode/layermove.rs` | 对应 |
| `save_snapshot` / `restore_to_layer` | `pigment-surface/src/lib.rs` `save_snapshot/load_snapshot` | 对应 |
| `_ManagedFile` | 无（Rust 文件句柄/路径直接管理） | 结构性替代 |
| `_write_strokemap` / `_write_strokemap_stroke` | `strokemap.rs` 的 `to_dat` / `write_dat` | 对应 |
| `StrokemappedPaintingLayerSnapshot` / `StrokemappedPaintingLayerMove` | `strokemap.rs` + `layermove.rs` | 对应 |

#### `lib/layer/error.py`

**状态：结构性替代**

`LoadingFailed` 由 `pigment-engine/src/fileio.rs::FileError` 等 `Result` 错误承载。

#### `lib/layer/group.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `LayerStack` | `pigment-surface/src/layer.rs:995` | 对应 |
| `LayerStack` 的插入/移除/重排/当前层 | `layer.rs` 的 `insert_at/remove_by_id/bubble_layer/...` | 对应 |
| `LayerStackSnapshot` | `pigment-surface/src/lib.rs` 图层快照 + `LayerStack::save_snapshot` | 对应 |
| `LayerStackMove` | `mode/layermove.rs` | 对应 |

#### `lib/layer/rendering.py`

**状态：结构性替代**

上游用 `Opcode`/`Spec`/`Renderable` 描述渲染操作列表，再由 `RootLayerStack` 执行。本地没有中间 op 列表；`pigment-surface/src/layer.rs` 直接递归合成（`composite_layer` / `composite_tile`）。行为对应，数据结构不可逐符号对应。

#### `lib/layer/test.py`

**状态：不适用**

测试辅助 `make_test_stack`。本地对应测试在 `pigment-engine/tests/layer_groups.rs` 与 `pigment-surface` 内置测试。

#### `lib/layer/tree.py`

**状态：对应**

| 上游符号/方法 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `RootLayerStack` | `pigment-surface/src/layer.rs:995 LayerStack` | 对应 | 本地用一个类型同时承担组栈与根栈。 |
| `RootLayerStack` 的当前路径/深层插入/删除 | `layer.rs` 的 `current_path/set_current_path/insert_at/remove_by_id` | 对应 | |
| `RootLayerStack.render` / `render_single_tile` | `layer.rs` 的 `composite_tile` / `composite_layer` | 对应 | |
| `RootLayerStack.render_layer_preview` | `layer.rs` 的 `composite_preview` | 对应 | |
| `RootLayerStack` 对称状态传播 | `document.rs` + `surface.set_symmetry_state` | 对应 | |
| 背景层 / `background_visible` | `layer.rs` 的 `BackgroundImage` / `set_background_*` | 对应 | |
| 当前层 solo / previewing / overlay | `layer.rs` 的 `current_layer_solo` / `set_preview_path` / `composite_preview` | 对应 | |
| `get_unique_name` | `pigment-engine/src/layer_views.rs:327 make_unique_name` | 对应 | |
| `_render_cache`（LRU） | `mipmap.rs` + `tile_composite_revision` | 结构性替代 | |
| `_mark_all_layers_for_rethumb` / 缩略图定时器 | `pigment-app` 的缩略图刷新 | 部分 | 无同名定时器；刷新由面板重绘驱动。 |

#### `lib/layervis.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `_View` / `_NamedViewsSet` | `pigment-engine/src/layer_views.rs:34/57` | 对应 |
| `LayerViewManager` | `layer_views.rs` 的 `LayerViews` | 对应 |
| `AddLayerView` / `RemoveActiveLayerView` / `RenameActiveLayerView` / `ActivateLayerView` / `SetActiveLayerViewLocked` | `command.rs:1075-1218` | 对应 |
| 视图切换 UI（`gui/layervis.py`） | 无入口（`app.rs:854 layer_view_rename` 字段从未被读取；无视图动作处理） | 缺失 |

#### `lib/localecodes.py`

**状态：对应**

`pigment-app/src/shell/locales_generated.rs` 由安装目录的 locale 数据生成；`supported_locales` 与标签进入偏好页。

#### `lib/meta.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `Compatibility` | `pigment-engine/src/fileio.rs:156` | 对应 |
| `compatibility` | `fileio.rs:209` | 对应 |
| `_parse_version_string` | `fileio.rs:168 parse_version_string` | 对应 |

#### `lib/modes.py`

**状态：对应**

`set_default_mode` / `default_mode` 的全局默认混合模式在本地由 `pigment-engine/src/compat.rs:239` 的兼容模式与 `CompatSettings` 承担。没有进程级可变全局函数；默认值来自偏好。

#### `lib/morphology.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `morph` | `pigment-engine/src/mode/fill_morphology.rs:168` | 对应 |
| `blur` | `fill_morphology.rs:256` | 对应 |
| `adjacent_tiles` / `complement_adjacent` / `directly_below` / `strand_partition` / `adj_full` | 无同名函数 | 部分（邻接/分区逻辑在 `fill_morphology.rs` 内联，未逐一核实） |

#### `lib/naming.py`

**状态：对应**

`make_unique_name` → `pigment-engine/src/layer_views.rs:327`。

#### `lib/observable.py`

**状态：结构性替代**

上游用描述符 + 事件实现属性观察（`observable`、`event`、`ObservableDict`）。本地没有观察者框架；UI 每帧从模型读取，修改直接落模型。`sync_pending_changes` 的批量提交语义没有 1:1 对应。

#### `lib/palette.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `Palette` | `pigment-engine/src/color/palette.rs:315` | 对应 |
| `load` / `save` | `palette.rs` 的 GIMP 调色板读写 | 对应 |
| `match_color` / `get_match_position` / `move_match_position` | `palette.rs` 的匹配与移动 | 对应 |
| `append` / `insert` / `reposition` / `pop` / `get_color` | `palette.rs` | 对应 |
| `to_simple_dict` / `new_from_simple_dict` | `palette.rs` 的简单字典序列化 | 对应 |
| `_outwards_from` / `_color_distance` | `palette.rs` 内部实现（未保持同名） | 对应 |

#### `lib/pixbuf.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `save` | `pigment-surface/src/png.rs` + `fileio.rs` | 对应 | |
| `load_from_file` | `png.rs` / `image` crate | 对应 | |
| `load_from_stream` | `png.rs` 的内存解码 | 对应 | |
| `load_from_zipfile` | `fileio.rs` 的 ORA 成员读取 | 对应 | |
| GdkPixbuf 特有的缩放/格式转换 | `pigment-surface/src/color.rs` / `thumb.rs` | 部分 | 不是 GdkPixbuf 实现。 |

#### `lib/pixbufsurface.py`

**状态：部分**

上游 `Surface` 是 GdkPixbuf 支撑的表面；`render_as_pixbuf` 把 tile 渲染为 pixbuf。本地：

- `pigment-surface/src/color.rs` 的 `tile_to_rgba8*` 提供像素导出；
- `pigment-surface/src/vector.rs` 提供 SVG 栅格化；
- `pigment-app/src/thumb.rs` 提供缩略图。

没有 GdkPixbuf 表面类型。

#### `lib/pycompat.py`

**状态：不适用**

Python 2/3 兼容垫片。Rust 无对应。

#### `lib/stroke.py`

**状态：对应**

| 上游方法 | 本地落点 | 状态 |
|---|---|---|
| `Stroke.start_recording` | `pigment-engine/src/stroke.rs:197` | 对应 |
| `record_event` | `stroke.rs:212` | 对应 |
| `stop_recording` | `stroke.rs:222` | 对应 |
| `is_empty` | `stroke.rs:240`（`total_painting_time == 0`） | 对应 |
| `render` / `copy_using_different_brush` | `pigment-engine/src/command.rs:329 Brushwork` 的重放 + `stroke.rs` | 对应 |
| 事件行格式（9 个字段：dtime/x/y/pressure/xtilt/ytilt/viewzoom/viewrotation/barrel_rotation） | `stroke.rs:44 EVENT_FIELDS` + `:48 StrokeEvent` | 对应 |

#### `lib/strokemap.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `StrokeShape` | `pigment-surface/src/strokemap.rs:37` | 对应 |
| `new_from_snapshots` | `strokemap.rs` 的 `from_snapshots` | 对应 |
| `touches_pixel` / `trim` / `translate` | `strokemap.rs` | 对应 |
| `save_to_string` / `init_from_string` | `strokemap.rs` 的 `to_bytes/from_bytes` / `write_dat/read_dat` | 对应 |
| `_Tile` 的压缩位图格式 | `strokemap.rs` 的 zlib `.dat` 编解码 | 对应 |
| 异步 `_TileDiffUpdateTask` / `_TileTranslateTask` / `_TileRecompressTask` | 同步/并行实现，无任务类 | 结构性替代 |

#### `lib/surface.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `Bounded` / `TileAccessible` / `TileBlittable` / `TileCompositable` | `hokusai-core/src/surface.rs` 的 `TiledSurface` + `pigment-surface` 的方法 | 结构性替代 |
| `get_tiles_bbox` | `pigment-surface/src/lib.rs` 的 `data_bbox` | 对应 |
| `scanline_strips_iter` | `lib.rs` 的合成/导出扫描 | 对应 |
| `save_as_png` | `pigment-surface/src/lib.rs:1209` | 对应 |

#### `lib/tiledsurface.py`

**状态：对应**

| 上游符号/方法 | 本地落点 | 状态 |
|---|---|---|
| `MyPaintSurface` | `pigment-surface/src/lib.rs:314 PigmentSurface` | 对应 |
| `_Tile` / `_SurfaceSnapshot` | `lib.rs` 的 `Tile` / `SurfaceSnapshot` | 对应 |
| `begin_atomic` / `end_atomic` | `lib.rs:1503/1513` | 对应 |
| `tile_request` / `blit_tile_into` / `composite_tile` | `lib.rs` 的 `tile_request_start/end`、`tile_to_rgba8`、`composite_tile` | 对应 |
| `save_snapshot` / `load_snapshot` | `lib.rs` 快照 API | 对应 |
| `load_from_surface` / `load_from_pixbufsurface` / `load_from_numpy` | `load_from_surface` / `load_from_rgba8` | 对应 |
| `load_from_png` / `save_as_png` / `render_as_pixbuf` | `lib.rs:1447/1209`、`color.rs` 编码 | 对应 |
| `get_bbox` / `get_tiles` / `is_empty` / `remove_empty_tiles` | `lib.rs` 同名方法 | 对应 |
| `flood_fill` | `pigment-engine/src/mode/fill.rs:1127` | 对应 |
| `cairo_request` | 无 | 结构性替代（无 Cairo） |
| `_TiledSurfaceMove` | `layer.rs` 的移动 + `mode/layermove.rs` | 对应 |
| `Background` | `layer.rs` 的 `BackgroundImage` + `LayerStack` 背景 | 对应 |
| `PNGFileUpdateTask` | `pigment-surface/src/png.rs` 同步写 | 结构性替代 |

#### `lib/validation.py`

**状态：结构性替代**

上游 `validate(input_value, default_value, input_type, predicate, message)` 是通用运行时校验。本地在偏好/画笔解析处使用类型化解析与 `unwrap_or`/`Result`，没有通用校验函数。

#### `lib/xml.py`

**状态：对应**

| 上游符号 | 本地落点 | 状态 |
|---|---|---|
| `xsd2bool` | `pigment-engine/src/fileio.rs:1636` | 对应 |
| `escape` | `pigment-engine/src/accelmap.rs:616` + ORA XML 转义 | 对应 |
| `indent_etree` | 手写缩进/属性顺序（`fileio.rs` 的 `write_empty_element` 等） | 结构性替代 |

#### `lib/__init__.py` / `lib/layer/__init__.py`

**状态：不适用 / 结构性替代**

空包与再导出文件；Rust 模块直接导出类型。

---

## 5. MyPaint `lib/` 原生 C/C++ 扩展逐文件

### 5.1 文件状态总表

| 上游文件 | 本地落点 | 状态 |
|---|---|---|
| `lib/blending.hpp` | `hokusai-core/src/spectral.rs`、`pigment-surface/src/combine.rs` | 对应 |
| `lib/compositing.hpp` | `pigment-surface/src/combine.rs`、`hokusai-core/src/spectral.rs` | 对应 |
| `lib/brush.hpp` | `hokusai-core/src/{brush,state}.rs` | 对应 |
| `lib/brushsettings.cpp` / `.hpp` | `hokusai-core/src/{setting,input}.rs` | 结构性替代 |
| `lib/colorchanger_crossed_bowl.hpp` | 无色变器 UI/模型 | 缺失 |
| `lib/colorchanger_wash.hpp` | 无 | 缺失 |
| `lib/colorring.hpp` | 无 | 缺失 |
| `lib/common.hpp` | Rust 语言层 | 结构性替代 |
| `lib/fastpng.cpp` / `.hpp` | `pigment-surface/src/png.rs` | 对应 |
| `lib/fix15.hpp` | `hokusai-core/src/fix15.rs` | 对应 |
| `lib/gdkpixbuf2numpy.cpp` / `.hpp` | 无 | 缺失（GdkPixbuf/NumPy 专用） |
| `lib/helpers2.hpp` | `hokusai-core/src/{color,spectral,stroke}.rs` 的宏/常量等价物 | 部分 |
| `lib/mapping.hpp` | `hokusai-core/src/mapping.rs` | 对应 |
| `lib/mypaintlib.hpp` / `.i` | Rust crate 导出 | 结构性替代 |
| `lib/pixops.cpp` / `.hpp` | `pigment-surface/src/combine.rs`、`color.rs`、`thumb.rs`、`mipmap.rs` | 对应 |
| `lib/python_brush.hpp` | `hokusai-core` 公开 API | 结构性替代 |
| `lib/pythontiledsurface.cpp` / `.h` | `pigment-surface/src/lib.rs` | 结构性替代 |
| `lib/surface.hpp` | `hokusai-core/src/surface.rs` | 结构性替代 |
| `lib/tiledsurface.hpp` | `pigment-surface/src/lib.rs` | 对应 |
| `lib/fill/*` | `pigment-engine/src/mode/{fill,fill_gap,fill_morphology}.rs` | 对应 |
| `lib/fastapprox/*` | `hokusai-core/src/spectral.rs`、`pigment-surface/src/combine.rs` | 部分 |

### 5.2 逐文件明细

#### `lib/pixops.cpp` / `pixops.hpp`

**状态：对应**

| 上游职责 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| 22 个图层混合模式枚举与名称 | `pigment-surface/src/combine.rs:... LayerMode`（22 项） | 对应 | 顺序与 `pixops.hpp` 的 `NumCombineModes` 一致。 |
| `combine_pixels` 类的逐像素合成 | `combine.rs` 的 `tile_combine` / `generic` / `generic_loop` | 对应 | 含 Normal、Multiply、Screen、Overlay、Darken、Lighten、HardLight、ColorDodge、ColorBurn、SoftLight、Difference、Exclusion、Hue、Saturation、Color、Luminosity、Spectral WGM 等。 |
| 预乘 RGBA16 的乘加/归一化 | `combine.rs` 的 `mul` / `sumprods` / `double` / `div` | 对应 | |
| `sample_from_pixbuf` / 缩放 | `pigment-surface/src/thumb.rs`、`mipmap.rs` | 部分 | 无 GdkPixbuf 采样器；用 tile 降采样替代。 |
| `get_color` 采样 | `hokusai-core/src/brushmodes.rs:1709/1788` | 对应 | |
| fix15 → RGBA8 编码 | `pigment-surface/src/color.rs` 的 `tile_to_rgba8*` | 对应 | |
| ORA 的 PNG 像素格式 | `png.rs` + `fileio.rs` | 对应 | |

#### `lib/blending.hpp` / `lib/compositing.hpp`

**状态：对应**

| 上游概念 | 本地落点 | 状态 |
|---|---|---|
| 光谱（10 波段）混色 | `hokusai-core/src/spectral.rs` 的 `rgb_to_spectral` / `spectral_to_rgb` / `mix_colors` / `spectral_blend_factor` | 对应 |
| pigment / paint 模式 | `hokusai-core/src/stroke.rs` 的 `paint_mode` 分支 + `brushmodes.rs` 的 `_Paint` 系列 | 对应 |
| WGM 图层合成 | `pigment-surface/src/combine.rs` 的 `wgm` | 对应 |
| 逐通道非分离混合（Hue/Sat/Color/Lum） | `combine.rs` 的 `nonsep_*` | 对应 |
| GPU 版光谱混合 | `pigment-render/src/gpu_dabs.rs` 的 WGSL | 追加实现（非上游文件） |

#### `lib/brush.hpp` / `lib/python_brush.hpp`

**状态：对应 / 结构性替代**

`brush.hpp` 定义 `MyPaintBrush` 的 C++ 包装、`num_buckets = 256`、状态数组接口。本地：

- `hokusai-core/src/brush.rs` 的 `Brush`（设置与映射）；
- `hokusai-core/src/state.rs:19 NUM_SMUDGE_BUCKETS = 256` 与 `BrushState`；
- `hokusai-core/src/stroke.rs` 的笔画求值。

`python_brush.hpp` 是 SWIG/Python 绑定层，本地由 Rust 公开 API 取代。

#### `lib/brushsettings.cpp` / `.hpp`

**状态：结构性替代**

上游把 `brushsettings.json` 编译成 C 结构并暴露给 Python。本地把同一份数据固化为 `BrushSetting`/`BrushInput` 枚举（65 + 18），构建期元数据只用于画笔列表展示。没有 C 结构体 API。

#### `lib/colorchanger_crossed_bowl.hpp` / `colorchanger_wash.hpp` / `colorring.hpp`

**状态：缺失**

| 上游类 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `CrossedBowl` | 无 | 缺失 | 动作 `CrossedBowlColorChangerTool` 在 `app.rs:5573` 被识别，但 `shell/chrome.rs:1680` 只打开通用颜色面板，没有交叉碗选色器。 |
| `Wash` | 无 | 缺失 | 同上（`WashColorChangerTool`）。 |
| `Rings` | 无 | 缺失 | 同上（`RingsColorChangerTool`）。 |

检索 `CrossedBowl`、`colorchanger`、`ColorRing`、`Wash`（作为颜色器）在 `pigment/crates` 与 `hokusai/crates` 无实现。

#### `lib/common.hpp` / `lib/helpers2.hpp`

**状态：结构性替代 / 部分**

- `common.hpp` 是 Python/NumPy C API 的共享符号声明，Rust 不需要。
- `helpers2.hpp` 的 `MIN`/`MAX`/`ROUND`/`SIGN`/`CLAMP`/`SQR` 在 Rust 中以 `min/max/round/signum/clamp` 或局部函数出现；`hsv_to_rgb_range_one`（输出 0..255）没有同名函数，颜色面板使用 0..1 的 `hsv_to_rgb`（`hokusai-core/src/color.rs:124`）。

#### `lib/fastpng.cpp` / `.hpp`

**状态：对应**

| 上游职责 | 本地落点 | 状态 |
|---|---|---|
| 从内存/文件读 PNG | `pigment-surface/src/png.rs` 的 `decode_rgba8_png` / `read_rgba8_png` | 对应 |
| 写 PNG（RGBA/RGB、sRGB/gAMA/cHRM） | `png.rs` 的 `write_png` / `write_rgba8_png` | 对应 |
| 渐进式 PNG 写入 | 无渐进式任务；同步写 | 结构性替代 |

#### `lib/fix15.hpp`

**状态：对应**

`fix15` 的 `u16` 表示、`(a*b + (1<<14)) >> 15` 取整在 `hokusai-core/src/fix15.rs`。常量与位移一致；有 `hokusai-compat` 像素快照约束。

#### `lib/gdkpixbuf2numpy.cpp` / `.hpp`

**状态：缺失**

GdkPixbuf ↔ NumPy 的零拷贝转换。本地没有 GdkPixbuf 或 NumPy，像素在 `pigment-surface/src/color.rs` 的 `tile_to_rgba8*` 与 `vector.rs`/`thumb.rs` 之间以 Rust 切片传递。

#### `lib/mapping.hpp`

**状态：对应**

`MappingWrapper` 的构造/析构/设置点/求值对应 `hokusai-core/src/mapping.rs` 的 `InputMapping` 与 `eval`，以及 `brush.rs` 的访问器。

#### `lib/mypaintlib.hpp` / `lib/mypaintlib.i`

**状态：结构性替代**

SWIG 接口与模块初始化。本地是 Rust crate 的公开类型，没有 SWIG。

#### `lib/pythontiledsurface.cpp` / `.h`、`lib/surface.hpp`、`lib/tiledsurface.hpp`

**状态：结构性替代 / 对应**

| 上游职责 | 本地落点 | 状态 |
|---|---|---|
| Python tile surface ↔ C surface 适配 | `pigment-surface/src/lib.rs` 直接实现 `hokusai::surface::TiledSurface` | 结构性替代 |
| `Surface` 抽象接口 | `hokusai-core/src/surface.rs` 的 trait | 结构性替代 |
| `TiledSurface` 的 tile 请求/快照/合成/移动 | `pigment-surface/src/lib.rs`、`layer.rs` | 对应 |

#### `lib/fill/fill_common.cpp` / `.hpp` / `.swig`

**状态：对应**

`TileBoundingBox`、九宫格邻接、tile 边界判断在 `pigment-engine/src/mode/fill.rs`（`:720 TileBoundingBox`）与 `fill_gap.rs`（`:639 nine_grid`、`:655 orthogonal`）。

#### `lib/fill/floodfill.cpp` / `.hpp`

**状态：对应**

扫描线填充、种子、溢出队列、目标色判定在 `pigment-engine/src/mode/fill.rs`（`starting_coordinates`、`seeds_by_tile`、`get_target_color`、`enqueue_overflows`、`scanline_fill`、`flood_fill`、`composite`）。

#### `lib/fill/gap_detection.cpp` / `.hpp`

**状态：对应**

缺口检测在 `pigment-engine/src/mode/fill_gap.rs`（`GapClosingOptions`、`gap_closing_fill`、邻接遍历）。

#### `lib/fill/gap_closing_fill.cpp`

**状态：对应**

缺口闭合填充在 `pigment-engine/src/mode/fill_gap.rs:816 gap_closing_fill`。

#### `lib/fill/morphology.cpp` / `.hpp`、`lib/fill/blur.cpp` / `.hpp`

**状态：对应**

形态学与模糊在 `pigment-engine/src/mode/fill_morphology.rs:168 morph`、`:256 blur`。

#### `lib/fill/fill_constants.cpp` / `.hpp`

**状态：对应**

填充常量（tile 尺寸、邻接位移、最大迭代）在 `fill.rs` / `fill_gap.rs` / `fill_morphology.rs` 的常量中；没有独立文件。

#### `lib/fastapprox/*.h`

**状态：部分**

已移植：`fastlog2`、`fastpow2`、`fastpow`（`hokusai-core/src/spectral.rs:40-56`；`pigment-surface/src/combine.rs` 另有独立副本供图层合成使用）。

未移植：`fastexp.h`、`fastgamma.h`、`fasthyperbolic.h`、`fastlambertw.h`、`fastsigmoid.h`、`fasttrig.h`、`fasterf.h`、`sse.h`、`cast.h` 中的函数与向量化包装。MyPaint/libmypaint 的混合路径只使用 `fastlog2`/`fastpow` 子集，其余头文件没有本地落点。

---

## 6. MyPaint `gui/`（Python/GTK）逐文件

> GUI 栈从 GTK3/PyGObject 换为 egui/winit/wgpu。本节的“对应”指用户可见行为或动作处理链有落点，不表示控件类名一一对应。

### 6.1 文件状态总表

| 上游文件 | 本地落点 | 状态 |
|---|---|---|
| `gui/__init__.py` | 无 | 不适用 |
| `gui/accelmap.py` | `pigment-engine/src/accelmap.rs`、`pigment-app/src/shell/accelmap.rs` | 对应 |
| `gui/application.py` | `pigment-app/src/{app,main,prefs}.rs` | 部分 |
| `gui/autorecover.py` | `pigment-app/src/app.rs` + `shell/dialogs.rs` | 对应 |
| `gui/backgroundwindow.py` | `pigment-engine/src/document.rs`（引擎 API）；无选择界面 | 部分 |
| `gui/blendmodehandler.py` | `pigment-app/src/blend_modes.rs` | 对应 |
| `gui/brushcolor.py` | `pigment-engine/src/color/manager.rs`、`pigment-app/src/color.rs` | 对应 |
| `gui/brusheditor.py` | `pigment-app/src/editor.rs` | 对应 |
| `gui/brushiconeditor.py` | 仅 `subwindow.rs` 元数据 | 缺失 |
| `gui/brushmanager.py` | `pigment-engine/src/{brushlib,brushpack}.rs`、`pigment-app/src/brushes.rs` | 对应 |
| `gui/brushmanip.py` | `pigment-engine/src/mode/brushresize.rs`、`shell/overlays.rs` | 对应 |
| `gui/brushmodifier.py` | `pigment-engine/src/color/manager.rs`、`blend_modes.rs` | 对应 |
| `gui/brushselectionwindow.py` | `pigment-app/src/brushes.rs`、`shell/chrome.rs` | 对应 |
| `gui/builderhacks.py` | 无 | 结构性替代 |
| `gui/buttonmap.py` | `pigment-app/src/shell/button_mapping.rs` | 部分 |
| `gui/colorpicker.py` | `pigment-engine/src/mode/pick.rs` | 对应 |
| `gui/colorpreview.py` | `shell/chrome.rs` 的色块、`app.rs` 的 `color_swatch` | 对应 |
| `gui/colors/__init__.py` | 无 | 不适用 |
| `gui/colors/adjbases.py` | `color/manager.rs`、`color/tools.rs`、`color/wheels.rs` | 部分 |
| `gui/colors/bases.py` | 无 | 结构性替代 |
| `gui/colors/changers.py` | 无 | 缺失 |
| `gui/colors/combined.py` | 单页颜色面板 | 结构性替代 |
| `gui/colors/hcywheel.py` | `color/hcywheel.rs`（数学） | 部分 |
| `gui/colors/hsvcube.py` | `color/wheels.rs`（数学） | 部分 |
| `gui/colors/hsvsquare.py` | `color/wheels.rs`（数学） | 部分 |
| `gui/colors/hsvwheel.py` | `color/wheels.rs`（数学） | 部分 |
| `gui/colors/paletteview.py` | `color/palette.rs`、`shell/chrome.rs` 的调色板 | 部分 |
| `gui/colors/sliders.py` | 单页颜色面板只有 H/S/V 数值显示，无滑块 | 部分 |
| `gui/colors/uimisc.py` | egui 主题按钮 | 结构性替代 |
| `gui/colors/util.py` | `color/tools.rs`、`shell/overlays.rs` | 对应 |
| `gui/colortools.py` | 动作映射到单页颜色面板 | 部分 |
| `gui/compatconfig.py` | `pigment-engine/src/compat.rs`、`prefs.rs` | 对应 |
| `gui/compatibility.py` | `pigment-engine/src/compat.rs` | 对应 |
| `gui/cursor.py` | `shell/overlays.rs` 的 `CursorStyle` / `paint_brush_cursor` | 对应 |
| `gui/curve.py` | `pigment-app/src/editor.rs` 的 `curve_canvas` | 对应 |
| `gui/device.py` | `pigment-app/src/device.rs`、`prefs.rs` | 对应 |
| `gui/dialogs.py` | `pigment-app/src/shell/dialogs.rs`、`native_dialog.rs` | 部分 |
| `gui/displayfilter.py` | `pigment-render/src/display_filter.rs` | 对应 |
| `gui/document.py` | `pigment-engine/src/document.rs`、`pigment-app/src/app.rs` | 对应 |
| `gui/drawutils.py` | `pigment-app/src/thumb.rs`、`shell/overlays.rs`、构建期图标 | 部分 |
| `gui/drawwindow.py` | `pigment-app/src/app.rs` + `shell/*` | 结构性替代 |
| `gui/externalapp.py` | `pigment-app/src/app.rs` 的外部编辑轮询 | 部分 |
| `gui/factoryaction.py` | `pigment-app/src/shell/actions.rs` | 对应 |
| `gui/filehandling.py` | `pigment-engine/src/{fileio,scrap,scratchpad}.rs`、`app.rs` | 对应 |
| `gui/fill.py` | `pigment-engine/src/mode/fill.rs`、`shell/overlays.rs` | 对应 |
| `gui/footer.py` | `shell/chrome.rs` 的状态栏 / 笔刷指示 | 对应 |
| `gui/framewindow.py` | `pigment-engine/src/mode/frameedit.rs`、`shell/overlays.rs` | 对应 |
| `gui/freehand.py` | `pigment-engine/src/mode/freehand.rs` | 对应 |
| `gui/gtkexcepthook.py` | `pigment-app/src/main.rs` 的 panic hook | 结构性替代 |
| `gui/history.py` | `pigment-app/src/brushes.rs` 历史条、`shell/chrome.rs` | 对应 |
| `gui/historypopup.py` | 无 | 缺失 |
| `gui/inktool.py` | `pigment-engine/src/mode/ink.rs` | 对应 |
| `gui/inputtestwindow.py` | `pigment-app/src/shell/dialogs.rs:457` | 对应 |
| `gui/keyboard.py` | `pigment-app/src/shell/{actions,accelmap}.rs` | 对应 |
| `gui/layermanip.py` | `pigment-engine/src/mode/layermove.rs` | 对应 |
| `gui/layermodes.py` | `pigment-app/src/shell/actions.rs`（`LayerMode` 动作） | 对应 |
| `gui/layerprops.py` | `shell/chrome.rs` 的图层面板；无独立对话框 | 部分 |
| `gui/layers.py` | `shell/chrome.rs:1196 layers_panel` | 对应 |
| `gui/layerswindow.py` | `shell/chrome.rs` 的 Layers 面板 | 对应 |
| `gui/layervis.py` | `pigment-engine/src/layer_views.rs` + 命令；无 UI | 缺失 |
| `gui/linemode.py` | `pigment-engine/src/mode/line.rs` | 对应 |
| `gui/main.py` | `pigment-app/src/main.rs` | 对应 |
| `gui/meta.py` | `pigment-app/src/shell/dialogs.rs:390` 的 About | 对应 |
| `gui/mode.py` | `pigment-engine/src/mode/mod.rs` | 对应 |
| `gui/mvp.py` | 无 | 结构性替代 |
| `gui/objfactory.py` | `pigment-app/src/shell/actions.rs` | 对应 |
| `gui/optionspanel.py` | 无 | 缺失 |
| `gui/overlays.py` | `pigment-app/src/shell/overlays.rs` | 对应 |
| `gui/picker.py` | `pigment-app/src/app.rs` 的 `pick_context` / `pick_layer` | 对应 |
| `gui/pixbuflist.py` | `pigment-app/src/brushes.rs` 的网格/分组 | 对应 |
| `gui/preferenceswindow.py` | `pigment-app/src/shell/dialogs.rs:44` | 对应 |
| `gui/previewwindow.py` | `pigment-app/src/app.rs` 的 `PreviewTool` | 对应 |
| `gui/profiling.py` | `pigment-app/src/app.rs` 的 `ProfileStats` | 对应 |
| `gui/quickchoice.py` | 动作已注册；`ColorChooserPopup` 为空分支 | 部分 |
| `gui/scratchwindow.py` | `pigment-app/src/scratch.rs`、`shell/chrome.rs` | 对应 |
| `gui/sliderwidget.py` | 无；使用 egui 控件 | 结构性替代 |
| `gui/spinbox.py` | 无；使用 egui `DragValue` | 结构性替代 |
| `gui/stategroup.py` | 部分状态由 `app.rs` 的计时字段承载 | 结构性替代 |
| `gui/style.py` | `pigment-app/src/shell/theme.rs`、`shell/overlays.rs` | 对应 |
| `gui/symmetry.py` | `pigment-engine/src/mode/symmetryedit.rs`、`shell/overlays.rs` | 对应 |
| `gui/tileddrawwidget.py` | `pigment-app/src/shell/canvas.rs`、`pigment-render/src/canvas.rs` | 对应 |
| `gui/toolbar.py` | `pigment-app/src/shell/toolbars.rs`、`build.rs` | 对应 |
| `gui/toolstack.py` | `shell/chrome.rs` 的左右 dock | 对应 |
| `gui/topbar.py` | `shell/chrome.rs` 的顶栏 | 对应 |
| `gui/uicolor.py` | `pigment-engine/src/color/space.rs`、`model.rs` | 结构性替代 |
| `gui/userconfig.py` | `pigment-engine/src/prefs.rs:288` | 对应 |
| `gui/viewmanip.py` | `pigment-app/src/app.rs` 的视图工具 | 对应 |
| `gui/widgets.py` | 无；egui 控件 | 结构性替代 |
| `gui/windowing.py` | `pigment-app/src/subwindow.rs`、`shell31.rs`、`shell/dialogs.rs` | 部分 |
| `gui/workspace.py` | `pigment-app/src/shell/canvas.rs` | 对应 |

### 6.2 逐文件明细

#### `gui/__init__.py`

**状态：不适用**

空包文件。

#### `gui/accelmap.py`

**状态：对应**

`AccelMapEditor` 的快捷键表、冲突检测、按键捕获、`accelmap.conf` 读写落在 `pigment-engine/src/accelmap.rs`（数据/持久化）与 `pigment-app/src/shell/accelmap.rs`（编辑器 UI）。构建期从上游 `resources.xml` 生成动作表；该文件含 156 个 `GtkAction` 条目与 105 条 `<accelerator>`。

#### `gui/application.py`

**状态：部分**

| 上游符号/行为 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `Application` 生命周期、单文档、启动文件、恢复 | `pigment-app/src/main.rs`、`app.rs`、`scratch.rs` | 部分 | 本地只有一个主文档 + 暂存区；没有 GTK Application 的多窗口/多实例管理。 |
| `StateDirs` / `PixbufDirectory` / `CallbackFinder` | 无 | 结构性替代 | 目录由 `prefs.rs` 解析；图标在 `build.rs` 构建期栅格化。 |
| `get_app` | 无 | 结构性替代 | 无全局单例；`PigmentApp` 为拥有者。 |
| 设置加载/保存 | `pigment-engine/src/prefs.rs` | 对应 | |
| 按钮映射应用 | `app.rs` + `shell/button_mapping.rs` | 对应 | |

#### `gui/autorecover.py`

**状态：对应**

`Presenter` 的恢复列表、描述、打开/删除操作在 `pigment-app/src/app.rs` 的恢复流程与 `shell/dialogs.rs:683 recovery_window`。`lib/autosave.py` 的元数据在 `pigment-engine/src/autosave.rs`。

#### `gui/backgroundwindow.py`

**状态：部分**

| 上游行为 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| 从文件设置背景 | `pigment-engine/src/document.rs:978 set_background_from_file` | 对应 | 引擎 API 存在。 |
| 默认背景纸纹 | `document.rs:933 default_paper_background`、`:950 reset_background` | 对应 | |
| 背景列表 / 选择对话框 / 平铺设置 | 无 | 缺失 | `BackgroundWindow` 动作在 `app.rs:5617` 仅被列为 sensitive；没有处理分支与对话框。 |

#### `gui/blendmodehandler.py`

**状态：对应**

`BlendModes` 的四开关（eraser/lock_alpha/colorize/normal）与激活历史栈在 `pigment-app/src/blend_modes.rs:129`；与画笔设置的映射在 `brushlib.rs`。有往返/历史行为测试。

#### `gui/brushcolor.py`

**状态：对应**

`BrushColorManager` 的颜色状态、历史、`restore_color` 选笔混色在 `pigment-engine/src/color/manager.rs`、`pigment-app/src/color.rs`、`app.rs:3555`。

#### `gui/brusheditor.py`

**状态：对应**

`BrushEditorWindow` 的设置分组、曲线编辑、预览、导入/导出在 `pigment-app/src/editor.rs`（`BrushEditor::show`、`curve_canvas`）与 `shell/dialogs.rs:545 brush_editor_window`。

#### `gui/brushiconeditor.py`

**状态：缺失**

上游 `BrushIconEditorWindow` / `BrushIconEditor` 提供 256×256 图标绘制、预览、应用。本地只有：

- `subwindow.rs:26 Kind::BrushIconEditor` 的标题/默认尺寸；
- `app.rs:5654` 把 `BrushIconEditorWindow` 列为 sensitive。

没有打开处理分支、没有图标绘制 UI，`subwindow.rs:114` 的关闭逻辑为空。检索 `BrushIconEditor`、`icon_editor` 仅得到上述元数据。

#### `gui/brushmanager.py`

**状态：对应**

| 上游行为 | 本地落点 | 状态 |
|---|---|---|
| 官方/用户画笔扫描、按组排序 | `pigment-engine/src/brushlib.rs`（`indexed`/`BrushEntry`/`build.rs` 元数据） | 对应 |
| `.myb` 读写 | `hokusai-brush` + `brushlib.rs` | 对应 |
| 画笔包 `order.conf` 导入 | `pigment-engine/src/brushpack.rs` | 对应 |
| 设备画笔（`devbrush_*.myb`） | `pigment-app/src/device.rs`、`app.rs` | 对应 |
| `ManagedBrush` 的缩略图/元数据 | `pigment-app/src/brushes.rs`、`thumb.rs` | 对应 |
| 画笔管理器 UI（树、分组、右键菜单） | `pigment-app/src/brushes.rs`、`shell/chrome.rs` | 对应 |
| `translate_group_name` | `brushlib.rs:113` | 对应 |

#### `gui/brushmanip.py`

**状态：对应**

`BrushSizeOverlay` / `BrushResizeMode` 在 `pigment-engine/src/mode/brushresize.rs:60` 与 `shell/overlays.rs`。视觉半径公式 `exp(radius) + 2*exp(radius)*by_random` 在 `brushresize.rs:107`。

#### `gui/brushmodifier.py`

**状态：对应**

`BrushModifier` 的 eraser/lock_alpha/colorize 组合与选笔刷颜色混色在 `blend_modes.rs` 与 `color/manager.rs`；`restore_color` 行为在 `app.rs:3555/3724`。

#### `gui/brushselectionwindow.py`

**状态：对应**

`BrushList` / `BrushGroupTool` / `BrushPopupMenu` 的画笔网格、组导航、上下文菜单、历史在 `pigment-app/src/brushes.rs`（`BrushPanel`、`BrushHistoryEntry`）与 `shell/chrome.rs` 的 brush panel。没有独立 GTK 窗口，作为停靠面板承载。

#### `gui/builderhacks.py`

**状态：结构性替代**

GtkBuilder 模板字符串补丁。egui 没有 GtkBuilder；布局在 `shell/chrome.rs`、`shell/dialogs.rs` 手工构建。

#### `gui/buttonmap.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `button_press_name` / `displayname` / `parse` | `shell/button_mapping.rs:39/54/93` | 对应 | |
| `get_handler_object` / 动作分派 | `shell/canvas.rs:227` + `button_mapping.rs:160 resolve` | 对应 | |
| `ButtonMapping` 数据模型 | `button_mapping.rs:24 ButtonBinding` | 对应 | |
| `ButtonMappingEditor` 编辑器 UI | 无 | 缺失 | 偏好页只应用默认/解析后的映射；没有按钮映射编辑界面。 |

#### `gui/colorpicker.py`

**状态：对应**

`ColorPickMode`、`PickVariant::{Full,Hue,Chroma,Luma}`、拖拽队列、修饰键回退在 `pigment-engine/src/mode/pick.rs:236/177`。`ColorPickPreviewOverlay` 的预览由 `shell/overlays.rs` 的取色覆盖层承担。

#### `gui/colorpreview.py`

**状态：对应**

`BrushColorIndicator` 由 `shell/chrome.rs` 的色块与 `app.rs:483 color_swatch` 承担；历史色块在颜色面板。

#### `gui/colors/__init__.py`

**状态：不适用**

空文件。

#### `gui/colors/adjbases.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `ColorManager` | `pigment-engine/src/color/manager.rs:83` | 对应 | 当前色、历史、调整器基类数据。 |
| `ColorAdjuster` / 颜色运算 | `color/tools.rs:138` | 对应 | |
| `HueSaturationWheelMixin` 的数学 | `color/wheels.rs` | 对应 | |
| `ColorAdjusterWidget` / `IconRenderableColorAdjusterWidget` / `PreviousCurrentColorAdjuster` / `SliderColorAdjuster` 的控件层 | 单页颜色面板 | 部分 | 控件层次被压平；没有“可切换调整器页面”的容器。 |

#### `gui/colors/bases.py`

**状态：结构性替代**

`CachedBgWidgetMixin` / `CachedBgDrawingArea` / `IconRenderable` 是 GTK 绘制缓存基础设施。egui 的纹理缓存与 `shell/chrome.rs` 的图标缓存承担。

#### `gui/colors/changers.py`

**状态：缺失**

`CrossedBowl` / `Wash` / `Rings` 三个颜色变换器没有实现。动作存在，但只打开通用颜色面板（`shell/chrome.rs:1678-1682`）。

#### `gui/colors/combined.py`

**状态：结构性替代**

`CombinedAdjusterPage` 是 GTK 页面容器；本地颜色面板是单页固定布局。

#### `gui/colors/hcywheel.py`

**状态：部分**

HCY 轮的几何、凸包、遮罩、颜色定位数学在 `pigment-engine/src/color/hcywheel.rs`（含 `convex_hull`、`point_in_convex_poly`、轮盘求色等）。缺失：

- HCY 轮/遮罩的 GTK 绘制控件；
- `HCYMaskTemplateDialog` / `HCYMaskPropertiesDialog`；
- 遮罩导入/导出入口（`app.rs:4276` 明确说明随颜色器界面移除）。

#### `gui/colors/hsvcube.py`

**状态：部分**

`tumble_faces`、`depth_slider_*`、切片求色在 `color/wheels.rs`；立方体页面与深度滑条 UI 没有实现。

#### `gui/colors/hsvsquare.py`

**状态：部分**

外环/内方块的求色与定位在 `color/wheels.rs`（`hsv_ring_*`、`slice_*`）；页面控件没有实现。

#### `gui/colors/hsvwheel.py`

**状态：部分**

HSV 轮的极坐标求色/定位在 `color/wheels.rs`；页面控件没有实现。

#### `gui/colors/paletteview.py`

**状态：部分**

调色板模型、匹配、添加、前后步进、GIMP `.gpl` 读写已移植（`color/palette.rs`、`app.rs` 的 load/save 文件弹层、`shell/chrome.rs` 的色块网格）。缺失：

- 独立 `PaletteEditorDialog`（列数、名称、重命名、删除、拖拽排序）；
- `PalettePage` 的完整编辑控件。

#### `gui/colors/sliders.py`

**状态：部分**

上游的组件滑块（RGB/HSV/HCY）没有 UI。本地颜色面板只显示 H/S/V 数值标签（`shell/chrome.rs:1004-1018`），颜色修改通过方块、色相条、笔刷/颜色动作完成。滑块的颜色数学未单独暴露。

#### `gui/colors/uimisc.py`

**状态：结构性替代**

`borderless_button` 是 GTK 样式辅助；egui 主题按钮在 `shell/theme.rs`。

#### `gui/colors/util.py`

**状态：对应**

`clamp` → `color/tools.rs:113`；`add_distance_fade_stops` → `color/wheels.rs:894 distance_fade_stops`；`draw_marker_circle` → `shell/overlays.rs` 的标记绘制。

#### `gui/colortools.py`

**状态：部分**

`HCYWheelTool` / `HSVWheelTool` / `PaletteTool` / `HSVCubeTool` / `HSVSquareTool` / `ComponentSlidersTool` / `WashColorChangerTool` / `RingsColorChangerTool` / `CrossedBowlColorChangerTool` 的动作 id 全部存在。实际行为：全部打开同一个通用颜色面板（`shell/chrome.rs:1669-1683`），没有按工具切换调整器页面。`ColorAdjustersToolItem` 的工厂菜单没有对应 UI。

#### `gui/compatconfig.py`

**状态：对应**

`CompatFileBehaviorConfig` 的 1.x/2.x 文件行为在 `pigment-engine/src/compat.rs` 与偏好页；`pigment_on_by_default`、`default_mode` 等字段在 `prefs.rs`/`brushlib.rs`。

#### `gui/compatibility.py`

**状态：对应**

`CompatFileBehavior` / `CompatibilityPreferences` / `set_compat_mode` / ORA 兼容检查在 `pigment-engine/src/compat.rs`。`incompatible_ora_warning_dialog` 的提示由 `shell/dialogs.rs` 的文件警告承担。

#### `gui/cursor.py`

**状态：对应**

`get_brush_cursor` / `draw_brush_cursor` / `cursor_line_segments` / `cursor_arc_segments` 在 `shell/overlays.rs:967/846/823`；`get_move_cursor_name_for_angle` 由 egui 系统光标映射承担。`CustomCursorMaker` 的位图光标没有 1:1 实现。

#### `gui/curve.py`

**状态：对应**

`CurveWidget` 的点编辑、插入、拖拽、曲线绘制在 `pigment-app/src/editor.rs:437 curve_canvas`，与画笔编辑器共用。没有独立 GTK 控件。

#### `gui/device.py`

**状态：对应**

`AllowedUsage` / `ScrollAction` / 设备设置 / 设备画笔 / 显示名在 `pigment-app/src/device.rs`（`ScrollAction` 在 `:135`）与 `prefs.rs`。`SettingsEditor` 的 GTK 页面没有同名类；设置在偏好页的输入设备部分呈现。

#### `gui/dialogs.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `confirm` | `shell/dialogs.rs` 的 `confirm_dialog` | 对应 | |
| `ask_for_name` | `shell/dialogs.rs` 的图层/视图重命名弹层 | 对应 | |
| `error` | `shell/dialogs.rs` 的 `file_warning_dialog` / 状态栏 | 部分 | 没有通用错误对话框。 |
| `confirm_rewrite_brush` / `confirm_rewrite_group` | 无 | 缺失 | 按符号名检索无结果。 |
| `open_dialog` / `save_dialog` | `native_dialog.rs` + `app.rs` 的文件弹层 | 对应 | |
| `confirm_brushpack_import` | `shell/dialogs.rs:953` readme + `app.rs:2655` 导入 | 对应 | |
| `ask_for_color` | 无 | 缺失 | 没有模态取色对话框；颜色通过颜色面板修改。 |

#### `gui/displayfilter.py`

**状态：对应**

`luma_only` / `invert_colors` / `sim_deuteranopia` / `sim_protanopia` / `sim_tritanopia` 在 `pigment-render/src/display_filter.rs`；GPU/CPU 显示路径都接入。

#### `gui/document.py`

**状态：对应**

`CanvasController` 的视图变换、当前模式、输入分派、命令提交在 `pigment-app/src/app.rs` 与 `shell/canvas.rs`；`Document` 的文档行为在 `pigment-engine/src/document.rs:122`。

#### `gui/drawutils.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `spline_4p` / `spline_iter` | `mode/freehand.rs:317`、`mode/ink.rs:101`、`thumb.rs:80/95` | 对应 | |
| `render_brush_preview_pixbuf` | `pigment-app/src/thumb.rs` + 构建期画笔预览 | 对应 | |
| `render_checks` | `thumb.rs` 的棋盘格预览 | 对应 | |
| `load_symbolic_icon` | `build.rs` 的 SVG→PNG 图标生成 + `shell/chrome.rs` | 对应 | |
| `render_round_floating_button` / `render_round_floating_color_chip` / `render_drop_shadow` / `get_drop_shadow_offsets` | 部分由 `shell/overlays.rs`/`theme.rs` 承担 | 部分 | 没有同名绘制函数；圆角/阴影由 egui 绘制。 |
| `_variable_pressure_scribble` | 无 | 缺失 | 测试预览辅助，按符号检索无结果。 |

#### `gui/drawwindow.py`

**状态：结构性替代**

`DrawWindow` 是 GTK 主窗口/子窗口基类，包含菜单、工具栏、停靠、弹出菜单、全屏等回调。本地没有 GTK 窗口类；这些行为分布在 `pigment-app/src/app.rs`（动作/状态）、`shell/chrome.rs`（布局）、`shell/overlays.rs`（覆盖层）、`shell31.rs`（winit 窗口）。用户可见行为有对应，类结构不可逐方法对应。

#### `gui/externalapp.py`

**状态：部分**

| 上游行为 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| 外部编辑器启动与环境恢复 | `pigment-app/src/app.rs:1509 launch_external_editor` + `ExternalEditState` 轮询 | 对应 | |
| `LayerEditManager` | `app.rs` 的外部编辑轮询/提交 | 对应 | |
| `OpenWithDialog`（选择打开方式） | 无 | 缺失 | 按 `OpenWith` 检索无 UI；外部编辑使用固定启动逻辑。 |

#### `gui/factoryaction.py`

**状态：对应**

`FactoryAction`（运行期生成子项的占位动作）在 `pigment-app/src/shell/actions.rs:59 Kind` 中作为工厂动作类型处理，`build.rs` 生成动作表。

#### `gui/filehandling.py`

**状态：对应**

`FileHandler` 的新建/打开/保存/另存/导入/导出/最近文件/Scrap 在 `pigment-app/src/app.rs`、`native_dialog.rs`、`pigment-engine/src/fileio.rs`、`scrap.rs`。`_IOProgressUI` 的进度对话框没有实现（见 `lib/feedback.py`）。

#### `gui/fill.py`

**状态：对应**

`FloodFillMode` / `FloodFillOverlay` / `FillOptions` 在 `pigment-engine/src/mode/fill.rs`（`FillOptions`、`flood_fill`）与 `shell/overlays.rs`、`app.rs:4796 fill_options_from_prefs`。`FloodFillOptionsWidget` 没有独立控件；选项来自偏好/菜单。`FlatLayerList` 的图层取样列表由图层面板选择承担。

#### `gui/footer.py`

**状态：对应**

`BrushIndicatorPresenter` 的当前笔刷/颜色指示在 `shell/chrome.rs` 的状态栏/笔刷面板；没有独立 presenter 类。

#### `gui/framewindow.py`

**状态：对应**

`FrameEditMode` / `FrameOverlay` 在 `pigment-engine/src/mode/frameedit.rs:24`、`shell/overlays.rs:240`。`FrameEditOptionsWidget` / `UnitAdjustment` 的独立控件没有实现；画框尺寸通过画框拖拽手柄调整，偏好中只有 `frame.color_rgba`。

#### `gui/freehand.py`

**状态：对应**

`FreehandMode` / `PressureAndTiltInterpolator` 在 `pigment-engine/src/mode/freehand.rs:536/342`；压力/倾斜插值、设备切换 reset、平滑参数均接入。

#### `gui/gtkexcepthook.py`

**状态：结构性替代**

GTK 异常对话框。本地 `pigment-app/src/main.rs` 安装 panic hook 并写日志；没有 GTK 异常对话框。

#### `gui/history.py`

**状态：对应**

`BrushHistoryView` / `ManagedBrushPreview` / `ColorHistoryView` / `ColorPreview` / `HistoryPanel` 在 `pigment-app/src/brushes.rs`（画笔历史）与 `shell/chrome.rs`（颜色历史条、色块）。历史长度/回退逻辑在 `brushes.rs:139/155/401`。

#### `gui/historypopup.py`

**状态：缺失**

上游 `HistoryPopup` 是撤销/重做历史的弹出列表。本地没有同名符号、没有撤销历史弹层；只有 Undo/Redo 动作与命令栈。检索 `HistoryPopup`、`undo history popup` 无结果。

#### `gui/inktool.py`

**状态：对应**

`InkingMode` / 节点 / 捕获与调整阶段 / 覆盖层在 `pigment-engine/src/mode/ink.rs:144`；样条与插值在 `mode/ink.rs`。

#### `gui/inputtestwindow.py`

**状态：对应**

`InputTestWindow` 的输入设备列表、运动统计、事件日志在 `pigment-app/src/shell/dialogs.rs:457` 与 `app.rs` 的 `input_test_*` 字段。

#### `gui/keyboard.py`

**状态：对应**

`KeyboardManager` 的键名解析、快捷键分派、动作到键的映射在 `shell/actions.rs`（键名表/`egui_key_for`）与 `shell/accelmap.rs`。

#### `gui/layermanip.py`

**状态：对应**

`LayerMoveMode` 在 `pigment-engine/src/mode/layermove.rs:18`；拖动平移图层、抬笔提交 `MoveLayer`。

#### `gui/layermodes.py`

**状态：对应**

`LayerModeMenuItem` 是工厂动作；图层模式动作 id 与 22 种模式在 `shell/actions.rs` 与 `combine.rs`。

#### `gui/layerprops.py`

**状态：部分**

`LayerPropertiesUI` 的名称/模式/可见/锁定/不透明度在 `shell/chrome.rs:1196 layers_panel` 内联。`LayerPropertiesDialog`（独立 F2 对话框）没有实现：`app.rs:5637` 的 F2 动作只 reveal 图层面板。`make_preview` 的图层预览由缩略图承担。

#### `gui/layers.py`

**状态：对应**

`RootStackTreeModelWrapper` / `RootStackTreeView` 的图层树、拖拽重排、可见/锁定、混合模式下拉在 `shell/chrome.rs:1196 layers_panel` 与 `LayerStack` 的 `bubble/insert/remove`。没有 GtkTreeView 模型。

#### `gui/layerswindow.py`

**状态：对应**

`LayersTool` 停靠面板在 `shell/chrome.rs` 的 Layers 面板（`ToggleLayersTool`/`RevealLayersTool`）。

#### `gui/layervis.py`

**状态：缺失**

上游 `LayerViewUI` 提供视图列表、添加/删除/重命名/锁定/切换。本地只有模型与命令（`layer_views.rs`、`command.rs:1075-1218`）；没有 UI 入口：`app.rs:854 layer_view_rename` 字段未被读取，动作表中也没有 LayerView 动作。

#### `gui/linemode.py`

**状态：对应**

`LineModeSettings` / 直线、序列、椭圆模式 / 几何辅助函数在 `pigment-engine/src/mode/line.rs`（`LineModeSettings:33`，`starting_point_for_ellipse:257`、`constrain_to_angle:238` 等）。`LineModeOptionsWidget`/`LineModeCurveWidget` 没有独立控件。

#### `gui/main.py`

**状态：对应**

命令行参数、locale 初始化、GTK 启动在 `pigment-app/src/main.rs`。没有 GTK；使用 winit 事件循环。

#### `gui/meta.py`

**状态：对应**

`run_about_dialog` 在 `shell/dialogs.rs:390 about_window`；版本/依赖信息由 Cargo 元数据提供。

#### `gui/mode.py`

**状态：对应**

`Behavior` / `InteractionMode` / `BrushworkModeMixin` / `SingleClickMode` / `DragMode` / `OneshotDragMode` / `ModeStack` 在 `pigment-engine/src/mode/mod.rs:51/306/740/745/787/822`。`ModeRegistry` 由模式动作表承担；`_NullMode` 无对应。

#### `gui/mvp.py`

**状态：结构性替代**

MVP/Presenter 框架用于 GTK UI 构建。egui 直接构建 UI；无 `Presenter`/`BuiltUIPresenter`/`_ViewWrapper`。

#### `gui/objfactory.py`

**状态：对应**

`ObjFactory` / `ConstructError` 的动态对象构造由 `shell/actions.rs` 的动作表与 `build.rs` 生成的菜单/工具栏承担。

#### `gui/optionspanel.py`

**状态：缺失**

上游 `ModeOptionsTool` 显示当前模式的选项页。本地：

- `ToggleModeOptionsTool` / `RevealModeOptionsTool` 动作已注册；
- 处理分支为空（`app.rs:6544-6547`）；
- `shell/chrome.rs:1693-1697` 注释明确“工具选项页尚未在新 chrome 中恢复”。

没有模式选项面板。

#### `gui/overlays.py`

**状态：对应**

`Overlay` / `FadingOverlay` / `ScaleOverlay` / `LastPaintPosOverlay` 在 `pigment-app/src/shell/overlays.rs`（淡出、比例提示、最后绘制位置、框选等）。`rounded_box` 由 egui 绘制辅助承担。

#### `gui/picker.py`

**状态：对应**

`PickingGrabPresenter` / `ContextPickingGrabPresenter` / `ColorPickingGrabPresenter` / `ButtonPresenter` 的取色交互在 `pigment-app/src/app.rs` 的 `pick_context`/`pick_layer` 与 `pigment-engine/src/mode/pick.rs`。

#### `gui/pixbuflist.py`

**状态：对应**

`PixbufList` 的图标网格、选中、滚动在 `pigment-app/src/brushes.rs:180 BrushPanel` 与 `shell/chrome.rs` 的画笔网格。

#### `gui/preferenceswindow.py`

**状态：对应**

`PreferencesWindow` 的常规/输入设备/画笔/颜色/兼容性页面在 `shell/dialogs.rs:44 preferences_window`；locale 下拉由 `locales_generated.rs` 提供。没有 GTK Notebook。

#### `gui/previewwindow.py`

**状态：对应**

`PreviewTool` / `VisibleAreaOverlay` / 预览面板缩放/平移在 `pigment-app/src/app.rs:549/855` 与 `shell/chrome.rs` 的 Preview 面板。

#### `gui/profiling.py`

**状态：对应**

`Profiler` 的启停、结果保存（`.txt`/`.dot`/可选 PNG）在 `pigment-app/src/app.rs:1082/1196/6056 ProfileStats`。没有 cProfile；统计来源为本地帧/内存计数器。

#### `gui/quickchoice.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `QuickBrushChooser` / `BrushChooserPopup` | 动作 `BrushChooserPopup` 已注册（`app.rs:5486`） | 缺失 | 没有处理分支，没有快速画笔弹层。 |
| `QuickColorChooser` / `ColorChooserPopup` / `Advanceable` | 动作 `ColorChooserPopup` / `ColorChooserPopupFastSubset` | 缺失 | `app.rs:6559` 是空分支 `=> {}`。 |

#### `gui/scratchwindow.py`

**状态：对应**

`ScratchpadTool` 在 `pigment-app/src/scratch.rs`（独立文档、模式栈、自动保存）与 `shell/chrome.rs` 的暂存区面板；`app.rs` 有 8 个暂存区动作。

#### `gui/sliderwidget.py`

**状态：结构性替代**

`InputSlider` / `ScaleDelegator` 是 GTK 尺度控件。本地用 egui `Slider`/`DragValue`；没有同名类。注意：颜色面板没有实现组件滑块 UI（见 `gui/colors/sliders.py`）。

#### `gui/spinbox.py`

**状态：结构性替代**

`ItemSpinBox` 由 egui `DragValue`/下拉承担。

#### `gui/style.py`

**状态：对应**

上游的棋盘格尺寸/颜色、浮动按钮尺寸、拖拽点/边宽、画布信息色、阴影参数等常量散落在 `pigment-app/src/shell/theme.rs`（面板/线条/文字色）与 `shell/overlays.rs`（棋盘格、画框、对称轴、最后绘制位置标记）。本地没有单一 `style.py` 常量文件；`shell/overlays.rs` 与 `thumb.rs` 的注释直接引用 `style.py` 行号核对颜色/线宽。

#### `gui/stategroup.py`

**状态：结构性替代**

`StateGroup` / `State` 是 GTK 临时状态组（弹出层、按钮状态）。本地用 `app.rs` 的计时字段与动作分支；没有通用状态组类型。`ColorHistoryPopup` 的弹出状态没有对应实现。

#### `gui/symmetry.py`

**状态：对应**

`SymmetryEditMode` 在 `pigment-engine/src/mode/symmetryedit.rs:17`；覆盖层、轴线、中心/角度拖拽在 `shell/overlays.rs:322`；对称状态落到 `PigmentSurface::set_symmetry_state`。`SymmetryEditOptionsWidget` 没有独立控件。

#### `gui/tileddrawwidget.py`

**状态：对应**

`TiledDrawWidget` / `CanvasTransformation` / `DrawCursorMixin` / `CanvasRenderer` 的视图变换、tile 绘制、缩放/旋转、光标在 `pigment-app/src/shell/canvas.rs`、`pigment-render/src/{canvas,view}.rs`。低缩放 mipmap 与 GPU dab 是本地追加路径，不是上游类。

#### `gui/toolbar.py`

**状态：对应**

`ToolbarManager` 的工具栏 XML、图标、动作在 `pigment-app/build.rs`（解析上游 toolbar XML）+ `shell/toolbars.rs`。

#### `gui/toolstack.py`

**状态：对应**

`ToolStack` / `ToolStackWindow` / `SizedVBoxToolWidget` 的停靠面板、显示/隐藏、尺寸在 `shell/chrome.rs` 的左右 dock 与 `app.rs` 的面板开关动作。没有 GtkNotebook。

#### `gui/topbar.py`

**状态：对应**

`TopBar` / `FakeMenuButton` 的顶栏、主菜单按钮、全屏/自动隐藏行为在 `shell/chrome.rs:235 chrome_topbar`。

#### `gui/uicolor.py`

**状态：结构性替代**

GdkColor/GdkRGBA/拖拽数据转换。本地颜色在 `pigment-engine/src/color/{model,space}.rs` 内以 `[f32;3]`/结构体表示；剪贴板颜色在 `shell31.rs`。

#### `gui/userconfig.py`

**状态：对应**

`default_configuration` / 平台默认值 / `settings.json` 读写 / 按钮映射默认在 `pigment-engine/src/prefs.rs:288` 与 `:236/257`。

#### `gui/viewmanip.py`

**状态：对应**

`PanViewMode` / `ZoomViewMode` / `RotateViewMode` 在 `pigment-app/src/app.rs` 的视图工具状态与 `shell/canvas.rs` 的拖拽处理；`FitView` / 视图重置在 `app.rs`。

#### `gui/widgets.py`

**状态：结构性替代**

GTK 小部件辅助（无边框按钮、边距、内联工具栏、等待光标）。egui 主题/控件在 `shell/theme.rs`、`shell/toolbars.rs`。

#### `gui/windowing.py`

**状态：部分**

| 上游符号 | 本地落点 | 状态 | 事实 |
|---|---|---|---|
| `SubWindow` 标题/默认尺寸/几何 | `pigment-app/src/subwindow.rs` | 对应 | 额外把几何写入偏好。 |
| OS 独立子窗口 | `shell31.rs` 有 `SubWindow` 结构 | 部分 | `subwindow.rs:98-104` 明确当前主窗口内以浮层承载，`is_open` 恒 `false`。 |
| `Dialog` / `PopupWindow` / `ChooserPopup` | `shell/dialogs.rs` 的 egui 浮层 | 结构性替代 | 无窗口管理器级弹层。 |
| `set_initial_window_position` / `_final_rectangle` / `_get_target_area_geometry` | `app.rs:258 final_window_rectangle` + `shell31.rs` 窗口定位 | 对应 | |

#### `gui/workspace.py`

**状态：对应**

`Workspace` 的画布 + 工具栏 + 状态栏布局在 `pigment-app/src/shell/canvas.rs` 与 `shell/chrome.rs`（deleted `shell/workspace.rs` 的内容已并入）。

---

## 7. 上游数据与资源

### 7.1 画笔包

| 项目 | 上游 | 本地 | 事实 |
|---|---|---|---|
| `.myb` 数量 | 196 | 196 | 路径集合完全一致。 |
| 文件内容 | `mypaint-brushes/brushes` | `pigment-engine/assets/brushes` | 196 个文件逐字节相同（`cmp` 全通过）。 |
| 画笔元数据 | 目录结构 + `order.conf` | 构建期生成 JSON 元数据 | 展示名/分组/备注进入画笔列表。 |

### 7.2 UI 定义

| 文件 | 结果 |
|---|---|
| `gui/menu.xml` | 与 `pigment-app/assets/gui/menu.xml` 逐字节相同。 |
| `gui/resources.xml` | 逐字节相同。 |
| `gui/toolbar.xml` | 逐字节相同。 |
| `gui/toolbar-{blendmodes,edit,file,linemodes,scrap,view-manips,view-modes,view-resets}.xml` | 8 个文件全部逐字节相同。 |

本地构建期解析这些 XML 生成动作表、菜单和工具栏（`pigment-app/build.rs`）。渲染由 egui 完成，不是 GTK。

### 7.3 其他数据

| 项目 | 上游数量 | 本地数量 | 事实 |
|---|---|---|---|
| 调色板 `.gpl` | 19 | 1（`MyPaint_Default.gpl`） | 该文件逐字节相同；其余 18 个未随应用打包。 |
| 背景图 | `backgrounds/` 下 87 个文件 | 2 个文件（`README.txt` + `mamurk_e_1.png`） | 默认纸纹 `mamurk_e_1.png` 逐字节相同。 |
| SVG 图标 | 176 个（含桌面/主题图标） | 118 个（`pigment-app/assets/icons`） | 本地图标由上游 symbolic SVG 栅格化；数量少于上游全集。 |
| 内置画笔预览 | 运行时由 GdkPixbuf 渲染 | 构建期/运行时渲染 | `thumb.rs` 与 `build.rs` 分工。 |

### 7.4 格式契约

| 格式 | 上游实现 | 本地实现 | 事实 |
|---|---|---|---|
| `.myb` v3 JSON | `lib/brush.py` + libmypaint | `hokusai-brush` | 65 setting / 18 input；未知键保留往返。 |
| `.myb` v2 旧文本 | `lib/brush.py` `_oldfmt_*` | 无 | 不支持。 |
| `.ora` | `lib/document.py` / `lib/layer/*` | `pigment-engine/src/fileio.rs` + `zip.rs` | ZIP_STORED 写、deflate 读；stack.xml 手写。 |
| `.png` | `lib/surface.py` / `lib/pixbuf.py` | `pigment-surface/src/png.rs` | sRGB/gAMA/cHRM 辅助块。 |
| strokemap `.dat` | `lib/strokemap.py` | `pigment-surface/src/strokemap.rs` | zlib 压缩 tile 位图；有真实字节对照测试。 |
| `accelmap.conf` | `gui/accelmap.py` | `pigment-engine/src/accelmap.rs` | 读写与冲突检测。 |
| `settings.json` | `gui/userconfig.py` | `pigment-engine/src/prefs.rs` | 键结构与平台默认值。 |
| 自动保存 | `lib/document.py` + `gui/autorecover.py` | `pigment-engine/src/autosave.rs` + `app.rs` | 缩略图/stack.xml/settings 分项写入。 |

---

## 8. 当前不等价事实

本节只列当前代码与上游的**已知差异**，不涉及修复顺序或历史。

### 8.1 引擎数值/行为

| 项 | 上游 | 本地当前事实 |
|---|---|---|
| GPU 延迟 smudge 采样 | `smudge_length_log=0` 时每个 dab 重采样 | GPU 路径默认每 8 个 dab 采样一次（`stroke.rs:1190` 读取 `PIGMENT_SMUDGE_SAMPLE_DABS`，默认 8）；设为 1 可逐 dab。 |
| GPU 光栅化数值 | CPU fix15 参考 | GPU 用 WGSL 浮点实现；`pigment-render/examples/gpu_dab_check.rs` 提供逐像素对照工具，本报告未重新量化误差。 |
| 低缩放显示 | 全分辨率 tile 直接缩放 | `pigment-surface` 的 mipmap 预览 + Instant Preview 是本地追加路径；最终像素由全分辨率事件重放保证，预览本身不是上游行为。 |
| 稀疏取色 RNG | 进程 libc `rand()` | Windows/MSVC 路径镜像 `holdrand*214013+2531011`；非 MSVC 使用 Park–Miller，不保证与当地 libc 逐位一致（`brushmodes.rs` 的 `c_rand`）。 |
| `.myb` v2 | 支持旧文本格式迁移 | 只支持 v3 JSON。 |
| 额外输入 `tilt` | 无 | `BrushInput::Tilt` 为本地扩展；使用该非标准输入的画笔文件行为不同。 |
| `Brush::stroke_to` 便捷封装 | 上游单一入口 | 固定 `viewzoom=1, viewrotation=0, barrel=0, linear=false`；应用走 `stroke_to_full`。 |
| `mypaint_brush_new_with_buckets` | 可变 bucket 数 | 固定 256；MyPaint 应用本身也是 256。 |
| `mypaint_brush_set_print_inputs` | brush 对象开关 | 环境变量 + 应用动作，没有对象级 API。 |
| `settings_base_values_have_changed` | 脏标记 | 每事件全量重算，无脏标记 API。 |
| 异步 operation queue | FIFO/operationqueue 线程模型 | 同步 tile 写 + 可预算 flush + GPU sink；调用时序不同。 |
| Python/SWIG/GEGL/GTK | 上游可选集成 | 不存在；本地是纯 Rust + egui/wgpu。 |

### 8.2 GUI 功能

| 上游功能 | 本地当前事实 |
|---|---|
| 颜色调整器页面（HCY 轮、HSV 轮、HSV 方块、HSV 立方、组件滑块、Wash/Rings/CrossedBowl） | 数学在 `pigment-engine/src/color/{hcywheel,wheels,tools}.rs`；UI 只有单页 HSV 方块 + 色相条 + 数值标签 + 历史 + 调色板。工具动作全部打开同一面板。 |
| 模式选项面板（`gui/optionspanel.py`） | 动作已注册；处理分支为空；`shell/chrome.rs:1693-1697` 注释明确未恢复。 |
| 图层视图 UI（`gui/layervis.py`） | 模型与命令已移植；无 UI 入口；`app.rs:854 layer_view_rename` 字段未使用。 |
| 背景选择窗口（`gui/backgroundwindow.py`） | 引擎 `set_background_from_file` 存在；`BackgroundWindow` 动作无处理分支，无背景列表/平铺设置 UI。 |
| 画笔图标编辑器（`gui/brushiconeditor.py`） | 只有 `subwindow.rs` 标题/尺寸元数据；无绘制/应用 UI。 |
| 快速选择弹层（`gui/quickchoice.py`） | `BrushChooserPopup` 无处理器；`ColorChooserPopup` 是空分支。 |
| 撤销历史弹层（`gui/historypopup.py`） | 无 `HistoryPopup` 实现。 |
| 图层属性对话框（`gui/layerprops.py`） | F2 只 reveal 图层面板；无独立对话框。 |
| 按钮映射编辑器（`gui/buttonmap.py`） | 解析/分派已移植；无编辑器 UI。 |
| 进度反馈（`lib/feedback.py`） | 无进度对象/进度条；保存/加载/填充同步执行。 |
| 画笔包/调色板完整编辑器 | 画笔包导入已移植；调色板只有加载/保存文件弹层与色块网格，无完整编辑对话框。 |

### 8.3 资源

| 项 | 事实 |
|---|---|
| 调色板 | 19 个上游 `.gpl` 中只打包 `MyPaint_Default.gpl`。 |
| 背景 | 87 个上游背景文件中只打包 `mamurk_e_1.png`。 |
| 图标 | 118 个本地 SVG，少于上游 176 个 SVG 全集。 |

### 8.4 GPU / CPU 分工

| 路径 | 当前事实 |
|---|---|
| GPU dab 光栅化 | 应用默认安装 `GpuDabEngine`（`app.rs:1911`、`shell31.rs:555-559`），除非设置 `PIGMENT_NO_GPU_DABS=1`。 |
| GPU 设备端 smudge | 仅覆盖同时满足：`smudge_mapping_active`、`paint_mode == 1.0`、`smudge_length_log == 0.0`、无 HSV/HSL 颜色动态的 dab（`stroke.rs:913-916`）。对称开启或设置 `PIGMENT_NO_DEFERRED_SMUDGE` 时也不走设备端链（`pigment-surface/src/lib.rs:467-475`）。 |
| GPU smudge 采样间隔 | 设备端链默认每 8 个 dab 取一次样（`PIGMENT_SMUDGE_SAMPLE_DABS`，`stroke.rs:1186-1197`）。 |
| CPU dab 求值 | 所有 dab 的位置/半径/颜色/曲线求值都在 CPU（`hokusai-core/src/stroke.rs`）；GPU 只接管像素落地与上述设备端 smudge 链。 |
| CPU 合成/其他 | 图层合成、mipmap、填充、strokemap、PNG、缩略图、显示滤镜均由 CPU/`pigment-surface` 完成；不是 GPU dab 路径的一部分。 |
| 诊断计数 | `PIGMENT_FRAME_PERF=1` 输出每帧 `dabs gpu/cpu`、flush 次数与 tile 数（`pigment_surface::perf`）。 |

---

## 9. 验证

### 9.1 测试门禁（本机实际执行）

| 命令 | 结果 |
|---|---|
| `cd pigment && cargo test --workspace --quiet` | 933 passed，0 failed（26 个 `test result: ok` 套件） |
| `cd hokusai && cargo test --workspace --quiet` | 98 passed，0 failed（20 个 `test result: ok` 套件） |

合计 1031 个测试通过。测试覆盖：引擎逐函数回归、笔画快照回归、CPU/GPU 采样对照、颜色向量、ORA/PNG/`.dat` I/O、图层合成、模式、偏好。

测试的边界：

- `hokusai-compat` 的 golden 当前由 hokusai 自身生成（`src/lib.rs` 的 “Path to true libmypaint parity” 明确说明），只检测回归，不是上游 C 输出；比较容差为每通道 `MAD <= 0.5`，9 个 fixture。
- `pigment-engine/tests/official_pack.rs` 对 196 支笔刷只断言“解析成功、落下 dab、像素有变化”，不比较 libmypaint 输出。
- 上述测试证明代码路径与已编码的断言一致，**不证明**第 8.2 节的缺失 UI 存在，也不证明 GPU 路径与 CPU 逐位一致。

### 9.2 libmypaint C 差分门禁（2026-10-21 快照）

门禁实现：`hokusai/crates/hokusai-compat/cref/`。

- `build_ref_msvc.bat`：用 MSVC/UCRT 编译上游 libmypaint 核心与 `ref_runner`，是当前基准（Rust 应用同用 UCRT，`expf/log/logf/powf/atan2f/hypotf/sin/cos` 与 Rust 逐位一致）。`build_ref.sh` 保留为 MinGW 备用构建；MinGW 的 msvcrt libm 与 UCRT 有约 1 ULP 差异，不用于判定。
- `ref_runner.c` + `ref_surface.c`：用 C 引擎渲染同一 `.myb` 与事件流，输出 RGBA u16 fix15。
- `src/bin/dump_fix15.rs`：Rust CPU 路径输出同样格式；`dump_fix15` 用 `stroke_to_full(..., linear=true)`。
- `gen_inputs.py`：从 `.myb` 生成设置/映射文本；`diff_gate.py`：批量运行并逐字节比较。

两侧共同约定：

- `new_with_buckets(256)` → `from_defaults` → 应用画笔设置 → `begin_atomic` → 首个事件位置 `dt=10` 预热 → 事件流 → `end_atomic`。
- MyPaint 包装层在进入引擎前做的 `transform_hsv(hsv, 2.2)` 由 C 参考端按 Python double 语义补齐。
- 事件 `dtime` 是 `double`：生成文本用 17 位有效数字，两侧都按 f64 传入（早期按 f32 传入会令 `partial_dabs` 在 1 ULP 处分叉）。
- 画笔文件里的每个 setting 按 MyPaint `BrushInfo` 同步语义处理：先清空该 setting 的全部 input mapping，再写入文件值；否则 libmypaint 内置的 `opaque_multiply` 压力曲线会残留。

当前结果（标准脚本：56 事件、压力斜坡、正弦轨迹、512×256）：

| 范围 | 逐位一致 | 有差异 | 错误 |
|---|---|---|---|
| `hokusai-compat` 9 个 fixture | 9 | 0 | 0 |
| 196 支官方 `.myb` | 194 | 2 | 0 |

复现核对（2026-10-21）：MinGW `gcc -O2` 与 MSVC `cl /O2` 两个参考端二进制给出**同一组数字**（194/2/0，且是同样两支笔刷、同样的最大差），所以结果不是单一编译器浮点行为的产物。9 个 fixture 那一行由 `cref/fixture_gate.py` 生成——它按每个 fixture 自己的事件流逐条跑门禁，任一非 IDENTICAL 即返回非零，可挂 CI。

**这组快照的事件是 4 字段（`x, y, pressure, dt`）**，`gen_inputs.py` 因此把 `xtilt/ytilt=0`、`viewzoom=1.0`、`viewrotation=0`、`barrel=0` 填成默认值。也就是说倾斜、画布缩放、画布旋转驱动的动态**没有被这条脚本覆盖**（代码路径已移植，但没有逐位证据）。补 10 字段事件扫描是当前门禁最高价值的扩展。

2 支差异（最大通道差，fix15 LSB）：

| 画笔 | 有差异像素 | 最大差 | 说明 |
|---|---|---|---|
| puantilism2 | 715 | 3 | 纯橡皮 dab（`alpha_eraser=0`），alpha 通道完全一致，仅 RGB 的 WGM 擦除结果差 1–3 LSB。 |
| Fan#1 | 6 | 10 | 使用 `surfacemap_x/y`；两侧都不支持该 input，差异集中在 6 个像素。 |

剩余差异来源：

- `puantilism2` 的所有逐事件状态一致，dab 的 alpha/opacity/几何一致；差异只在 `color_a=0` 的 spectral 擦除路径对画布自身颜色做 WGM 时的末位舍入。
- `surfacemap_x/y` 输入不在 libmypaint 的 18 个 input 中，门禁在生成输入时跳过并记录；hokusai 也把它保留为未知输入。使用该输入的 12 支官方笔刷没有参与这一维度的比较。

这组数字是**单一脚本快照**，不是 196 支笔刷的全事件空间证明；它把“与上游 C 的差异”从不可见变成可复现、可定位的门禁。

### 9.3 资源一致性

| 检查 | 结果 |
|---|---|
| 196 个 `.myb` 与上游画笔包逐字节比较 | 196/196 相同。 |
| 11 个菜单/工具栏/资源 XML 与上游比较 | 11/11 相同。 |
| `MyPaint_Default.gpl` | 相同。 |
| `mamurk_e_1.png` | 相同。 |

### 9.4 复现方式

```text
# 引擎/应用测试
cd pigment
cargo test --workspace --quiet

# hokusai 引擎
cd hokusai
cargo test --workspace --quiet

# 构建 C 参考端（MSVC/UCRT，基准；无需 json-c/glib）
cd hokusai\crates\hokusai-compat\cref
cmd //c build_ref_msvc.bat
cd hokusai
cargo build --release -p hokusai-compat --bin dump_fix15

# 196 支差分门禁（事件流已入库：cref/scripts/brushes196.json）
cd (workspace root)
python hokusai/crates/hokusai-compat/cref/diff_gate.py \
  --brushes mypaint-brushes/brushes \
  --script hokusai/crates/hokusai-compat/cref/scripts/brushes196.json \
  --work pigment/target/cref/gate196 \
  --ref pigment/target/cref/ref_runner_msvc.exe \
  --rust hokusai/target/release/dump_fix15.exe \
  --tolerance 0 --markdown pigment/target/cref/gate196.md

# 9 个 fixture 逐条门禁（每个用自己的事件流；非全同即非零退出）
cd hokusai
python crates/hokusai-compat/cref/fixture_gate.py \
  --fixtures crates/hokusai-compat/fixtures \
  --work ../pigment/target/cref/fixture_gate \
  --ref ../pigment/target/cref/ref_runner_msvc.exe \
  --rust target/release/dump_fix15.exe

# 资源比较示例
cmp _mypaint_ref\gui\menu.xml pigment\crates\pigment-app\assets\gui\menu.xml
```

### 9.5 逐项对齐验收（C 门禁口径）

验收级别：

- **逐位一致**：在标准脚本下最终 fix15 画布与上游 C 逐字节相同。
- **部分覆盖**：模块参与门禁，但只覆盖 CPU/部分输入/部分路径。
- **未覆盖**：本地有实现，但门禁没有执行到该路径。
- **不适用**：属于 GUI、资源或后端替代，不参与引擎差分。

| 功能/模块 | 本地落点 | 验收 | 依据与边界 |
|---|---|---|---|
| dab 主循环、插值、`partial_dabs`、stroke 计时 | `hokusai-core/src/stroke.rs` | 逐位一致 | 194/196 画布；`coarse_bulk`、`fur`、`texture-06`、`long_grass`、`puantilism2` 的逐事件 44 项状态抽检一致。 |
| setting 求值、多输入 mapping、默认值 | `evaluator.rs`、`mapping.rs`、`input.rs` | 逐位一致 | mapping 按 libmypaint input 枚举序求和；角度求值轨迹 1126/1126 步一致。 |
| 速度/方向/慢跟踪/自定义输入滤波 | `stroke.rs` | 逐位一致 | 逐步 `count_dabs_to`、方向滤波、压力插值轨迹逐位一致。 |
| 椭圆半径/角度、dab 计数 | `stroke.rs` | 逐位一致 | 椭圆 `cos/sin` 采用 C 的 double→float 窄化；`texture-06` 修正后逐位一致。 |
| smudge bucket、`apply_smudge`、eraser | `stroke.rs`、`spectral.rs` | 部分覆盖 | 光谱/legacy 混合与门禁笔刷一致；`puantilism2` 纯擦除路径 RGB 差 1–3 LSB（alpha 完全一致）。 |
| 光谱 WGM、`fastpow`、`spectral_to_rgb` | `spectral.rs` | 逐位一致 | 常量、SIMD 末级、double 除法均已对齐；除 `puantilism2` 擦除 RGB 末位外无差异。 |
| 光栅化：Normal/Eraser/Paint/LockAlpha/Color/Posterize | `brushmodes.rs` | 逐位一致（CPU） | 64×64 tile、mask、AA、fix15 混合；GPU 路径不在本项。 |
| `get_color` / `get_color_pigment` 采样 | `brushmodes.rs` | 部分覆盖 | 采样坐标、半径、RNG 与门禁一致；`puantilism2` 擦除采样的 1–3 LSB 残差。 |
| RNG（Knuth lagged Fibonacci、`rand_gauss`、libc `rand` 镜像） | `rng.rs`、`spectral.rs` | 逐位一致 | 逐事件 `RngSeed` 状态与抖动用例一致。 |
| HSV/HSL 转换、去线性化/线性化 | `color.rs`、`stroke.rs` | 逐位一致 | 按 `helpers.c` 的 double 语义与 `(float)(1/2.2)` 指数；`long_grass` 修正后一致。 |
| 画笔 JSON v3 解析/序列化 | `hokusai-brush/src/lib.rs` | 部分覆盖 | 门禁用同一 `.myb` 生成两侧输入；C 端通过公共 API 设置，不经过 json-c 解析。v2 旧格式缺失。 |
| `surfacemap_x/y` 输入 | 无 | 未覆盖（两侧均缺） | libmypaint 18 个 input 中没有该名；12 支官方笔刷该维度未比较。 |
| 对称、矩阵、矩形裁剪 | `pigment-surface/src/symmetry.rs` 等 | 未覆盖 | 门禁未启用对称；本地实现有独立测试。 |
| 固定 tile surface 后端 | `hokusai-tile-mem` | 未覆盖 | 门禁用的是 `MemSurface`，不是固定尺寸后端。 |
| mipmap、strokemap、ORA/PNG | `pigment-surface`、`pigment-engine` | 未覆盖 | 门禁只比较 level 0；这些由各自测试覆盖。 |
| GPU dab / GPU 采样 | `pigment-render` | 未覆盖 | 本报告没有 GPU vs CPU 的量化门禁；第 8.4 节只记录分工。 |
| GUI、颜色选择器、模式面板、图层视图 | `pigment-app` | 不适用 | 属于第 8.2 节列出的 UI/data 缺口。 |
| 资源文件 | `pigment-app/assets`、`pigment-engine/assets` | 逐位一致 | 196 个 `.myb`、11 个 UI XML、默认调色板、默认背景图逐字节相同（第 9.3 节）。 |

门禁未覆盖项汇总：

- GPU 与 CPU 的逐位/容差对照（需单独跑 `gpu_dab_check` / `gpu_sample_check` 并量化 dab 数）。
- 对称、mipmap、固定 tile 后端、`.dat` strokemap、ORA/PNG I/O（有独立测试，但不在 C 差分门禁内）。
- `surfacemap_x/y` 及 v2 `.myb`。
- 196 支笔刷仅一个 56 事件脚本的快照，不是全事件空间证明。

逐步调试入口：`build_ref_msvc_trace.bat` 生成带环境变量的 C 追踪二进制（`CREF_TRACE_COUNT` / `CREF_TRACE_DABARGS` / `CREF_TRACE_STATE` / `CREF_TRACE_INPUTBITS` / `CREF_TRACE_DIR` / `CREF_TRACE_STEPDX` 等），Rust 侧对应 `HOKUSAI_TRACE_*`。

---

## 10. 覆盖统计

统计口径：第 4、6 节的“文件状态总表”按文件计数；第 3、5 节按核心文件组计数。`对应` 不等于逐位等价（见第 0 节）。

| 范围 | 文件/组数 | 对应 | 部分 | 缺失 | 结构性替代 | 不适用 |
|---|---|---|---|---|---|---|
| libmypaint 文件组 | 20 | 12 | 0 | 0 | 5 | 3 |
| MyPaint `lib/` Python | 48 | 24 | 7 | 1 | 11 | 5 |
| MyPaint `lib/` 原生 C/C++ | 21 | 9 | 2 | 4 | 6 | 0 |
| MyPaint `gui/` Python | 90 | 54 | 17 | 5 | 12 | 2 |

来源文件总数：libmypaint 20 个文件/头文件组（含 fastapprox/gegl/glib/tests 分组）；MyPaint `lib/` 48 个 Python 文件 + 21 个 C/C++ 文件组；`gui/` 90 个 Python 文件。

### 10.1 已确认的缺失内容（按文件）

| 文件 | 缺失内容 |
|---|---|
| `lib/alg.py` | `poly_centroid`、`nearest_point_on_segment`、`nearest_point_in_poly`、`nearest_point_on_line`、`_nearest_point`、`_intersects`、`intersection_of_segments` |
| `lib/brush.py` | `.myb` v2 旧格式解析（`_oldfmt_*`、`brushinfo_unquote`） |
| `lib/feedback.py` | `Progress` 及所有进度回调 |
| `gui/backgroundwindow.py` | 背景列表、选择、平铺设置 UI |
| `gui/brushiconeditor.py` | 图标绘制/预览/应用 UI |
| `gui/colors/changers.py` | `CrossedBowl`、`Wash`、`Rings` 颜色变换器 |
| `gui/historypopup.py` | `HistoryPopup` |
| `gui/layervis.py` | `LayerViewUI` |
| `gui/optionspanel.py` | `ModeOptionsTool` 模式选项面板 |
| `gui/quickchoice.py` | `QuickBrushChooser`、`QuickColorChooser`、两个弹层 |
| `lib/colorchanger_crossed_bowl.hpp` | `CrossedBowl` 颜色模型 |
| `lib/colorchanger_wash.hpp` | `Wash` 颜色模型 |
| `lib/colorring.hpp` | `Rings` 颜色模型 |
| `lib/gdkpixbuf2numpy.cpp` | GdkPixbuf ↔ NumPy 转换 |

### 10.2 已确认的部分实现（差异点）

| 文件 | 部分实现事实 |
|---|---|
| `lib/brush.py` | 无 `begin_atomic`/`add_pending_update`/`end_atomic` 的待提交更新批次；设置直接改 `Brush`。 |
| `lib/fileutils.py` | 无通用 `safename`/`via_tempfile` 封装。 |
| `lib/helpers.py` | 无 `Rect` 类型；GdkPixbuf/缩略图缓存/`transform_hsv` 等未实现。 |
| `lib/layervis.py` | 模型与命令有；无 UI。 |
| `lib/pixbuf.py` / `lib/pixbufsurface.py` | PNG 与像素导出有；无 GdkPixbuf 表面。 |
| `gui/application.py` | 单文档 + 暂存区；无多窗口/多实例 GTK Application。 |
| `gui/buttonmap.py` | 解析/分派有；编辑器无。 |
| `gui/dialogs.py` | 确认/重命名/文件弹层有；无通用错误框、画笔/分组覆盖确认、模态取色器。 |
| `gui/colors/adjbases.py` | `ColorManager` 与颜色数学有；调整器页面容器无。 |
| `gui/colors/hcywheel.py` | 数学有；轮盘/遮罩 UI 无。 |
| `gui/colors/hsvcube.py` / `hsvsquare.py` / `hsvwheel.py` | 数学有；页面控件无。 |
| `gui/colors/paletteview.py` | 模型/加载保存有；完整编辑对话框无。 |
| `gui/colors/sliders.py` | 无组件滑块 UI。 |
| `gui/colortools.py` | 动作存在；所有工具共用单页颜色面板。 |
| `gui/drawutils.py` | 样条/预览/图标有；浮动按钮/阴影为 egui 重绘。 |
| `gui/externalapp.py` | 外部编辑有；`OpenWithDialog` 无。 |
| `gui/layerprops.py` | 面板内联属性有；独立对话框无。 |
| `gui/quickchoice.py` | 动作 id 有；行为无。 |
| `gui/windowing.py` | 子窗口元数据有；主窗口内浮层承载，无独立 OS 子窗口。 |

### 10.3 目标状态

以 100% 移植为目标，当前台账的缺口是：3 个颜色变换器、6 类颜色调整器页面（HCY 轮、HSV 轮、HSV 方块、HSV 立方、组件滑块、调色板编辑器）、模式选项面板、图层视图 UI、背景选择窗口、画笔图标编辑器、快速选择弹层、撤销历史弹层、进度反馈、按钮映射编辑器、图层属性对话框，以及 `.myb` v2 与 GdkPixbuf/NumPy 桥接。引擎核心的公开 API 与像素路径有对应实现；其等价性由第 9 节的测试门禁约束。

### 10.4 未核实项

以下内容在本报告中**没有**被证明为缺失或不等价，只是没有逐项核实：

- `lib/floodfill.py` 的 `update_bbox`、`unseep`、`complement_gc_seeds`、`gc_seeds_skippable`：按符号名无结果，但可能已内联进 `fill.rs` / `fill_gap.rs` 的阶段逻辑。
- `lib/morphology.py` 的 `adjacent_tiles`、`complement_adjacent`、`directly_below`、`strand_partition`、`adj_full`：同上，可能内联。
- `lib/helpers.py` 中标注为“部分”的 Rust 标准库替代项（`utf8`、`casefold`、`grouper`、`fmt_time_period_abbr`）。
- GPU dab/采样与 CPU 的逐位误差：本报告只记录“存在浮点实现差异”，没有重新运行 `gpu_dab_check` / `gpu_sample_check` 量化。
- 所有 `对应` 行的逐位等价性：本报告只验证职责落点；逐位等价需依赖第 9 节测试或额外的逐像素对照。

---

*文件结束。*


