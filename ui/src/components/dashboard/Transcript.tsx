// Transcript.tsx — live log tail for the selected working session. Mono 12px, quiet marker at 90s silence.
import { useEffect, useMemo, useRef, useState } from 'react';
import type { Session } from '../../lib/bridge';
import type { LogsMap } from '../../lib/use-logs';

const TOOL_RE = /apply_patch|bash|read|write|edit/i;

export function Transcript({ session, logs, revisionChangedAt }: {
  session: Session | null;
  logs: LogsMap;
  revisionChangedAt: Record<string, number>;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const lines = session ? (logs[session.id]?.lines ?? []) : [];
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight; // pin the tail
  }, [lines.length, session?.id]);

  const quiet = useMemo(() => {
    if (!session || session.status !== 'working') return false;
    const at = revisionChangedAt[session.id];
    return at !== undefined && now - at >= 90_000;
  }, [session, revisionChangedAt, now]);

  if (!session) {
    return (
      <div ref={scrollRef} className="transcript">
        <p className="transcript-empty">Select a session — its live tail lands here.</p>
      </div>
    );
  }
  return (
    <div ref={scrollRef} className="transcript" role="log" aria-label={`Log tail for ${session.id}`}>
      <div className="transcript-head">
        <span className="label">Transcript — {session.id}</span>
        <span className="mono text-[10.5px]" style={{ color: 'rgba(255,255,255,.34)' }}>tail 200</span>
      </div>
      {lines.length === 0 ? (
        <p className="transcript-empty">No output yet — the session starts silent.</p>
      ) : (
        lines.map((line, i) => (
          <div key={i} className="transcript-line">
            {TOOL_RE.test(line) && <span className="tool-mark" aria-hidden="true">›</span>}
            <span>{line}</span>
          </div>
        ))
      )}
      {quiet && (
        <div className="transcript-quiet">session is quiet (90s+) — no revision change observed</div>
      )}
    </div>
  );
}
