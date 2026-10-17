#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成色轮 / 色方块 / 色立方的向量 fixture —— Rust 侧逐位门禁用（B4-4b）。

用法
----
在仓库根（``E:\\MyPaint\\pigment``）下跑：

    python crates/pigment-engine/tests/tools/gen-wheel-vectors.py

可选参数：

    --ref   <源码根>    默认 E:\\MyPaint\\_mypaint_ref
    --out   <输出文件>  默认 <本脚本目录>/../fixtures/wheel_vectors.txt
    --quiet 不打印各类向量的条数

脚本可重复跑，输出确定（无随机数）。

事实来源
--------
控件层文件（``gui/colors/adjbases.py`` / ``hsvwheel.py`` / ``hsvsquare.py`` /
``hsvcube.py``）顶部就 import GTK / cairo，**不能 import**。所以用 B4-2 / B4-4a
的老办法：``ast`` 把目标方法的**函数体原文**抽出来，包成模块级的普通函数后
``exec``，再用"最小替身"对象提供 ``self`` 上真正被读到的属性/方法。

抽的是（行号由脚本打印）：

    adjbases.py
        ColorManager.distort_hue / undistort_hue          色相畸变（:317-345）
        HueSaturationWheelMixin.get_radius / get_center   几何（:1116-1136）
        HueSaturationWheelMixin.get_color_at_position     位置 → 颜色（:1148-1167）
        HueSaturationWheelMixin.get_pos_for_color         颜色 → 位置（:1301-1315）
        ColorManager._HUE_DISTORTION_TABLES               表（:80-95，表达式求值）
        ColorAdjusterWidget 的常量                          BORDER_WIDTH 等（:449-461）
    hsvwheel.py
        HSVHueSaturationWheel.get_normalized_polar_pos_for_color（:49-51）
        HSVHueSaturationWheel.color_at_normalized_polar_pos（:53-57）
        HSVHueSaturationWheel.__scroll_cb（:35-47）
    hsvsquare.py
        _HSVSquareOuterRing 的四个方法（:148-192）
        _HSVSquareInnerSquare.__get_faces / get_color_at_position /
            get_position_for_color（:324-412）
    hsvcube.py
        HSVCubePage.tumble（:100-105）
        HSVCubeSlider.get_color_for_bar_amount / get_bar_amount_for_color（:133-142）
        HSVCubeSlice 的 __get_faces / get_color_at_position /
            get_position_for_color（:155-243）
        两个 tooltip 字典（:44-49）
    gui/colors/util.py
        **真 import**（该文件只 import math / lib.pycompat）：
        clamp()（:18-24）与 add_distance_fade_stops()（:27-40）
    gui/colors/uimisc.py
        最小尺寸常量（:21-26，模块级赋值表达式求值）

关于 Python 的名字改写（name mangling）
---------------------------------------
``class C: def f(self): self.__x`` 里的 ``self.__x`` 在**类体内编译**时会被改写成
``self._C__x``。本脚本把函数体抽到**模块级**再编译，改写不再发生，于是替身对象
上用 ``setattr(obj, "__x", ...)`` 挂的字面量 ``__x`` 正好被读到 —— 不需要改动
源码原文里的任何标识符。

输出格式
--------
纯文本、``\\t`` 分隔、``#`` 开头是注释。浮点一律 Python ``repr()``（最短往返），
Rust 解析是正确舍入的，所以可以 ``assert_eq!(to_bits())``。

行类型（第一列）：

    wh_const        <名字> <值>
    wh_rgba         <名字> <r> <g> <b> <a>
    wh_table        <轮类型> <段数> <in0> <in1> <out0> <out1> …（每段 4 个数）
    wh_distort      <轮类型> <h> <h'>
    wh_undistort    <轮类型> <h> <h'>
    wh_geom         <wd> <ht> <border> <radius> <cx> <cy>
    wh_polar        <轮类型> <wd> <ht> <x> <y> <r> <theta>
                    （mixin 的几何输出；钩子只做探针记录，不参与计算）
    wh_color        <kind> <轮类型> <wd> <ht> <x> <y> <h0> <s0> <v0> <h1> <s1> <v1>
                    kind = wheel | ring
    wh_pos          <kind> <轮类型> <wd> <ht> <h0> <s0> <v0> <x> <y>
    wh_scroll       <v0> <方向> <是否落色 0/1> <v1>
    wh_fade_meta    <case> <nstops> <gamma> <alpha> <色标数>
    wh_fade_stop    <case> <序号> <stop> <r> <g> <b> <a>
    wh_slice_color  <page> <faces> <wd> <ht> <x> <y> <h0> <s0> <v0> <h1> <s1> <v1>
                    page = square | cube
    wh_slice_pos    <page> <faces> <wd> <ht> <h0> <s0> <v0> <x> <y>
    wh_faces        <page> <faces> <f1> <f2>
    wh_tumble       <起始 faces> <tumble 次数> <结果 faces>
    wh_slider_amt   <face0> <h> <s> <v> <amt>
    wh_slider_color <face0> <h0> <s0> <v0> <amt> <h1> <s1> <v1>
    wh_tooltip      <face> <滑条提示> <切片提示>
