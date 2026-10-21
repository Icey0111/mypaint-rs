# 绘画引擎移植审计

审计日期：2026-10-19  
审计范围：笔刷设置/输入 → 笔画求值 → dab 栅格化 → tile surface → 图层合成  
结论：**原始审计不是 100% 移植。**  
> 2026-10-20 更新：P0/P1 的大部分缺口已落地（见第 6 节），第 2 节保留原始缺口清单作为审计基线。
> 2026-10-21 更新：第 7 节“196 支对 libmypaint C 的复测本机不可执行”**已被推翻**。`hokusai` fork 里的 `crates/hokusai-compat/cref/` 用最小 `config.h`/`json.h` 桩编译上游 C 核心，不需要 json-c/GLib/pkg-config；当前门禁结果见 `docs/UPSTREAM_ALIGNMENT.md` §9.2 与 README 的 Parity evidence 一节。本文件的历史缺口清单仍按原样保留。

## 0. 审计基线（逐提交核对，不凭印象）

| 项目 | 本地路径 | 远端 | 提交 |
|---|---|---|---|
| hokusai | `hokusai` | `github.com/reearth/hokusai` | `f7e9981`（v0.3.0，当前 `main`/最新 tag） |
| libmypaint | `_libmypaint_ref` | `github.com/mypaint/libmypaint` | `d5a88fb`（当前 `master`） |
| MyPaint | `_mypaint_ref` | `github.com/mypaint/mypaint` | `fb6f7d6`（当前 `master`，2.0.0a0） |

hokusai v0.3.0 的 README 自己写的是 **188 / 196 支 stock brush 在 MAD ≤ 0.5 内**，8 支 amber（≤ 5），0 支 red。这不是 100%；而且这组数字是 hokusai 的 C wrapper 对 **libmypaint v1.6.1** 测的，不等于 MyPaint 2.0 的端到端行为。

## 1. 量化结果

| 维度 | 参考实现 | 当前实现 | 结论 |
|---|---|---|---|
| 笔刷设置键 | `brushsettings.json`：65 | `BrushSetting`：63 | **缺 2**：`smudge_bucket`（1.6.1 就有）、`softness`（2.0 新增） |
| 输入键 | 18 | 18 + 额外 `tilt` | 键都在，但 `viewzoom` / `barrel_rotation` **恒为 0**；`viewrotation` 完全没有状态 |
| `stroke_to` 参数 | 10 个（x,y,pressure,xtilt,ytilt,dtime,viewzoom,viewrotation,barrel,linear） | 6 个 | **缺 4**：viewzoom / viewrotation / barrel / linear |
| `draw_dab` 参数 | 17 个（含 `softness`） | 16 个 | **缺 softness** |
| stock brush 一致性 | 196 | 188（hokusai 自报） | **8 支 amber**，不是 100% |
| 图层混合模式 | `pixops.hpp`：22 | `combine.rs`：22 | 枚举齐全；逐公式未全部独立复核 |
| 测试 | — | hokusai 40 + pigment 183 + app 217 = 440 通过 | 测试通过**不代表**上述缺口被覆盖 |

## 2. 硬缺口（按影响排序）

### G1. 多 smudge bucket 完全没移植（影响 19 支 stock brush）

libmypaint 每支笔有 256 个 bucket：

- `_libmypaint_ref/mypaint-brush.c:906-918` `fetch_smudge_bucket()`；
- `_libmypaint_ref/mypaint-brush.c:197-207` `mypaint_brush_new_with_buckets()`；
- `_mypaint_ref/lib/brush.hpp:26-27` MyPaint 固定 `num_buckets = 256`。

hokusai 只有一组 `smudge_ra/ga/ba/a` 和一组 `prev_col_*`：

- `hokusai/crates/hokusai-core/src/state.rs:126-142`；
- `hokusai/crates/hokusai-core/src/setting.rs:46-111` 没有 `SmudgeBucket`；
- `hokusai/crates/hokusai-core/src/stroke.rs:746-858` 只更新这一组。

`assets/brushes` 的 196 支里有 **19 支**写了 `smudge_bucket`，而且是用 `custom` 曲线在 0..255 间扫（例如 `Dieterle/Round#1.myb:812-825`）。当前实现会把 256 个 bucket 压成 1 个，颜色变化/脏色行为必然不同。

