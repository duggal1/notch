# FONT.md — notch typography contract

Companion to `DESIGN.md` (skin laws) and `TASTE.md` (what to say). Where this file disagrees with `DESIGN.md` on type, **this file wins** (user override 2026-11-19). Where this file is silent, `DESIGN.md` wins.

## 0. Read first

Status: v1 (2026-11-19). This file exists because type is the product's voice made visible. If a screen could swap its type and nothing about it changed, the type is doing no work. Fix that.

## 1. The one family, plus mono

- **Inter**, variable, bundled (`@fontsource-variable/inter`; the variable file ships 100–900 — we use none of the middle weights). Bundled, never `system-ui`, never a CDN. Same glyphs on every machine.
- **System mono** for IDs, labels, timestamps, code: `ui-monospace, 'SF Mono', Menlo, monospace`.
- Nothing else. No third font, ever. If a new surface wants a font, the answer is no.

Why Inter and not Instrument Sans: both pass. Inter ships the exact OpenType pair we lean on (slashed zero + true tabular figures + disambiguated `1lI0`), the x-height holds at 10.5px on a black void, and its default apertures kill the "geometric specimen" look. Instrument Sans is prettier on marketing pages — we are not a marketing page. Decided: **Inter**.

## 2. Weights — 400 and 500. Nothing else.

- `400` — running voice: body copy, descriptions, secondary lines, mono IDs.
- `500` — the voice with its head up: titles, numerals, buttons, the thing being looked at.
- `600` is banned everywhere (DESIGN.md said 400/500/600 — user override: 400/500 only).
- Never fake-bold (`font-synthesis: none`), never `<b>`. Hierarchy comes from size, value, and spacing — not weight.

## 3. The case law — no all caps. Never.

- `FIXED` is a mistake. `Fixed` is a word.
- Every label, button, badge, column head, and section marker is sentence case. Proper nouns keep their own case.
- This replaces mono-uppercase-with-wide-tracking as the label idiom. Labels become: mono, sentence case, 10.5px, `400/500`, color at 56–60% white. The voice drops half a register and lands twice as sharp.
- Exceptions: none. Unit abbreviations are written as units (`ms`, `s`, `MB/s`), not caps.

## 4. Tracking is tied to size, not picked per vibe

Tracking is a function of rendered size and weight. The scale below is binding:

| Size band | Tracking | Notes |
|---|---|---|
| `.num-xl` 88px | `-.045em` | display numerals, the orb count |
| `.num-l` 40px | `-.035em` | card numerals |
| `.title` 20px | `-.02em` | panel titles |
| body 13px | `-.01em` | base UI |
| mono labels / IDs 10.5–11px | `.08em` | positive tracking is for small mono only — it buys legibility without caps |

No other values. No `letter-spacing: 0.18em` on an uppercase label — that idiom is deleted.

## 5. Smoothing — aggressively on the void

The shell is pure black. White-on-black on macOS must use grayscale AA, not subpixel.

```css
html, body {
  -webkit-font-smoothing: antialiased;        /* grayscale */
  -moz-osx-font-smoothing: grayscale;
  text-rendering: optimizeLegibility;          /* kerning + ligatures */
  font-synthesis: none;                        /* no faux bold/oblique */
  font-optical-sizing: auto;                   /* Inter's opsz axis */
  font-feature-settings: 'tnum' 1, 'zero' 1;   /* tabular + slashed zero */
}
```

Sharp at every size means: `-webkit-font-smoothing` is never omitted on this product, and any screenshot showing fuzzy or rainbow-fringed stems on the void is a FAIL in review, not a nit.

## 6. Type scale — nothing between 88 and 10.5

`88 / 40 / 20 / 13 / 10.5 mono`. Nothing in between, ever. Hierarchy inside a zone is value and space, not newsizes.

## 7. Copy atoms (where type meets TASTE.md)

- Numbers flow per `DESIGN.md §9.1` (`NumberRoll`, per-digit springs, velocity blur). Identifiers (`S1-OC02`, pane IDs) never animate.
- Units sit outside the number, in the quieter voice (`140` `ms`).
- Denominators and windows always attached to a metric ("across 2.1M steps this week"). No floating precision.
- No sentence-ending periods on labels. Sentences carry numbers; numbers carry units; labels carry neither periods nor caps.
- `…` is not elision — it is a loading state, rendered as actual animated ellipsis only when content is pending; static truncation uses a real ellipsis character at low opacity.

## 8. Enforcement

- DESIGN.md laws 6–7 reference this file.
- Reviewers check: weights ∈ {400, 500} · case is sentence · tracking in the table · smoothing block present · numerals tabular · units outside numbers · zero `text-transform: uppercase` occurrences anywhere in `ui/src`.
- The build has a lint (`grep -R "uppercase\|font-weight:\s*[6-9]00\|tracking" ui/src --include=*.css --include=*.tsx`) whose only acceptable hits are the definition sites in `tokens`/`base` CSS.
