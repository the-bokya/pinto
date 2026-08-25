# pinto

A single self-contained binary that renders Frappe print-format HTML to PDF **entirely
in-process** — no Chrome, no browser, no external process. Frappe just calls `pinto`.

```
pinto --html printview.html --options config.json --out invoice.pdf
```

That's it. ~11 MB static binary, ~10 MB RAM, no downloads.

## Two engines

- **native** (default): a built-in HTML/CSS layout + PDF engine. Self-contained, low-memory,
  fast. This is the product.
- **chrome** (`--chrome --chrome-path <headless_shell>`): the original backend that drives a
  headless Chrome over CDP and reproduces Frappe's pipeline pixel-identically. Kept as a
  reference/validation oracle. See `git log` / earlier docs for its details.

## Native engine

Pipeline (`src/render/`):

| stage | module | what it does |
|---|---|---|
| parse | kuchikiki (html5ever) | HTML → DOM |
| style | `style.rs` | CSS cascade: selectors (tag/.class/#id/descendant/comma/*), specificity, UA defaults, inline styles, inheritance |
| layout | `layout.rs`, `table.rs` | block + inline + table flow, margin collapsing, box model, page-break candidates |
| text | cosmic-text | shaping, line-breaking, font metrics, font fallback |
| paint | `paint.rs` + krilla | glyphs (subset-embedded fonts), fills, gradients, images → PDF |
| driver | `engine.rs` | page geometry, pagination, repeating header/footer + page numbers |

Supported: block/inline/table layout; fonts (system, via fontdb) with weight/style/size;
color, background-color, linear-gradient; borders (per-side + collapsed table grid);
padding/margin (+ collapsing); images (data: URIs, disk, host-relative via a resolver);
page sizes (A/B/C series, Letter, Custom), orientation, margins; multi-page pagination;
repeating `#header-html`/`#footer-html` with dynamic "Page X of Y".

Not (yet) supported: flexbox/grid, floats, position:absolute, transforms, box-shadow,
border-radius, multi-column, `<canvas>`/SVG. These are uncommon in print formats.

## Fidelity vs Chrome

Text and common layouts render very close to Chrome (often near-identical, since both use the
same system fonts). It is **not pixel-identical** to Chrome and cannot be: a different engine
has a different rasterizer/hinting, the full Bootstrap/print CSS Frappe ships isn't bundled, and
Chrome has its own quirks (e.g. it squishes the print `#header-html` to ~6pt). Use `--chrome`
when byte-for-byte Chrome parity is required.

## Cost comparison (same 3-page doc)

| | client RSS | Chrome | total | 1-page time |
|---|---|---|---|---|
| native | ~10 MB | — | **~10 MB** | ~125 ms (cold, incl. font indexing) |
| chrome backend | ~7 MB | ~120 MB | ~127 MB | ~180 ms cold / ~32 ms warm |
| Frappe (Python+Chrome) | ~102 MB | ~120 MB | ~220 MB | ~173 ms |

## Config JSON

```jsonc
{
  "options": { "page-size": "A4", "orientation": "Portrait", "margin-top": "15mm", … },
  "host_url": "http://localhost:8000/",
  "site_public_path": "/…/sites/<site>/public",   // for host-relative <img>
  "bench_sites_path": "/…/sites",
  "default_page_size": "A4"
}
```
`.print-format { … }` CSS in the HTML overrides page/margin options, as in Frappe.

## Tests

`cargo test` runs 24 tests, no external tools or Chrome needed:

- **Unit** (`src/**`): unit conversion & page sizes, `.print-format` CSS extraction, colors,
  lengths, `@media` evaluation (print vs screen vs feature queries), selector parsing (pseudo-
  class vs pseudo-element vs attribute), and the CDP interceptor path guard.
- **Scenario integration** (`tests/scenarios.rs`): drives `engine::layout_pages` and asserts on
  the display list across — basic text, bordered tables, float/Bootstrap grid (side-by-side
  columns), `@media print`/`screen` selection, `visible-pdf`/`hidden-pdf` toggle, page
  sizes/orientation/custom, multi-page pagination, repeating header/footer with page numbers,
  linear gradients, data-URI images, and margin collapsing. Includes **regression guards** for
  the two bugs that once blanked real pages: an attribute-only selector (`[hidden]`) matching
  everything, and a pseudo-element rule (`.row:before`) leaking onto its base element.

Cross-engine fidelity against Chrome is checked separately with `tools/reference.py` +
`tools/pdfdiff.sh` (needs the Chrome build; renders both and pixel-diffs).

## Dev

```
cargo build --release
cargo test
./target/release/pinto --html f.html --options c.json --out o.pdf         # native
./target/release/pinto --chrome --chrome-path <headless_shell> …           # chrome oracle
```
