# PLAN — Notch: OpenCode orchestration control plane for the MacBook notch

Goal: a thin, strict orchestration layer on top of OpenCode + herdr (Ghostty). Nothing more.
This file is the agreed plan (the user's "Updated Plan – Full Rewrite"), restated with the concrete build decisions made for this repo.

## Reality check

- Terminal is Ghostty + herdr. Everything built here respects it and never fights it.
- OpenCode already has sub-agents. We do not rebuild them, do not reimplement tool calling, editing, bash, or the agent loop.
- Inference for everything (leaf agents and supervisors) comes from OpenCode. Free models, zero extra spend.
- What we build is a thin orchestration layer: org chart, IDs, watchdog, communication rules, cleanup.

## Architecture

```
Orchestrator (notchd — Rust, one long-lived process)
│  pure control plane. No model calls in the control path.
│  Owns: global tree, watchdog, conflict rules, message bus, attribution, cleanup.
│
├── Supervisor-1 (SV-1 — our process, talks to OpenCode via herdr)
│   ├── OpenCode session S1-OC01   (full OpenCode primary agent, black-box unit)
│   ├── OpenCode session S1-OC02
│   └── OpenCode session S1-OC03
│
├── Supervisor-2 (SV-2)
└── Supervisor-3 (SV-3)
```

- Leaf unit = a full OpenCode primary-agent session. We give it a permanent ID (`S2-OC04`). Every log line, status update, attribution carries that ID.
- OpenCode sessions NEVER talk to each other directly. Same-Supervisor sessions are coordinated by their Supervisor (it reads outputs, feeds context). Cross-Supervisor goes through the Orchestrator (the bus).
- Intelligence budget stays inside OpenCode. We add structure, identity, supervision, hygiene.
- Supervisor decisions (re-prompt logic) may use a dedicated lightweight OpenCode call owned by the Supervisor — still free. Deterministic policy is the default; the OpenCode-backed decider is optional and off by default for the prototype.

## Terminal integration (Ghostty + herdr) — non-negotiable

- Never open a new Ghostty window. Never create a competing multiplexer.
- The existing Ghostty instance with herdr running is the workspace. herdr is the workspace authority; we may request panes and launch OpenCode inside them, nothing more.
- Spawning a session = `herdr pane split --direction right --cwd <dir>` (or a new controlled tab when splitting is not possible) then `herdr agent start <name> --kind opencode --pane <id>`.
- Existing panes/tabs stay untouched. `herdr pane list` is read-only telemetry.
- "Show me how my agents are doing" = `herdr agent focus <target>` (or open one new controlled pane). Never scatter windows.

## Tech stack (revised 2026-11-19 — user override)

Latest everything, user-mandated: **Tauri v2 + Rust + React 19 + TypeScript (strict) + Tailwind CSS v4 + Vite 8**.

- **Rust (`crates/notchd`)**: the engine — orchestrator, supervisors, ID registry, bus, watchdog, attribution, cleanup, herdr client, snapshot producer. Both a library (embedded in the Tauri process) and a CLI/daemon (`notchd serve`, headless ops).
- **Tauri v2 (`app/`)**: the macOS surface. Borderless, transparent, always-on-top window fused to the notch (NSWindow level/collectionBehavior set from Rust via objc2 — no Swift, no WKWebView authorship; WKWebView is Tauri's internal engine). Tray (NSStatusItem-equivalent) = menu bar. Zero webview permissions beyond event listen + the command allowlist.
- **React 19 + TypeScript + Tailwind v4 (`ui/`, Vite 8)**: the popover. Implements DESIGN.md exactly — tokens, glow system (WebGL2), springs, white-hot rows, wave. `motion/react` for component motion; DESIGN.md values throughout. Loads inside the Tauri window; dev mode (`vite dev`) shows the same panel fed by demo snapshots.

- **Tauri commands + events** (in-process, primary) — the frontend calls `invoke('kill', …)`; the backend emits `snapshot` on change.
- **Unix socket (headless secondary)** — `notchd serve` keeps the NDJSON socket for CLI/ops use; unchanged from v1 of this doc.
- Socket path: `${NOTCH_SOCK:-~/Library/Application Support/notch/notch.sock}`; state dir `~/Library/Application Support/notch/`.
- Full wire contract: **docs/bridge.md** (single source of truth for all three parts).

## Build order and ownership (3 sub-agents + lead)

| # | Step | Owner | Dir | Verify |
|---|------|-------|-----|--------|
| 1 | Rust orchestrator: process model, IDs, bus, watchdog | Agent A | `crates/notchd/` | `cargo test` (tests-first) |
| 2 | OpenCode launcher + status reader + kill (herdr client) | Agent A | same | tests with fake herdr; live paths read-only |
| 3 | Tauri layer: notch window geometry (objc2), tray, IPC commands/events | Agent C | `app/` | `cargo test`, hidden-launch if possible |
| 4 | Supervisor logic: spawn N, re-prompt on failure, aggregate, escalate | Agent A | `crates/notchd/` | tests + `notchd run-plan` demo |
| 5 | Attribution + conflict rules + auto-cleanup hooks | Agent A | `crates/notchd/` | tests |
| 6 | Cross-Supervisor messaging through the Orchestrator | Agent A | `crates/notchd/` | tests |
| 7 | Popover (React + Tailwind, DESIGN.md brutal) | Agent B | `ui/` | headless screenshots of every state |

Overlap rule: dirs are disjoint. `docs/bridge.md` is lead-owned; change requires lead sign-off.

## UX (two surfaces + the dashboard the user demanded 2026-11-19)

1. **Menu bar + notch popover** — live counts (working/idle/failed/total). Click any supervisor or OpenCode session ID → status, last action, kill / re-prompt / view logs. Daily driver. Gains one quiet affordance: "Open dashboard" in the header.
2. **Standard macOS dashboard window** (a regular Tauri window in the same app) — full tree with IDs, status, filter, a **task composer**, per-session transcript + real re-prompts, kill/focus/cleanup, attribution + conflicts. This is where you *talk to the system*: type the job, Supervisor + session appear with permanent IDs.

When you ask “show me how my agents are doing,” it focuses the relevant herdr pane or the dashboard; it never scatters windows.

## What we explicitly do NOT build

Agent runtime / tool loop / sub-agents; interception of OpenCode internals; new terminal or multiplexer; screenshot/computer-use loops; negotiation protocols; cloud/teams/plugins/themes/marketplace; permanent disk mess; new Ghostty windows.

## Prototype acceptance (the demo)

1. `notchd serve` running; `Notch.app` in menu bar shows counts.
2. `notchd plan examples/two-by-two.json` → Orchestrator spawns SV-1 and SV-2 → each spawns 2 OpenCode sessions (IDs S1-OC01…S2-OC02) in new herdr panes in the **current** workspace — no windows.
3. A stuck session (watchdog: no `revision` change + still `working` past timeout) gets killed by the orchestrator, cleaned, marked failed in the popover.
4. Done sessions swept; temp dirs removed; global sweep works.
5. "Show me" → `herdr agent focus S1-OC01` focuses that pane.
6. Popover: every DESIGN.md state exists (rest/peek/open), white-hot hover, orb glow = state, numbers roll, heartbeat scrolls, no rejection-checklist hits.

## Risks / failure modes

- herdr socket unavailable → notchd runs in `--offline` (fake clock + fixture herdr); UI still demoable; report UNVERIFIED for live paths.
- WKWebView WebGL2 unavailable → UI degrades to static glow (documented), mark UNVERIFIED for glow checks.
- Screen Recording permission missing → GUI capture UNVERIFIED; rely on `swift test` + hidden launch + accessibility API. Name the exact permission if blocked.
- Duplicate spawns after daemon restart → ID registry + pane client IDs make spawn idempotent per plan hash.
