// derive.ts — pure selectors for the fleet surface. Structure stolen from
// agentbox `live-line.ts` / `advance.ts` / `one-thing-at-a-time.ts`:
// one honest line per event, one decision on stage, advance without a hop.
// No visual language is copied; our tokens and classes stay ours.
import type { Snapshot } from '../../lib/bridge';
import type { Approval, FleetUnit } from '../../lib/fleet-wiring';

export interface ActivityItem {
  seq: number;
  at: number;
  kind: 'status' | 'decision' | 'message' | 'output';
  text: string;
  session?: string;
}

/**
 * Structured activity records, newest first, with a real sequence number.
 * DISPLAY ONLY, NEVER EVIDENCE: the snapshot is the source of truth and these
 * lines are its readable projection (the honesty split agentbox makes between
 * `run.frame.live` and a persisted frame). No screenshots, no capture loop.
 */
export function deriveActivity(snap: Snapshot | null, limit = 24): ActivityItem[] {
  const raw: Omit<ActivityItem, 'seq'>[] = [];
  for (const d of snap?.decisions ?? []) {
    raw.push({ at: d.at, kind: 'decision', session: d.session, text: `${d.supervisor} ${d.action} ${d.session} — ${d.reason}` });
  }
  for (const m of snap?.busMessages ?? []) {
    raw.push({ at: m.at, kind: 'message', session: m.to, text: `${m.from} → ${m.to} · ${m.type}: ${m.preview}` });
  }
  for (const u of snap?.sessions ?? []) {
    if (u.lastAction) raw.push({ at: (u.startedAt ?? 0) + 1, kind: 'output', session: u.id, text: `${u.id} · ${u.lastAction}` });
    raw.push({ at: snap?.generatedAt ?? 0, kind: 'status', session: u.id, text: `${u.id} · ${u.status}${u.failureReason ? ` (${u.failureReason})` : ''}` });
  }
  raw.sort((a, b) => b.at - a.at);
  return raw.slice(0, limit).map((item, i) => ({ ...item, seq: raw.length - i }));
}

export interface DecisionCard {
  id: string;
  session: string;
  supervisor: string;
  kind: string;
  reason: string;
  age: number;
  digest: string;
  signed: boolean;
}

/** One card per session, oldest first. An answer from the list stays in the list. */
export function stageQueue(approvals: Approval[], units: FleetUnit[], now = Date.now()): DecisionCard[] {
  const known = new Set(units.map((u) => u.id));
  return approvals
    .filter((a) => !known.size || known.has(a.session))
    .map((a) => ({
      id: a.id, session: a.session, supervisor: a.supervisor, kind: a.kind,
      reason: a.reason, age: Math.max(0, now - a.at), digest: a.digest, signed: a.signed,
    }));
}

/** Step within the SAME stage, stopping at either end (no navigation away). */
export function stepStage(cards: DecisionCard[], index: number, dir: 1 | -1): number {
  if (cards.length === 0) return index;
  return Math.min(cards.length - 1, Math.max(0, index + dir));
}

/** Which unit is the honest focus when a supervisor row is picked. */
export function firstUnitOf(units: FleetUnit[], sup: string): FleetUnit | null {
  return units.find((u) => u.sup === sup && u.status === 'working')
    ?? units.find((u) => u.sup === sup)
    ?? null;
}

export function fmtStageAge(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}
