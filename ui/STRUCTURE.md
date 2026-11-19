# STRUCTURE — notch popover (Agent B, ui/)

Structure pass + the two mandatory screenshot-review passes. Updated in place as
decisions changed. Stack (lead override, 2026-11-19): **Tauri v2 + Rust shell,
React 19 + TypeScript strict, Tailwind CSS v4 (CSS-first), Vite 8, motion
(motion/react).** DESIGN.md v1.1 + FONT.md v1 are the law; TASTE.md governs copy
and subtraction.

## 1. What this is, physically

The popover is the menu-bar control plane made physical: a black shell fused to
the hardware notch that hangs below the menu bar like the laptop's own chin. It
is not a window, not a dashboard. It is the notch growing a screen. One glance
answers: **how many of my agents are working right now, and is anything stuck?**

## 2. Primary thing, one idea per view

Primary thing: **the working count, embodied as light.** The orb (ref 2) is the
one large element; its rim color = worst status, its level = load. Everything
else — heartbeat, grid, radar, drift, IDs, rows — is the fine print under that
sentence.

One idea per zone:
- Header: whose machine this is + the firehose (eps) + the worst-status word.
- Zone 1 (orb + instruments): how much work is in flight, and how hot the bus is.
- Zone 2 (tiles): the shape of the swarm — supervisors as a radar, sessions as a dot matrix.
- Zone 3 (list): the actual sessions, scannable, keyboard-navigable, actionable in place.
- Zone 4 (drift): is step latency honest this run?
- Footer: the one housekeeping action (sweep temp orphans). Nothing else.

## 3. States

Machine: `rest → peek → open`. `Esc` / loss of key → `rest`. Pointer leaves →
`rest` after 240 ms (never from `open`). Pointer enters → `peek` after 60 ms.

- **rest** — hardware-fused black bar; two wing metrics (`NN Working` left,
  `N.N ev/s` right); static; orb stopped (fps 0, one static render per status
  change); no ambient loops.
- **peek** — +120 px × +44 px; count + label only (`NN — In flight`).
- **open** — 560×640, r 36, e 12; cards rise 8 px with 40 ms stagger;
  ambient loops live; haptic `alignment` once per open.
- **open · mixed / empty / failed / all-done / idle / big** — all six designed.
  Empty shows `Nothing in flight` + the literal copyable command chip
  (`notchd run-plan examples/two-by-two.json`), never a fake run button.
- **boot** — `Waiting for notchd — subscribe pending` until the first snapshot.

## 4. Hierarchy (what you see, in order)

1. Orb `NN` (88 px, weight 500, −.045em) + `In flight` mono label.
2. `Events / sec` Heartbeat dot-matrix field (240 ms sample, 9 rows × 11 px).
3. `Tree — N sessions` list rows: status dot · title · mono meta · elapsed.
4. `Load` 2 px meter; `eps` NumberRoll in the header.
5. Tiles (radar / grid / drift) — calm texture, one glance each.
6. Footer: `Notch v0.1 — Orchestrator` / `Sweep temp orphans`.

Spacing levels: 4/8 inside a group · 12/16 between groups · hairlines between zones.
Type scale only: 88 / 40 / 20 / 13 / 10.5 mono (FONT.md §6). Radii 8 · 16 · 28 · 36 · 999.

## 5. Tokens used (from tokens.ts — nothing invented)

