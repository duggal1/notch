# ADOPT_PLAN.md — port superior backend code from agentbox into Notch

Orchestrator note, 2026-11-20. Read-only. No code changed by this plan.

## 0. Correcting the word "steal"

The instruction was to "steal superior code from agentbox." Stated precisely, because this is the version I can stand behind:

- `agentbox` is a **separate Git repository** (`9494b19f`) at
  `/Users/harshitduggal/workspace/notch/agentbox`, **0 of its files are tracked by notch**.
- It is **GPL-3.0** (verified: `agentbox/LICENSE`, GPL v3 text). Its README badge says GPL-3.0.
- Copying its source verbatim into notch would attach GPL-3.0 obligations to the
  combined work. That is a real consequence, not a detail to wave away.
- Notch **already ports five agentbox designs** and says so in its own source:
  `approvals.rs:3`, `durable.rs:30`, `ledger.rs:3`, `harness_registry.rs:2`,
  `mcp_bridge.rs:3`. Every one is a structure/algorithm port with attribution,
  not a file copy. That is the correct model and this plan follows it.

So: **port superior structure and algorithms with attribution and license compliance.**
Do not copy files wholesale. Do not import agentbox's dependencies, frameworks,
config systems, or UI. No over-engineering — a port must be smaller than the bug
it fixes or the capability it adds, or it does not happen.

## 1. Validation of our repository (what is real right now)

- Source of truth for fixes: `fix/integration` = `f308000`, 48 commits ahead of
  `main` (`b111c8c`, which diverged via external PR #5).
- Gate: three consecutive green runs on `f308000` — 9/9 steps,
  **207 passed / 0 failed / 1 ignored**, wiring-check OK. Evidence:
  `.proof/fix/gate-final-do{1,2,3}.md`.
- 11 agents DONE; all evidence in `.proof/fix/a1.md` … `a11.md`.
- Known open items: A1 race fix `6063ee1` unmerged (no RED/GREEN test);
  coverage 387→207 after A11's fake-CLI deletion; A9 live-state breach;
  A3/A9/A2 partner-approval gaps; dev target 7.6 GB over cap.
- Notch stack: Rust `notchd` (lib + daemon + CLI), Tauri v2 shell, React 19 UI.

## 2. What is genuinely superior in agentbox and missing from notch

Validated by file inspection (line counts and exact paths), not by reputation.

| # | Agentbox source | Superior property | Notch today | Adopt? |
|---|---|---|---|---|
| A | `main/store/work-items.mts:70-113` + `shared/work-items.mts` | One `O_APPEND` write of one buffer; short write throws; epoch minted per claim; zombie-writer lines discarded by fold; refused writes parked with path in error | `durable.rs`, `ledger.rs` already port this | **Already adopted** — deepen, don't repot |
| B | `main/store/pending-writes.mts:59-131` | Refused write returns **where it was parked**, so a worker can re-claim and retry; collection MOVES to `collected/`, never deletes | `ledger.rs` parks but `let _ = park(...)` swallows; no path surfaced | **YES** — surface parked path in error; smallest change |
| C | `main/approvals.mts:68-103` + `shared/approval-card.mts` | ed25519 with **public verifier handed to workers**; digest of the card *displayed*, not the file at answer time | HMAC-SHA256, both sign+verify in-process; digest correct | **Selective** — keep HMAC (§7 forbids swapping); adopt the "digest of displayed card" discipline already present |
| D | `main/harnesses.mts` + `harnesses/common.mts` | 34-operation capability set; decliners refuse before side effects; optional reads report `Absent`; registered ≠ offered | `harness_registry.rs` ports 8 ops and these rules | **Selective** — add only `models`-class optional reads if a real finding needs them; no 34-op expansion |
| E | `main/notify.mts`, `notes-wake.mts`, `phone-inbox.mts` (992 lines) | Inbox as the primary UI: 12 agents finish, only the ones needing you arrive; wake on state change, not polling | Polling 2 s + snapshot events; no wake/inbox | **NO for now** — UI/product redesign, out of scope (§7) |
| F | `main/supervisor.mts` (14,616 lines) | Mature spawn/continuation/transcript machinery | `daemon/poll.rs` + `supervise.rs` (thin, black-box) | **NO** — violates the core law: never reimplement the agent runtime |
| G | `shared/artifact-marks.mts` (1,152), `main/task-folders.mts` (1,415), `shared/spawn-trouble.mts` (1,145) | Exact attribution marks, task folder lifecycle, spawn-failure diagnosis per engine | `attribution.rs`, `worktree.rs`, capability decliners | **STUDY only** — take a single algorithm if a measured gap appears; no bulk port |

## 3. The plan (ordered, minimal)

1. **B — parked-write path surfaced (do first).** Change `ledger.rs` so a refused write's
   parked path reaches the caller's error string, and stop swallowing `park()` errors.
   Real regression test: stale-epoch write refused → error contains the parked path →
   file exists. This closes A1's finding and is the one clear superiority gap.
2. **A — verify the existing ports are exact.** Add tests that pin the atomic invariants
   already claimed (one-write-per-line, short-write throws, fold idempotence). No new code
   in production; test-only.
3. **C/D — document the deliberate deviations.** One paragraph each in `approvals.rs` and
   `harness_registry.rs`: agentbox uses ed25519 and 34 ops; notch uses HMAC and 8 ops, and
   why. No behavior change.
4. **G — one-page study note** on artifact marks / task folders / spawn-trouble with a
   verdict per algorithm: adopt / observe / reject. No code.
5. **E/F — explicitly rejected**, with reasons in §2, so nobody "helpfully" imports them.

## 4. Non-goals

No new features. No W2 expansion. No A2A attach. No harness breadth. No UI/inbox redesign.
No agentbox dependencies. No verbatim file copies. No GPL-3.0 obligations taken on silently.
No new lanes — the 11 agents are DONE; this is lead analysis.

## 5. Definition of done for this plan

Each adopted item has: attribution comment with exact agentbox path, a real regression
test shown RED→GREEN, and a gate green 3× on `fix/integration`. Anything not measured is
UNVERIFIED with the exact blocker. The main-branch merge (5 conflicts, §lead report) stays
a separate human decision.
