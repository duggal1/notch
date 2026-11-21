# bridge.md — the notch wire contract (frozen once written)

Three parties, one contract. If any of the three disagrees with this file, the implementation is wrong.

Version: **1.4** (2026-11-20). v1.1–v1.3 history below still applies. **v1.4: additive real-time WebSocket contract, proven 2026-11-20 — `fleet` frame (all managed units: orchestrator/supervisors/sessions with id/status/pane/workspace/last action/revision), WS `focus` command (herdr pane focus only, never a window), plus an explicit opt-in browser-compatible handshake. Default strict mode still rejects any `Origin` and requires `Authorization: Bearer <token>` + exact `Host`; with `NOTCH_WS=1 NOTCH_WS_BROWSER=1`, `ws://127.0.0.1:<port>/?token=<64hex>` is accepted with `Origin` present ONLY when the token is unguessable and Host exact (browsers cannot set custom headers or hide Origin). Measured p95: hello→fleet 0.284 ms, status→fleet 3.986 ms, command→ack 0.099 ms, 100-client fan-out 0.090 ms, browser handshake 16.218 ms. Webview transport stays Tauri `listen('snapshot')`; WS is for external clients.** Changes require lead sign-off.

---

## 0. In one picture

```
Webview (React, ui/)             Tauri backend (Rust, app/)          notchd engine (crates/notchd/)
        │                                  │                                  │
        │ invoke('kill', {id})             │                                  │
        │─────────────────────────────────▶│  direct lib call (in-process)    │
        │                                  │─────────────────────────────────▶│
        │                                  │◀─────────────────────────────────│
        │ listen('snapshot', cb)           │  app.emit("snapshot", snap)      │
        │◀─────────────────────────────────│                                  │
        │                                  │
        └──── optional headless path: `notchd serve` NDJSON Unix socket (§2) ─┘
```

---

## 1. Primitives

### 1.1 Status — exactly DESIGN.md's four

`"working" | "idle" | "failed" | "done"`

`Status = 'working' | 'idle' | 'failed' | 'done'` — no fifth state anywhere. Empty slots are `null`.

Mapping from herdr (`agent_status`): `working→working`, `idle→idle`, `done→done`, `blocked→failed` (reason `blocked_waiting`), `unknown→idle`. Plus daemon-side overrides: watchdog-killed ⇒ `failed` (`reason: "stuck"`), spawn failure ⇒ `failed` (`reason: "spawn_failed"`).

### 1.2 IDs

