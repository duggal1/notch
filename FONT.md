# FONT.md — notch typography contract

Companion to `DESIGN.md` (skin laws) and `TASTE.md` (what to say). Where this file disagrees with
`DESIGN.md` on type, the **latest user override wins**.

## 0. Read first

Status: v2 (2026-11-20). User override: the current face reads loose and wrong. Instrument Sans, tight
tracking, grayscale smoothing, weights 450/500. This file now reflects that override. v1 preferred
Inter; that preference is retired.

## 1. The one family, plus mono

- **Instrument Sans**, variable, bundled (`@fontsource-variable/instrument-sans`). Bundled, never
  `system-ui`, never a CDN. Same glyphs on every machine.
- **System mono** for raw IDs and terminal text only: `ui-monospace, 'SF Mono', Menlo, monospace`,
  `letter-spacing: 0`. No wide-tracked mono labels.
- Nothing else. No third font, ever.

Why Instrument Sans: the user's override. It is sharper at small sizes on pure black with the tight
tracking below, and the current Inter + wide mono labels were judged "way too loose" and "not the font
we want". Decided: **Instrument Sans**.

## 2. Weights — 450 and 500. Nothing else.

- `450` — running voice: body copy, descriptions, secondary lines, labels.
- `500` — the voice with its head up: titles, numerals, buttons, the thing being looked at.
- `400` and `600` are banned. Never fake-bold (`font-synthesis: none`), never `<b>`. Hierarchy comes
  from size, value, and spacing — not weight.

## 3. The case law — no all caps. Never.

- `FIXED` is a mistake. `Fixed` is a word.
- Every label, button, badge, column head, and section marker is sentence case. Proper nouns keep case.
- The mono-uppercase-with-wide-tracking idiom is deleted. Labels are sans, sentence case, 10.5px, 450,
  color at 56–62% white. The voice drops half a register and lands twice as sharp.
- Exceptions: none. Unit abbreviations are written as units (`ms`, `s`, `MB/s`), not caps.

## 4. Tracking is tied to size, not picked per vibe

| Size band | Tracking | Notes |
|---|---|---|
| `.num-xl` 88px | `-.05em` | display numerals, the orb count |
| `.num-l` 40px | `-.04em` | card numerals |
| `.title` 20px | `-.025em` | panel titles |
| body 13px | `-.015em` | base UI and labels |
| mono IDs / terminal 10.5–11px | `0` | no positive tracking anywhere |

No other values. No `letter-spacing: 0.08em` on a label — that idiom is deleted.

## 5. Smoothing — aggressively on the void

The shell is pure black. White-on-black on macOS must use grayscale AA, not subpixel. The block is set
on the document and **every window root, including the popover shell**:

```css
html, body, #root, .shell {
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  text-rendering: optimizeLegibility;
  font-synthesis: none;
  font-variant-numeric: tabular-nums;
  font-feature-settings: 'tnum' 1, 'zero' 1;
}
```

Any screenshot showing fuzzy or rainbow-fringed stems on the void is a FAIL in review, not a nit.

## 6. Type scale — nothing between 88 and 10.5

`88 / 40 / 20 / 13 / 10.5`. Nothing in between, ever. Hierarchy inside a zone is value and space.

## 7. Copy atoms

- Numbers flow per `DESIGN.md §9.1`. Identifiers (`S1-OC02`, pane IDs) never animate.
- Units sit outside the number, in the quieter voice (`140` `ms`).
- No sentence-ending periods on labels. Sentences carry numbers; numbers carry units; labels carry neither periods nor caps.
- `…` is a loading state, rendered as animated ellipsis only when content is pending.

## 8. Enforcement

- Reviewers check: weights ∈ {450, 500} · case is sentence · tracking in the table · smoothing block
  present on all roots · numerals tabular · zero `text-transform: uppercase` occurrences in `ui/src`.
- Lint: `grep -R "uppercase\|font-weight:\s*[4-9]00\|letter-spacing:\s*[0-9]*em" ui/src --include=*.css --include=*.tsx`
  — acceptable hits are the definition sites in `tokens`/`base` CSS only.
