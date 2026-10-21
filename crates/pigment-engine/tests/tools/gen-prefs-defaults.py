#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成用户偏好的默认键表 fixture —— Rust 侧逐条门禁用（B6-1）。

用法
----
在仓库根（``E:\\MyPaint\\pigment``）下跑：

    python crates/pigment-engine/tests/tools/gen-prefs-defaults.py

可选参数：

    --ref   <源码根>   默认 E:\\MyPaint\\_mypaint_ref
    --out   <输出文件> 默认 <本脚本目录>/../fixtures/prefs_defaults.txt
    --docs  <文档目录> Windows 分支的「文档」目录替身，默认 C:\\Users\\test\\Documents
    --quiet            不打印摘要

脚本可重复跑，输出确定（无随机数、键按字典序）。

事实来源与"真源码"的边界
-------------------------
``gui/userconfig.py`` 顶部 import 了 ``lib.gibindings``（PyGObject/GLib）与
``lib.eotf``、``gui.compatconfig``，``import gui.userconfig`` 在本机不可用
（没有 PyGObject）。所以本脚本用 ``ast`` 从**源码原文**抽出
``default_configuration()`` 这一个函数，包成模块后 ``exec``，再**真的调用它**
两次（``sys.platform`` 分别给 ``"win32"`` / ``"linux"``）：

- 函数体里只出现 ``sys.platform`` / ``os.path.join`` / ``lib.glib.get_user_special_dir``
  / ``GLib.UserDirectory.DIRECTORY_DOCUMENTS`` / ``DEFAULT_EOTF`` / ``COMPAT_CONFIG``
  六个外部名字，全部给最小替身（见 ``run_defaults()``）。
- ``DEFAULT_EOTF`` 取自 ``lib/eotf.py`` 的赋值（``2.2``），``COMPAT_CONFIG``
  取自 ``gui/compatconfig.py`` 的 ``DEFAULT_CONFIG`` —— 两个文件都是纯字面量，
  用同样的 ``ast`` 过滤（只留赋值/类定义）后 ``exec`` 取值，**不是**手抄。
- ``os.path`` 在 win32 那一次换成 ``ntpath``、linux 那一次换成 ``posixpath``，
  于是 fixture 与运行本脚本的宿主平台无关（Windows 路径一律反斜杠）。

于是产物与 Rust 侧 ``prefs::default_configuration()`` 的对照是逐键、逐值的：
"平台分支"（Windows 的 ``MyPaint\\scrap`` 前缀与 Button2/Button3 互换）也被覆盖。

输出格式
--------
纯文本、``\\t`` 分隔、``#`` 开头是注释：

    header  <name>  <value>              元信息（platforms / docs_dir_win32 ...）
    count   <platform>  <n>              该平台默认键的条数
    default <platform>  <key>  <type>  <json-value>

