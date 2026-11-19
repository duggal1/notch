// bridge.ts — the Tauri wire. Object shapes stay exactly bridge.md §1.3 (camelCase).
// Tauri mode: listen('snapshot' | 'logs') + invoke(...). Browser (?demo=) mode: demo provider emits the same shapes.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Status } from './tokens';

export interface Session {
  id: string;
  sup: string;
  status: Status;
  title: string;
  task: string;
  cwd: string;
  paneId: string;
  startedAt: number; // ms epoch
  lastAction: string;
  lastRevision: number;
  latencyMs: number | null; // rolling avg step latency
  failureReason: string | null; // e.g. "stuck" | "spawn_failed" | "blocked_waiting"
}

export interface Supervisor {
  id: string;
  status: Status;
  working: number;
  idle: number;
  failed: number;
  done: number;
}

export interface Snapshot {
  version: 1;
  generatedAt: number;
  orchestrator: { id: 'ORCH'; status: Status };
  supervisors: Supervisor[];
  sessions: Session[];
  eps: number; // bus events/sec
  conflicts: { file: string; sessions: string[] }[];
  attribution: { session: string; file: string }[];
  warnings: string[];
}

export interface LogsPayload { id: string; lines: string[] }

export const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export type UiAction =
  | { type: 'set_frame'; mode: 'rest' | 'peek' | 'open' }
  | { type: 'kill'; id: string }
  | { type: 'reprompt'; id: string; text: string }
  | { type: 'logs'; id: string }
  | { type: 'focus'; id: string }
  | { type: 'cleanup' }
  | { type: 'haptic'; kind: 'alignment' | 'levelChange' }
  | { type: 'snapshot' };

/** Fire-and-forget. In browser mode actions are echoed to the demo provider, never to a network. */
export function send(a: UiAction): void {
  if (isTauri()) {
    const { type, ...args } = a as { type: string } & Record<string, unknown>;
    invoke(type, args).catch((err) => console.warn('[notch] invoke failed', type, err));
    return;
  }
  demoAction?.(a);
}

// Demo provider hook — demos.ts registers itself here in browser mode.
let demoAction: ((a: UiAction) => void) | null = null;
export function registerDemoAction(fn: (a: UiAction) => void) { demoAction = fn; }

type Handlers = { onSnapshot: (s: Snapshot) => void; onLogs: (p: LogsPayload) => void };

/** Subscribe. Tauri: listen('snapshot' | 'logs'); browser: caller must have demo provider registered. */
export async function subscribe({ onSnapshot, onLogs }: Handlers): Promise<() => void> {
  if (isTauri()) {
    const un1 = await listen<Snapshot>('snapshot', (e) => onSnapshot(e.payload));
    const un2 = await listen<LogsPayload>('logs', (e) => onLogs(e.payload));
    invoke('snapshot').catch(() => undefined); // request one fresh snapshot; harmless if unhandled
    return () => { un1(); un2(); };
  }
  const { installDemo } = await import('./demos');
  return installDemo({ onSnapshot, onLogs });
}
