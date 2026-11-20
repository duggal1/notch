// bridge.ts — the v1.3 Tauri wire. Object shapes stay exactly bridge.md §1.3 (camelCase).
// Tauri mode: listen('snapshot') + invoke(..., returns the payload directly).
// Browser (?demo=) mode: demo provider emits the same shapes.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Status } from './tokens';
import { connectWs, credentialsFromQuery, buildWsUrl, FLEET_EVENT, type WsCredentials } from './ws-transport';

export interface Session {
  id: string;
  sup: string;
  status: Status;
  title: string;
  task: string; // original prompt head
  cwd: string;
  paneId: string;
  startedAt: number; // ms epoch
  lastAction: string; // best-effort, single line
  lastRevision: number;
  latencyMs: number | null;
  failureReason: string | null;
  // v1.3 optional store
  goal?: string;
  definitionOfDone?: string;
  iterations?: number;
  maxIterations?: number;
  wallClockDeadline?: number;
  lastSupervisorAction?: string;
  model?: string;
}

export interface Supervisor {
  id: string;
  status: Status;
  working: number;
  idle: number;
  failed: number;
  done: number;
  paused?: boolean;
}

export type DecisionAction = 'reprompt' | 'done' | 'escalate' | 'verify' | 'assign';
export interface Decision {
  at: number;
  supervisor: string;
  session: string;
  action: DecisionAction;
  reason: string;
}

export type EscalationKind = 'stuck' | 'loop_detected' | 'budget_exceeded' | 'blocked';
export interface Escalation {
  id: string;
  session: string;
  supervisor: string;
  kind: EscalationKind;
  reason: string;
  at: number;
}

export type BusMessageType = 'task' | 'result' | 'question' | 'handoff' | 'escalate';
export interface BusMessage {
  id: string;
  from: string;
  to: string;
  type: BusMessageType;
  preview: string; // ≤ 140 chars
  at: number;
}

export interface LogsPayload { id: string; lines: string[] }

export interface Snapshot {
  version: 1;
  generatedAt: number;
  orchestrator: { id: 'ORCH'; status: Status };
  supervisors: Supervisor[];
  sessions: Session[];
  eps: number;
  conflicts: { file: string; sessions: string[] }[];
  attribution: { session: string; file: string }[];
  decisions?: Decision[]; // newest-first, cap 12
  escalations?: Escalation[]; // unresolved only
  busMessages?: BusMessage[]; // newest-first, cap 30
  warnings: string[];
}

export const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export type UiAction =
  | { cmd: 'set_frame'; mode: 'rest' | 'peek' | 'open' }
  | { cmd: 'open_dashboard' }
  | { cmd: 'kill'; id: string }
  | { cmd: 'reprompt'; id: string; text: string }
  | { cmd: 'logs'; id: string; tail?: number }
  | { cmd: 'focus'; id: string }
  | { cmd: 'cleanup'; id?: string }
  | { cmd: 'haptic'; kind: 'alignment' | 'levelChange' }
  | { cmd: 'snapshot' }
  | { cmd: 'submit_task'; task: string; cwd?: string; title?: string; definitionOfDone?: string; model?: string; supervisorId?: string }
  | { cmd: 'message'; from: string; to: string; type: BusMessageType; body: string }
  | { cmd: 'pause_supervisor'; id: string }
  | { cmd: 'resume_supervisor'; id: string }
  | { cmd: 'kill_switch' }
  | { cmd: 'list_models' };

/** Fire-and-forget send — errors are logged, never thrown into the render path. */
export function send(a: UiAction): void {
  // Headless interaction probe: records the command only when a harness installs
  // window.__notchActions. No array → no work, no allocation in the shipped app.
  const log = (window as unknown as { __notchActions?: unknown[] }).__notchActions;
  if (Array.isArray(log)) log.push({ ...a, at: Date.now() });
  invokeInner(a).catch((err) => console.warn('[notch] invoke failed', a.cmd, err));
}

async function invokeInner(a: UiAction): Promise<unknown> {
  if (isTauri()) {
    const { cmd, ...args } = a;
    return invoke(cmd, args);
  }
  return demoInvoke?.(a);
}

