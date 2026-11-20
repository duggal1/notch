// WhiteHotRow.tsx — session row. Rest: title, id, elapsed. Hot: white wipe, ink flip, actions slide in. 16pt icons.
import { memo } from 'react';
import type { Session } from '../lib/bridge';
import { failureLabel, fmtElapsed } from '../lib/format';
import { PixelIcon } from './PixelIcon';

export const SessionRow = memo(function SessionRow({ session, now, killing, onLogs, onReprompt, onFocus, onKill }: {
  session: Session;
  now: number;
  killing: boolean;
  onLogs: (s: Session) => void;
  onReprompt: (s: Session) => void;
  onFocus: (id: string) => void;
  onKill: (id: string) => void;
}) {
  const failed = session.status === 'failed';
  return (
    <div
      className={`item item--row row row-enter${killing ? ' row--killing' : ''}`}
      role="row"
      tabIndex={0}
      data-id={session.id}
      data-status={session.status}
      onClick={() => onFocus(session.id)}
      onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); onFocus(session.id); } }}
      aria-label={`${session.id} — ${session.title} — ${session.status}`}
    >
      <i className={`dot dot--${session.status}`} aria-hidden="true" />
      <div className="row-main">
        <div className="row-title">{session.title}</div>
        <div className="label dim row-meta">
          <span>{session.id} · {session.sup}</span>
          {failed && <span className="row-fail">· {failureLabel(session.failureReason)}</span>}
        </div>
      </div>
      <div className="row-right">
        <div className="row-elapsed" data-elapsed style={killing ? { opacity: 1 } : undefined}>
          {killing ? 'Kill sent' : fmtElapsed(session.startedAt, now)}
        </div>
        <div className="row-actions" data-actions>
          <button className="act" aria-label="Logs" title="Logs" onClick={(e) => { e.stopPropagation(); onLogs(session); }}>
            <PixelIcon name="logs" size={12} />
          </button>
          <button className="act act--reprompt" aria-label="Re-prompt" title="Re-prompt" data-emphasis={failed ? '' : undefined}
            onClick={(e) => { e.stopPropagation(); onReprompt(session); }}>
            <PixelIcon name="reprompt" size={12} />
          </button>
          <button className="act" aria-label="Focus pane" title="Focus pane" onClick={(e) => { e.stopPropagation(); onFocus(session.id); }}>
            <PixelIcon name="focus" size={12} />
          </button>
          <button className="act" aria-label="Kill session" title="Kill" onClick={(e) => { e.stopPropagation(); onKill(session.id); }}>
            <PixelIcon name="kill" size={12} />
          </button>
        </div>
      </div>
    </div>
  );
});