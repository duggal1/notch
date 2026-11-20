// fleet-wiring.ts — the W2-5 bridge to the wave-2 capability surface.
// One command vocabulary, one event path, one projection used by both the
// app (Tauri invoke) and the browser/demo build. No snapshot key is renamed;
// `fleet` is additive and the frozen `snapshot` path is untouched.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { isTauri, type Snapshot } from './bridge';
import { FLEET_EVENT } from './ws-transport';

export type FleetCommand =
  | { cmd: 'fleet_state' }
  | { cmd: 'list_harnesses' }
  | { cmd: 'list_approvals' }
  | { cmd: 'answer_approval'; id: string; allow: boolean; note?: string }
  | { cmd: 'fleet_control'; action: 'hold' | 'release' | 'kill_all'; id?: string };

/** Fire-and-forget with the same headless recorder used by bridge.ts `send`. */
export function sendFleet(a: FleetCommand): void {
  const log = (window as unknown as { __notchActions?: unknown[] }).__notchActions;
  if (Array.isArray(log)) log.push({ ...a, at: Date.now() });
  if (!isTauri()) return;
  const { cmd, ...args } = a;
  invoke(cmd, args).catch(() => undefined);
}

export interface FleetUnit {
  id: string; kind: string; sup: string; status: string; word: string;
  detail: string; title: string; paneId: string; workspace: string;
  lastAction: string; revision: number; escalation: string;
}
export interface FleetState {
  source: string;
  generatedAt: number;
  topStatus: string;
  units: FleetUnit[];
  groups: { working: number; idle: number; stopped: number; done: number };
  storm: { holds: number; guard: string; starts: number };
  ledger: { seq: number; fence: string; epoch: number; decisions: number; messages: number };
}
export interface HarnessCapability { name: string; supported: boolean }
export interface Harness { id: string; label: string; capabilities: HarnessCapability[] }
export interface Approval {
  id: string; session: string; supervisor: string; kind: string;
  reason: string; at: number; digest: string; signed: boolean;
}
export interface FleetData { state: FleetState; approvals: Approval[]; harnesses: Harness[] }

const s = (v: Record<string, unknown> | undefined, k: string): string =>
  typeof v?.[k] === 'string' ? (v[k] as string) : '';
const n = (v: Record<string, unknown> | undefined, k: string): number =>
  typeof v?.[k] === 'number' ? (v[k] as number) : 0;

/** FNV-1a — mirrors commands.rs `card_digest`; an honest identifier, not a signature. */
function digest(id: string, session: string, kind: string, reason: string, at: number): string {
  let h = 0xcbf29ce484222325n;
  const mask = 0xffffffffffffffffn;
  for (const b of new TextEncoder().encode(`${id}|${session}|${kind}|${reason}|${at}`)) {
    h = ((h ^ BigInt(b)) * 0x100000001b3n) & mask;
  }
  return h.toString(16).padStart(16, '0');
}

function unitLine(status: string, reason: string, kind: string): { word: string; detail: string } {
  if (status === 'working') return { word: 'Working', detail: 'An agent is on this session right now.' };
  if (status === 'failed' && (kind === 'blocked' || reason === 'blocked_waiting'))
    return { word: 'Stopped', detail: 'The agent stopped on a request only you can answer. Nothing moves until it is answered.' };
  if (status === 'failed' && (kind === 'stuck' || reason === 'stuck'))
    return { word: 'Stopped', detail: 'The watchdog stopped this session and kept its work. Send it a fresh prompt to continue.' };
  if (status === 'failed') return { word: 'Stopped', detail: 'This session failed. Its last action is kept on the row.' };
  if (status === 'done') return { word: 'Done', detail: 'The session finished and its work was verified.' };
  return { word: 'Idle', detail: 'No agent is on this session, and none is waiting to start.' };
}

