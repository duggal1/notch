// bus.ts — bus type vocabulary, colors, and the snapshot validator (shared by UI + tests).
import { C } from './tokens';
import type { Snapshot, BusMessageType } from './bridge';

export const BUS_TYPES = ['task', 'result', 'question', 'handoff', 'escalate'] as const satisfies readonly BusMessageType[];

export const TYPE_COLOR: Record<BusMessageType, string> = {
  task: C.peri,
  result: '#FFFFFF',
  question: C.lilac,
  handoff: C.blue,
  escalate: C.red,
};

const STATUSES = new Set(['working', 'idle', 'failed', 'done']);

/** Validate a snapshot against bridge.md §1.3. Returns violations; empty array = valid. */
export function validateSnapshot(s: Snapshot): string[] {
  const out: string[] = [];
  if (s == null || typeof s !== 'object') return ['not an object'];
  if (s.version !== 1) out.push('version must be 1');
  if (typeof s.generatedAt !== 'number') out.push('generatedAt must be a number');
  if (s.orchestrator?.id !== 'ORCH' || !STATUSES.has(s.orchestrator?.status)) out.push('orchestrator must be ORCH with a valid status');
  if (!Array.isArray(s.supervisors)) out.push('supervisors must be an array');
  if (!Array.isArray(s.sessions)) out.push('sessions must be an array');
  if (typeof s.eps !== 'number' || !(s.eps >= 0)) out.push('eps must be a non-negative number');

  for (const sv of s.supervisors ?? []) {
    if (!/^SV-\d+$/.test(sv?.id ?? '')) out.push(`supervisor id invalid: ${sv?.id}`);
    if (!STATUSES.has(sv?.status)) out.push(`supervisor status invalid: ${sv?.status}`);
    for (const k of ['working', 'idle', 'failed', 'done'] as const) {
      if (typeof sv[k] !== 'number') out.push(`supervisor ${sv.id}.${k} must be a number`);
    }
  }
  for (const sess of s.sessions ?? []) {
    if (!/^S\d+-OC\d+$/.test(sess?.id ?? '')) out.push(`session id invalid: ${sess?.id}`);
    if (!STATUSES.has(sess?.status)) out.push(`session status invalid: ${sess?.status}`);
    if (typeof sess?.title !== 'string' || sess.title.length === 0) out.push(`session ${sess.id} needs a title`);
    if (!/^SV-\d+$/.test(sess?.sup ?? '')) out.push(`session ${sess.id}.sup invalid: ${sess?.sup}`);
    if (sess.latencyMs !== null && typeof sess.latencyMs !== 'number') out.push(`session ${sess.id} latencyMs must be number|null`);
    if (sess.failureReason !== null && typeof sess.failureReason !== 'string') out.push(`session ${sess.id} failureReason must be string|null`);
    if (sess.iterations !== undefined && typeof sess.iterations !== 'number') out.push(`session ${sess.id} iterations must be number`);
    if (sess.maxIterations !== undefined && typeof sess.maxIterations !== 'number') out.push(`session ${sess.id} maxIterations must be number`);
  }
  const decisions = s.decisions ?? [];
  if (!Array.isArray(decisions)) out.push('decisions must be an array when present');
  else for (const d of decisions) {
    if (!['reprompt', 'done', 'escalate', 'verify', 'assign'].includes(d?.action)) out.push(`decision action invalid: ${d?.action}`);
    if (typeof d?.at !== 'number' || typeof d?.reason !== 'string' || typeof d?.session !== 'string' || typeof d?.supervisor !== 'string') out.push('decision missing fields');
  }
  const escalations = s.escalations ?? [];
  if (!Array.isArray(escalations)) out.push('escalations must be an array when present');
  else for (const e of escalations) {
    if (!['stuck', 'loop_detected', 'budget_exceeded', 'blocked'].includes(e?.kind)) out.push(`escalation kind invalid: ${e?.kind}`);
    if (typeof e?.id !== 'string' || typeof e?.session !== 'string' || typeof e?.supervisor !== 'string' || typeof e?.reason !== 'string' || typeof e?.at !== 'number') out.push('escalation missing fields');
  }
  const busMessages = s.busMessages ?? [];
  if (!Array.isArray(busMessages)) out.push('busMessages must be an array when present');
  else for (const m of busMessages) {
    if (!(BUS_TYPES as readonly string[]).includes(m?.type ?? '')) out.push(`bus message type invalid: ${m?.type}`);
    if (typeof m?.preview !== 'string' || m.preview.length > 140) out.push(`bus message ${m?.id} preview must be ≤140 chars`);
    if (typeof m?.at !== 'number' || typeof m?.from !== 'string' || typeof m?.to !== 'string' || typeof m?.id !== 'string') out.push('bus message missing fields');
  }
  if (!Array.isArray(s.conflicts) || !Array.isArray(s.attribution) || !Array.isArray(s.warnings)) out.push('conflicts/attribution/warnings must be arrays');
  return out;
}
