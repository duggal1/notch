// MultiplexerTab.tsx — responsive grid of live session cards; grid or flat list. Click → Tree + select.
import { useMemo, useState } from 'react';
import type { Snapshot } from '../../lib/bridge';
import { useLogsPoller } from '../../lib/use-logs';
import { GridIcon, ListIcon } from '../Icons';

export function MultiplexerTab({ snapshot, onJumpToTree }: {
  snapshot: Snapshot | null;
  onJumpToTree: (sessionId: string) => void;
}) {
  const [layout, setLayout] = useState<'grid' | 'list'>('grid');
  const sessions = snapshot?.sessions ?? [];
  const logs = useLogsPoller(sessions, true, 1000);
  const escalations = snapshot?.escalations ?? [];
  const escalatedIds = useMemo(() => new Set(escalations.map((e) => e.session)), [escalations]);

  const badge = (s: typeof sessions[number]) =>
    s.status === 'done' ? { text: 'Done', cls: 'badge--done' }
    : escalatedIds.has(s.id) && escalations.some((e) => e.session === s.id && e.kind === 'stuck') ? { text: 'Stuck', cls: 'badge--stuck' }
    : escalatedIds.has(s.id) ? { text: 'Escalated', cls: 'badge--escalated' }
    : null;

  return (
    <div className="multiplexer">
      <div className="multiplexer-bar">
        <span className="label">{sessions.length} sessions</span>
        <div className="layout-toggle" role="group" aria-label="Layout">
          <button className={layout === 'grid' ? 'toggle-btn is-active' : 'toggle-btn'} onClick={() => setLayout('grid')} aria-pressed={layout === 'grid'} aria-label="Grid layout">
            <GridIcon size={14} />
          </button>
          <button className={layout === 'list' ? 'toggle-btn is-active' : 'toggle-btn'} onClick={() => setLayout('list')} aria-pressed={layout === 'list'} aria-label="List layout">
            <ListIcon size={14} />
          </button>
        </div>
      </div>
      {sessions.length === 0 ? (
        <div className="multiplexer-empty"><span className="label">Nothing assigned</span></div>
      ) : (
        <div className={layout === 'grid' ? 'session-grid' : 'session-list'}>
          {sessions.map((s) => {
            const b = badge(s);
            const it = s.iterations ?? 0, max = s.maxIterations ?? 8;
            const tail = logs[s.id]?.lines?.slice(-3) ?? [];
            return (
              <div key={s.id} role="button" tabIndex={0} className={layout === 'grid' ? 'mux-card' : 'mux-row'} onClick={() => onJumpToTree(s.id)} onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onJumpToTree(s.id); } }} aria-label={`${s.id} — ${s.status} — ${s.title}`}>
                <div className="mux-head">
                  <i className={`dot dot--${s.status}`} />
                  <span className="mono mux-id">{s.id}</span>
                  {b && <span className={`badge ${b.cls}`}>{b.text}</span>}
                </div>
                <div className="mux-title">{s.title}</div>
                {s.goal && <div className="mux-goal">{s.goal}</div>}
                <div className="mux-foot">
                  <span className="mono" style={{ color: 'rgba(255,255,255,.44)' }}>Iteration {it}/{max}</span>
                  {s.lastSupervisorAction && <span className="mux-action">{s.lastSupervisorAction}</span>}
                </div>
                {tail.length > 0 && (
                  <pre className="mux-tail">{tail.join('\n')}</pre>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
