#!/usr/bin/env node
/**
 * contract-check.mjs — Layer B of the contract-drift guard (bridge.md v1.3).
 *
 * Layer A (crates/notchd/tests/conformance_snapshot.rs) proves the snapshot
 * shape in-process against a fake runner. Layer B proves the LIVE daemon
 * honors the same contract over the real NDJSON socket, from JavaScript, and
 * that the command surface matches docs/bridge.md §2. Zero dependencies
 * (node stdlib only). Nothing is ever sent to the real herdr or the real
 * `opencode` CLI — a hermetic fixture shim is generated per run.
 *
 * Usage:
 *   node tools/contract-check.mjs
 *
 * What it does:
 *   1. Self-tests its own §1.3 validator against 2 good + 8 bad canned frames,
 *      including a frame carrying the undocumented `supervisors[].item.title`
 *      key the Rust layer caught (exit 2 if the checker itself is broken).
 *   2. Diffs docs/bridge.md §2's command table against the hardcoded
 *      COMMAND_MANIFEST below — a stale manifest is a failure.
 *   3. Generates a fake HERDR shim (+ fake `opencode`) inside a fresh temp
 *      dir (mktemp under /private/var/folders/ct/cwpf7vpn059fkp932tq9zm4m0000gn/T/contract-XXXXXX —
 *      NOT notch-*). No external fixture files are needed; the fixtures are
 *      rewritten on every run and removed afterwards.
 *   4. Spawns `cargo run -q -p notchd -- serve` with NOTCH_STATE_DIR,
 *      NOTCH_SOCK, NOTCH_HERDR_CMD and NOTCH_OPENCODE_CMD pointing at that
 *      temp dir. External NOTCH_* values from your shell are ignored.
 *   5. Speaks NDJSON: reads the `hello` frame, then pings every §2 command in
 *      the manifest (harmless probes — no task is submitted), then sends
 *      `subscribe` and captures one snapshot frame.
 *      The captured frame is the empty tree (fresh state dir; NO fake task
 *      is submitted). Per-item key rules are exercised by the self-test
 *      canned frames above and by Layer A against the real Daemon.
 *   6. Validates: §1.3 snapshot rules on the captured frame (exact key sets,
 *      camelCase keys, status enum, f64 eps lexical form, integer-ms fields,
 *      version == 1), the `hello` frame, and every command's ack envelope
 *      against the manifest. The daemon must NOT answer `unknown_command`
 *      for any manifest command.
 *   7. On any violation prints a readable diff and exits 1. Infra problems
 *      (no cargo, daemon did not come up, checker broken) exit 2.
 *
 * Env:
 *   CONTRACT_CHECK_KEEP_TMP=1   keep the temp dir for inspection (default: removed)
 *   CONTRACT_CHECK_BRIDGE_MD    override the docs/bridge.md path (checker testing)
 *
 * Exit codes:
 *   0  contract holds
 *   1  contract violations found (diff printed to stderr)
 *   2  checker/infra failure (not a verdict on the contract)
 *
 * THE MANIFEST MIRRORS docs/bridge.md §2 (frozen v1.3). If the contract
 * changes with lead sign-off, update COMMAND_MANIFEST and the key lists in
 * this file and in conformance_snapshot.rs together. Do not update them to
 * make a failure go away.
 */

import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..');
const TEMPLATES_ROOT = '/private/var/folders/ct/cwpf7vpn059fkp932tq9zm4m0000gn/T';
const TMP_PREFIX = 'contract-'; // NOT notch-* — required by the tasking

const FRAME_TIMEOUT_MS = 10_000;
const SOCK_WAIT_MS = 180_000; // first `cargo run` may compile
const CONNECT_RETRIES = 100;
const BRIDGE_MD = process.env.CONTRACT_CHECK_BRIDGE_MD || path.join(REPO_ROOT, 'docs', 'bridge.md');

// ---------- canonical key lists (checked in; mirror docs/bridge.md §1.3 v1.3) ----------

const TOP_REQUIRED = [
  'version', 'generatedAt', 'orchestrator', 'supervisors', 'sessions',
  'eps', 'conflicts', 'attribution', 'warnings',
];
const TOP_OPTIONAL = ['decisions', 'escalations', 'busMessages'];

const ORCHESTRATOR_KEYS = ['id', 'status'];

const SUPERVISOR_REQUIRED = ['id', 'status', 'working', 'idle', 'failed', 'done'];
const SUPERVISOR_OPTIONAL = ['paused'];

