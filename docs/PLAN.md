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

## Tech stack

- **Rust (`crates/notchd`)**: core library + CLI/daemon. Orchestrator, supervisors, ID registry, message bus (newline-JSON over Unix socket), watchdog, attribution, cleanup, herdr client, snapshot producer. Long-lived daemon: `notchd serve`.
- **Swift (`app/`, SwiftPM)**: the macOS surface. NSPanel notch host with concave ears, `NSScreen` notch geometry (never hardcoded; 200×32 fallback), menu-bar NSStatusItem with live counts, WKWebView hosting the UI, bridge (JS ↔ Swift ↔ notchd), haptics, SF Symbols via custom URL scheme. IPC to notchd over the same Unix socket.
- **TypeScript (`ui/`, vanilla + esbuild)**: everything in `WKWebView`. Implements DESIGN.md literally — tokens.ts, motion engine, glow system (WebGL2 orb + wave), shell clip-path morph, popover surface. No framework.

## IPC — Unix socket, newline-delimited JSON

Path: `$XDG_RUNTIME_DIR/notch/notch.sock` fallback `~/Library/Application Support/notch/notch.sock`.
Full wire contract: **docs/bridge.md** (the single source of truth for all three parts).

- Web → Swift: `webkit.messageHandlers.notch.postMessage(...)`
- Swift → daemon: socket commands `snapshot | subscribe | spawn | kill | reprompt | logs | focus | cleanup | ping`
- daemon → Swift: `{ "event": "snapshot", ... }` pushed on subscribe; acks/errors per command.
- Swift → Web: `window.__push(snapshot)` via `WKUserScript` / `evaluateJavaScript`.

## Build order and ownership (3 sub-agents + lead)

| # | Step | Owner | Dir | Verify |
|---|------|-------|-----|--------|
| 1 | Rust orchestrator: process model, ID registry, bus, watchdog, tree, event log | Agent A | `crates/notchd/` | `cargo test` (tests-first on logic/concurrency) |
| 2 | OpenCode launcher + status reader + kill (herdr client) | Agent A | same | integration test with real `herdr` read-only + fake-herdr fixture for destructive paths |
| 3 | Swift layer: notch detection, panel, Ghostty/herdr focus, menu bar, WKWebView, bridge | Agent C | `app/` | `swift build`, `swift test` (pure geometry/state logic), hidden launch + window capture |
| 4 | Supervisor logic: spawn N sessions, re-prompt on failure, aggregate, escalate | Agent A | `crates/notchd/` | tests + `notchd plan examples/two-by-two.json` demo |
| 5 | Attribution + conflict rules + auto-cleanup hooks | Agent A | `crates/notchd/` | tests |
| 6 | Cross-Supervisor messaging through the Orchestrator | Agent A | `crates/notchd/` | tests |
| 7 | Notch popover + tree view polish (DESIGN.md, brutal) | Agent B (+lead) | `ui/` | headless screenshots of every state |

Overlap rule: dirs are disjoint. `docs/bridge.md` and `ui/` bundle output path are frozen once written — change requires lead sign-off.

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
