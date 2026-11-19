// App.tsx — the popover. One snapshot in, the whole tree out. Mode machine: rest → peek → open.
import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { Shell } from './components/Shell';
import { Orb } from './components/GlowOrb';
import { NumberRoll } from './components/NumberRoll';
import { HeartbeatField } from './components/HeartbeatField';
import { SessionGrid } from './components/SessionGrid';
import { Radar } from './components/Radar';
import { Drift } from './components/Drift';
import { FloatingIds } from './components/FloatingIds';
import { TreeList } from './components/TreeList';
import { EmptyState } from './components/EmptyState';
import { LogsPanel } from './components/LogsPanel';
import { Specular } from './components/Specular';
import { OrchestratorMark } from './components/Icons';
import { subscribe, send } from './lib/bridge';
import type { Snapshot, Session, LogsPayload } from './lib/bridge';
import { topStatus, workingCount, normalizeEps, pad2, fmtEps } from './lib/format';
import { playWave } from './lib/wave';
import { reduced } from './lib/motion';
import { PAL } from './lib/tokens';
import type { Mode } from './lib/shell-geometry';

export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [mode, setMode] = useState<Mode>('rest');
  const [killingIds, setKillingIds] = useState<Set<string>>(new Set());
  const [logsPayload, setLogsPayload] = useState<LogsPayload | null>(null);
  const [logsOpen, setLogsOpen] = useState(false);
  const [sweepSent, setSweepSent] = useState(false);

  const shellRef = useRef<HTMLDivElement>(null);
  const zoneOneRef = useRef<HTMLElement>(null);
  const epsRef = useRef(0);
  const modeRef = useRef<Mode>('rest');
  const intentTimer = useRef(0);
  const prevTopRef = useRef<ReturnType<typeof topStatus> | null>(null);

  // ---------- data ----------
  useEffect(() => {
    let alive = true;
    const unsub = subscribe({
      onSnapshot: (s) => {
        if (!alive) return;
        setSnapshot(s);
        epsRef.current = s.eps;
      },
      onLogs: (p) => {
        if (!alive) return;
        setLogsPayload(p);
        setLogsOpen(true);
      },
    });
    return () => {
      alive = false;
      unsub.then((fn) => fn?.()).catch(() => undefined);
    };
  }, []);

  useEffect(() => { (window as unknown as { __ready?: boolean }).__ready = true; }, []);

  // browser-preview test hook: fire the whole-card wave on demand (absent in Tauri)
  useEffect(() => {
    if ((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) return;
    (window as unknown as { __waveTest?: () => void }).__waveTest = () => {
      const zone = zoneOneRef.current;
      if (!zone) return;
      void playWave({ host: zone, oldEl: zone, newEl: zone, src: [108, 108], pal: PAL.failed, seed: 4.1, lens: 60 })
        .then(() => { zone.hidden = false; });
    };
  }, []);

  const sessions = snapshot?.sessions ?? [];
  const supervisors = snapshot?.supervisors ?? [];
  const eps = snapshot?.eps ?? 0;
  const top = snapshot ? topStatus(sessions) : 'idle';
  const wcount = workingCount(sessions);
  const load = normalizeEps(eps);
  const booted = snapshot !== null;

  // failure/done seen → haptic; whole-card top status flip → wave across zone one
  useEffect(() => {
    if (!snapshot) return;
    const prev = prevTopRef.current;
    if (prev === top) return;
    prevTopRef.current = top;
    if (prev === null) return; // boot: no haptic storm, no wave on first paint
    send({ type: 'haptic', kind: top === 'failed' ? 'levelChange' : 'alignment' });
    if (!reduced && top !== 'idle' && zoneOneRef.current) {
      const zone = zoneOneRef.current;
      // oldEl === newEl: the veil resolves the new state in place; restore visibility when the wave ends
      void playWave({ host: zone, oldEl: zone, newEl: zone, src: [108, 108], pal: PAL[top], seed: 4.1, lens: 60 })
        .then(() => { zone.hidden = false; });
    }
  }, [top, snapshot]);

  // ---------- intent machine (DESIGN.md §5) ----------
  const go = useCallback((m: Mode, ms: number) => {
    window.clearTimeout(intentTimer.current);
    intentTimer.current = window.setTimeout(() => {
      modeRef.current = m;
      setMode(m);
      send({ type: 'set_frame', mode: m });
      if (m === 'open') send({ type: 'haptic', kind: 'alignment' });
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
    send({ type: 'kill', id });
    setKillingIds((s) => new Set(s).add(id));
    window.setTimeout(() => setKillingIds((s) => { const n = new Set(s); n.delete(id); return n; }), 3000);
  }, []);
  const reprompt = useCallback((s: Session) => send({ type: 'reprompt', id: s.id, text: 'resume' }), []);
  const focus = useCallback((id: string) => send({ type: 'focus', id }), []);
  const logs = useCallback((s: Session) => send({ type: 'logs', id: s.id }), []);
  const cleanup = useCallback(() => {
    send({ type: 'cleanup' });
    setSweepSent(true);
    window.setTimeout(() => setSweepSent(false), 800);
  }, []);

  const fidIds = sessions.filter((s) => s.status === 'working').map((s) => ({ id: s.id, live: true }))
    .concat(sessions.filter((s) => s.status !== 'working').map((s) => ({ id: s.id, live: false })));

  const open = mode === 'open';

  return (
    <Shell mode={mode} shellRef={shellRef} onEnter={onEnter} onLeave={onLeave} onTap={onTap}>
      {/* floor glow — load as a number, status as colour */}
      <div className="floor" data-s={top} style={{ '--lvl': load } as CSSProperties} />

      {/* rest / peek chrome */}
      <div className="wings pointer-events-none absolute inset-0 flex items-center justify-between px-0" aria-hidden={open}>
        <div className="wing"><span className="wing-num num">{pad2(wcount)}</span><span className="wing-label">WORKING</span></div>
        <div className="wing"><span className="wing-num num">{fmtEps(Math.min(eps, 99.9))}</span><span className="wing-label">EV/S</span></div>
      </div>
      <div className="peek-center" aria-hidden="true">
        <span className="num" style={{ font: '500 20px/1 "Inter Variable", Inter, sans-serif', letterSpacing: '-.02em' }}>{pad2(wcount)}</span>
        <span className="label">IN FLIGHT</span>
      </div>

      <FloatingIds ids={fidIds} active={open} onFocus={focus} />

      {/* open panel — 32px black fusion head + zones, on the void */}
      <div className="panel-content absolute left-0 top-0 flex h-full w-[560px] flex-col">
        {/* header strip */}
        <header className="flex h-8 shrink-0 select-none items-center justify-between px-5">
          <span className="flex items-center gap-2">
            <OrchestratorMark size={13} className="text-white/80" />
            <span className="label" style={{ color: 'rgba(255,255,255,.62)' }}>Orchestrator</span>
          </span>
          <span className="flex items-center gap-2">
            <span className="label" style={{ fontSize: 8.5 }}>EPS</span>
            <NumberRoll value={Math.min(eps, 99.9)} format={(n) => n.toFixed(1)} className="mono text-[12px]" />
            <span className="select-none" style={{ color: 'rgba(255,255,255,.22)' }}>·</span>
            <span className="flex items-center gap-1.5">
              <i className={`live-dot live-dot--${top}`} />
              <span className="label" style={{ color: 'rgba(255,255,255,.82)' }}>{top}</span>
            </span>
          </span>
        </header>

        {/* zone 1 — state: orb + instruments */}
        <section ref={zoneOneRef} className="relative shrink-0 overflow-hidden" style={{ height: 216 }}>
          <div className="flex h-full items-center gap-5 px-5">
            <Orb status={top} running={open}>
              <NumberRoll value={wcount} format={(n) => pad2(Math.round(n))} className="num-xl text-white" />
              <span className="label" style={{ color: 'rgba(255,255,255,.56)' }}>In Flight</span>
            </Orb>
            <div className="flex min-w-0 flex-1 flex-col gap-1.5">
              <span className="label">Events / Sec</span>
              <HeartbeatField epsRef={epsRef} active={open} />
              <div className="flex items-baseline justify-between">
                <span className="mono" style={{ fontSize: 8, letterSpacing: '.18em', color: 'rgba(255,255,255,.3)' }}>−10S</span>
                <span className="mono" style={{ fontSize: 8, letterSpacing: '.18em', color: 'rgba(255,255,255,.3)' }}>NOW</span>
              </div>
              <div className="mt-1.5 flex items-baseline justify-between">
                <span className="label">Load</span>
                <span className="mono" style={{ fontSize: 8, color: 'rgba(255,255,255,.3)' }}>{Math.round(load * 100)} / 100</span>
              </div>
              <div className="load-track"><div className="load-fill" style={{ '--lvl': load } as CSSProperties} /></div>
            </div>
          </div>
        </section>

        <hr className="z-hairline mx-5" />

        {/* zone 2 — the swarm, two tiles */}
        <section className="flex shrink-0 gap-3 px-5 py-1.5" style={{ height: 162 }}>
          <div className="tile tile--blue flex h-full shrink-0 flex-col overflow-hidden p-3" style={{ width: 268 }}>
            <span className="label" style={{ color: 'rgba(255,255,255,.66)' }}>Supervisors</span>
            <div className="relative min-h-0 flex-1">
              <Radar supervisors={supervisors} />
            </div>
            <span className="mono text-[9px]" style={{ color: 'rgba(255,255,255,.68)' }}>
              NEAREST RING · {supervisors.length} SUPERVISOR{supervisors.length === 1 ? '' : 'S'}
            </span>
          </div>
          <div className="tile flex h-full min-w-0 flex-1 flex-col p-3">
            <div className="flex items-baseline justify-between">
              <span className="label">Sessions</span>
              <span className="mono text-[8.5px]" style={{ color: 'rgba(255,255,255,.35)' }}>{sessions.length} SLOTS</span>
            </div>
            <div className="flex min-h-0 flex-1 items-center justify-center">
              <SessionGrid supervisors={supervisors} sessions={sessions} active={open} />
            </div>
            <div className="flex items-center gap-2.5">
              {(['working', 'idle', 'failed', 'done'] as const).map((k) => (
                <span key={k} className="mono flex items-center gap-1.5 text-[8px]" style={{ color: 'rgba(255,255,255,.56)' }}>
                  <i className={`legend-dot legend-dot--${k}`} />
                  {k.toUpperCase()}
                </span>
              ))}
            </div>
          </div>
        </section>

        <hr className="z-hairline mx-5" />

        {/* zone 3 — the tree */}
        <section className="flex min-h-0 flex-1 flex-col">
          <div className="flex shrink-0 items-baseline justify-between px-5 pb-1 pt-2">
            <span className="label">Tree — {sessions.length} Sessions</span>
            <span className="mono text-[9.5px]" style={{ color: 'rgba(255,255,255,.3)', letterSpacing: '.18em' }}>SORT: ID</span>
          </div>
          <div className="min-h-0 flex-1">
            {!booted ? (
              <div className="flex h-full items-center justify-center">
                <span className="label" style={{ color: 'rgba(255,255,255,.45)' }}>WAITING FOR NOTCHD — SUBSCRIBE PENDING</span>
              </div>
            ) : sessions.length === 0 ? (
              <EmptyState />
            ) : (
              <TreeList
                sessions={sessions}
                killingIds={killingIds}
                onLogs={logs}
                onReprompt={reprompt}
                onFocus={focus}
                onKill={kill}
              />
            )}
          </div>
        </section>

        <hr className="z-hairline mx-5" />

        {/* zone 4 — step-latency honesty */}
        <div className="shrink-0 px-5 pb-1 pt-1">
          <Drift sessions={sessions} />
        </div>

        {/* footer bar */}
        <footer className="flex h-[25px] shrink-0 items-center justify-between border-t border-white/10 px-5">
          <span className="mono text-[9px]" style={{ color: 'rgba(255,255,255,.5)', letterSpacing: '.14em' }}>NOTCH v0.1 — ORCHESTRATOR</span>
          <button className="text-btn" onClick={cleanup}>{sweepSent ? 'SWEEP SENT' : 'SWEEP TEMP ORPHANS'}</button>
        </footer>
      </div>

      <Specular shellRef={shellRef} />
      <div className="grain" aria-hidden="true" />
      <LogsPanel open={logsOpen} payload={logsPayload} onClose={() => setLogsOpen(false)} />
    </Shell>
  );
}
