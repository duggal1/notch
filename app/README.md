# Notch — regular macOS app for the OpenCode orchestration control plane (Tauri v2)

A standard macOS app: menu-bar status item, Dock icon, title bar, ⌘Tab — plus
an always-on-top notch popover (transparent, borderless) and a standard
dashboard window. Rust backend in `src-tauri`; the UI is the React bundle from
`../../ui` (Agent B), embedded at build time.

Regular app since bridge.md v1.3 (user reviewed the product and demanded the
standard app): `LSUIElement` is gone — Dock icon at every launch, stable
bundle identity `com.notch.app`. No activation-policy juggling anywhere.

## Prerequisites

- macOS 14 (Sonoma) or newer
- Xcode Command Line Tools, Rust stable (verified with 1.100), Node ≥ 20

## What it is (v1.3)

- **Menu-bar status item** — template ring glyph (18×18 black PNG, generated
  by `assets/generate_icons.py`); title `‹N›` while N sessions are in flight
  (icon-only at rest); left click toggles the popover; right click opens the
  menu: `N in flight` (disabled, live count) · `Open dashboard` ·
  `Show / hide popover` · `Kill all sessions` · `Sweep temp orphans` ·
  `Quit`. Tooltip: `notch — N in flight · M failed` (failure count only when > 0).
- **Notch popover** — transparent, borderless, always-on-top NSWindow at
  `NSMainMenuWindowLevel + 3`, `canJoinAllSpaces | stationary |
  fullScreenAuxiliary | ignoresCycle`, resizable=false, non-movable, centered
  on the main screen at y = 0; window chrome is applied in `macos.rs`;
  `set_frame(rest|peek|open)` resizes instantly (the webview's clip-path
  morph owns the smoothness).
- **Dashboard** — standard macOS window: regular decorations, titled `Notch`,
  resizable, min 880×600, default 1120×760 centered on the main screen,
  normal level, Dock-visible. Created **hidden at startup**; `open_dashboard`
  (or the tray's "Open dashboard") shows + unminimizes + focuses it.
  Loads `index.html?surface=dashboard`; never calls `set_frame`.
- **Engine** — `notchd` is embedded in-process (`reconcile` → `run_poll_loop`
  → subscriber flusher, exactly the `notchd serve` wiring minus the Unix
  socket). The webview talks to it through Tauri commands + `app.emit`.
- **Single instance** — second launch activates the running instance
  (NSRunningApplication, bundle-id match, `ActivateAllWindows`) and exits
  before any window or state is touched.

## Integration commands (what the lead runs)

```sh
cd ui && npm ci && npm run build     # produce ui/dist (Agent B's bundle)
cd app && npm ci                     # tauri-cli 2.12.x
cd app && npx tauri build --debug    # builds + signs ad hoc → target/debug/bundle/macos/Notch.app
open -g -j app/src-tauri/target/debug/bundle/macos/Notch.app   # hidden launch, no focus steal
pkill -x notch                       # stop
```

For the release build: `cd app && npm run build` (same, `release` profile).

## Development

```sh
cargo test --manifest-path src-tauri/Cargo.toml   # 35 tests
cd app && npx tauri dev                            # hot dev
cd app && npx tauri info                           # environment report
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
```

`frontendDist` is `../../ui/dist` — the app **will not compile** without
`ui/dist` (the webview assets are embedded). Run the `npm ci && npm run build`
step in `../ui` first. There is intentionally no placeholder anymore; the
bundle embeds the real UI.

## How it fits together

- `src-tauri/src/lib.rs` — setup: single-instance guard (activates an
  existing instance and exits) → `Daemon::open` → `reconcile()` (re-adopts,
  vanished panes → `done`) → popover (transparent, borderless, always-on-top,
  skip taskbar, `__NOTCH__` injected at document start) → `NSWindow` chrome →
  **dashboard created hidden** (regular window, same bundle) → tray → emit
  thread.
- `src-tauri/src/engine.rs` — shell ↔ `notchd` wiring ONLY: poll loop +
  subscriber flusher (same as `notchd serve`) + NDJSON→`app.emit("snapshot")`
  with a 150 ms coalesce window. **The Unix socket is not used.**
- `src-tauri/src/commands.rs` — `snapshot, kill, reprompt, logs, focus,
  cleanup, spawn, run_plan, submit_task, message, pause_supervisor,
  resume_supervisor, kill_switch, list_models, symbol, open_dashboard,
  set_frame, haptic`. Thin wrappers over `notchd::Daemon` (in-process).
  `list_models` is the engine's own (shells out to `opencode models`,
  5-min cache).
- `src-tauri/src/symbols.rs` — SF Symbol validation/rasterization to
  `data:image/png;base64`.
- `src-tauri/src/macos.rs` — the only module that reaches AppKit. NSScreen
  notch measurement (`safeAreaInsets.top` + `auxiliaryTopLeft/RightArea`),
  `NSWindow` chrome (`NSMainMenuWindowLevel + 3`, collection behavior
  `CanJoinAllSpaces | Stationary | FullScreenAuxiliary | IgnoresCycle`,
  borderless), NSHapticFeedbackManager.
- `src-tauri/src/frame.rs` — pure geometry (notch fallback 200×32, mode→rect
  table: rest `notch.w+120 × notch.h`, peek `notch.w+240 × notch.h+44`,
  open `560 × 640`, y = 0 always).