Colors: `void` (shell stays pure #000 — the notch is hardware), `panel #171826`
(material card body, DESIGN.md v1.1, flat, zero slope), `core`, `navy`, `indigo`,
`blue`, `peri`, `lilac`, `mist`, `violet`, `orchid`, `white`, `red`, `pink`,
`ink`. GLOW presets and PAL wave palettes verbatim from DESIGN.md §3.
EASE out/inOut/snap · MS 80/140/220/320/420 · WAVE .30/.62/1.15 ·
SPRING open 380/34 · hover 520/38 · press 700/42 · digit 260/26.
Space: 4 pt grid; hairlines 1 px.

## 6. THE ONE novel idea

**Light is the status sentence.** The popover has exactly one soft thing in it —
the light — and the light is load-bearing: rim color = worst status (orb, floor,
heartbeat head and grid scan all read from the same `top` status), floor level =
normalized load rendered as a number through `@property --lvl` so the glow rises
like liquid, breathing = liveness, and drifting real session IDs behind the
content are *grabbable* (click → `focus` → herdr pane). The interface is the
instrument; the text is the calibration marks. Swap the logo and nothing on this
screen survives — which is the test.

(Supporting move, not a second idea: `Tree` rows carry their own elapsed clock;
nothing else in the popover is a timer.)

## 7. Deletion log (what was refused)

- No pill backgrounds in the header (the ref-2 pill is the orb's arc device, not ours).
- No fake "Run plan" button — the wire has no such UI message; the honest
  affordance is the copyable command chip.
- Zone 3 right side is a static `Sorted by id` (non-interactive = honest), the
  duplicate footer CLEANUP was deleted; one sweep affordance lives in the footer.
- No gradient text, no drop shadows, no second accent, no fourth font, no emoji.
- No shadcn, no component kits, no icon fonts. One hand-authored stroke family
  (1.75 px, round caps) is the fallback; primary glyphs are on-device SF Symbols
  (`invoke('symbol')`) through the `.sym` mask + `currentColor` — the white-hover
  flip works for free. SF rasterization itself is UNVERIFIED until Agent C's
  Rust lands (the empty-string → stroke-fallback path is what dev screenshots
  exercise, and it is correct).
- No NumberFlow — DESIGN.md §9.1 wins (per-digit spring columns + velocity blur).
- TREE keeps `Sorted by id` order (brief); severity sort was rejected as scope creep.
- Failed rows: `Re-prompt` posts `{type:'reprompt', id, text:'resume'}` — bridge
  requires a text; `resume` is the smallest honest imperative.

## 7b. Stack mapping (vanilla → React + Tauri)

- `postMessage/__push/__logs` → `@tauri-apps/api` `listen('snapshot' | 'logs')`
  + `invoke('set_frame'|'kill'|'reprompt'|'logs'|'focus'|'cleanup'|'haptic'|'snapshot')`.
  Object shapes are bridge.md §1.3 verbatim (camelCase). Browser `?demo=` mode replaces
  the host with a seeded in-page provider emitting the same shapes. Event/command
  names are UNVERIFIED until Agent C lands (single `isTauri()` gate in lib/bridge.ts;
  one-line change if names drift). `window.__ready` is set post-mount (bridge.md §3).
- DESIGN.md `Spring`/`ticker` kept for canvas rAF paths; React-visible size motion
  uses `motion/react` with the same stiffness/damping and EASE.out.
- Shell morph stays JS-driven clip-path spring (DESIGN.md §5); React owns mount/unmount.
- Row white-hot wipe stays the DESIGN.md clip-path CSS with pointer-entry marker.
- WebGL2 (orb + wave) stays plain TS classes behind refs; `useEffect` owns lifetime;
  zero leaked rAF/contexts; wave context exists only while a wave plays.
- `__waveTest()` window hook (browser mode only, never in Tauri) fires the
  whole-card wave for preview verification.
- Tailwind v4 (`@import "tailwindcss"`, `@theme` mapping DESIGN tokens) for
  layout/spacing/positioning; every named surface class stays plain CSS in
  styles/base.css + styles/panel.css.

## 7c. Honesty / derivation notes

- **Drift sentence** derived only from `latencyMs` (rolling avg, may be null) vs
  rolling min. Fewer than 2 timed sessions → "Collecting step latency — first
  sample pending." + `n/2 sessions timed` in the SVG half. With data:
  `Step latency never drifted more than X.XX% across N sessions.` Bars are drawn
  from rolling min (A) to each session's avg; last bar full white; A/B dashed
  lines labeled `A 0.00%` / `B X.XX%`.
- **Load**: `min(1, eps / 40)` (40 ev/s full scale).
- **Elapsed** ticks in place, plain mono, never NumberRoll (identifiers don't roll).
- **Failure reasons** map bridge codes: `stuck → Stuck — watchdog`,
  `spawn_failed → Spawn failed`, `blocked_waiting → Blocked — waiting on input`,
  unknown codes pass through as words (snake/underscores → spaces); null → `Failed`.
- **Kill** posts and marks the row `Kill / sent` until the next snapshot — the
  daemon owns truth; rows are never removed locally.
- **Focus** posts `focus` → herdr pane. Rows' click = focus.

## 8. Screenshot review — pass 1 (as built, first look)

Findings (from `*-p1.png`):
1. Floor glow rendered in rest/peek — leaked a red sliver under the hardware bar.
   **Fix:** `.shell[data-mode='rest'|'peek'] .floor { opacity: 0 }`.
2. `.fid` (floating IDs) rendered ABOVE panel content (orb/tiles/rows/footer).
   **Fix:** `.panel-content { z-index: 10 }`; fids stay z 1 behind content.
3. Zone 3 showed ~1.3 rows; elapsed column clipped at panel edge.
   **Fix:** rows 48 → 44 px; drift tile/svg compacted; drift zone padding trimmed;
   the list is a scroll region with a bottom fade (mask-image).
4. EMPTY state content overflowed upward, colliding with `Sorted by id` and the
   drift card. **Fix:** compact column, gap 2, caption line later deleted.
5. Wave-end cleanup hid `oldEl` — with oldEl === newEl (zone 1) the orb zone
   disappeared permanently after a wave. **Fix:** caller restores `zone.hidden =
   false` when the wave resolves (test: `wave-mid-mixed`).
6. Wing metrics: two-line block overflowed the 32 px bar visually. **Fix:**
   centered wing stack, quieter label, pure-black shell stays clean.

## 9. Screenshot review — pass 2 (second look, after fixes — "someone else made it")

Findings (from `*-p2.png`, `*-p3.png`, `*-p4.png`):
1. **Card law compliance (delta 2):** rows/tiles/cards/logs drawer were palette-
   correct but not yet the mandated material. **Fix:** all card bodies →
   `rgba(23,24,38,.78)` + `backdrop-filter: blur(22px) saturate(1.35)`; radar tile
   keeps its one sanctioned DESIGN.md §9.4 blue gradient (the only gradient in a
   card; DELETING it breaks the radar's sea/identity language — flagged).
   Rows gained an inset 1 px hairline (light only) so cards delineate on the void.
   Hot rows drop the blur for the solid #FFFFFF wipe.
2. **FONT.md enforcement (delta 1):** all caps everywhere (labels, wing text,
   elapsed, sweep button, empty state, failure reasons); 6.5/8/8.5/9/9.5/11 px
   sizes off the 88/40/20/13/10.5 scale; `letter-spacing` outside the size→tracking
   table; font-synthesis not pinned. **Fix:** sentence case everywhere (label idiom
   is mono sentence case 10.5, `.08em`); every off-scale size swept to 10.5/13/20/
   88/40; full grayscale-AA + ligature + `font-synthesis: none` + optical-sizing
   + `tnum`/`zero` smoothing block on html,body (verified in cropped pixels: stems
   are clean, no fringing).
3. **Sweep button** had no glyph; added trash SF Symbol (fallback stroke). Footer
   legibility over the floor glow raised to 50% white.
4. **Tailwind scan bug:** `px-3` was fused to a template-literal placeholder and
   silently dropped from the build (row content touched the row's edge). Found by
   measurement, fixed by isolating the class; scanned every other token for the
   same fused-placeholder failure — none.
5. Sidebar cap strings in aria/titles — all sentence case already; failures: none.

Slop-test verdict per block (item 19 mindset, applied): header earns its place
(whose machine + firehose + worst word), each tile earns it (swarm shape — nothing
else says it), rows earn it (the actual work), drift earns it (honesty of the
tool), footer earns it (one action). Deleted in the pass: empty-state caption
line, redundant `IN FLIGHT`-style duplication. Swap-the-logo test: every element
answers the notch's own three questions — the screen could not belong to another
product.

**Unresolved / flagged:**
- Orb exposure: DESIGN.md's presets + halo math render working/done/failed at
  level 1.0 as very bright lobes (done = near-white squircle). Implemented the
  law verbatim and flagged it; the X-band lobe pattern over the box-SDF is
  likewise intrinsic to whole harmonics on a squircle (the law mandates both).
- Orb readback for tests failed headless without `preserveDrawingBuffer` — orb is
  verified visually only; a blank orb would show in screenshots (none did).
- Slop check on `.tile--blue`: kept per DESIGN.md §9.4 (the one sanctioned card
  gradient); flagged for lead awareness under item 19's wording.
- `SF Symbol` raster path UNVERIFIED end-to-end (needs Agent C's `invoke('symbol')`);
  browser dev uses the fallback stroke family everywhere, which is what the
  screenshots show. `__waveTest` hook proves the wave runs; top-status flips
  from real snapshots would trigger it in the Tauri app.
- `SORT: ID` → `Sorted by id` (brief's literal label, kept honest as words).
- Kill doesn't collapse the row locally — the daemon owns truth; the row shows
  `Kill / sent` until the next snapshot (deviation from DESIGN.md §12's collapse,
  reasoned and flagged).
- Density: 640 px forces ~1.5–2 tree rows visible; the list is a scroll region
  with a fade. Severity sort rejected to keep the brief's `Sorted by id`.

---

## 11. v1.3 delta — round 2 (lead override, 2026-11-19): popover fixes + the dashboard

### Part 1 (notch) — fixed, screenshot-verified
- **Frame probe** (`?probe=1`, off by default): rAF ring buffer (240), flags >16.7ms,
  overlay readout avg/p95/max + jank count. Screenshots `popover-probe.png`.
- **Shell morph** now zero-layout: element box FIXED at 560×640; only the clip-path
  silhouette springs (measured: ~37% shorter frames at dsf 2 — see report).
  Pointerdown opens (no blocking work before first frame). Spring retuned k 380/34 →
  420/38 (ζ≈.93, visible motion ≈ 0.24s ≤ 300ms target).
- **Clipped pill text** ("lorking"/"ev/s" — user-called): root-caused by measurement —
  wings were centered on the screen while the (then) variable-size shell drew the
  silhouette from its own left edge; with the fixed shell both center on the same
  axis. Wing labels deleted (a number is one metric — subtraction) — wing numerals
  inset 14px inside the ear arcs, `white-space: nowrap`.
- **Supervisor list replaces the radar tile** (hairline list, `f'%'` rows, white-hot
  wipe consistent): id + state dot + charter (newest decision, honest fallbacks) +
  model + real counts (`2w 1i 0f`) + age; hot → Message / Pause-Resume / Kill-xmark
  (kill = fan out `kill` per its sessions — flagged, bridge has no kill_supervisor).
  Click → `open_dashboard`; Message → dashboard with target chip; Pause/Resume →
  `pause_supervisor`/`resume_supervisor` (demo provider toggles for real).
- **Sessions tile shows real IDs** (S1-OC01 · dot by status · title) with white wipe;
  SessionGrid/Radar modules deleted as dead code.
- **`logs` is invoke-returned** (no event) per bridge v1.1+ (my old `listen('logs')` was wrong).
- Popover header gains **Escalation chip** (red dot + count) when `escalations.length > 0`
  → opens dashboard Escalations tab via localStorage `notch.dashboardTab`.
- New task empty state: primary **New task** pill (opens dashboard); CLI hint demoted
  to a quiet mono secondary link.

### Part 2 (dashboard) — built, against canned demos, two review passes
- One window, four tabs (Tree | Multiplexer | Comms | Escalations), segmented control
  with sliding white pill (motion `layoutId`, over-damped spring). Cmd+1..4, arrow keys,
  roving tabindex, `cursor: pointer` on every interactive element, 2px focus rings.
- **Tree**: sidebar 272px (filter field, Orchestrator root row, 3-line supervisor rows —
  age, charter, `working n · idle n · failed n` — + hot icons, `+ New supervisor` /
  `+ New session` real buttons) · center (20px/500 title + mono meta; transcript — mono
  11.5px `›`-marked tool lines, quiet-90s marker fed by a client-side revision clock;
  composer — Enter sends `submit_task` (supervisorId or new supervisor), Shift+Enter
  newline, Cmd+K focuses, optional `Done when…` DoD row, model picker from
  `list_models` (fallback `hy4-flash-free` + `auto`), persisted cwd, `Assign` pill
  `E9E5FB`) · right inspector 300px (big mono id, sentence-case status, model, cwd
  middle-truncated, started, elapsed rolls 1s, `2/8` iteration hairline ink-on-lilac,
  DoD in dim quotes, files touched from `attribution`, pink conflict rows when a file
  is claimed by ≥2 sessions, controls: Re-prompt / Pause / Kill / Open in herdr).
- **Multiplexer**: responsive grid ≥240px cards (id, dot, title, goal, `Iteration n/m`,
  `lastSupervisorAction`, 3-line live tail from the shared 1s logs poller), badges
  DONE/STUCK/ESCALATED, Grid|List toggle; click → Tree + select.
- **Comms**: newest-first bus log, filter (from/to/type/preview), legend (dim = same
  supervisor, lilac = cross), type chips colored (task peri / result white / question
  lilac / handoff blue / escalate red); click → Tree + FROM selected + TO flashed.
- **Escalations**: unresolved inbox; each card = kind (sentence case) + session/sup +
  reason + age; actions **Re-prompt fresh** (composer prefilled `Fresh take on the
  goal: …`) · **Reassign** (fresh session, same supervisor+goal+DoD) · **Kill
  session**. No dismiss — clears on state change; documented in-product.
- Notch badge ↔ dashboard Escalations via localStorage handoff.

### Interaction pass (headless, 15/15 PASS)
Cmd+3/Cmd+4/Cmd+1 tabs · comms row jump (FROM selected) · re-prompt fresh → composer
prefilled + Tree · reassign → inline `Reassigned to …` confirm · pause → chip flips,
resume clears · composer Enter → `Assigned — supervisor is on it` · mux card → Tree ·
grid/list toggle · DoD row expands · sidebar filter narrows · no page errors.

### Slop audit per surface (one line each)
Header earns it (whose machine + firehose + worst word + the one alarm that matters).
TreeEarn: every element acts or informs — no decorative chrome anywhere. Sidebar
supervisor rows ARE the control plane. Inspector answers the operator's four
questions (what, how far, how proven, what touches it). Multiplexer = "which
sessions need me", no other reading survives a logo swap. Comms = the bus, nothing
else. Escalations = the unresolved inbox only. Empty dashboard = the composer,
which IS the product doing its job with zero ceremony.
