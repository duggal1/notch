# COORD.md — hardening team coordination

Plan: `docs/HARDENING.md`. Audit: 10 defects, 5 agents, merge order Phase 0 → A2 → A4 → A3 → A1 → A5 → chaos suite.

## Standing overrides (user decisions, 2026-11-20)
- **Git: SOLO MODE ON MAIN.** All agents share one working tree. No branches, no checkout/switch, no rebase/reset/restore/force push. Pull before push, stage only explicit owned paths (never `git add .`/`-A`), 6–12 word commits.
- Terminal: before any herdr work, `herdr workspace create --cwd <fresh temp dir> --label notch-hardening-<id> --no-focus`; work only there; close only it; never touch user's panes.
- Frontend is user-owned: no `ui/**`, no `docs/frontend.md`, no app surface files. Backend only (`crates/notchd/**`).
- Gate: red test first, no `#[ignore]`, no weakened asserts, no `cargo clean`. Check `df -h` before heavy tooling; UNVERIFIED with exact blocker if tools/disk missing.

## Log (append: start / block / push / contract request)
- 2026-11-20 lead: plan persisted as docs/HARDENING.md; team launching A1–A5; Phase 0 contracts first.
- 2026-11-20 lead: A2 owns proc.rs stub + process runner; A4 owns model.rs schema + registry/durable; A3 owns clock.rs; A1 owns decider/socket/ws auth; A5 owns worktree/attribution/cleanup.
- 2026-11-20 A1: START section 3.3/3.7 + T7/T8. Branch is agent/timeout-fix (will NOT switch). Disk 1.2GiB free (<2GiB gate) => cargo mutants/TSan expected UNVERIFIED. FIRST TASK: live-test OPENCODE_PERMISSION deny-all on installed opencode. proc.rs not yet present in src/ -> will build against frozen Spec signature and file contract request.