### G2. `viewzoom` 输入恒为 0（影响 13 支 stock brush）

hokusai 直接写死：

- `hokusai/crates/hokusai-core/src/stroke.rs:235-239`：
  `dab_inputs.set(BrushInput::Viewzoom, 0.0);`

libmypaint：

- `_libmypaint_ref/mypaint-brush.c:801`：
  `INPUT(VIEWZOOM) = BASEVAL(RADIUS_LOGARITHMIC) - logf(base_radius / STATE(VIEWZOOM));`

app 明明把缩放传进来了（`pigment/crates/pigment-app/src/app.rs:732` `viewzoom: view.zoom`），但 `StrokeEvent::apply()` 没有把它交给引擎（`pigment/crates/pigment-engine/src/stroke.rs:126-135`）。

后果：在 6.25%（`ln(0.0625) = -2.7726`）时，libmypaint 的 viewzoom 输入是 **-2.7726**，hokusai 是 **0**。13 支用 `viewzoom` 曲线的笔刷（`modelling`、`Blender`、`Dissolver`、`Eraser`、`Fan#1`、`Flat2#1`、`Flight_Feathers`、`Posterizer`、`Splash`、`Tail_Feathers`、`Tail_Feathers2`、`WateryFlatbrush` 等）在非 100% 缩放下半径/动态会偏。

### G3. `viewrotation` / `barrel_rotation` 记录后被丢弃

MyPaint 的画笔引擎会按视图旋转修正 direction / tilt_ascension / ellipse angle / directional offsets：

- `_libmypaint_ref/mypaint-brush.c:728-729`、`:794-803`、`:903`；
- `_libmypaint_ref/mypaint-brush.c:614-657` 的 `directional_offsets()`。

hokusai：

- `hokusai/crates/hokusai-core/src/stroke.rs:1307`、`:1337` 直接 `viewrotation = 0.0`；
- `barrel_rotation` 只在 `:239` 写 0。

而 app 已经支持画布旋转（`pigment/crates/pigment-app/src/shell/canvas.rs:734-737` 的 `viewrotation: view.rotation`），`StrokeEvent` 也存了三个字段，但 `apply()` 在 `pigment/crates/pigment-engine/src/stroke.rs:126-135` 只传 6 个参数。`stroke.rs:30-33` 自己注释了“重放丢弃 3 个输入……这是引擎 API 的缺口”。

196 支里 `direction` 42 支、`direction_angle` 11 支、`attack_angle` 18 支；画布一转，这些曲线就会按“未旋转”求值。

### G4. 线性 sRGB 颜色链没移植（影响所有颜色 + 43 支色彩动态 brush）

MyPaint 的完整链是：

1. `_mypaint_ref/lib/brush.py:651`：`linear = eotf() != 1.0`（默认 `DEFAULT_EOTF = 2.2`，所以是 true）；
2. `_mypaint_ref/lib/brush.py:658-682`：`_transform_brush_color()` 先把 `color_h/s/v` 做 sRGB→linear；
3. `_libmypaint_ref/mypaint-brush.c:1153-1190`：HSV/HSL 动态前 `powf(..., 1/2.2)` 去线性，动态后 `powf(..., 2.2)` 回线性。

hokusai：

- `Brush::stroke_to` 没有 `linear` 参数（`stroke.rs:34-44`）；
- `build_dab()` 直接对线性值做 HSV/HSL：`stroke.rs:1060-1079`；
- app 的 `color_hsv()` 来自 `UIColor`，没有 `_transform_brush_color` 这一步（`pigment/crates/pigment-app/src/app.rs:3522-3524`；`color.rs` 只做 f32/f64 边界转换）。

具体例子：UI 里 50% 灰（S=0, V=0.5）。MyPaint 先 `0.5^2.2 = 0.2176` 再当线性值，显示回来是 0.5；hokusai 直接把 0.5 当线性值，显示约为 `1.055*0.5^(1/2.4)-0.055 = 0.735`，也就是 **73.5% 灰**。43 支至少有一个 `change_color_*` 非零 base 或曲线的 brush 还会再叠加动态误差。

### G5. `softness` 没有进入 Dab / 栅格化（2.0 设置）

libmypaint：

