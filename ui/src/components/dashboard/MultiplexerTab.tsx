// MultiplexerTab.tsx — the session array. Two deliberate layouts:
// grid = equal-height instruments; list = one dense, scannable row per session.
import { useMemo, useState } from 'react';
import { send, type Snapshot, type Session } from '../../lib/bridge';
import { useLogsPoller } from '../../lib/use-logs';
import { PixelIcon } from '../PixelIcon';
import { DitherField } from '../pixel/DitherField';
import { PixelMeter } from '../pixel/PixelMeter';

type Layout = 'grid' | 'list';

function statusLabel(s: Session, escalated: Set<string>, escalations: { session: string; kind: string }[]) {
  if (s.status === 'done') return 'Done';
  if (escalated.has(s.id) && escalations.some((e) => e.session === s.id && e.kind === 'stuck')) return 'Stuck';
  if (escalated.has(s.id)) return 'Escalated';
  if (s.status === 'failed') return 'Failed';
  if (s.status === 'working') return 'Working';
  return 'Idle';
}

export function MultiplexerTab({ snapshot, onJumpToTree, onAssign }: {
  snapshot: Snapshot | null;
  onJumpToTree: (sessionId: string) => void;
  onAssign: () => void;
}) {
  const [layout, setLayout] = useState<Layout>('grid');
  const sessions = snapshot?.sessions ?? [];
  const logs = useLogsPoller(sessions, true, 1000);
  const escalations = snapshot?.escalations ?? [];
  const escalatedIds = useMemo(() => new Set(escalations.map((e) => e.session)), [escalations]);
  const counts = {
    working: sessions.filter((s) => s.status === 'working').length,
    done: sessions.filter((s) => s.status === 'done').length,
    failed: sessions.filter((s) => s.status === 'failed').length,
  };

  const open = (id: string) => { onJumpToTree(id); send({ cmd: 'focus', id }); };

  if (sessions.length === 0) {
    return (
      <section className="mux-root">
        <div className="mx-empty">
          <span className="title">Nothing assigned</span>
          <p className="empty-line">Sessions appear here the moment work is assigned.</p>
          <button className="esc-btn esc-btn--primary ripple" onClick={onAssign}>Open Tree</button>
        </div>
      </section>
    );
  }

  return (
    <section className="mux-root">
      <header className="mx-head">
        <div className="mx-count">
          <span className="num-l">{sessions.length}</span>
          <div className="mx-dist">
            <span className="label"><i className="dot dot--working" />{counts.working} working</span>
            <span className="label"><i className="dot dot--done" />{counts.done} done</span>
            <span className="label"><i className="dot dot--failed" />{counts.failed} failed</span>
          </div>
        </div>
        <div className="mx-toggle" role="group" aria-label="Layout">
          <button className={layout === 'grid' ? 'mx-toggle-btn is-active' : 'mx-toggle-btn'}
            aria-pressed={layout === 'grid'} aria-label="Grid layout" onClick={() => setLayout('grid')}>
            <PixelIcon name="grid" size={12} />
          </button>
          <button className={layout === 'list' ? 'mx-toggle-btn is-active' : 'mx-toggle-btn'}
            aria-pressed={layout === 'list'} aria-label="List layout" onClick={() => setLayout('list')}>
            <PixelIcon name="list" size={12} />
          </button>
        </div>
      </header>

      {layout === 'grid' ? (
        <div className="mx-grid">
          {sessions.map((s) => {
            const it = s.iterations ?? 0, max = s.maxIterations ?? 8;
            const tail = (logs[s.id]?.lines ?? []).slice(-3);
            return (
              <button key={s.id} className="mx-card" onClick={() => open(s.id)}
                aria-label={`${s.id} — ${s.status} — ${s.title}`}>
                <span className="mx-crown"><DitherField mode="linear" angle={0} cell={2}
                  floor={s.status === 'failed' ? 0.72 : s.status === 'working' ? 0.42 : 0.12} /></span>
                <span className="mx-card-body">
                  <span className="mx-meta">
                    <i className={`dot dot--${s.status}`} />
                    <span className="mx-id">{s.id}</span>
                    <span className="mx-status">{statusLabel(s, escalatedIds, escalations)}</span>
                  </span>
                  <span className="mx-title">{s.title}</span>
                  {s.goal && s.goal !== s.title && <span className="mx-goal">{s.goal}</span>}
                  <PixelMeter value={s.status === 'done' ? 1 : it / max} cells={12}
                    tone={s.status === 'failed' ? 'pale' : 'hot'} label={`Iteration ${it} of ${max}`} />
                  <span className="mx-foot">
                    <span className="label">Iter {it}/{max}</span>
                    <span className="mx-action">{s.lastSupervisorAction ?? '—'}</span>
                  </span>
                </span>
                <span className="mx-tail">{tail.length ? tail.join('\n') : 'no output yet'}</span>
              </button>
            );
          })}
        </div>
      ) : (
        <div className="mx-list">
          {sessions.map((s) => {
            const it = s.iterations ?? 0, max = s.maxIterations ?? 8;
            return (
              <button key={s.id} className="mx-row" onClick={() => open(s.id)}
                aria-label={`${s.id} — ${s.status} — ${s.title}`}>
                <span className={`mx-rail mx-rail--${s.status}`} />
                <span className="mx-row-main">
                  <span className="mx-meta"><i className={`dot dot--${s.status}`} /><span className="mx-id">{s.id}</span>
                    <span className="mx-status">{statusLabel(s, escalatedIds, escalations)}</span></span>
                  <span className="mx-title">{s.title}</span>
                </span>
                <span className="mx-row-side">
                  <PixelMeter value={s.status === 'done' ? 1 : it / max} cells={10}
                    tone={s.status === 'failed' ? 'pale' : 'hot'} label={`Iteration ${it} of ${max}`} />
                  <span className="label">Iter {it}/{max}</span>
                  <span className="mx-action">{s.lastSupervisorAction ?? '—'}</span>
                </span>
              </button>
            );
          })}
        </div>
      )}
    </section>
  );
}
