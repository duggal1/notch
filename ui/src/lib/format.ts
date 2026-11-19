// format.ts — derived, honest numbers. Everything here is a pure function of the snapshot.
import type { Session } from './bridge';
import type { Status } from './tokens';

export const pad2 = (n: number) => String(n).padStart(2, '0');

/** Elapsed since startedAt. mm:ss under 1h, h:mm:ss after. Never negative. */
export function fmtElapsed(startedAt: number, now = Date.now()): string {
  const s = Math.max(0, Math.floor((now - startedAt) / 1000));
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  return h > 0 ? `${h}:${pad2(m)}:${pad2(r)}` : `${pad2(m)}:${pad2(r)}`;
}

/** Worst-wins aggregate: failed > any working > all-done > idle. */
export function topStatus(sessions: Session[]): Status {
  if (sessions.some((s) => s.status === 'failed')) return 'failed';
  if (sessions.some((s) => s.status === 'working')) return 'working';
  if (sessions.length > 0 && sessions.every((s) => s.status === 'done')) return 'done';
  return 'idle';
}

export const workingCount = (sessions: Session[]) =>
  sessions.filter((s) => s.status === 'working').length;

/** eps normalised 0..1 against 40 events/s full scale. */
export const normalizeEps = (eps: number) => Math.min(1, Math.max(0, eps / 40));

/** bridge failureReason → human words. Unknown codes pass through uppercased; null stays honest. */
export function failureLabel(reason: string | null): string {
  switch ((reason ?? '').toLowerCase()) {
    case 'stuck': return 'STUCK — WATCHDOG';
    case 'spawn_failed': return 'SPAWN FAILED';
    case 'blocked_waiting': return 'BLOCKED — WAITING ON INPUT';
    case '': return 'FAILED';
    default: return (reason as string).toUpperCase().replace(/_/g, ' ');
  }
}

export interface Drift {
  n: number;            // sessions with latency data
  pct: number | null;   // (worst - best) / best * 100, null when n < 2
  best: number | null;
  worst: number | null;
}

/** Step-latency honesty: best vs worst rolling avg across sessions. Returns null parts when insufficient. */
export function drift(sessions: Session[]): Drift {
  const vals = sessions.map((s) => s.latencyMs).filter((v): v is number => typeof v === 'number' && Number.isFinite(v));
  if (vals.length === 0) return { n: 0, pct: null, best: null, worst: null };
  const best = Math.min(...vals), worst = Math.max(...vals);
  return {
    n: vals.length,
    best: Math.round(best),
    worst: Math.round(worst),
    pct: vals.length < 2 || best === 0 ? null : ((worst - best) / best) * 100,
  };
}

export const fmtMs = (v: number) => `${Math.round(v)} ms`;

export const fmtEps = (eps: number) => eps.toFixed(1);
