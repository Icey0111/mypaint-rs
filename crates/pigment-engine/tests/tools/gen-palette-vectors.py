#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成调色板模型的向量 fixture —— Rust 侧逐位门禁用（B4-4a）。

用法
----
在仓库根（``E:\\MyPaint\\pigment``）下跑：

    python crates/pigment-engine/tests/tools/gen-palette-vectors.py

可选参数：

    --ref     <源码根>   默认 E:\\MyPaint\\_mypaint_ref
    --out     <输出文件> 默认 <本脚本目录>/../fixtures/palette_vectors.txt
                         （即 crates/pigment-engine/tests/fixtures/palette_vectors.txt，
                         由 pigment-engine 的 `tests/color_palette.rs` 消费）
    --out-pv  <输出文件> 默认 crates/pigment-app/tests/fixtures/paletteview_vectors.txt
                         （`gui/colors/paletteview.py` 的布局向量，由 pigment-app 的
                         `shell/palette_panel.rs` 测试消费）
    --quiet              不打印各类向量的条数

脚本可重复跑，输出确定（无随机数）。

事实来源与"真源码"的边界
------------------------
模型（``lib/palette.py``）**可以真 import**：它自己只 import ``re`` / ``copy`` /
``logging`` / ``io`` 与几个 ``lib.*`` 模块。挡路的是 ``lib.helpers`` ——
它顶部就 ``import mypaintlib``（C 扩展）、``GdkPixbuf``、``lib.pixbuf`` …，
本机没有。于是：

- ``lib.gibindings`` → 换成空模块（B4-1 的 ``gen-color-vectors.py`` 同一做法，
  只是为了让 ``lib.color`` 能 import）；
- ``lib.helpers`` → 换成一个只装**真 `clamp()`** 的模块：``clamp`` 用
  ``ast`` 从 ``lib/helpers.py:244-249`` 的原文抽出来 ``exec``，不是照抄。

除此之外 ``import lib.palette`` 走的是**源码本体**：被测的 `Palette` 类、
`_outwards_from()`、`_color_distance()` 全是源码那几行，一条都没有重写。

控件层（``gui/colors/paletteview.py``）**不能 import**（顶部就 import GTK/
cairo）。所以那一份用 B4-2 的老办法：``ast`` 把 `_PaletteGridLayout` 里七个
纯数学方法的函数体抽出来 ``exec``，再喂一个最小替身 ``_FakeGrid``
（只提供 ``get_allocation()`` / ``get_color_manager()`` 与三个缓存的布局属性）。

输出格式
--------
纯文本、``\\t`` 分隔、``#`` 开头是注释。浮点一律 Python ``repr()``
（最短往返表示）—— Rust 的 ``str::parse::<f64>()`` 是正确舍入的，所以
Rust 解析出的位模式与 Python 侧完全相同，可以 ``assert_eq!(to_bits())``。

模型 fixture 的行类型（第一列）：

    pal_meta        <pal_id> <名字|-> <columns> <len>
    pal_entry       <pal_id> <i> <empty 0/1> <r> <g> <b> <hasname 0/1> <名字|->
    pal_load_ok     <pal_id> <0/1>           载入是否成功（0 = 抛了异常）
    pal_load_text   <pal_id> <行号> <原文行>  `.gpl` 输入（`\\` 与 `\\t` 已转义）
    pal_dump        <pal_id> <行号> <整行文本>
                    unicode(palette) 的逐行（含末尾的空行）
    pal_repr        <pal_id> <repr 文本>
    pal_simpledict  <pal_id> <columns> <hasname> <名字|-> <条目数>
    pal_simpledict_entry <pal_id> <i> <empty> <hex|-> <hasname> <名字|->
    pal_distance    <a 类型> a0 a1 a2 <b 类型> b0 b1 b2 <距离>
    pal_outwards    <n> <i> <下标…>          _outwards_from() 的输出序列
    pal_match       <pal_id> <预置位置|-> <有 order 0/1> <order 下标逗号串|->
                    <exact 0/1> <颜色类型> c0 c1 c2 <返回 0/1> <命中位置|->
                    <approx 0/1>
    pal_seq         <seq_id> <pal_id>        一个操作序列作用在这个调色板上
    pal_step        <seq_id> <step> <op> <参数…>
    pal_state       <seq_id> <step> <len> <命中位置|-> <approx 0/1>
    pal_state_entry <seq_id> <step> <i> <empty> <r> <g> <b> <hasname> <名字|->