const SESSION_REQUIRED = [
  'id', 'sup', 'status', 'title', 'task', 'cwd', 'paneId',
  'startedAt', 'lastAction', 'lastRevision', 'latencyMs', 'failureReason',
];
const SESSION_OPTIONAL = [
  'goal', 'definitionOfDone', 'iterations', 'maxIterations',
  'wallClockDeadline', 'lastSupervisorAction', 'model',
];

const CONFLICT_KEYS = ['file', 'sessions'];
const ATTRIBUTION_KEYS = ['session', 'file'];
const DECISION_KEYS = ['at', 'supervisor', 'session', 'action', 'reason'];
const ESCALATION_KEYS = ['id', 'session', 'supervisor', 'kind', 'reason', 'at'];
const BUS_MESSAGE_KEYS = ['id', 'from', 'to', 'type', 'preview', 'at'];

const STATUS_ENUM = ['working', 'idle', 'failed', 'done'];

// ---------- §1.3 validator (mirrors conformance_snapshot.rs) ----------

const isCamelCaseKey = (k) => /^[a-z][a-zA-Z0-9]*$/.test(k);

const checkObjectKeys = (obj, required, optional, p, out) => {
  if (obj === null || typeof obj !== 'object' || Array.isArray(obj)) {
    out.push(`${p}: expected an object, got ${JSON.stringify(obj)}`);
    return;
  }
  const allowed = new Set([...required, ...optional]);
  for (const r of required) {
    if (!Object.prototype.hasOwnProperty.call(obj, r)) out.push(`${p}: missing required key \`${r}\``);
  }
  for (const k of Object.keys(obj)) {
    if (!allowed.has(k)) {
      out.push(`${p}: unexpected key \`${k}\` (allowed here: [${[...allowed].join(', ')}])`);
    }
  }
};

const checkItems = (v, field, required, optional, itemName, out) => {
  if (!Object.prototype.hasOwnProperty.call(v, field)) {
    out.push(`top: missing required array \`${field}\``);
    return;
  }
  const arr = v[field];
  if (!Array.isArray(arr)) {
    out.push(`top: \`${field}\` must be an array`);
    return;
  }
  arr.forEach((item, i) => checkObjectKeys(item, required, optional, `${itemName}[${i}]`, out));
};

const snapshotKeyViolations = (v) => {
  const out = [];
  checkObjectKeys(v, TOP_REQUIRED, TOP_OPTIONAL, '<top>', out);
  if (v && typeof v === 'object') {
    if (v.orchestrator === undefined) out.push('<top>: missing required key `orchestrator`');
    else checkObjectKeys(v.orchestrator, ORCHESTRATOR_KEYS, [], 'orchestrator', out);
    checkItems(v, 'supervisors', SUPERVISOR_REQUIRED, SUPERVISOR_OPTIONAL, 'supervisors', out);
    checkItems(v, 'sessions', SESSION_REQUIRED, SESSION_OPTIONAL, 'sessions', out);
    checkItems(v, 'conflicts', CONFLICT_KEYS, [], 'conflicts', out);
    checkItems(v, 'attribution', ATTRIBUTION_KEYS, [], 'attribution', out);
    if (v.decisions !== undefined) checkItems(v, 'decisions', DECISION_KEYS, [], 'decisions', out);
    if (v.escalations !== undefined) checkItems(v, 'escalations', ESCALATION_KEYS, [], 'escalations', out);
    if (v.busMessages !== undefined) checkItems(v, 'busMessages', BUS_MESSAGE_KEYS, [], 'busMessages', out);
  }
  return out;
};

const walkKeys = (v, p, out) => {
  if (Array.isArray(v)) {
    v.forEach((item, i) => walkKeys(item, `${p}[${i}]`, out));
    return;
  }
  if (v !== null && typeof v === 'object') {
    for (const [k, sub] of Object.entries(v)) {
      if (!isCamelCaseKey(k)) out.push(`${p}.${k}: key does not match ^[a-z][a-zA-Z0-9]*$`);
      walkKeys(sub, `${p}.${k}`, out);
    }
  }
};

const statusViolations = (v) => {
  const out = [];
  const seen = [];
  if (v && v.orchestrator && typeof v.orchestrator.status === 'string') {
    seen.push(['orchestrator.status', v.orchestrator.status]);
  }
  (Array.isArray(v?.supervisors) ? v.supervisors : []).forEach((s, i) => {
    if (typeof s?.status === 'string') seen.push([`supervisors[${i}].status`, s.status]);
  });
  (Array.isArray(v?.sessions) ? v.sessions : []).forEach((s, i) => {
    if (typeof s?.status === 'string') seen.push([`sessions[${i}].status`, s.status]);
  });
  for (const [p, s] of seen) {
    if (!STATUS_ENUM.includes(s)) out.push(`${p}: \`${s}\` is not one of ${STATUS_ENUM.join('|')}`);
  }
  return out;
};

