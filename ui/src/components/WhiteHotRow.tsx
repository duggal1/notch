// WhiteHotRow.tsx — the .item.row from DESIGN.md §8/§9: white clip-path wipe, ink flip, actions slide in.
// White-hot switching itself is imperative (entry-point marker) — see TreeList's useWhiteHot.
import { memo } from 'react';
import type { Session } from '../lib/bridge';
import { failureLabel, fmtElapsed } from '../lib/format';
import { LogsIcon, RepromptIcon, FocusIcon, KillIcon } from './Icons';

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
  const repromptEmphasis = failed;
  return (
    <div
      className={`item row flex h-11 items-center gap-3 px-3${killing ? ' row--killing' : ''}`}
      role="row"
      tabIndex={0}
      data-id={session.id}
      data-status={session.status}
      data-killing={killing ? '' : undefined}
      onClick={() => onFocus(session.id)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); onFocus(session.id); }
      }}
      aria-label={`${session.id} — ${session.title} — ${session.status}`}
    >
      <i className={`dot dot--${session.status}`} aria-hidden="true" />
      <div className="flex min-w-0 flex-1 flex-col justify-center gap-[3px]">
        <div className="truncate text-[13px] font-medium leading-none tracking-[-.01em]">{session.title}</div>
        <div className="label row-meta-dim flex items-center gap-1.5 whitespace-nowrap" style={{ letterSpacing: '.14em' }}>
          <span>{session.id} · {session.sup}</span>
          {failed && (
            <>
              <span aria-hidden="true" style={{ color: 'rgba(255,179,193,.5)' }}>·</span>
              <span className="row-meta-dim" style={{ color: '#FFB3C1' }}>{failureLabel(session.failureReason)}</span>
            </>
          )}
        </div>
      </div>
      {/* right cell: elapsed at rest, actions on hot */}
      <div className="relative h-full shrink-0" style={{ width: 136 }}>
        <div
          className="absolute inset-0 flex flex-col items-end justify-center transition-opacity duration-140"
          style={{ opacity: killing ? 1 : undefined }}
          data-elapsed
        >
          <span className="mono num text-[13px] leading-none" style={{ color: 'rgba(255,255,255,.82)' }}>
            {killing ? 'KILL' : fmtElapsed(session.startedAt, now)}
          </span>
          <span className="label mt-1" style={{ fontSize: 7.5, letterSpacing: '.22em', color: 'rgba(255,255,255,.4)' }}>
            {killing ? 'SENT' : 'ELAPSED'}
          </span>
        </div>
        <div className="absolute inset-0 flex items-center justify-end gap-1" data-actions>
          <button className="act" aria-label="Logs" title="Logs" onClick={(e) => { e.stopPropagation(); onLogs(session); }}>
            <LogsIcon size={15} />
          </button>
          <button
            className="act act--reprompt"
            aria-label="Re-prompt" title="Re-prompt"
            data-emphasis={repromptEmphasis ? '' : undefined}
            onClick={(e) => { e.stopPropagation(); onReprompt(session); }}
          >
            <RepromptIcon size={15} />
          </button>
          <button className="act" aria-label="Focus pane" title="Focus pane" onClick={(e) => { e.stopPropagation(); onFocus(session.id); }}>
            <FocusIcon size={15} />
          </button>
          <button className="act" aria-label="Kill session" title="Kill" onClick={(e) => { e.stopPropagation(); onKill(session.id); }}>
            <KillIcon size={15} />
          </button>
        </div>
      </div>
    </div>
  );
});
