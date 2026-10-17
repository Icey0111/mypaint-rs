#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成 `lib/color.py` 的颜色向量 fixture —— Rust 侧逐位门禁用。

用法
----
在仓库根（`E:\\MyPaint\\pigment`）下跑：

    python crates/pigment-engine/tests/tools/gen-color-vectors.py

可选参数：

    --ref   <源码根>   默认 E:\\MyPaint\\_mypaint_ref
    --out   <输出文件> 默认 <本脚本目录>/../fixtures/color_vectors.txt
                       （即 crates/pigment-engine/tests/fixtures/color_vectors.txt）
    --quiet            不打印每类向量的条数

脚本可重复跑，输出是确定的（没有随机数；除 Python `repr()` 外不依赖
本机 Python 版本的具体行为）。

为什么脚本在仓库里（而不是只在 `E:\\MyPaint\\.workbuddy\\tools\\`）
---------------------------------------------------------------
`E:\\MyPaint\\.workbuddy\\` **不在** git 仓库（仓库根是 `E:\\MyPaint\\pigment`），
在那里放脚本就等于不入库。门禁需要"脚本与 fixture 一起版本化、可重跑"，
所以入库的是这一份；`E:\\MyPaint\\.workbuddy\\tools\\gen-color-vectors.py`
是一个只负责转发的薄包装（同一条命令、同样的输出）。

事实来源与"真源码"的边界
------------------------
1. **颜色数学**：本脚本 `sys.path.insert(0, <源码根>)` 后 `import lib.color`
   —— 跑的是**真** `lib/color.py`。所有 `RGB_to_HCY` / `HCY_to_RGB` /
   `RGB_to_YCbCr_BT601` / `YCbCr_to_RGB_BT601` / `UIColor.get_hsv` /
   `get_luma` / `to_hex_str` / `to_contrasting` / `to_fill_pixel` /
   `new_from_hex_str` / `interpolate` / `__eq__` 的向量都是它算的,
   不是这里重写的公式。

   ⚠ 一处必要补救：`lib/color.py:27` 有 `from lib.gibindings import GdkPixbuf`
   （GTK 绑定）—— 即 `lib/color.py` **并非**纯 Python。本机没装 PyGObject，
   直接 `import lib.color` 会 `ModuleNotFoundError: gi`。所以脚本在 import
   之前把 `lib.gibindings` 换成一个只含 `GdkPixbuf = None` 的空模块。
   `GdkPixbuf` 在 `lib/color.py` 里只被 `new_from_pixbuf_average()`
   （`:222-262`）用到，本脚本不生成那类向量，stub 不会被任何被测函数读到。
   装了 PyGObject 的机器上脚本走真绑定，不做替换（脚本会打印是哪一种）。

2. **颜色调整回调**：`Brighter/Darker/Purer/Grayer/IncreaseHue/DecreaseHue`
   与 `Bigger/Smaller/MoreOpaque/LessOpaque` 的行为在 `gui/document.py`
   的回调里（`:1571-1644`），而 `gui/document.py` 顶部 import 了整套
   Gtk/Gdk/gui，无法 import。所以脚本用 `ast` 从 `gui/document.py` **原文**
   抽出这些函数的函数体、包成函数后 `exec`（见 `extract_callbacks()`）
   —— 执行的是源码那几行，不是照抄的公式。
   `self.app.brush` / `self.app.brush_adjustment` 用最小 stub 顶替：
   `GtkAdjustment` 的语义只有 `set_value()` 把值夹到 `[lower, upper]`
   （`gui/application.py:563-570` 用 `s.min`/`s.max` 建 adjustment）。

3. **笔刷设置上下界**：`s.min`/`s.max` 来自 libmypaint 设置元数据
   （`_libmypaint_ref/brushsettings.json` 经 `generate.py` 生成
   `brushsettings-gen.h`）。脚本直接读该 json 取
   `radius_logarithmic` 与 `opaque` 的上下界。