const integerMsViolations = (v) => {
  const out = [];
  const assertIntMs = (x, p) => {
    if (typeof x !== 'number' || !Number.isInteger(x) || x < 0) {
      out.push(`${p}: expected integer ms since epoch, got ${JSON.stringify(x)}`);
    }
  };
  if (v && typeof v === 'object') {
    assertIntMs(v.generatedAt, 'generatedAt');
    (Array.isArray(v.sessions) ? v.sessions : []).forEach((s, i) => {
      assertIntMs(s?.startedAt, `sessions[${i}].startedAt`);
      if (s?.wallClockDeadline !== undefined) assertIntMs(s.wallClockDeadline, `sessions[${i}].wallClockDeadline`);
    });
    (Array.isArray(v.decisions) ? v.decisions : []).forEach((d, i) => assertIntMs(d?.at, `decisions[${i}].at`));
    (Array.isArray(v.escalations) ? v.escalations : []).forEach((e, i) => assertIntMs(e?.at, `escalations[${i}].at`));
    (Array.isArray(v.busMessages) ? v.busMessages : []).forEach((m, i) => assertIntMs(m?.at, `busMessages[${i}].at`));
  }
  return out;
};

/** Raw-text check: `eps` must be serialized as an f64 (fractional or exponent form). */
const epsLexemeViolations = (raw) => {
  const m = raw.match(/"eps"\s*:\s*(-?\d+(\.\d+)?([eE][+-]?\d+)?)/);
  if (!m) return ['eps: missing or unparsable in raw frame'];
  if (!/[.eE]/.test(m[1])) {
    return [`eps: must serialize as an f64, got lexical form "${m[1]}"`];
  }
  return [];
};

// A canonical frame is ALWAYS { raw: string, json: parsedOrUndefined } — as
// produced by the socket reader, and by frameOf() in the self-test. Never a
// bare parsed object; that distinction is what keeps the f64-lexeme check
// honest (parsed JSON cannot see `0.0` vs `0`).
const frameOf = (json, raw) => ({ raw: raw ?? JSON.stringify(json), json });

/** Full §1.3 validation of one snapshot frame. */
const validateSnapshotFrame = (frame) => {
  const out = [];
  if (!frame || typeof frame !== 'object') return ['snapshot frame: not a frame object'];
  const { raw, json: f } = frame;
  if (f === undefined || f === null || typeof f !== 'object' || Array.isArray(f)) {
    return [`snapshot frame: not a JSON object: ${String(raw).slice(0, 200)}`];
  }
  if (f.event !== 'snapshot') out.push(`snapshot frame: expected event "snapshot", got ${JSON.stringify(f.event)}`);
  if (f.cmd !== undefined) out.push('snapshot frame: must not be an ack (unexpected key `cmd`)');
  if (f.ok !== undefined) out.push('snapshot frame: must not carry `ok`');
  // Lexical f64 check must run on the raw line, not the parsed object.
  out.push(...epsLexemeViolations(raw ?? ''));
  const v = f.snapshot;
  if (v === null || typeof v !== 'object' || Array.isArray(v)) {
    out.push(`snapshot frame: \`snapshot\` must be an object, got ${JSON.stringify(v)}`);
    return out;
  }
  if (v.version !== 1) out.push(`<top>: version must be exactly 1, got ${JSON.stringify(v.version)}`);
  if (typeof v.eps !== 'number' || !Number.isFinite(v.eps)) {
    out.push(`eps: must be a finite number, got ${JSON.stringify(v.eps)}`);
  }
  out.push(...snapshotKeyViolations(v));
  const camel = [];
  walkKeys(v, '$', camel);
  out.push(...camel);
  out.push(...statusViolations(v));
  out.push(...integerMsViolations(v));
  if (v.warnings !== undefined) {
    if (!Array.isArray(v.warnings) || v.warnings.some((w) => typeof w !== 'string')) {
      out.push('warnings: must be an array of strings');
    }
  }
  return out;
};

// ---------- validator self-test (good + bad canned frames) ----------

// NOTE: JS numbers can't spell `0.0` — the raw line is what the f64-lexeme
// check inspects, so canned frames carry explicit raw text.
const GOOD_EMPTY = frameOf(
  {
    event: 'snapshot',
    snapshot: {
      version: 1,
      generatedAt: 1_763_520_000_000,
      orchestrator: { id: 'ORCH', status: 'idle' },
      supervisors: [],
      sessions: [],
      eps: 0,
      conflicts: [],
      attribution: [],
      warnings: [],
    },
  },
  '{"event":"snapshot","snapshot":{"version":1,"generatedAt":1763520000000,"orchestrator":{"id":"ORCH","status":"idle"},"supervisors":[],"sessions":[],"eps":0.0,"conflicts":[],"attribution":[],"warnings":[]}}'
);