- Supervisor: `SV-<n>` (1-based, allocated by orchestrator).
- Session: `S<supIndex>-OC<nn>` (supIndex = owning supervisor's 1-based number; nn = 1-based per supervisor). Example `S1-OC02`, `S2-OC04`.
- Orchestrator: `ORCH`.
- Registry (`~/Library/Application Support/notch/registry.json`) is the only allocator. IDs are permanent and never reused, never recycled after cleanup.

### 1.3 Snapshot (daemon → Swift → JS)

```jsonc
{
  "version": 1,
  "generatedAt": 1763520000000,          // ms since epoch
  "orchestrator": { "id": "ORCH", "status": "working" },
  "supervisors": [
    { "id": "SV-1", "status": "working",
      "working": 2, "idle": 0, "failed": 0, "done": 0,
      "title": "Refactor auth middleware",     // additive v1.3.1: supervisor display charter; lead sign-off 2026-11-20
      "paused": false                           // additive v1.3: pause gates the decision loop only
    }
  ],
  "sessions": [
    {
      "id": "S1-OC01", "sup": "SV-1", "status": "working",
      "title": "Refactor auth middleware",   // short, imperative, no filler
      "task":  "Refactor auth middleware",   // original prompt head, single line
      "cwd":   "/Users/x/proj",
      "paneId": "w80:p2",
      "startedAt": 1763520000000,
      "lastAction": "editing src/auth.rs",   // best-effort, single line, from pane output
      "lastRevision": 12,                    // herdr pane revision (heartbeat)
      "latencyMs": 840,                      // rolling avg step latency, or null
      "failureReason": null,                 // string when status == "failed"
      "goal": "…",                           // v1.3 optional store: the job text
      "definitionOfDone": "tests pass + p95 < 200ms",  // v1.3 optional, user-supplied DoD
      "iterations": 2,                       // v1.3 optional; re-prompts used on this session
      "maxIterations": 8,                    // v1.3 optional; budget (default 8)
      "wallClockDeadline": 1763523600000,    // v1.3 optional; ms epoch
      "lastSupervisorAction": "re-prompt: tests not run",  // v1.3 optional + reason, one line
      "model": "auto"                        // v1.3 optional; "auto" = harness default
    }
  ],
  "eps": 6.5,                                // bus events/sec, float
  "conflicts": [ { "file": "src/auth.rs", "sessions": ["S1-OC01", "S1-OC02"] } ],
  "attribution": [ { "session": "S1-OC01", "file": "src/auth.rs" } ],   // cap 24, newest first
  "decisions": [ { "at": 1763520000000, "supervisor": "SV-1", "session": "S1-OC01", "action": "reprompt"|"done"|"escalate"|"verify"|"assign", "reason": "…" } ],  // v1.3, newest-first, cap 12
  "escalations": [ { "id": "esc_1", "session": "S1-OC01", "supervisor": "SV-1", "kind": "stuck"|"loop_detected"|"budget_exceeded"|"blocked", "reason": "…", "at": 1763520000000 } ],  // v1.3, unresolved only
  "busMessages": [ { "id": "msg_1", "from": "S1-OC01", "to": "S1-OC02", "type": "task"|"result"|"question"|"handoff"|"escalate", "preview": "…(≤140 chars)", "at": 1763520000000 } ],  // v1.3, newest-first, cap 30
  "warnings": []
}
```

Notes:
- Extra optional fields may be added; none may be removed or renamed in v1. All v1.3 fields are optional — v1.2 consumers keep working.
- `supervisors` is ordered by id; `sessions` is ordered by id.
- Empty tree is legal: `"supervisors": [], "sessions": [], "eps": 0`.
- JS receives this object verbatim via `window.__push(snapshot)` — keys are `camelCase`.
- v1.3 optional blocks always present when data exists; the UI must not require them.

---

## 2. Daemon socket API

Frame = one NDJSON object per line, either direction. Server sends `{ "event": "hello", "version": 1, "pid": 1234 }` on connect.

### Client → daemon

| cmd | fields | reply |
|---|---|---|
| `ping` | — | `{ "event": "ack", "cmd": "ping", "ok": true, "pong": 1763520000000 }` |
| `snapshot` | — | `{ "event": "snapshot", "snapshot": {…} }` |
| `subscribe` | — | `{ "event": "ack", "cmd": "subscribe", "ok": true }` then a fresh `snapshot` event immediately, then on every change (debounced 100 ms) |
| `run_plan` | `path: string` | spawns orchestrator plan; acks `{ ok, spawned: ["SV-1","SV-2"] }` or `{ ok:false, error }` |
| `submit_task` | `task: string, cwd?: string, title?: string, definitionOfDone?: string, model?: string, supervisorId?: string` | v1.3. If `supervisorId` is set, routes into that existing supervisor (one more session under it); else allocates `SV-<n>` (title = task head ≤ 48 chars). Session `S<n>-OC<nn>` with `task` as the OpenCode prompt; `definitionOfDone` stored on the session and shown in UI; `model` passed to the harness at spawn. Returns `{ ok, supervisorId, sessionId, error? }`. `cwd` validated (must exist, be a dir). Deterministic skeleton — judgment happens only inside OpenCode calls (§8). |
| `message` | `from: string, to: string, type: "task"\|"result"\|"question"\|"handoff"\|"escalate", body: string` | v1.3. Bus message, logged with FROM/TO, attributed, delivered to the target session as a normal prompt via `herdr agent prompt <target> <body>` (never a direct session→session channel). Ack `{ ok, messageId?, error? }`. |
| `pause_supervisor` | `id: string` | v1.3. Stops the supervisor's re-prompting/new-work decisions for its sessions; live sessions keep running; `status` preserved (`paused: true` in snapshot). Ack. |
| `resume_supervisor` | `id: string` | v1.3. Inverse of pause. Ack. |
| `kill_switch` | — | v1.3. One click: closes every session the daemon manages (`herdr pane close <pane>`), marks them `failed` with reason `kill_switch`, runs cleanup. Ack with `closed: n`. |
| `list_models` | — | v1.3. Returns `{ models: string[] }` parsed from `opencode models` (read-only), cached 5 min. On failure, `{ models: [], error }` — UI falls back to its default entry. |
| `logs` | `id: string, tail?: number` (default 200) | `{ "event": "logs", "id": …, "lines": […] }` |
| `reprompt` | `id: string, text: string` | ack; `herdr agent prompt <target> <text>` |
| `cleanup` | `id?: string` | one session or global sweep; ack with `removed: n` |
| `focus` | `id: string` | ack; `herdr agent focus <target>` — moves focus to that pane, never opens a window |
| `kill` | `id: string, reason?: string` | ack; kills via `herdr pane close <paneId>` (positional arg); session → `failed` with reason; cleanup hook runs |
| `spawn` | `supervisor: string, prompt: string, cwd: string, title: string` | ack with allocated `session.id` |
| `status` | — | `{ "event": "status", "pid": …, "sessions": n, "uptimeS": s }` |

Errors: `{ "event": "ack", "cmd": …, "ok": false, "error": "<machine-readable code or plain sentence>" }`.

### herdr invocation surface (what the daemon is allowed to call)

`herdr pane list` · `herdr agent list` · `herdr pane split --direction right --cwd <cwd>` · `herdr agent start <name> --kind opencode --pane <paneId>` · `herdr agent prompt <target> <text> [--wait]` · `herdr agent read <target>` · `herdr agent wait <target> --until idle --until done --until blocked` · `herdr pane close <paneId>`   ← positional argument, NOT `--pane` (v1.1.1 correction from Agent A's report; passing `--pane` errors out and the watchdog could never kill) · `herdr agent focus <target>` · `opencode api GET /api/session`   ← v1.3: read-only activity signal (§8), via the same injectable runner (command: `opencode`) · `opencode models` (read-only, cached 5 min) · `opencode run --model <m> <prompt>` ← v1.3: supervisor decider calls only (§8; free models; black-box; never in the control path of the orchestrator itself).

Nothing else. The daemon never creates windows, never touches panes it doesn't own, never runs `open` or `screencapture`. No filesystem access to OpenCode's internals.

---

## 3. JS ↔ Tauri (what ui/ may call, what app/ must implement)

### ui → app: `invoke(<cmd>, { … })` — lowercase snake_case, camelCase fields

```jsonc
{ "cmd": "snapshot" }                       // → snapshot object (same shape as daemon)
{ "cmd": "kill", "id": "S1-OC01", "reason"?: "…" }
{ "cmd": "reprompt", "id": "S1-OC01", "text": "…" }
{ "cmd": "logs", "id": "S1-OC01", "tail"?: 200 }      // returns { id, lines: string[] }
{ "cmd": "focus", "id": "S1-OC01" }                   // focuses the herdr pane, never a window
{ "cmd": "cleanup", "id"?: "S1-OC01" }                // id → one session; absent → global sweep; returns { removed: n }
{ "cmd": "spawn", "supervisor": "SV-1", "prompt": "…", "cwd": "/…", "title": "…" }  // → { id }
{ "cmd": "run_plan", "path": "/…/two-by-two.json" }   // → { ok, spawned: […] }
{ "cmd": "submit_task", "task": "…", "cwd"?: "/…", "title"?: "…", "definitionOfDone"?: "…", "model"?: "…", "supervisorId"?: "SV-1" }  // v1.3 → { ok, supervisorId, sessionId, error? }
{ "cmd": "message", "from": "…", "to": "…", "type": "task"|"result"|"question"|"handoff"|"escalate", "body": "…" }              // v1.3 → { ok, messageId?, error? }
{ "cmd": "pause_supervisor", "id": "SV-1" }                // v1.3 → { ok }
{ "cmd": "resume_supervisor", "id": "SV-1" }               // v1.3 → { ok }
{ "cmd": "kill_switch" }                                  // v1.3 → { ok, closed: n }
{ "cmd": "list_models" }                                  // v1.3 → { models: string[], error? }
{ "cmd": "open_dashboard" }                        // v1.2 → shows + focuses the dashboard window; ack only
{ "cmd": "set_frame", "mode": "rest" | "peek" | "open" } // resizes the notch window instantly; returns the applied rect
{ "cmd": "haptic", "kind": "alignment" | "levelChange" }  // no-op allowed in v0, must not error
```

### app → ui: events (via `listen(<event>, cb)`)

```jsonc
"snapshot"        → the §1.3 snapshot object (emitted ~150 ms debounce on change, plus once on subscribe-equivalent; carries decisions/escalations/busMessages caps)
```

`logs` returns its payload directly from `invoke` — there is no separate log event in v1.1+. `set_frame` resolves after the window is resized. Unknown commands → `Result::Err` with a plain sentence; the webview must treat every error as non-fatal and stay alive.

v1.3 event additions (app → ui, same channel style): none beyond `snapshot` — the snapshot is the single source of truth for decisions/escalations/busMessages (capped, newest-first). The dashboard derives the multiplexer, comms and escalations views from it; do not add a second event path without bumping this contract.

## 7. Surfaces (v1.2; v1.3 dashboard slices)

Two webview surfaces ship from the **same** Vite bundle, routed by query string:

- `index.html` (default) — the notch popover. Transparent, borderless, always-on-top; the only surface that calls `set_frame`.
- `index.html?surface=dashboard` — the standard macOS dashboard window (regular decorations, titled, resizable, min 880×600, 1120×760 default, normal level, dock-visible; v1.3: app is a regular app — no LSUIElement trick, no activation-policy juggling). Never calls `set_frame`. Polls `logs` for live transcript (1 s cadence, only for the selected working session). Dashboard tabs (v1.3): **Tree** (default: sidebar + conversation + inspector, 3 panes), **Multiplexer** (grid of all sessions with live tails), **Comms** (bus log), **Escalations** (inbox of unresolved escalations). One window; tabs, not windows.

The shell (app/) owns creating and focusing both; the frontend route-switch keys off `new URLSearchParams(location.search).get('surface') ?? 'popover'`.

## 8. Supervisor + bus semantics (v1.3 — the only things notch adds)

**OpenCode is a black box.** We never touch or mirror its sub-agents, tool loop, editing, or bash. A harness session is started, prompted, observed, killed. Nothing else.

**A) Agent-to-agent communication.** All traffic goes through our bus → orchestrator/supervisor → the target session as a normal prompt. Never direct session→session channels. Every message is logged (`from`, `to`, `type`, `at`) and attributed, and visible in the UI comms view. Types: `task`, `result`, `question`, `handoff`, `escalate`. `message` command (§2/§3) is the only ingress. Delivery = `herdr agent prompt <target> <body>`.

**B) Agent multiplexer + autonomous supervisor.** The supervisor spawns sessions, assigns each a goal, and keeps them working so the user never babysits.

