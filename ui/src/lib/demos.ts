// demos.ts — the browser/preview provider. Emits bridge.md v1.3 §1.3 snapshots on a 240 ms cadence
// with a seeded random walk so screenshots show living UI without lying about mechanics.
import { mulberry32 } from './motion';
import { registerDemoInvoke } from './bridge';
import type { Snapshot, Session, Supervisor, Decision, Escalation, BusMessage, UiAction } from './bridge';
import type { Status } from './tokens';

type DemoName = 'mixed' | 'empty' | 'failed' | 'alldone' | 'idle' | 'big';

interface SeedSession {
  id: string; sup: string; status: Status; title: string; startedMinAgo: number;
  latencyMs: number | null; failureReason?: string;
  goal?: string; definitionOfDone?: string; iterations?: number; maxIterations?: number;
  lastSupervisorAction?: string; model?: string; wallClockMinFromNow?: number;
}

const T = () => Date.now();

const SEEDS: Record<DemoName, { orchestrator: Status; supervisors: { id: string; status: Status; paused?: boolean }[]; sessions: SeedSession[]; eps: number; decisions: Omit<Decision, 'at'>[]; escalations: Omit<Escalation, 'at'>[]; busMessages: Omit<BusMessage, 'at'>[] }> = {
  mixed: {
    orchestrator: 'working',
    supervisors: [{ id: 'SV-1', status: 'working' }, { id: 'SV-2', status: 'working' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'working', title: 'Refactor auth middleware', startedMinAgo: 2.2, latencyMs: 812, goal: 'Refactor auth middleware', definitionOfDone: 'tests pass + p95 < 200ms', iterations: 2, maxIterations: 8, lastSupervisorAction: 're-prompt: tests not run', model: 'opencode/hy4-flash-free', wallClockMinFromNow: 47 },
      { id: 'S1-OC02', sup: 'SV-1', status: 'working', title: 'Port vector store to sqlite-vec', startedMinAgo: 6.7, latencyMs: 834, goal: 'Port vector store to sqlite-vec', definitionOfDone: 'vector search p95 < 120ms across 10k rows', iterations: 4, maxIterations: 8, lastSupervisorAction: 'verify: waiting on apply_patch', model: 'opencode/hy4-flash-free' },
      { id: 'S1-OC03', sup: 'SV-1', status: 'idle', title: 'Write property tests for the bus', startedMinAgo: 11.4, latencyMs: 795, goal: 'Write property tests for the bus', definitionOfDone: 'property suite green on 1k seeds', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC01', sup: 'SV-2', status: 'failed', title: 'Fix race in herdr pane reconcile', startedMinAgo: 14.9, latencyMs: 990, failureReason: 'stuck', goal: 'Fix race in herdr pane reconcile', definitionOfDone: 'no 409s in reconcile under load test', iterations: 7, maxIterations: 8, lastSupervisorAction: 're-prompt: tests not run', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC02', sup: 'SV-2', status: 'done', title: 'Trim watchdog timeout table', startedMinAgo: 18.3, latencyMs: 806, goal: 'Trim watchdog timeout table', definitionOfDone: 'timeouts table matches config schema', iterations: 1, maxIterations: 8, lastSupervisorAction: 'done: evidence verified', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC03', sup: 'SV-2', status: 'working', title: 'Route attribution writes through ctx', startedMinAgo: 4.1, latencyMs: 846, goal: 'Route attribution writes through ctx', definitionOfDone: 'attribution writes serialized and idempotent', iterations: 3, maxIterations: 8, model: 'opencode/hy4-flash-free' },
    ],
    eps: 6.5,
    decisions: [
      { supervisor: 'SV-2', session: 'S2-OC01', action: 'reprompt', reason: 'tests not run' },
      { supervisor: 'SV-1', session: 'S1-OC02', action: 'verify', reason: 'waiting on apply_patch result' },
      { supervisor: 'SV-1', session: 'S1-OC01', action: 'assign', reason: 'goal accepted: auth middleware refactor' },
    ],
    escalations: [
      { id: 'esc_demo_1', session: 'S2-OC01', supervisor: 'SV-2', kind: 'stuck', reason: 'No revision change in 180s while editing src/auth.rs — same file read three times' },
    ],
    busMessages: [
      { id: 'msg_demo_2', from: 'S1-OC02', to: 'S2-OC01', type: 'handoff', preview: 'Schema for pane reconcile lives in sqlite-vec; migrations are yours — do not reorder the journal' },
      { id: 'msg_demo_1', from: 'S2-OC01', to: 'SV-2', type: 'question', preview: 'Should the reconcile path retry on 409 Conflict or fail the step and surface it?' },
      { id: 'msg_demo_3', from: 'SV-2', to: 'S2-OC02', type: 'result', preview: 'Trim watchdog table landed: 14 timeouts now bounded; evidence pasted below' },
    ],
  },
  empty: {
    orchestrator: 'idle',
    supervisors: [],
    sessions: [],
    eps: 0,
    decisions: [],
    escalations: [],
    busMessages: [],
  },
  failed: {
    orchestrator: 'working',
    supervisors: [{ id: 'SV-1', status: 'failed' }, { id: 'SV-2', status: 'working' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'working', title: 'Refactor auth middleware', startedMinAgo: 3.6, latencyMs: 824, goal: 'Refactor auth middleware', definitionOfDone: 'tests pass + p95 < 200ms', iterations: 1, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S1-OC02', sup: 'SV-1', status: 'failed', title: 'Fix retry storm on 409 conflicts', startedMinAgo: 9.2, latencyMs: 1044, failureReason: 'stuck', goal: 'Fix retry storm on 409 conflicts', definitionOfDone: 'zero 409s in a 10k-request soak', iterations: 6, maxIterations: 8, lastSupervisorAction: 're-prompt: same error repeated', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC01', sup: 'SV-2', status: 'working', title: 'Split sync writer from bus reader', startedMinAgo: 5.8, latencyMs: 831, goal: 'Split sync writer from bus reader', definitionOfDone: 'writes never blocked by reads under load', iterations: 2, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC02', sup: 'SV-2', status: 'idle', title: 'Pin herdr pane ids to registry', startedMinAgo: 12.1, latencyMs: null, goal: 'Pin herdr pane ids to registry', definitionOfDone: 'pane ids stable across daemon restarts', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC03', sup: 'SV-2', status: 'working', title: 'Emit heartbeat revision on every edit', startedMinAgo: 7.4, latencyMs: 857, goal: 'Emit heartbeat revision on every edit', definitionOfDone: 'revision increments on every pane write', iterations: 5, maxIterations: 8, model: 'opencode/hy4-flash-free' },
    ],
    eps: 3.1,
    decisions: [
      { supervisor: 'SV-1', session: 'S1-OC02', action: 'escalate', reason: 'stuck: same error three times — escalated' },
      { supervisor: 'SV-2', session: 'S2-OC01', action: 'verify', reason: 'waiting on bench output' },
    ],
    escalations: [
      { id: 'esc_demo_2', session: 'S1-OC02', supervisor: 'SV-1', kind: 'loop_detected', reason: 'Same 409 conflict retry 5 times — identical output, no progress' },
    ],
    busMessages: [
      { id: 'msg_demo_4', from: 'SV-1', to: 'S1-OC02', type: 'task', preview: 'Retry against a fresh checkout of the conflict path, not the same pane' },
    ],
  },
  alldone: {
    orchestrator: 'done',
    supervisors: [{ id: 'SV-1', status: 'done' }, { id: 'SV-2', status: 'done' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'done', title: 'Refactor auth middleware', startedMinAgo: 22.0, latencyMs: 818, goal: 'Refactor auth middleware', definitionOfDone: 'tests pass + p95 < 200ms', iterations: 1, maxIterations: 8, lastSupervisorAction: 'done: evidence verified', model: 'opencode/hy4-flash-free' },
      { id: 'S1-OC02', sup: 'SV-1', status: 'done', title: 'Port vector store to sqlite-vec', startedMinAgo: 26.4, latencyMs: 845, goal: 'Port vector store to sqlite-vec', definitionOfDone: 'vector search p95 < 120ms', iterations: 3, maxIterations: 8, lastSupervisorAction: 'done: evidence verified', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC01', sup: 'SV-2', status: 'done', title: 'Fix race in herdr pane reconcile', startedMinAgo: 31.7, latencyMs: 872, goal: 'Fix race in herdr pane reconcile', definitionOfDone: 'no 409s in reconcile under load', iterations: 2, maxIterations: 8, lastSupervisorAction: 'done: evidence verified', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC02', sup: 'SV-2', status: 'done', title: 'Trim watchdog timeout table', startedMinAgo: 28.9, latencyMs: 800, goal: 'Trim watchdog timeout table', definitionOfDone: 'timeouts table matches schema', iterations: 0, maxIterations: 8, lastSupervisorAction: 'done: evidence verified', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC03', sup: 'SV-2', status: 'done', title: 'Route attribution writes through ctx', startedMinAgo: 19.3, latencyMs: 837, goal: 'Route attribution writes through ctx', definitionOfDone: 'attribution idempotent', iterations: 1, maxIterations: 8, lastSupervisorAction: 'done: evidence verified', model: 'opencode/hy4-flash-free' },
    ],
    eps: 0.4,
    decisions: [
      { supervisor: 'SV-2', session: 'S2-OC02', action: 'done', reason: 'evidence verified: timeouts bounded' },
    ],
    escalations: [],
    busMessages: [
      { id: 'msg_demo_5', from: 'SV-2', to: 'S2-OC03', type: 'result', preview: 'Attribution writes verified serialized — 0 conflicts across 400 writes' },
    ],
  },
  idle: {
    orchestrator: 'idle',
    supervisors: [{ id: 'SV-1', status: 'idle', paused: true }, { id: 'SV-2', status: 'idle' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'idle', title: 'Write property tests for the bus', startedMinAgo: 34.0, latencyMs: 790, goal: 'Write property tests for the bus', definitionOfDone: 'property suite green on 1k seeds', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S1-OC02', sup: 'SV-1', status: 'idle', title: 'Document pane reconcile invariant', startedMinAgo: 40.5, latencyMs: 803, goal: 'Document pane reconcile invariant', definitionOfDone: 'invariant written + linked from README', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC01', sup: 'SV-2', status: 'idle', title: 'Survey sqlite-vec journal modes', startedMinAgo: 27.6, latencyMs: null, goal: 'Survey sqlite-vec journal modes', definitionOfDone: 'recommendation with numbers attached', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC02', sup: 'SV-2', status: 'idle', title: 'Audit attribution cap accounting', startedMinAgo: 45.2, latencyMs: 815, goal: 'Audit attribution cap accounting', definitionOfDone: 'cap holds at 24 with newest-first order', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
    ],
    eps: 0.3,
    decisions: [],
    escalations: [],
    busMessages: [],
  },
  big: {
    orchestrator: 'working',
    supervisors: [{ id: 'SV-1', status: 'working' }, { id: 'SV-2', status: 'working' }, { id: 'SV-3', status: 'working' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'working', title: 'Refactor auth middleware', startedMinAgo: 2.2, latencyMs: 813, goal: 'Refactor auth middleware', definitionOfDone: 'tests pass + p95 < 200ms', iterations: 2, maxIterations: 8, lastSupervisorAction: 're-prompt: tests not run', model: 'opencode/hy4-flash-free' },
      { id: 'S1-OC02', sup: 'SV-1', status: 'working', title: 'Port vector store to sqlite-vec', startedMinAgo: 6.7, latencyMs: 836, goal: 'Port vector store to sqlite-vec', definitionOfDone: 'vector search p95 < 120ms', iterations: 3, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S1-OC03', sup: 'SV-1', status: 'working', title: 'Write property tests for the bus', startedMinAgo: 11.4, latencyMs: 798, goal: 'Write property tests for the bus', definitionOfDone: 'property suite green on 1k seeds', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S1-OC04', sup: 'SV-1', status: 'idle', title: 'Pin herdr pane ids to registry', startedMinAgo: 15.8, latencyMs: 824, goal: 'Pin herdr pane ids to registry', definitionOfDone: 'pane ids stable', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC01', sup: 'SV-2', status: 'working', title: 'Fix retry storm on 409 conflicts', startedMinAgo: 9.2, latencyMs: 841, goal: 'Fix retry storm on 409 conflicts', definitionOfDone: 'zero 409s in a 10k-request soak', iterations: 4, maxIterations: 8, lastSupervisorAction: 'verify: waiting on soak output', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC02', sup: 'SV-2', status: 'working', title: 'Split sync writer from bus reader', startedMinAgo: 5.8, latencyMs: 832, goal: 'Split sync writer from bus reader', definitionOfDone: 'writes never blocked by reads', iterations: 1, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC03', sup: 'SV-2', status: 'done', title: 'Trim watchdog timeout table', startedMinAgo: 18.3, latencyMs: 801, goal: 'Trim watchdog timeout table', definitionOfDone: 'timeouts bounded', iterations: 2, maxIterations: 8, lastSupervisorAction: 'done: evidence verified', model: 'opencode/hy4-flash-free' },
      { id: 'S2-OC04', sup: 'SV-2', status: 'working', title: 'Emit heartbeat revision on every edit', startedMinAgo: 7.4, latencyMs: 859, goal: 'Emit heartbeat revision on every edit', definitionOfDone: 'revision increments on every write', iterations: 5, maxIterations: 8, model: 'opencode/kimi-k2' },
      { id: 'S3-OC01', sup: 'SV-3', status: 'working', title: 'Route attribution writes through ctx', startedMinAgo: 4.1, latencyMs: 847, goal: 'Route attribution writes through ctx', definitionOfDone: 'attribution idempotent', iterations: 3, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S3-OC02', sup: 'SV-3', status: 'working', title: 'Reconcile orphan panes on boot', startedMinAgo: 13.0, latencyMs: 866, goal: 'Reconcile orphan panes on boot', definitionOfDone: 'boot reconcile adopts or marks done', iterations: 2, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S3-OC03', sup: 'SV-3', status: 'idle', title: 'Index events.jsonl for tail -f', startedMinAgo: 23.5, latencyMs: null, goal: 'Index events.jsonl for tail -f', definitionOfDone: 'tail -f events.jsonl streams without full scans', iterations: 0, maxIterations: 8, model: 'opencode/hy4-flash-free' },
      { id: 'S3-OC04', sup: 'SV-3', status: 'working', title: 'Flush cleanup hooks on session done', startedMinAgo: 8.8, latencyMs: 830, goal: 'Flush cleanup hooks on session done', definitionOfDone: 'cleanup hooks run exactly once per done', iterations: 1, maxIterations: 8, model: 'opencode/hy4-flash-free' },
    ],
    eps: 14.2,
    decisions: [
      { supervisor: 'SV-2', session: 'S2-OC02', action: 'verify', reason: 'waiting on bench output' },
      { supervisor: 'SV-1', session: 'S1-OC01', action: 'reprompt', reason: 'tests not run' },
      { supervisor: 'SV-3', session: 'S3-OC02', action: 'assign', reason: 'goal accepted: orphan reconcile' },
    ],
    escalations: [
      { id: 'esc_demo_3', session: 'S2-OC04', supervisor: 'SV-2', kind: 'budget_exceeded', reason: '5 of 8 iterations used with the same bench failure — approaching budget' },
    ],
    busMessages: [
      { id: 'msg_demo_6', from: 'S3-OC02', to: 'S1-OC01', type: 'handoff', preview: 'Orphan pane rule: anything old enough gets closed, not adopted — write it in the registry first' },
      { id: 'msg_demo_7', from: 'S2-OC02', to: 'SV-2', type: 'question', preview: 'Writer split ok to land without the reader flag guarded?' },
      { id: 'msg_demo_8', from: 'SV-3', to: 'S3-OC01', type: 'task', preview: 'Attribution cap first — the auth middleware path is generating 3 writes per step' },
    ],
  },
};

function withAges<T extends object>(items: T[], base: number): (T & { at: number })[] {
  const offsets = [75, 180, 320, 560]; // seconds ago, newest-first
  return items.map((it, i) => ({ ...it, at: base - (offsets[i] ?? 700) * 1000 }));
}

export function toSnapshot(name: DemoName, eps: number, jitterSeed: number): Snapshot {
  const seed = SEEDS[name];
  const jitter = mulberry32(jitterSeed);
  const now = T();
  const sessions: Session[] = seed.sessions.map((s) => ({
    id: s.id,
    sup: s.sup,
    status: s.status,
    title: s.title,
    task: s.title,
    cwd: '/Users/harshitduggal/workspace/notch',
    paneId: `w80:p${1 + (parseInt(s.id.split('-OC')[1] ?? '1', 10) % 4)}`,
    startedAt: now - Math.round(s.startedMinAgo * 60 * 1000),
    lastAction: s.status === 'working' ? 'editing src/' : s.status === 'done' ? 'revision 12' : 'waiting for input',
    lastRevision: Math.round(30 + jitter() * 80),
    latencyMs: s.latencyMs === null ? null : Math.max(120, Math.round(s.latencyMs + (jitter() - 0.5) * 14)),
    failureReason: s.status === 'failed' ? (s.failureReason ?? 'stuck') : null,
    goal: s.goal,
    definitionOfDone: s.definitionOfDone,
    iterations: s.iterations,
    maxIterations: s.maxIterations,
    wallClockDeadline: s.wallClockMinFromNow != null ? now + s.wallClockMinFromNow * 60 * 1000 : undefined,
    lastSupervisorAction: s.lastSupervisorAction,
    model: s.model,
  }));
  const counts = new Map<string, { working: number; idle: number; failed: number; done: number }>();
  for (const s of sessions) {
    const c = counts.get(s.sup) ?? { working: 0, idle: 0, failed: 0, done: 0 };
    c[s.status] += 1;
    counts.set(s.sup, c);
  }
  const supervisors: Supervisor[] = seed.supervisors.map((sv) => {
    const c = counts.get(sv.id) ?? { working: 0, idle: 0, failed: 0, done: 0 };
    return { id: sv.id, status: sv.status, paused: sv.paused ?? pausedSupervisors.has(sv.id), ...c };
  });
  const decisions: Decision[] = withAges(seed.decisions, now).sort((a, b) => b.at - a.at);
  const escalations: Escalation[] = withAges(seed.escalations, now).sort((a, b) => b.at - a.at);
  const busMessages: BusMessage[] = withAges(seed.busMessages, now).sort((a, b) => b.at - a.at);
  return {
    version: 1,
    generatedAt: now,
    orchestrator: { id: 'ORCH', status: seed.orchestrator },
    supervisors,
    sessions,
    eps,
    conflicts: [],
    attribution: [
      { session: 'S1-OC01', file: 'src/auth.rs' },
      { session: 'S1-OC02', file: 'src/auth.rs' },
      { session: 'S1-OC01', file: 'crates/notchd/src/daemon.rs' },
      { session: 'S2-OC02', file: 'crates/notchd/src/registry.rs' },
    ],
    decisions,
    escalations,
    busMessages,
    warnings: [],
  };
}

const DEMO_LOG_LINES: Record<string, string[]> = {};

function demoLog(id: string): string[] {
  const cached = DEMO_LOG_LINES[id];
  if (cached) return cached;
  const lines = [
    `[${id}] 09:12:04 herdr agent start --kind opencode`,
    `[${id}] 09:12:04 spawned pane ${id.toLowerCase()} · rev 31`,
    `[${id}] 09:12:11 plan → locate claim path`,
    `[${id}] 09:12:29 apply_patch src/auth.rs (rev 44)`,
    `[${id}] 09:13:02 watchdog: revision advancing (44 → 47)`,
    `[${id}] 09:13:40 bash "cargo test -p notchd" · 2 passed`,
    `[${id}] 09:14:03 attribution: src/auth.rs`,
    `[${id}] 09:14:12 read src/auth.rs:140-190`,
    `[${id}] 09:14:18 last tool: apply_patch · 0 errors`,
    `[${id}] 09:14:18 waiting for next instruction`,
  ];
  DEMO_LOG_LINES[id] = lines;
  return lines;
}

const pausedSupervisors = new Set<string>();

export async function installDemo({ onSnapshot }: { onSnapshot: (s: Snapshot) => void }): Promise<() => void> {
  const params = new URLSearchParams(window.location.search);
  const raw = (params.get('demo') ?? 'mixed') as DemoName;
  const name: DemoName = raw in SEEDS ? raw : 'mixed';
  let eps = SEEDS[name].eps;
  let tick = 0;
  onSnapshot(toSnapshot(name, eps, tick));
  const id = window.setInterval(() => {
    tick += 1;
    const walk = mulberry32(0x5EED + tick)();
    eps = Math.max(0, Math.min(40, eps + (walk - 0.5) * 0.7));
    onSnapshot(toSnapshot(name, eps, tick));
  }, 240);
  registerDemoInvoke((a: UiAction): unknown => {
    switch (a.cmd) {
      case 'logs': return { id: a.id, lines: demoLog(a.id) };
      case 'snapshot': return toSnapshot(name, eps, tick);
      case 'submit_task':
        return { ok: true, supervisorId: a.supervisorId ?? `SV-${(tick % 3) + 1}`, sessionId: `S${(tick % 3) + 1}-OC${(tick % 8) + 10}` };
      case 'open_dashboard': return { ok: true };
      case 'message': return { ok: true, messageId: `msg_demo_${tick}` };
      case 'pause_supervisor': pausedSupervisors.add(a.id); return { ok: true };
      case 'resume_supervisor': pausedSupervisors.delete(a.id); return { ok: true };
      case 'kill_switch':
      case 'cleanup':
      case 'kill':
      case 'reprompt':
        return { ok: true };
      case 'list_models':
        return { models: ['opencode/hy4-flash-free', 'opencode/kimi-k2', 'auto'] };
      case 'spawn':
        return { ok: true, sessionId: `S${(tick % 3) + 1}-OC${(tick % 8) + 10}`, supervisorId: a.supervisor };
      case 'run_plan':
        return { ok: true, path: a.path };
      case 'symbol':
        return { ok: true, name: a.name };
      case 'set_frame':
        return { ok: true, mode: a.mode, rect: { w: 560, h: 640 } };
      case 'haptic':
        return { ok: true };
      default:
        return { ok: true };
    }
  });
  return () => window.clearInterval(id);
}