输出格式
--------
纯文本、`\\t` 分隔、`#` 开头是注释。浮点一律 Python `repr()`
（最短往返表示）——Rust 的 `str::parse::<f64>()` 是正确舍入的，所以
Rust 解析出的位模式与 Python 侧**完全相同**，可以断言 `==`。

行类型（第一列）：

    rgb2hsv      r g b -> h s v
    hsv2rgb      h s v -> r g b
    rgb2hcy      r g b -> h c y
    hcy2rgb      h c y -> r g b
    rgb2ycbcr    r g b -> Y Cb Cr
    ycbcr2rgb    Y Cb Cr -> r g b
    hsv2hcy      h s v -> h c y      (HCYColor(color=HSVColor(...))，含纯黑特例)
    hcy2hsv      h c y -> h s v      (HCYColor.get_hsv()，含纯黑特例)
    hsv_rt       r g b -> r' g' b'   (RGB→HSV→RGB 往返)
    hcy_rt       r g b -> r' g' b'   (RGB→HCY→RGB 往返)
    ycbcr_rt     r g b -> r' g' b'   (RGB→YCbCr→RGB 往返)
    luma         r g b -> l          (UIColor.get_luma / RGBColor)
    hcy_luma     h c y -> l          (HCYColor.get_luma 覆盖实现)
    ycc_luma     Y Cb Cr -> l        (YCbCrColor.get_luma 覆盖实现)
    contrasting  r g b -> c          (to_contrasting 的灰值)
    hex          r g b -> "#rrggbb"  (to_hex_str)
    from_hex     "#..." -> r g b     (new_from_hex_str，含非法输入)
    fillpixel    r g b -> u32        (to_fill_pixel)
    eq           kindA a0 a1 a2 kindB b0 b1 b2 -> (A==B) (B==A)
    adjust       action h s v -> h' s' v'
    brushstep    action value -> value'
    interp       kind a0 a1 a2 b0 b1 b2 steps -> steps 组三元组（展平）
