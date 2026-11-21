# Integration manifest: `agentbox` → `notch/agentbox-port`

Verified by cw-6512 (role `verify-port`, team `notch`) on 2026-11-21.

## What was ported

A byte-exact vendored snapshot of the `agentbox` repository (an Electron/React
desktop app) copied into `notch/agentbox-port/` by the port agents. The vendored
tree includes the application source (`main`, `renderer`, `shared`, `cloud`,
`mcp`, `mobile`, `bin`), product collateral (`briefs`, `designs`, `docs`,
`scripts`), its `tests` suite, `.github` automation, and the top-level
reference files (`README.md`, `CLAUDE.md`, `CONTRIBUTING.md`, `SECURITY.md`,
`.worktreeinclude`, `preload.cjs`, `LICENSE`).

This is a **source/reference import only** — see "Scope limitation" below.

## Byte-identity results

Each top-level directory was compared with `diff -rq agentbox/<x>
agentbox-port/<x>`. All 13 checked entries are byte-identical:

| Entry | Result |
| --- | --- |
| `main` | OK |
| `renderer` | OK |
| `shared` | OK |
| `cloud` | OK |
| `mcp` | OK |
| `mobile` | OK |
| `bin` | OK |
| `briefs` | OK |
| `designs` | OK |
| `docs` | OK |
| `scripts` | OK |
| `tests` | OK |
| `.github` | OK |

`diff -rq` compares recursively, so "OK" means every file in the directory is
present in the port with identical bytes and no extra/missing files.

`agentbox/LICENSE` → `agentbox-port/LICENSE` is byte-identical (`cmp` clean),
35,149 bytes, SHA-256
`3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986`.

## File counts

- `agentbox-port` files: **4827** (`find agentbox-port -type f`)
- `agentbox` files: **4832** (`find agentbox -type f -not -path '*/.git/*'
  -not -path '*/node_modules/*'`)
- Delta: **5 files**, all intentionally outside the port scope (local/hidden
  metadata, not application source):
  - `.agentbox/cloud-setup.sh`
  - `.agentbox/private-folder.sh`
  - `.agentbox/private.json`
  - `.gitignore`
  - `AGENTS.md`
- Files present in the port but not in the source: **none**.

## Product UI and back end untouched

Checked before and again after the builds; all outputs empty:

- `git status --short -- ui` → empty
- `git diff --stat HEAD -- ui` → empty
- `git status --short -- app crates` → empty

Only `agentbox-port/*` paths appear as new files in `git status`. No tracked
notch product file was modified, staged, or deleted by this port.

## Headless build results

Both checks ran headless and passed, proving the port did not break notch's own
build:

- `cargo check --workspace` → exit 0, `Finished dev profile in 7.68s`
  (`notchd v0.1.0` checked clean).
- `cd ui && npm run build` → exit 0, `tsc --noEmit && vite build` succeeded,
  `494 modules transformed`, built in 423ms.

No GUI, browser, or preview was opened for these checks.

## License / attribution

`agentbox` is licensed **GPL-3.0-or-later** (`package.json`:
`"license": "GPL-3.0-or-later"`; `README.md` carries the GPL-3.0 badge and a
"GPL-3.0-or-later. See [LICENSE](LICENSE)." section). The full GPL-3.0 license
text is reproduced verbatim at `agentbox-port/LICENSE`. This attribution is
retained intentionally with the vendored source.

## Scope limitation (important)

The Electron/React source under `notch/agentbox-port/` is **vendored as
reference/source material only. It is NOT wired into notch's Tauri app at
runtime.** The two projects use different stacks — agentbox is Electron/React,
while notch's product UI is a Tauri/Rust app under `ui/` and `app/`. There are
no runtime imports, shared build steps, or code paths connecting the port to
notch's application; the clean product-tree status and the passing notch-only
builds above confirm this isolation. The port exists for reference and future
evaluation, not as a running component of notch.