- `_libmypaint_ref/mypaint-tiled-surface.c:636` 存 `softness`；
- `:644` `softness == 1.0` 直接不画；
- `:700` 传给 `draw_dab`；
- `:408-411` mask 的两段斜率和偏移都乘 `(1 - softness)`。

hokusai：

- `hokusai/crates/hokusai-core/src/surface.rs:12-28` 的 `Dab` 没有 `softness`；
- `hokusai/crates/hokusai-core/src/brushmodes.rs:149-157` 的早退只检查 radius/hardness/opaque；
- `pigment/crates/pigment-surface/src/symmetry.rs:38-39` 自己注释了“softness == 1.0 那条查不了”。

后果：`softness=1.0` 在 libmypaint 是空操作，在 hokusai 会正常画出；`softness` 中间值会改变边缘衰减。stock pack 目前 0 支用 `softness`，但它是 MyPaint 2.0 的合法设置，不能算 100%。

### G6. 笔画切段（stroke splitting）没有移植

libmypaint 在 `mypaint-brush.c:1504-1546` 根据 painting/idling 时间返回“该切段了”。MyPaint 的 `mode.py:841-845` 看到 `cmd.split_due` 就提交上一段、开新段。

hokusai：

- `Brush::stroke_to` 只返回“有没有画到 dab”，没有 split 信号；
- `pigment/crates/pigment-engine/src/command.rs:463-464` 直接 `self.split_due = false;`。

后果：长笔画永远是一个 undo 单元，autosave/undo 的内存与原子提交粒度都和 MyPaint 不同。

### G7. 数位板 `dtime` 预热补丁没移植

libmypaint：

- `_libmypaint_ref/mypaint-brush.c:1346-1351`：`dtime > 0.100 && pressure && STATE(PRESSURE) == 0` 时，先递归发一个零压力事件，避免从上一个笔画的压力线性插值过来。

hokusai 的 `stroke_to`（`stroke.rs:34-199`）没有这个分支。后果：暂停后落笔的第一段压力可能被插值，起笔处出现本不该有的深色/浅色过渡。

### G8. app 每笔把 `BrushState` 重置成 `default()`，RNG 被重新播种（影响 62 支 random brush）

- `hokusai/crates/hokusai-core/src/state.rs:246-251`：`Default` 调 `new(1000)`，RNG 种子固定 1000；
- `state.rs:237-243`：`reset()` 会保留 RNG；
- `pigment/crates/pigment-app/src/shell/canvas.rs:673` 和 `:786`：每笔 `self.stroke_state = BrushState::default()`；
- MyPaint 只在设备切换时 `brush.reset()`（`_mypaint_ref/gui/freehand.py:394`），`brush_reset` 不动 RNG（`mypaint-brush.c:145-167`）。

后果：62 支用 `random` 输入的笔刷，每一笔的随机序列从同一个 1000 开始，scatter/纹理模式会重复。这不是“每笔新状态”的等价物。

### G9. `paint_mode` 默认值只实现了 MyPaint 的 “off” 分支

- `hokusai/crates/hokusai-brush/src/lib.rs:61-67` 把 `Paint` 默认设为 **0.0**；
- `_libmypaint_ref/brushsettings.json` 的 `paint_mode` 默认是 **1.0**，最小值 0.0；
- MyPaint 不是无条件用 0.0 或 1.0，而是由偏好 `pigment_by_default` 决定：
  - `_mypaint_ref/gui/brushmanager.py:609-614`：true → `setting_info.default`（1.0），false → `setting_info.min`（0.0）；
  - `_mypaint_ref/gui/brushmanager.py:1214-1215` 用这个 override 构造每个 `BrushInfo`。
- hokusai 固定 0.0，等价于 `pigment_by_default = false`，没有任何开关能表达 true 分支。

196 支里 177 支没有显式写 `paint_mode`，所以这些笔刷的默认颜料模式在 hokusai 里**永远关闭**。这不是“所有刷子都错”，而是“MyPaint 的这个偏好维度没移植”；如果目标 UI 也没有这个偏好，可以算一个已记录差异，但不能算 100%。

### G10. `DABS_PER_*` 状态晚一个求值步

libmypaint 在 `update_states_and_setting_values()` 里**先算当前 SETTING，再写 STATE**：

- `_libmypaint_ref/mypaint-brush.c:818-824`。

