import { useEffect, useState } from 'react';
import type { Snapshot, Escalation, Session } from '../../lib/bridge';
import { fmtAge } from '../../lib/format';
import { PixelIcon } from '../PixelIcon';
import { DitherField } from '../pixel/DitherField';

const KIND_LABEL: Record<Escalation['kind'], string> = {
  stuck: 'Stuck',
  loop_detected: 'Loop detected',
  budget_exceeded: 'Budget exceeded',
  blocked: 'Blocked',
};

export function EscalationsTab({ snapshot, onRepromptFresh, onReassign, onKill, onOpenSession }: {
  snapshot: Snapshot | null;
  onRepromptFresh: (sessionId: string, goal: string) => void;
  onReassign: (sup: string, goal: string, dod: string | undefined, model: string | undefined) => Promise<string | null>;
  onOpenSession: (id: string) => void;
  onKill: (sessionId: string) => void;
}) {
  const escalations = snapshot?.escalations ?? [];
  const sessions = snapshot?.sessions ?? [];
  const [feedback, setFeedback] = useState<Record<string, string>>({});
  const [dismissed, setDismissed] = useState<Set<string>>(() => new Set());
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const id = window.setInterval(() => setNow(Date.now()), 1000); return () => window.clearInterval(id); }, []);

  const sessionOf = (id: string) => sessions.find((s) => s.id === id);
  const visible = escalations.filter((alarm) => !dismissed.has(alarm.id));
  const cards = visible.map((esc, i) => {
    const s: Session | undefined = sessionOf(esc.session);
    const note = feedback[esc.id];
    return (
      <div key={esc.id} style={{ animationDelay: `${Math.min(i, 8) * 34}ms` }} className="esc-card" role="article" aria-label={`Escalation ${esc.id} — ${KIND_LABEL[esc.kind]} — session ${esc.session}`}>
        <div className="esc-rail"><DitherField mode="linear" angle={90} cell={4} floor={0.72} /></div>
        <div className="esc-body">
          <div className="esc-head">
            <span className="esc-kind"><PixelIcon name="alert" size={12} />{KIND_LABEL[esc.kind]}</span>
            <span className="esc-age tabular-nums">{fmtAge(esc.at, now)} ago</span>
          </div>
          <div className="esc-meta">
            <span className="tabular-nums">{esc.session}</span>
            <span><em>under</em> {esc.supervisor}</span>
          </div>
          <h2 className="esc-reason">{esc.reason}</h2>
          {note && <p role="status" className="esc-done">{note}</p>}
            <div className="esc-actions">
              <button className="esc-btn esc-btn--primary ripple" onClick={() => onRepromptFresh(esc.session, s?.goal ?? s?.task ?? 'the goal')}>
                <PixelIcon name="reprompt" size={12} /><span>Re-prompt</span>
              </button>
              <button className="esc-btn ripple" onClick={async () => {
                const id = await onReassign(esc.supervisor, s?.goal ?? s?.task ?? 'the goal', s?.definitionOfDone, s?.model);
                setFeedback((previous) => ({ ...previous, [esc.id]: id ? `Reassigned to ${id}` : 'Reassign failed. Try again.' }));
              }}>
                <span>Reassign</span>
              </button>
              <button className="esc-btn esc-btn--kill ripple" onClick={() => {
                onKill(esc.session);
                setFeedback((previous) => ({ ...previous, [esc.id]: 'Kill sent' }));
              }}>
                <PixelIcon name="kill" size={12} /><span>Kill session</span>
              </button>
              <button className="esc-btn" onClick={() => onOpenSession(esc.session)}>Open session</button>
              <button className="esc-btn" onClick={() => setDismissed((previous) => new Set([...previous, esc.id]))}>Dismiss</button>
            </div>
        </div>
      </div>
    );
  });

  return (
    <div className="escalations">
      {visible.length === 0 ? (
        <div className="empty-state alarms-empty">
          <div className="alarms-field" aria-hidden="true"><DitherField mode="radial" cell={4} floor={0} ceil={0.28} /></div>
          <div><h2 className="title">All clear</h2><p className="empty-line">No alarms need your attention.</p></div>
        </div>
      ) : (
        <>
          <div className="esc-scroll"><div className="esc-list">{cards}</div></div>
          <p className="escalations-note">Alarms clear when the session changes state or you act.</p>
        </>
      )}
    </div>
  );
}