- `src-tauri/src/tray.rs` — `NSStatusItem` via tauri tray API: template ring
  glyph (18×18 black PNG in `assets/tray-glyph.png`), title `‹N›` while N
  sessions are in flight (icon-only at rest), tooltip carries the numbers,
  left click toggles the popover, right click opens the menu
  (`N in flight` [disabled] · `Open dashboard` · `Show / hide popover` ·
  `Kill all sessions` · `Sweep temp orphans` · `Quit`).
  `show_menu_on_left_click(false)` so left click does not open the menu.
- `src-tauri/src/snapshot.rs` — test-side serde mirror of bridge.md §1.3.
  Events are emitted as raw JSON from notchd's own serializer; the UI receives
  the exact §1.3 object.

`window.__NOTCH__` is injected via an initialization script (integer pixels),
matching `ui/src/lib/shell-geometry.ts`.

## Environment knobs (all optional)

- `NOTCH_STATE_DIR` — registry.json / state.json / events.jsonl location
  (default `~/Library/Application Support/notch`)
- `NOTCH_HERDR_CMD` — herdr binary name/path (default: probed from
  `~/.local/bin/herdr` etc., else bare `herdr`)
- `NOTCH_OPENCODE_CMD` — opencode binary name/path (same probing)
- `NOTCH_POLL_INTERVAL_MS` — engine poll cadence (default `2000`)
- `NOTCH_STUCK_AFTER_SECS` — watchdog: revision-stagnant working session
  considered stuck (default `900`)

## Known limitations

1. **notchd is a moving target.** The v1.3 engine API (`submit_task` with
   DoD/model/supervisorId, `message` with verbatim delivery + comms log,
   `pause/resume_supervisor`, `kill_switch`, `list_models`) landed mid-task;
   the app is wired to it and the full bridge command set is functional.
   Agent A is still actively landing changes (the tree broke mid-edit twice
   on this session and recovered). If a future `crates/notchd` revision breaks
   the build, the failure is in the engine — not this directory.
2. **Not an NSPanel.** Tauri cannot host one. The popover is a borderless,
   non-opaque NSWindow at `NSMainMenuWindowLevel + 3`. A click on it
   participates in app activation like a normal borderless window (not
   non-activating). Everything else from DESIGN.md §5.1 is applied.
3. **Notch detection is cached at launch** (NSScreen is main-thread only).
   Display reconfiguration (docking/undocking, lid close) does not re-measure
   until the app restarts. Resizes via `set_frame` always recompute from the
   cached measurement.
4. **Untitled 500×500 window owned by the app** (layer 0, at `(0,400)`) is
   visible in `CGWindowListCopyWindowInfo`. Reproduced identically with the
   `WebviewWindowBuilder` path and with a plain static `tauri.conf.json`
   window, and with the tray disabled — it is a framework/OS artifact of this
   tao/AppKit build on macOS 27, not from our code. It is hidden whenever the
   popover is hidden and has never been observed to steal focus.
5. **Screen Recording permission** was not granted to the build/test session,
   so window contents are UNVERIFIED (captures return black frames; metadata —
   bounds, layer, level — is verified). Tray visibility likewise verified only
   to "API call succeeded"; the ring glyph appears in the menu bar on a normal
   launch. Dock-icon presence verified at the Info.plist + LS registration
   level (`LSUIElement` absent; `lsappinfo` shows `com.notch.app` as a regular
   app), not visually.
6. **Single-instance relaunch** (second `open` activates the running app via
   `NSRunningApplication` + `ActivateAllWindows` and exits) is unit-tested in
   its decision logic but deliberately NOT fired against a hidden instance —
   activation would unhide the popover onto the user's screen. Runtime path is
   review-verified only.
7. **Haptics** are best effort: `alignment` on open, `levelChange` on failure,
   straight to `NSHapticFeedbackManager.defaultPerformer()`.
8. **Bundle identifier** `com.notch.app` is mandated; tauri warns it ends with
   `.app` on macOS. Expected; change only with lead sign-off.
9. The popover **does not steal focus** on launch (`focus: false`); the tray
   left-click toggles visibility. `Esc`/click-outside behavior is the
   webview's responsibility (bridge.ts).

## File map

```
app/
  package.json            scripts: dev, build, check, info; devDep @tauri-apps/cli
  src-tauri/
    Cargo.toml            tauri 2.12 (macos-private-api), notchd path dep (required)
    build.rs              tauri-build
    tauri.conf.json       productName Notch, com.notch.app, frontendDist ../../ui/dist
    Info.plist            merged by codegen+bundler: NSHighResolutionCapable, utilities
    (regular app: no LSUIElement — Dock icon every launch)
    capabilities/default.json   event listen/unlisten only — no fs/shell/http/dialog
    icons/icon.png        1024×1024 app icon (generated)
    icons/icon.icns       generated (iconutil)
    src/{main,lib,frame,macos,engine,commands,symbols,snapshot,tray,trayfmt}.rs
    tests/{frame,snapshot,trayfmt,engine_integration,herdr_resolve,opencode_resolve,dashboard_frame,single_instance,symbols}.rs
  assets/
    tray-glyph.png        18×18 black template ring
    generate_icons.py     regenerates both raster assets
```
