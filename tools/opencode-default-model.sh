#!/bin/zsh
# opencode-default-model.sh — REPORT the operator's current OpenCode default model.
#
# notch does NOT set, force, or otherwise rewrite the model in the operator's
# OpenCode config. The harness default is the default: every user's machine
# picks its own model, and nothing here leaks a house choice into their config.
# This tool exists only as a read-only diagnostic for debugging/launch scripts.
#
# Env:
#   NOTCH_OC_CONFIG   override config path (for tests). Default: ${XDG_CONFIG_HOME:-~/.config}/opencode/opencode.json
#   NOTCH_OC_EXPECT   'skip' suppresses output. Default: print the current default.
set -uo pipefail

CONFIG="${NOTCH_OC_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/opencode/opencode.json}"

if [[ "${NOTCH_OC_EXPECT:-report}" == "skip" ]]; then
  exit 0
fi

node - "$CONFIG" <<'EOF'
const fs = require('fs');
const file = process.argv[2];
let model = null;
try {
  const doc = JSON.parse(fs.readFileSync(file, 'utf8'));
  const raw = doc.model;
  if (typeof raw === 'string') model = raw;
  else if (raw && typeof raw === 'object') {
    const provider = raw.providerID ?? raw.provider;
    const id = raw.model ?? raw.modelID;
    if (provider && id) model = `${provider}/${id}`;
    else if (id) model = String(id);
  }
} catch (e) {
  console.log(`opencode default model: unreadable (${e.message}); harness chooses`);
  process.exit(0);
}
console.log(model
  ? `opencode default model: ${model} (set by the operator's harness; notch does not override it)`
  : 'opencode default model: not set in config; harness chooses');
EOF
