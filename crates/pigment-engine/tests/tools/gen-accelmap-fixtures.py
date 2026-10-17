#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成 B6-2 加速键的两个 fixture —— Rust 侧逐条门禁用。

用法
----
需要 **PyGObject + GTK 3** 的解释器（本机是 MyPaint 自带的那支）：

    E:/mypaint-w64/mingw64/bin/python.exe \\
        E:/MyPaint/pigment/crates/pigment-engine/tests/tools/gen-accelmap-fixtures.py

可选参数：

    --ref   <源码根>   默认 E:\\MyPaint\\_mypaint_ref
    --out   <fixture 目录> 默认 <本脚本目录>/../fixtures
    --quiet            不打印摘要

产物（可重复跑，除 conf 内的条目顺序外完全确定 —— GTK 的 save 用哈希序，
顺序不可依赖，Rust 侧按"行集合"比对）：

1. ``accelmap_gtk.conf`` —— **真 GTK** 写出的 ``accelmap.conf``：
   ``Gtk.AccelMap.change_entry()`` 灌入全部绑定后 ``Gtk.AccelMap.save()``。
   这给出文件格式的唯一权威样本（头三行、条目行、被清空条目的注释行）。
2. ``accel_names.tsv`` —— 加速键字符串 → GTK 规范名 / 显示名的真值表。

事实来源
--------
- 绑定集合：``crates/pigment-app/assets/gui/resources.xml``（vendored，与
  ``_mypaint_ref/gui/resources.xml`` 逐字节一致，见 actions.rs 的门禁）
  里的 105 条 ``<accelerator>``；加上 ``gui/document.py:467-492``
  ``_init_context_actions`` 运行期注册的 20 条槽位键。
- 规范名/显示名：**真 GTK** 算出来的（``gtk_accelerator_name`` /
  ``gtk_accelerator_get_label``），不是推导出来的。GTK 版本记录在 fixture 头。

