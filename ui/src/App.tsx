// App.tsx — the popover. Static shell: rest → peek → open.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Shell } from './components/Shell';
import { Panel } from './components/Panel';
import { LogsPanel } from './components/LogsPanel';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { send, logs as fetchLogs, isTauri } from './lib/bridge';
import type { Session, LogsPayload } from './lib/bridge';
import { topStatus, workingCount, normalizeEps } from './lib/format';
import { useSnapshot } from './lib/use-snapshot';
import { useRipple } from './lib/use-ripple';
import { frameFor, type Mode } from './lib/shell-geometry';
import type { Status } from './lib/tokens';

export default function App() {
  const snapshot = useSnapshot();
  const [mode, setMode] = useState<Mode>('rest');
  const [killingIds, setKillingIds] = useState<Set<string>>(new Set());
  const [logsPayload, setLogsPayload] = useState<LogsPayload | null>(null);
  const [logsOpen, setLogsOpen] = useState(false);
  const [sweepSent, setSweepSent] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const shellRef = useRef<HTMLDivElement>(null);
  const modeRef = useRef<Mode>('rest');
  const intentTimer = useRef(0);

  const sessions = useMemo(() => snapshot?.sessions ?? [], [snapshot]);
  const supervisors = useMemo(() => snapshot?.supervisors ?? [], [snapshot]);
  const idle = sessions.length === 0;
  const contentH = idle ? 320 : 640;
  const eps = snapshot?.eps ?? 0;
  const epsRef = useRef(eps);
  epsRef.current = eps;

  const top = useMemo(() => (snapshot ? topStatus(sessions) : 'idle'), [snapshot, sessions]);
  const wcount = useMemo(() => workingCount(sessions), [sessions]);
  const load = normalizeEps(eps);
  const booted = snapshot !== null;
  const escalations = snapshot?.escalations;
  const decisions = snapshot?.decisions;

  // intent machine: peek 60ms on hover, rest 240ms on leave, snap-open on click
  const go = useCallback((m: Mode, ms: number) => {
    window.clearTimeout(intentTimer.current);
    intentTimer.current = window.setTimeout(() => {
      modeRef.current = m;
      setMode(m);
      send({ cmd: 'set_frame', mode: m, contentH });
      if (m === 'open') send({ cmd: 'haptic', kind: 'alignment' });
    }, ms);
  }, [contentH]);
  const onEnter = useCallback(() => { if (modeRef.current === 'rest') go('peek', 60); }, [go]);
  const onLeave = useCallback(() => { if (modeRef.current !== 'open') go('rest', 240); }, [go]);
  const onTap = useCallback(() => { if (modeRef.current !== 'open') go('open', 0); }, [go]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (logsOpen) { setLogsOpen(false); return; }
      if (modeRef.current !== 'rest') go('rest', 0);
    };
    const onBlur = () => { if (modeRef.current !== 'rest') go('rest', 0); };
    window.addEventListener('keydown', onKey);
    window.addEventListener('blur', onBlur);
    return () => { window.removeEventListener('keydown', onKey); window.removeEventListener('blur', onBlur); };
  }, [go, logsOpen]);

  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    let alive = true;
    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused && modeRef.current !== 'rest') go('rest', 0);
    }).then((fn) => { if (alive) off = fn; else fn(); }).catch(() => undefined);
    return () => { alive = false; off?.(); };
  }, [go]);

  useEffect(() => {
    if (!toast) return;
    const t = window.setTimeout(() => setToast(null), 2800);
    return () => window.clearTimeout(t);
  }, [toast]);

  // ---------- actions ----------
  const kill = useCallback((id: string) => {
    send({ cmd: 'kill', id });
    setKillingIds((s) => new Set(s).add(id));
    window.setTimeout(() => setKillingIds((s) => { const n = new Set(s); n.delete(id); return n; }), 3000);
  }, []);
  const reprompt = useCallback((s: Session) => send({ cmd: 'reprompt', id: s.id, text: 'resume' }), []);
  const focus = useCallback((id: string) => send({ cmd: 'focus', id }), []);
  const cleanup = useCallback(() => {
    send({ cmd: 'cleanup' });
    setSweepSent(true);
    window.setTimeout(() => setSweepSent(false), 1200);
  }, []);

  const invokeOpenDashboard = useCallback(async () => {
    try {
      if (isTauri()) await invoke('open_dashboard');
      else send({ cmd: 'open_dashboard' });
      go('rest', 0);
    } catch (e) {
      console.error('[notch] open_dashboard failed', e);
      setToast('Could not open the dashboard');
    }
  }, [go]);
  const openDashboardTarget = useCallback((kind: 'supervisor' | 'session', id: string) => {
    try { localStorage.setItem('notch.target', JSON.stringify({ kind, id })); } catch { /* storage may be unavailable */ }
    void invokeOpenDashboard();
  }, [invokeOpenDashboard]);
  const openDashboard = useCallback(() => {
    try { localStorage.setItem('notch.target', JSON.stringify({ kind: 'none' })); } catch { /* storage may be unavailable */ }
    void invokeOpenDashboard();
  }, [invokeOpenDashboard]);

  const logs = useCallback((s: Session) => {
    fetchLogs(s.id).then((p) => { setLogsPayload(p); setLogsOpen(true); });
  }, []);
  const onTogglePause = useCallback((sup: { id: string; paused?: boolean }) =>
    send({ cmd: sup.paused ? 'resume_supervisor' : 'pause_supervisor', id: sup.id }), []);
  const onKillSupervisor = useCallback((supId: string) => {
    for (const s of sessions) if (s.sup === supId) send({ cmd: 'kill', id: s.id });
  }, [sessions]);

  useRipple(shellRef);

  const open = mode === 'open';
  const rest = frameFor('rest', contentH);

  return (
    <Shell mode={mode} shellRef={shellRef} onEnter={onEnter} onLeave={onLeave} onTap={onTap}>
      {!open && (
        <div className="rest-indicators" style={{ width: rest.w }} aria-hidden="true">
          <span className="rest-brand"><span className="brand-logo" /><i className={`live-dot live-dot--${top}`} aria-hidden="true" /></span>
        </div>
      )}

      <Panel
        open={open}
        top={top}
        wcount={wcount}
        eps={eps}
        epsRef={epsRef}
        load={load}
        booted={booted}
        sessions={sessions}
        supervisors={supervisors}
        decisions={decisions}
        escalations={escalations}
        killingIds={killingIds}
        sweepSent={sweepSent}
        logs={logs}
        reprompt={reprompt}
        focus={focus}
        kill={kill}
        onCleanup={cleanup}
        openDashboard={openDashboard}
        onMessageSupervisor={(id) => openDashboardTarget('supervisor', id)}
        onTogglePause={onTogglePause}
        onKillSupervisor={onKillSupervisor}
        onOpenTarget={openDashboardTarget}
      />

      <LogsPanel open={logsOpen} payload={logsPayload} onClose={() => setLogsOpen(false)} />
      {toast && <div className="toast" role="status">{toast}</div>}
    </Shell>
  );
}