``type`` 是 Python 的可见类型名，映射到 JSON 的可见类型：
``null`` / ``bool`` / ``int`` / ``float`` / ``str`` / ``list`` / ``dict``
（元组按 JSON 语义报 ``list``；源码里 ``frame.color_rgba`` 就是元组）。
``json-value`` 是 ``json.dumps(value)`` —— Rust 侧用 ``serde_json`` 解析，
浮点按最短往返表示，``0.12`` / ``2.2`` 与 Rust 侧 ``json!(0.12)`` 逐位相同。
"""

import argparse
import ast
import json
import ntpath
import posixpath
import types
from pathlib import Path

DEFAULT_REF = r"_mypaint_ref"
DEFAULT_DOCS = r"C:\Users\test\Documents"


# ---------------------------------------------------------------------------
# 用 ast 从源码取值（不 import 那些依赖 GI 的模块）
# ---------------------------------------------------------------------------


def exec_literal_module(path: Path, wanted):
    """执行 ``path`` 里的**赋值与类定义**，返回 ``wanted`` 里列出的名字。

    过滤掉 import / 函数定义（``lib/eotf.py`` 的 eotf() 系列、
    ``gui/compatconfig.py`` 的 class 之外没有副作用），于是不需要 GI。
    """
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    body = [
        n
        for n in tree.body
        if isinstance(n, (ast.Assign, ast.AnnAssign, ast.ClassDef))
    ]
    ns = {}
    exec(compile(ast.Module(body=body, type_ignores=[]), str(path), "exec"), ns)
    return {name: ns[name] for name in wanted}


def extract_function(path: Path, name: str):
    """抽出单个顶层函数，编译成可执行的 code 对象（全局名由调用方喂）。"""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    fn = next(
        (n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == name),
        None,
    )
    if fn is None:
        raise SystemExit("源码里找不到函数 %s（%s）" % (name, path))
    mod = ast.Module(body=[fn], type_ignores=[])
    return compile(mod, str(path), "exec")


def run_defaults(code, platform, docs_stub, default_eotf, compat_config):
    """在替身环境里调用 ``default_configuration()``，返回它的 dict。"""
    fake_sys = types.SimpleNamespace(platform=platform)
    fake_lib = types.SimpleNamespace(
        glib=types.SimpleNamespace(get_user_special_dir=lambda _d: docs_stub)
    )
    fake_glib = types.SimpleNamespace(
        UserDirectory=types.SimpleNamespace(DIRECTORY_DOCUMENTS=1)
    )
    fake_os = types.SimpleNamespace(path=ntpath if platform == "win32" else posixpath)
    ns = {
        "sys": fake_sys,
        "os": fake_os,
        "lib": fake_lib,
        "GLib": fake_glib,
        "DEFAULT_EOTF": default_eotf,
        "COMPAT_CONFIG": compat_config,
    }
    exec(code, ns)
    return ns["default_configuration"]()


# ---------------------------------------------------------------------------
# 类型标签（JSON 可见类型）
# ---------------------------------------------------------------------------


def type_label(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int):
        return "int"
    if isinstance(value, float):
        return "float"
    if isinstance(value, str):
        return "str"
    if isinstance(value, (list, tuple)):
        return "list"
    if isinstance(value, dict):
        return "dict"
    raise TypeError("fixture 不支持的类型：%r" % (type(value).__name__,))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--ref", default=DEFAULT_REF)
    ap.add_argument(
        "--out",
        default=str(Path(__file__).resolve().parent.parent / "fixtures" / "prefs_defaults.txt"),
    )
    ap.add_argument("--docs", default=DEFAULT_DOCS)
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    ref = Path(args.ref)
    userconfig = ref / "gui" / "userconfig.py"
    if not userconfig.is_file():
        raise SystemExit("找不到源码 %s" % userconfig)

    eotf = exec_literal_module(ref / "lib" / "eotf.py", ["DEFAULT_EOTF"])["DEFAULT_EOTF"]
    compat = exec_literal_module(
        ref / "gui" / "compatconfig.py", ["DEFAULT_CONFIG"]
    )["DEFAULT_CONFIG"]
    code = extract_function(userconfig, "default_configuration")

    lines = []
    lines.append("# 由 tests/tools/gen-prefs-defaults.py 从 _mypaint_ref 生成；不要手改。")
    lines.append("# gui/userconfig.py default_configuration() 的完整默认键表（含平台分支）。")
    lines.append("# 源码：gui/userconfig.py:50-140；COMPAT_CONFIG 见 gui/compatconfig.py:36-49。")
    lines.append("header\tref\tsource:gui/userconfig.py:50-140")
    lines.append("header\tdocs_dir_win32\t%s" % args.docs)
    lines.append("header\tdocs_dir_linux\t-")

    platforms = ["win32", "linux"]
    counts = {}
    for platform in platforms:
        defaults = run_defaults(code, platform, args.docs, eotf, compat)
        counts[platform] = len(defaults)
        lines.append("count\t%s\t%d" % (platform, len(defaults)))
        for key in sorted(defaults):
            value = defaults[key]
            lines.append(
                "default\t%s\t%s\t%s\t%s"
                % (
                    platform,
                    key,
                    type_label(value),
                    json.dumps(value, ensure_ascii=False, sort_keys=True),
                )
            )

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    if not args.quiet:
        print("写出 %s" % out)
        for platform in platforms:
            print("  %-6s %d 条默认键" % (platform, counts[platform]))
        # 平台差异摘要（人看的，不进门禁）
        win = run_defaults(code, "win32", args.docs, eotf, compat)
        lin = run_defaults(code, "linux", args.docs, eotf, compat)
        for key in sorted(win):
            if win[key] != lin[key]:
                print("  平台差异 %s:\n    win32 = %r\n    linux = %r" % (key, win[key], lin[key]))


if __name__ == "__main__":
    main()
