#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成色轮 / 色方块 / 色立方 / HCY 轮的向量 fixture —— Rust 侧逐位门禁用。

B4-4b：HSV 三页（``wh_*`` 行）；B4-4c：HCY 轮 + 色域遮罩（``hcy_*`` 行）。

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
``hsvcube.py`` / ``hcywheel.py`` / ``sliders.py``）顶部就 import GTK / cairo，
**不能 import**。所以用 B4-2 / B4-4a 的老办法：``ast`` 把目标方法的**函数体
原文**抽出来，包成模块级的普通函数后 ``exec``，再用"最小替身"对象提供
``self`` 上真正被读到的属性/方法。

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
    hcywheel.py（B4-4c）
        MaskableWheelMixin.get_mask_voids / colors_to_mask_void（:180-204）
        MaskableWheelMixin.get_color_at_position                   遮罩取色（:206-238）
        MaskableWheelMixin._get_void_size                          尺寸（:240-244）
        MaskableWheelMixin.draw_mask                               绘制与尺寸过滤（:260-312）
        HCYHueChromaWheelMixin 的两个钩子（:328-336）
        HCYHueChromaWheel.__scroll_cb（:356-367）
        HCYMaskEditorWheel.get_managed_color / set_managed_color（:811-822）
        HCYMaskTemplateDialog.__templates                          五个 mood 预设（:868-988）
    sliders.py（B4-4c）
        HCYLumaSlider.samples 属性（:287-292）
    lib/alg.py
        **真 import**（该文件只 import math）：convex_hull / poly_area /
        point_in_convex_poly / nearest_point_in_segment / pairwise（:25-596）

关于 Python 的名字改写（name mangling）
---------------------------------------
``class C: def f(self): self.__x`` 里的 ``self.__x`` 在**类体内编译**时会被改写成
``self._C__x``。本脚本把函数体抽到**模块级**再编译，改写不再发生，于是替身对象
上用 ``setattr(obj, "__x", ...)`` 挂的字面量 ``__x`` 正好被读到 —— 不需要改动
源码原文里的任何标识符。

``HCYMaskEditorWheel.get_managed_color`` 里的 ``super(HCYMaskEditorWheel, self)``
是显式两参形式，模块级编译后仍然可用：脚本在命名空间里放一个同名基类，
替身对象是它的实例，``super()`` 顺着 MRO 找到基类的普通方法。

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

B4-4c 追加：

    hcy_const          <名字> <值>
    hcy_template       <序号> <名字> <说明> <形状数>
    hcy_template_point <序号> <形状> <点> <h> <c> <y>          （未畸变的原始值）
    hcy_hull           <点数 n> <in 点…> <出点数 m> <出点…>
    hcy_pip            <多边形点数 n> <多边形点…> <x> <y> <0/1>
    hcy_nps            <x1> <y1> <x2> <y2> <x> <y> <有无 0/1> [<ix> <iy>]
    hcy_area           <点数 n> <点…> <area>
    hcy_void_size      <点数 n> <点…> <size>
    hcy_color          <轮类型> <wd> <ht> <x> <y> <h0> <c0> <y0> <h1> <c1> <y1>
    hcy_pos            <轮类型> <wd> <ht> <h0> <c0> <y0> <x> <y>
    hcy_scroll         <y0> <方向> <是否落色 0/1> <y1>
    hcy_luma           <y_in> <y_out>
    hcy_samples        <长度> <边框> <段数>
    hcy_mask           <case> <轮类型> <wd> <ht> <形状数> [<颜色数> <h> <c> <y>…]
    hcy_mask_voids     <case> <void 数> [<点数> <x> <y>…]     （get_mask_voids）
    hcy_mask_draw      <case> <画出几个> <alpha> <线宽> [<点数> <x> <y>…]（draw_mask）
    hcy_masked         <case> <x> <y> <h0> <c0> <y0> <h1> <c1> <y1>
