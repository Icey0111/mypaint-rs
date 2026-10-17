#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成颜色工具模型的向量 fixture —— Rust 侧逐位门禁用（B4-2）。

用法
----
在仓库根（``E:\\MyPaint\\pigment``）下跑：

    python crates/pigment-engine/tests/tools/gen-color-tools-vectors.py

可选参数：

    --ref   <源码根>   默认 E:\\MyPaint\\_mypaint_ref
    --out   <输出文件> 默认 <本脚本目录>/../fixtures/color_tools_vectors.txt
                       （即 crates/pigment-engine/tests/fixtures/color_tools_vectors.txt）
    --quiet            不打印每类向量的条数

脚本可重复跑，输出确定（无随机数）。

事实来源与"真源码"的边界
------------------------
``gui/colors/adjbases.py`` / ``sliders.py`` 顶部 import 了 Gtk/Gdk/cairo，
无法 ``import``。所以本脚本用 ``ast`` 从**源码原文**抽出这些函数的函数体，
包成普通函数后 ``exec``（``extract_functions()``）：

- ``gui/colors/util.py`` ``clamp()``                （绝大多数的夹取规则）
- ``gui/colors/adjbases.py``
    - ``SliderColorAdjuster.point_to_amount()``     （像素 → 条值，含边框扣除）
    - ``SliderColorAdjuster.__scroll_cb()``         （滚轮：方向符号 + 夹取 + 换色）
    - ``ColorAdjusterWidget.__motion_notify_cb()``  （按钮 2/3 的 HCY 弯曲）
- ``gui/colors/sliders.py`` 九个滑条类的
    - ``get_color_for_bar_amount()``
    - ``get_bar_amount_for_color()``

也就是：**执行的是源码那几行**，不是照抄的公式。抽出来之后给它们喂最小
替身 ``self`` / ``Gdk`` / ``Gtk``（``SliderSelf`` / ``FakeGdk`` / ...），
这些替身只提供源码那几行真正读到的东西（``get_allocation()``、
``get_managed_color()``、``Gdk.ScrollDirection``、``Gdk.ModifierType``、
``Gtk.TextDirection``）。

⚠ 方法体被搬到模块层执行，所以源码里的私有属性名（``self.__button_down``
等）**不会**被 Python 做类名改写 —— 替身对象用字符串 ``setattr`` 存同名
属性即可（见 ``bend()``）。这只影响替身，不影响被测的那几行源码。

为什么不用 ``import gui.colors.adjbases``：整个模块在 class 定义期就要
``Gtk.EventBox`` / ``GObject.ParamFlags`` / ``cairo`` / ``Palette`` ...，
还要把 ``gui.dialogs``、``gui.uicolor`` 全部 stub 掉；用 ``ast`` 只抽
**纯数学**那几行，边界更清楚。

输出格式
--------
纯文本、``\\t`` 分隔、``#`` 开头是注释。浮点一律 Python ``repr()``
（最短往返表示）—— Rust 的 ``str::parse::<f64>()`` 是正确舍入的，所以
Rust 解析出的位模式与 Python 侧完全相同，可以 ``assert_eq!(to_bits())``。

行类型（第一列）：

    slider_set     <滑条类> <管理色类型> m0 m1 m2 <条值> <结果类型> r0 r1 r2
                   get_color_for_bar_amount(amt)
    slider_amt     <滑条类> <管理色类型> m0 m1 m2 <条值>
                   get_bar_amount_for_color(管理色)
    slider_point   <坐标> <总长> <边框> <竖直 0/1> <条值>
                   point_to_amount()
    slider_scroll  <滑条类> <管理色类型> m0 m1 m2 <竖直 0/1> <方向>
                   <结果类型> r0 r1 r2
                   __scroll_cb() 的整条路径（含方向符号与 0..1 夹取）
    bend           <起点色类型> s0 s1 s2 <按钮> <宽> <高> <sx> <sy> <ex> <ey>
                   <shift 0/1> <ctrl 0/1> <rtl 0/1> <结果类型> r0 r1 r2
                   __motion_notify_cb() 的按钮 > 1 分支

