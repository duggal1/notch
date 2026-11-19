# Hardening plan — notch Rust daemon

Source: user-supplied external audit + hardening plan, 2026-11-20. Binding for the five hardening agents A1–A5.
Read fully before touching code. This is a rewrite of security/lifecycle behavior, not a feature task.

## 0. Ground rules (all agents)
1. Red first. Commit a test that FAILS on current main, then the fix that turns it green. No red test, no PR.
2. No `#[ignore]`, no loosened asserts, no sleep-as-synchronization, no `cargo clean`.
3. One owner per file (section 1). Need a change in someone else's file? Write a contract request in `COORD.md`; the owner lands it.
4. Cross-review ring: A1 reviews A2, A2 reviews A3, A3 reviews A4, A4 reviews A5, A5 reviews A1. The reviewer must write one adversarial test against the PR before approving.
5. **Git override (user decision 2026-11-20): SOLO MODE ON MAIN.** All agents share ONE working tree. Do NOT create/switch branches (`fix/hardening`, `fix/aN-topic`, or otherwise). Pull before push, stage only explicitly owned paths (never `git add .`/`-A`), commit with 6–12 word messages, push origin main. No rebase/reset/restore/force push.
6. Gate on every merge: `cargo test --workspace`, `cargo clippy -- -D warnings`, `cargo mutants` on touched files (≥85% caught), ThreadSanitizer on daemon tests. If a tool is unavailable or disk is <2 GiB free, report UNVERIFIED with the exact blocker — never fake the gate. Check `df -h` first.
7. `COORD.md` at repo root: append a line when you start, block, or push; read it before picking up work.
8. Do NOT use notch's own decider to supervise these agents until A1 merges (it runs `--auto`). Plain herdr panes only.
9. Terminal isolation: any agent driving herdr MUST first `herdr workspace create --cwd <fresh temp dir> --label notch-hardening-<id> --no-focus`, work only in that workspace, close only it, never touch pre-existing panes.
10. Frontend is user-owned: never edit `ui/**`, `docs/frontend.md`, `app/**` surface files. This plan is backend-only (`crates/notchd/**`).

## 1. Team and ownership

| Agent | Mission | Owns | Reviews |
|---|---|---|---|
| A1 sandbox | Decider lockdown, prompt-injection hardening, WS auth, socket perms | `decider.rs`, `opencode.rs`, `ws.rs`, `socket.rs`, `paths.rs` | A2 |
| A2 process | One subprocess runner with hard timeouts, judge pool off watchdog thread, herdr adapter | `proc.rs`, `herdr.rs`, `daemon/supervise.rs`, `daemon/lifecycle.rs` | A3 |
| A3 liveness | Monotonic clock, correct stuck logic, stall guard, blocked semantics | `clock.rs`, `watchdog.rs`, `supervisor.rs`, `daemon/poll.rs` | A4 |
| A4 identity/state | Registry race, recovery, coalesced state writes, time-spaced backups, schema | `registry.rs`, `durable.rs`, `model.rs`, `daemon/state.rs`, `daemon/types.rs` | A5 |
| A5 isolation | Worktree per session, exact attribution, real cleanup | `attribution.rs`, `daemon/attribution.rs`, `cleanup.rs`, `daemon/spawn.rs`, new `worktree.rs` | A1 |

Dependence: A1, A3, A5 shell out → need A2's `proc::run`. A3, A4, A5 touch session fields → A4 owns `model.rs`. A1's judge scratch dir comes from A5's temp-dir layout. A3's kill path calls A5's `worktree::snapshot`. Nobody can finish alone.

## 2. Phase 0 contracts (A2 + A4, under 1 hour)

Land compiling stubs first so everyone codes against frozen signatures.