hokusai 在每次循环**开头**从 `prev_settings` 写 STATE：

- `hokusai/crates/hokusai-core/src/stroke.rs:405-408`；
- 当前 `dab_sv` 算完后只更新 `state.prev_settings`（`:670`），没有再写 `state.dabs_per_*`。

后果：`count_dabs_to()` 和 `opaque_linearize` 读到的 `dabs_per_*` 比 libmypaint 晚一步；当 dabs-per-radius 受压力/速度曲线驱动时，dab 间距会在阈值附近偏移。

另外 `count_dabs_to()` 在 state 为 0 时回退到 base value（`:1277-1294`），而 C 是直接用 STATE（`mypaint-brush.c:1279-1281`），这也是行为差异。

### G11. `tilt_declinationx/y` 的无倾角默认值错误

libmypaint 无倾角时把目标设为 **90**：

- `_libmypaint_ref/mypaint-brush.c:1306-1320`：`tilt_declinationx = 90.0; tilt_declinationy = 90.0;`，只有 `xtilt/ytilt` 非零时才改成 `xtilt*60` / `ytilt*60`。

hokusai：

- `stroke.rs:220-225` 无倾角时只把 `tilt_declination` 设为 90；
- `stroke.rs:436-437` 的 `step_decl_x/y` 目标始终是 `xtilt*60` / `ytilt*60`，无倾角就是 0。

stock pack 里 0 支用 `tilt_declinationx/y`，但第三方笔刷会读到 0 而不是 90。

### G12. 其他精确差异（低风险，但都不是 100%）

| 项 | 参考 | hokusai |
|---|---|---|
| `opaque_multiply` 恒等特判 | 显式 0 就应是 0 | `stroke.rs:973-979` 把 base=0/no-inputs 当 1.0 |
| `get_color` 的 softness | `mypaint-tiled-surface.c:813` 固定 0.5 | `brushmodes.rs:865-908` 实际按 0 算，mask 量化会有舍入差 |
| 稀疏 `get_color` 的 `rand()` | Windows/libc 的 `rand()` | `brushmodes.rs:821-856` 用 BSD Park–Miller；README 自己承认只对 macOS/BSD 逐位一致 |
| `pressure` 上限 | C 只夹 `<=0`，不夹 1 | `stroke.rs:45` 夹到 `[0,1]` |
| `dtime` 重置边界 | `dtime > 5.0` | `stroke.rs:91` 用 `>= 5.0` |
| 状态/映射内省 API | `is_constant`、`get_inputs_used_n`、`get/set_state`、smudge bucket state、`reset_requested` | 都没有对应公开 API |
| `restore_color` | `gui/brushmodifier.py:101-106` 选笔刷时混色 | 解析了，但 app 没有接行为 |

## 3. 已经对齐/可以确认的部分

- 64×64 tile、fix15、tile 坐标布局与 libmypaint 一致（`hokusai-core/src/tile.rs`）。
- 曲线求值 `base + Σ mapping` 与 `mypaint-mapping.c` 一致（`hokusai-core/src/mapping.rs:27-61`）。
- RNG 算法（KK=10/LL=7/TT=7，seed 1000）与 `rng-double.c` 一致（`rng.rs`）。
- `draw_dab` 的 Normal / Eraser / LockAlpha / Colorize / Posterize / Normal_Paint / Normal_and_Eraser_Paint 各 pass 的结构与 `brushmodes.c` / `process_op` 对齐。
- 22 个 layer combine mode 的 ID 与 `_mypaint_ref/lib/pixops.hpp:93-116` 的 `NumCombineModes` 一致。
- 对称绘制在 `pigment-surface/src/symmetry.rs` 单独补齐，端到端可用。
- 快照/写时复制 undo、ORA/PNG I/O、fill/gap/morphology 代码路径存在且有测试。

## 4. 测试与复现

本轮实际运行：

```text
cargo test -p hokusai-core -p hokusai-brush -p hokusai-tile-mem   # 45 passed（core 33 + brush 4 + tile 8）
cargo test -p pigment-engine -p pigment-surface                  # 183 passed (含 surface 100)
cargo test -p pigment-app                                        # 217 passed
```

