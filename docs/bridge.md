# bridge.md — the notch wire contract (frozen once written)

Three parties, one contract. If any of the three disagrees with this file, the implementation is wrong.

Version: **1.1** (2026-11-19). v1.1: user stack override — Tauri v2 replaces the Swift/WKWebView host; §3 rewritten (JS ↔ Tauri commands/events instead of `webkit.messageHandlers`); §4 ownership rows updated. §1 (snapshot, IDs, status) and §2 (socket) unchanged. Changes require lead sign-off.

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
      "working": 2, "idle": 0, "failed": 0, "done": 0 }
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
      "failureReason": null                  // string when status == "failed"
    }
  ],
  "eps": 6.5,                                // bus events/sec, float
  "conflicts": [ { "file": "src/auth.rs", "sessions": ["S1-OC01", "S1-OC02"] } ],
  "attribution": [ { "session": "S1-OC01", "file": "src/auth.rs" } ],   // cap 24, newest first
  "warnings": []
}
```

Notes:
- Extra optional fields may be added; none may be removed or renamed in v1.
- `supervisors` is ordered by id; `sessions` is ordered by id.
- Empty tree is legal: `"supervisors": [], "sessions": [], "eps": 0`.
- JS receives this object verbatim via `window.__push(snapshot)` — keys are `camelCase`.

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
| `kill` | `id: string, reason?: string` | ack; kills via `herdr pane close`; session → `failed` with reason; cleanup hook runs |
| `reprompt` | `id: string, text: string` | ack; `herdr agent prompt <target> <text>` |
| `cleanup` | `id?: string` | one session or global sweep; ack with `removed: n` |
| `logs` | `id: string, tail?: number` (default 200) | `{ "event": "logs", "id": …, "lines": […] }` |
| `focus` | `id: string` | ack; `herdr agent focus <target>` — moves focus to that pane, never opens a window |
| `spawn` | `supervisor: string, prompt: string, cwd: string, title: string` | ack with allocated `session.id` |
| `status` | — | `{ "event": "status", "pid": …, "sessions": n, "uptimeS": s }` |

Errors: `{ "event": "ack", "cmd": …, "ok": false, "error": "<machine-readable code or plain sentence>" }`.

### herdr invocation surface (what the daemon is allowed to call)

`herdr pane list` · `herdr agent list` · `herdr pane split --direction right --cwd <cwd>` · `herdr agent start <name> --kind opencode --pane <paneId>` · `herdr agent prompt <target> <text> [--wait]` · `herdr agent read <target>` · `herdr agent wait <target> --until idle --until done --until blocked` · `herdr pane close --pane <paneId>` · `herdr agent focus <target>`.

Nothing else. The daemon never creates windows, never touches panes it doesn't own, never runs `open` or `screencapture`.

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
{ "cmd": "set_frame", "mode": "rest" | "peek" | "open" } // resizes the notch window instantly; returns the applied rect
{ "cmd": "haptic", "kind": "alignment" | "levelChange" }  // no-op allowed in v0, must not error
```

### app → ui: events (via `listen(<event>, cb)`)

```jsonc
"snapshot"  → the §1.3 snapshot object (emitted ~150 ms debounce on change, plus once on subscribe-equivalent)
```

`logs` returns its payload directly from `invoke` — there is no separate log event in v1.1. `set_frame` resolves after the window is resized. Unknown commands → `Result::Err` with a plain sentence; the webview must treat every error as non-fatal and stay alive.

### Permissions
The webview receives ONLY: `core:event:allow-listen` / `allow-emit` (for snapshot) and the ten commands above. No fs, shell, dialog, http, or window plugins granted.

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
- Event log line: `{ "ts": …, "session": "S1-OC01"|null, "kind": "spawn|kill|status|reprompt|cleanup|conflict|escalate|watchdog", "detail": … }`

## 6. Process layout

`notchd serve` = long-lived daemon (also `notchd run-plan <file>`, `notchd snapshot`, `notchd kill …` etc. as thin CLI wrappers over the same code). Swift app connects on launch, `subscribe`s, reconnects with backoff (0.5 s, 1 s, … cap 5 s). Daemon stays up when the app quits. Killing the daemon never kills OpenCode sessions (they're in herdr panes) — it just stops supervising; next start re-adopts panes that still carry `NOTCH_SESSION_ID` in their environment, else marks them `done` by inspection and moves on.