"""

import argparse
import ast
import json
import sys
import textwrap
from pathlib import Path

DEFAULT_REF = r"E:\MyPaint\_mypaint_ref"
HERE = Path(__file__).resolve()                     # .../tests/tools/gen-color-vectors.py
DEFAULT_OUT = HERE.parents[1] / "fixtures" / "color_vectors.txt"

#: RGB 立方体网格：每通道 0,17,...,255（16 档 → 16³ = 4096 点）
GRID_STEP = 17
GRID = list(range(0, 256, GRID_STEP))

#: luma / hex / fillpixel / contrasting 这几种逐点向量隔几个网格点取一次
SPARSE = 8

#: 往返向量（hsv_rt/hcy_rt/ycbcr_rt）隔几个网格点取一次
RT_STRIDE = 8


def stub_gibindings():
    """把 `lib.gibindings` 换成空模块（仅当本机没有 PyGObject）。"""
    import types

    try:
        import gi  # noqa: F401

        return False  # 有真绑定，不用 stub
    except ImportError:
        pass
    mod = types.ModuleType("lib.gibindings")
    mod.GdkPixbuf = None
    sys.modules["lib.gibindings"] = mod
    return True


def load_color_module(ref_root):
    """`sys.path.insert(0, <源码根>)` 后 import 真 `lib/color.py`。"""
    stubbed = stub_gibindings()
    sys.path.insert(0, str(ref_root))
    import lib.color as color  # noqa: E402

    src = Path(color.__file__).resolve()
    want = (Path(ref_root) / "lib" / "color.py").resolve()
    if src != want:
        raise SystemExit(f"导入到的不是源码文件：{src} != {want}")
    print(f"# lib.color 来自 {src}", file=sys.stderr)
    print(f"# lib.gibindings 走 stub: {stubbed}", file=sys.stderr)
    return color


# --------------------------------------------------------------------------
# 从 gui/document.py 抽回调（原文执行，不重写公式）
# --------------------------------------------------------------------------


class FakeAdjustment:
    """`GtkAdjustment` 的最小替身：`set_value` 夹到 [lower, upper]。"""

    def __init__(self, value, lower, upper):
        self._value = value
        self.lower = lower
        self.upper = upper

    def get_value(self):
        return self._value

    def set_value(self, value):
        self._value = min(max(value, self.lower), self.upper)


class FakeBrush:
    """`lib.brush.Brush` 的最小替身：只有颜色 HSV 三个基值。"""

    def __init__(self, hsv):
        self.hsv = tuple(hsv)

    def get_color_hsv(self):
        return self.hsv

    def set_color_hsv(self, hsv):
        self.hsv = tuple(hsv)


class FakeApp:
    def __init__(self, hsv, adjustments):
        self.brush = FakeBrush(hsv)
        self.brush_adjustment = adjustments


class FakeDocument:
    """`gui.document.Document` 的最小替身（这些回调只用到 `self.app`）。"""

    def __init__(self, app):
        self.app = app


CALLBACK_NAMES = [
    # 颜色调整（gui/document.py:1592-1644）
    "brighter_cb",
    "darker_cb",
    "increase_hue_cb",
    "decrease_hue_cb",
    "purer_cb",
    "grayer_cb",
    # 笔刷尺寸 / 不透明度（gui/document.py:1571-1590）
    "brush_bigger_cb",
    "brush_smaller_cb",
    "more_opaque_cb",
    "less_opaque_cb",
]


def extract_callbacks(document_py):
    """从 `gui/document.py` 原文抽出回调函数体并 exec。

    返回 {函数名: callable(self, action)}。函数体来自
    `ast.get_source_segment`，与源码逐字一致（只去掉 docstring）。
    """
    src_path = Path(document_py)
    src = src_path.read_text(encoding="utf-8")
    tree = ast.parse(src, filename=str(src_path))
    out = {}
    for node in ast.walk(tree):
        if not isinstance(node, ast.FunctionDef) or node.name not in CALLBACK_NAMES:
            continue
        body = list(node.body)
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)
        ):
            body = body[1:]  # 去掉 docstring
        stmts = "\n".join(ast.get_source_segment(src, s) for s in body)
        wrapper = f"def {node.name}(self, action):\n" + textwrap.indent(stmts, "    ")
        ns = {}
        exec(compile(wrapper, str(src_path), "exec"), ns)
        out[node.name] = ns[node.name]
    missing = [n for n in CALLBACK_NAMES if n not in out]
    if missing:
        raise SystemExit(f"gui/document.py 里找不到这些回调：{missing}")
    return out


def setting_limits(json_path, cname):
    """从 `brushsettings.json` 读某个设置项的 (min, max)。"""
    data = json.loads(Path(json_path).read_text(encoding="utf-8"))
    for s in data["settings"]:
        if s["internal_name"] == cname:
            return float(s["minimum"]), float(s["maximum"])
    raise SystemExit(f"brushsettings.json 里没有 {cname}")


# --------------------------------------------------------------------------
# 向量生成
# --------------------------------------------------------------------------


def fmt(x):
    """浮点 → 最短往返字符串（Python repr）；布尔 → 0/1。"""
    if isinstance(x, bool):
        return "1" if x else "0"
    if isinstance(x, float):
        return repr(x)
    return str(x)


def line(*parts):
    return "\t".join(fmt(p) for p in parts)


def dedup(seq):
    seen = set()
    out = []
    for p in seq:
        if p not in seen:
            seen.add(p)
            out.append(p)
    return out


def boundary_rgb():
    """边界 RGB 点（0..255 整数通道）：极暗/极亮/饱和色/灰阶。"""
    vals = [0, 1, 2, 127, 128, 254, 255]
    pts = [(r, g, b) for r in vals for g in vals for b in vals]
    # 灰阶，含极暗与极亮
    for k in [0, 1, 2, 3, 4, 127, 128, 252, 253, 254, 255]:
        pts.append((k, k, k))
    # 极暗的饱和色（非零通道只有 1..3）
    for k in [1, 2, 3]:
        for hi in [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (255, 0, 255)]:
            pts.append(tuple(k if c else 0 for c in hi))
    return dedup(pts)


def hsv_space():
    """HSV 空间边界点：六个色相扇区、s/v 的 0 与 1 及附近。"""
    hs = [0.0, 1.0 / 6.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 5.0 / 6.0, 0.999, 1.0]
    ss = [0.0, 1e-6, 1e-3, 0.334, 0.5, 0.999, 1.0]
    vs = [0.0, 1e-6, 1e-3, 0.005, 0.5, 0.995, 0.999, 1.0]
    return [(h, s, v) for h in hs for s in ss for v in vs]


def hcy_space():
    """HCY 空间边界点：h 六扇区、c 的 0/2 与附近、y 的 0/1 与附近。"""
    hs = [0.0, 1.0 / 6.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 5.0 / 6.0, 0.999, 1.0]
    cs = [0.0, 1e-9, 1e-6, 0.001, 0.25, 0.5, 1.0, 2.0]
    ys = [0.0, 1e-9, 1e-6, 0.001, 0.005, 0.5, 0.995, 1.0]
    return [(h, c, y) for h in hs for c in cs for y in ys]


def ycbcr_space():
    """BT601 YCbCr 空间边界点：Y∈[0,1]、Cb/Cr∈[-0.5,0.5]（含超色域）。"""
    ys = [0.0, 0.001, 0.299, 0.5, 0.587, 0.999, 1.0]
    cbs = [-0.5, -0.331, -0.25, -0.169, 0.0, 0.25, 0.5]
    crs = [-0.5, -0.419, -0.25, -0.081, 0.0, 0.25, 0.5]
    return [(y, cb, cr) for y in ys for cb in cbs for cr in crs]


def main():
    ap = argparse.ArgumentParser(description="生成颜色向量 fixture")
    ap.add_argument("--ref", default=DEFAULT_REF)
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    ref = Path(args.ref)
    if not (ref / "lib" / "color.py").is_file():
        raise SystemExit(f"找不到源码：{ref / 'lib' / 'color.py'}")
    color = load_color_module(ref)

    RGBColor = color.RGBColor
    HSVColor = color.HSVColor
    HCYColor = color.HCYColor
    YCbCrColor = color.YCbCrColor
    RGB_to_HCY = color.RGB_to_HCY
    HCY_to_RGB = color.HCY_to_RGB
    RGB_to_YCbCr_BT601 = color.RGB_to_YCbCr_BT601
    YCbCr_to_RGB_BT601 = color.YCbCr_to_RGB_BT601

    callbacks = extract_callbacks(ref / "gui" / "document.py")
    json_path = ref.parent / "_libmypaint_ref" / "brushsettings.json"
    radius_lo, radius_hi = setting_limits(json_path, "radius_logarithmic")
    opaque_lo, opaque_hi = setting_limits(json_path, "opaque")

    lines = [
        "# 由 .workbuddy/tools/gen-color-vectors.py 生成，请勿手改。",
        f"# 事实来源：{ref}（lib/color.py 逐位对照；gui/document.py 回调原文 exec）",
        "# 浮点 = Python repr（最短往返）；Rust 侧应逐位 assert_eq!。",
        f"# radius_logarithmic ∈ [{radius_lo}, {radius_hi}]，opaque ∈ [{opaque_lo}, {opaque_hi}]",
    ]
    counts = {}

    def emit(kind, *parts):
        counts[kind] = counts.get(kind, 0) + 1
        lines.append(line(kind, *parts))

    # ---- 1. RGB 网格（4096 点）+ 边界点：三种模型的正向换算 ----
    grid_pts = [(r, g, b) for r in GRID for g in GRID for b in GRID]
    grid_rgb = [tuple(c / 255.0 for c in p) for p in grid_pts]
    bnd_rgb = [tuple(c / 255.0 for c in p) for p in boundary_rgb()]
    rgb_points = grid_rgb + bnd_rgb

    for i, (r, g, b) in enumerate(rgb_points):
        h, s, v = RGBColor(r, g, b).get_hsv()
        emit("rgb2hsv", r, g, b, h, s, v)
        hh, cc, yy = RGB_to_HCY((r, g, b))
        emit("rgb2hcy", r, g, b, hh, cc, yy)
        Y, Cb, Cr = RGB_to_YCbCr_BT601((r, g, b))
        emit("rgb2ycbcr", r, g, b, Y, Cb, Cr)
        # 往返：两个方向都受这里和后面两组向量约束，但组合结果仍单独固化，
        # 这样 Rust 侧可以用 == 断言整条链（源码里往返不是恒等变换）
        n_grid = len(grid_rgb)
        if i % RT_STRIDE == 0 or i >= n_grid:
            emit("hsv_rt", r, g, b, *HSVColor(h, s, v).get_rgb())
            emit("hcy_rt", r, g, b, *HCY_to_RGB((hh, cc, yy)))
            emit("ycbcr_rt", r, g, b, *YCbCr_to_RGB_BT601((Y, Cb, Cr)))
        # 稀疏的逐点标量量
        if i % SPARSE == 0 or i >= n_grid:
            col = RGBColor(r, g, b)
            emit("luma", r, g, b, col.get_luma())
            emit("contrasting", r, g, b, col.to_contrasting().r)
            emit("hex", r, g, b, col.to_hex_str())
            emit("fillpixel", r, g, b, col.to_fill_pixel())

    # ---- 2. HSV → RGB / HSV → HCY ----
    for h, s, v in hsv_space():
        emit("hsv2rgb", h, s, v, *HSVColor(h, s, v).get_rgb())
        hcy = HCYColor(color=HSVColor(h, s, v))
        emit("hsv2hcy", h, s, v, hcy.h, hcy.c, hcy.y)

    # ---- 3. HCY → RGB / HCY → HSV / HCY.get_luma ----
    for h, c, y in hcy_space():
        emit("hcy2rgb", h, c, y, *HCY_to_RGB((h, c, y)))
        emit("hcy2hsv", h, c, y, *HCYColor(h, c, y).get_hsv())
        emit("hcy_luma", h, c, y, HCYColor(h, c, y).get_luma())

    # ---- 4. YCbCr → RGB / YCbCr.get_luma ----
    for Y, Cb, Cr in ycbcr_space():
        emit("ycbcr2rgb", Y, Cb, Cr, *YCbCr_to_RGB_BT601((Y, Cb, Cr)))
        emit("ycc_luma", Y, Cb, Cr, YCbCrColor(Y, Cb, Cr).get_luma())

    # ---- 5. hex 解析（含非法输入 → default [0.5,0.5,0.5]）----
    hex_cases = [
        "#ff0000", "#00ff00", "#0000ff", "#ffffff", "#000000", "#808080",
        "#a1b2c3", "#ee3333", "#003153", "#44aa66", "#aa6633", "#336699",
        "0xff8800", "0X1e2f3a", "#fff", "#000", "#abc", "#ABC", "#012",
        # 非法 → 默认灰
        "", "#", "ff0000", "#ff00", "#ff00000", "#gggggg", "#ff 0000",
        "0x", "#a", "#ab", "#abcd", "#ff0000 ", " #ff0000",
    ]
    for s in hex_cases:
        col = RGBColor.new_from_hex_str(s)
        emit("from_hex", s, col.r, col.g, col.b)

    # ---- 6. 相等语义（四个子类各自的 __eq__；显式记录非对称性）----
    def mk(kind, vals):
        return {
            "rgb": lambda: RGBColor(*vals),
            "hsv": lambda: HSVColor(*vals),
            "hcy": lambda: HCYColor(*vals),
            "ycbcr": lambda: YCbCrColor(*vals),
        }[kind]()

    eq_pairs = [
        # round(3) 比较 / 8bpc 比较 / v==0 / y==0 / y==1 / 跨类
        ("rgb", (0.7, 0.45, 0.55), "rgb", (0.7, 0.45, 0.55)),
        ("rgb", (0.7, 0.45, 0.55), "rgb", (0.7004, 0.45, 0.55)),
        ("rgb", (0.7, 0.45, 0.55), "rgb", (0.4, 0.55, 0.45)),
        ("rgb", (0.5, 0.5, 0.5), "hsv", (0.0, 0.0, 0.5)),
        ("rgb", (0.0, 0.0, 0.0), "hsv", (0.7, 0.45, 0.0)),
        ("rgb", (1.0, 0.0, 0.0), "hsv", (0.0, 1.0, 1.0)),
        ("rgb", (0.7, 0.45, 0.55), "hsv", (0.7, 0.45, 0.55)),
        ("rgb", (0.0, 1.0, 0.0), "hcy", (1.0 / 3.0, 1.0, 0.5)),
        ("rgb", (0.0, 0.0, 1.0), "ycbcr", RGB_to_YCbCr_BT601((0.0, 0.0, 1.0))),
        ("hsv", (0.7, 0.45, 0.0), "hsv", (0.4, 0.55, 0.0)),
        ("hsv", (0.7, 0.45, 1.0), "hsv", (0.4, 0.55, 1.0)),
        ("hsv", (0.7, 0.45, 0.55), "hsv", (0.7, 0.4504, 0.55)),
        ("hsv", (0.7, 0.45, 0.55), "hcy", (0.7, 0.45, 0.55)),
        ("hsv", (0.0, 1.0, 1.0), "rgb", (1.0, 0.0, 0.0)),
        ("hcy", (0.7, 0.45, 0.55), "hcy", (0.7, 0.45, 0.55)),
        ("hcy", (0.7, 0.45, 0.0), "hcy", (0.4, 0.55, 0.0)),
        ("hcy", (0.7, 0.45, 1.0), "hcy", (0.4, 0.55, 1.0)),
        ("hcy", (0.7, 0.45, 0.55), "hcy", (0.7, 0.45, 0.5504)),
        ("hcy", (0.7, 0.45, 0.55), "rgb", (0.7, 0.45, 0.55)),
        ("ycbcr", (0.7, 0.45, 0.55), "ycbcr", (0.7, 0.45, 0.55)),
        ("ycbcr", (0.7, 0.45, 0.55), "ycbcr", (0.5, 0.45, 0.55)),
        ("ycbcr", (0.7, 0.45, 0.55), "rgb", (0.7, 0.45, 0.55)),
        # 非对称的证据：HCY/YCbCr 与非同类比时退回 8bpc 比较（AttributeError 分支），
        # 反方向走 RGBColor.__eq__ 的 round(3)。0.5001 与 0.5009 的 8bpc 都是 127，
        # 但 round(3) 分别是 0.5 与 0.501 —— 于是 A==B 为真、B==A 为假。
        ("hcy", (0.3, 0.0, 0.5001), "rgb", (0.5009, 0.5009, 0.5009)),
        ("ycbcr", (0.5001, 0.0, 0.0), "rgb", (0.5009, 0.5009, 0.5009)),
    ]
    for ka, va, kb, vb in eq_pairs:
        a, b = mk(ka, va), mk(kb, vb)
        emit("eq", ka, *va, kb, *vb, bool(a == b), bool(b == a))

    # ---- 7. 颜色调整回调（gui/document.py 原文 exec）----
    color_actions = {
        "Brighter": "brighter_cb",
        "Darker": "darker_cb",
        "IncreaseHue": "increase_hue_cb",
        "DecreaseHue": "decrease_hue_cb",
        "Purer": "purer_cb",
        "Grayer": "grayer_cb",
    }
    adj_hsv = [
        (h, s, v)
        for h in [0.0, 0.1, 0.5, 0.999]
        for s in [0.0, 0.005, 0.0075, 0.08, 0.5, 0.9999, 1.0]
        for v in [0.0, 0.005, 0.0075, 0.08, 0.5, 0.9999, 1.0]
    ]
    for name, cb_name in color_actions.items():
        cb = callbacks[cb_name]
        for h, s, v in adj_hsv:
            app = FakeApp((h, s, v), {})
            cb(FakeDocument(app), None)
            nh, ns, nv = app.brush.get_color_hsv()
            emit("adjust", name, h, s, v, nh, ns, nv)

    # ---- 8. 笔刷尺寸 / 不透明度（Bigger/Smaller/MoreOpaque/LessOpaque）----
    brush_actions = {
        "Bigger": ("brush_bigger_cb", "radius_logarithmic", radius_lo, radius_hi,
                   [-2.0, -1.9, -1.7, -1.0, 0.0, 1.2, 3.7, 5.8, 5.9, 6.0, 8.0]),
        "Smaller": ("brush_smaller_cb", "radius_logarithmic", radius_lo, radius_hi,
                    [-2.0, -1.9, -1.7, -1.0, 0.0, 1.2, 3.7, 5.8, 5.9, 6.0, 8.0]),
        "MoreOpaque": ("more_opaque_cb", "opaque", opaque_lo, opaque_hi,
                       [0.0, 0.0001, 0.1, 0.55, 0.6, 1.0, 1.1, 1.2, 1.9, 2.0, 3.0]),
        "LessOpaque": ("less_opaque_cb", "opaque", opaque_lo, opaque_hi,
                       [0.0, 0.0001, 0.1, 0.55, 0.6, 1.0, 1.1, 1.2, 1.9, 2.0, 3.0]),
    }
    for name, (cb_name, cname, lo, hi, values) in brush_actions.items():
        cb = callbacks[cb_name]
        for value in values:
            adj = FakeAdjustment(value, lo, hi)
            app = FakeApp((0.0, 0.0, 0.0), {cname: adj})
            cb(FakeDocument(app), None)
            emit("brushstep", name, value, adj.get_value())

    # ---- 9. 插值（HSV/HCY 走最短弧，RGB/YCbCr 线性）----
    interp_cases = [
        ("rgb", (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 3),
        ("rgb", (0.0, 0.0, 0.0), (1.0, 1.0, 1.0), 3),
        ("rgb", (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), 5),
        ("hsv", (0.0, 1.0, 1.0), (1.0 / 3.0, 1.0, 1.0), 3),
        ("hsv", (1.0 / 3.0, 1.0, 1.0), (0.0, 1.0, 1.0), 3),
        ("hsv", (0.9, 1.0, 1.0), (0.1, 1.0, 1.0), 5),
        ("hcy", (0.0, 0.8, 0.5), (1.0 / 3.0, 0.8, 0.5), 5),
        ("hcy", (0.9, 0.5, 0.2), (0.1, 0.9, 0.8), 4),
        ("ycbcr", RGB_to_YCbCr_BT601((1.0, 1.0, 0.0)),
         RGB_to_YCbCr_BT601((1.0, 0.0, 0.0)), 3),
    ]
    for kind, a, b, steps in interp_cases:
        ca, cb_ = mk(kind, a), mk(kind, b)
        flat = []
        for col in ca.interpolate(cb_, steps):
            if kind == "rgb":
                flat.extend(col.get_rgb())
            elif kind == "hsv":
                flat.extend(col.get_hsv())
            elif kind == "hcy":
                flat.extend((col.h, col.c, col.y))
            else:
                flat.extend((col.Y, col.Cb, col.Cr))
        emit("interp", kind, *a, *b, steps, *flat)

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")

    if not args.quiet:
        total = sum(counts.values())
        print(f"写入 {out_path}")
        print(f"向量总行数 {total}（另有 {4} 行注释）：")
        for k in sorted(counts):
            print(f"  {k:12s} {counts[k]}")


if __name__ == "__main__":
    main()