Per session the supervisor tracks:
- `goal` — the job text.
- `definitionOfDone` — a user-supplied, checkable condition (free text; the supervisor must verify it, not trust self-reports).
- `iterationBudget` — max re-prompts per goal (default **8**). `wallClockBudget` — default 3600 s per goal.
- `iterationsUsed` — incremented on each re-prompt.

**Idle detection (verify-then-build).** Primary signal: poll `opencode api GET /api/session` (read-only v2 API, verified 2026-11-19: returns `time.busy`/`time.idle` ms, model, tokens, cost, directory, session ids matching herdr `agent_session.value`) every 2 s via the injectable runner; map pid-pane → session-id via `herdr agent list`; session is idle when `time.idle ≥ time.busy`. Secondary: herdr `agent_status` transitions (`working→idle/done`). Fallback: output-quiet heuristic (no `revision` or output change for 90 s while not spawning). **Log which signal fired** (`events.jsonl kind:"idle_signal"`). If primary is unavailable (opencode API unreachable), fall back automatically; never crash the poll loop.

**Supervisor decision loop** (when a session's idle is detected):
1. Read the session's tail output (`herdr agent read <target>`, last ~200 lines).
2. Ask an OpenCode call (black-box headless `opencode run [--model …]`) to judge: `done` / `continue` / `stuck` — with the goal, the DoD, the tail output, and the iteration counter. **Default: no `--model` flag**, so the operator's own OpenCode default model is used; an explicit per-session model (UI/`--model`) is passed through verbatim. The orchestrator makes **zero** model calls.
3. `done` → verify DoD via the same call demanding evidence (tests/output/screenshot paths). Only mark session `done` when the verifier accepts evidence; otherwise re-prompt for the missing evidence.
4. `continue` → paste the next prompt (concrete, specific, references what's missing). `iterations += 1`.
5. `stuck` → re-prompt with a **different framing** (new instruction, not the same one). If stuck persists across 3 consecutive identical outputs or the same error repeats → **loop detection** → change strategy (fresh framing / new session / model swap) once, then **escalate**.
6. Budgets: `iterations ≥ maxIterations` or wall-clock exceeded → **escalate**. Rate-limit (429/quota in the pane tail or the decider call) → backoff with jitter and **one** retry/swap to a good-quality free model that the operator's `opencode models` actually lists (toy/tiny models excluded); the choice is persisted on the session. Never hammer; no forced house model.
7. **Never** re-prompt a session that is `working`. **Never** re-prompt during an active tool call.
8. Concurrent edits: attribute every change to a session id; serialize sessions touching the same files (wait for one to go idle); real conflicts → escalate, never auto-merge.
9. **Kill switch**: `kill_switch` (§2/§3) — one click, closes all managed panes, marks failed, cleans up. **Pause**: `pause_supervisor` per supervisor. Both always one click, always work.
10. Every decision is logged with its reason (`kind:"decision"`) and shown in the UI: e.g. `SV-1 re-prompted S1-OC02: tests not run`.

**Acceptance test** (must pass, with logs + screenshots; the live demo for v1):
1. One supervisor, one goal, with a checkable DoD.
2. It spawns 2 sessions; one finishes idle with incomplete work.
3. With zero user input, the supervisor detects idle, pastes a follow-up prompt, the session resumes.
4. Session 1 sends a `handoff` to session 2 through the bus; it shows in the comms view with FROM/TO.
5. A deliberately looping session is caught by loop detection and escalated — not retried forever.
6. The supervisor marks DONE only after verifying the DoD with evidence.

### Permissions
The webview receives ONLY: `core:event:allow-listen` / `allow-emit` (for snapshot) and the commands above. No fs, shell, dialog, http, or window plugins granted.

---

## 4. File ownership (disjoint)

| Path | Owner | Notes |
|---|---|---|
| `docs/bridge.md` | lead | frozen (v1.1) |
| `docs/PLAN.md` | lead | |
| `.proof/**` | lead | agents may only use `.proof/agents/<name>/` |
| `crates/notchd/**` | Agent A (rust-core) | lib + CLI/daemon; engine |
| `examples/two-by-two.json` | Agent A | the demo plan |
| `ui/**` | Agent B (web-ui) | Vite 8 + React 19 + TS strict + Tailwind v4 → `ui/dist/` |
| `app/**` | Agent C (tauri-shell) | Tauri v2; loads `ui/dist/` as frontendDist (relative) |
| `tools/**` | lead | |
| `NOTCH_UI_DIR` / fallback | `app/` must load `ui/dist/` by relative path `../../ui/dist` (frontendDist) | if missing, tauri build must fail loudly — no placeholder UI is ever committed |

## 5. Naming / state files

- State dir: `~/Library/Application Support/notch/` (override with `NOTCH_STATE_DIR`): `registry.json` (ID allocation), `state.json` (tree), `events.jsonl` (append-only event log, rotated at 4 MB), `notch.sock`.
- Per-session temp dir: `$TMPDIR/notch-<id>/` — removed by cleanup.
- Event log line: `{ "ts": …, "session": "S1-OC01"|null, "kind": "spawn|kill|status|reprompt|cleanup|conflict|escalate|watchdog|message|decision|idle_signal", "detail": … }`

## 6. Process layout

`notchd serve` = long-lived daemon (also `notchd run-plan <file>`, `notchd snapshot`, `notchd kill …` etc.). The Tauri app (`app/`) embeds the same engine in-process; the CLI talks to a reachable daemon over the socket when one is running, else acts in-process. Killing the daemon never kills OpenCode sessions (they're in herdr panes) — it stops supervising; next start re-adopts panes recorded in `state.json`, vanished panes → sessions marked `done`.
