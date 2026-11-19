# STRUCTURE — notch popover (Agent B, ui/)

Pre-build structure pass, per DESIGN-RULES.md process step 1–4. Written before
any UI code. Stack (lead override, 2026-11-19): **Tauri v2 + Rust shell, React 19
+ TypeScript strict, Tailwind CSS v4 (CSS-first), Vite 8, motion (motion/react).**
DESIGN.md remains the visual law; its "vanilla TS, no framework" line is void;
everything else stands. Revised in place as decisions change; the two review
passes are at the bottom.

## 1. What this is, physically

The popover is the menu-bar control plane made physical: a black shell fused
to the hardware notch that hangs below the menu bar like the laptop's own chin.
It is not a window, not a dashboard, not a "card in a page". It is the notch
growing a screen. One glance answers: **how many of my agents are working right
now, and is anything stuck?**

## 2. Primary thing, one idea per view

Primary thing: **the working count, embodied as light.** The orb (ref 2) is
the one large element; its rim color = worst status, its radius = nothing, its
level = load. Everything else — heartbeat, grid, radar, drift, IDs, rows — is
the fine print under that sentence. If you see one thing, you see `IN FLIGHT`
and feel the load.

One idea per zone:
- Header: whose machine this is + the firehose (eps) + the worst-status word.
- Zone 1 (orb + instruments): how much work is in flight, and how hot the bus is.
- Zone 2 (tiles): the shape of the swarm — supervisors as a radar, sessions as a dot matrix.
- Zone 3 (list): the actual sessions, scannable, keyboard-navigable, actionable in place.
- Zone 4 (drift): is step latency honest this run? (slowness, drift, honesty of the tool)
- Footer: the one housekeeping action (sweep temp orphans). Nothing else.

## 3. States

Machine: `rest → peek → open`, `Esc`/click-outside/loss-of-key → `rest`,
pointer-leave after 240 ms → `rest` (never from `open`).

Per state, all designed (not defaulted):
- **rest** — hardware-fused black bar; two wing metrics (`NN IN FLIGHT` left,
  `N.N EV/S` right); static; orb stopped (fps 0); nothing loops (DESIGN.md §12).
- **peek** — +120 px × +44 px; count + label only; orb at 30 fps when started.
- **open** — 560×640, r 36, e 12; cards rise 8 px with 40 ms stagger; ambient
  loops live; haptic `alignment` once per open.
- **open · mixed** (real swarm, 3–10 working) — the happy-path resting state.
- **open · empty** — zero sessions. `NOTHING IN FLIGHT` + literal copyable chip
  `notchd run-plan examples/two-by-two.json`. No fake run button (bridge has no
  such message from UI). Affordance honesty over a dead control.
- **open · failed** — a session is stuck. Hollow pink dot, `STUCK — WATCHDOG`,
  orb → red failed preset, floor glow → red, heartbeat keeps scrolling, Re-prompt
  emphasized on the failed row. Haptic `levelChange` when failure is *first seen*.
- **open · all-done** — every session done; `00 IN FLIGHT` looks wrong, so the
  empty-of-work-but-not-zero-sessions case keeps the tree list (done rows stay,
  status `done`, lilac dots) and eps decays to idle. Orb → done preset (white rim).
- **open · idle** — supervisors parked, sessions idle, eps < 1. Grey-indigo orb.
- **open · big** — 12 sessions / 3 supervisors; the list scrolls; the grid
  fills; nothing breaks layout. Density earns its keep.
- **boot** — before the first `__push`, the panel shows
  `WAITING FOR NOTCHD — SUBSCRIBE PENDING` in place of the list. Transient,
  honest; replaced on first snapshot.

## 4. Hierarchy (what you see, in order)

1. Orb: `NN` (88 px, weight 500, −0.045em) + `IN FLIGHT` mono label. Owns the left half.
2. `EVENTS / SEC` Heartbeat dot-matrix field (left→right scroll, head dot). Owns top-right.
3. `TREE — N SESSIONS` list rows: status dot · title · mono meta · elapsed.
4. `LOAD` 2 px meter; `EPS` NumberRoll in the header.
5. Tiles (radar / grid / drift) — calm texture, one glance each.
6. Footer bar: `NOTCH v0.1 — ORCHESTRATOR` / `SWEEP TEMP ORPHANS`.

Spacing levels: 4/8 inside a group · 12/16 between groups · hairlines between zones.
Type scale only: 88 / 20 / 13 / 10.5-mono. Radii: 8 · 16 · 28 · 36 · 999.

## 5. Tokens used (from tokens.ts — nothing invented)

