// demos.ts — the browser/preview provider. Emits bridge.md §1.3 snapshots on a 240 ms cadence
// with a seeded random walk so screenshots show living UI without lying about mechanics.
import { mulberry32 } from './motion';
import type { Snapshot, Session, Supervisor, LogsPayload, UiAction } from './bridge';
import { registerDemoAction } from './bridge';
import type { Status } from './tokens';

type DemoName = 'mixed' | 'empty' | 'failed' | 'alldone' | 'idle' | 'big';

interface SeedSession { id: string; sup: string; status: Status; title: string; startedMinAgo: number; latencyMs: number | null; failureReason?: string }

const T = () => Date.now();

const SEEDS: Record<DemoName, { orchestrator: Status; supervisors: { id: string; status: Status }[]; sessions: SeedSession[]; eps: number }> = {
  mixed: {
    orchestrator: 'working',
    supervisors: [{ id: 'SV-1', status: 'working' }, { id: 'SV-2', status: 'working' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'working', title: 'Refactor auth middleware', startedMinAgo: 2.2, latencyMs: 812 },
      { id: 'S1-OC02', sup: 'SV-1', status: 'working', title: 'Port vector store to sqlite-vec', startedMinAgo: 6.7, latencyMs: 834 },
      { id: 'S1-OC03', sup: 'SV-1', status: 'idle', title: 'Write property tests for the bus', startedMinAgo: 11.4, latencyMs: 795 },
      { id: 'S2-OC01', sup: 'SV-2', status: 'failed', title: 'Fix race in herdr pane reconcile', startedMinAgo: 14.9, latencyMs: 990, failureReason: 'stuck' },
      { id: 'S2-OC02', sup: 'SV-2', status: 'done', title: 'Trim watchdog timeout table', startedMinAgo: 18.3, latencyMs: 806 },
      { id: 'S2-OC03', sup: 'SV-2', status: 'working', title: 'Route attribution writes through ctx', startedMinAgo: 4.1, latencyMs: 846 },
    ],
    eps: 6.5,
  },
  empty: {
    orchestrator: 'idle',
    supervisors: [],
    sessions: [],
    eps: 0,
  },
  failed: {
    orchestrator: 'working',
    supervisors: [{ id: 'SV-1', status: 'failed' }, { id: 'SV-2', status: 'working' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'working', title: 'Refactor auth middleware', startedMinAgo: 3.6, latencyMs: 824 },
      { id: 'S1-OC02', sup: 'SV-1', status: 'failed', title: 'Fix retry storm on 409 conflicts', startedMinAgo: 9.2, latencyMs: 1044, failureReason: 'stuck' },
      { id: 'S2-OC01', sup: 'SV-2', status: 'working', title: 'Split sync writer from bus reader', startedMinAgo: 5.8, latencyMs: 831 },
      { id: 'S2-OC02', sup: 'SV-2', status: 'idle', title: 'Pin herdr pane ids to registry', startedMinAgo: 12.1, latencyMs: null },
      { id: 'S2-OC03', sup: 'SV-2', status: 'working', title: 'Emit heartbeat revision on every edit', startedMinAgo: 7.4, latencyMs: 857 },
    ],
    eps: 3.1,
  },
  alldone: {
    orchestrator: 'done',
    supervisors: [{ id: 'SV-1', status: 'done' }, { id: 'SV-2', status: 'done' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'done', title: 'Refactor auth middleware', startedMinAgo: 22.0, latencyMs: 818 },
      { id: 'S1-OC02', sup: 'SV-1', status: 'done', title: 'Port vector store to sqlite-vec', startedMinAgo: 26.4, latencyMs: 845 },
      { id: 'S2-OC01', sup: 'SV-2', status: 'done', title: 'Fix race in herdr pane reconcile', startedMinAgo: 31.7, latencyMs: 872 },
      { id: 'S2-OC02', sup: 'SV-2', status: 'done', title: 'Trim watchdog timeout table', startedMinAgo: 28.9, latencyMs: 800 },
      { id: 'S2-OC03', sup: 'SV-2', status: 'done', title: 'Route attribution writes through ctx', startedMinAgo: 19.3, latencyMs: 837 },
    ],
    eps: 0.4,
  },
  idle: {
    orchestrator: 'idle',
    supervisors: [{ id: 'SV-1', status: 'idle' }, { id: 'SV-2', status: 'idle' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'idle', title: 'Write property tests for the bus', startedMinAgo: 34.0, latencyMs: 790 },
      { id: 'S1-OC02', sup: 'SV-1', status: 'idle', title: 'Document pane reconcile invariant', startedMinAgo: 40.5, latencyMs: 803 },
      { id: 'S2-OC01', sup: 'SV-2', status: 'idle', title: 'Survey sqlite-vec journal modes', startedMinAgo: 27.6, latencyMs: null },
      { id: 'S2-OC02', sup: 'SV-2', status: 'idle', title: 'Audit attribution cap accounting', startedMinAgo: 45.2, latencyMs: 815 },
    ],
    eps: 0.3,
  },
  big: {
    orchestrator: 'working',
    supervisors: [{ id: 'SV-1', status: 'working' }, { id: 'SV-2', status: 'working' }, { id: 'SV-3', status: 'working' }],
    sessions: [
      { id: 'S1-OC01', sup: 'SV-1', status: 'working', title: 'Refactor auth middleware', startedMinAgo: 2.2, latencyMs: 813 },
      { id: 'S1-OC02', sup: 'SV-1', status: 'working', title: 'Port vector store to sqlite-vec', startedMinAgo: 6.7, latencyMs: 836 },
      { id: 'S1-OC03', sup: 'SV-1', status: 'working', title: 'Write property tests for the bus', startedMinAgo: 11.4, latencyMs: 798 },
      { id: 'S1-OC04', sup: 'SV-1', status: 'idle', title: 'Pin herdr pane ids to registry', startedMinAgo: 15.8, latencyMs: 824 },
      { id: 'S2-OC01', sup: 'SV-2', status: 'working', title: 'Fix retry storm on 409 conflicts', startedMinAgo: 9.2, latencyMs: 841 },
      { id: 'S2-OC02', sup: 'SV-2', status: 'working', title: 'Split sync writer from bus reader', startedMinAgo: 5.8, latencyMs: 832 },
      { id: 'S2-OC03', sup: 'SV-2', status: 'done', title: 'Trim watchdog timeout table', startedMinAgo: 18.3, latencyMs: 801 },
      { id: 'S2-OC04', sup: 'SV-2', status: 'working', title: 'Emit heartbeat revision on every edit', startedMinAgo: 7.4, latencyMs: 859 },
      { id: 'S3-OC01', sup: 'SV-3', status: 'working', title: 'Route attribution writes through ctx', startedMinAgo: 4.1, latencyMs: 847 },
      { id: 'S3-OC02', sup: 'SV-3', status: 'working', title: 'Reconcile orphan panes on boot', startedMinAgo: 13.0, latencyMs: 866 },
      { id: 'S3-OC03', sup: 'SV-3', status: 'idle', title: 'Index events.jsonl for tail -f', startedMinAgo: 23.5, latencyMs: null },
      { id: 'S3-OC04', sup: 'SV-3', status: 'working', title: 'Flush cleanup hooks on session done', startedMinAgo: 8.8, latencyMs: 830 },
    ],
    eps: 14.2,
  },
};

