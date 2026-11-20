// TreeTab.tsx — supervisors · live tail · composer · inspector. The operator surface.
// Structure: a real three-pane workstation, not a stack of cards. Before the first snapshot
// the centre says only "Connecting to notchd…" — no invented state.
import { useEffect, useMemo, useRef, useState } from 'react';
import type { Snapshot, Supervisor, Session } from '../../lib/bridge';
import { SupervisorRow } from '../SupervisorRow';
import { Transcript } from './Transcript';
import { Composer, type ComposerTarget } from './Composer';
import { Inspector } from './Inspector';
import { SearchIcon, PlusIcon } from '../Icons';
import { useLogsPoller } from '../../lib/use-logs';
import { PixelSphere } from '../pixel/PixelSphere';
import { PixelMeter } from '../pixel/PixelMeter';

export type Selection = ComposerTarget;

export function TreeTab({ snapshot, selection, onSelect, onTogglePause, onKillSupervisor, onKillSession, onOpenHerdr, onFocusComposerRequest, composerPrefill, composerFocusSignal, flashId }: {
  snapshot: Snapshot | null;
  selection: Selection;
  onSelect: (sel: Selection) => void;
  onTogglePause: (sup: Supervisor) => void;
  onKillSupervisor: (id: string) => void;
  onKillSession: (id: string) => void;
  onOpenHerdr: (id: string) => void;
  onFocusComposerRequest: () => void;
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
  const revRef = useRef<Record<string, number>>({});
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

  const header = selectedSession
    ? { title: selectedSession.title || selectedSession.id,
        meta: `${selectedSession.id} · ${selectedSession.sup} · ${selectedSession.model?.split('/')[1] ?? 'auto'} · started ${new Date(selectedSession.startedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` }
    : selectedSupervisor
      ? { title: `${selectedSupervisor.id}${selectedSupervisor.paused ? ' — paused' : ''}`,
          meta: `${sessions.filter((s) => s.sup === selectedSupervisor.id).length} sessions · working ${selectedSupervisor.working} idle ${selectedSupervisor.idle} failed ${selectedSupervisor.failed}` }
      : { title: 'Orchestrator',
          meta: `${supervisors.length} supervisors · ${sessions.length} sessions${snapshot ? ` · ${snapshot.eps.toFixed(1)} ev/s` : ''}` };

  const status = selectedSession?.status ?? selectedSupervisor?.status ?? snapshot?.orchestrator.status ?? 'idle';
  const logs = useLogsShim(selectedSession, snapshot);
  const load = snapshot ? Math.min(1, snapshot.eps / 8) : 0;

  return (
    <div className="tree-grid">
      <aside className="sidebar" aria-label="Supervisors">
        <div className="sidebar-head">
          <span className="label">Supervisors</span>
          <span className="view-head-count">{String(supervisors.length).padStart(2, '0')}</span>
        </div>
        <div className="sidebar-body">
          <label className="sidebar-filter">
            <SearchIcon size={12} />
            <input className="sidebar-filter-input" placeholder="Filter id or status" value={filter}
              onChange={(e) => setFilter(e.target.value)} aria-label="Filter by id or status" spellCheck={false} />
          </label>
          <button className="orch-root" onClick={() => onSelect(null)} aria-pressed={selection === null}>
            <i className={`dot dot--${snapshot?.orchestrator.status ?? 'idle'}`} />
            <span className="tabular-nums">Orchestrator</span>
            <span className="orch-root-meta tabular-nums">{supervisors.length} sup</span>
          </button>
          <div className="sidebar-scroll">
            {matchedSessions.length > 0 && (
              <div className="sidebar-group">
                <span className="label">Matching sessions</span>
                {matchedSessions.map((s) => (
                  <button key={s.id} className="inspector-session-row" onClick={() => onSelect({ kind: 'session', id: s.id })}>
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
                    onMessage={() => { onSelect({ kind: 'supervisor', id: sv.id }); onFocusComposerRequest(); }}
                    onTogglePause={onTogglePause}
                    onKill={onKillSupervisor}
                    onSelect={(id) => onSelect({ kind: 'supervisor', id })} />
                </div>
              ))}
            </div>
            {matchedSupervisors.length === 0 && (
              <p className="sidebar-empty">{filter.trim() ? `Nothing matches “${filter.trim()}”.` : 'No supervisors yet — assign work below.'}</p>
            )}
          </div>
        </div>
        <div className="sidebar-actions">
          <button className="sidebar-btn" onClick={() => { onSelect(null); onFocusComposerRequest(); }}>
            <PlusIcon size={11} /><span>New supervisor</span>
          </button>
          <button className="sidebar-btn" onClick={() => { const sel = selection?.kind === 'supervisor' ? selection : selectedSession ? { kind: 'supervisor' as const, id: selectedSession.sup } : null; onSelect(sel); onFocusComposerRequest(); }}>
            <PlusIcon size={11} /><span>New session</span>
          </button>
        </div>
      </aside>

      <section className="center-pane">
        {snapshot === null ? (
          <div className="dash-connecting" role="status">
            <PixelSphere size={64} cell={3} live />
            <span className="label">Connecting to notchd…</span>
          </div>
        ) : (
          <>
            <header className="center-head">
              <div className="center-head-top">
                <div style={{ minWidth: 0 }}>
                  <h1 className="center-title">{header.title}</h1>
                  <div className="center-meta tabular-nums">{header.meta}</div>
                </div>
                <span className={`center-status center-status--${status}`}>
                  <i className={`dot dot--${status}`} />{status}
                </span>
              </div>
              <div className="center-dither">
                <PixelMeter value={load} cells={40} tone="hot" label="Event rate" />
              </div>
            </header>

            {sessions.length === 0 && supervisors.length === 0 ? (
              <div className="empty-state">
                <PixelSphere size={112} cell={4} live />
                <span className="empty-title">Nothing running</span>
                <p className="empty-line">
                  No supervisors, no sessions. Write the work below — Enter assigns it to a new supervisor.
                </p>
                <button className="esc-btn esc-btn--primary empty-action" onClick={onFocusComposerRequest}>
                  <PlusIcon size={11} /> Assign the first task
                </button>
              </div>
            ) : (
              <Transcript session={selectedSession} logs={logs} revisionChangedAt={revAtRef.current} />
            )}

            <Composer selection={selection} sessions={sessions} prefill={composerPrefill} focusSignal={composerFocusSignal} />
          </>
        )}
      </section>

      <Inspector
        selection={selection}
        snapshot={snapshot}
        onReprompt={(id) => { onSelect({ kind: 'session', id }); onFocusComposerRequest(); }}
        onTogglePause={onTogglePause}
        onKill={onKillSession}
        onOpenHerdr={onOpenHerdr}
        onFocusComposer={onFocusComposerRequest}
        onSelectSession={(id) => onSelect({ kind: 'session', id })}
      />
    </div>
  );
}

// Logs for the transcript — snapshot is the event source; 1 s poll only while working.
function useLogsShim(selectedSession: Session | null, snapshot: Snapshot | null) {
  const sessions = snapshot?.sessions ?? [];
  const scope = selectedSession ? sessions.filter((s) => s.id === selectedSession.id) : [];
  return useLogsPoller(scope, true, 1000);
}