- Colors: `void` (not used as bg — shell stays pure #000), `panel #1E1F2E` (tiles/cards),
  `core #14153A`, `navy`, `indigo`, `blue #4455EE`, `peri`, `lilac`, `mist`, `violet`,
  `orchid`, `white`, `red #FF4D6D`, `pink #FFB3C1`, `ink #0A0B1E` (text on white hot state).
- GLOW presets working/idle/failed/done verbatim from DESIGN.md §3.
- PAL waves six stops per status verbatim.
- EASE out/inOut/snap · MS 80/140/220/320/420 · WAVE lean .30 / cross .62 / settle 1.15 ·
  SPRING open 380/34 · hover 520/38 · press 700/42 · digit 260/26.
- Type: Inter variable (bundled woff2) + ui-monospace/SF Mono system stack. Weights 400/500/600.
- Space: 4 pt grid; hairlines 1 px.

## 6. THE ONE novel idea

**Light is the status sentence.** The popover has exactly one soft thing in it —
the light — and the light is load-bearing: rim color = worst status (orb, floor,
heartbeat head, grid scan all read from the same `top` status), floor level =
normalized load rendered as a number through `@property --lvl` so the glow rises
like liquid, breathing = liveness, and drifting real session IDs behind the
content are *grabbable* (click → `focus` message → herdr pane). The interface
is the instrument; the text is the calibration marks. Swap the logo and nothing
on this screen survives — which is the test.

(Supporting move, not a second idea: `TREE` rows carry their own elapsed clock;
nothing else in the popover is a timer.)

## 7. Deletion log (what I refused to build)

- No pill backgrounds in the header (DESIGN.md pill is ref-2's arc device, not ours).
- No fake "Run plan" button in the empty state — the wire has no such message
  from UI; the honest affordance is a copyable literal command chip. DELETED the button.
- Zone 3 right-side `CLEANUP` text button — duplicates footer `SWEEP TEMP ORPHANS`
  (one action, one place). Replaced with static `SORT: ID` (non-interactive = honest).
- No gradient text, no drop shadows, no second accent, no fourth font, no emoji.
- `SORT: ID` is static, not a fake control.
- Failed-row Re-prompt posts `reprompt` with `text: "resume"` — the wire requires
  a text; `resume` is the smallest honest imperative. Logged as an assumption.
- NO shadcn, NO component kits, NO lucide — one hand-drawn stroke family
  (1.75 px, round caps), outline at rest / fill when hot, original glyphs.
- No NumberFlow — DESIGN.md §9.1 wins on the numeral mechanic (per-digit spring
  columns + velocity blur); TASTE.md's flow applies via the same mechanism.

## 7b. Stack mapping (vanilla → React, decided)

- `postMessage/__push/__logs` → `@tauri-apps/api` `listen`/`invoke`
  (`listen('snapshot')`, `listen('logs')`, `invoke('set_frame'|'kill'|'reprompt'|
  'logs'|'focus'|'cleanup'|'haptic'|'snapshot')`). Browser `?demo=` mode replaces
  Swift with an in-page provider emitting the same object shapes. Event names are
  UNVERIFIED until Agent C's Rust side lands; if it emits different names, one
  constant in `lib/bridge.ts` changes, nothing else.
- DESIGN.md `Spring` class → kept for canvas rAF paths (orb, heartbeat, grid,
  specular, NumberRoll strips); React-visible size/position motion uses
  `motion/react` with the same stiffness/damping (380/34 · 520/38 · 700/42 · 260/26)
  and EASE.out cubic-bezier(.16,1,.3,1).
- Shell morph stays JS-driven clip-path spring (DESIGN.md §5), React owns only
  mount/unmount + mode state; sweep re-render never fights the 60 fps morph.
- Row white-hot wipe stays the DESIGN.md clip-path CSS with pointer-entry marker
  (directionality), imperative effect on the list ref — not React state.
- WebGL2 modules stay plain TS classes behind refs; `useEffect` owns lifetime,
  zero leaked rAF/contexts; wave context exists only while a wave plays.

## 8. Honesty / derivation notes

- **Drift sentence** derived only from `latencyMs` (rolling avg per session, may be
  null) vs rolling min. Fewer than 2 sessions with latency data →
  "Collecting step latency — first sample pending." No invented precision.
  With data: `Step latency varied X.XX% across N sessions — best L ms, worst H ms.`
  Bars: each session drawn from rolling-min (A) to its own avg; last session full white.
- **Measured-steps denominator** is "sessions with timing data", never a fabricated
  step counter.
- **EPS normalization** for load/heartbeat: `min(1, eps / 40)` (40 e/s = full scale).
- **Failure display** maps bridge reasons: `stuck → STUCK — WATCHDOG`,
  `spawn_failed → SPAWN FAILED`, `blocked_waiting → BLOCKED — WAITING ON INPUT`,
  unknown → uppercased raw reason. Never a bare code.
- **symbol://** images have a CSS fallback: `<img onerror>` swap is not possible for
  CSS masks; instead `.sym` is a mask on a `<i>` — when the scheme 404s, the mask
  paints nothing (invisible, not broken). Dev-only fallback dot is added in dev.html
  only (body gets `data-dev`, `.sym` there renders as a 1 px dot). Real path
  UNVERIFIED until Swift scheme lands (Agent C).
- **Kill** does not remove the row locally (the daemon owns truth); the row enters
  `data-killing` (dot pulses, `KILL SENT` replaces elapsed) until the next snapshot.
- **Reduced motion**: springs → 120 ms opacity fades; no breathing; no ambient loops;
  no wave (crossfade only); heartbeat/grid draw single frames.

## 9. Screenshot review pass 1

_(filled after first shoot — see bottom of file)_

## 10. Screenshot review pass 2

_(filled after second shoot — see bottom of file)_
