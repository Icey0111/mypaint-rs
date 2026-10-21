# MyPaint（pigment）全交互 / 全笔画检测报告

- 日期：2026-10-20
- 应用：`pigment-app`（winit 0.31 + egui 0.37 + wgpu 30）
- 隔离配置：`pigment/target/e2e/config`（默认不自动恢复、不写自动备份）
- 驱动：Win32 `SendInput` 真实鼠标/键盘 + `PrintWindow` 截图 + 应用侧 `PIGMENT_E2E_STATUS` 状态转储
- 判定：每个交互后同时核对**应用状态**（当前工具、图层栈、surface clock、撤销/重做深度）与**画布像素差**，不靠“看起来没崩”

## 结论

核心交互与全部笔画工具在修复后通过。共执行 **73 个交互检查点**，覆盖 9 个顶级菜单、9 个工具模式、自由手绘/直线/连接线/椭圆/墨迹/填充/移层/画框/对称 9 类笔画，以及颜色、笔刷、图层、视图、面板、对话框与文件路径。

没有发现会导致崩溃、死锁或画布永久损坏的问题。共发现并修复 **5 个真实交互缺陷**（见下）。

## 发现并修复的缺陷

| # | 现象 | 根因 | 修复 |
|---|---|---|---|
| 1 | 直线/连接线/椭圆模式的工具条选中态和底栏工具名不显示具体模式，自动化看到 `?` | `LineMode` 只实现了 `name()`，漏了 `action_name()`；`InteractionMode` 默认返回 `None` | 返回 `Some(self.kind.action_name())`，并加三种模式的回归测试。[line.rs](../hokusai/crates/hokusai-core/src/mode/line.rs:587) |
| 2 | 快速拖动时对称中心“拖不动” | 用 `response.hover_pos()` 判按下命中；egui 的 `drag_started()` 在越过阈值后才为真，此时指针已离开中心 | 改用 `pointer.press_origin()` 判命中。[canvas.rs](crates/pigment-app/src/shell/canvas.rs:1218) |
| 3 | 画框角柄快速拖动时无法缩放（引擎单测能过，UI 不行） | 所有模式都把 `mode_button_press` 延后到 `drag_started`，并传入**已移动后**的位置；5px 角柄命中区被跨过 | 起笔事件统一用 `press_origin` 的真实按下点。[canvas.rs](crates/pigment-app/src/shell/canvas.rs:684) |
| 4 | Layer 菜单过长，底部条目超出窗口、点不到 | 菜单行只有 19pt，但继承了默认 8pt 纵向 `item_spacing`，整张菜单被撑到 855px | 菜单、子菜单、最近文件列表都把纵向行距设为 0，分隔线单独留白；Layer 菜单现在在 900px 窗口内完整显示。[menubar.rs](crates/pigment-app/src/shell/menubar.rs:120) |
| 5 | 恢复笔刷快捷键槽位后，半径/不透明度变了，底栏“笔刷”仍显示旧名字 | `restore_brushkey` 恢复了快照，但没有同步 `brush_name` | 恢复快照名。[app.rs](crates/pigment-app/src/app.rs:5057) |

## 交互检查矩阵

### 菜单（9/9）

File、Edit、View、Brush、Color、Layer、Scratch、Window、Help 全部能打开、显示正确条目、Esc 关闭并恢复原画面。Layer 菜单修复后 bbox 为 `y=75..782`，完整落在 900px 窗口内。

### 工具模式（9/9）

工具条九个按钮 + 键盘快捷键双路径均通过：

| 工具 | 按钮 | 快捷键 | 结果 |
|---|---|---|---|
| 徒手 | ✔ | `p` | FreehandMode |
| 墨迹 | ✔ | `i` | InkingMode |
| 直线/曲线 | ✔ | `k` | StraightMode |
| 连接线 | ✔ | `j` | SequenceMode |
| 椭圆/圆 | ✔ | `o` | EllipseMode |
| 洪水填充 | ✔ | `g` | FloodFillMode |
| 移动图层 | ✔ | `m` | LayerMoveMode |
| 编辑画框 | ✔ | — | FrameEditMode |
| 编辑对称 | ✔ | `Ctrl+U` | SymmetryEditMode |

### 笔画（9 类 + 3 种笔刷/混合）

