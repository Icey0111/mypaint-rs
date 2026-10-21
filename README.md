# mypaint-rs

A Rust port of [MyPaint](https://github.com/mypaint/mypaint) 2.x and
[libmypaint](https://github.com/mypaint/libmypaint): brush engine, tile
surface, document model, and an egui + wgpu application. It is an independent
project, not an official MyPaint repository, and it is nowhere near
release-ready as a replacement — what it aims at is *behavioural parity*, and
this README is about how much of that is measured rather than asserted.

The crate names inside are still `pigment-*` (the port's working title).

**It exists because the port works, and it is posted because one person cannot
maintain a painting program.** See [Looking for maintainers](#looking-for-maintainers).

## The two repositories

| Repository | Contents | Licence |
|---|---|---|
| this one | `pigment-app` (egui shell), `pigment-engine` (document, commands, modes, file I/O), `pigment-surface` (tiles, layers, mipmaps, strokemap, symmetry), `pigment-render` (wgpu canvas), `tablet-probe` | GPL-3.0-or-later |
| [Icey0111/hokusai](https://github.com/Icey0111/hokusai) | fork of [reearth/hokusai](https://github.com/reearth/hokusai) `f7e9981` (v0.3.0) carrying the libmypaint parity fixes and the C differential gate | MIT OR Apache-2.0 |

The engine is consumed as a git dependency (`branch = "main"`, pinned in
`Cargo.lock`), so `cargo build` works on a fresh clone with no sibling checkouts.

## Parity evidence

| Check | Result | How to reproduce |
|---|---|---|
| libmypaint C vs Rust CPU, whole official brush pack | **194 / 196 bit-identical**, 2 differ, 0 errors | [`cref` gate](https://github.com/Icey0111/hokusai/tree/main/crates/hokusai-compat/cref) |
| Same, the 9 `hokusai-compat` stroke scripts, each with its own event stream | **9 / 9 bit-identical** | `cref/fixture_gate.py` |
| Bundled `.myb` files vs upstream `mypaint-brushes` | **196 / 196 byte-identical** | `sha256` compare |
| Bundled `gui/*.xml` (menu, toolbars, resources) vs upstream MyPaint | **11 / 11 byte-identical** | same |
| `MyPaint_Default.gpl`, `mamurk_e_1.png` | identical | same |
| Test suites | 933 (pigment) + 98 (hokusai) = **1031 passing, 0 failing** | `cargo test --workspace` in each |

The C gate compiles a libmypaint checkout with minimal `config.h` / `json.h`
stubs — no json-c, GLib or pkg-config — renders the same brush and the same
events through both engines, and compares the RGBA u16 fix15 buffers byte for
byte. MinGW `gcc -O2` and MSVC `cl /O2` reference builds give the same 194 and
the same two exceptions, so this is not one compiler's float behaviour.

The two remaining brushes: `puantilism2.myb` (715 values, max 3 LSB — pure
eraser dabs, alpha exact, only the spectral erasing of RGB differs) and
`Fan#1.myb` (6 values, max 10 LSB).

### Read this before quoting "194/196"

- The committed event stream uses **4-field events** (`x, y, pressure, dt`):
  tilt = 0, **viewzoom = 1.0**, viewrotation = 0, barrel = 0. So the
  tilt-, zoom- and rotation-driven dynamics are **not exercised**, even though
  the code paths for them were ported. Sweeping 10-field events is the single
  highest-value addition to this gate.
- It is **one trajectory, one pressure ramp, 56 events at 512×256**. Dab
  spacing, speed filters and `partial_dabs` behaviour are only tested along it.
- The `hokusai-compat` **PNG goldens are self-generated**, so
  `cargo test -p hokusai-compat` detects regressions, not parity. Only the C
  gate above is parity evidence.
- The GPU path is deliberately approximate (see below) and is compared to the
  CPU path with a tolerance, not bit-exactly.
- Nothing here has been tested against MyPaint's *saved files* at scale: ORA
  output is verified against the format's structure and unit fixtures, not
  against a corpus of real drawings.

## What works today

Windows-only so far (Win32 code is `cfg(windows)`-gated and POSIX config paths
exist, but no Linux/macOS build has been run). Developed and tested against a
real graphics tablet, mouse-only paths included.

Verified through 73 scripted interaction checkpoints driving the actual app via
Win32 `SendInput`, each asserting both application state and canvas pixel deltas
([`docs/UI_INTERACTION_TEST_REPORT.md`](docs/UI_INTERACTION_TEST_REPORT.md)):

- All 9 tool modes: freehand, inking, straight line, connection/sequence,
  ellipse, flood fill, layer move, frame edit, symmetry edit.
- All 9 top-level menus, brush chooser (search, groups, hotkeys, size keys),
  colour panel (HSV square, hue bar, history, palettes, picker), layer panel
  (new/group/duplicate/remove/visibility/opacity/blend mode/reorder),
  view (zoom, pan, rotate, fit, reset), brush editor, preferences, input test,
  About, native open/save dialogs.
- Documents: ORA and PNG read/write, autosave and recovery, undo/redo
  (copy-on-write tile snapshots), scratchpad, symmetric drawing, 22 layer
  combine modes, spectral (wide-gamut) colour mixing.
- Low-zoom painting: mipmap-based preview keeps distant zooms responsive
  (thumbnail rebuild 965 ms → 2.8 ms measured at 12.5% on an 8040×5971 canvas).
- Localisation: English + Simplified Chinese, using MyPaint's own `po/` catalogues.

## What is missing or approximate

Known absent (the port has the model but not the surface):

- Colour adjuster pages: HCY wheel, HSV wheel, HSV square, HSV cube, component
  sliders, and the Wash / Rings / CrossedBowl changers. Only a single HSV
  square + hue bar page exists; every colour tool action opens that same page.
- Mode options panel (`gui/optionspanel.py`) — actions registered, handlers empty.
- Layer view UI, layer properties dialog (F2 only reveals the layer panel).
- Background chooser window, brush icon editor, quick brush/colour popups,
  undo-history popup, button-mapping editor, full palette editor.
- Progress reporting (`lib/feedback.py`): save, load and fill run synchronously.
- Single document only — no multi-window session.
- `.myb` v2 (pre-JSON) files are not migrated.

Deliberate divergence, all measurable and all reversible:

- GPU rasterises dabs and can hold the smudge chain on-device, sampling every
  8 dabs by default (`PIGMENT_SMUDGE_SAMPLE_DABS=1` restores per-dab sampling;
  eligibility is narrow and CPU fallback is the normal case).
- `PIGMENT_NO_GPU_DABS=1` forces the CPU path end to end — that is the parity
  reference the numbers above were taken on.
- Mipmap / Instant-Preview display at low zoom is a local addition; final
  pixels come from replaying full-resolution events.
- An extra non-standard `tilt` input, inherited from hokusai.
- Sparse `get_color` mirrors MSVC `rand()` on Windows; other platforms use
  Park–Miller and are not bit-exact against their local libc.

The full per-file ledger — upstream symbol by upstream symbol, with what has a
local counterpart, what is partial and what is missing — is
[`docs/UPSTREAM_ALIGNMENT.md`](docs/UPSTREAM_ALIGNMENT.md) (~2000 lines,
Chinese). [`docs/ENGINE_PORT_AUDIT.md`](docs/ENGINE_PORT_AUDIT.md) is the
function-level audit that produced the engine fix list.

## Build

Rust ≥ 1.88. On Windows, the MSVC toolchain (`windows-sys` is used; `wgpu`
needs it anyway).

```sh
git clone https://github.com/Icey0111/mypaint-rs.git
cd mypaint-rs
cargo run -p pigment-app            # starts with the bundled brush pack
cargo test --workspace              # 933 tests
```

Asset XML (menus, toolbars, actions) is parsed at build time from
`crates/pigment-app/assets/gui/`, so the app's action table is generated from
MyPaint's own definitions.

To run the C differential gate you additionally need a libmypaint checkout, the
brush pack, and a C compiler — see the
[`cref` README](https://github.com/Icey0111/hokusai/tree/main/crates/hokusai-compat/cref).

## Licence

Mixed, deliberately, and per layer — [`LICENSES.md`](LICENSES.md) has the table
and the attribution list:

- every crate in this repository — **GPL-3.0-or-later**. They port MyPaint's
  `lib/*.py`, `lib/pixops.cpp` and `gui/*.py`, which are GPL-2.0-or-later, so
  copyleft is not a style choice here.
- the [hokusai engine fork](https://github.com/Icey0111/hokusai) stays
  **MIT OR Apache-2.0** like upstream. libmypaint is **ISC**, so bit-exact
  parity work carries no copyleft obligation, and keeping the permissive grant
  leaves the door open to upstreaming those fixes to reearth/hokusai.
- bundled data keeps its own terms: `.myb` brushes CC0 1.0, icons derived from
  MyPaint's `svg/` and `desktop/icons/` (public domain), `MyPaint_Default.gpl`
  CC0, `mamurk_e_1.png` CC0, `gui/*.xml` and `po/zh_CN.po` GPL-2.0-or-later and
  unmodified. **The brush settings are intentionally unrestricted** — that is
  upstream's licensing policy, not an oversight, so don't "fix" it by adding
  copyleft to asset files.

`LICENSE` at the root is the GPL-3 text.

## Looking for maintainers

The port reached the point where the remaining work is not one person's: ~14
missing UI surfaces, an event-space sweep for the parity gate, Linux/macOS
bring-up, and a GPU path that should be proven rather than tolerated.

If you want to take a piece of it — or all of it — start with
`docs/UPSTREAM_ALIGNMENT.md` §8 and §10, which list what is missing with file
and line references, and with the `cref` gate, which tells you whether a change
moved the numbers. Come and say hello on the
[MyPaint forum](https://community.mypaint.app/c/development/) first; the port's
assumptions are worth arguing about before anyone writes code.