const GOOD_FULL = frameOf({
  event: 'snapshot',
  snapshot: {
    version: 1,
    generatedAt: 1_763_520_000_000,
    orchestrator: { id: 'ORCH', status: 'working' },
    supervisors: [
      { id: 'SV-1', status: 'failed', working: 0, idle: 0, failed: 1, done: 1, paused: true },
    ],
    sessions: [
      {
        id: 'S1-OC01', sup: 'SV-1', status: 'done',
        title: 'Audit sign-off', task: 'Ship the audit-signoff release checklist',
        cwd: '/tmp/repo', paneId: 'w91:p1', startedAt: 1_763_520_000_000,
        lastAction: null, lastRevision: 7, latencyMs: 840, failureReason: null,
        goal: 'Ship the audit-signoff release checklist',
        definitionOfDone: 'cargo test passes on main',
        iterations: 1, maxIterations: 8,
        wallClockDeadline: 1_763_523_600_000,
        lastSupervisorAction: 'reprompt: CONTINUE: finish the checklist',
        model: 'opencode/hy4-flash-free',
      },
    ],
    eps: 0.3,
    conflicts: [{ file: 'src/shared.rs', sessions: ['S1-OC01', 'S1-OC02'] }],
    attribution: [{ session: 'S1-OC01', file: 'src/shared.rs' }],
    decisions: [{ at: 1_763_520_000_000, supervisor: 'SV-1', session: 'S1-OC01', action: 'reprompt', reason: 'tests not run' }],
    escalations: [{ id: 'esc_1', session: 'S1-OC02', supervisor: 'SV-1', kind: 'blocked', reason: 'blocked_waiting', at: 1_763_520_000_000 }],
    busMessages: [{ id: 'msg_1', from: 'S1-OC01', to: 'S1-OC02', type: 'handoff', preview: 'cutover notes', at: 1_763_520_000_000 }],
    warnings: ['S1-OC02 escalated: blocked'],
  },
});

const deepClone = (x) => JSON.parse(JSON.stringify(x));

// Re-wrap a mutated canned snapshot object into the canonical frame shape.
const withRaw = (obj) => frameOf(obj);

const selfTestCases = () => {
  const goodEmpty = GOOD_EMPTY;
  const goodFull = GOOD_FULL;

  const extraTop = deepClone(GOOD_EMPTY).json;
  extraTop.snapshot.extraTop = 'x';

  const snakeNested = deepClone(GOOD_FULL).json;
  snakeNested.snapshot.busMessages[0].agent_status = 'working';

  const missingWarnings = deepClone(GOOD_EMPTY).json;
  delete missingWarnings.snapshot.warnings;

  const badStatus = deepClone(GOOD_FULL).json;
  badStatus.snapshot.sessions[0].status = 'blocked';

  const badVersion = deepClone(GOOD_EMPTY).json;
  badVersion.snapshot.version = 2;

  const badV12Vanished = deepClone(GOOD_FULL).json;
  delete badV12Vanished.snapshot.orchestrator.id; // a v1.2 key silently dropped

  const badSlithered = deepClone(GOOD_EMPTY).json;
  badSlithered.snapshot.supervisors = [{ id: 'SV-1', status: 'idle', working: 0, idle: 1, failed: 0, done: 0, snake_key: 1 }];

  // The live drift this guard caught first: an undocumented additive key on
  // supervisors[].item (`title` — emitted by the daemon, absent from the
  // frozen §1.3 shape and from the v1.3 allowed-optional list).
  const supervisorTitle = deepClone(GOOD_FULL).json;
  supervisorTitle.snapshot.supervisors[0].title = 'Audit sign-off';

  return [
    { name: 'good empty-tree frame passes', frame: goodEmpty, expectValid: true },
    { name: 'good full v1.3 frame passes', frame: goodFull, expectValid: true },
    { name: 'extra top-level key rejected', frame: withRaw(extraTop), expectValid: false },
    { name: 'snake_case nested key rejected', frame: withRaw(snakeNested), expectValid: false },
    { name: 'missing required key rejected', frame: withRaw(missingWarnings), expectValid: false },
    { name: 'non-enum status rejected', frame: withRaw(badStatus), expectValid: false },
    { name: 'version != 1 rejected', frame: withRaw(badVersion), expectValid: false },
    { name: 'v1.2 key vanished rejected', frame: withRaw(badV12Vanished), expectValid: false },
    { name: 'snake_case key at item level rejected', frame: withRaw(badSlithered), expectValid: false },
    { name: 'undocumented supervisor `title` rejected', frame: withRaw(supervisorTitle), expectValid: false },
  ];
};