```rust
// proc.rs (A2) — the ONLY place notchd spawns child processes
pub struct Spec<'a> {
  pub cmd: &'a str, pub args: &'a [String], pub cwd: Option<&'a std::path::Path>,
  pub env: &'a [(String, String)], pub timeout: std::time::Duration, pub max_out: usize,
}
pub enum ProcError { Spawn(String), Timeout { after: std::time::Duration }, Failed { code: Option<i32>, stderr: String } }
pub struct Out { pub stdout: String, pub stderr: String }
pub fn run(spec: &Spec) -> Result<Out, ProcError> { todo!() }

// clock.rs (A3)
pub trait Clock: Send + Sync {
  fn now_ms(&self) -> u64;   // wall clock: display and persistence only
  fn mono_ms(&self) -> u64;  // monotonic: every duration and timeout
}

// model.rs (A4): new SessionState fields, all #[serde(default)]
pub worktree: Option<String>, pub branch: Option<String>, pub base_rev: Option<String>,
```

Merge order: Phase 0, then A2, A4, A3, A1, A5, then the chaos suite (everyone).

## 3. Fixes

### 3.1 One subprocess runner with hard timeouts (A2)
`Command::output()` has no timeout in `herdr.rs` or `opencode.rs`; one hung call freezes the poll/watchdog. Add `libc` (kill the whole process group — opencode spawns children), use `process_group(0)`, spawn reader threads that keep draining past `cap` so children never block, poll `try_wait` against `Instant::now() + timeout`, kill group + reap on timeout, bounded 500 ms grace for pipe EOF. Timeout constants (one place): herdr list/read 5 s, herdr mutate 10 s, opencode api 8 s, git 5 s, decider 90 s. Then `Herdr::invoke_argv` and `Opencode::invoke_argv` become thin wrappers over `proc::run`; attribution's `sample_git` and the pre-spawn hook move onto it too. Delete the pre-spawn hook's relative-path fallback (executes a script relative to unknown daemon cwd).

### 3.2 Judgments off the watchdog thread (A2)
`supervise_once` must not call `opencode run` inside `poll_once`. `JudgePool`: bounded `SyncSender<Job>` (32), `done` receiver under Mutex, `in_flight: HashSet<String>` (≤1 job/session), N worker threads calling blocking `job.run(&oc)` (proc::run enforces deadline). `poll_once`: drain finished jobs → apply; then prepare + `try_submit`. Verify is a second Job kind. Then watchdog gets its own thread/tick.

### 3.3 Decider lockdown (A1)
`run_decider_session` currently passes `--auto` (approves every permission) and feeds raw untrusted pane output into one shared session in the daemon's cwd (often `/`). Fix: remove `--auto`; deny all tools via `OPENCODE_PERMISSION` (verify schema on installed OpenCode first — test T7); run in empty scratch `--dir <tmp>/notch-judge`; one session per notch session (`ses_notch_<id>` keeps required `ses` prefix); fence untrusted tail with random per-call `UNTRUSTED_<16hex>` marker + ANSI/control stripping + 6000-char cap; parser accepts verdict only from FIRST non-empty line, nothing from inside fence; `sanitize_instruction` rejects shell-shaped text (backticks, `$(`, URLs, >240 chars), returning `None` → unparseable strike path.

### 3.4 Correct stuck logic + monotonic clock (A3)
Stuck check reads the timestamp from BEFORE this poll's revision change; wall-clock jumps. Add `Clock::mono_ms()` (static `OnceLock<Instant>`); `revision_changed_at_ms` monotonic, never persisted, reset on restart. `Watchdog::classify`: Alive if status != Working or revision changed now or `oc_busy == Some(true)`; Stuck if quiet ≥ 2× threshold; Suspect if quiet ≥ threshold. Stall guard: if gap between polls > 3× poll interval, reset all `revision_changed_at_ms` to now, log `watchdog stall … grace granted`, no kills. Action ladder: Suspect → nudge prompt + `stuck_suspect` escalation; Stuck → escalation and pane close only if `kill_on_stuck` (default OFF); before ANY close snapshot work via A5's `worktree::snapshot`. Verify macOS `Instant` sleep behavior with T4; stall guard covers other cases.