"""

import argparse
import ast
import math
import sys
import textwrap
import types
from pathlib import Path

DEFAULT_REF = r"E:\MyPaint\_mypaint_ref"
HERE = Path(__file__).resolve()
DEFAULT_OUT = HERE.parents[1] / "fixtures" / "wheel_vectors.txt"


# --------------------------------------------------------------------------
# 真 `lib/color.py`（同 gen-color-vectors.py / gen-palette-vectors.py 的做法）
# --------------------------------------------------------------------------


def load_color_module(ref_root):
    """`sys.path.insert(0, <源码根>)` 后 import 真 `lib.color`。"""
    stubbed_gdk = False
    try:
        import gi  # noqa: F401
    except ImportError:
        mod = types.ModuleType("lib.gibindings")
        mod.GdkPixbuf = None
        sys.modules["lib.gibindings"] = mod
        stubbed_gdk = True
    sys.path.insert(0, str(ref_root))
    import lib.color as color  # noqa: E402

    src = Path(color.__file__).resolve()
    want = (Path(ref_root) / "lib" / "color.py").resolve()
    if src != want:
        raise SystemExit(f"导入到的不是源码文件：{src} != {want}")
    print(f"# lib.color 来自 {src}", file=sys.stderr)
    print(f"# lib.gibindings 走 stub: {stubbed_gdk}", file=sys.stderr)
    return color


def load_util_module(ref_root):
    """`gui/colors/util.py` 只依赖 math / lib.pycompat —— 直接 exec 原文。"""
    src_path = (Path(ref_root) / "gui" / "colors" / "util.py").resolve()
    src = src_path.read_text(encoding="utf-8")
    pycompat = types.ModuleType("lib.pycompat")
    pycompat.xrange = range
    ns = {"__name__": "mypaint_gui_colors_util", "math": math}
    saved = sys.modules.get("lib.pycompat")
    sys.modules["lib.pycompat"] = pycompat
    try:
        exec(compile(src, str(src_path), "exec"), ns)
    finally:
        if saved is not None:
            sys.modules["lib.pycompat"] = saved
    if "clamp" not in ns or "add_distance_fade_stops" not in ns:
        raise SystemExit(f"{src_path} 里找不到 clamp / add_distance_fade_stops")
    print(f"# gui/colors/util.py 真 import 自 {src_path}", file=sys.stderr)
    return types.SimpleNamespace(clamp=ns["clamp"],
                                 add_distance_fade_stops=ns["add_distance_fade_stops"])


# --------------------------------------------------------------------------
# ast 抽取
# --------------------------------------------------------------------------

#: 要抽的 (文件, 类名, 方法名)
WANTED = [
    ("adjbases.py", "ColorManager", "distort_hue"),
    ("adjbases.py", "ColorManager", "undistort_hue"),
    ("adjbases.py", "HueSaturationWheelMixin", "get_radius"),
    ("adjbases.py", "HueSaturationWheelMixin", "get_center"),
    ("adjbases.py", "HueSaturationWheelMixin", "get_color_at_position"),
    ("adjbases.py", "HueSaturationWheelMixin", "get_pos_for_color"),
    ("hsvwheel.py", "HSVHueSaturationWheel", "get_normalized_polar_pos_for_color"),
    ("hsvwheel.py", "HSVHueSaturationWheel", "color_at_normalized_polar_pos"),
    ("hsvwheel.py", "HSVHueSaturationWheel", "__scroll_cb"),
    ("hsvsquare.py", "_HSVSquareOuterRing", "get_normalized_polar_pos_for_color"),
    ("hsvsquare.py", "_HSVSquareOuterRing", "color_at_normalized_polar_pos"),
    ("hsvsquare.py", "_HSVSquareOuterRing", "get_color_at_position"),
    ("hsvsquare.py", "_HSVSquareOuterRing", "get_pos_for_color"),
    ("hsvsquare.py", "_HSVSquareInnerSquare", "__get_faces"),
    ("hsvsquare.py", "_HSVSquareInnerSquare", "get_color_at_position"),
    ("hsvsquare.py", "_HSVSquareInnerSquare", "get_position_for_color"),
    ("hsvcube.py", "HSVCubePage", "tumble"),
    ("hsvcube.py", "HSVCubeSlider", "get_color_for_bar_amount"),
    ("hsvcube.py", "HSVCubeSlider", "get_bar_amount_for_color"),
    ("hsvcube.py", "HSVCubeSlice", "__get_faces"),
    ("hsvcube.py", "HSVCubeSlice", "get_color_at_position"),
    ("hsvcube.py", "HSVCubeSlice", "get_position_for_color"),
]


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
    """把函数体原文按"首条到末条语句"的原始行区间整块 dedent（同 B4-2）。"""
    if not body:
        return "pass"
    lines = src.splitlines()
    start = body[0].lineno - 1
    end = body[-1].end_lineno
    return textwrap.dedent("\n".join(lines[start:end]))


def _find_class(tree, name):
    for node in tree.body:
        if isinstance(node, ast.ClassDef) and node.name == name:
            return node
    return None


def _find_func(cls_node, name):
    for sub in cls_node.body:
        if isinstance(sub, ast.FunctionDef) and sub.name == name:
            return sub
    return None


def extract_methods(ref_root, ns):
    """抽出 WANTED 里的方法体，包成模块级函数后 exec 进 `ns`。"""
    cache = {}
    chunks = []
    for filename, cls_name, func_name in WANTED:
        rel = f"gui/colors/{filename}"
        if filename not in cache:
            path = (Path(ref_root) / rel).resolve()
            if not path.is_file():
                raise SystemExit(f"找不到源码：{path}")
            cache[filename] = (path, path.read_text(encoding="utf-8"))
        path, src = cache[filename]
        tree = ast.parse(src, filename=str(path))
        cls_node = _find_class(tree, cls_name)
        if cls_node is None:
            raise SystemExit(f"{path} 里找不到 class {cls_name}")
        node = _find_func(cls_node, func_name)
        if node is None:
            raise SystemExit(f"{path} 里找不到 {cls_name}.{func_name}")
        alias = f"{cls_name}__{func_name}"
        # 去掉前导下划线，避免 alias 出现在 `__all__` 之类的误会上（这里无碍）
        body = _body_source(src, _strip_docstring(node.body))
        wrapper = f"def {alias}({ast.unparse(node.args)}):\n" + textwrap.indent(body, "    ")
        chunks.append(
            f"# ---- {filename}:{node.lineno} {cls_name}.{func_name} ----\n{wrapper}\n"
        )
        print(f"# 抽自 {filename}:{node.lineno} -> {alias}", file=sys.stderr)
    exec(compile("\n".join(chunks), "<wheel-extracted>", "exec"), ns)
    return ns


#: 求值类属性/模块常量时可用的全局名：源码里有 `dict(...)` 与 gettext 的 `_()`。
_ATTR_GLOBALS = {
    "__builtins__": {"dict": dict, "float": float, "int": int, "str": str},
    "_": lambda s: s,
}


def class_attr(tree, cls_name, attr):
    """取类体里的字面量/表达式赋值（`_HUE_DISTORTION_TABLES` 之类）。"""
    cls_node = _find_class(tree, cls_name)
    if cls_node is None:
        return None
    for stmt in cls_node.body:
        if isinstance(stmt, ast.Assign):
            for target in stmt.targets:
                if isinstance(target, ast.Name) and target.id == attr:
                    expr = ast.Expression(stmt.value)
                    ast.fix_missing_locations(expr)
                    return eval(compile(expr, "<attr>", "eval"), dict(_ATTR_GLOBALS))
    return None


def module_attr(tree, attr):
    for stmt in tree.body:
        if isinstance(stmt, ast.Assign):
            for target in stmt.targets:
                if isinstance(target, ast.Name) and target.id == attr:
                    expr = ast.Expression(stmt.value)
                    ast.fix_missing_locations(expr)
                    return eval(compile(expr, "<attr>", "eval"), dict(_ATTR_GLOBALS))
    return None


# --------------------------------------------------------------------------
# 替身对象（把抽出来的函数绑定成"方法"）
# --------------------------------------------------------------------------


class _Alloc:
    def __init__(self, width, height):
        self.width = width
        self.height = height


class _FakeManager:
    """只提供 `distort_hue` / `undistort_hue` 与 `_hue_distorts`。"""

    def __init__(self, ns, tables, wheel_type):
        self._ns = ns
        self._hue_distorts = tables[wheel_type]

    def distort_hue(self, h):
        return self._ns["ColorManager__distort_hue"](self, h)

    def undistort_hue(self, h):
        return self._ns["ColorManager__undistort_hue"](self, h)


class _Probe:
    """探针钩子的返回值：记下 mixin 交过来的 `(r, theta)`。"""

    def __init__(self, r, theta):
        self.r = r
        self.theta = theta


def make_wheel(ns, width, height, managed, manager, kind, probe=None):
    """色轮替身：`kind` = wheel / ring / probe。"""
    obj = types.SimpleNamespace()
    obj.BORDER_WIDTH = 2
    obj.SAT_GAMMA = ns["SAT_GAMMA"]
    obj.HUE_SLICES = ns["HUE_SLICES"]
    obj.SAT_SLICES = ns["SAT_SLICES"]
    obj._alloc = _Alloc(width, height)
    obj._managed = managed
    obj._manager = manager

    def get_allocation():
        return obj._alloc

    def get_color_manager():
        return obj._manager

    def get_managed_color():
        return obj._managed

    def get_radius(wd=None, ht=None, border=None, alloc=None):
        return ns["HueSaturationWheelMixin__get_radius"](obj, wd, ht, border, alloc)

    def get_center(wd=None, ht=None, alloc=None):
        return ns["HueSaturationWheelMixin__get_center"](obj, wd, ht, alloc)

    obj.get_allocation = get_allocation
    obj.get_color_manager = get_color_manager
    obj.get_managed_color = get_managed_color
    obj.get_radius = get_radius
    obj.get_center = get_center

    if kind == "wheel":
        obj.color_at_normalized_polar_pos = lambda r, theta: ns[
            "HSVHueSaturationWheel__color_at_normalized_polar_pos"
        ](obj, r, theta)
        obj.get_normalized_polar_pos_for_color = lambda col: ns[
            "HSVHueSaturationWheel__get_normalized_polar_pos_for_color"
        ](obj, col)
        obj.get_color_at_position = lambda x, y: ns[
            "HueSaturationWheelMixin__get_color_at_position"
        ](obj, x, y)
        obj.get_pos_for_color = lambda col: ns[
            "HueSaturationWheelMixin__get_pos_for_color"
        ](obj, col)
    elif kind == "ring":
        obj.color_at_normalized_polar_pos = lambda r, theta: ns[
            "_HSVSquareOuterRing__color_at_normalized_polar_pos"
        ](obj, r, theta)
        obj.get_normalized_polar_pos_for_color = lambda col: ns[
            "_HSVSquareOuterRing__get_normalized_polar_pos_for_color"
        ](obj, col)
        obj.get_color_at_position = lambda x, y: ns[
            "_HSVSquareOuterRing__get_color_at_position"
        ](obj, x, y)
        obj.get_pos_for_color = lambda col: ns[
            "_HSVSquareOuterRing__get_pos_for_color"
        ](obj, col)
    elif kind == "probe":
        # 钩子只记录 mixin 算出来的 (r, theta)，不做任何颜色换算
        obj.color_at_normalized_polar_pos = lambda r, theta: _Probe(r, theta)
        obj.get_normalized_polar_pos_for_color = lambda col: probe
        obj.get_color_at_position = lambda x, y: ns[
            "HueSaturationWheelMixin__get_color_at_position"
        ](obj, x, y)
        obj.get_pos_for_color = lambda col: ns[
            "HueSaturationWheelMixin__get_pos_for_color"
        ](obj, col)
    else:
        raise AssertionError(kind)
    return obj


class _FakeCube:
    def __init__(self, faces):
        self._faces = list(faces)


def make_slice(ns, page, width, height, managed, faces):
    """切片替身：`page` = square（内方块）/ cube（立方切片）。"""
    obj = types.SimpleNamespace()
    obj.BORDER_WIDTH = 2
    obj._alloc = _Alloc(width, height)
    obj._managed = managed
    # 抽出的函数体在模块级编译，读的是**字面量** `self.__cube`
    setattr(obj, "__cube", _FakeCube(faces))

    def get_allocation():
        return obj._alloc

    def get_managed_color():
        return obj._managed

    obj.get_allocation = get_allocation
    obj.get_managed_color = get_managed_color

    if page == "square":
        def get_faces():
            return ns["_HSVSquareInnerSquare____get_faces"](obj)

        obj.__get_faces = get_faces
        obj.get_color_at_position = lambda x, y: ns[
            "_HSVSquareInnerSquare__get_color_at_position"
        ](obj, x, y)
        obj.get_position_for_color = lambda col: ns[
            "_HSVSquareInnerSquare__get_position_for_color"
        ](obj, col)
    elif page == "cube":
        def get_faces():
            return ns["HSVCubeSlice____get_faces"](obj)

        obj.__get_faces = get_faces
        obj.get_color_at_position = lambda x, y: ns[
            "HSVCubeSlice__get_color_at_position"
        ](obj, x, y)
        obj.get_position_for_color = lambda col: ns[
            "HSVCubeSlice__get_position_for_color"
        ](obj, col)
    else:
        raise AssertionError(page)
    return obj


class _FakeCubePage:
    def __init__(self):
        self._faces = ["h", "s", "v"]
        # 抽出的 tumble() 在模块级编译，`self.__slider` 是字面量
        setattr(self, "__slider", types.SimpleNamespace(queue_draw=lambda: None))
        setattr(self, "__slice", types.SimpleNamespace(queue_draw=lambda: None))
        self._update_tooltips = lambda: None


class _FakeSliderCube:
    def __init__(self, faces):
        self._faces = list(faces)


# --------------------------------------------------------------------------
# fixture 文本
# --------------------------------------------------------------------------

LINES = []
COUNTS = {}


def fmt(x):
    if isinstance(x, bool):
        return "1" if x else "0"
    if isinstance(x, float):
        return repr(x)
    if x is None:
        return "-"
    return str(x)


def emit(kind, *parts):
    COUNTS[kind] = COUNTS.get(kind, 0) + 1
    LINES.append("\t".join([kind] + [fmt(p) for p in parts]))


def hsv_fields(col):
    h, s, v = col.get_hsv()
    return (h, s, v)


# --------------------------------------------------------------------------
# 主流程
# --------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description="生成色轮/色方块/色立方向量 fixture")
    ap.add_argument("--ref", default=DEFAULT_REF)
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    ref = Path(args.ref)
    for rel in ("gui/colors/adjbases.py", "gui/colors/hsvwheel.py",
                "gui/colors/hsvsquare.py", "gui/colors/hsvcube.py",
                "gui/colors/util.py", "gui/colors/uimisc.py"):
        if not (ref / rel).is_file():
            raise SystemExit(f"找不到源码：{ref / rel}")

    color_mod = load_color_module(ref)
    util = load_util_module(ref)
    HSVColor = color_mod.HSVColor

    # 抽函数体前先把它们依赖的全局名放进命名空间
    ns = {"math": math, "clamp": util.clamp, "HSVColor": HSVColor,
          "RGBColor": color_mod.RGBColor, "xrange": range}
    class _ScrollDirection:
        UP = 0
        DOWN = 1
        LEFT = 2
        RIGHT = 3
        SMOOTH = 4

    ns["Gdk"] = types.SimpleNamespace(ScrollDirection=_ScrollDirection)
    extract_methods(ref, ns)

    # ---- 常量 / 表 / 提示字典（类属性与模块常量，表达式求值）----
    adj_src = (ref / "gui/colors/adjbases.py").read_text(encoding="utf-8")
    adj_tree = ast.parse(adj_src)
    sq_src = (ref / "gui/colors/hsvsquare.py").read_text(encoding="utf-8")
    sq_tree = ast.parse(sq_src)
    cu_src = (ref / "gui/colors/hsvcube.py").read_text(encoding="utf-8")
    cu_tree = ast.parse(cu_src)
    ui_src = (ref / "gui/colors/uimisc.py").read_text(encoding="utf-8")
    ui_tree = ast.parse(ui_src)

    tables = class_attr(adj_tree, "ColorManager", "_HUE_DISTORTION_TABLES")
    if tables is None:
        raise SystemExit("抽不到 ColorManager._HUE_DISTORTION_TABLES")
    default_wheel_type = class_attr(adj_tree, "ColorManager", "_DEFAULT_WHEEL_TYPE")
    prefs_key = module_attr(adj_tree, "PREFS_KEY_WHEEL_TYPE")
    hue_slices = class_attr(adj_tree, "HueSaturationWheelMixin", "HUE_SLICES")
    sat_slices = class_attr(adj_tree, "HueSaturationWheelMixin", "SAT_SLICES")
    sat_gamma = class_attr(adj_tree, "HueSaturationWheelMixin", "SAT_GAMMA")
    border_width = class_attr(adj_tree, "ColorAdjusterWidget", "BORDER_WIDTH")
    scroll_delta = class_attr(adj_tree, "ColorAdjusterWidget", "SCROLL_DELTA")
    outline_width = class_attr(adj_tree, "ColorAdjusterWidget", "OUTLINE_WIDTH")
    outline_rgba = class_attr(adj_tree, "ColorAdjusterWidget", "OUTLINE_RGBA")
    edge_width = class_attr(adj_tree, "ColorAdjusterWidget", "EDGE_HIGHLIGHT_WIDTH")
    edge_rgba = class_attr(adj_tree, "ColorAdjusterWidget", "EDGE_HIGHLIGHT_RGBA")
    ring_samples = class_attr(sq_tree, "_HSVSquareOuterRing", "samples")
    cube_slider_samples = class_attr(cu_tree, "HSVCubeSlider", "samples")
    slider_tips = class_attr(cu_tree, "HSVCubePage", "_slider_tooltip_map")
    slice_tips = class_attr(cu_tree, "HSVCubePage", "_slice_tooltip_map")
    min_w = module_attr(ui_tree, "PRIMARY_ADJUSTERS_MIN_WIDTH")
    min_h = module_attr(ui_tree, "PRIMARY_ADJUSTERS_MIN_HEIGHT")
    slider_min_w = module_attr(ui_tree, "SLIDER_MIN_WIDTH")
    slider_min_l = module_attr(ui_tree, "SLIDER_MIN_LENGTH")

    ns["HUE_SLICES"] = hue_slices
    ns["SAT_SLICES"] = sat_slices
    ns["SAT_GAMMA"] = sat_gamma

    emit("wh_const", "HUE_SLICES", hue_slices)
    emit("wh_const", "SAT_SLICES", sat_slices)
    emit("wh_const", "SAT_GAMMA", float(sat_gamma))
    emit("wh_const", "BORDER_WIDTH", float(border_width))
    emit("wh_const", "SCROLL_DELTA", float(scroll_delta))
    emit("wh_const", "OUTLINE_WIDTH", float(outline_width))
    emit("wh_const", "EDGE_HIGHLIGHT_WIDTH", float(edge_width))
    emit("wh_const", "PRIMARY_ADJUSTERS_MIN_WIDTH", float(min_w))
    emit("wh_const", "PRIMARY_ADJUSTERS_MIN_HEIGHT", float(min_h))
    emit("wh_const", "SLIDER_MIN_WIDTH", float(slider_min_w))
    emit("wh_const", "SLIDER_MIN_LENGTH", float(slider_min_l))
    emit("wh_const", "RING_SAMPLES", int(ring_samples))
    emit("wh_const", "CUBE_SLIDER_SAMPLES", int(cube_slider_samples))
    emit("wh_const", "DEFAULT_WHEEL_TYPE", default_wheel_type)
    emit("wh_const", "PREFS_KEY_WHEEL_TYPE", prefs_key)
    emit("wh_rgba", "OUTLINE_RGBA", *[float(c) for c in outline_rgba])
    emit("wh_rgba", "EDGE_HIGHLIGHT_RGBA", *[float(c) for c in edge_rgba])

    # ---- 畸变表 ----
    for wheel_type, table in tables.items():
        if table is None:
            emit("wh_table", wheel_type, 0)
            continue
        parts = [wheel_type, len(table)]
        for (in0, in1), (out0, out1) in table:
            parts.extend([float(in0), float(in1), float(out0), float(out1)])
        emit("wh_table", *parts)

    hs = [0.0, 1e-9, 0.02, 1.0 / 12.0, 1.0 / 6.0, 0.2, 1.0 / 3.0, 0.4, 0.5,
          2.0 / 3.0, 0.75, 0.9, 0.999999, 1.0, -0.5, -5.0 / 6.0, 1.25, 2.5]
    for wheel_type in ("rgb", "ryb", "rygb"):
        mgr = _FakeManager(ns, tables, wheel_type)
        for h in hs:
            emit("wh_distort", wheel_type, float(h), float(mgr.distort_hue(h)))
            emit("wh_undistort", wheel_type, float(h), float(mgr.undistort_hue(h)))

    # ---- 几何（半径 / 圆心 / 极坐标）----
    sizes = [(100.0, 100.0, 2.0), (200.0, 140.0, 2.0), (99.0, 99.0, 2.0),
             (8.0, 8.0, 2.0), (3.0, 3.0, 2.0), (100.0, 100.0, 0.0),
             (50.0, 80.0, 3.0), (1000.0, 600.0, 2.0)]
    managed = HSVColor(0.4, 0.3, 0.7)
    for wd, ht, border in sizes:
        probe_wheel = make_wheel(ns, wd, ht, managed, None, "probe",
                                probe=(0.0, 0.0))
        radius = probe_wheel.get_radius(wd, ht, border)
        cx, cy = probe_wheel.get_center(wd, ht)
        emit("wh_geom", float(wd), float(ht), float(border), float(radius),
             float(cx), float(cy))

    points = [(0.0, 0.0), (1.0, 1.0), (50.0, 50.0), (50.0, 2.0),
              (50.0, 98.0), (2.0, 50.0), (98.0, 50.0), (100.0, 100.0),
              (-10.0, 50.0), (50.0, -10.0), (150.0, 50.0), (50.0, 150.0),
              (51.0, 51.0), (99.0, 20.0), (20.0, 99.0)]
    polar_sizes = [(100.0, 100.0, 2.0), (200.0, 140.0, 2.0)]
    for wd, ht, border in polar_sizes:
        for wheel_type in ("rgb", "rygb"):
            mgr = _FakeManager(ns, tables, wheel_type)
            # 探针的 get_normalized_polar_pos_for_color 给固定 (0.5, 0.25)，
            # 这样 get_color_at_position 只考几何 + 逆畸变
            for x, y in points:
                wheel = make_wheel(ns, wd, ht, managed, mgr, "probe",
                                   probe=(0.5, 0.25))
                got = wheel.get_color_at_position(x, y)
                emit("wh_polar", wheel_type, float(wd), float(ht),
                     float(x), float(y), float(got.r), float(got.theta))

    # ---- 色轮 / 色环：取色与标记位置 ----
    managed_colors = [
        HSVColor(0.0, 0.0, 0.0), HSVColor(0.0, 0.0, 1.0),
        HSVColor(0.0, 1.0, 1.0), HSVColor(0.25, 0.5, 0.75),
        HSVColor(0.4, 0.2, 0.6), HSVColor(0.75, 1.0, 0.3),
        HSVColor(1.0, 0.5, 0.5), HSVColor(0.9, 0.05, 0.02),
    ]
    wheel_sizes = [(100.0, 100.0), (200.0, 140.0), (99.0, 77.0)]
    for kind in ("wheel", "ring"):
        for wd, ht in wheel_sizes:
            for wheel_type in ("rgb", "rygb"):
                mgr = _FakeManager(ns, tables, wheel_type)
                for managed in managed_colors:
                    for x, y in points:
                        wheel = make_wheel(ns, wd, ht, managed, mgr, kind)
                        col = wheel.get_color_at_position(x, y)
                        h1, s1, v1 = hsv_fields(col)
                        emit("wh_color", kind, wheel_type, float(wd), float(ht),
                             float(x), float(y), float(managed.h),
                             float(managed.s), float(managed.v),
                             float(h1), float(s1), float(v1))
        for wd, ht in wheel_sizes:
            for wheel_type in ("rgb", "rygb"):
                mgr = _FakeManager(ns, tables, wheel_type)
                for managed in managed_colors:
                    for color in managed_colors:
                        wheel = make_wheel(ns, wd, ht, managed, mgr, kind)
                        x, y = wheel.get_pos_for_color(color)
                        emit("wh_pos", kind, wheel_type, float(wd), float(ht),
                             float(color.h), float(color.s), float(color.v),
                             float(x), float(y))

    # ---- 轮上滚轮（`__scroll_cb`）----
    directions = ["UP", "DOWN", "LEFT", "RIGHT", "SMOOTH"]
    for v0 in (0.0, 0.5, 1.0, 0.985, 0.01):
        for dir_name in directions:
            fake = types.SimpleNamespace()
            fake.SCROLL_DELTA = scroll_delta
            fake._managed = HSVColor(0.3, 0.4, v0)
            recorded = {"set": False, "color": None}

            def get_managed_color():
                return fake._managed

            def set_managed_color(col):
                recorded["set"] = True
                recorded["color"] = col

            fake.get_managed_color = get_managed_color
            fake.set_managed_color = set_managed_color
            event = types.SimpleNamespace(
                direction=getattr(_ScrollDirection, dir_name))
            ns["HSVHueSaturationWheel____scroll_cb"](fake, None, event)
            v1 = recorded["color"].v if recorded["set"] else v0
            emit("wh_scroll", float(v0), dir_name, 1 if recorded["set"] else 0,
                 float(v1))

    # ---- 径向去饱和渐变色标（真 util.add_distance_fade_stops）----
    fade_cases = [
        (5, 1.0 / 1.5, 1.0, (0.5, 0.5, 0.5)),
        (5, 1.0 / 1.5, 1.0, (0.2, 0.6, 0.9)),
        (0, 2.0, 1.0, (1.0, 0.0, 0.0)),
        (3, 0.5, 0.25, (0.1, 0.2, 0.3)),
    ]
    for case, (nstops, gamma, alpha, rgb) in enumerate(fade_cases):
        class _Gradient:
            def __init__(self):
                self.stops = []

            def add_color_stop_rgba(self, stop, r, g, b, a):
                self.stops.append((stop, r, g, b, a))

        gr = _Gradient()
        util.add_distance_fade_stops(gr, rgb, nstops=nstops, gamma=gamma,
                                     alpha=alpha)
        emit("wh_fade_meta", case, int(nstops), float(gamma), float(alpha),
             len(gr.stops))
        for i, (stop, r, g, b, a) in enumerate(gr.stops):
            emit("wh_fade_stop", case, i, float(stop), float(r), float(g),
                 float(b), float(a))

    # ---- 切片（方块内块 / 立方切片）：坐标 ↔ HSV ----
    all_faces = [
        "hsv", "hvs", "shv", "svh", "vhs", "vsh",
    ]
    slice_sizes = [(100.0, 100.0), (200.0, 140.0), (57.0, 91.0)]
    slice_points = [(2.0, 2.0), (98.0, 98.0), (50.0, 50.0), (2.0, 98.0),
                    (0.0, 0.0), (49.0, 17.0), (120.0, 120.0), (-5.0, -5.0)]
    for page in ("square", "cube"):
        for wd, ht in slice_sizes:
            for faces in all_faces:
                for managed in managed_colors[:4]:
                    for x, y in slice_points:
                        sl = make_slice(ns, page, wd, ht, managed, faces)
                        col = sl.get_color_at_position(x, y)
                        h1, s1, v1 = hsv_fields(col)
                        emit("wh_slice_color", page, faces, float(wd),
                             float(ht), float(x), float(y), float(managed.h),
                             float(managed.s), float(managed.v), float(h1),
                             float(s1), float(v1))
                for managed in managed_colors[:4]:
                    for color in managed_colors[:4]:
                        sl = make_slice(ns, page, wd, ht, managed, faces)
                        x, y = sl.get_position_for_color(color)
                        emit("wh_slice_pos", page, faces, float(wd), float(ht),
                             float(color.h), float(color.s), float(color.v),
                             float(x), float(y))
                # `__get_faces()` 的结果
                sl = make_slice(ns, page, wd, ht, managed_colors[0], faces)
                f1, f2 = getattr(sl, "__get_faces")()
                emit("wh_faces", page, faces, f1, f2)

    # ---- 立方旋转 ----
    for start in ("hsv", "svh", "vhs"):
        page = _FakeCubePage()
        page._faces = list(start)
        emit("wh_tumble", start, 0, "".join(page._faces))
        for step in (1, 2, 3):
            ns["HSVCubePage__tumble"](page)
            emit("wh_tumble", start, step, "".join(page._faces))

    # ---- 深度滑条 ----
    for faces in (["h", "s", "v"], ["s", "v", "h"], ["v", "h", "s"]):
        cube = _FakeSliderCube(faces)
        slider = types.SimpleNamespace()
        slider.BORDER_WIDTH = 2
        slider.get_managed_color = lambda: slider._managed
        # 同 make_slice：抽出的函数体读字面量 `self.__cube`
        setattr(slider, "__cube", cube)
        for managed in managed_colors:
            slider._managed = managed
            amt = ns["HSVCubeSlider__get_bar_amount_for_color"](slider, managed)
            emit("wh_slider_amt", faces[0], float(managed.h), float(managed.s),
                 float(managed.v), float(amt))
            for amount in (0.0, 0.25, 0.5, 1.0, 1.5):
                col = ns["HSVCubeSlider__get_color_for_bar_amount"](slider, amount)
                h1, s1, v1 = hsv_fields(col)
                emit("wh_slider_color", faces[0], float(managed.h),
                     float(managed.s), float(managed.v), float(amount),
                     float(h1), float(s1), float(v1))

    # ---- tooltip 映射 ----
    for face, slider_tip in slider_tips.items():
        emit("wh_tooltip", face, slider_tip, slice_tips[face])

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    header = [
        "# 由 crates/pigment-engine/tests/tools/gen-wheel-vectors.py 生成，请勿手改。",
        f"# 事实来源：{ref} 的 gui/colors/adjbases.py + hsvwheel.py + hsvsquare.py"
        " + hsvcube.py（ast 抽函数体后 exec）+ gui/colors/util.py（真 import）",
        "# 一行一类：见脚本 docstring 的 wh_* 说明。浮点 = Python repr。",
    ]
    out_path.write_text("\n".join(header + LINES) + "\n", encoding="utf-8",
                        newline="\n")

    if not args.quiet:
        total = sum(COUNTS.values())
        print(f"写入 {out_path}")
        print(f"向量总行数 {total}：")
        for k in sorted(COUNTS):
            print(f"  {k:16s} {COUNTS[k]}")


if __name__ == "__main__":
    main()
