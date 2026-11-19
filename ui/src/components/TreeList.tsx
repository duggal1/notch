// TreeList.tsx — id-ordered session rows, flush to the section edge (same 16px inset as every label).
import { useEffect, useRef, useState } from 'react';
import type { Session } from '../lib/bridge';
import { SessionRow } from './WhiteHotRow';
import { useWhiteHot } from '../lib/hot';

export function TreeList({ sessions, killingIds, onLogs, onReprompt, onFocus, onKill }: {
  sessions: Session[];
  killingIds: Set<string>;
  onLogs: (s: Session) => void;
  onReprompt: (s: Session) => void;
  onFocus: (id: string) => void;
  onKill: (id: string) => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  useWhiteHot(rootRef);

  return (
    <div ref={rootRef} className="list-scroll tree-rows" role="table" aria-label="Sessions">
      {sessions.map((s) => (
        <SessionRow key={s.id} session={s} now={now} killing={killingIds.has(s.id)}
          onLogs={onLogs} onReprompt={onReprompt} onFocus={onFocus} onKill={onKill} />
      ))}
    </div>
  );
}