### 3.5 Registry race, recovery, state writes (A4)
`Daemon.registry: Mutex<Registry>` loaded once in `open()`; `alloc_session` saves registry BEFORE id use. `load_or_recover`: try live + 3 backups, rebuild from state, else hard error (never silently reset to SV-1 on corruption). State writes: dirty flag + single writer thread at 1 Hz; synchronous `save_now()` on spawn/kill/escalate/shutdown; real diff in `poll_once` so `changed` true only on actual status/last_action/revision change. `durable.rs` time-spaced backups: tiers 60 s / 900 s / 3600 s, not 6 seconds of identical snapshots. Log warnings on state parse failure.

### 3.6 Worktree per session, exact attribution, real cleanup (A5)
Each session gets its own branch + worktree (`notch/<id>`), used as pane cwd; attribution = commits relative to base + porcelain (exact, no shared-cwd blame); `conflicts()` via `git merge-tree --write-tree` exit 1; `worktree::snapshot(wt, why)` commits WIP before kill; never auto-merge, never remove dirty worktree. Non-git cwd: spawn with loud warning + old heuristic fallback. Real cleanup: pass `TMPDIR=<session tmp>` into the pane env, optional `NOTCH_ISOLATE_BUILD=1` → `CARGO_TARGET_DIR`; disk budget `NOTCH_MAX_TMP_BYTES` default 5 GiB, evict dead sessions oldest-first.

### 3.7 Auth before WS wiring (A1)
WS has no auth/Origin check — wiring it as documented allows drive-by code execution from any website. Reject any `Origin` header; require `Authorization: Bearer <token>` (32 random bytes from /dev/urandom, hex, file 0600, state dir 0700); check `Host == 127.0.0.1:<port>` (DNS rebinding); constant-time compare; use `accept_hdr_with_config`. Cap NDJSON lines at 1 MiB with `.take(1<<20)` and drop on overflow. WS stays unwired until tested; then behind `NOTCH_WS=1`.

### 3.8 Blocked is not failure (A3)
herdr `blocked` = waiting on a human; typing "diagnose and continue" into a permission prompt is wrong. `FailureAction::{Reprompt, Escalate, NeedsHuman}`; `blocked_waiting` → `NeedsHuman` (escalation `needs_you`, pane + temp dir kept, session not failed).

### 3.9 Polling → events spike (A2, after everything else merges)
Wrap source behind `trait AgentSource { snapshot(); subscribe() -> Option<Receiver<AgentEvent>> }`; read herdr SOCKET_API.md and document findings in COORD.md (no guessing); same for `opencode serve`. Ship only if soak (T10) shows fewer spawns and no regressions.

## 4. Brutal tests (nothing is smoke)
T1 one red-then-green commit pair per defect, visible in git history. T2 fault-injection shims (hang, ignore SIGTERM, 1 GB stdout, garbage JSON, setsid grandchild, byte drip, fd leak): return within timeout+500 ms, no child alive (pgrep -g), RSS growth <20 MB, daemon answers status. T3 watchdog under stall (20 live sessions, fake 120 s hangs, one truly stuck): tick gap p99 ≤2× poll interval, stuck flagged within threshold+2 ticks, zero false positives; include the regression test where a stalled hour + revision change → session not killed. T4 clock chaos (fake +1 h/−1 h jumps, wall/mono skew; real Mac sleep script) → zero kills. T5 crash consistency: SIGKILL harness 1,000 random offsets → state/registry parse or recover, no `.tmp` adopted, zero ID repeats. T6 concurrency storm: 64×200 `submit_task` against fake herdr, restart, repeat; TSan + loom of registry-then-inner lock order. T7 prompt-injection red team: 100 payloads (ANSI, homoglyph DONE:, base64, fake/nested fences, 1 MB tails) unit + 20 live runs per payload with `/tmp/NOTCH_PWNED_<nonce>` canary; deny-all permission probe verified. T8 WS abuse: Origin/token/Host/17th client rejected; 100k random frames + 256 KB boundary frames; daemon responsive. T9 isolation: 6 agents edit same file in separate worktrees, zero false attribution, conflict check agrees with real merge, non-git warns, dirty removal refused. T10 8-hour soak: flat threads/fds/RSS <1%/h, state writes ≤1/s, 3 backups with 3 ages, p99 steady, zero spurious kills. T11 `cargo mutants` ≥85% caught on watchdog/registry/durable/proc/parser+sanitizers; survivors justified. T12 dogfood: notch runs 5 fresh agents on a real task 2 hours unattended; zero false kills, duplicate IDs, decider escapes; clean merges.