const runSelfTest = () => {
  const results = [];
  for (const tc of selfTestCases()) {
    const violations = validateSnapshotFrame(tc.frame);
    const valid = violations.length === 0;
    results.push({ name: tc.name, pass: valid === tc.expectValid, detail: violations.join('; ') });
  }
  // Raw-lexeme eps check lives outside the structural validator path for canned frames.
  const epsInt = validateSnapshotFrame(frameOf(
    {
      event: 'snapshot',
      snapshot: { version: 1, generatedAt: 1763520000000, orchestrator: { id: 'ORCH', status: 'idle' }, supervisors: [], sessions: [], eps: 3, conflicts: [], attribution: [], warnings: [] },
    },
    '{"event":"snapshot","snapshot":{"version":1,"generatedAt":1763520000000,"orchestrator":{"id":"ORCH","status":"idle"},"supervisors":[],"sessions":[],"eps":3,"conflicts":[],"attribution":[],"warnings":[]}}'
  ));
  results.push({ name: 'integer eps lexeme rejected', pass: epsInt.some((m) => m.includes('f64')), detail: epsInt.join('; ') });
  return results;
};

// ---------- §2 command manifest (mirror docs/bridge.md §2, frozen v1.3) ----------

// Every safe probe keeps the fresh daemon empty and side-effect free:
// errors are preferred over success for mutating commands.
const COMMAND_MANIFEST = [
  { cmd: 'ping',             request: { cmd: 'ping' },                                                    expect: { event: 'ack', ok: true, fields: { pong: 'integer' } } },
  { cmd: 'snapshot',         request: { cmd: 'snapshot' },                                                expect: { event: 'snapshot' } },
  // subscribe is validated in its own phase (it emits two frames and would desync the manifest loop).
  { cmd: 'subscribe',        request: { cmd: 'subscribe' },                                               expect: { event: 'ack', ok: true }, skip: 'validated in the subscribe phase' },
  { cmd: 'run_plan',         request: { cmd: 'run_plan', path: '/nonexistent/notch-contract-check-plan.json' }, expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'submit_task',      request: { cmd: 'submit_task', task: '' },                                   expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'message',          request: { cmd: 'message', from: 'S1-OC01', to: 'S2-OC01', type: 'nope', body: 'probe' }, expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'pause_supervisor', request: { cmd: 'pause_supervisor', id: 'SV-99' },                           expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'resume_supervisor',request: { cmd: 'resume_supervisor', id: 'SV-99' },                          expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'kill_switch',      request: { cmd: 'kill_switch' },                                             expect: { event: 'ack', ok: true, fields: { closed: 'integer' } } },
  { cmd: 'list_models',      request: { cmd: 'list_models' },                                             expect: { event: 'ack', ok: true, fields: { models: 'array' } } },
  { cmd: 'logs',             request: { cmd: 'logs', id: 'S1-OC99' },                                     expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'reprompt',         request: { cmd: 'reprompt', id: 'S1-OC99', text: 'probe' },                  expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'cleanup',          request: { cmd: 'cleanup' },                                                 expect: { event: 'ack', ok: true, fields: { removed: 'integer' } } },
  { cmd: 'focus',            request: { cmd: 'focus', id: 'S1-OC99' },                                    expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'kill',             request: { cmd: 'kill', id: 'S1-OC99' },                                     expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'spawn',            request: { cmd: 'spawn' },                                                   expect: { event: 'ack', ok: false, errorNonEmpty: true } },
  { cmd: 'status',           request: { cmd: 'status' },                                                  expect: { event: 'status', fields: { pid: 'integer', sessions: 'integer', uptimeS: 'integer' } } },
];

/** Diff docs/bridge.md §2's client→daemon command table against the hardcoded manifest. */
const bridgeCommandDiff = () => {
  const text = fs.readFileSync(BRIDGE_MD, 'utf8');
  const section = text.split('### Client → daemon')[1];
  if (!section) return { fatal: 'docs/bridge.md: `### Client → daemon` section not found' };
  const table = section.split('### ')[0];
  const cmds = [...new Set([...table.matchAll(/^\|\s*`([a-z_]+)`/gm)].map((m) => m[1]))];
  if (cmds.length === 0) {
    return { fatal: 'docs/bridge.md §2: no commands parsed from the client→daemon table (doc format changed? — update this checker, do not weaken it)' };
  }
  const manifest = COMMAND_MANIFEST.map((e) => e.cmd);
  const missing = cmds.filter((c) => !manifest.includes(c));
  const stale = manifest.filter((c) => !cmds.includes(c));
  return { missing, stale, doc: cmds, manifest };
};

