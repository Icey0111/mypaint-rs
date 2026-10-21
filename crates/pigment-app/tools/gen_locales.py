#!/usr/bin/env python3
"""Regenerate `src/shell/locales_generated.rs` from an installed MyPaint tree.

Usage:
    python tools/gen_locales.py E:/mypaint-w64/mingw64/lib/mypaint/lib

The source data are `config.py::supported_locales` and
`localecodes.py::LOCALE_DICT`; labels follow `preferenceswindow.py:434-479`.
"""

from __future__ import annotations

import ast
import json
import os
import sys


def load_assign(path: str, name: str):
    tree = ast.parse(open(path, encoding="utf-8").read())
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id == name for t in node.targets
        ):
            return ast.literal_eval(node.value)
    raise SystemExit(f"{name} not found in {path}")


def main() -> None:
    base = sys.argv[1] if len(sys.argv) > 1 else (
        r"E:\mypaint-w64\mingw64\lib\mypaint\lib"
    )
    locales = load_assign(os.path.join(base, "config.py"), "supported_locales")
    names = load_assign(os.path.join(base, "localecodes.py"), "LOCALE_DICT")

    rows = []
    for loc in locales:
        en, native = names.get(loc, (loc, loc))
        label = en if (not native or native == en) else f"{en} - ({native})"
        rows.append((loc, label, en))
    rows.sort(key=lambda r: (r[2].lower(), r[0]))

    out = [
        "//! 从安装版 MyPaint 的 `lib/config.py` + `lib/localecodes.py` 生成的",
        "//! 语言列表（`supported_locales` 全表；标签规则照",
        "//! `preferenceswindow.py:434-479`：英文名 + 当地拼写）。",
        "//! 这是**生成文件**，不要手改；重新生成见 `tools/gen_locales.py`。",
        "",
        "use super::prefs_window::OptionDef;",
        "",
        "pub(crate) static LOCALES: &[OptionDef] = &[",
        '    OptionDef { id: "", label: "System Language" },',
        '    OptionDef { id: "en_US", label: "English (US)" },',
    ]
    for loc, label, _en in rows:
        out.append(
            f"    OptionDef {{ id: {json.dumps(loc, ensure_ascii=False)}, "
            f"label: {json.dumps(label, ensure_ascii=False)} }},"
        )
    out.append("];")
    dest = os.path.join(
        os.path.dirname(__file__), "..", "src", "shell", "locales_generated.rs"
    )
    with open(dest, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print(f"wrote {os.path.abspath(dest)} ({len(rows)} locales)")


if __name__ == "__main__":
    main()
