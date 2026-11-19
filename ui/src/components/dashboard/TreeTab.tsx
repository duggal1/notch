// TreeTab.tsx — sidebar + center (header / transcript / composer) + inspector. The operator surface.
import { useEffect, useMemo, useRef, useState } from 'react';
import type { Snapshot, Supervisor, Session } from '../../lib/bridge';
import { topStatus, pad2 } from '../../lib/format';
import { SupervisorRow } from '../SupervisorRow';
import { Transcript } from './Transcript';
import { Composer, type ComposerTarget } from './Composer';
import { Inspector } from './Inspector';
import { SearchIcon, PlusIcon } from '../Icons';
import { useLogsPoller } from '../../lib/use-logs';

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
    if (!q) return { matchedSessions: [], matchedSupervisors: supervisors };
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

  return (
    <div className="tree-grid">
      {/* left sidebar */}
      <aside className="sidebar" aria-label="Supervisors">
        <label className="sidebar-filter">
          <SearchIcon size={13} />
          <input className="sidebar-filter-input" placeholder="Filter by id or status…" value={filter}
            onChange={(e) => setFilter(e.target.value)} aria-label="Filter by id or status" />
        </label>
        <button className="orch-root" onClick={() => onSelect(null)} aria-pressed={selection === null}>
          <i className={`dot dot--${snapshot?.orchestrator.status ?? 'idle'}`} />
          <span className="mono">Orchestrator</span>
          <span className="orch-root-meta">{supervisors.length} supervisors</span>
        </button>
        <div className="sidebar-scroll">
          {matchedSessions.length > 0 && (
            <div className="sidebar-group">
              <span className="label">Matching sessions</span>
              {matchedSessions.map((s) => (
                <button key={s.id} className="inspector-session-row" onClick={() => onSelect({ kind: 'session', id: s.id })}>
                  <i className={`dot dot--${s.status}`} />
                  <span className="mono">{s.id}</span>
                  <span className="truncate">{s.title}</span>
                </button>
              ))}
            </div>
          )}
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
          {matchedSupervisors.length === 0 && <p className="sidebar-empty">{filter.trim() ? `Nothing matches “${filter.trim()}”.` : 'No supervisors yet — assign work below.'}</p>}
        </div>
        <div className="sidebar-actions">
          <button className="sidebar-btn" onClick={() => { onSelect(null); onFocusComposerRequest(); }}>
            <PlusIcon size={12} /><span>New supervisor</span>
          </button>
          <button className="sidebar-btn" onClick={() => { const sel = selection?.kind === 'supervisor' ? selection : selectedSession ? { kind: 'supervisor' as const, id: selectedSession.sup } : null; onSelect(sel); onFocusComposerRequest(); }}>
            <PlusIcon size={12} /><span>New session</span>
          </button>
        </div>
      </aside>

      {/* center: header + transcript + composer */}
      <section className="center-pane">
        <header className="center-head">
          <h1 className="center-title">{header.title}</h1>
          <div className="center-meta">{header.meta}</div>
        </header>
        <Transcript session={selectedSession} logs={useLogsShim(selectedSession, snapshot)} revisionChangedAt={revAtRef.current} />
        <Composer
          selection={selection}
          sessions={sessions}
          prefill={composerPrefill}
          focusSignal={composerFocusSignal}
        />
      </section>

      {/* right inspector */}
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

// logs for the transcript — polls the selected session at 1s
function useLogsShim(selectedSession: Session | null, snapshot: Snapshot | null) {
  const sessions = snapshot?.sessions ?? [];
  const scope = selectedSession ? sessions.filter((s) => s.id === selectedSession.id) : [];
  return useLogsPoller(scope, true, 1000);
}