/** Browser/demo projection. Identical field shape to `fleet_state` in Rust. */
export function projectFleet(snap: Snapshot | null): FleetState {
  const sessions = snap?.sessions ?? [];
  const supervisors = snap?.supervisors ?? [];
  const decisions = snap?.decisions ?? [];
  const bus = snap?.busMessages ?? [];
  const escalations = snap?.escalations ?? [];
  const groups = { working: 0, idle: 0, stopped: 0, done: 0 };
  const units: FleetUnit[] = sessions.map((x) => {
    const esc = escalations.find((e) => e.session === x.id);
    const line = unitLine(x.status, x.failureReason ?? '', esc?.kind ?? '');
    if (x.status === 'working') groups.working += 1;
    else if (x.status === 'failed') groups.stopped += 1;
    else if (x.status === 'done') groups.done += 1;
    else groups.idle += 1;
    return {
      id: x.id, kind: 'session', sup: x.sup, status: x.status,
      word: line.word, detail: line.detail, title: x.title,
      paneId: x.paneId ?? '', workspace: x.cwd ?? '', lastAction: x.lastAction ?? '',
      revision: x.lastRevision ?? 0, escalation: esc?.kind ?? '',
    };
  });
  const topStatus = groups.stopped ? 'failed' : groups.working ? 'working'
    : groups.done > 0 && groups.done === sessions.length ? 'done' : 'idle';
  const holds = supervisors.filter((v) => v.paused).length;
  let fh = 0xcbf29ce484222325n;
  const mask = 0xffffffffffffffffn;
  for (const b of new TextEncoder().encode(`${snap?.generatedAt ?? 0}|${decisions.length}|${bus.length}`)) {
    fh = ((fh ^ BigInt(b)) * 0x100000001b3n) & mask;
  }
  return {
    source: 'snapshot-projection', generatedAt: snap?.generatedAt ?? 0, topStatus, units, groups,
    storm: { holds, guard: holds > 0 ? 'holding' : 'open', starts: decisions.filter((d) => d.action === 'reprompt').length },
    ledger: { seq: decisions.length + bus.length, fence: fh.toString(16).padStart(16, '0'), epoch: snap?.version ?? 1, decisions: decisions.length, messages: bus.length },
  };
}

/** Browser/demo approval projection: one card per session, oldest first. */
export function projectApprovals(snap: Snapshot | null): Approval[] {
  const out: Approval[] = [];
  for (const e of snap?.escalations ?? []) {
    if (e.kind !== 'blocked' && e.kind !== 'stuck') continue;
    out.push({
      id: e.id, session: e.session, supervisor: e.supervisor, kind: e.kind,
      reason: e.reason, at: e.at, signed: false,
      digest: digest(e.id, e.session, e.kind, e.reason, e.at),
    });
  }
  for (const x of snap?.sessions ?? []) {
    if (x.failureReason !== 'blocked_waiting' || out.some((a) => a.session === x.id)) continue;
    const reason = 'The agent is waiting on input only you can give.';
    out.push({
      id: `blocked-${x.id}`, session: x.id, supervisor: x.sup, kind: 'blocked', reason,
      at: x.startedAt, signed: false, digest: digest(`blocked-${x.id}`, x.id, 'blocked', reason, x.startedAt),
    });
  }
  return out.sort((a, b) => a.at - b.at).slice(0, 12);
}

/** Declared harness capabilities. Optional reads DECLINE before any side effect. */
export function declaredHarnesses(): Harness[] {
  const optional = new Set(['signIn', 'usage', 'profiles', 'remoteControl']);
  return [{
    id: 'opencode', label: 'OpenCode',
    capabilities: ['spawn', 'read', 'prompt', 'kill', 'focus', 'status', 'signIn', 'usage', 'profiles', 'remoteControl']
      .map((name) => ({ name, supported: !optional.has(name) })),
  }];
}

/** Load the capability surface: project locally, then overlay real command payloads. */
export async function loadFleet(snap: Snapshot | null): Promise<FleetData> {
  const local: FleetData = {
    state: projectFleet(snap),
    approvals: projectApprovals(snap),
    harnesses: declaredHarnesses(),
  };
  if (!isTauri()) return local;
  const [state, approvals, harnesses] = await Promise.all([
    invoke<FleetState>('fleet_state').catch(() => null),
    invoke<{ approvals: Approval[] }>('list_approvals').catch(() => null),
    invoke<{ harnesses: Harness[] }>('list_harnesses').catch(() => null),
  ]);
  return {
    state: state?.units ? state : local.state,
    approvals: approvals?.approvals ?? local.approvals,
    harnesses: harnesses?.harnesses ?? local.harnesses,
  };
}

/**
 * Tauri `fleet` event → the SAME window CustomEvent the existing use-snapshot
 * hook already listens to. Returned disposer is idempotent. No browser work.
 */
export function registerFleetEvents(onFleet?: (state: FleetState) => void): () => void {
  if (!isTauri()) return () => undefined;
  let stop: (() => void) | null = null;
  let disposed = false;
  listen<{ snapshot: Snapshot; fleet: FleetState }>('fleet', (e) => {
    const payload = e.payload;
    if (!payload?.snapshot) return;
    window.dispatchEvent(new CustomEvent(FLEET_EVENT, { detail: payload.snapshot }));
    onFleet?.(payload.fleet);
  }).then((fn) => { if (disposed) fn(); else stop = fn; }).catch(() => undefined);
  return () => { disposed = true; stop?.(); };
}
