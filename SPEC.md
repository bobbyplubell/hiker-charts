# hiker-charts — Product Spec

A reusable charting component for native Rust UIs, built for Hiker. It turns tabular data
(CSV today) into charts described by a **compact, declarative grammar-of-graphics spec** — so
a chart is a small piece of text that lives next to the data and notes it describes:
version-controllable, diffable, and reproducible.

The **chart spec (the DSL) is the durable asset**. The renderer is a detail behind it: v1
draws through [plotters](https://github.com/plotters-rs/plotters) for breadth, but the spec
model never names a plotters type, so the renderer can be replaced without touching a single
chart, note, or CSV.

The component is **UI-toolkit-agnostic**; egui (Hiker's toolkit) is the reference host. It
produces a self-contained artifact (SVG today; a native draw list later) that a host embeds.

This document is the **user-facing requirements** — what the component does, plus the phased
plan for getting there. Implementation detail lives in a companion `IMPLEMENTATION.md`.

> **Status / changelog.** This revision (2026-06) reflects three decisions made after the
> first draft:
> 1. **Renderer:** plotters as a *swappable backend* for v1, behind a `Backend` trait — not
>    a first-party geometry engine yet (§4, §14). The first-party engine is the phase-2
>    north star, not a v1 requirement.
> 2. **DSL:** the grammar **is** a serde struct deserialized from YAML/JSON — no hand-written
>    parser (§2). A terser bespoke syntax is deferred (§8.4).
> 3. **Visual builder:** demoted from "co-equal surface" to **phase 2** (§8). v1 is
>    text-spec-only. The DSL, not the builder, is the priority.

---

## 1. What this is

A drop-in chart component Hiker (and other hosts) embed. Given a chart spec plus tabular
data, it computes and paints a chart. The host owns everything around it — where the spec
text lives, how data files are read, theming, change-watching, and where the rendered output
is placed.

It is **not** a data-analysis tool or dashboard. The value is narrow and deliberate: a chart
definition that is **co-located** with its source, **reproducible** from text, and
**diffable** in version control. Heavy data wrangling is out of band — in v1 the data is
assumed plot-ready.

### 1.1 The two Hiker surfaces it serves
1. **Inline chart in a note** — a fenced ```` ```chart ```` block in a `.md` (YAML config,
   authored as text), rendered to a `BlockWidget` exactly like the existing `mermaid` /
   `wavedrom` blocks (§11.1).
2. **Standalone CSV tab** — open a `.csv` in Hiker and configure a chart with a
   **buttons-and-dropdowns** builder + live preview, then optionally **export the YAML** to
   paste into a note. Config optional (defaults inferred from the columns). (§8.2, §11.2)

A third consumer reuses the same core: **in-app debug/internal graphs** built by
constructing a `ChartSpec` directly in Rust. One charting stack across the whole app.

---

## 2. Specification format (the DSL)

### 2.1 A grammar of graphics, as a serde struct
A chart is `data` + `mark` + `encoding`, where each encoding channel binds a data field to a
visual channel (position, color, …) with a declared type. The model is borrowed in *shape*
from established grammar-of-graphics designs (Vega-Lite especially; see `references/`), but
it is the component's **own** format, not a subset of any standard.

**The grammar is the struct definition.** The canonical model is a serde `ChartSpec` struct;
YAML (and JSON) are just *constructors* for it — `serde_yml`/`serde_json` deserialize
straight into `ChartSpec`. There is **no hand-written parser** to maintain, round-tripping is
free, and the same struct is what in-app Rust code builds directly.

Channels are **flat top-level fields** (`x`/`y`/`color`), each a bare-string shorthand *or* a
full object when a type/scale is needed — terse for the common case, expressive when not:

```chart
mark: line
x: month                                  # shorthand
y: [revenue, profit]                      # single field or a list (wide multi-series)
color: { field: region, type: nominal }   # full form when you need the type
data: sales.csv
```

### 2.2 The format is bounded by what the renderer can draw
A mark, channel, or scale exists in the format only if the active backend can draw it (§4) —
so there is no "the spec says X but we can't draw it" gap. In v1 the bound is plotters'
capability surface. Unknown or malformed fields are reported as **diagnostics**, never
silently dropped.

### 2.3 v1 scope
- **Marks:** `bar`, `line`, `point`, `area`. (plotters covers these directly; more marks as
  plotters and, later, the first-party engine allow.)
- **Encoding channels:** `x`, `y`, `color`. `y` accepts **a single field or a list** of
  fields (§2.3.1).
- **Types:** `quantitative`, `temporal`, `ordinal`, `nominal`, declared in the encoding.
  Temporal (date/time axis) is in v1; date parsing is self-contained (§3.4) and is the
  riskiest piece, to be de-risked early.
- **Data:** inline values, or a CSV referenced by identifier; **wide or long** shape
  (§2.3.1).
- **Config:** title, axis titles, legend on/off, palette overrides (defaulting to the
  injected theme, §6).
- **Deferred (first follow-up, not v1):** stacking (`stack` on bar/area — grouped bars and
  single/overlaid areas only in v1); log scales; dual axes; `size`/`shape` channels. See §12.

#### 2.3.1 Multi-series: wide and long data
A chart's series can come from either CSV shape, both supported in v1; internally both lower
to one `Vec<Series>`, so the renderer never distinguishes them.

- **Wide** — one column per series; name the columns in a `y` list:
  ```chart
  mark: line
  x: month
  y: [revenue, profit]        # 2 columns → 2 series
  ```
- **Long** ("tidy") — series names live as values in one column; split on it with `color`:
  ```chart
  mark: line
  x: month
  y: value                    # the single value column
  color: metric               # one series per distinct value of `metric`
  ```

Wide is the natural shape for hand-authored note blocks; long is the natural shape for
exported/queried data. v1 accepts both so neither use case requires reshaping data by hand.

The recognizable v1 chart set that these primitives produce: line (single/multi),
time-series line & area (temporal x), scatter (`point`, optionally colored), grouped bar, and
area.

### 2.4 Growth and optional portability bridge
Coverage grows by adding marks, channels, scales, and (later) optional data transforms —
each bounded by what the renderer supports (§2.2). Because the model is shaped to resemble
established interchange grammars, an **optional exporter** to such a grammar (to open a chart
in an external viewer) stays cheap to add later. Cross-tool portability is a *future bridge*,
not a foundational dependency.

---

## 3. Data: model, sources, dependencies

### 3.1 Data is supplied, never read directly
A spec references its data by an opaque **identifier** (path or URI). The component does
**not** read files itself — the host's **data resolver** (§10) maps an identifier to tabular
rows, keeping file access, sandboxing, and network policy host-side. Inline data (values
embedded in the spec, or the CSV body of a ```` ```chart ```` block) is also supported.

### 3.2 Declared dependencies
The component exposes the **set of data identifiers a spec depends on** (§10), so the host
can watch those sources and ask for a re-render when they change. It does not poll or watch
anything itself.

### 3.3 Identifiers are opaque
The component never interprets, resolves, rewrites, or persists a data identifier. Renames,
moves, and relinking are the host's concern; the component only ever hands an identifier to
the resolver.

### 3.4 Typing and coercion
- The encoding's declared **type** is authoritative.
- When a type is absent, it is **inferred** by sniffing the column (numeric vs date vs
  string) — also what drives a bare CSV's default chart (§1.1).
- The host may optionally supply an external **column-type schema** (e.g. Frictionless Table
  Schema) to make typing explicit and portable.
- All string→typed coercion (numbers, dates/times) is **self-contained** — no native or
  system math/locale libraries.

### 3.5 Tabular abstraction
CSV (delimited text) is the v1 input, but the resolver interface is **format-agnostic**: it
yields typed columns/rows, so a host can back it with any tabular source.

---

## 4. Rendering

### 4.1 One model, swappable backends (plotters in v1)
The `ChartSpec` model is **renderer-agnostic**. A `Backend` trait paints a spec+data into an
output artifact; backends are swappable without touching the model. **v1 ships one backend
built on plotters** (SVG output), reusing the existing `hiker-render/chart` prototype.

This is a deliberate near-term/long-term split:
- **Near term (v1):** plotters gives many plot types immediately for little code. The DSL —
  the durable asset — stays independent of it.
- **Long term (phase 2):** a **first-party geometry engine** (own scale math, ticks, axes,
  legends, mark shapes) becomes a second backend behind the same trait, unlocking the egui
  draw-list path (§4.3), byte-determinism (§4.4), and full theming (§6). See §14.

**Invariant that makes the swap free:** no plotters type ever appears in a `pub` signature of
the `core` crate (§9). plotters lives in its own backend crate (or behind a feature). If that
invariant holds, replacing or adding a backend touches no spec, note, CSV, or host.

### 4.2 SVG backend (v1)
Produces a **self-contained SVG** (and a rasterized RGBA form via the host's existing
resvg→texture pipeline) suitable for embedding, snapshotting, or export. plotters' SVG
backend emits `<text>` and leaves glyph drawing to the SVG consumer (resvg with the bundled
Liberation Sans), so the SVG path needs no native font backend (`plotters/ttf` /
font-kit stays **off** — it breaks wasm and splits measure-vs-render). Render once, cache, blit.

### 4.3 Native draw-list backend — *phase 2*
A future backend paints chart geometry directly into the egui draw list:
resolution-independent (crisp at any zoom), cheap to redraw per frame, the foundation for
interactivity. **Requires the first-party geometry engine** (plotters has no egui draw-list
backend), so it is explicitly phase 2, not v1.

### 4.4 Determinism
- **v1 (plotters):** plotters' SVG is **not guaranteed byte-identical**, so v1 does *not*
  promise byte-stable output. Testing is snapshot-on-the-model plus tolerance-based image
  diffs on the SVG (§13).
- **Phase 2 (first-party engine):** identical `(spec + resolved data + theme + size)` →
  **byte-identical** output, enabling golden-file tests.

### 4.5 No heavyweight runtime
Rendering is native code. **No embedded browser, no JavaScript engine**, no external service.
The SVG path has no GPU requirement.

### 4.6 Self-contained math
Any numerical work the component does itself — scale computation, binning/regression/density
later — is implemented without native/system numerical libraries. (plotters does its own
internal math in v1; this constraint binds the first-party engine and any transforms we add.)

---

## 5. Sizing & layout

### 5.1 Constraint-driven
The chart renders to a host-provided **available width** with an aspect/height policy
(fill-width at an aspect ratio, or explicit dimensions). Given constraints, it reports the
size it occupies so the host can reserve space.

### 5.2 DPI-aware
Rasterized output is produced at the host's pixel ratio for sharpness. The phase-2 native
draw-list backend is resolution-independent and needs no DPI input.

---

## 6. Theming

### 6.1 Injected palette
Colors — background, axis, gridlines, text, and categorical/series scales — come from a
**host-supplied theme**, so a chart matches Hiker's light or dark appearance.

> **v1 caveat:** plotters' theming surface is limited, so v1 applies the host theme as far as
> plotters allows (series colors, background, text/axis where exposed). **Full** theme
> injection across every drawn element is a first-party-engine capability (§14).

### 6.2 Theme is part of identity
A theme change alters the rendered output and therefore the content identity (§7), so caches
invalidate correctly on theme switches.

### 6.3 Spec overrides
A spec may override palette choices (via the config/encoding) but **defaults to the injected
theme** when it does not.

---

## 7. Caching & identity

The component exposes a **stable content hash over the *inputs*** —
`(spec + resolved data + theme + size)` — **not over the rendered output bytes** (plotters'
SVG isn't byte-stable, §4.4). A host keys its rendered-output cache on this hash: any change
to any input → new hash → re-render; no change → cache hit, no work per frame. This is the
single contract that lets a host re-render exactly when an input — including the referenced
data file — actually changes. The contract is backend-independent and survives the phase-2
renderer swap unchanged.

---

## 8. Authoring surfaces

There are three constructors for one `ChartSpec` (§2.1): inline YAML text, the CSV-tab comfy
builder, and direct Rust. v1 ships the first two; live two-way text sync is phase 2.

### 8.1 Inline text surface (v1)
The spec text (YAML/JSON → `ChartSpec`) in a ```` ```chart ```` block is the **source of
truth** for inline charts and round-trips **losslessly** through serde. Parse errors, unknown
fields, and unsupported-feature notices are reported back to the host as diagnostics. Inline
blocks are **authored as text** in v1 — rendered live, but edited by hand (no in-place
widgets on a note's chart; that is §8.3).

### 8.2 Comfy builder in the CSV tab (v1)
The standalone CSV tab (§11.2) provides a **buttons-and-dropdowns** panel over an in-memory
`ChartSpec`: a mark dropdown; `x`/`y`/`color` field pickers **populated from the CSV's actual
columns** with inferred types shown; a single "series from columns / values in a column"
control that writes the wide or long encoding (§2.3.1); type pickers; title; legend toggle —
all beside a **live preview**. Every action mutates the same `ChartSpec`, so a panel-built
spec is identical to a hand-authored one.

This tier is cheap precisely because the source of truth is **in-memory tab state** (persisted
to a sidecar / tab settings) — there is **no text to keep in sync**. Column-populated controls
also mean a field can't be misspelled and an undrawable encoding can't be chosen (enforcing
§2.2 by construction).

**Export to YAML (v1):** an "Copy as ```` ```chart ```` block" action serializes the spec
(`serde_yml::to_string`) so a user can design a chart visually in the CSV tab and paste it
into a note. This is the **one-directional bridge** between the two surfaces; it needs no
format-preserving round-trip and is the reason live inline editing (§8.3) can wait.

### 8.3 "Open in chart editor" — round-trip on save (v1)
An action on an inline ```` ```chart ```` block (§11.1) that opens the **same comfy builder**
(§8.2), seeded from the block's parsed `ChartSpec`. On **save (Ctrl+S)** the builder
re-serializes the spec and **splices it back into the note**, replacing the block's byte range,
then writes the note. This gives inline charts an editable visual surface **without** the hard
continuous-sync problem (§8.3.1).

It works because the round-trip is **discrete, not continuous**: save regenerates the whole
config block from the struct in one shot, so no format-preserving incremental edit is needed.
Mechanism:
- **Provenance:** the block's `(note id + byte range)` — the byte range already comes from the
  fence detector (`editor-md`'s `*_spans`, §11.1). Save splices into exactly that range.
- **Data stays pristine:** for a block with an inline CSV body, only the **config** portion is
  regenerated; the original CSV bytes are re-attached verbatim.
- **Forward-compat:** unknown config fields survive via the spec's `#[serde(flatten)]` extra
  capture (§8.5-style).
- **Accepted loss:** comments / exact formatting *inside the YAML config* are not preserved —
  acceptable because this is an explicit edit-and-save, not as-you-type sync.

#### 8.3.1 Continuous live two-way sync — *deferred, likely unnecessary*
Keeping the widgets and the in-note text in sync on *every keystroke* (a format-preserving
incremental round-trip) is the genuinely hard variant. §8.3's open→edit→save flow covers the
practical workflow, so this is deferred indefinitely unless a concrete need appears.

### 8.4 All optional
A host may embed the model+renderer alone (inline-text-only), the CSV-tab builder, or both.
The builder is a separate layer that depends on the core, not the reverse.

### 8.5 Compact syntaxes share the core — *deferred*
A terser, purpose-specific syntax (Observable-Plot-style brevity) may later be added as a
**thin front-end that lowers to the same `ChartSpec`**, never a parallel renderer. Deferred:
YAML→struct is expressive enough for v1, and a bespoke parser is exactly the maintenance the
"grammar is the struct" decision avoids. Revisit only if the YAML proves too verbose in
practice.

---

## 9. Packages

Mirrors Hiker's "portable core + thin toolkit layer" split:

- **`core`** — UI-agnostic and **renderer-agnostic**: the `ChartSpec` model (GoG types +
  serde), the data model and typing/coercion, the `Backend` trait, content hashing,
  dependency extraction, and the host interfaces (data resolver, theme). **No GUI-toolkit
  dependency, and no plotters in any `pub` signature** (§4.1).
- **`backend-plotters`** (name TBD) — the v1 `Backend` impl over plotters → SVG. Reuses the
  `hiker-render/chart` prototype. Isolated so it can be swapped/feature-gated.
- **`gui`** — the reference egui layer: rasterizes the SVG via resvg→texture and embeds it;
  later hosts the draw-list backend and visual builder. Depends on `core` + the toolkit.
- **`cli` / examples** — headless render (`spec + data → SVG/PNG`) for snapshot tests and
  batch export.

---

## 10. Host integration surface

| Interface | Direction | Responsibility |
| --- | --- | --- |
| **Data resolver** | host implements | identifier → typed tabular rows; owns files/sandbox/network |
| **Theme / palette** | host implements | supplies colors |
| **Dependency query** | host consumes | the data identifiers a spec depends on, for change-watching |
| **Content hash** | host consumes | cache key over `(spec + data + theme + size)` (inputs, not output) |
| **Size constraints** | host provides | available width + aspect/height policy |
| **Output sink** | host consumes | receives SVG / RGBA (native draw list in phase 2) |
| **Diagnostics** | host consumes | parse errors, unknown fields, unsupported-feature notices |

---

## 11. Hiker integration: inline block & CSV tab

### 11.1 Inline ```` ```chart ```` block
Detected like the existing diagram blocks: `editor-md` reports the fence's byte range + inner
source range (the `*_spans` pattern in `editor/editor-md/src/diagrams.rs`), and the app turns
each span into a `BlockWidget` rendered by this component. `editor-md` stays renderer-unaware.

An **"open in chart editor"** affordance on the rendered block opens the same comfy builder
seeded from the block, and a save writes the regenerated block back to the note (§8.3) — the
byte range the detector already reports is what the save splices into.

Block shape — YAML config, optional `---` separator, optional inline CSV body (the CSV half
stays a clean, valid CSV):

```chart
mark: bar
x: month
y: [revenue, profit]
---
month,revenue,profit
jan,100,20
feb,140,35
```

…or pure config referencing an external file (host resolves the path, §3.1):

```chart
mark: line
x: month
y: revenue
data: sales.csv
```

### 11.2 Standalone CSV tab
Opening a `.csv` renders it as a chart with editable plotting settings. With no config, a
default `ChartSpec` is inferred (first column → x, numeric columns → series) via the type
sniffing in §3.4. Settings the user adjusts serialize back to a `ChartSpec` (e.g. a sidecar
or in-tab config), so the standalone and inline paths share one model.

### 11.3 Distinct from mermaid's `xychart`
mermaid's `xychart` is *diagram*-oriented (fixed categorical axes). This is for *data*: real
numeric/temporal axes, auto-scaling, many series. They stay separate widgets.

---

## 12. Out of scope (v1)

- **First-party geometry engine, native egui draw-list backend, byte-determinism, full theme
  injection** — phase 2 (§14).
- **Continuous live two-way text↔widget sync** — deferred, likely unnecessary (§8.3.1). The
  CSV-tab comfy builder (§8.2) *and* "open in chart editor → save back" for inline blocks
  (§8.3) are both in v1; only per-keystroke in-place sync is deferred.
- **Terse bespoke DSL** — deferred (§8.4).
- **Stacking, log/secondary axes, `size`/`shape` channels** — first follow-up after v1
  (§2.3). v1 does grouped bars and single/overlaid areas only.
- **Data transforms** — aggregation, filtering, joins, statistics. v1 data is plot-ready.
- **Interactivity** — hover, tooltips, zoom, pan, drill-down, selection.
- **Composition** beyond v1 scope — layering, faceting, concatenation, repetition.
- **Geographic/map marks**, animation, non-tabular data.
- **Adopting an external chart-spec standard** as the native format (§2.1). Interop is via an
  optional exporter (§2.4), not by rendering a foreign spec directly.

---

## 13. Testing

- **Model round-trip:** `YAML/JSON → ChartSpec → YAML/JSON` is stable, including preservation
  of fields the model doesn't yet understand (serde flatten/extra-fields capture).
- **Coercion tests:** string→number and string→date/time edge cases.
- **Render snapshots (v1):** since plotters output isn't byte-stable (§4.4), assert on the
  *model* and use **tolerance-based image diffs** on the rasterized SVG, not exact golden
  bytes. Each mark × encoding combination across the v1 scope.
- **Phase 2:** swap to exact **golden-file SVG snapshots** once the first-party engine
  guarantees byte-identical output.

---

## 14. Phased plan

**Phase 1 — v1 (this spec's scope).** `core` model (`ChartSpec` + serde, data/typing, host
traits, content hash, dependency extraction), the plotters SVG backend behind the `Backend`
trait, the Hiker ```` ```chart ```` inline block (authored as text, §8.1), the standalone CSV tab
**with the comfy builder + export-to-YAML** (§8.2), and **"open in chart editor → save back"**
for inline blocks (§8.3). Goal: charts in notes and from CSVs, the DSL as the stable contract,
and a buttons-and-dropdowns surface for both surfaces.

**Phase 2 — first-party engine.** A first-party geometry-engine backend behind the same trait
→ egui draw-list backend, byte-determinism (golden tests), full theme injection. The terse DSL
front-end if the YAML proves too verbose. (Continuous live two-way sync, §8.3.1, remains
deferred — likely unnecessary.) None of this changes the v1 spec model, blocks, or CSVs —
that's the payoff of the §4.1 invariant.

**Later — the long tail.** Data transforms, interactivity, layering/faceting, the optional
interchange-grammar exporter, more marks/channels/scales.

---

## 15. Performance expectations

- A static render of a **modest** dataset (up to a few thousand marks) is fast enough to
  produce on change without perceptible lag, and is cached after first paint (§7).
- **Large** datasets (tens of thousands of marks) are a known limit of the SVG/raster path;
  the phase-2 native backend and/or data reduction address this later.
- Re-rendering happens **only on input change** (§7), never per frame for a static chart.