颜色类型串：``rgb`` / ``hsv`` / ``hcy`` / ``ycbcr``。
"""

import argparse
import ast
import sys
import textwrap
from pathlib import Path

DEFAULT_REF = r"E:\MyPaint\_mypaint_ref"
HERE = Path(__file__).resolve()
DEFAULT_OUT = HERE.parents[1] / "fixtures" / "color_tools_vectors.txt"


# --------------------------------------------------------------------------
# 真 `lib/color.py`
# --------------------------------------------------------------------------


def stub_gibindings():
    """把 `lib.gibindings` 换成空模块（仅当本机没有 PyGObject）。"""
    import types

    try:
        import gi  # noqa: F401

        return False
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
# 替身：只提供源码那几行真正读到的东西
# --------------------------------------------------------------------------


class FakeAllocation:
    def __init__(self, width, height):
        self.width = width
        self.height = height


class FakeGdk:
    """`Gdk.ScrollDirection` / `Gdk.ModifierType` 的最小替身。"""

    class ScrollDirection:
        UP = "up"
        DOWN = "down"
        LEFT = "left"
        RIGHT = "right"
        SMOOTH = "smooth"

    class ModifierType:
        SHIFT_MASK = 1
        CONTROL_MASK = 4


class FakeGtk:
    """`Gtk.TextDirection` 的最小替身。"""

    class TextDirection:
        LTR = 0
        RTL = 1


class FakeLogger:
    def debug(self, *args, **kwargs):
        pass


class SliderSelf:
    """`SliderColorAdjuster` 子类的最小替身（滑条换算用）。

    `bar_fn` / `color_fn` 指向从 `sliders.py` 抽出来的真方法，
    按未绑定的函数调用（`bar_fn(self, col)`）。
    """

    BORDER_WIDTH = 2
    SCROLL_DELTA = 0.015

    def __init__(self, managed=None, vertical=False, width=100, height=24, border=2):
        self._managed = managed
        self.vertical = vertical
        self.BORDER_WIDTH = border
        self._alloc = FakeAllocation(width, height)
        self.bar_fn = None
        self.color_fn = None
        self.set_calls = []

    def get_allocation(self):
        return self._alloc

    def get_managed_color(self):
        return self._managed

    def set_managed_color(self, color):
        self._managed = color
        self.set_calls.append(color)

    def get_bar_amount_for_color(self, col):
        return self.bar_fn(self, col)

    def get_color_for_bar_amount(self, amt):
        return self.color_fn(self, amt)


class BendSelf:
    """`ColorAdjusterWidget` 的最小替身（HCY 弯曲用）。

    `__button_down` / `__drag_start_pos` / `__drag_start_color` 三个私有
    名字用字符串 `setattr` 存 —— 抽取出来的函数体在模块层，不做类名改写，
    所以这里也必须用字面名字（见模块文档）。
    """

    IS_DRAG_SOURCE = False
    ALLOW_HCY_TWEAKING = True

    def __init__(self, area, rtl):
        self._alloc = FakeAllocation(*area)
        self._rtl = rtl
        self._managed = None
        self.set_calls = []
        setattr(self, "__button_down", None)
        setattr(self, "__drag_start_pos", None)
        setattr(self, "__drag_start_color", None)

    def get_allocation(self):
        return self._alloc

    def get_direction(self):
        return FakeGtk.TextDirection.RTL if self._rtl else FakeGtk.TextDirection.LTR

    def get_managed_color(self):
        return self._managed

    def set_managed_color(self, color):
        self._managed = color
        self.set_calls.append(color)


# --------------------------------------------------------------------------
# 从源码原文抽函数体并 exec
# --------------------------------------------------------------------------

#: (相对源码根的路径, 类名或 None, 函数名)
WANTED = [
    ("gui/colors/util.py", None, "clamp"),
    ("gui/colors/adjbases.py", "SliderColorAdjuster", "point_to_amount"),
    ("gui/colors/adjbases.py", "SliderColorAdjuster", "__scroll_cb"),
    ("gui/colors/adjbases.py", "ColorAdjusterWidget", "__motion_notify_cb"),
]
#: sliders.py 里九个滑条类 × 两个方法
SLIDER_CLASSES = [
    "RGBRedSlider",
    "RGBGreenSlider",
    "RGBBlueSlider",
    "HSVHueSlider",
    "HSVSaturationSlider",
    "HSVValueSlider",
    "HCYHueSlider",
    "HCYChromaSlider",
    "HCYLumaSlider",
]
for _cls in SLIDER_CLASSES:
    WANTED.append(("gui/colors/sliders.py", _cls, "get_color_for_bar_amount"))
    WANTED.append(("gui/colors/sliders.py", _cls, "get_bar_amount_for_color"))


def _strip_docstring(body):
    body = list(body)
    if (
        body
        and isinstance(body[0], ast.Expr)
        and isinstance(body[0].value, ast.Constant)
        and isinstance(body[0].value.value, str)
    ):
        body = body[1:]
    return body


def _body_source(src, body):
    """把函数体原文按行切出来，再统一去掉函数体那一层的缩进。

    不能逐条语句 ``ast.get_source_segment()`` 后统一 ``textwrap.indent()``：
    源码里 ``if``/``else`` 两条语句的绝对缩进一样，逐条切再整体加 4 格会让
    ``else`` 比 ``if`` 更深（IndentationError）。按"第一条到最后一条语句的
    原始行区间"整块 dedent，相对缩进才保得住。
    """
    if not body:
        return "pass"
    lines = src.splitlines()
    start = body[0].lineno - 1
    end = body[-1].end_lineno  # 1-based, inclusive → 切片右端
    return textwrap.dedent("\n".join(lines[start:end]))


def extract_functions(ref_root):
    """从源码原文抽函数体，拼成一段模块源码后 exec。

    返回 (命名空间, {别名: (源文件, 行号)})。
    """
    cache = {}
    chunks = []
    origins = {}
    for rel, cls, fname in WANTED:
        src_path = (Path(ref_root) / rel).resolve()
        if src_path not in cache:
            src_text = src_path.read_text(encoding="utf-8")
            cache[src_path] = (
                src_text,
                ast.parse(src_text, filename=str(src_path)),
            )
        src, tree = cache[src_path]
        node = None
        for item in ast.walk(tree):
            if cls is None:
                if isinstance(item, ast.FunctionDef) and item.name == fname and item.col_offset == 0:
                    node = item
                    break
            elif isinstance(item, ast.ClassDef) and item.name == cls:
                for sub in item.body:
                    if isinstance(sub, ast.FunctionDef) and sub.name == fname:
                        node = sub
                        break
        if node is None:
            raise SystemExit(f"{src_path} 里找不到 {cls or '<module>'}.{fname}")
        alias = f"{cls}__{fname}" if cls else fname
        stmts = _body_source(src, _strip_docstring(node.body))
        wrapper = f"def {alias}({ast.unparse(node.args)}):\n" + textwrap.indent(stmts, "    ")
        chunks.append(
            f"# ---- {rel}:{node.lineno} {cls or ''}.{fname} ----\n{wrapper}\n"
        )
        origins[alias] = (str(src_path), node.lineno)

    module_src = "\n".join(chunks)
    ns = {
        "math": __import__("math"),
        "Gdk": FakeGdk,
        "Gtk": FakeGtk,
        "logger": FakeLogger(),
    }
    exec(compile(module_src, "<extracted>", "exec"), ns)
    # clamp 由 util.py 的原文提供，其它函数体里的 clamp() 都解析到它
    for alias, (path, lineno) in sorted(origins.items()):
        print(f"# 抽自 {path.rsplit(chr(92), 1)[-1]}:{lineno} -> {alias}", file=sys.stderr)
    return ns, origins


# --------------------------------------------------------------------------
# 颜色 ↔ fixture 文本
# --------------------------------------------------------------------------

KIND_OF_CLASS = {
    "RGBColor": "rgb",
    "HSVColor": "hsv",
    "HCYColor": "hcy",
    "YCbCrColor": "ycbcr",
}


def kind_of(col):
    return KIND_OF_CLASS[type(col).__name__]


def space_triple(kind, col):
    if kind == "rgb":
        return col.get_rgb()
    if kind == "hsv":
        return col.get_hsv()
    if kind == "hcy":
        return (col.h, col.c, col.y)
    if kind == "ycbcr":
        return (col.Y, col.Cb, col.Cr)
    raise AssertionError(kind)


def fmt(x):
    if isinstance(x, bool):
        return "1" if x else "0"
    if isinstance(x, float):
        return repr(x)
    return str(x)


def line(*parts):
    return "\t".join(fmt(p) for p in parts)


# --------------------------------------------------------------------------
# 向量生成
# --------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description="生成颜色工具向量 fixture")
    ap.add_argument("--ref", default=DEFAULT_REF)
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    ref = Path(args.ref)
    for rel in ("lib/color.py", "gui/colors/adjbases.py", "gui/colors/sliders.py",
                "gui/colors/util.py"):
        if not (ref / rel).is_file():
            raise SystemExit(f"找不到源码：{ref / rel}")

    color = load_color_module(ref)
    ns, origins = extract_functions(ref)
    ns.update(
        RGBColor=color.RGBColor,
        HSVColor=color.HSVColor,
        HCYColor=color.HCYColor,
        YCbCrColor=color.YCbCrColor,
    )
    RGB_to_YCbCr_BT601 = color.RGB_to_YCbCr_BT601

    def make(kind, vals):
        return {
            "rgb": color.RGBColor,
            "hsv": color.HSVColor,
            "hcy": color.HCYColor,
            "ycbcr": color.YCbCrColor,
        }[kind](*vals)

    # 从 sliders.py 抽出来的方法按 (类名, 方法) 索引
    def slider_methods(cls):
        return (
            ns[f"{cls}__get_bar_amount_for_color"],
            ns[f"{cls}__get_color_for_bar_amount"],
        )

    def fn(cls, name):
        return ns[f"{cls}__{name}"]

    point_to_amount = fn("SliderColorAdjuster", "point_to_amount")
    scroll_cb = fn("SliderColorAdjuster", "__scroll_cb")
    motion_notify_cb = fn("ColorAdjusterWidget", "__motion_notify_cb")

    # 管理色：三原色/黑白/三种非 RGB 空间，覆盖跨空间换算
    managed_colors = [
        ("rgb", (0.3, 0.6, 0.7)),
        ("rgb", (0.0, 0.0, 0.0)),
        ("rgb", (1.0, 1.0, 1.0)),
        ("hsv", (0.25, 0.6, 0.8)),
        ("hcy", (0.5, 0.8, 0.4)),
        ("ycbcr", RGB_to_YCbCr_BT601((0.2, 0.4, 0.9))),
    ]
    # 条值：0/0.5/1 三个边界 + 越界值（源码不夹取，如实记录）
    amounts = [0.0, 0.25, 0.5, 1.0, 1.5, -0.25]

    lines = [
        "# 由 crates/pigment-engine/tests/tools/gen-color-tools-vectors.py 生成，请勿手改。",
        f"# 事实来源：{ref}（adjbases.py / sliders.py / util.py 的函数体 ast 抽出后 exec）",
        "# 浮点 = Python repr（最短往返）；Rust 侧应逐位 assert_eq!。",
        "# 抽出的函数：",
    ]
    for alias in sorted(origins):
        path, lineno = origins[alias]
        lines.append(f"#   {path}:{lineno}  {alias}")
    counts = {}

    def emit(kind, *parts):
        counts[kind] = counts.get(kind, 0) + 1
        lines.append(line(kind, *parts))

    # ---- 1. 滑条：条值 ↔ 分量（sliders.py 原文方法）----
    for cls in SLIDER_CLASSES:
        bar_fn, color_fn = slider_methods(cls)
        for mkind, mvals in managed_colors:
            managed = make(mkind, mvals)
            # get_bar_amount_for_color(managed)
            fake = SliderSelf(managed=managed)
            fake.bar_fn = bar_fn
            amt = bar_fn(fake, managed)
            emit("slider_amt", cls, mkind, *mvals, amt)
            # get_color_for_bar_amount(amt)
            for a in amounts:
                fake = SliderSelf(managed=managed)
                fake.color_fn = color_fn
                col = color_fn(fake, a)
                okind = kind_of(col)
                emit("slider_set", cls, mkind, *mvals, a, okind, *space_triple(okind, col))

    # ---- 2. 滑条坐标 → 条值（adjbases.py:1055-1067 原文）----
    for length in (100.0, 120.0, 20.0):
        for border in (2.0, 0.0):
            for vertical in (False, True):
                for pos in (-10.0, 0.0, 1.0, 2.0, 2.5, 50.0, 97.5, 99.0, 100.0, 101.0, 120.0):
                    fake = SliderSelf(vertical=vertical, width=length, height=length, border=border)
                    # 源码签名是 (x, y)：横条读 x，竖条读 y（adjbases.py:1057-1062）
                    if vertical:
                        amt = point_to_amount(fake, 0.0, pos)
                    else:
                        amt = point_to_amount(fake, pos, 0.0)
                    emit("slider_point", pos, length, border, vertical, amt)

    # ---- 3. 滚轮（adjbases.py:1069-1083 原文，含方向符号与 0..1 夹取）----
    directions = [
        ("UP", FakeGdk.ScrollDirection.UP),
        ("DOWN", FakeGdk.ScrollDirection.DOWN),
        ("LEFT", FakeGdk.ScrollDirection.LEFT),
        ("RIGHT", FakeGdk.ScrollDirection.RIGHT),
        ("SMOOTH", FakeGdk.ScrollDirection.SMOOTH),
    ]
    for cls in SLIDER_CLASSES:
        bar_fn, color_fn = slider_methods(cls)
        for mkind, mvals in managed_colors:
            for vertical in (False, True):
                for dname, direction in directions:
                    managed = make(mkind, mvals)
                    fake = SliderSelf(managed=managed, vertical=vertical)
                    fake.bar_fn = bar_fn
                    fake.color_fn = color_fn
                    event = type("FakeEvent", (), {"direction": direction})()
                    scroll_cb(fake, fake, event)
                    col = fake.get_managed_color()
                    okind = kind_of(col)
                    emit("slider_scroll", cls, mkind, *mvals, vertical, dname,
                         okind, *space_triple(okind, col))

    # ---- 4. 按钮 2/3 拖动的 HCY 弯曲（adjbases.py:739-788 原文）----
    bend_starts = [
        ("hcy", (0.5, 0.6, 0.5)),
        ("rgb", (0.2, 0.4, 0.9)),
        ("hsv", (0.1, 0.7, 0.8)),
        ("hcy", (0.0, 0.0, 0.0)),
        ("hcy", (0.999, 0.99, 0.999)),
        ("ycbcr", RGB_to_YCbCr_BT601((0.9, 0.2, 0.2))),
    ]
    areas = [(100, 100), (120, 80)]
    deltas = [
        (0, 0), (0, -10), (0, 10), (10, 0), (-10, 0),
        (7, -3), (-7, 3), (30, -30), (-30, 30), (0.5, 0.4),
    ]
    mods = [(False, False), (True, False), (False, True)]
    rtl_values = [False, True]

    def bend_case(start_kind, start_vals, button, area, dxy, shift, control, rtl):
        start = make(start_kind, start_vals)
        w, h = area
        sx, sy = w / 2.0, h / 2.0
        ex, ey = sx + dxy[0], sy + dxy[1]
        fake = BendSelf(area, rtl)
        state = (FakeGdk.ModifierType.SHIFT_MASK if shift else 0) | (
            FakeGdk.ModifierType.CONTROL_MASK if control else 0
        )
        event = type("FakeEvent", (), {"x": ex, "y": ey, "state": state, "button": button})()
        setattr(fake, "__button_down", button)
        setattr(fake, "__drag_start_pos", (sx, sy))
        setattr(fake, "__drag_start_color", start)
        motion_notify_cb(fake, fake, event)
        col = fake.get_managed_color()
        okind = kind_of(col)
        assert okind == "hcy", f"弯曲结果应为 HCYColor，实际 {okind}"
        emit("bend", start_kind, *start_vals, button, w, h, sx, sy, ex, ey,
             shift, control, rtl, okind, *space_triple(okind, col))

    for start_kind, start_vals in bend_starts:
        for area in areas:
            for dxy in deltas:
                for shift, control in mods:
                    for rtl in rtl_values:
                        bend_case(start_kind, start_vals, 2, area, dxy, shift, control, rtl)
    # 按钮 3 与按钮 2 走同一分支（源码只判 `button_down > 1`），抽样固化
    for start_kind, start_vals in bend_starts:
        for shift, control in mods:
            bend_case(start_kind, start_vals, 3, (100, 100), (-12, 5), shift, control, False)

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")

    if not args.quiet:
        total = sum(counts.values())
        print(f"写入 {out_path}")
        print(f"向量总行数 {total}：")
        for k in sorted(counts):
            print(f"  {k:14s} {counts[k]}")


if __name__ == "__main__":
    main()