| 笔画 | 验证 |
|---|---|
| 自由手绘 | 单笔 21,517 px 改变；undo 像素完全恢复；redo 恢复 1,249 px 差（仅 UI 文本变化） |
| 直线 | 一次拖拽 9,961 px；undo 后与起笔前 0 px 差 |
| 连接线 | 两段 20,665 px；两次 undo 回到 0 px 差 |
| 椭圆 | 33,790 px；undo 后 0 px 差 |
| 墨迹 Inking | 收笔进入 ADJUST，节点/OK/垃圾桶按钮出现；点 OK 后 undo +1，点 X 取消；截图见 `target/e2e/ink2_adjust.png` |
| 洪水填充 | 闭合椭圆内点填：undo +1，tiles 28→31；外点/内点分别命中不同连通域 |
| 移动图层 | 拖动后 data bbox 平移，undo 恢复原 bbox |
| 画框 | 单击创建 256×256；拖右下角柄变为 306×296（+50/+40）；垃圾桶按钮关闭画框 |
| 对称 | `Ctrl+U` 自动开对称；中心从 `[482,393]` 拖到 `[562,433]`；笔画在轴两侧各出一份 |
| 橡皮笔刷 | 选中 `kneaded eraser large`，横穿底色后 3,353 px 改变；undo 后画布像素与起笔前一致（4,464 px 差全部在左/下 UI 区） |
| 涂抹笔刷 | 选中 `smudge+paint`，跨底色拖动后 surface clock +47、2,488 px 改变；undo 恢复 |
| 混合模式 | `n` 正常、`e` 橡皮、`Ctrl+E` 锁 alpha、`Shift+E` 颜色化；状态位正确。`e` 笔画 undo+1/clock+54；`Shift+E` 用红色在橙色底上画出 5,855 px 差 |

### 撤销 / 重做

- 直线、连接线、椭圆、墨迹、橡皮、涂抹、填充、移层均验证 undo。
- 空白→画→undo→redo 的整幅像素恢复验证通过。
- `Ctrl+Z` / `Ctrl+Y` 均走动作表，状态栏同步。

### 颜色（5/5）

- HSV 方块点击：`#003153` → `#436882`。
- 色相条点击：→ `#644382`。
- 历史色块：`#ee3333`。
- 调色板色块：`#262626`、`#ababab`、`#ffe5b4` 等。
- 取色 `r`：在 `#fee5b5` 笔迹上取值，状态色变为 `#fee5b5` 附近并回到 FreehandMode。

> 十六进制框是只读展示，不是输入框；本项没有把“不可编辑”误判成失败。

### 笔刷（5/5）

- 缩略图选择：`kneaded eraser large`、`smudge+paint`、`pencil`、`rough` 均正确。
- 搜索：输入 `smudge` 后列出 7 项，点第一项成功。
- 分组：Deevad → Classic，组名与笔刷列表刷新。
- 快捷键槽位：`Ctrl+1` 存 rough，切到 pencil 后按 `1`，半径/不透明度/名称都回到 rough（修复后）。
- 尺寸快捷键：`f` 增大、`d` 减小（radius 2.50→3.10→2.80）。

### 图层（8/8）

新建绘画层、新建组、复制层、删除层、显示/隐藏眼睛、不透明度滑条、混合模式下拉、撤销全部通过。

- 新建：1→2 层，undo +1。
- 组：2→3，组 `[0]` 含两个子层。
- 复制：`DuplicateLayer`，3 层。
- 删除：`RemoveLayer`，回到 1 层。
- 眼睛：`visible true→false→true`。
- 不透明度：100% → 46.9% → 25.2%。
- 模式：`Standard(SpectralWGM)` → `Standard(Normal)` → `Standard(Multiply)`。

### 视图（6/6）

- 滚轮缩放：100% → 448%，再回 100%。
- 右键拖拽平移（Windows 默认 Button3=Pan）：offset 改变。
- `Shift+右键` 旋转：0 → 0.1889 rad。
- `Ctrl+右键` 缩放：1.0 → 1.271。
- `F10` FitView：1.0 → 1.843。
- `ResetView` 菜单：zoom=1、rotation=0、mirror=false、offset 归零附近。

> 中键在 Windows 默认映射是弹出菜单（`Button2 → ShowPopupMenu`），不是平移；本次按源码的 Windows 映射验证，右键平移/Shift 旋转/Ctrl 缩放均通过。

### 面板与对话框

- 左笔刷栏、颜色、图层三个面板的 X 关闭与工具条按钮重开全部通过。
- Preferences：打开、显示兼容性区（默认模式、1.x/2.x 颜料开关）、关闭。
- About：打开/关闭。
- Input Test：打开，显示鼠标未激活、压力 1.000、倾斜 0、按键映射；关闭。
- Brush Editor：`Ctrl+B` 打开，显示 11 组设置；关闭。
- 原生 Save As / Open 对话框：`Ctrl+S`/`Ctrl+O` 能弹出，Esc 取消后应用恢复响应，未写文件。
- New：`Ctrl+N` 清空文档，undo=0、clock=0、bbox 空。