原始测试没有覆盖 G1–G11 的行为差异；本轮已补 6 个针对性回归（见第 6 节）。  
`hokusai` README 的 188/196 无法在本机复跑：`tools/libmypaint-render` 需要 `libmypaint` + `json-c` + `pkg-config`，本机没有这些依赖，仓库里也没有 `tmp/brush-pack-report.md`。因此我不猜 8 支 amber 的具体名单；README 只给了最差 `texture-06` 3.8。

## 5. 建议修复顺序

**P0（用户可见，先修）**

1. app 每笔不要 `BrushState::default()`：保留持久状态，或至少用 `reset()` 保留 RNG（G8）。
2. 给 `Brush::stroke_to` 补 viewzoom / viewrotation / barrel 参数，app 的 `StrokeEvent::apply` 把三个字段真正传下去（G2/G3）。
3. 补 sRGB→linear 颜色变换 + HSV/HSL 动态的 delinearize/linearize（G4）。
4. 补 256 smudge bucket + `smudge_bucket` 设置（G1）。
5. 补 `softness` 到 setting / `Dab` / `draw_dab` / symmetry 判据（G5）。

**P1（行为/内存）**

6. 补 stroke split 返回值和 app 的 `split_due`（G6）。
7. 补 `dtime > 0.1` 的数位板预热分支（G7）。
8. 修 `DABS_PER_*` 的写入顺序和 0 回退（G10）。
9. 补 `pigment_by_default` 偏好与 `paint_mode` 的 default/min override（G9）。

**P2（边角/API）**

10. `tilt_declinationx/y` 默认 90（G11）。
11. `opaque_multiply` 的显式 0、`get_color` softness=0.5、稀疏采样 RNG、pressure/dtime 边界（G12）。
12. 补 libmypaint 的状态/映射内省 API（如果要做完整重放/调试工具）。

如果“100%”指 MyPaint 2.0 的端到端行为，P0 的 1–4 项不修，就不能称为全面移植；如果只指 hokusai 当前声明的 v1.6.1 笔刷核心，G1/G2/G3/G4/G6/G7 仍然是 1.6.1 就存在的缺口。

## 6. 2026-10-20 修复状态

已落地（代码已改并跑过测试）：

| 项 | 状态 | 落点 |
|---|---|---|
| G1 256 smudge bucket + `smudge_bucket` | 已修 | `SmudgeBucket` 数组、`BrushSetting::SmudgeBucket` |
| G2 viewzoom | 已修 | `stroke_to_full` + `INPUT(VIEWZOOM)` + `norm_dx/dy` |
| G3 viewrotation / barrel | 已修 | direction/tilt/ellipse/offset/barrel state |
| G4 linear-sRGB 颜色 | 已修 | base sRGB→linear + HSV/HSL 前后 delinearize/linearize |
| G5 softness | 已修 | `Dab.softness`、mask 缩放、`softness=1` 空操作 |
| G6 stroke split | 已修 | `BrushState::split_due` + `Brushwork`/`ModeStack` 接线 |
| G7 数位板 dtime 预热 | 已修 | `dtime > 0.1 && pressure>0 && state.pressure==0` |
| G8 RNG 每笔重播种 | 已修 | 每笔不再 reset；设备切换 `BrushState::reset()` 保留 RNG |
| G10 DABS_PER 写入顺序 | 已修 | 评估后写 `state.dabs_per_*`；第二轮删除了 0 回退 |
| G11 tilt_declinationx/y 默认 90 | 已修 | 无倾角目标 90/90 |
| G12 `get_color` softness=0.5 | 已修 | `for_each_mask_pixel` / `get_color_via_sample` |

仍缺（本轮未改）：

- **无**（见第 7 节；本轮已把遗留清单项全部落地）。

测试更新后：

```text
hokusai-core 45 + hokusai-brush 5 + hokusai-tile-mem 9 + compat 17 = 76
pigment-engine 553（330 lib + 223 integration）
pigment-surface 113（100 lib + 13 integration）
pigment-app 217
```

