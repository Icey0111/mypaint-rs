# pigment UI 结构

这是 MyPaint 的 Rust/egui 重构外壳。界面版式取作者另一套
ArrowVortex 编辑器升级稿的
编辑器密度（画布优先、窄顶栏/状态栏、1px 分隔、无卡片投影），颜色取
MyPaint 参考机 GTK3 / Adwaita Dark 的实测色阶。

## 布局

```text
┌ 菜单栏  文件 / 编辑 / 视图 / 笔刷 / 颜色 / 图层 / 暂存区 / 窗口 / 帮助
├ 工具条  文件 · 撤销 · 工具模式 · 笔刷尺寸/不透明度 · 当前色/笔刷 · 缩放 · 面板开关 · 全屏
│
│ 笔刷侧栏         画布（无限画布 + GPU tile）           颜色 / 图层侧栏
│
└ 状态栏  瞬时消息 · 图层 · 工具 · 笔刷 · 颜色 · 缩放 · 画布 · 版本
```

- 左右侧栏可拖拽调宽；工具条右侧三个图标分别开关笔刷、颜色、图层面板。
- 全屏 + `workspace.layout.autohide` 时，鼠标不在屏幕顶边会收起顶栏。
- 状态栏信息严格居中，瞬时消息在左侧、版本在右侧。

## 源文件

| 文件 | 职责 |
|---|---|
| `shell/theme.rs` | Adwaita Dark 色板、字号、行高、自绘按钮/滑条/复选/状态栏 |
| `shell/chrome.rs` | 根布局、工具条、笔刷网格、HSV 颜色面板、图层树、状态栏 |
| `shell/menubar.rs` | `assets/gui/menu.xml` 生成的九棵菜单；零圆角/零子菜单间距 |
| `shell/toolbars.rs` | 构建期图标 PNG 解码、白色着色纹理缓存、动作提示 |
| `shell/dialogs.rs` | 偏好、关于、输入测试、笔刷编辑器、重命名、恢复、确认、文件路径 |
| `editor.rs` | 笔刷三层编辑器（设置 → 输入 → 曲线） |

## 交互接线

- 菜单/工具条全部走 `PigmentApp::run_action`；可用性由 `action_available`，
  勾选态由 `chrome::action_checked` 提供。
- 面板开关、偏好窗口由 `chrome::chrome_action` 消费，不再落到旧停靠系统的
  no-op。
- 颜色改动经 `set_color_hsv` 写回 `ColorManager`，再投影到笔刷。
- 图层可见性/锁定/不透明度/混合模式直接走 `Document` 的带撤销命令。
- 模态文件/确认弹层打开时，画布与全局快捷键暂停输入。
- `Ctrl+B` 打开笔刷编辑器；`Tab`/`F11`/`F10` 等仍由原动作表处理。

## 尺寸约定

`UI_SCALE = 1.25`（`shell31.rs`）：正文 16pt、图标 21pt，实际像素约为
字号 × 1.25。侧栏宽度与菜单宽度是独立常量，不随字号一起放大。

## 绘制热路径

GPU tile 同步以 `LayerStack::tile_composite_revision(tx, ty)` 为水位线，而不是
全局 `composite_revision()`。全局版本在每一笔后都会变化，会让所有可见 tile
都判脏；per-tile 版本只包含该坐标上的背景/图层属性/各层 tile revision，
一笔只会重传笔尖覆盖的 1–4 块 64×64 tile。

诊断开关：`PIGMENT_PERF=1` 启动时，日志会每 30 帧输出：

```text
perf t=... frames=... ui_avg=...ms paint_frames=... paint_frame_avg=...ms last_uploads=...
```

`last_uploads` 在正常绘画时应为个位数；持续接近 `MAX_UPLOADS_PER_FRAME`
说明 tile 判脏范围过大。

### 远景缩放（<75%）

图案背景在 100% 时只覆盖约 24×16 块 tile，但 6% 时视口覆盖约 400×260 块。
如果继续逐 tile 合成纸纹，会超过显存容量并形成“每帧上传 128 块、淘汰 128 块”
的永久抖动。低于 `BG_TILE_MIN_ZOOM = 0.75` 时：

- 背景改用文档背景均色兜底（等价于源码 mipmap 在远景下的平均色）；
- 只给真实有笔迹的 tile 上传/发 draw call；
- 背景 tile 从显存移除，回到正常缩放后按需重建。

### 大画布远景预览

内容本身很大时，仅跳过背景 tile 仍然不够：视口内的内容 tile 也可能超过
显存容量。低于 75% 缩放时改为：

- 按 `log2(1/zoom)` 选择 mipmap level，把整幅文档渲染成一张缩略图；
- 远景只画 1 个带纹理的 quad（支持旋转/镜像）；
- 平移不重建；绘画期间保留旧预览，松笔后按新版本重建；
- 回到正常缩放时清空 GPU 水位线，恢复逐 tile 路径。

这样大画布在 fit/6% 下不会再有“每帧上传 128 / 淘汰 128”的永久抖动。

防抖：滚轮从 100% 缩到 6% 会连续跨过多个 mipmap level；如果每跨一级都
同步重建整幅预览，缩放过程会被多次全图扫描卡住。现在 level 变化只重置
200ms 计时器，等缩放停稳后只构建最终一级；等待期间继续显示旧预览。
绘画期间同样推迟，松笔后再重建。

### 面板关闭

笔刷 / 颜色 / 图层三个面板头右侧都有可点击的 `×`，点击后对应面板收起；
可再用 View 菜单里的 Brush Groups / Color Adjusters / Toggle Layers 重新打开。
按钮的命中区是独立 widget，不是装饰图标。