function toSnapshot(name: DemoName, eps: number, jitterSeed: number): Snapshot {
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
  }));
  const counts = new Map<string, { working: number; idle: number; failed: number; done: number }>();
  for (const s of sessions) {
    const c = counts.get(s.sup) ?? { working: 0, idle: 0, failed: 0, done: 0 };
    c[s.status] += 1;
    counts.set(s.sup, c);
  }
  const supervisors: Supervisor[] = seed.supervisors.map((sv) => {
    const c = counts.get(sv.id) ?? { working: 0, idle: 0, failed: 0, done: 0 };
    return { id: sv.id, status: sv.status, ...c };
  });
  return {
    version: 1,
    generatedAt: now,
    orchestrator: { id: 'ORCH', status: seed.orchestrator },
    supervisors,
    sessions,
    eps,
    conflicts: [],
    attribution: [],
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
    `[${id}] 09:12:29 editing src/auth.rs (rev 44)`,
    `[${id}] 09:13:02 watchdog: revision advancing (44 → 47)`,
    `[${id}] 09:13:40 editing src/auth.rs (rev 51)`,
    `[${id}] 09:14:03 attribution: src/auth.rs`,
    `[${id}] 09:14:12 last tool: edit · 0 errors`,
    `[${id}] 09:14:12 waiting for next instruction`,
  ];
  DEMO_LOG_LINES[id] = lines;
  return lines;
}

export async function installDemo({ onSnapshot, onLogs }: { onSnapshot: (s: Snapshot) => void; onLogs: (p: LogsPayload) => void }): Promise<() => void> {
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
  registerDemoAction((a: UiAction) => {
    if (a.type === 'logs') onLogs({ id: a.id, lines: demoLog(a.id) });
    if (a.type === 'snapshot') onSnapshot(toSnapshot(name, eps, tick));
  });
  return () => window.clearInterval(id);
}
