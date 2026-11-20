// EscalationsTab.tsx — the alarm inbox. One wave instance for the whole list: an action
// resolves into its confirmation through the wave (never N waves, one per card).
// No dismiss — escalations clear when the session changes state or you act.
import { useEffect, useState, type MouseEvent } from 'react';
import { DistortionWave, type PaletteName } from '../../vendor/distortion-waves/src';
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

type Acted = { id: string; label: string; palette: PaletteName } | null;

export function EscalationsTab({ snapshot, onRepromptFresh, onReassign, onKill }: {
  snapshot: Snapshot | null;
  onRepromptFresh: (sessionId: string, goal: string) => void;
  onReassign: (sup: string, goal: string, dod: string | undefined, model: string | undefined) => Promise<string | null>;
  onKill: (sessionId: string) => void;
}) {
  const escalations = snapshot?.escalations ?? [];
  const sessions = snapshot?.sessions ?? [];
  const [acted, setActed] = useState<Acted>(null);
  const [sourceEl, setSourceEl] = useState<HTMLElement | null>(null);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const id = window.setInterval(() => setNow(Date.now()), 1000); return () => window.clearInterval(id); }, []);

  const sessionOf = (id: string) => sessions.find((s) => s.id === id);
  const act = (e: MouseEvent<HTMLButtonElement>, next: Acted) => {
    setSourceEl(e.currentTarget);
    setActed(null);                                  // back to the first view before replaying
    window.requestAnimationFrame(() => setActed(next));
  };

  const cards = (confirmed: boolean) => escalations.map((esc, i) => {
    const s: Session | undefined = sessionOf(esc.session);
    const isActed = confirmed && acted?.id === esc.id;
    const paletteFor = (kind: Escalation['kind']): PaletteName =>
      kind === 'stuck' || kind === 'blocked' ? 'sunset' : 'ocean';
    return (
      <div key={esc.id} style={{ animationDelay: `${Math.min(i, 8) * 34}ms` }} className="esc-card" role="article" aria-label={`Escalation ${esc.id} — ${KIND_LABEL[esc.kind]} — session ${esc.session}`}>
        <div className="esc-rail"><DitherField mode="linear" angle={90} cell={2} floor={0} /></div>
        <div className="esc-body">
          <div className="esc-head">
            <span className="esc-kind"><i className="esc-kind-dot" />{KIND_LABEL[esc.kind]}</span>
            <span className="esc-age tabular-nums">{fmtAge(esc.at, now)} ago</span>
          </div>
          <div className="esc-meta">
            <span className="tabular-nums">{esc.session}</span>
            <span><em>under</em> {esc.supervisor}</span>
          </div>
          <p className="esc-reason">{esc.reason}</p>
          {isActed ? (
            <div className="esc-done"><PixelIcon name="check" size={12} /> {acted?.label}</div>
          ) : (
            <div className="esc-actions">
              <button className="esc-btn esc-btn--primary ripple" onClick={() => onRepromptFresh(esc.session, s?.goal ?? s?.task ?? 'the goal')}>
                <PixelIcon name="reprompt" size={12} /><span>Re-prompt fresh</span>
              </button>
              <button className="esc-btn ripple" onClick={async (e) => {
                const id = await onReassign(esc.supervisor, s?.goal ?? s?.task ?? 'the goal', s?.definitionOfDone, s?.model);
                if (id) act(e, { id: esc.id, label: `Reassigned to ${id}`, palette: paletteFor(esc.kind) });
              }}>
                <span>Reassign</span>
              </button>
              <button className="esc-btn esc-btn--kill ripple" onClick={(e) => {
                onKill(esc.session);
                act(e, { id: esc.id, label: 'Kill sent', palette: 'sunset' });
              }}>
                <PixelIcon name="kill" size={12} /><span>Kill session</span>
              </button>
            </div>
          )}
        </div>
      </div>
    );
  });

  return (
    <div className="escalations">
      <div className="view-head">
        <span className="label">Alarms</span>
        <span className="view-head-count">{String(escalations.length).padStart(2, '0')}</span>
        <span className="view-head-rule" />
        <span className="view-head-note">No dismiss — an alarm clears when the session changes state or you act.</span>
      </div>
      {escalations.length === 0 ? (
        <div className="empty-state">
          <span className="display">All clear.</span>
          <p className="empty-line">Every session is inside its budget and moving. Nothing needs you right now.</p>
        </div>
      ) : (
        <DistortionWave
          className="esc-wave"
          revealed={!!acted}
          type="vertical"
          palette={acted?.palette ?? 'ocean'}
          scale="auto"
          cornerRadius={0}
          prewarm={false}
          source={sourceEl}
          revealedContent={<div className="esc-scroll"><div className="esc-list">{cards(true)}</div></div>}
        >
          <div className="esc-scroll"><div className="esc-list">{cards(false)}</div></div>
        </DistortionWave>
      )}
    </div>
  );
}
