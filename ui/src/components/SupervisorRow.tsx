// SupervisorRow.tsx — popover (compact, 44px) + dashboard sidebar (58px). Plain-language counts, no cryptic "1w 0i 0f".
import { memo, useMemo } from 'react';
import type { Supervisor, Session, Decision } from '../lib/bridge';
import { fmtAge, capitalizeSentence } from '../lib/format';
import { PixelIcon } from './PixelIcon';

export const SupervisorRow = memo(function SupervisorRow({ sup, sessions, decisions, now, onMessage, onTogglePause, onKill, onSelect, variant, selected }: {
  sup: Supervisor;
  sessions: Session[];
  decisions?: Decision[];
  now: number;
  onMessage: (supId: string) => void;
  onTogglePause: (sup: Supervisor) => void;
  onKill: (supId: string) => void;
  selected?: boolean;
  onSelect: (supId: string) => void;
  variant: 'popover' | 'dashboard';
}) {
  const own = useMemo(() => sessions.filter((s) => s.sup === sup.id), [sessions, sup.id]);
  const last = useMemo(() => decisions?.find((d) => d.supervisor === sup.id) ?? null, [decisions, sup.id]);

  const charter = useMemo(() => {
    if (last) return capitalizeSentence(last.reason);
    const working = own.find((s) => s.status === 'working');
    if (working?.lastAction) return `${working.id} ${working.lastAction}`;
    if (sup.paused) return 'Paused — live sessions keep running';
    if (own.length === 0) return 'No sessions assigned';
    return 'Idle — no decisions yet';
  }, [last, own, sup.paused]);

  const model = useMemo(() => own.find((s) => s.model)?.model ?? null, [own]);
  const age = last ? fmtAge(last.at, now) : '—';
  const summary = [
    sup.working > 0 && `${sup.working} working`,
    sup.failed > 0 && `${sup.failed} failed`,
    sup.working === 0 && sup.failed === 0 && (sup.done > 0 ? `${sup.done} done` : 'Idle'),
  ].filter(Boolean).join(' · ');
  const dash = variant === 'dashboard';

  return (
    <div className={`item ${dash ? 'item--row-tall' : 'item--row'} sup-row`} role="row" tabIndex={0}
      data-sv={sup.id} aria-label={`${sup.id} — ${charter}`}
      aria-pressed={selected}
      onClick={() => onSelect(sup.id)}
      onKeyDown={(e) => { if (e.currentTarget === e.target && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); e.stopPropagation(); onSelect(sup.id); } }}>
      <div className="sup-line">
        <i className={`dot dot--${sup.status}`} aria-hidden="true" />
        <span className="sup-id">{sup.id}{sup.paused && <span className="dim sup-paused">Paused</span>}</span>
        <span className="sup-right" style={{ ['--rw' as string]: dash ? '84px' : '150px' }}>
          <span className="sup-rest dim" data-rest>{dash && last ? age : summary}</span>
          <span className="sup-actions" data-actions>
            <button className="act" aria-label={`Message ${sup.id}`} title="Message supervisor"
              onClick={(e) => { e.stopPropagation(); onMessage(sup.id); }}>
              <PixelIcon name="bubble" size={12} />
            </button>
            <button className="act" aria-label={sup.paused ? `Resume ${sup.id}` : `Pause ${sup.id}`} title={sup.paused ? 'Resume' : 'Pause'}
              onClick={(e) => { e.stopPropagation(); onTogglePause(sup); }}>
              {sup.paused ? <PixelIcon name="play" size={12} /> : <PixelIcon name="pause" size={12} />}
            </button>
            <button className="act" aria-label={`Kill all sessions under ${sup.id}`} title={`Kill every session under ${sup.id}`}
              onClick={(e) => { e.stopPropagation(); onKill(sup.id); }}>
              <PixelIcon name="kill" size={12} />
            </button>
          </span>
        </span>
      </div>
      <div className="label dim sup-charter truncate">
        <span>{charter}</span>
        {model && <span> · {model.split('/')[1] ?? model}</span>}
      </div>
      {dash && <div className="label dim sup-counts">{summary}</div>}
    </div>
  );
});