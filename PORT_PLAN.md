# PORT PLAN — instruction to a superior coding agent

Read both repositories in full: `/Users/harshitduggal/workspace/notch` (our Rust `notchd` + Tauri + React control plane) and `/Users/harshitduggal/workspace/notch/agentbox` (GPL-3.0-or-later, `package.json:33`). Our fix line is `fix/integration` @ `f308000`; `main` has diverged — never force-push.

Your job: audit every backend module in agentbox and port every superior backend design into our Rust backend. Ignore weak code. Take structure and algorithms, not files. Attribute each port with its exact agentbox path. GPL-3.0 permits copying and modification; keep attribution and license compliance. No shame, no defensiveness — this is legitimate reuse of superior code.

Reject: the 14,616-line supervisor (never reimplement the agent runtime), UI/inbox modules, 34-operation harness breadth, imported dependencies, new frameworks. Already ported: approvals, ledger, durable, harness registry, MCP bridge — deepen, don't repot.

Top target: `pending-writes.mts:59-131` — refused writes return their parked path so a worker can retry; our `ledger.rs` swallows it. Then artifact marks and spawn-failure diagnosis, one algorithm at a time.

Every port: real RED→GREEN test, measured before/after, `clippy -D warnings`, three green gate runs. No new abstractions. Net LOC down. Anything unmeasured is UNVERIFIED with the exact blocker. Make it legitimate and powerful without making it big.
