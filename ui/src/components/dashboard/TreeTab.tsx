// TreeTab.tsx — supervisors · live tail · composer · inspector. The operator surface.
// Structure: a real three-pane workstation, not a stack of cards. Before the first snapshot
// the centre says only "Connecting to notchd…" — no invented state.
import { useEffect, useMemo, useRef, useState } from 'react';
import type { Snapshot, Supervisor, Session } from '../../lib/bridge';
import { SupervisorRow } from '../SupervisorRow';
import { Transcript } from './Transcript';
import { Composer, type ComposerTarget } from './Composer';
import { Inspector } from './Inspector';
import { PixelIcon } from '../PixelIcon';
import { useLogsPoller } from '../../lib/use-logs';
import { PixelSphere } from '../pixel/PixelSphere';
import { PixelMeter } from '../pixel/PixelMeter';

export type Selection = ComposerTarget;

export function TreeTab({ snapshot, selection, onSelect, onTogglePause, onKillSupervisor, onKillSession, onOpenHerdr, onFocusComposerRequest, onPrefill, composerPrefill, composerFocusSignal, flashId }: {
  snapshot: Snapshot | null;
  selection: Selection;
  onSelect: (sel: Selection) => void;
  onTogglePause: (sup: Supervisor) => void;
  onKillSupervisor: (id: string) => void;
  onKillSession: (id: string) => void;
  onOpenHerdr: (id: string) => void;
  onFocusComposerRequest: () => void;
  onPrefill: (text: string) => void;
  composerPrefill: { text: string; at: number } | null;
  composerFocusSignal: number;
  flashId?: string | null;
}) {
  const [filter, setFilter] = useState('');
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  // revision clocks for the quiet marker — keyed by session id, updated when lastRevision moves
  const revRef = useRef<Record<string, number | null>>({});
  const revAtRef = useRef<Record<string, number>>({});
  useEffect(() => {
    if (!snapshot) return;
    for (const s of snapshot.sessions) {
      if (revRef.current[s.id] !== s.lastRevision) {
        revRef.current[s.id] = s.lastRevision;
        revAtRef.current[s.id] = Date.now();
      }
    }
  }, [snapshot]);

  const sessions = snapshot?.sessions ?? [];
  const supervisors = snapshot?.supervisors ?? [];
  const decisions = snapshot?.decisions;

  const { matchedSessions, matchedSupervisors } = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return { matchedSessions: [] as Session[], matchedSupervisors: supervisors };
    const match = (id: string, title: string, status: string) =>
      id.toLowerCase().includes(q) || title.toLowerCase().includes(q) || status.toLowerCase().includes(q);
    return {
      matchedSessions: sessions.filter((s) => match(s.id, s.title, s.status)),
      matchedSupervisors: supervisors.filter((sv) =>
        sv.id.toLowerCase().includes(q) || sv.status.toLowerCase().includes(q)
        || sessions.some((s) => s.sup === sv.id && match(s.id, s.title, s.status))),
    };
  }, [filter, sessions, supervisors]);

  const flashSupId = useMemo(() => {
    if (!flashId) return null;
    return sessions.find((s) => s.id === flashId)?.sup ?? null;
  }, [flashId, sessions]);
  const selectedSession = selection?.kind === 'session' ? sessions.find((s) => s.id === selection.id) ?? null : null;
  const selectedSupervisor = selection?.kind === 'supervisor' ? supervisors.find((s) => s.id === selection.id) ?? null : null;

  const idle = snapshot !== null && sessions.length === 0 && supervisors.length === 0;
  const hasTarget = Boolean(selectedSession || selectedSupervisor);
  const title = selectedSession?.title || selectedSession?.id || selectedSupervisor?.id || 'Orchestrator';
  const meta = selectedSession
    ? [selectedSession.id, selectedSession.sup, selectedSession.model, new Date(selectedSession.startedAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })].filter(Boolean).join(' · ')
    : selectedSupervisor?.paused ? 'Paused' : '';
  const status = selectedSession?.status ?? selectedSupervisor?.status ?? snapshot?.orchestrator.status ?? 'idle';
  const logs = useLogsShim(selectedSession, snapshot);
  const load = snapshot ? Math.min(1, snapshot.eps / 40) : 0;

  return (
    <div className={`tree-grid${hasTarget ? ' tree-grid--target' : ''}${idle ? ' tree-grid--idle' : ''}`}>
      <aside className="sidebar" aria-label="Supervisors">
        <div className="sidebar-head">
          <span className="label">Supervisors</span>
          {supervisors.length > 0 && <span className="view-head-count">{supervisors.length}</span>}
        </div>
        <div className="sidebar-body">
          {!idle && <label className="sidebar-filter">
            <PixelIcon name="search" size={12} />
            <input className="sidebar-filter-input" placeholder="Filter id or status" value={filter}
              onChange={(e) => setFilter(e.target.value)} aria-label="Filter by id or status" spellCheck={false} />
          </label>}
          <button className="orch-root" onClick={() => onSelect(null)} aria-pressed={selection === null}>
            <i className={`dot dot--${snapshot?.orchestrator.status ?? 'idle'}`} />
            <span className="tabular-nums">Orchestrator</span>

          </button>
          <div className="sidebar-scroll">
            {matchedSessions.length > 0 && (
              <div className="sidebar-group">
                <span className="label">Matching sessions</span>
                {matchedSessions.map((s) => (
                  <button key={s.id} className="inspector-session-row ripple" onClick={() => { onSelect({ kind: 'session', id: s.id }); onOpenHerdr(s.id); }}>
                    <i className={`dot dot--${s.status}`} />
                    <span className="tabular-nums">{s.id}</span>
                    <span className="truncate">{s.title}</span>
                  </button>
                ))}
              </div>
            )}
            <div className="sidebar-group">
              {matchedSupervisors.map((sv) => (
                <div key={sv.id} className={flashSupId === sv.id ? 'sup-flash' : undefined}>
                  <SupervisorRow sup={sv} sessions={sessions} decisions={decisions} now={now}
                    variant="dashboard"
                    selected={selectedSupervisor?.id === sv.id}
                    onMessage={() => { onSelect({ kind: 'supervisor', id: sv.id }); onFocusComposerRequest(); }}
                    onTogglePause={onTogglePause}
                    onKill={onKillSupervisor}
                    onSelect={(id) => {
                      // Plain click = select + focus: selection is set synchronously before the
                      // fire-and-forget send; herdr focus is idempotent and opens no window.
                      onSelect({ kind: 'supervisor', id });
                      const target = sessions.find((s) => s.sup === id && s.status === 'working')
                        ?? sessions.find((s) => s.sup === id);
                      if (target) onOpenHerdr(target.id);
                    }} />
                  <div className="spine-sessions">
                    {sessions.filter((session) => session.sup === sv.id).map((session) => (
                      <button key={session.id} className="spine-session" aria-pressed={selectedSession?.id === session.id}
                        title={session.title} onClick={() => onSelect({ kind: 'session', id: session.id })}>
                        <i className={`dot dot--${session.status}`} />
                        <span className="truncate">{session.title || session.id}</span>
                      </button>
                    ))}
                  </div>
                </div>
              ))}
            </div>
            {filter.trim() && matchedSupervisors.length === 0 && (
              <p className="sidebar-empty">{`Nothing matches “${filter.trim()}”.`}</p>
            )}
          </div>
        </div>
        <div className="sidebar-actions">
          <button className="sidebar-btn ripple" onClick={() => { onSelect(null); onFocusComposerRequest(); }}>
            <PixelIcon name="plus" size={12} /><span>New supervisor</span>
          </button>
          <button className="sidebar-btn" disabled={supervisors.length === 0} onClick={() => {
            const id = selectedSupervisor?.id ?? selectedSession?.sup ?? supervisors[0]?.id;
            if (id) { onSelect({ kind: 'supervisor', id }); onFocusComposerRequest(); }
          }}>
            <PixelIcon name="plus" size={12} /><span>New session</span>
          </button>
        </div>
      </aside>

      <section className={`center-pane${idle ? ' center-pane--idle' : ''}`}>
        {snapshot === null ? (
          <div className="dash-connecting" role="status">
            <PixelSphere size={64} cell={3} live />
            <span className="label">Connecting to notchd…</span>
          </div>
        ) : (
          <div className={idle ? 'tree-start' : 'tree-workspace'}>
            {idle ? (
              <>
                <div className="tree-start-heading">
                  <span className="tree-start-mark brand-logo" aria-hidden="true" />
                  <h2 className="title">Assign the first task</h2>
                </div>
                <div className="tree-start-choices">
                  <span className="label">Start from</span>
                  <div className="empty-templates">
                    {[
                      { label: 'Fix a test', task: 'Fix the failing test in ui/ and run the suite' },
                      { label: 'Review a change', task: 'Review the current diff and report correctness issues' },
                      { label: 'Write docs', task: 'Write the missing bridge section of the README' },
                    ].map(({ label, task }) => (
                      <button key={label} className="template-pill" onClick={() => onPrefill(task)}>{label}</button>
                    ))}
                  </div>
                </div>
              </>
            ) : (
              <>
                <header className="center-head">
                  <div className="center-head-top">
                    <div className="center-heading">
                      <h2 className="center-title">{title}</h2>
                      {meta && <p className="center-meta">{meta}</p>}
                    </div>
                    <span className={`center-status center-status--${status}`}>
                      <i className={`dot dot--${status}`} />{status[0]?.toUpperCase()}{status.slice(1)}
                    </span>
                  </div>
                  {load > 0 && status === 'working' && (
                    <div className="center-dither"><PixelMeter value={load} cells={32} tone="hot" label="Event rate" /></div>
                  )}
                </header>
                <Transcript session={selectedSession} logs={logs} revisionChangedAt={revAtRef.current} />
              </>
            )}
            <Composer selection={selection} sessions={sessions} prefill={composerPrefill} focusSignal={composerFocusSignal} />
          </div>
        )}
      </section>

      {hasTarget && <Inspector
        selection={selection}
        snapshot={snapshot}
        onReprompt={(id) => { onSelect({ kind: 'session', id }); onFocusComposerRequest(); }}
        onTogglePause={onTogglePause}
        onKill={onKillSession}
        onOpenHerdr={onOpenHerdr}
        onFocusComposer={onFocusComposerRequest}
        onSelectSession={(id) => onSelect({ kind: 'session', id })}
      />}
    </div>
  );
}

// Logs for the transcript — snapshot is the event source; 1 s poll only while working.
function useLogsShim(selectedSession: Session | null, snapshot: Snapshot | null) {
  const sessions = snapshot?.sessions ?? [];
  const scope = selectedSession ? sessions.filter((s) => s.id === selectedSession.id) : [];
  return useLogsPoller(scope, true, 1000);
}
