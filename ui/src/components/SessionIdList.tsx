// SessionIdList.tsx — the popover sessions tile: real IDs with state dots, hairline rows.
// Click → open dashboard to that session (selection lands dashboard-side; we store intent).
import { memo } from 'react';
import type { Session } from '../lib/bridge';

export const SessionIdList = memo(function SessionIdList({ sessions, onOpen }: {
  sessions: Session[];
  onOpen: (id: string) => void;
}) {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto px-1.5 py-1" role="list" aria-label="Sessions">
      {sessions.map((s) => (
        <button key={s.id}
          className="item item--id flex shrink-0 items-center gap-2 px-2 text-left"
         
          onClick={() => onOpen(s.id)}
          aria-label={`Session ${s.id} — ${s.status} — ${s.title}`}>
          <i className={`dot dot--${s.status}`} aria-hidden="true" />
          <span className="mono shrink-0 text-[10.5px]" style={{ color: 'rgba(255,255,255,.82)' }}>{s.id}</span>
          <span className="min-w-0 flex-1 truncate text-[10.5px]" style={{ color: 'rgba(255,255,255,.58)' }}>{s.title}</span>
          <span className="mono shrink-0 text-[10.5px]" style={{ color: 'rgba(255,255,255,.36)' }}>{s.status}</span>
        </button>
      ))}
    </div>
  );
});
