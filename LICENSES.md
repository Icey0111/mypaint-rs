# Licences

This repository is a port of upstream Free/Libre/Open-Source work, so the
licences are declared per layer rather than repo-wide. The short version:
**everything derived from MyPaint's own sources is GPL, everything derived from
libmypaint's behaviour may be permissive, and the artist data stays public
domain / CC0.**

## Code

| Path | Licence | Why |
|---|---|---|
| `crates/pigment-app` | GPL-3.0-or-later | embeds MyPaint's `gui/*.xml` (GPL-2.0-or-later) and ports `gui/*.py` |
| `crates/pigment-engine` | GPL-3.0-or-later | ports MyPaint `lib/document.py`, `lib/command.py`, `lib/color.py`, `lib/palette.py`, `gui/mode.py` (GPL-2.0-or-later) |
| `crates/pigment-surface` | GPL-3.0-or-later | ports `lib/tiledsurface.py`, `lib/layer/*`, `lib/pixops.cpp`, `lib/fill/*` (GPL-2.0-or-later) |
| `crates/pigment-render` | GPL-3.0-or-later | ports the rendering responsibilities of `gui/tileddrawwidget.py`, `gui/displayfilter.py` |
| `crates/tablet-probe` | GPL-3.0-or-later | takes over the diagnostics of `gui/inputtestwindow.py` |
| [hokusai fork](https://github.com/Icey0111/hokusai) | MIT OR Apache-2.0 | reimplements behaviour documented by **ISC**-licensed libmypaint; no copyleft obligation, and permissive keeps the fixes upstreamable to [reearth/hokusai](https://github.com/reearth/hokusai) |

MyPaint is GPL-**2.0-or-later**, so redistributing ported modules under
GPL-3.0-or-later is allowed by the "or later" option. `LICENSE` at the repo root
is the GPL-3 text.

## Data and generated assets

Nothing here is ours to relicense, and several of these terms are deliberate:

| Path | Source | Licence |
|---|---|---|
| `crates/pigment-engine/assets/brushes/**` | [mypaint-brushes](https://github.com/mypaint/mypaint-brushes) | CC0-1.0 for the brush settings; the pack repo itself is GPL-2.0-or-later |
| `crates/pigment-engine/assets/palettes/MyPaint_Default.gpl` | MyPaint | CC0-1.0 |
| `crates/pigment-engine/assets/backgrounds/mamurk_e_1.png` | MyPaint `backgrounds/mrmamurk` | CC0-1.0 |
| `crates/pigment-app/assets/icons/**` | MyPaint `svg/` + `desktop/icons/` | public domain |
| `crates/pigment-app/assets/gui/*.xml` | MyPaint `gui/` | GPL-2.0-or-later, unmodified (11/11 byte-identical) |
| `crates/pigment-app/assets/i18n/zh_CN.po` | MyPaint `po/zh_CN.po` | GPL-2.0-or-later, unmodified (byte-identical) |

The brush settings are CC0 **by upstream policy** ("By policy, MyPaint's brush
settings are released into the public domain", `Licenses.dep5:45-50`). They are
meant to be usable in any artwork or application without conditions. Adding
copyleft to those files would be a mistake, not a hardening — see also
<https://github.com/mypaint/mypaint/wiki/Licensing-policy>.

Note that neither GPL nor CC0 restricts commercial use: GPL's requirement is
that redistributors pass on source and terms, and CC0 imposes none at all.

## Attribution

- libmypaint: Copyright (c) 2007-2011 Martin Renold and contributors (ISC).
- MyPaint: Copyright (c) 2005-2016 Martin Renold and the MyPaint Development
  Team (GPL-2.0-or-later); icons and desktop assets released into the public
  domain by Andrew Chadwick, David Revoy and the Tango Desktop Project.
- mypaint-brushes: individual authors, mostly CC0-1.0 (see `Licenses.dep5`).
- hokusai: Copyright (c) Re:Earth and contributors (MIT OR Apache-2.0).