``accel_names.tsv`` 的列：``spec <TAB> canonical_name <TAB> label <TAB> origin``。
``canonical_name`` 为 ``INVALID`` 表示 ``Gtk.accelerator_parse()`` 拒绝该字符串
（Rust 侧据此断言"非法输入必须被拒"）。
"""

import argparse
import os
import sys
import xml.etree.ElementTree as ET

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GLib  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))

#: `resources.xml` 的 `modifiers` 属性 → GTK 加速键字符串里的修饰键写法。
#: 与 `.workbuddy/tools/gen-accel-labels.py` 同一张表（那个脚本产出
#: `accel_labels.tsv`，本脚本产出**规范名**，两者共同钉住 actions.rs）。
MODS = {
    "GDK_CONTROL_MASK": "<Control>",
    "GDK_MOD1_MASK": "<Alt>",
    "GDK_SHIFT_MASK": "<Shift>",
    "GDK_SUPER_MASK": "<Super>",
    "GDK_META_MASK": "<Meta>",
    "GDK_HYPER_MASK": "<Hyper>",
}

#: `accel_names.tsv` 里额外要覆盖的**别名/边界**样本。每条都写明为什么在这里：
#: 它们是 `accelmap.conf` 的解析器必须接受（或必须拒绝）的写法，而源码/GTK
#: 都能给出真值。`resources.xml` 与 `document.py` 里只出现其中一部分。
EXTRA_SPECS = [
    # document.py:492 用**小写** `<control>` 写槽位键 → GTK 认为等价于 `<Primary>`
    ("<control>0", "document.py:492 的小写写法"),
    ("<Control>0", "大写 <Control>（GTK 别名）"),
    ("<Ctrl>z", "<Ctrl> 也是 GTK 别名"),
    ("<Mod1>r", "<Mod1> 是 <Alt> 的别名"),
    ("<primary>z", "修饰键名大小写不敏感"),
    # 单字符：GTK 只接受字母数字，且**一律小写化**（探针结果，见模块文档）
    ("Z", "单个大写字母 → 规范名是小写"),
    ("z", "单个小写字母"),
    ("0", "单个数字"),
    ("!", "单个标点：GTK 拒绝（必须用键值名 exclam）"),
    (" ", "空格字符：GTK 拒绝（必须用键值名 space）"),
    # 必须用 gdk_keyval_name 的拼写，拼错就是非法
    ("BackSpace", "GDK 名是 BackSpace"),
    ("Backspace", "拼写错误 → 拒绝"),
    ("Page_Up", "GDK 名是 Page_Up"),
    ("PageUp", "拼写错误 → 拒绝"),
    ("Space", "GDK 名是 space（小写）"),
    ("space", "小写 space 才对"),
    ("Menu", "GDK 的 Menu 键（egui 无对应物，保留在表里）"),
    ("", "空串 → 拒绝"),
    ("<Shift>", "只有修饰键 → 拒绝"),
    # 下面这些是 shell/accelmap.rs 的**捕获表**会产出的键名，让 GTK 判断拼写
    ("Return", "捕获表：Enter"),
    ("Escape", "捕获表：Esc"),
    ("Tab", "捕获表：Tab"),
    ("Delete", "捕获表：Delete"),
    ("Insert", "捕获表：Insert"),
    ("Home", "捕获表：Home"),
    ("End", "捕获表：End"),
    ("Page_Down", "捕获表：PageDown"),
    ("Up", "捕获表：↑"),
    ("Down", "捕获表：↓"),
    ("Left", "捕获表：←"),
    ("Right", "捕获表：→"),
    ("comma", "捕获表：,"),
    ("period", "捕获表：."),
    ("minus", "捕获表：-"),
    ("plus", "捕获表：+"),
    ("equal", "捕获表：="),
    ("slash", "捕获表：/"),
    ("backslash", "捕获表：\\\\"),
    ("semicolon", "捕获表：;"),
    ("apostrophe", "捕获表：'"),
    ("grave", "捕获表：`"),
    ("bracketleft", "捕获表：["),
    ("bracketright", "捕获表：]"),
    ("less", "捕获表：Shift+,"),
    ("greater", "捕获表：Shift+."),
    ("bar", "捕获表：Shift+\\\\ / |"),
    ("colon", "捕获表：:"),
    ("question", "捕获表：?"),
    ("exclam", "捕获表：!"),
    ("braceleft", "捕获表：{"),
    ("braceright", "捕获表：}"),
    ("F35", "捕获表：F35（功能键上界）"),
    # GTK 的 key_labels[] 与小键盘显示名（accelmap::key_label 的第 1、2 条规则）。
    # 这几个的显示名与键名不同，必须由真 GTK 定：
    ("BackSpace", "GTK 显示 Backspace（不是 BackSpace）"),
    ("Multi_key", "GTK 显示 Multikey"),
    ("KP_Enter", "GTK 显示 Enter (keypad)"),
    ("KP_Add", "小键盘 +：GTK 走键值 → Unicode"),
    ("KP_Subtract", "小键盘 -"),
    ("KP_4", "小键盘 4（源码里绑 RotateLeft）"),
    ("KP_5", "小键盘 5（ResetRotation）"),
    ("KP_6", "小键盘 6（RotateRight）"),
    ("KP_Decimal", "小键盘 ."),
    ("Caps_Lock", "下划线换空格：Caps Lock"),
    ("Scroll_Lock", "下划线换空格：Scroll Lock"),
    ("Print", "原样：Print"),
    ("Help", "原样：Help"),
    # egui 的 Super/Meta/Hyper 无对应位 → 我们**拒绝**这三族，GTK 接受。
    # 记在这里是为了让"我们比 GTK 窄"这件事有据可查（Rust 侧单独断言）。
    ("<Super>x", "GTK 接受；egui 无 Super 位 → 我们拒绝"),
    ("<Meta>x", "GTK 接受；egui 无 Meta 位 → 我们拒绝"),
    ("<Hyper>x", "GTK 接受；egui 无 Hyper 位 → 我们拒绝"),
]

#: 只用来验证"被清空的条目在文件里是注释行"的合成路径（不对应任何动作）。
CLEARED_PATH = "<Actions>/GenAccelMapFixture/ClearedEntry"


def source_accels(ref_root):
    """从真源码取加速键：返回 ``[(path, spec, origin)]``。"""
    xml_path = os.path.join(
        HERE, "..", "..", "..", "pigment-app", "assets", "gui", "resources.xml"
    )
    rows = []
    root = ET.parse(xml_path).getroot()
    for group in root.iter("object"):
        if group.get("class") != "GtkActionGroup":
            continue
        group_name = group.get("id")
        for child in group.findall("child"):
            obj = child.find("object")
            action_id = obj.get("id")
            for accel in child.findall("accelerator"):
                spec = "".join(
                    MODS[m.strip()]
                    for m in (accel.get("modifiers") or "").split("|")
                    if m.strip()
                ) + accel.get("key")
                rows.append(
                    (
                        "<Actions>/%s/%s" % (group_name, action_id),
                        spec,
                        "resources.xml:%s/%s" % (group_name, action_id),
                    )
                )

    # 槽位键：`gui/document.py:467-492` 的 `_init_context_actions`。
    # 源码用 `ag.add_actions()` 把它们（连同 accel 字符串）注册进
    # DocumentActions 组，于是也进 GTK 的 AccelMap，路径同样是
    # `<Actions>/{group}/{action}`（`gui/accelmap.py:169`）。
    doc_py = os.path.join(ref_root, "gui", "document.py")
    with open(doc_py, encoding="utf-8") as f:
        text = f.read()
    start = text.index("def _init_context_actions")
    body = text[start : text.index("def _init_stategroups")]
    for literal, why in (('"%d" % x', "restore"), ('"<control>%d" % x', "store")):
        if literal not in body:
            raise SystemExit(
                "document.py 的 _init_context_actions 里没有 %s —— 源码变了，"
                "请核对槽位键的注册方式" % literal
            )
    for x in range(10):
        rows.append(
            (
                "<Actions>/DocumentActions/Context0%d" % x,
                "%d" % x,
                "document.py:478-485 槽位恢复 Context0%d" % x,
            )
        )
        rows.append(
            (
                "<Actions>/DocumentActions/Context0%ds" % x,
                "<control>%d" % x,
                "document.py:478-485 槽位存入 Context0%ds" % x,
            )
        )
    return rows


def name_and_label(spec):
    keyval, mods = Gtk.accelerator_parse(spec)
    keyval = int(keyval)
    mods = Gdk.ModifierType(int(mods))
    if not keyval:
        return None, None
    return Gtk.accelerator_name(keyval, mods), Gtk.accelerator_get_label(keyval, mods)


def write_conf(path, bindings):
    GLib.set_prgname("mypaint")
    accel_map = Gtk.AccelMap.get()
    bad = 0
    for accel_path, spec, origin in bindings:
        keyval, mods = Gtk.accelerator_parse(spec)
        if not keyval:
            print("跳过（GTK 拒绝 %r）：%s" % (spec, origin))
            bad += 1
            continue
        accel_map.change_entry(accel_path, int(keyval), Gdk.ModifierType(int(mods)), True)
    # 一条"被清空"的条目：GTK 写出来是注释行（见 gtkaccelmap.c 的 save），
    # 解析时必须忽略 —— 于是"删掉一个默认绑定"不会跨进程保留。
    accel_map.change_entry(CLEARED_PATH, 0, Gdk.ModifierType(0), True)
    Gtk.AccelMap.save(path)
    return bad


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--ref",
        default=os.path.join(HERE, "..", "..", "..", "..", "..", "_mypaint_ref"),
    )
    ap.add_argument("--out", default=os.path.join(HERE, "..", "fixtures"))
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    ref_root = os.path.abspath(args.ref)
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)

    bindings = source_accels(ref_root)
    if len(bindings) != 125:
        raise SystemExit("绑定数应为 105 + 20 = 125，实得 %d" % len(bindings))

    # ---- 1. 真 GTK 写出的 accelmap.conf ----
    conf_path = os.path.join(out_dir, "accelmap_gtk.conf")
    bad = write_conf(conf_path, bindings)

    # ---- 2. 加速键字符串 → 规范名 / 显示名 ----
    tsv_path = os.path.join(out_dir, "accel_names.tsv")
    seen = set()
    rows = []
    for _accel_path, spec, origin in bindings:
        if spec in seen:
            continue
        seen.add(spec)
        rows.append((spec, origin))
    for spec, why in EXTRA_SPECS:
        if spec in seen:
            continue
        seen.add(spec)
        rows.append((spec, why))

    written = 0
    with open(tsv_path, "w", encoding="utf-8", newline="\n") as f:
        f.write(
            "# @generated by crates/pigment-engine/tests/tools/gen-accelmap-fixtures.py"
            " —— 请勿手工编辑。\n"
        )
        f.write(
            "# 内容：加速键字符串的 Gtk.accelerator_parse()/accelerator_name()/"
            "accelerator_get_label() 真值\n"
        )
        f.write(
            "# 解释器：MyPaint 自带（PyGObject + GTK %d.%d.%d）\n"
            % (Gtk.get_major_version(), Gtk.get_minor_version(), Gtk.get_micro_version())
        )
        f.write("# 列：spec <TAB> canonical_name <TAB> label <TAB> origin\n")
        for spec, origin in rows:
            name, label = name_and_label(spec)
            if name is None:
                f.write("%s\tINVALID\tINVALID\t%s\n" % (spec, origin))
            else:
                f.write("%s\t%s\t%s\t%s\n" % (spec, name, label, origin))
            written += 1

    if not args.quiet:
        with open(conf_path, "rb") as f:
            conf_lines = f.read().decode("utf-8").splitlines()
        print("wrote %d entries to %s (%d 行)" % (len(bindings), conf_path, len(conf_lines)))
        print("wrote %d rows to %s" % (written, tsv_path))
        if bad:
            print("警告：GTK 拒绝了 %d 条绑定" % bad)
        print("GTK %d.%d.%d, prgname=%r"
              % (Gtk.get_major_version(), Gtk.get_minor_version(),
                 Gtk.get_micro_version(), GLib.get_prgname()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
