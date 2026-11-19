# bridge.md — the notch wire contract (frozen once written)

Three parties, one contract. If any of the three disagrees with this file, the implementation is wrong.

Version: **1**. Date frozen: 2026-11-19. Changes require lead sign-off; bump nothing without editing this file.

---

## 0. In one picture

```
WKWebView (TypeScript, ui/)          Swift app (app/)                  notchd (crates/notchd/)
        │                                  │                                  │
        │ webkit.messageHandlers.notch     │                                  │
        │─────────────────────────────────▶│  Unix socket (NDJSON)             │
        │                                  │─────────────────────────────────▶│
        │                                  │◀─────────────────────────────────│
        │ window.__push(snapshot)          │  { "event": "snapshot", … }       │
        │◀─────────────────────────────────│                                  │
```

- JS → Swift: `(window as any).webkit?.messageHandlers?.notch?.postMessage(obj)`
- Swift → JS: `window.__push(snapshot)` via evaluating `JSBridge` from `Bridge.swift`
- Swift ↔ daemon: Unix domain socket, newline-delimited JSON (NDJSON). One JSON object per line.
- Socket path: `${NOTCH_SOCK:-~/Library/Application Support/notch/notch.sock}` — honor `NOTCH_SOCK` env for tests; fallback to a temp path if Application Support is unwritable.

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

## 3. JS ↔ Swift bridge (what ui/ may send, what app/ must handle)

### ui → app (postMessage)

```jsonc
{ "type": "setFrame", "mode": "rest" | "peek" | "open" }   // asks host to resize NSPanel (app owns real geometry)
{ "type": "kill", "id": "S1-OC01" }
{ "type": "reprompt", "id": "S1-OC01", "text": "…" }
{ "type": "logs", "id": "S1-OC01" }                        // app asks daemon; reply delivered as window.__logs
{ "type": "focus", "id": "S1-OC01" }                       // focus herdr pane
{ "type": "cleanup" }
{ "type": "haptic", "kind": "alignment" | "levelChange" }
{ "type": "snapshot" }                                     // request fresh snapshot
```

### app → ui

```jsonc
window.__push(snapshot)          // §1.3 object
window.__logs(id, lines)         // reply to logs request
window.__ready = true            // after WKWebView finished loading + JS did mount()
```

All postMessage objects carry no other fields. Unknown `type` ⇒ ignored, logged once.

---

## 4. File ownership (disjoint)

| Path | Owner | Notes |
|---|---|---|
| `docs/bridge.md` | lead | frozen |
| `docs/PLAN.md` | lead | |
| `.proof/**` | lead | agents do not write here; they report in their final message |
| `crates/notchd/**` | Agent A (rust-core) | Cargo workspace root is repo root: root `Cargo.toml` lists `crates/notchd` |
| `examples/two-by-two.json` | Agent A | the demo plan |
| `ui/**` | Agent B (web-ui) | esbuild → `ui/dist/` |
| `app/**` | Agent C (swift-host) | SwiftPM package `Notch`, executable + Info.plist build script |
| `tools/**` | lead | |
| `NOTCH-UI-BUNDLE-PATH` | `app/` must load `ui/dist/` by relative path with sensible fallback | see below |

The Swift app loads the UI from: `NOTCH_UI_DIR` env if set → `<repo>/ui/dist` (found by walking up from the binary) → bundled copy inside the .app if present. If none load, show an empty black shell + status plate text and log; never crash.

## 5. Naming / state files

- State dir: `~/Library/Application Support/notch/` (override with `NOTCH_STATE_DIR`): `registry.json` (ID allocation), `state.json` (tree), `events.jsonl` (append-only event log, rotated at 4 MB), `notch.sock`.
- Per-session temp dir: `$TMPDIR/notch-<id>/` — removed by cleanup.
- Event log line: `{ "ts": …, "session": "S1-OC01"|null, "kind": "spawn|kill|status|reprompt|cleanup|conflict|escalate|watchdog", "detail": … }`

## 6. Process layout

`notchd serve` = long-lived daemon (also `notchd run-plan <file>`, `notchd snapshot`, `notchd kill …` etc. as thin CLI wrappers over the same code). Swift app connects on launch, `subscribe`s, reconnects with backoff (0.5 s, 1 s, … cap 5 s). Daemon stays up when the app quits. Killing the daemon never kills OpenCode sessions (they're in herdr panes) — it just stops supervising; next start re-adopts panes that still carry `NOTCH_SESSION_ID` in their environment, else marks them `done` by inspection and moves on.