``gui/colors/paletteview.py`` 的布局 fixture 行类型：

    pv_constrain    <输入> <边长>            _constrain_swatch_size()
    pv_dims         <columns> <ncolors> <nrows|-> <ncolumns|->
    pv_nat_h        <columns> <ncolors> <宽度> <自然高度>
    pv_size_alloc   <columns> <ncolors> <宽> <高> <行数> <列数> <边长>
    pv_offset       <行> <列> <边长> <宽> <高> <dx> <dy>
    pv_pos          <行> <列> <边长> <宽> <高> <下标> <x> <y>
    pv_index        <行> <列> <边长> <宽> <高> <ncolors> <x> <y>
                    <nearest 0/1> <insert 0/1> <下标|->


``pal_id`` 有两种来源：``*.gpl`` 文件的文件茎（真 `Palette(filename=…)` 载入），
以及脚本里手工构造的调色板（`greys16` / `red2blue5` / `empty_mix` / `dups`
/ `black_only`）。``pal_step`` 的 ``op`` 与参数在 Rust 侧按同一份表回放。
"""

import argparse
import ast
import sys
import textwrap
import types
from pathlib import Path

DEFAULT_REF = r"E:\MyPaint\_mypaint_ref"
HERE = Path(__file__).resolve()
DEFAULT_OUT = HERE.parents[1] / "fixtures" / "palette_vectors.txt"
#: 布局向量给 **pigment-app** 的控件层测试用（`crates/pigment-app/tests/`）
DEFAULT_OUT_PV = (
    HERE.parents[3] / "pigment-app" / "tests" / "fixtures" / "paletteview_vectors.txt"
)


# --------------------------------------------------------------------------
# 真 `lib/palette.py`
# --------------------------------------------------------------------------


def _extract_clamp(ref_root):
    """从 `lib/helpers.py:244-249` 抽出真 `clamp()` 的函数体。"""
    src_path = (Path(ref_root) / "lib" / "helpers.py").resolve()
    src = src_path.read_text(encoding="utf-8")
    tree = ast.parse(src, filename=str(src_path))
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name == "clamp":
            seg = ast.get_source_segment(src, node)
            if seg is None:
                break
            ns = {}
            exec(compile(seg, str(src_path), "exec"), ns)
            print(f"# clamp 抽自 {src_path}:{node.lineno}", file=sys.stderr)
            return ns["clamp"]
    raise SystemExit(f"{src_path} 里找不到 clamp()")


def load_palette_module(ref_root):
    """`sys.path.insert(0, <源码根>)` 后 import 真 `lib/palette.py`。"""
    # 1) lib.gibindings：没有 PyGObject 就换成空模块（lib.color 需要）
    stubbed_gdk = False
    try:
        import gi  # noqa: F401
    except ImportError:
        mod = types.ModuleType("lib.gibindings")
        mod.GdkPixbuf = None
        sys.modules["lib.gibindings"] = mod
        stubbed_gdk = True

    sys.path.insert(0, str(ref_root))

    # 2) lib.helpers：拖进 C 扩展，换成只装真 clamp() 的替身
    helpers = types.ModuleType("lib.helpers")
    helpers.clamp = _extract_clamp(ref_root)
    sys.modules["lib.helpers"] = helpers

    import lib.palette as palette  # noqa: E402

    src = Path(palette.__file__).resolve()
    want = (Path(ref_root) / "lib" / "palette.py").resolve()
    if src != want:
        raise SystemExit(f"导入到的不是源码文件：{src} != {want}")
    print(f"# lib.palette 来自 {src}", file=sys.stderr)
    print(f"# lib.gibindings 走 stub: {stubbed_gdk}；lib.helpers 走 stub（真 clamp）",
          file=sys.stderr)
    return palette


# --------------------------------------------------------------------------
# 真 `gui/colors/paletteview.py` 的布局数学（ast 抽出函数体后 exec）
# --------------------------------------------------------------------------

#: `_PaletteGridLayout` 里**纯数学**的那几个方法（B4-2 对 adjbases.py 的同一做法）
WANTED_PV = [
    ("_PaletteGridLayout", "_constrain_swatch_size"),
    ("_PaletteGridLayout", "_get_palette_dimensions"),
    ("_PaletteGridLayout", "_size_alloc_cb"),
    ("_PaletteGridLayout", "get_position_for_index"),
    ("_PaletteGridLayout", "get_painting_offset"),
    ("_PaletteGridLayout", "get_index_at_pos"),
    ("_PaletteGridLayout", "do_get_preferred_height_for_width"),
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


def extract_paletteview(ref_root):
    """从 `gui/colors/paletteview.py` 抽 7 个纯数学方法，返回命名空间。"""
    src_path = (Path(ref_root) / "gui" / "colors" / "paletteview.py").resolve()
    src = src_path.read_text(encoding="utf-8")
    tree = ast.parse(src, filename=str(src_path))
    chunks = []
    for cls, fname in WANTED_PV:
        node = None
        for item in tree.body:
            if isinstance(item, ast.ClassDef) and item.name == cls:
                for sub in item.body:
                    if isinstance(sub, ast.FunctionDef) and sub.name == fname:
                        node = sub
                        break
        if node is None:
            raise SystemExit(f"{src_path} 里找不到 {cls}.{fname}")
        alias = f"{cls}__{fname}"
        stmts = _body_source(src, _strip_docstring(node.body))
        wrapper = f"def {alias}({ast.unparse(node.args)}):\n" + textwrap.indent(stmts, "    ")
        chunks.append(f"# ---- {cls}.{fname} @ :{node.lineno} ----\n{wrapper}\n")
        print(f"# 抽自 paletteview.py:{node.lineno} -> {alias}", file=sys.stderr)
    ns = {"math": __import__("math")}
    exec(compile("\n".join(chunks), "<paletteview-extracted>", "exec"), ns)
    return ns


class _FakeAllocation:
    def __init__(self, width, height):
        self.width = width
        self.height = height


class _FakePalette:
    def __init__(self, ncolors, columns):
        self._ncolors = ncolors
        self._columns = columns

    def get_columns(self):
        return self._columns

    def __len__(self):
        return self._ncolors


class _FakeManager:
    def __init__(self, palette):
        self.palette = palette


class _FakeGrid:
    """`_PaletteGridLayout` 的最小替身：只提供被抽出的那几行真正读到的东西。"""

    _SWATCH_SIZE_MIN = 8
    _SWATCH_SIZE_MAX = 50
    _SWATCH_SIZE_NOMINAL = 20
    _PREFERRED_COLUMNS = 5

    def __init__(self, ns, alloc_w, alloc_h, ncolors, columns,
                 rows=None, cols=None, size=None):
        self._ns = ns
        self._alloc = _FakeAllocation(alloc_w, alloc_h)
        self._palette = _FakePalette(ncolors, columns)
        self._rows = rows
        self._columns = cols
        self._swatch_size = size

    # 被抽出的函数体按模块层执行，所以这些"方法"得手工绑上去
    def _get_palette_dimensions(self):
        return self._ns["_PaletteGridLayout___get_palette_dimensions"](self)

    def _constrain_swatch_size(self, size):
        fn = self._ns["_PaletteGridLayout___constrain_swatch_size"]
        return fn(_FakeGrid, size)

    def get_painting_offset(self):
        return self._ns["_PaletteGridLayout__get_painting_offset"](self)

    def get_allocation(self):
        return self._alloc

    def get_color_manager(self):
        return _FakeManager(self._palette)


# --------------------------------------------------------------------------
# fixture 文本
# --------------------------------------------------------------------------

LINES = []
COUNTS = {}
#: `gui/colors/paletteview.py` 的布局向量单独一个文件 —— 它由 **pigment-app**
#: 的测试消费（布局换算在控件层），模型向量由 pigment-engine 消费。
PV_LINES = []
PV_COUNTS = {}


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


def emit_pv(kind, *parts):
    PV_COUNTS[kind] = PV_COUNTS.get(kind, 0) + 1
    PV_LINES.append("\t".join([kind] + [fmt(p) for p in parts]))


def color_fields(col):
    """一个 UIColor → (类型串, 三分量)。"""
    name = type(col).__name__
    if name == "RGBColor":
        return ("rgb",) + tuple(col.get_rgb())
    if name == "HSVColor":
        return ("hsv",) + tuple(col.get_hsv())
    if name == "HCYColor":
        return ("hcy", col.h, col.c, col.y)
    if name == "YCbCrColor":
        return ("ycbcr", col.Y, col.Cb, col.Cr)
    raise AssertionError(name)


def emit_palette(pal_id, pal):
    """把一个 Palette 的元数据与逐格内容写成向量。"""
    emit("pal_meta", pal_id, pal.get_name(), pal.get_columns(), len(pal))
    for i, col in enumerate(pal.iter_colors()):
        name = pal.get_color_name(i)
        hasname = 0 if name is None else 1
        if col is None:
            emit("pal_entry", pal_id, i, 1, "-", "-", "-", hasname, name)
        else:
            r, g, b = col.get_rgb()
            emit("pal_entry", pal_id, i, 0, float(r), float(g), float(b), hasname, name)


def emit_dump(pal_id, pal):
    text = pal.__unicode__()
    for i, line in enumerate(text.split("\n")):
        emit("pal_dump", pal_id, i, line)


def emit_repr(pal_id, pal):
    emit("pal_repr", pal_id, repr(pal))


def emit_simpledict(pal_id, pal):
    d = pal.to_simple_dict()
    name = d["name"]
    emit("pal_simpledict", pal_id, d["columns"],
         0 if name is None else 1, name, len(d["entries"]))
    for i, entry in enumerate(d["entries"]):
        if entry is None:
            emit("pal_simpledict_entry", pal_id, i, 1, "-", 0, "-")
        else:
            s, n = entry
            emit("pal_simpledict_entry", pal_id, i, 0, s,
                 0 if n is None else 1, n)


def emit_state(seq_id, step, pal):
    pos = pal.match_position
    emit("pal_state", seq_id, step, len(pal), pos, pal.match_is_approx)
    for i, col in enumerate(pal.iter_colors()):
        name = pal.get_color_name(i)
        hasname = 0 if name is None else 1
        if col is None:
            emit("pal_state_entry", seq_id, step, i, 1, "-", "-", "-", hasname, name)
        else:
            r, g, b = col.get_rgb()
            emit("pal_state_entry", seq_id, step, i, 0,
                 float(r), float(g), float(b), hasname, name)


# --------------------------------------------------------------------------
# 主流程
# --------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description="生成调色板向量 fixture")
    ap.add_argument("--ref", default=DEFAULT_REF)
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--out-pv", default=str(DEFAULT_OUT_PV),
                    help="paletteview 布局向量（pigment-app 消费）")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    ref = Path(args.ref)
    for rel in ("lib/palette.py", "lib/helpers.py", "lib/color.py",
                "lib/observable.py", "lib/pycompat.py"):
        if not (ref / rel).is_file():
            raise SystemExit(f"找不到源码：{ref / rel}")
    palettes_dir = ref / "palettes"
    if not palettes_dir.is_dir():
        raise SystemExit(f"找不到源码调色板目录：{palettes_dir}")

    palette_mod = load_palette_module(ref)
    Palette = palette_mod.Palette
    # `lib/palette.py` 只 import 了 RGBColor / YCbCrColor / UIColor（`:23-25`），
    # 其余颜色类从**同一个** `lib.color` 模块取（palette 已经把它导入了）
    import lib.color as color_mod  # noqa: E402

    RGBColor = color_mod.RGBColor
    HCYColor = color_mod.HCYColor
    HSVColor = color_mod.HSVColor
    YCbCrColor = color_mod.YCbCrColor

    header = [
        "# 由 crates/pigment-engine/tests/tools/gen-palette-vectors.py 生成，请勿手改。",
        f"# 事实来源：{ref}（真 import lib/palette.py，只把 lib.helpers 换成装着真 clamp() 的替身）",
        "# 浮点 = Python repr（最短往返）；Rust 侧应逐位 assert_eq!。",
        "# 行格式见脚本 docstring。",
    ]
    LINES.extend(header)

    # ---- 1. 真 .gpl 文件：载入 + 元数据 + 逐格 + 序列化 ----
    gpl_files = sorted(palettes_dir.glob("*.gpl"))
    if not gpl_files:
        raise SystemExit(f"{palettes_dir} 里没有 .gpl 文件")
    builtin = {}
    for gpl in gpl_files:
        pal_id = gpl.stem
        pal = Palette(filename=str(gpl))
        builtin[pal_id] = pal
        emit_palette(pal_id, pal)
        emit_dump(pal_id, pal)
        emit_repr(pal_id, pal)
        emit_simpledict(pal_id, pal)

    # ---- 2. 手工构造的调色板（覆盖空槽/重复色/单色）----
    greys16 = list(RGBColor(1, 1, 1).interpolate(RGBColor(0, 0, 0), 16))
    red2blue5 = list(RGBColor(1, 0, 0).interpolate(RGBColor(0, 1, 1), 5))
    extra = {}
    extra["greys16"] = Palette(colors=greys16)
    extra["red2blue5"] = Palette(colors=red2blue5)
    # 带空槽的：红 / 空 / 绿 / 黑 / 空 / 白
    empty_mix = Palette()
    for col in (RGBColor(1, 0, 0), None, RGBColor(0, 1, 0),
                RGBColor(0, 0, 0), None, RGBColor(1, 1, 1)):
        empty_mix.append(col)
    extra["empty_mix"] = empty_mix
    # 重复色 + 带名字
    dups = Palette()
    dups.append(RGBColor(0.8, 0.1, 0.1), "Red")
    dups.append(RGBColor(0.8, 0.1, 0.1), "Red again")
    dups.append(RGBColor(0.1, 0.1, 0.8), "Blue")
    dups.name = "Dups"
    dups.columns = 2
    extra["dups"] = dups
    # 只有一个纯黑：`match_color` 的 exact 分支会命中，`append(unique)` 也会
    black_only = Palette()
    black_only.append(RGBColor(0, 0, 0))
    extra["black_only"] = black_only
    for pal_id, pal in extra.items():
        emit_palette(pal_id, pal)
        emit_dump(pal_id, pal)
        emit_simpledict(pal_id, pal)

    all_palettes = dict(builtin)
    all_palettes.update(extra)

    # ---- 3. .gpl 解析的边界（异常/容错分支）----
    load_texts = {
        # 首行不是 GIMP Palette → RuntimeError
        "bad_header": "Not a GIMP Palette\n1 2 3 X\n",
        # Columns 不是整数 → ValueError
        "bad_columns": "GIMP Palette\nColumns: seven\n0 0 0 X\n",
        # 各种分隔、空白、越界通道值、注释、空行
        "messy": (
            "GIMP Palette\n"
            "Name:  Messy  Palette \n"
            "Columns: 3\n"
            "# a comment\n"
            "\n"
            "  0   0   0\tBlack\n"
            " 38\t38  38   Gray 15\n"
            "300 -5 20 Clamped\n"
            "1 2\n"
            "1 2 3\n"
            "0 0 0 __NONE__\n"
            "0 0 0 -dash name\n"
            "12 34 56\n"
            "   # indented comment\n"
            "9 9 9\t\tTwo tabs\n"
            "7 7 7Black\n"
        ),
        # 表头结束后再出现 `键: 值` 行 → 按颜色行解析、解析失败被跳过
        "late_field": "GIMP Palette\nName: X\nColumns: 2\nName: Y\n4 5 6 Tail\n",
        # 未知表头键 → 只警告
        "unknown_field": "GIMP Palette\nFoo: bar\nColumns: 1\n1 2 3 A\n",
        # 只有表头
        "only_header": "GIMP Palette\n",
        # 空槽 + 无名色 + 大小写/下划线键
        "slots": "GIMP Palette\nname: Slots\ncolumns: 4\n0 0 0 __NONE__\n0 0 0\n255 255 255 White\n",
    }
    for text_id, text in load_texts.items():
        # 先把输入原文写进 fixture（Rust 侧要自己跑 load_str）。制表符与反斜杠
        # 用 `\t` / `\\` 转义，Rust 侧反转义；空行是"末尾空字段"。
        for i, line in enumerate(text.split("\n")):
            emit("pal_load_text", text_id, i, _esc(line))
        pal = Palette()
        try:
            pal.load(_FakeHandle(text), silent=True)
            ok = 1
        except Exception as ex:  # noqa: BLE001
            emit("pal_load_ok", text_id, 0)
            print(f"# load({text_id}) 抛 {type(ex).__name__}: {ex}", file=sys.stderr)
            ok = 0
        if ok:
            emit("pal_load_ok", text_id, 1)
            emit_palette(text_id, pal)
            emit_dump(text_id, pal)

    # ---- 4. 颜色距离（逐位）----
    dist_colors = [
        RGBColor(0.0, 0.0, 0.0),
        RGBColor(1.0, 1.0, 1.0),
        RGBColor(0.45, 0.45, 0.45),
        RGBColor(0.5, 0.5, 0.5),
        RGBColor(0.855, 0.365, 0.180),  # #DA5D2E，源码注释里的橙
        RGBColor(0.2, 0.4, 0.9),
        HSVColor(0.25, 0.6, 0.8),
        HSVColor(0.999, 0.01, 0.5),
        HCYColor(0.5, 0.8, 0.4),
        HCYColor(0.0, 0.0, 0.0),
        YCbCrColor(Y=0.3, Cb=0.1, Cr=-0.2),
    ]
    for a in dist_colors:
        for b in dist_colors:
            ak, a0, a1, a2 = color_fields(a)
            bk, b0, b1, b2 = color_fields(b)
            emit("pal_distance", ak, a0, a1, a2, bk, b0, b1, b2,
                 palette_mod._color_distance(a, b))

    # ---- 5. 向外扩散的搜索顺序 ----
    for n in range(1, 7):
        for i in range(n):
            seq = list(palette_mod._outwards_from(n, i))
            emit("pal_outwards", n, i, ",".join(str(s) for s in seq))

    # ---- 6. match_color（无状态调用，每条独立）----
    probe_colors = [
        RGBColor(0.45, 0.45, 0.45),
        RGBColor(0.5, 0.5, 0.5),
        RGBColor(1.0, 0.0, 0.0),
        RGBColor(0.0, 1.0, 1.0),
        RGBColor(0.0, 0.0, 0.0),
        RGBColor(1.0, 1.0, 1.0),
        RGBColor(0.123, 0.456, 0.789),
        HSVColor(0.5, 0.5, 0.5),
        HCYColor(0.25, 0.5, 0.5),
    ]
    match_palettes = ["greys16", "red2blue5", "empty_mix", "dups", "black_only"]
    order_sets = [[0], [0, 2, 1, 3], [2, 0, 1], [4, 3, 2, 1, 0]]
    for pal_id in match_palettes:
        n = len(all_palettes[pal_id])
        presets = [None] + [p for p in (0, 1, min(2, n - 1), n - 1) if p < n]
        presets = sorted(set(presets), key=lambda x: (-1 if x is None else x))
        for probe in probe_colors:
            for exact in (False, True):
                for preset in presets:
                    orders_here = [None] + [
                        o for o in order_sets if max(o) < n
                    ]
                    for order in orders_here:
                        pal = Palette()
                        pal.update(all_palettes[pal_id])
                        if preset is not None:
                            pal.match_position = preset
                        ck, c0, c1, c2 = color_fields(probe)
                        ret = pal.match_color(probe, exact=exact, order=order)
                        emit("pal_match", pal_id, preset,
                             0 if order is None else 1,
                             "-" if order is None else ",".join(str(o) for o in order),
                             exact, ck, c0, c1, c2,
                             ret, pal.match_position, pal.match_is_approx)

    # ---- 7. 有状态的操作序列 ----
    def seq(seq_id, base_id, ops):
        pal = Palette()
        pal.update(all_palettes[base_id])
        emit("pal_seq", seq_id, base_id)
        for step, (op, payload, fn) in enumerate(ops):
            fn(pal, *payload)
            emit("pal_step", seq_id, step, op, *payload)
            emit_state(seq_id, step, pal)

    red = RGBColor(1.0, 0.0, 0.0)
    blue = RGBColor(0.0, 0.0, 1.0)

    # append：unique / match 四种组合（payload = 种类,c0,c1,c2,hasname,name,unique,match）
    ops = []
    for unique in (False, True):
        for match in (False, True):
            ops.append((
                "append",
                ["rgb", 0.8, 0.1, 0.1, 1, "Red", int(unique), int(match)],
                lambda p, *a: p.append(_color_from(palette_mod, a), a[5],
                                       unique=bool(a[6]), match=bool(a[7])),
            ))
    ops.append((
        "append",
        ["none", "-", "-", "-", 0, "-", 0, 0],
        lambda p, *a: p.append(None),
    ))
    ops.append((
        "append",
        ["rgb", -1.0, -1.0, -1.0, 0, "-", 1, 1],
        lambda p, *a: p.append(_color_from(palette_mod, a),
                               unique=bool(a[6]), match=bool(a[7])),
    ))
    seq("append_unique", "dups", ops)

    # insert：命中位置在插入点之后/之前/正好
    def insert_op(i, col):
        return (
            "insert",
            [i, "rgb", col.r, col.g, col.b, 0, "-"],
            lambda p, *a: p.insert(None if a[0] == "-" else int(a[0]),
                                   _color_from(palette_mod, a[1:])),
        )

    ops = [
        ("setmatch", [3], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        insert_op(5, red),
        insert_op(0, blue),
        insert_op(3, RGBColor(0.25, 0.5, 0.75)),
        ("insert", ["-", "rgb", 0.1, 0.2, 0.3, 0, "-"],
         lambda p, *a: p.insert(None, _color_from(palette_mod, a[1:]))),
    ]
    seq("insert_positions", "greys16", ops)

    # pop：删命中项 / 删命中项之前 / 之后。注意源码 `pop()` 在
    # `match_position is None` 时那句 `None > i` 会抛 TypeError（见 Rust 侧
    # `palette.rs` 的模块文档），所以每一步都先显式设好命中位置。
    ops = [
        ("setmatch", [5], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("pop", [8], lambda p, *a: p.pop(int(a[0]))),
        ("setmatch", [5], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("pop", [2], lambda p, *a: p.pop(int(a[0]))),
        ("setmatch", [5], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("pop", [5], lambda p, *a: p.pop(int(a[0]))),
        ("setmatch", [1], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("pop", [0], lambda p, *a: p.pop(int(a[0]))),
        ("setmatch", [0], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("pop", [0], lambda p, *a: p.pop(int(a[0]))),
        # 越界：源码 `self._colors.pop(i)` 先抛 IndexError 并直接 return，
        # 所以不会走到 `None > i` 那句
        ("pop", [20], lambda p, *a: p.pop(int(a[0]))),
    ]
    seq("pop_positions", "greys16", ops)

    # reposition：复制到空槽 / 向左移动 / 向右移动 / 移到末尾
    ops = [
        ("setmatch", [2], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("reposition", [4, 1], lambda p, *a: p.reposition(int(a[0]), int(a[1]))),
        ("reposition", [0, 4], lambda p, *a: p.reposition(int(a[0]), int(a[1]))),
        ("setmatch", [0], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("reposition", [0, 3], lambda p, *a: p.reposition(int(a[0]), int(a[1]))),
        ("setmatch", [3], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("reposition", [3, 0], lambda p, *a: p.reposition(int(a[0]), int(a[1]))),
        ("setmatch", [1], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("reposition", [1, 4], lambda p, *a: p.reposition(int(a[0]), int(a[1]))),
        ("reposition", [5, "-"], lambda p, *a: p.reposition(int(a[0]), None)),
        ("reposition", [2, 2], lambda p, *a: p.reposition(int(a[0]), int(a[1]))),
    ]
    seq("reposition_mix", "empty_mix", ops)

    # set / set_color_name / 名字查询
    ops = [
        ("set", [1, "rgb", 0.5, 0.25, 0.125], 
         lambda p, *a: p.__setitem__(int(a[0]), _color_from(palette_mod, a[1:]))),
        ("setname", [1, 1, "Middle"],
         lambda p, *a: p.set_color_name(int(a[0]), a[2])),
        ("setname", [0, 0, "-"],
         lambda p, *a: p.set_color_name(int(a[0]), None)),
        ("set", [0, "none", "-", "-", "-"],
         lambda p, *a: p.__setitem__(int(a[0]), None)),
    ]
    seq("set_and_name", "empty_mix", ops)

    # move_match_position：近似命中 → 落实 → 步进 → 跳过空槽
    def move_op(direction):
        return (
            "move",
            [direction],
            lambda p, *a: _move_keep_result(p, int(a[0])),
        )

    # 注意：`move_match_position` 的返回值不写进 fixture（它只是颜色），
    # 状态里的 match_position / approx 已经把语义钉住。
    ops = [
        move_op(-1),
        move_op(-1),
        move_op(-1),
        move_op(1),
        move_op(1),
        move_op(1),
        move_op(0),
    ]
    seq("move_greys", "greys16", ops)

    ops = [
        move_op(1), move_op(1), move_op(1), move_op(1),
        move_op(-1), move_op(-1), move_op(-1), move_op(-1),
    ]
    seq("move_empty_mix", "empty_mix", ops)

    # clear / update
    ops = [
        ("setmatch", [2], lambda p, *a: setattr(p, "match_position", int(a[0]))),
        ("clear", [], lambda p: p.clear()),
        ("update", ["dups"], lambda p, *a: p.update(all_palettes[a[0]])),
    ]
    seq("clear_update", "greys16", ops)

    # ---- 8. `gui/colors/paletteview.py` 的布局数学（真源码 ast 抽出）----
    pv = extract_paletteview(ref)

    # 8a. 色块边长约束
    for size in (-5, 0, 1, 7, 8, 9, 10, 19, 20, 21, 49, 50, 51, 200):
        out = pv["_PaletteGridLayout___constrain_swatch_size"](_FakeGrid, size)
        emit_pv("pv_constrain", size, out)

    # 8b. 行列数
    for columns in (-3, 0, 1, 2, 3, 4, 5, 7, 12):
        for ncolors in (0, 1, 3, 5, 7, 16, 77):
            _n, rows, cols = pv["_PaletteGridLayout___get_palette_dimensions"](
                _FakeGrid(pv, 100, 100, ncolors, columns)
            )
            emit_pv("pv_dims", columns, ncolors, rows, cols)

    # 8c. 自然高度 + 布局（这两步在 GTK 里是两轮协商，Rust 侧串起来用）
    layout_cases = []
    for columns in (0, 1, 3, 5, 7, 12):
        for ncolors in (1, 5, 16, 77, 300):
            for width in (60.0, 100.0, 150.0, 200.0, 333.0, 640.0):
                layout_cases.append((columns, ncolors, width))

    for columns, ncolors, width in layout_cases:
        grid = _FakeGrid(pv, width, 200.0, ncolors, columns)
        _min_h, nat_h = pv["_PaletteGridLayout__do_get_preferred_height_for_width"](
            grid, width
        )
        emit_pv("pv_nat_h", columns, ncolors, width, float(nat_h))

        grid = _FakeGrid(pv, width, nat_h, ncolors, columns)
        pv["_PaletteGridLayout___size_alloc_cb"](grid, None, grid.get_allocation())
        emit_pv("pv_size_alloc", columns, ncolors, width, float(nat_h),
             grid._rows, grid._columns, grid._swatch_size)

    # 8d. 位置 / 偏移 / 命中（用 8c 里算出的布局，覆盖"最后一行没填满"）
    for columns, ncolors, width in layout_cases:
        grid = _FakeGrid(pv, width, 200.0, ncolors, columns)
        _min_h, nat_h = pv["_PaletteGridLayout__do_get_preferred_height_for_width"](
            grid, width
        )
        grid = _FakeGrid(pv, width, nat_h, ncolors, columns)
        pv["_PaletteGridLayout___size_alloc_cb"](grid, None, grid.get_allocation())
        rows, cols, size = grid._rows, grid._columns, grid._swatch_size
        if not rows or not cols:
            continue
        dx, dy = pv["_PaletteGridLayout__get_painting_offset"](grid)
        emit_pv("pv_offset", rows, cols, size, width, float(nat_h), dx, dy)
        for index in range(min(ncolors, rows * cols)):
            x, y = pv["_PaletteGridLayout__get_position_for_index"](grid, index)
            emit_pv("pv_pos", rows, cols, size, width, float(nat_h), index, x, y)
        # 命中：四角、格心、每一列的左右边界、最后一行之外的空白
        xs = [0.0, 1.0, dx + 0.5, dx + size / 2.0, dx + cols * size - 0.5,
              dx + cols * size + 10.0, -5.0]
        ys = [0.0, 1.0, dy + 0.5, dy + size / 2.0, dy + rows * size - 0.5,
              dy + rows * size + 10.0, -5.0]
        for x in xs:
            for y in ys:
                for nearest in (False, True):
                    for insert in (False, True):
                        got = pv["_PaletteGridLayout__get_index_at_pos"](
                            grid, x, y, nearest=nearest, insert=insert
                        )
                        emit_pv("pv_index", rows, cols, size, width, float(nat_h),
                             ncolors, x, y, nearest, insert, got)

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(LINES) + "\n", encoding="utf-8", newline="\n")

    pv_path = Path(args.out_pv)
    pv_path.parent.mkdir(parents=True, exist_ok=True)
    pv_header = [
        "# 由 crates/pigment-engine/tests/tools/gen-palette-vectors.py 生成，请勿手改。",
        f"# 事实来源：{ref} 的 gui/colors/paletteview.py（ast 抽出 _PaletteGridLayout 的方法体后 exec）",
        "# 一行一类：见脚本 docstring 的 pv_* 说明。浮点 = Python repr。",
    ]
    pv_path.write_text("\n".join(pv_header + PV_LINES) + "\n", encoding="utf-8", newline="\n")

    if not args.quiet:
        total = sum(COUNTS.values())
        print(f"写入 {out_path}")
        print(f"向量总行数 {total}：")
        for k in sorted(COUNTS):
            print(f"  {k:22s} {COUNTS[k]}")
        pv_total = sum(PV_COUNTS.values())
        print(f"写入 {pv_path}")
        print(f"布局向量总行数 {pv_total}：")
        for k in sorted(PV_COUNTS):
            print(f"  {k:22s} {PV_COUNTS[k]}")


# --------------------------------------------------------------------------
# 小工具
# --------------------------------------------------------------------------


class _FakeHandle:
    """`Palette.load()` 要的 file-like：`readline()` + 迭代行。"""

    def __init__(self, text):
        self._lines = text.splitlines(keepends=True)

    def readline(self):
        return self._lines.pop(0) if self._lines else ""

    def __iter__(self):
        return iter(self._lines)


def _esc(s):
    """fixture 字段转义：反斜杠 → `\\\\`、制表符 → `\\t`（Rust 侧反转义）。"""
    return s.replace("\\", "\\\\").replace("\t", "\\t")


def _color_from(mod, fields):
    """`[类型, c0, c1, c2, ...]` → UIColor（`-` 表示 None）。"""
    if fields[0] == "-" or fields[0] == "none":
        return None
    kind, c0, c1, c2 = fields[0], float(fields[1]), float(fields[2]), float(fields[3])
    if kind == "rgb":
        return mod.RGBColor(c0, c1, c2)
    if kind == "hsv":
        return mod.HSVColor(c0, c1, c2)
    if kind == "hcy":
        return mod.HCYColor(c0, c1, c2)
    if kind == "ycbcr":
        return mod.YCbCrColor(c0, c1, c2)
    raise AssertionError(kind)


def _move_keep_result(pal, direction):
    """`move_match_position()` —— 参考色取当前命中格的颜色（没有就用灰）。"""
    pos = pal.match_position
    ref = pal[pos] if pos is not None else None
    if ref is None:
        import lib.color as _c
        ref = _c.RGBColor(0.5, 0.55, 0.45)
    pal.move_match_position(direction, ref)


if __name__ == "__main__":
    main()
