// fleet-wiring.ts — the UI binds to the ONE backend-derived fleet view (C3).
// No TypeScript projection: fleet_state / list_approvals / list_harnesses and the
// `fleet` event carry Rust truth; this file only transports and types it.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { isTauri, type Snapshot } from './bridge';
import { FLEET_EVENT, FLEET_STATE_EVENT } from './ws-transport';

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
  id: string; kind: string; sup: string; status: string; topStatus: string;
  word: string; detail: string; title: string; paneId: string; workspace: string;
  lastAction: string | null; revision: number | null; escalation: string;
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
export interface Harness { id: string; label: string; offered?: boolean; capabilities: HarnessCapability[] }
export interface Approval {
  id: string; session: string; supervisor: string; kind: string;
  reason: string; at: number; digest: string; signed: boolean;
}
export interface FleetData { state: FleetState; approvals: Approval[]; harnesses: Harness[] }

export const EMPTY_FLEET_STATE: FleetState = {
  source: 'empty', generatedAt: 0, topStatus: 'idle', units: [],
  groups: { working: 0, idle: 0, stopped: 0, done: 0 },
  storm: { holds: 0, guard: 'open', starts: 0 },
  ledger: { seq: 0, fence: '0000000000000000', epoch: 0, decisions: 0, messages: 0 },
};

/** Load the backend view once. Browser mode has no backend: an honest empty deck. */
export async function loadFleet(): Promise<FleetData> {
  if (!isTauri()) return { state: EMPTY_FLEET_STATE, approvals: [], harnesses: [] };
  const [state, approvals, harnesses] = await Promise.all([
    invoke<FleetState>('fleet_state').catch(() => null),
    invoke<{ approvals: Approval[] }>('list_approvals').catch(() => null),
    invoke<{ harnesses: Harness[] }>('list_harnesses').catch(() => null),
  ]);
  return {
    state: state ?? EMPTY_FLEET_STATE,
    approvals: approvals?.approvals ?? [],
    harnesses: harnesses?.harnesses ?? [],
  };
}

/**
 * Tauri `fleet` event → the same window CustomEvent the snapshot hook already uses,
 * plus the raw fleet view. WS browser mode delivers the same view via FLEET_STATE_EVENT.
 * Returned disposer is idempotent.
 */
export function registerFleetEvents(onFleet?: (state: FleetState) => void): () => void {
  const onWsState = (e: Event) => {
    const detail = (e as CustomEvent<FleetState>).detail;
    if (detail && Array.isArray(detail.units)) onFleet?.(detail);
  };
  window.addEventListener(FLEET_STATE_EVENT, onWsState);
  if (!isTauri()) return () => window.removeEventListener(FLEET_STATE_EVENT, onWsState);

  let stop: (() => void) | null = null;
  let disposed = false;
  listen<{ snapshot: Snapshot; fleet: FleetState }>('fleet', (e) => {
    const payload = e.payload;
    if (!payload?.snapshot) return;
    window.dispatchEvent(new CustomEvent(FLEET_EVENT, { detail: payload.snapshot }));
    onFleet?.(payload.fleet);
  }).then((fn) => { if (disposed) fn(); else stop = fn; }).catch(() => undefined);
  return () => {
    disposed = true;
    stop?.();
    window.removeEventListener(FLEET_STATE_EVENT, onWsState);
  };
}