## 5. Done means
All 12 groups green with command lines pasted into COORD.md; every defect has red-then-green pair; REPORT.md updated with measured numbers and named gaps; Tauri NSPanel focus limitation listed, not hidden.

## 6. Agent prompts
Shared preamble: "You are one of 5 agents hardening the Rust daemon in crates/notchd. Read docs/PLAN.md, docs/REPORT.md and docs/HARDENING.md fully first. Rules: red test first, no #[ignore], no weakened asserts, no `cargo clean`. Edit ONLY files you own; anything else is a contract request in COORD.md. Append to COORD.md when you start, block, or push; read it before picking up work. Git: SOLO MODE ON MAIN (no branches/checkout/switch); pull before push; stage only explicit owned paths; 6–12 word commits; no reset/restore/force push. Any herdr testing requires your own `herdr workspace create --no-focus`. Never touch ui/** or docs/frontend.md. Review your assigned peer's work and write one adversarial test before approving. Report measured numbers, never adjectives."

A1 sandbox: own `decider.rs`, `opencode.rs`, `ws.rs`, `socket.rs`, `paths.rs`. Do 3.3 + 3.7, T7 + T8. First task: live-test whether `OPENCODE_PERMISSION` deny-all works on installed OpenCode; if not, find the real mechanism and write it to COORD.md before building. Use A2's `proc::run` only. Review A2.
A2 process: own `proc.rs`, `herdr.rs`, `daemon/supervise.rs`, `daemon/lifecycle.rs`. Land Phase 0 `proc` stub within the hour, then 3.1 + 3.2, T2 + T3. Move every subprocess onto `proc::run` (coordinate via COORD.md). Last: 3.9 spike. Review A3.
A3 liveness: own `clock.rs`, `watchdog.rs`, `supervisor.rs`, `daemon/poll.rs`. Land `Clock::mono_ms` in Phase 0, then 3.4 + 3.8, T3 + T4. Agree `worktree::snapshot` signature with A5 in COORD.md. Review A4.
A4 identity/state: own `registry.rs`, `durable.rs`, `model.rs`, `daemon/state.rs`, `daemon/types.rs`. Land Phase 0 model fields, then 3.5, T5 + T6 + TSan + loom. Schema owner: every new field goes through you with `#[serde(default)]` + back-compat fixture. Review A5.
A5 isolation: own `attribution.rs`, `daemon/attribution.rs`, `cleanup.rs`, `daemon/spawn.rs`, `worktree.rs`. Do 3.6, T9. Depends on `proc::run` (A2) and model fields (A4). Provide `worktree::snapshot` to A3 and scratch-dir layout to A1. Never auto-merge, never remove dirty worktree. Review A1.

Lead: merge in order, run T10–T12, refuse any PR without red-then-green pair.

## Audit defect list (ordered by blast radius)
1. Decider is RCE hole (`--auto`, untrusted tail, shared session, cwd `/`). 2. Watchdog not independent (blocking LLM calls inside poll; no timeouts). 3. Healthy sessions killed (wall-clock stuck check computed before revision applied). 4. Duplicate IDs (no registry lock across socket threads). 5. Attribution heuristic (shared cwd, git polling). 6. Cleanup cosmetic (empty temp dir only). 7. Features unwired (bus only in tests, A2A leases unwired, `from` unverified, decider flag misleading). 8. WS no auth/Origin check. 9. Wasteful state writes (every 2 s; useless backups). 10. `blocked` mapped to failed, then prompted.
Also: polling spawns dozens of subprocesses/tick; Tauri popover steals focus (can't be NSPanel); plan doc stale re Swift/Tauri.
