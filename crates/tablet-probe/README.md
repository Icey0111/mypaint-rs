# tablet-probe

临时诊断工具：验证 **winit 0.31** 的 `PointerSource::TabletTool` 在当前机器 /
驱动 / 后端上能拿到数位板数据，同时验证
`winit 0.31 + egui 0.37 + egui-wgpu` 这条 Plan A 外壳能跑。

```powershell
cd E:\MyPaint\pigment
cargo run -p tablet-probe
```

打开窗口后用笔在窗口内画：

- 窗口里显示 `kind` / `pressure` / `tilt` / `pos`；
- 压力 > 0 时在指针处画一个半径随压力变化的圆（橡皮端为红色）；
- 控制台也会打印 `tablet move kind=Pen ... pressure=...`。

如果只显示 `mouse move` / 压力恒 0，说明当前输入被当成鼠标（或驱动没有走
Windows pointer/tablet 路径）。

## 控制台版

```powershell
cargo run -p tablet-probe --bin console
```

只打印事件、不创建 GPU 窗口，适合快速确认事件是否到达。

这不是应用的一部分，验证完可以删除这个 crate。
