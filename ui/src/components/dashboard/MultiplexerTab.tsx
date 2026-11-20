// MultiplexerTab.tsx — the session array. Grid or flat list; every card is a live tile with
// a dithered crown, an iteration meter and the last three log lines. Click → Tree + select.
import { useMemo, useState } from 'react';
import { send, type Snapshot } from '../../lib/bridge';
import { useLogsPoller } from '../../lib/use-logs';
import { PixelIcon } from '../PixelIcon';
import { DitherField } from '../pixel/DitherField';
import { PixelMeter } from '../pixel/PixelMeter';

export function MultiplexerTab({ snapshot, onJumpToTree, onAssign }: {
  snapshot: Snapshot | null;
  onJumpToTree: (sessionId: string) => void;
  onAssign: () => void;
}) {
  const [layout, setLayout] = useState<'grid' | 'list'>('grid');
  const sessions = snapshot?.sessions ?? [];
  const logs = useLogsPoller(sessions, true, 1000);
  const escalations = snapshot?.escalations ?? [];
  const escalatedIds = useMemo(() => new Set(escalations.map((e) => e.session)), [escalations]);
  const working = sessions.filter((s) => s.status === 'working').length;
  const done = sessions.filter((s) => s.status === 'done').length;
  const failed = sessions.filter((s) => s.status === 'failed').length;

  const badge = (s: typeof sessions[number]) =>
    s.status === 'done' ? { text: 'Done', cls: 'badge--done' }
    : escalatedIds.has(s.id) && escalations.some((e) => e.session === s.id && e.kind === 'stuck') ? { text: 'Stuck', cls: 'badge--stuck' }
    : escalatedIds.has(s.id) ? { text: 'Escalated', cls: 'badge--escalated' }
    : null;

  return (
    <div className="mux">
      {sessions.length > 0 && <div className="mux-bar">
        <div className="mux-totals" aria-label="Session totals">
          <span className="num-l">{sessions.length}</span>
          <div className="mux-distribution">
            {[[working, 'working'], [done, 'done'], [failed, 'failed']].filter(([count]) => Number(count) > 0).map(([count, status]) => (
              <span className="label" key={status}><i className={`dot dot--${status}`} />{count} {status}</span>
            ))}
          </div>
        </div>
        <div className="layout-toggle" role="group" aria-label="Layout">
          <button className={layout === 'grid' ? 'toggle-btn is-active' : 'toggle-btn'} onClick={() => setLayout('grid')} aria-pressed={layout === 'grid'} aria-label="Grid layout">
            <PixelIcon name="grid" size={12} />
          </button>
          <button className={layout === 'list' ? 'toggle-btn is-active' : 'toggle-btn'} onClick={() => setLayout('list')} aria-pressed={layout === 'list'} aria-label="List layout">
            <PixelIcon name="list" size={12} />
          </button>
        </div>
      </div>}

      {sessions.length === 0 ? (
        <div className="empty-state">
          <span className="title">Nothing assigned</span>
          <p className="empty-line">
            Assigned sessions appear here with their latest output.
          </p>
          <button className="esc-btn esc-btn--primary ripple" onClick={onAssign}>Open Tree</button>
        </div>
      ) : (
        <div className={layout === 'grid' ? 'session-grid' : 'session-list'}>
          {sessions.map((s, i) => {
            const b = badge(s);
            const it = s.iterations ?? 0, max = s.maxIterations ?? 8;
            const tail = logs[s.id]?.lines?.slice(-3) ?? [];
            const sub = s.goal && s.goal !== s.title ? s.goal : null;
            const card = (
              <>
                <div className="mux-crown"><DitherField mode="linear" angle={0} cell={2} floor={s.status === 'failed' ? 0.72 : s.status === 'working' ? 0.42 : 0.12} /></div>
                <div className="mux-card-body">
                  <div className="mux-head">
                    <i className={`dot dot--${s.status}`} />
                    <span className="mux-id">{s.id}</span>
                    {b && <span className={`badge ${b.cls}`}>{b.text}</span>}
                  </div>
                  <div className="mux-title">{s.title}</div>
                  {sub && <div className="mux-goal">{sub}</div>}
                  <PixelMeter value={s.status === 'done' ? 1 : it / max} cells={12} tone={s.status === 'failed' ? 'pale' : 'hot'} label={`Iteration ${it} of ${max}`} />
                  <div className="mux-foot">
                    <span className="label">Iter {it}/{max}</span>
                    {s.lastSupervisorAction && <span className="mux-action">{s.lastSupervisorAction}</span>}
                  </div>
                </div>
                {tail.length > 0 ? (
                  <pre className="mux-tail">{tail.join('\n')}</pre>
                ) : (
                  <div className="mux-tail mux-tail--empty"><span>no output yet</span></div>
                )}
              </>
            );
            return (
              <div
                key={s.id}
                role="button"
                data-ripple
                tabIndex={0}
                style={{ animationDelay: `${Math.min(i, 8) * 32}ms` }}
                className={layout === 'grid' ? 'mux-card' : 'mux-row'}
                onClick={() => { onJumpToTree(s.id); send({ cmd: 'focus', id: s.id }); }}
                onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onJumpToTree(s.id); send({ cmd: 'focus', id: s.id }); } }}
                aria-label={`${s.id} — ${s.status} — ${s.title}`}
              >
                {card}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