新增回归：`viewzoom_changes_input_and_radius`、`linear_colour_converts_base_srgb_to_linear`、`viewrotation_rotates_elliptical_dab_angle`、`long_stroke_requests_split`、`softness_one_is_a_no_op`、`softness_reduces_center_opacity`、`opaque_multiply_explicit_zero_zeroes_the_dab`、`pressure_above_one_is_not_clamped_by_the_engine`、`dtime_exactly_five_seconds_is_not_a_reset`、`smudge_bucket_watermarks_track_selected_bucket`、`state_enum_matches_libmypaint_count_and_order`、`every_state_field_roundtrips`、`new_stroke_only_resets_timers`、`reset_clears_buckets_but_preserves_rng`、`mapping_api_roundtrip_and_guards`、`explicit_opaque_multiply_zero_roundtrips`、`c_rand_matches_the_platform_crt`。

## 7. 2026-10-20 第二轮：清单清零

上一轮遗留的清单项（含两项上轮已实现、本轮补核对/补历史路径的），逐项落点如下。所有路径均相对 `(workspace root)`。

| 遗留项 | 处理 | 落点 / 证据 |
|---|---|---|
| `opaque_multiply` 显式 0 被当成 1 | 删除 `base=0 && inputs.is_empty() → 1.0` 特判，直接读 `SETTING(OPAQUE_MULTIPLY)`；默认恒等只放在 `from_defaults` / `.myb` 解析的 `[(0,0),(1,1)]` 压力曲线里 | `hokusai-core/src/stroke.rs` `opaque_multiplier`；`hokusai-brush/src/lib.rs` `apply_libmypaint_defaults` |
| `count_dabs_to` 在 state=0 时回退 base | 改为直接用 `STATE(DABS_PER_*)`，末尾只保留 C 的 `isnan || <0 → 0`；同时删除 `dabs_per_second` 的 NaN 回退 | `hokusai-core/src/stroke.rs` `count_dabs_to` |
| 稀疏 `get_color` 的 libc `rand()` 在 Windows 不一致 | MSVC 下改为 `holdrand = holdrand*214013+2531011`、`(holdrand>>16)&0x7fff`、`RAND_MAX=32767`；非 MSVC 保留 Park–Miller | `hokusai-core/src/brushmodes.rs` `c_rand` / `C_RAND_MAX`；回归 `c_rand_matches_the_platform_crt` |
| `pressure` 上限 | 只做 `pressure <= 0 → 0`，不再夹到 1；GUI 自己在 `freehand.py:598` 夹 | `hokusai-core/src/stroke.rs` `stroke_to_full`；回归 `pressure_above_one_is_not_clamped_by_the_engine` |
| `dtime == 5.0` 边界 | `>= 5.0` 改为严格 `> 5.0`，与 `mypaint-brush.c:1398` 一致 | 同上；回归 `dtime_exactly_five_seconds_is_not_a_reset` |
| 状态/映射内省 API | 新增 `Brush::{get,set}_base_value`、`is_constant`、`get_inputs_used_n`、`get/set_mapping_n`、`get/set_mapping_point`；新增 44 项 `BrushStateField` 与 `BrushState::{get,set}_state`、`new_stroke`、smudge bucket get/set 与 min/max used | `hokusai-core/src/brush.rs`、`state.rs`；回归 4 个 |
| 每笔 `reset()` 清空 smudge bucket | app 起笔/单击不再 `BrushState::reset()`；状态跨笔画保留，只在设备切换时 reset（`freehand.py:394`）。`BrushState::new_stroke()` 只复位两个计时器 | `pigment-app/src/shell/canvas.rs` 起笔分支；`hokusai-core/src/state.rs` `new_stroke` |
| `restore_color` 选笔刷行为 | 主库选笔刷已有；补上笔刷历史路径：`select_brush_history` 现在与 `brush_selected_cb` 一样按旧/新 `restore_color` 混色，并回填 `last_selected_color` | `pigment-app/src/app.rs` `color_after_brush_selection` / `select_brush_history` |
| G9 `pigment_by_default` | 第一轮已接通 `CompatSettings.pigment_on_by_default` → `brushes.pigment_by_default` → `load_with_default()`；第二轮补 runtime：模式切换只重置未显式写 `paint_mode` 的当前笔刷；偏好页新增兼容性控件（默认模式 + 1.x/2.x 的颜料/图层开关） | `pigment-app/src/app.rs` `apply_compat_mode` / `reset_undefined_pigment`；`pigment-app/src/shell/dialogs.rs` 偏好页；`pigment-engine/src/brushlib.rs` `has_explicit_paint_mode` / `parse_brush_json_with_default` |
| 序列化丢显式 0 | `to_string_pretty` 从“跳过全零设置”改为写出全部 65 个设置，与 MyPaint `BrushInfo.to_json()` 一致；显式 `opaque_multiply=0` 不再在存盘/重载后变回压力曲线 | `hokusai-brush/src/lib.rs`；回归 `explicit_opaque_multiply_zero_roundtrips` |