// ---------- NDJSON socket client ----------

const makeFrameReader = (socket) => {
  let buf = '';
  const queue = [];
  const waiters = [];
  socket.on('data', (chunk) => {
    buf += chunk.toString('utf8');
    let idx;
    while ((idx = buf.indexOf('\n')) !== -1) {
      const line = buf.slice(0, idx);
      buf = buf.slice(idx + 1);
      if (!line.trim()) continue;
      let json;
      try { json = JSON.parse(line); } catch { json = undefined; }
      const frame = { raw: line, json };
      if (waiters.length) waiters.shift()(frame);
      else queue.push(frame);
    }
  });
  return (label, timeoutMs = FRAME_TIMEOUT_MS) =>
    new Promise((resolve, reject) => {
      if (queue.length) return resolve(queue.shift());
      const timer = setTimeout(() => {
        const i = waiters.findIndex((w) => w.timer === timer);
        if (i !== -1) waiters.splice(i, 1);
        reject(new Error(`timeout (${timeoutMs}ms) waiting for frame after ${label}`));
      }, timeoutMs);
      const waiter = (frame) => { clearTimeout(timer); resolve(frame); };
      waiter.timer = timer;
      waiters.push(waiter);
    });
};

const sendCmd = (socket, obj) => socket.write(JSON.stringify(obj) + '\n');

// ---------- manifest response checker ----------

const checkExpect = (entry, frame, violations) => {
  const f = frame?.json;
  const { cmd, expect } = entry;
  if (f === undefined || f === null || typeof f !== 'object') {
    violations.push(`${cmd}: frame is not a JSON object: ${JSON.stringify(frame?.raw)}`);
    return;
  }
  if (f.error === 'unknown_command') {
    violations.push(`${cmd}: daemon answered unknown_command — the command documented in bridge.md §2 is missing from the implementation`);
    return;
  }
  if (f.event !== expect.event) {
    violations.push(`${cmd}: expected event "${expect.event}", got ${JSON.stringify(f.event)} (frame: ${frame.raw.slice(0, 300)})`);
    return;
  }
  if (expect.event === 'ack') {
    if (f.cmd !== cmd) violations.push(`${cmd}: ack must echo cmd "${cmd}", got ${JSON.stringify(f.cmd)}`);
    if (typeof f.ok !== 'boolean') violations.push(`${cmd}: ack ok must be a boolean, got ${typeof f.ok}`);
    else if (f.ok !== expect.ok) violations.push(`${cmd}: ack ok must be ${expect.ok}, got ${f.ok} (frame: ${frame.raw.slice(0, 300)})`);
    if (expect.errorNonEmpty && !(typeof f.error === 'string' && f.error.length > 0)) {
      violations.push(`${cmd}: error ack must carry a non-empty error string (frame: ${frame.raw.slice(0, 300)})`);
    }
  }
  if (expect.event === 'snapshot') {
    violations.push(...validateSnapshotFrame(frame).map((m) => `snapshot: ${m}`));
  }
  if (expect.fields) {
    for (const [name, type] of Object.entries(expect.fields)) {
      const val = f[name];
      if (val === undefined) { violations.push(`${cmd}: missing documented field \`${name}\` (frame: ${frame.raw.slice(0, 300)})`); continue; }
      if (type === 'integer') {
        if (typeof val !== 'number' || !Number.isInteger(val)) violations.push(`${cmd}: \`${name}\` must be an integer, got ${JSON.stringify(val)}`);
      } else if (type === 'array') {
        if (!Array.isArray(val)) violations.push(`${cmd}: \`${name}\` must be an array, got ${JSON.stringify(val)}`);
      }
    }
  }
};

// ---------- fixture shims ----------

const FAKE_HERDR = `#!/bin/sh
# Generated by tools/contract-check.mjs — hermetic fake herdr. Never calls the real herdr.
case "$1 $2" in
  "agent list") printf '{"id":"cli:agent:list","result":{"type":"agent_list","agents":[]}}\\n' ;;
  "pane list")  printf '{"id":"cli:pane:list","result":{"type":"pane_list","panes":[]}}\\n' ;;
  "pane split") printf '{"id":"cli:pane:split","result":{"type":"pane_info","pane":{"pane_id":"w91:p1","tab_id":"w1:t1","workspace_id":"w1"}}}\\n' ;;
  "agent start") printf '{"id":"cli:agent:start","result":{"type":"agent_started","agent":{"pane_id":"w91:p1","agent_status":"working","revision":1},"argv":["opencode"]}}\\n' ;;
  "agent prompt") printf '{"id":"cli:agent:prompt","result":{"type":"agent_prompted","agent":{"pane_id":"w91:p1","agent_status":"working","revision":2}}}\\n' ;;
  "agent read") printf 'fake pane output line\\n' ;;
  "agent wait") printf '{"id":"cli:agent:wait","result":{"type":"ok"}}\\n' ;;
  "pane close") printf '{"id":"cli:pane:close","result":{"type":"ok"}}\\n' ;;
  "agent focus") printf '{"id":"cli:agent:focus","result":{"type":"ok"}}\\n' ;;
  *) echo "fake-herdr: unexpected args: $*" >&2; exit 2 ;;
esac
`;

