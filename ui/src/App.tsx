// App.tsx — the popover. Silhouette morph only: rest → peek → open. No glow anywhere.
// Open = flat dark cards over the libraries.dev dither (load = height, status = palette) + a DistortionWave on status flips.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Shell } from './components/Shell';
import { Panel } from './components/Panel';
import { LogsPanel } from './components/LogsPanel';
import { VoiceBeam, type VoiceBeamColorVariant } from './vendor/voice-glow/src';
import type { DistortionWaveHandle } from './vendor/distortion-waves/src';
import { send, logs as fetchLogs } from './lib/bridge';
import type { Session, LogsPayload } from './lib/bridge';
import { topStatus, workingCount, normalizeEps, pad2, fmtEps } from './lib/format';
import { useSnapshot } from './lib/use-snapshot';
import { useWhiteHot } from './lib/hot';
import { reduced } from './lib/motion';
import { frameFor, type Mode } from './lib/shell-geometry';
import type { Status } from './lib/tokens';

const BEAM: Record<Status, VoiceBeamColorVariant> = { working: 'colorful', idle: 'mono', failed: 'sunset', done: 'ice' };

export default function App() {
  const snapshot = useSnapshot();
  const [mode, setMode] = useState<Mode>('rest');
  const [killingIds, setKillingIds] = useState<Set<string>>(new Set());
  const [logsPayload, setLogsPayload] = useState<LogsPayload | null>(null);
  const [logsOpen, setLogsOpen] = useState(false);
  const [sweepSent, setSweepSent] = useState(false);

  const shellRef = useRef<HTMLDivElement>(null);
  const waveRef = useRef<DistortionWaveHandle>(null);
  const epsRef = useRef(0);
  const levelRef = useRef(0);
  const modeRef = useRef<Mode>('rest');
  const intentTimer = useRef(0);
  const prevTopRef = useRef<Status | null>(null);

  const sessions = useMemo(() => snapshot?.sessions ?? [], [snapshot]);
  const supervisors = useMemo(() => snapshot?.supervisors ?? [], [snapshot]);
  const eps = snapshot?.eps ?? 0;
  useEffect(() => {
    epsRef.current = eps;
    levelRef.current = Math.sqrt(normalizeEps(eps)); // sqrt: low event rates still lift the dither visibly
  }, [eps]);
  const getLevel = useCallback(() => levelRef.current, []);

  const top = useMemo(() => (snapshot ? topStatus(sessions) : 'idle'), [snapshot, sessions]);
  const wcount = useMemo(() => workingCount(sessions), [sessions]);
  const load = normalizeEps(eps);
  const booted = snapshot !== null;
  const escalations = snapshot?.escalations;
  const decisions = snapshot?.decisions;

  // top status flip → haptic + distortion wave across the state cards (never on first paint)
  useEffect(() => {
    if (!snapshot) return;
    const prev = prevTopRef.current;
    if (prev === top) return;
    prevTopRef.current = top;
    if (prev === null) return;
    send({ cmd: 'haptic', kind: top === 'failed' ? 'levelChange' : 'alignment' });
    if (!reduced && top !== 'idle') waveRef.current?.play();
  }, [top, snapshot]);

  // ---------- intent machine ----------
  const go = useCallback((m: Mode, ms: number) => {
    window.clearTimeout(intentTimer.current);
    intentTimer.current = window.setTimeout(() => {
      modeRef.current = m;
      setMode(m);
      send({ cmd: 'set_frame', mode: m });
      if (m === 'open') send({ cmd: 'haptic', kind: 'alignment' });
    }, ms);
  }, []);
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

  const openDashboardTarget = useCallback((kind: 'supervisor' | 'session', id: string) => {
    try { localStorage.setItem('notch.target', JSON.stringify({ kind, id })); } catch { /* storage may be unavailable */ }
    send({ cmd: 'open_dashboard' });
  }, []);
  const openDashboard = useCallback(() => {
    try { localStorage.setItem('notch.target', JSON.stringify({ kind: 'none' })); } catch { /* storage may be unavailable */ }
    send({ cmd: 'open_dashboard' });
  }, []);

  const logs = useCallback((s: Session) => {
    fetchLogs(s.id).then((p) => { setLogsPayload(p); setLogsOpen(true); });
  }, []);
  const onTogglePause = useCallback((sup: { id: string; paused?: boolean }) =>
    send({ cmd: sup.paused ? 'resume_supervisor' : 'pause_supervisor', id: sup.id }), []);
  const onKillSupervisor = useCallback((supId: string) => {
    for (const s of sessions) if (s.sup === supId) send({ cmd: 'kill', id: s.id });
  }, [sessions]);

  useWhiteHot(shellRef);

  const open = mode === 'open';
  const rest = frameFor('rest');
  const peek = frameFor('peek');

  return (
    <Shell mode={mode} shellRef={shellRef} onEnter={onEnter} onLeave={onLeave} onTap={onTap}>
      {/* rest wings */}
      <div className="wings pointer-events-none absolute top-0 flex items-center justify-between"
        style={{ left: '50%', transform: 'translateX(-50%)', width: rest.w, height: rest.h }} aria-hidden={open}>
        <div className="wing"><span className="wing-num">{pad2(wcount)}</span></div>
        <div className="wing"><span className="wing-num">{fmtEps(Math.min(eps, 99.9))}</span></div>
      </div>
      {/* peek */}
      <div className="peek-center" aria-hidden="true"
        style={{ left: '50%', transform: 'translateX(-50%)', width: peek.w, top: peek.h - 44 }}>
        <span className="peek-num num">{pad2(wcount)}</span>
        <span className="label">In flight</span>
      </div>

      {/* open: the dither is the load instrument; the panel sits above it */}
      <VoiceBeam
        type="mobile"
        look="dither"
        theme="dark"
        colorVariant={BEAM[top]}
        level={getLevel}
        active={open}
        idle={top === 'idle' ? 0.04 : 0.12}
        strength={top === 'idle' ? 0.55 : 1}
        borderRadius={36}
        aria-hidden={!open}
        style={{ position: 'absolute', inset: 0 }}
      >
        <Panel
          open={open}
          top={top}
          wcount={wcount}
          eps={eps}
          load={load}
          booted={booted}
          sessions={sessions}
          supervisors={supervisors}
          decisions={decisions}
          escalations={escalations}
          killingIds={killingIds}
          sweepSent={sweepSent}
          epsRef={epsRef}
          waveRef={waveRef}
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
      </VoiceBeam>

      <LogsPanel open={logsOpen} payload={logsPayload} onClose={() => setLogsOpen(false)} />
    </Shell>
  );
}