### 仍然不能在本机量化的部分

代码清单已清空。唯一没有本地数字的是 hokusai README 自报的 **188/196** stock-brush C 对比：本机没有 `libmypaint` 的 C 构建依赖（`pkg-config`、`json-c`、GLib），也没有 `tmp/brush-pack-report.md`，所以不能复跑，也不能替它把 8 支 amber 猜成 0。也就是说：

- 引擎/应用层已知缺口：**0 项**；
- 对 libmypaint C 的 196 支整包 MAD 复测：**本机不可执行**，不是“已确认 196/196”。

在这台机器上可执行的门禁是：`hokusai-core/brush/tile-mem/compat` 76 个测试、`pigment-engine` 553 个、`pigment-surface` 113 个、`pigment-app` 217 个，共 **959 个测试全部通过**；其中 `hokusai-compat` 的 17 个 C golden 快照仍逐像素通过（golden `calligraphy_slow_drag.png` 因第一轮 `DABS_PER_*` 顺序修正而有意更新）。

## 8. 2026-10-21 第三轮：逐函数复核上游后新发现并修复的偏离

本轮以 `_libmypaint_ref/mypaint-brush.c`、`mypaint-tiled-surface.c`、`brushmodes.c`、`helpers.c`、`rng-double.c` 与 `_mypaint_ref/lib/brush.py` 为权威，逐函数复核。上一轮清单之外仍存在以下**真实语义偏离**，已修复并补回归：

| # | 偏离（相对于上游） | 影响 | 修复 / 回归 |
|---|---|---|---|
| 1 | `radius_by_random` 被 `max(0)` 截断，上游使用**有符号** `rand_gauss()*setting` | Dieterle 等 9 支官方笔刷的 `custom → ±0.07` 半径抖动丢失一半 | `hokusai-core/src/stroke.rs:849`；`radius_by_random_uses_the_signed_setting` |
| 2 | `direction_filter` 低通使用未乘 `viewzoom` 的步长；上游先乘 `STATE.VIEWZOOM` | 所有带方向滤波的笔刷在非 100% 缩放下滤波强度错误（低缩放工作流尤其明显） | `stroke.rs:726`；`direction_filter_uses_viewzoom_scaled_steps` |
| 3 | 采样 smudge 颜色时直接传浮点 dab 坐标；上游传 `ROUND(x), ROUND(y)` | 所有 smudge 笔刷的采样中心偏移 ≤0.5px | CPU `stroke.rs:1280`、GPU 事件采样 `stroke.rs:933`；`smudge_sampling_uses_c_round_pixel_coordinates` |
| 4 | `get_color_pigment` 的频域累加器跨 64px tile 连续携带；上游每个 tile 都从累计 `sum_r/g/b` 重新升频 | 采样半径跨 tile 的大 smudge 笔刷颜色偏差 | `brushmodes.rs:1814`、`pigment-surface/src/color_sample.rs:125`、`pigment-render/src/gpu_sample.rs:491`；CPU/GPU 采样对照通过 |
| 5 | HSV/HSL 动态先把 V/L 夹到 [0,1] 再做饱和度运算；上游用**未夹**值，最后才在转换函数里夹 | `change_color_v` + `change_color_hsv_s` 等组合（官方 2 支）颜色偏差 | `stroke.rs:1435/1447`；`hsv_saturation_dynamics_use_unclamped_value` |
| 6 | `LockAlpha_Paint` 会写回 `a_out` 并改 alpha；上游锁 alpha、始终按原 `rgba[3]` 预乘 | lock_alpha + paint_mode 的第三方笔刷每 dab 丢 1 fix15 alpha，长笔画可见褪色 | `brushmodes.rs:850`；`lock_alpha_paint_leaves_destination_alpha_untouched` |
| 7 | legacy/spectral smudge 只看当 dab 的 `paint > 0`；上游 `legacy_smudge = paint <= 0 && paint 设置是常量` | `paint_mode` 有曲线且某 dab 为 0 的第三方笔刷走错混合路径 | `stroke.rs:412/895/1280/1318`；`curve_driven_paint_mode_zero_uses_spectral_sampling` |
| 8 | `opaque_multiply` 因子先夹到 [0,1]；上游保留 0..2，只夹乘积 | 0.5 opaque × 2 应为全不透明，旧实现只有 0.5 | `stroke.rs:1137`；`opaque_multiply_above_one_can_reach_full_opacity` |
| 9 | 引擎内 `base_radius` 被夹到 0.2..1000；上游仅在 `count_dabs_to` 里夹，速度归一化/噪声/offset/VIEWZOOM 用未夹值 | 极小半径笔刷的速度/噪声/offset/viewzoom 曲线偏差 | `stroke.rs:349`（fresh seed 同步） |
| 10 | Gridmap 用 `rem_euclid` 且 `exp(scale)` 下限 1e-3；上游用 `mod_arith` 且不设下限 | 极端 gridmap 曲线偏差 | `stroke.rs:637` |
| 11 | 重置/长暂停分支返回“未画”且不请求切段；上游返回 TRUE | >5s 停顿后的 undo 切段粒度不同 | `stroke.rs:287`，预滚事件会清掉自己的 split |
| 12 | 数位板预热递归事件把当前 barrel_rotation 传进去；上游传 0 | 带笔杆旋转的笔刷在 dtime>0.1 预热时状态不同 | `stroke.rs:101-120` |
| 13 | `skipped_dtime` 用 f64 累加；上游是 float | 连续 tracking_noise 跳事件时累计 dt 精度不同 | `state.rs` / `stroke.rs:152` |
| 14 | line 模式用 `BrushState::default()` 重置，RNG 重新播种 1000；上游 `brush.reset()` 保留 RNG | 直线/椭圆工具每笔的 random 序列重复 | `pigment-engine/src/mode/line.rs:402/542` |

