#!/bin/zsh
# opencode-default-model.sh — ensure OpenCode's default model is opencode/hy4-flash-free.
# User decision 2026-11-19: HY4 Flash Free is the default, NOT inkling-2-small-free.
# Idempotent. Backs up before every write (timestamped). Atomic write. Prints what it did.
#
# Why: OpenCode ships with a different default (Nemotron). Notch and the build agents spawn
# OpenCode sessions; this enforces the house default before any prompt is ever sent.
# Run it yourself, wire it as a pre-prompt hook, or call it from the launcher.
#
# Env:
#   NOTCH_OC_CONFIG   override config path (for tests). Default: ${XDG_CONFIG_HOME:-~/.config}/opencode/opencode.json
#   NOTCH_OC_EXPECT   'snapshot' (default) prints effective model via `opencode debug config` after ensuring. Set to 'skip' to suppress.
set -euo pipefail

CONFIG="${NOTCH_OC_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/opencode/opencode.json}"
WANT_PROVIDER="opencode"
WANT_MODEL="hy4-flash-free"
WANT_VARIANT=""        # unspecified = provider default variant (no "max" thinking override)

node - "$CONFIG" "$WANT_PROVIDER" "$WANT_MODEL" "$WANT_VARIANT" <<'EOF'
const fs = require('fs');
const [,, file, provider, model, variant] = process.argv;

let doc = {};
try { doc = JSON.parse(fs.readFileSync(file, 'utf8')); } catch (e) {
  console.error(`cannot read ${file}: ${e.message}`);
  process.exit(1);
}
const canonical = variant ? { providerID: provider, model, variant } : `${provider}/${model}`;
const cur = doc.model;
const stringForm = cur === `${provider}/${model}` && !variant;
const objectForm = cur && typeof cur === 'object' && cur.providerID === provider && cur.model === model && variant ? cur.variant === variant : !cur.variant;
const same = stringForm || objectForm;
if (same) {
  console.log(`default already ${provider}/${model}${variant ? ` (variant: ${variant})` : ''} — nothing to do`);
  process.exit(0);
}
const stamp = new Date().toISOString().replace(/[:T]/g, '-').slice(0, 19);
fs.copyFileSync(file, `${file}.bak-${stamp}`);
const merged = { ...doc, model: canonical };          // only the model key changes; everything else preserved
const tmp = `${file}.tmp-${process.pid}`;
fs.writeFileSync(tmp, JSON.stringify(merged, null, 2) + '\n');
fs.renameSync(tmp, file);                             // atomic on same filesystem
console.log(`default model set -> ${typeof canonical === 'string' ? canonical : JSON.stringify(canonical)}`);
console.log(`previous model was -> ${typeof cur === 'string' ? cur : JSON.stringify(cur)}`);
console.log(`backup -> ${file}.bak-${stamp}`);
EOF

if [[ "${NOTCH_OC_EXPECT:-snapshot}" != "skip" ]]; then
  echo "--- effective default (opencode debug config) ---"
  opencode debug config 2>&1 | grep -m1 -E '"model": *(\{|"opencode/hy4-flash-free")' -A2 || { echo "warning: could not verify effective config"; exit 0; }
fi