const FAKE_OPENCODE = `#!/bin/sh
# Generated by tools/contract-check.mjs — hermetic fake opencode. Never calls the real opencode.
case "$1" in
  api) printf '{"cursor":null,"data":[]}\\n' ;;
  models) printf 'opencode/hy4-flash-free\\nopencode/inkling-2-small-free\\n' ;;
  *) echo "fake-opencode: unexpected args: $*" >&2; exit 2 ;;
esac
`;

// ---------- main ----------

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const main = async () => {
  const violations = [];
  const notes = [];

  // 1. Validator self-test.
  const selfTest = runSelfTest();
  const selfFailures = selfTest.filter((r) => !r.pass);
  if (selfFailures.length) {
    console.error('contract-check: FAIL — the checker itself is broken (exit 2):');
    for (const f of selfFailures) console.error(`  - ${f.name}\n      ${f.detail}`);
    process.exit(2);
  }
  notes.push(`validator self-test ${selfTest.length}/${selfTest.length}`);

  // 2. Manifest vs docs/bridge.md §2.
  const diff = bridgeCommandDiff();
  if (diff.fatal) {
    console.error(`contract-check: FAIL — checker could not parse the contract docs (exit 2): ${diff.fatal}`);
    process.exit(2);
  }
  for (const c of diff.missing) {
    violations.push(`docs/bridge.md §2 documents \`${c}\` but COMMAND_MANIFEST does not — update the manifest with lead sign-off`);
  }
  for (const c of diff.stale) {
    violations.push(`COMMAND_MANIFEST lists \`${c}\` which docs/bridge.md §2 does not — stale manifest`);
  }

  // 3. Temp dir + fixtures.
  if (!fs.existsSync(path.join(REPO_ROOT, 'Cargo.toml'))) {
    console.error(`contract-check: FAIL — repo root not found at ${REPO_ROOT}`);
    process.exit(2);
  }
  const tmp = fs.mkdtempSync(path.join(TEMPLATES_ROOT, TMP_PREFIX));
  const keepTmp = process.env.CONTRACT_CHECK_KEEP_TMP === '1';
  const stateDir = path.join(tmp, 'state');
  const sockPath = path.join(tmp, 'daemon.sock');
  const fakeHerdr = path.join(tmp, 'fake-herdr');
  const fakeOpencode = path.join(tmp, 'fake-opencode');
  fs.writeFileSync(fakeHerdr, FAKE_HERDR, { mode: 0o755 });
  fs.writeFileSync(fakeOpencode, FAKE_OPENCODE, { mode: 0o755 });

  let child = null;
  let childOut = '';
  const killGroup = (signal) => {
    if (!child || child.pid == null) return;
    try { process.kill(-child.pid, signal); } // the whole group: cargo + notchd
    catch { try { child.kill(signal); } catch { /* already gone */ } }
  };
  const cleanup = async () => {
    if (child && child.exitCode === null && !child.killed) {
      killGroup('SIGTERM');
      const exited = await Promise.race([
        new Promise((r) => child.once('exit', () => r(true))),
        sleep(3000).then(() => false),
      ]);
      if (!exited) killGroup('SIGKILL');
    }
    if (!keepTmp) fs.rmSync(tmp, { recursive: true, force: true });
    else console.error(`contract-check: temp dir kept at ${tmp}`);
  };
  const bail2 = async (msg) => {
    console.error(`contract-check: FAIL — ${msg} (exit 2)`);
    if (childOut.trim()) console.error(`--- daemon output tail ---\n${childOut.slice(-2000)}`);
    await cleanup();
    process.exit(2);
  };

  process.on('SIGINT', () => cleanup().then(() => process.exit(130)));
  process.on('SIGTERM', () => cleanup().then(() => process.exit(143)));

  // 4. Spawn the daemon as its own process group leader so cleanup can
  // SIGTERM/SIGKILL the whole group — `cargo run` wraps notchd, and killing
  // only the cargo wrapper orphans the daemon.
  child = spawn('cargo', ['run', '-q', '-p', 'notchd', '--', 'serve'], {
    cwd: REPO_ROOT,
    detached: true,
    env: {
      ...process.env,
      NOTCH_STATE_DIR: stateDir,
      NOTCH_SOCK: sockPath,
      NOTCH_HERDR_CMD: fakeHerdr,
      NOTCH_OPENCODE_CMD: fakeOpencode,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout.on('data', (d) => { childOut += d; });
  child.stderr.on('data', (d) => { childOut += d; });
  child.on('error', (e) => { void bail2(`cannot spawn cargo: ${e.message} — is Rust on PATH?`); });

  const sockDeadline = Date.now() + SOCK_WAIT_MS;
  while (!fs.existsSync(sockPath)) {
    if (child.exitCode !== null) return bail2(`daemon exited before binding the socket (code ${child.exitCode})`);
    if (Date.now() > sockDeadline) return bail2(`daemon did not create ${sockPath} within ${SOCK_WAIT_MS}ms`);
    await sleep(100);
  }

  // 5. NDJSON session.
  let socket = null;
  for (let attempt = 0; attempt < CONNECT_RETRIES && !socket; attempt++) {
    socket = await new Promise((resolve) => {
      const s = net.createConnection(sockPath);
      s.once('connect', () => resolve(s));
      s.once('error', () => { s.destroy(); resolve(null); });
    });
    if (!socket) await sleep(100);
  }
  if (!socket) return bail2('could not connect to the daemon socket');

  const nextFrame = makeFrameReader(socket);
  const send = (obj) => sendCmd(socket, obj);

  // hello frame.
  try {
    const hello = await nextFrame('hello', FRAME_TIMEOUT_MS);
    if (hello.json?.event !== 'hello') violations.push(`hello: expected event "hello", got ${hello.raw.slice(0, 200)}`);
    if (hello.json?.version !== 1) violations.push(`hello: version must be 1, got ${JSON.stringify(hello.json?.version)}`);
    if (!Number.isInteger(hello.json?.pid)) violations.push(`hello: pid must be an integer, got ${JSON.stringify(hello.json?.pid)}`);
  } catch (e) {
    violations.push(`hello: ${e.message}`);
  }

  // 6a. Command manifest probes (subscribe last — it emits two frames).
  for (const entry of COMMAND_MANIFEST) {
    if (entry.skip) continue;
    try {
      send(entry.request);
      const frame = await nextFrame(entry.cmd, FRAME_TIMEOUT_MS);
      checkExpect(entry, frame, violations);
    } catch (e) {
      violations.push(`${entry.cmd}: ${e.message}`);
    }
  }

  // 6b. subscribe — one ack, then one captured snapshot frame.
  try {
    send({ cmd: 'subscribe' });
    const ack = await nextFrame('subscribe ack', FRAME_TIMEOUT_MS);
    checkExpect({ cmd: 'subscribe', expect: { event: 'ack', ok: true } }, ack, violations);
    const snap = await nextFrame('subscribe snapshot frame', FRAME_TIMEOUT_MS);
    violations.push(...validateSnapshotFrame(snap).map((m) => `subscribe snapshot frame: ${m}`));
    notes.push('subscribe ack + one snapshot frame captured');
  } catch (e) {
    violations.push(`subscribe: ${e.message}`);
  }

  socket.destroy();
  await cleanup();

  // 7. Verdict.
  if (violations.length) {
    console.error(`contract-check: FAIL — ${violations.length} violation(s) vs docs/bridge.md (exit 1):`);
    violations.forEach((v, i) => console.error(`  ${String(i + 1).padStart(2)}. ${v}`));
    console.error('(notes: ' + notes.join('; ') + ')');
    process.exit(1);
  }
  console.log(`contract-check: PASS — docs/bridge.md §1.3 + §2 hold over the live socket.`);
  console.log(`  - ${notes.join('\n  - ')}`);
  console.log(`  - manifest: ${COMMAND_MANIFEST.length}/${COMMAND_MANIFEST.length} commands answered with the documented envelope`);
  console.log(`  - snapshot frame: key sets, camelCase, status enum, f64 eps, integer ms, version == 1 — clean`);
  process.exit(0);
};

const isMain = (() => {
  try {
    return process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
  } catch {
    return false;
  }
})();

if (isMain) {
  main().catch(async (e) => {
    console.error(`contract-check: unexpected failure: ${e?.stack ?? e} (exit 2)`);
    process.exit(2);
  });
}

// Exported for checker testing only; the script has no other consumers.
export { COMMAND_MANIFEST, bridgeCommandDiff, validateSnapshotFrame, runSelfTest };