另修正：`hokusai-wasm` 的两个测试原本假设“第 2 个事件就出 dab”。上游在 reset 事件之后还需要一个只更新 `DABS_PER_*` 的事件，第三个事件才出 dab；测试已改为三事件（`crates/hokusai-wasm/tests/api.rs:60`）。

### 本轮仍保留的“有意差异/近似”

- **GPU 延迟 smudge 每 8 个 dab 采样一次**（`PIGMENT_SMUDGE_SAMPLE_DABS`，默认 8；设 1 可逐 dab）。上游 `smudge_length_log=0` 时每个 dab 都重采样。这是性能近似，不是逐像素等价。
- **Instant Preview / mipmap 显示**：低缩放时预览按当前 mip 级别绘制，最终像素由全分辨率事件重放保证；预览本身不是上游行为。
- **GPU 后端数值差**：`gpu_dab_check` 单 dab 有 10/16384 个通道差 1 fix15 LSB；GPU 采样 `gpu_sample_check` maxdiff≈1e-5。属于浮点实现差异，不是设置/公式偏离。
- **额外输入 `tilt`**：hokusai 保留的扩展，上游没有；仅影响使用该非标准输入的笔刷文件。
- **`Brush::stroke_to` 旧包装固定 `linear=false`**；MyPaint 应用走 `stroke_to_full(..., true)`。pigment 的 `StrokeEvent::apply` 已传 `true`。
- **`.myb` v2（# 旧格式）不支持**；只支持 v3 JSON。MyPaint 会迁移 v2，hokusai 不会。
- **GUI 层尚未对齐**：`FreehandMode` 的首次 begin abrupt reset（`BrushworkState::begin` 里 `first_begin` 被丢弃）与设备切换 `brush.reset()` 未接。这会让“进入模式后的第一笔”少一个预热事件，并可能沿用上一模式的 smudge/位置状态。它属于交互层，不在本轮核心公式修复范围。
- **Stroke 录制**：hokusai 的 `BrushState` 快照包含 RNG；MyPaint 的 `get_states_as_array` 不含 RNG。两者重放语义不同（像素级不影响）。
- **稀疏取色 RNG 平台差异**：Windows 已对齐 MSVC `rand()`；Linux/macOS 与当地 libc 不保证逐位一致（上游用进程 libc）。