## 低缩放画笔卡顿回归（后续追加）

现象：在 12.5%～13% 这类远景缩放下，落笔后画面长时间不动，松笔后还要等一会儿才出现笔迹；大文档时整窗无响应接近 1 秒。

根因是两处叠加：

1. **缩略图按输出像素重复采样**：`render_thumbnail_rgba8` 对每个输出像素调用 `render_halved` 的 2^level × 2^level 块采样，level=4 时每个输出像素 256 次源采样；实测 4352×4352 bbox 单次重建 **965 ms**，直接堵死 UI。
2. **按住笔期间无限推迟重建**：`ensure_low_zoom_thumb` 在 `painting` 时把 `rebuild_at` 推到 `now+0.30` 并返回，整笔都画在旧缩略图上，松笔后再等 300ms。

修复：

- 缩略图改为「只遍历 content tile，每 tile 合成一次、降采样一次」，并加 **per-tile 编码块缓存**（revision 失效）和 **背景层缓存**；背景/图层内容不变时直接 memcpy。[layer.rs](crates/pigment-surface/src/layer.rs:2317)
- 缩略图 bbox 改为「视口世界包围盒 ∪ 内容包围盒」，纸纹铺满整个画布，不会只在笔迹矩形里出现。[canvas.rs](crates/pigment-app/src/shell/canvas.rs:385)
- 按住笔时每 **50ms** 更新一次缩略图，松笔立即重建；不再无限推迟。[canvas.rs](crates/pigment-app/src/shell/canvas.rs:493)
- 新增参考路径 parity 测试、块平均 parity 测试、缓存失效测试。

实测（载入用户备份，bbox 8040×5971，12.5%）：

| 阶段 | 耗时 |
|---|---|
| 冷启动/换 bbox 首帧 | 143 ms |
| 连续作画中每次缩略图重建 | **2.8 ms**（fill 0.6ms + tile 0.8ms + 其余） |
| 绘制帧平均 | 7.0 ms（约 140fps） |

截图 `target/e2e/z125_mid.png`、`target/e2e/vp_mid.png` 可见落笔过程中笔迹实时出现，纸纹覆盖整个视口。

## 测试总数

修复后重跑：

```text
pigment 工作区：887 passed / 0 failed（24 个 test binary）
  pigment-app            217
  pigment-engine         331 lib + 223 integration = 554
  pigment-surface        103 lib + 2 combine + 3 parity + 8 symmetry = 116
hokusai 4 个被应用依赖的 crate：76 passed / 0 failed
  hokusai-core 45 + hokusai-brush 5 + hokusai-tile-mem 9 + hokusai-compat 17
合计：963 passed / 0 failed
```

官方内嵌笔刷包：`official_pack.rs` 的 196/196 支笔刷全部解析、全部产生 dab 且至少改变一个像素；`pigment-surface` 的逐像素 parity / symmetry 门禁全绿。

> `hokusai-wasm` 是独立 JS 绑定 crate，不在 `pigment` 依赖图中；其 native API 测试仍有两个旧用例失败（`two_events_paint_pixels`、`tilt_arguments_accepted_and_affect_output`），不计入上面的 960，也不影响 `pigment-app`。本次没有把它包装成“全绿”。

## 证据

关键截图在 `pigment/target/e2e/`：

- `m3_*_open.png` — 九个顶级菜单
- `t_straight_after.png`、`t_seq_after.png`、`t_ellipse_after.png` — 三种几何笔画
- `ink2_adjust.png`、`ink2_accept.png` — 墨迹调整/接受
- `fill_center.png` — 闭合区域填充
- `layermove.png`、`frame_resized2.png` — 移层/画框缩放
- `symmetry.png`、`symcenter3.png` — 对称笔画与中心拖动
- `smudge_after.png`、`erase_after.png`、`colorize_after.png` — 笔刷/混合模式
- `prefs.png`、`about.png`、`inputtest.png`、`brusheditor_main.png` — 对话框

## 未覆盖 / 边界

- 真实数位板的压力、倾斜、笔尾橡皮端需要硬件事件；本次用鼠标路径验证了 `TabletInput` 的默认值与映射显示，没有伪造 HID 报文。
- 原生文件对话框只验证了“打开/取消”和“另存为/取消”，没有落盘写文件（避免污染工作区）。
- 全屏切换会改变窗口坐标，未纳入本轮自动点击；它不涉及绘画路径。
- 非 100% 缩放、旋转视图下的逐像素笔画位置只做了视图数值与画面存在性验证，没有做像素级几何对齐。
