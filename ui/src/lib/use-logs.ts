// use-logs.ts — one poller for log tails; shared by the transcript and the multiplexer tails.
// Polls invoke('logs', {id, tail:200}) at 1s for the given sessions; caches last lines per id.
import { useEffect, useRef, useState } from 'react';
import { logs as fetchLogs } from './bridge';
import type { Session } from './bridge';

export interface LogsTick { at: number; lines: string[] }
export type LogsMap = Record<string, LogsTick>;

export function useLogsPoller(sessions: Session[], active: boolean, intervalMs = 1000): LogsMap {
  const [map, setMap] = useState<LogsMap>({});
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const timer = useRef(0);

  useEffect(() => {
    if (!active) return;
    const tick = () => {
      if (document.hidden) return;
      const targets = sessionsRef.current.filter((s) => s.status === 'working').slice(0, 12);
      if (targets.length === 0) return;
      void Promise.all(targets.map((s) => fetchLogs(s.id).then((p) => [s.id, p.lines] as const)))
        .then((entries) => {
          if (!entries.length) return;
          const at = Date.now();
          setMap((prev) => {
            const next = { ...prev };
            for (const [id, lines] of entries) next[id] = { at, lines };
            return next;
          });
        });
    };
    tick();
    timer.current = window.setInterval(tick, intervalMs);
    return () => window.clearInterval(timer.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, intervalMs]);

  return map;
}
