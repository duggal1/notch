// EscalationsTab.tsx — the unresolved inbox. Each card answers: what kind, whose, why, how old.
// Actions: Re-prompt fresh (composer prefilled) · Reassign (fresh session, same goal+DoD) · Kill.
// No dismiss — escalations clear when the session changes state or you act.
import { useEffect, useState } from 'react';
import type { Snapshot, Escalation } from '../../lib/bridge';
import { fmtAge } from '../../lib/format';
import { RepromptIcon, KillIcon, CheckIcon } from '../Icons';

const KIND_LABEL: Record<Escalation['kind'], string> = {
  stuck: 'Stuck',
  loop_detected: 'Loop detected',
  budget_exceeded: 'Budget exceeded',
  blocked: 'Blocked',
};

export function EscalationsTab({ snapshot, onRepromptFresh, onReassign, onKill }: {
  snapshot: Snapshot | null;
  onRepromptFresh: (sessionId: string, goal: string) => void;
  onReassign: (sup: string, goal: string, dod: string | undefined, model: string | undefined) => Promise<string | null>;
  onKill: (sessionId: string) => void;
}) {
  const escalations = snapshot?.escalations ?? [];
  const sessions = snapshot?.sessions ?? [];
  const [reassigned, setReassigned] = useState<Record<string, string>>({});
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const id = window.setInterval(() => setNow(Date.now()), 1000); return () => window.clearInterval(id); }, []);

  const sessionOf = (id: string) => sessions.find((s) => s.id === id);

  return (
    <div className="escalations">
      {escalations.length === 0 ? (
        <div className="escalations-empty">
          <span className="label">No escalations</span>
          <p className="escalations-empty-line">Every session is inside its budget.</p>
        </div>
      ) : (
        escalations.map((esc) => {
          const s = sessionOf(esc.session);
          const done = reassigned[esc.id];
          return (
            <div key={esc.id} className="esc-card" role="article" aria-label={`Escalation ${esc.id} — ${KIND_LABEL[esc.kind]} — session ${esc.session}`}>
              <div className="esc-head">
                <span className="esc-kind"><i className="esc-kind-dot" />{KIND_LABEL[esc.kind]}</span>
                <span className="mono esc-age">{fmtAge(esc.at, now)} ago</span>
              </div>
              <div className="esc-meta">
                <span className="mono">{esc.session}</span>
                <span className="mono" style={{ color: 'rgba(255,255,255,.4)' }}>under {esc.supervisor}</span>
              </div>
              <p className="esc-reason">{esc.reason}</p>
              {done ? (
                <div className="esc-done"><CheckIcon size={12} /> Reassigned to {done}</div>
              ) : (
                <div className="esc-actions">
                  <button className="esc-btn esc-btn--primary" onClick={() => onRepromptFresh(esc.session, s?.goal ?? s?.task ?? 'the goal')}>
                    <RepromptIcon size={13} /><span>Re-prompt fresh</span>
                  </button>
                  <button className="esc-btn" onClick={async () => {
                    const id = await onReassign(esc.supervisor, s?.goal ?? s?.task ?? 'the goal', s?.definitionOfDone, s?.model);
                    if (id) setReassigned((p) => ({ ...p, [esc.id]: id }));
                  }}>
                    <span>Reassign</span>
                  </button>
                  <button className="esc-btn esc-btn--kill" onClick={() => onKill(esc.session)}>
                    <KillIcon size={13} /><span>Kill session</span>
                  </button>
                </div>
              )}
            </div>
          );
        })
      )}
      <p className="escalations-note">Escalations clear when the session changes state or you act — there is no dismiss.</p>
    </div>
  );
}
