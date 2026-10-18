# tablet-probe

临时诊断工具：验证 **winit 0.31** 的 `PointerSource::TabletTool` 在当前机器 /
驱动 / 后端上能拿到数位板数据。

```powershell
cargo run -p tablet-probe
```

打开窗口后用笔在窗口内画：

- 有笔数据时会打印
  `tablet move kind=Pen pos=(…) pressure=0.xxx tilt=(…)`；
- 只打印 `mouse move` 说明当前输入被当成鼠标（或驱动没有走 Windows
  pointer/tablet 路径）。

这不是应用的一部分，验证完可以删除这个 crate。