"""

import argparse
import ast
import copy
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


def load_alg_module(ref_root):
    """`lib/alg.py` 只 import `math.sqrt` —— 直接 import 真源码。"""
    try:
        import lib.alg as alg  # noqa: F401
    except ImportError:
        sys.path.insert(0, str(ref_root))
        import lib.alg as alg  # noqa: F401

    src = Path(alg.__file__).resolve()
    want = (Path(ref_root) / "lib" / "alg.py").resolve()
    if src != want:
        raise SystemExit(f"导入到的不是源码文件：{src} != {want}")
    print(f"# lib.alg 真 import 自 {src}", file=sys.stderr)
    return alg


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
    # ---- B4-4c：HCY 轮与色域遮罩 ----
    ("hcywheel.py", "MaskableWheelMixin", "get_mask_voids"),
    ("hcywheel.py", "MaskableWheelMixin", "colors_to_mask_void"),
    ("hcywheel.py", "MaskableWheelMixin", "get_color_at_position"),
    ("hcywheel.py", "MaskableWheelMixin", "_get_void_size"),
    ("hcywheel.py", "MaskableWheelMixin", "draw_mask"),
    ("hcywheel.py", "HCYHueChromaWheelMixin", "get_normalized_polar_pos_for_color"),
    ("hcywheel.py", "HCYHueChromaWheelMixin", "color_at_normalized_polar_pos"),
    ("hcywheel.py", "HCYHueChromaWheel", "__scroll_cb"),
    ("hcywheel.py", "HCYMaskEditorWheel", "get_managed_color"),
    ("hcywheel.py", "HCYMaskEditorWheel", "set_managed_color"),
    ("hcywheel.py", "HCYMaskTemplateDialog", "__templates"),
    ("sliders.py", "HCYLumaSlider", "samples"),
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
    "C_": lambda ctx, s: s,
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
# B4-4c：HCY 轮的替身对象
# --------------------------------------------------------------------------


class _Toggle:
    """`self.mask_toggle` 的最小替身（`hcywheel.py:71`、`:215`）。"""

    def __init__(self, active):
        self._active = active

    def get_active(self):
        return self._active

    def set_active(self, active):
        self._active = bool(active)


class _RecordingCairo:
    """记录 cairo 调用序列的替身 —— 用来从 `draw_mask()` 反推它画了什么。"""

    def __init__(self):
        self.calls = []

    def __getattr__(self, name):
        if name.startswith("__"):
            raise AttributeError(name)

        def record(*a, **kw):
            self.calls.append((name, a, kw))

        return record

    def polygons(self):
        """把 `new_sub_path/move_to/line_to` 的序列还原成多边形列表。"""
        out = []
        cur = None
        for name, a, _kw in self.calls:
            if name == "new_sub_path":
                cur = []
                out.append(cur)
            elif name in ("move_to", "line_to") and cur is not None:
                cur.append((float(a[0]), float(a[1])))
        return out

    def first(self, name):
        for n, a, _kw in self.calls:
            if n == name:
                return a
        return None


def make_hcy_wheel(ns, width, height, managed, manager, mask=None, active=False):
    """HCY 轮替身：钩子走源码的 `HCYHueChromaWheelMixin`，遮罩走 mixin。"""
    obj = types.SimpleNamespace()
    obj.BORDER_WIDTH = 2
    obj.SAT_GAMMA = ns["SAT_GAMMA"]
    obj.HUE_SLICES = ns["HUE_SLICES"]
    obj.SAT_SLICES = ns["SAT_SLICES"]
    obj.min_shape_size = ns["MIN_SHAPE_SIZE"]
    obj._alloc = _Alloc(width, height)
    obj._managed = managed
    obj._manager = manager
    obj.mask_toggle = _Toggle(active)
    setattr(obj, "__mask", mask)

    obj.get_allocation = lambda: obj._alloc
    obj.get_color_manager = lambda: obj._manager
    obj.get_managed_color = lambda: obj._managed

    def get_radius(wd=None, ht=None, border=None, alloc=None):
        return ns["HueSaturationWheelMixin__get_radius"](obj, wd, ht, border, alloc)

    def get_center(wd=None, ht=None, alloc=None):
        return ns["HueSaturationWheelMixin__get_center"](obj, wd, ht, alloc)

    obj.get_radius = get_radius
    obj.get_center = get_center

    obj.color_at_normalized_polar_pos = lambda r, theta: ns[
        "HCYHueChromaWheelMixin__color_at_normalized_polar_pos"
    ](obj, r, theta)
    obj.get_normalized_polar_pos_for_color = lambda col: ns[
        "HCYHueChromaWheelMixin__get_normalized_polar_pos_for_color"
    ](obj, col)
    obj.get_color_at_position_plain = lambda x, y: ns[
        "HueSaturationWheelMixin__get_color_at_position"
    ](obj, x, y)
    obj.get_color_at_position = lambda x, y, ignore_mask=False: ns[
        "MaskableWheelMixin__get_color_at_position"
    ](obj, x, y, ignore_mask)
    obj.get_pos_for_color = lambda col: ns["HueSaturationWheelMixin__get_pos_for_color"](
        obj, col
    )
    obj.get_mask_voids = lambda: ns["MaskableWheelMixin__get_mask_voids"](obj)
    obj.colors_to_mask_void = lambda colors: ns[
        "MaskableWheelMixin__colors_to_mask_void"
    ](obj, colors)
    obj._get_void_size = lambda void: ns["MaskableWheelMixin___get_void_size"](void)
    obj.draw_mask = lambda cr, wd, ht: ns["MaskableWheelMixin__draw_mask"](
        obj, cr, wd, ht
    )
    # `draw_mask()` 要读主题色的两个钩子（`:246-258` 读 GTK style）
    obj._get_mask_bg = lambda: (0.25, 0.25, 0.25)
    obj._get_mask_fg = lambda: (0.9, 0.9, 0.9)
    return obj


def make_editor(ns, managed):
    """`HCYMaskEditorWheel` 替身 —— 只为两个明度夹取方法（`:811-822`）。

    `super(HCYMaskEditorWheel, self)` 是显式两参形式，所以命名空间里的同名类
    必须真的在 MRO 里位于替身之前；这里返回它的实例。
    """

    class _Base:
        def get_managed_color(self):
            return self._managed

        def set_managed_color(self, color):
            self._set = color

    class HCYMaskEditorWheel(_Base):
        pass

    ns["HCYMaskEditorWheel"] = HCYMaskEditorWheel
    obj = HCYMaskEditorWheel()
    obj._managed = managed
    obj._set = None
    # 抽出的函数体在模块级编译，读的是字面量 `self.__MIN_LUMA`
    setattr(obj, "__MIN_LUMA", ns["EDITOR_MIN_LUMA"])
    setattr(obj, "__MAX_LUMA", ns["EDITOR_MAX_LUMA"])
    return obj


def collect_calls(src, cls_name, func_name, call_name):
    """把某个方法体里 `call_name(...)` 的实参求值成列表（按出现顺序）。

    `MaskableWheelMixin.__init__` 里的 `Gtk.ToggleAction(name, C_(标题),
    C_(提示), None)` 就是靠这个抽出来的（`:79-90`）。
    """
    tree = ast.parse(src)
    cls_node = _find_class(tree, cls_name)
    node = _find_func(cls_node, func_name)
    out = []
    for sub in ast.walk(node):
        if not isinstance(sub, ast.Call):
            continue
        fn = sub.func
        name = getattr(fn, "id", None) or getattr(fn, "attr", None)
        if name != call_name:
            continue
        args = []
        for a in sub.args:
            try:
                args.append(ast.literal_eval(a))
            except (ValueError, SyntaxError):
                args.append(None)
        out.append(tuple(args))
    return out


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
# B4-4c：HCY 轮 + 色域遮罩
# --------------------------------------------------------------------------


def _hcy_class_attrs(hcy_tree):
    """`hcywheel.py` 里的类属性/模块常量（表达式求值）。"""
    attrs = {
        "MIN_SHAPE_SIZE": class_attr(hcy_tree, "MaskableWheelMixin", "min_shape_size"),
        "MIN_LUMA": class_attr(hcy_tree, "HCYMaskEditorWheel", "__MIN_LUMA"),
        "MAX_LUMA": class_attr(hcy_tree, "HCYMaskEditorWheel", "__MAX_LUMA"),
        "CTRLPOINT_RADIUS": class_attr(
            hcy_tree, "HCYMaskEditorWheel", "__ctrlpoint_radius"
        ),
        "CTRLPOINT_GRAB_RADIUS": class_attr(
            hcy_tree, "HCYMaskEditorWheel", "__ctrlpoint_grab_radius"
        ),
        "MAX_NUM_SHAPES": class_attr(hcy_tree, "HCYMaskEditorWheel", "__max_num_shapes"),
        "PREFS_MASK_KEY": module_attr(hcy_tree, "PREFS_MASK_KEY"),
        "PREFS_ACTIVE_KEY": module_attr(hcy_tree, "PREFS_ACTIVE_KEY"),
        "WHEEL_TOOLTIP": class_attr(hcy_tree, "HCYHueChromaWheel", "STATIC_TOOLTIP_TEXT"),
        "EDITOR_TOOLTIP": class_attr(
            hcy_tree, "HCYMaskEditorWheel", "STATIC_TOOLTIP_TEXT"
        ),
    }
    missing = [k for k, v in attrs.items() if v is None]
    if missing:
        raise SystemExit(f"hcywheel.py 里抽不到这些常量：{missing}")
    return attrs


def _flatten_voids(voids):
    """`[[(x, y), …], …]` → `[点数, x, y, …]` 的扁平字段表（每个 void 前带点数）。"""
    out = []
    for void in voids:
        out.append(len(void))
        for p in void:
            out.extend([float(p[0]), float(p[1])])
    return out


def _mask_from_template(template, manager):
    """模板 → 遮罩（`HCYColor` 列表）—— `hcywheel.py:1008-1015` 的循环。

    `h` 过 `undistort_hue`，`c`/`y` 原样；与 `HCYMaskTemplateDialog.__init__`
    逐行对应。
    """
    HCYColor = manager._ns["HCYColor"]
    out = []
    for shape_float in template[2]:
        shape = []
        for h, c, y in shape_float:
            shape.append(HCYColor(manager.undistort_hue(h), c, y))
        out.append(shape)
    return out


def emit_hcy(ns, color_mod, alg, hcy_src, sliders_src, templates, attrs, tables):
    """B4-4c 的全部 `hcy_*` 向量。"""
    HCYColor = color_mod.HCYColor
    HSVColor = color_mod.HSVColor

    # ---- 常量 ----
    for name in (
        "MIN_SHAPE_SIZE",
        "MIN_LUMA",
        "MAX_LUMA",
        "CTRLPOINT_RADIUS",
        "CTRLPOINT_GRAB_RADIUS",
        "MAX_NUM_SHAPES",
    ):
        emit("hcy_const", name, float(attrs[name]))
    for name in ("PREFS_MASK_KEY", "PREFS_ACTIVE_KEY", "WHEEL_TOOLTIP", "EDITOR_TOOLTIP"):
        emit("hcy_const", name, attrs[name])
    # `MaskableWheelMixin.__init__` 里内联的两个 C_()（`:79-90`）
    toggle_calls = collect_calls(hcy_src, "MaskableWheelMixin", "__init__", "C_")
    if len(toggle_calls) != 2:
        raise SystemExit(f"MaskableWheelMixin.__init__ 里应有 2 个 C_()，实得 {len(toggle_calls)}")
    emit("hcy_const", "MASK_ACTIVE_LABEL", toggle_calls[0][1])
    emit("hcy_const", "MASK_ACTIVE_TOOLTIP", toggle_calls[1][1])
    # 绘制常量来自 `draw_mask()` 的调用实参（`:303` 的 `set_line_width(2.0)`、
    # `:311` 的 `paint_with_alpha(0.666)`）—— 所以从函数体里抽，不写死
    widths = collect_calls(hcy_src, "MaskableWheelMixin", "draw_mask", "set_line_width")
    alphas = collect_calls(hcy_src, "MaskableWheelMixin", "draw_mask", "paint_with_alpha")
    if len(widths) != 1 or len(alphas) != 1:
        raise SystemExit(
            f"draw_mask() 里的 set_line_width/paint_with_alpha 数量不对：{widths} {alphas}"
        )
    emit("hcy_const", "MASK_EDGE_LINE_WIDTH", float(widths[0][0]))
    emit("hcy_const", "MASK_DRAW_ALPHA", float(alphas[0][0]))

    # ---- 五个 mood 预设（`__templates`，`:868-988`）----
    for idx, (name, desc, shapes) in enumerate(templates):
        emit("hcy_template", idx, name, desc, len(shapes))
        for si, shape in enumerate(shapes):
            for pi, (h, c, y) in enumerate(shape):
                emit("hcy_template_point", idx, si, pi, float(h), float(c), float(y))

    # ---- `lib/alg.py`：凸包 / 点在多边形内 / 垂足 / 面积 ----
    hull_cases = [
        [(1, 1), (1, -1), (0, 0), (-1, -1), (-1, 1)],
        [(0, 0), (4, 0), (4, 4), (0, 4), (2, 2)],
        [(0, 0), (4, 0), (4, 4), (0, 4), (2, 2), (2, 2)],
        [(0, 0), (1, 0), (0, 1)],
        [(0, 0), (2, 0), (1, 3)],
        [(0, 0), (3, 0), (3, 3), (1.5, 4.5), (0, 3)],
        [(0, 0), (100, 0), (100, 100), (0, 100), (50, 50), (10, 90), (90, 10)],
        [(0, 0), (50, 0), (100, 0)],  # 共线
        [(0, 0), (0, 0), (0, 0)],  # 全重合
        [(5, 5)],  # 单点
        [(0, 0), (10, 0), (10, 0), (0, 10)],
        [(-5, -5), (5, -5), (5, 5), (-5, 5), (0, -6), (0, 6)],
    ]
    # 再加几组半像素点（`get_pos_for_color` 的输出就是 x.5）
    hull_cases += [
        [(1.5, 2.5), (10.5, 2.5), (10.5, 11.5), (1.5, 11.5)],
        [(0.5, 0.5), (3.5, 0.5), (3.5, 3.5), (0.5, 3.5), (2.5, 2.5)],
        [(20.0, 10.5), (10.5, 20.0), (1.5, 10.5)],
        [(0, 1), (1, 0), (2, 1), (1, 2)],
        [(0, 0), (6, 0), (6, 6), (3, 3), (0, 6)],
    ]
    for pts in hull_cases:
        hull = alg.convex_hull(pts)
        flat_in = [c for p in pts for c in p]
        flat_out = [c for p in hull for c in p]
        emit("hcy_hull", len(pts), *[float(v) for v in flat_in], len(hull),
             *[float(v) for v in flat_out])

    pip_polys = [
        [(-1, -1), (1, -1), (1, 1), (-1, 1)],
        [(0.5, 0.5), (5.5, 0.5), (3.0, 6.0)],
        [(10.0, 10.5), (100.5, 120.0), (2.5, 90.0)],
        [(1.5, 100.5), (198.5, 100.5), (100.5, 198.5)],
        [(0, 1), (1, 0), (2, 1), (1, 2)],
        [(20.0, 20.0), (80.0, 20.0), (80.0, 80.0), (20.0, 80.0)],
    ]
    pip_points = [
        (0.0, 0.0), (0.0, 1.0), (0.0, 1.1), (2.0, 0.0), (1.0, 1.0),
        (-1.0, -1.0), (0.5, 0.5), (3.0, 3.0), (100.0, 150.0), (100.0, 50.0),
        (0.0, 100.0), (50.0, 100.5), (1.5, 100.5), (99.0, 101.0), (100.5, 198.5),
    ]
    for poly in pip_polys:
        for pt in pip_points:
            flat = [c for p in poly for c in p]
            inside = alg.point_in_convex_poly(pt, poly)
            emit("hcy_pip", len(poly), *[float(v) for v in flat], float(pt[0]),
                 float(pt[1]), inside)

    nps_cases = [
        ((0, 0), (4, 0), (2, 2)),
        ((1, 1), (3, 3), (2, 1)),
        ((0, 0), (3, 0), (0, 1)),
        ((1, 1), (3, 3), (12, -1)),
        ((1, 1), (1, 1), (2, 2)),
        ((1.5, 100.5), (198.5, 100.5), (100.0, 50.0)),
        ((100.5, 198.5), (1.5, 100.5), (0.0, 100.0)),
        ((0, 0), (0, 4), (2, 3)),
        ((0, 0), (4, 0), (6, 5)),
        ((-1, 1), (1, -1), (-3, 1)),
    ]
    for a, b, p in nps_cases:
        got = alg.nearest_point_in_segment(a, b, p)
        parts = [float(a[0]), float(a[1]), float(b[0]), float(b[1]), float(p[0]),
                 float(p[1]), got is not None]
        if got is not None:
            parts += [float(got[0]), float(got[1])]
        emit("hcy_nps", *parts)

    area_polys = [
        [(-1, -1), (1, -1), (1, 1), (-1, 1)],
        [(0, 0), (1, 0), (0, 1)],
        [(0, 0), (2, 0), (1, 3)],
        [(0, 0), (3, 0), (3, 3), (1.5, 4.5), (0, 3)],
        [(0.5, 0.5), (3.5, 0.5), (3.5, 3.5), (0.5, 3.5)],
        [(1.5, 100.5), (198.5, 100.5), (100.5, 198.5)],
        [(0, 1), (1, 0), (2, 1), (1, 2)],
        [(0, 0), (0, 0)],
        [(0, 0), (10, 0), (10, 0), (0, 10)],
        [(100.0, 100.5), (120.0, 140.5), (80.0, 130.0)],
    ]
    for poly in area_polys:
        flat = [c for p in poly for c in p]
        emit("hcy_area", len(poly), *[float(v) for v in flat],
             float(alg.poly_area(poly)))
        emit("hcy_void_size", len(poly), *[float(v) for v in flat],
             float(ns["MaskableWheelMixin___get_void_size"](poly)))

    # ---- 两个 HCY 钩子：位置 ↔ HCY ----
    sizes = [(100.0, 100.0), (200.0, 140.0), (99.0, 77.0), (300.0, 300.0)]
    points = [
        (0.0, 0.0), (1.0, 1.0), (50.0, 50.0), (50.0, 2.0), (50.0, 98.0),
        (2.0, 50.0), (98.0, 50.0), (100.0, 100.0), (-10.0, 50.0),
        (50.0, -10.0), (150.0, 50.0), (50.0, 150.0), (51.0, 51.0),
        (99.0, 20.0), (20.0, 99.0), (100.0, 100.5),
    ]
    hcy_manageds = [
        HCYColor(0.0, 0.0, 0.0),
        HCYColor(0.0, 0.0, 1.0),
        HCYColor(0.25, 0.5, 0.5),
        HCYColor(0.4, 1.0, 0.6),
        HCYColor(0.95, 0.03, 0.5),
        HCYColor(1.0, 2.0, 0.75),
        HCYColor(0.6, 0.0, 0.3),
        HCYColor(0.1, 1.5, 0.1),
    ]
    for wd, ht in sizes:
        for wheel_type in ("rgb", "rygb"):
            mgr = _FakeManager(ns, tables, wheel_type)
            for managed in hcy_manageds:
                for x, y in points:
                    wheel = make_hcy_wheel(ns, wd, ht, managed, mgr)
                    col = wheel.get_color_at_position_plain(x, y)
                    emit("hcy_color", wheel_type, float(wd), float(ht), float(x),
                         float(y), float(managed.h), float(managed.c),
                         float(managed.y), float(col.h), float(col.c),
                         float(col.y))
    pos_colors = [
        HCYColor(0.0, 0.0, 0.5),
        HCYColor(0.25, 1.0, 0.5),
        HCYColor(0.5, 2.0, 0.7),
        HCYColor(0.75, 0.5, 0.25),
        HCYColor(0.95, 0.03, 0.5),
        HCYColor(1.25, 1.0, 0.9),
        HCYColor(-0.1, 0.4, 0.5),
        HCYColor(0.6, 0.0, 0.5),
    ]
    for wd, ht in sizes[:3]:
        for wheel_type in ("rgb", "rygb"):
            mgr = _FakeManager(ns, tables, wheel_type)
            for col in pos_colors:
                wheel = make_hcy_wheel(ns, wd, ht, col, mgr)
                x, y = wheel.get_pos_for_color(col)
                emit("hcy_pos", wheel_type, float(wd), float(ht), float(col.h),
                     float(col.c), float(col.y), float(x), float(y))

    # ---- 滚轮：只改明度（`:356-367`）----
    directions = ["UP", "DOWN", "LEFT", "RIGHT", "SMOOTH"]
    for y0 in (0.0, 0.5, 1.0, 0.985, 0.01):
        for dir_name in directions:
            fake = types.SimpleNamespace()
            fake.SCROLL_DELTA = ns["SCROLL_DELTA"]
            fake._managed = HCYColor(0.3, 0.4, y0)
            recorded = {"set": False, "color": None}

            def get_managed_color():
                return fake._managed

            def set_managed_color(col):
                recorded["set"] = True
                recorded["color"] = col

            fake.get_managed_color = get_managed_color
            fake.set_managed_color = set_managed_color
            event = types.SimpleNamespace(
                direction=getattr(ns["Gdk"].ScrollDirection, dir_name)
            )
            ns["HCYHueChromaWheel____scroll_cb"](fake, None, event)
            y1 = recorded["color"].y if recorded["set"] else y0
            emit("hcy_scroll", float(y0), dir_name, 1 if recorded["set"] else 0,
                 float(y1))

    # ---- 编辑器明度夹取（`:811-822`）----
    for y0 in (-0.5, 0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0, 1.5):
        editor = make_editor(ns, HCYColor(0.3, 0.4, y0))
        got = ns["HCYMaskEditorWheel__get_managed_color"](editor)
        emit("hcy_luma", float(y0), float(got.y))
        editor2 = make_editor(ns, HCYColor(0.3, 0.4, 0.5))
        ns["HCYMaskEditorWheel__set_managed_color"](editor2, HCYColor(0.3, 0.4, y0))
        emit("hcy_luma_set", float(y0), float(editor2._set.y))

    # ---- 明度滑条采样段数（`sliders.py:287-292`）----
    # 属性体里的 `self.vertical and alloc.height or alloc.width` 是 Python 的
    # and/or 短路：竖条但高度为 0.0（假值）时会回落到宽度 —— 这里把 width 与
    # height 都给出来，让源码自己决定走哪一支。
    for length in (0.0, 3.0, 6.0, 50.0, 100.0, 194.0, 200.0, 299.0, 1000.0):
        for border in (2.0, 3.0):
            slider = types.SimpleNamespace()
            slider.vertical = True
            slider.BORDER_WIDTH = border
            slider.get_allocation = lambda: _Alloc(18, length)
            n = ns["HCYLumaSlider__samples"](slider)
            emit("hcy_samples", 18.0, float(length), 1, float(border), int(n))
    # 横条：`vertical` 为假 → 长度取宽度
    for width in (18.0, 50.0, 300.0):
        slider = types.SimpleNamespace()
        slider.vertical = False
        slider.BORDER_WIDTH = 2.0
        slider.get_allocation = lambda: _Alloc(width, 200.0)
        n = ns["HCYLumaSlider__samples"](slider)
        emit("hcy_samples", float(width), 200.0, 0, 2.0, int(n))

    # ---- 遮罩：void / 绘制 / 取色裁剪 ----
    mask_sizes = [(100.0, 100.0), (200.0, 140.0)]
    mask_cases = []
    for tpl_idx, template in enumerate(templates):
        mask_cases.append((f"tpl{tpl_idx}", template, mask_sizes[tpl_idx % 2]))
    # 两个特殊形状：点太少（不参与）与挤在盘心（尺寸太小，不画）
    mask_cases.append((
        "tiny_shape",
        ("tiny", "two colours + a tiny one", [[(0.0, 0.0, 0.5), (0.01, 0.001, 0.5),
                                              (0.02, 0.002, 0.5)]]),
        (200.0, 140.0),
    ))
    mask_cases.append((
        "mixed",
        ("mixed", "one real shape + one two-point shape",
         [templates[3][2][0], [(0.1, 0.2, 0.5), (0.2, 0.3, 0.5)]]),
        (200.0, 140.0),
    ))
    mask_points = [
        (0.0, 0.0), (1.0, 1.0), (50.0, 50.0), (50.0, 2.0), (50.0, 98.0),
        (2.0, 50.0), (98.0, 50.0), (100.0, 100.0), (150.0, 50.0),
        (50.0, 150.0), (51.0, 51.0), (99.0, 20.0), (20.0, 99.0),
        (100.0, 150.0), (100.0, 50.0), (0.0, 100.0), (30.0, 30.0),
        (70.0, 20.0), (120.0, 120.0), (10.0, 70.0), (45.5, 30.5),
        (95.0, 35.0), (55.0, 82.0), (60.0, 60.0),
    ]
    for case, template, (wd, ht) in mask_cases:
        for wheel_type in ("rgb", "rygb"):
            mgr = _FakeManager(ns, tables, wheel_type)
            mask = _mask_from_template(template, mgr)
            tag = f"{case}_{wheel_type}"
            emit("hcy_mask", tag, wheel_type, float(wd), float(ht), len(mask),
                 *[v for shape in mask
                   for v in ([len(shape)] + [c for col in shape
                                             for c in (col.h, col.c, col.y)])])
            wheel = make_hcy_wheel(ns, wd, ht, HCYColor(0.0, 0.0, 0.5), mgr,
                                   mask=mask, active=True)
            voids = wheel.get_mask_voids()
            emit("hcy_mask_voids", tag, len(voids), *_flatten_voids(voids))
            cr = _RecordingCairo()
            wheel.draw_mask(cr, wd, ht)
            drawn = cr.polygons()
            emit("hcy_mask_draw", tag, len(drawn), *_flatten_voids(drawn))
            for x, y in mask_points:
                got = wheel.get_color_at_position(x, y)
                emit("hcy_masked", tag, float(x), float(y), float(got.h),
                     float(got.c), float(got.y))


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
                "gui/colors/hcywheel.py", "gui/colors/sliders.py",
                "gui/colors/util.py", "gui/colors/uimisc.py"):
        if not (ref / rel).is_file():
            raise SystemExit(f"找不到源码：{ref / rel}")

    color_mod = load_color_module(ref)
    util = load_util_module(ref)
    alg = load_alg_module(ref)
    HSVColor = color_mod.HSVColor
    HCYColor = color_mod.HCYColor

    # 抽函数体前先把它们依赖的全局名放进命名空间
    ns = {"math": math, "clamp": util.clamp, "HSVColor": HSVColor,
          "HCYColor": HCYColor, "RGBColor": color_mod.RGBColor,
          "xrange": range, "geom": alg, "deepcopy": copy.deepcopy,
          "C_": lambda ctx, s: s,
          "cairo": types.SimpleNamespace(OPERATOR_OVER=0, OPERATOR_SOURCE=1)}
    # 遮罩 mixin 的 `get_color_at_position()` 里 `sup = HueSaturationWheelMixin`
    # 然后 `sup.get_color_at_position(self, x, y)` —— 用命名空间里的同名钩子接住。
    ns["HueSaturationWheelMixin"] = types.SimpleNamespace(
        get_color_at_position=lambda obj, x, y: ns[
            "HueSaturationWheelMixin__get_color_at_position"
        ](obj, x, y)
    )
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
    ns["SCROLL_DELTA"] = scroll_delta

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

    # ---- B4-4c：HCY 轮 + 色域遮罩 ----
    hy_src = (ref / "gui/colors/hcywheel.py").read_text(encoding="utf-8")
    hy_tree = ast.parse(hy_src)
    sl_src = (ref / "gui/colors/sliders.py").read_text(encoding="utf-8")
    sl_tree = ast.parse(sl_src)
    attrs = _hcy_class_attrs(hy_tree)
    ns["MIN_SHAPE_SIZE"] = attrs["MIN_SHAPE_SIZE"]
    ns["EDITOR_MIN_LUMA"] = attrs["MIN_LUMA"]
    ns["EDITOR_MAX_LUMA"] = attrs["MAX_LUMA"]
    templates = ns["HCYMaskTemplateDialog____templates"](None)
    if len(templates) != 5:
        raise SystemExit(f"__templates 应有 5 个预设，实得 {len(templates)}")
    # 明度滑条的采样段数走 `HCYLumaSlider.samples` 属性体；顺手确认源文件里
    # 真有这个属性（防止 ast 抽到别的东西）
    if _find_func(_find_class(sl_tree, "HCYLumaSlider"), "samples") is None:
        raise SystemExit("sliders.py 里找不到 HCYLumaSlider.samples")
    emit_hcy(ns, color_mod, alg, hy_src, sl_src, templates, attrs, tables)

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    header = [
        "# 由 crates/pigment-engine/tests/tools/gen-wheel-vectors.py 生成，请勿手改。",
        f"# 事实来源：{ref} 的 gui/colors/adjbases.py + hsvwheel.py + hsvsquare.py"
        " + hsvcube.py + hcywheel.py + sliders.py（ast 抽函数体后 exec）"
        "+ gui/colors/util.py + lib/alg.py（真 import）",
        "# 一行一类：见脚本 docstring 的 wh_* / hcy_* 说明。浮点 = Python repr。",
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
