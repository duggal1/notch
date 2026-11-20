# notch

Agent control plane for OpenCode, living in the MacBook notch. Thin, strict orchestration over **OpenCode + herdr + Ghostty** — org chart, permanent IDs, watchdog, communication discipline, forced cleanup. It never reimplements the agent runtime.

## Architecture

```
notchd engine (Rust — crates/notchd)   ← orchestrator: tree, bus, watchdog, attribution, cleanup
   │  embedded in-process by the Tauri backend (and runnable as `notchd serve` headless)
   ▼
Tauri v2 app (app/ — Rust)             ← notch window + menu-bar tray; emits `snapshot` events
   ▼
React popover (ui/ — React 19 + TS + Tailwind v4, Vite 8)  ← DESIGN.md, implemented brutally
```

Contract: [docs/bridge.md](docs/bridge.md) · Plan: [docs/PLAN.md](docs/PLAN.md) · Design law: [DESIGN.md](DESIGN.md) + [TASTE.md](TASTE.md) + [DESIGN-RULES.md](DESIGN-RULES.md)

## Build & run

Run `scripts/gate.sh` from the repo root after `npm --prefix ui ci`. Rust integration tests require `herdr` on PATH and start isolated headless servers.

```sh
# engine
cd crates/notchd && cargo test
cargo run -- serve            # headless daemon (NDJSON socket)
cargo run -- run-plan ../../examples/two-by-two.json

# popover
cd ui && npm ci && npm run build     # → ui/dist

# app
cd app && npm ci && npx tauri build --debug
./src-tauri/target/debug/bundle/macos/Notch.app
```

Rules of engagement with the machine: never `open` a new window; panes are requested inside the existing herdr workspace via `herdr pane split`; existing tabs/panes are untouched; "show me" focuses the herdr pane (`herdr agent focus <id>`).

## Layout

| Path | What |
|---|---|
| `crates/notchd/` | engine (lib + CLI), watchdog, herdr client, event log |
| `ui/` | React popover + dashboard, implementing DESIGN.md |
| `app/` | Tauri v2 shell (notch window, tray, dashboard window, commands/events) |
| `docs/` | PLAN.md · bridge.md (wire contract) |
| `examples/two-by-two.json` | prototype plan: 2 supervisors × 2 sessions |
| `FONT.md` | type contract (weights 400/500, sentence case, tracking tied to size) |

Icons in the UI are a hand-authored stroke family plus **on-device SF Symbols** (rendered by the Rust shell at runtime; never bundled or exported). Haptics via NSHapticFeedbackManager from the shell.

## Talking to it

Dashboard window (regular macOS window, in the dock / ⌘Tab): type the job in the composer, Enter — the orchestrator creates `SV-n` and `S<n>-OC01` with your task, and you can watch it, re-prompt it (real text, not a fixed string), kill it, read its transcript, or focus its herdr pane. The popover is for glance-status; the dashboard is for steering.