/** Invoke and return the payload (v1.3: commands return payloads directly). */
export async function invokePayload<T>(a: UiAction, fallback: T): Promise<T> {
  try {
    const v = (await invokeInner(a)) as T | undefined | null;
    return v ?? fallback;
  } catch {
    return fallback;
  }
}

export const logs = (id: string, tail = 200) => invokePayload<{ id: string; lines: string[] }>({ cmd: 'logs', id, tail }, { id, lines: [] });
export const submitTask = (a: Omit<Extract<UiAction, { cmd: 'submit_task' }>, 'cmd'>) =>
  invokePayload<{ ok: boolean; supervisorId?: string; sessionId?: string; error?: string }>({ cmd: 'submit_task', ...a }, { ok: false, error: 'submit_task unavailable outside the app' });
export const listModels = () =>
  invokePayload<{ models: string[]; error?: string }>({ cmd: 'list_models' }, { models: [], error: 'list_models unavailable outside the app' });

// Demo provider hook — demos.ts registers itself here in browser mode.
let demoInvoke: ((a: UiAction) => unknown) | null = null;
export function registerDemoInvoke(fn: (a: UiAction) => unknown) { demoInvoke = fn; }

type Handlers = { onSnapshot: (s: Snapshot) => void; onFleet?: (s: Snapshot) => void };

// Optional bridge-supplied daemon state-dir credential. app/ can inject
// `window.__NOTCH_WS__ = { port, token }` or register a provider; WS is then added
// on top of the Tauri transport. Without it the Tauri `listen('snapshot')` path is
// the sole app transport, unchanged.
let wsCredentialProvider: (() => WsCredentials | null) | null = null;
export function registerWsCredentials(fn: () => WsCredentials | null) { wsCredentialProvider = fn; }
function bridgeCredentials(): WsCredentials | null {
  try {
    const injected = (window as unknown as { __NOTCH_WS__?: { port?: unknown; token?: unknown } }).__NOTCH_WS__;
    if (injected && typeof injected.token === 'string') {
      const port = Number(injected.port);
      if (Number.isInteger(port) && port > 0 && port <= 65535) return { port, token: injected.token };
    }
    return wsCredentialProvider?.() ?? null;
  } catch { return null; }
}

/** One normalized fleet delivery path: existing callback, optional callback, then the single `fleet` event. */
function emitFleet(s: Snapshot, onSnapshot: (s: Snapshot) => void, onFleet?: (s: Snapshot) => void) {
  onSnapshot(s);
  onFleet?.(s);
  try {
    (window as unknown as { dispatchEvent?: (e: Event) => void }).dispatchEvent?.(
      new CustomEvent(FLEET_EVENT, { detail: s }));
  } catch { /* non-DOM test host: the callbacks above still fire */ }
}

/**
 * THE transport switch (one place). Tauri keeps `listen('snapshot')` primary and adds
 * WS only when the bridge supplies the state-dir token. Browser uses WS with `?token=`
 * or falls back to the original demo provider, so screenshots are unaffected.
 */
export async function subscribe({ onSnapshot, onFleet }: Handlers): Promise<() => void> {
  const deliver = (s: Snapshot) => emitFleet(s, onSnapshot, onFleet);
  const disposers: (() => void)[] = [];
  const w = window as unknown as { __push?: (s: Snapshot) => void };
  if (typeof w.__push !== 'function') w.__push = (s) => { if (s) deliver(s); };

  if (isTauri()) {
    const un = await listen<Snapshot>('snapshot', (e) => deliver(e.payload));
    invoke<Snapshot>('snapshot').then((s) => { if (s) deliver(s); }).catch(() => undefined);
    disposers.push(() => un());
    const creds = bridgeCredentials();
    if (creds) disposers.push(connectWs({ url: buildWsUrl(creds), token: creds.token, onFleet: deliver }));
    return () => { for (const d of disposers) d(); };
  }

  const creds = credentialsFromQuery();
  if (creds) {
    disposers.push(connectWs({ url: buildWsUrl(creds), token: creds.token, onFleet: deliver }));
    return () => { for (const d of disposers) d(); };
  }
  const { installDemo } = await import('./demos');
  const stopDemo = await installDemo({ onSnapshot: deliver });
  return () => { stopDemo(); for (const d of disposers) d(); };
}
