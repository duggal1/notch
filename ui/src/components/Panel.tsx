// Panel.tsx — header strip (status only, no logo) · state · supervisors · sessions · latency · footer.
import { useEffect, useState, type MutableRefObject, type Ref } from 'react';
import { DistortionWave, type DistortionWaveHandle, type PaletteName } from '../vendor/distortion-waves/src';
import { NumberRoll } from './NumberRoll';
import { HeartbeatField } from './HeartbeatField';
import { Drift } from './Drift';
import { SupervisorRow } from './SupervisorRow';
import { TreeList } from './TreeList';
import { EmptyState } from './EmptyState';
import { PixelIcon } from './PixelIcon';
import { PixelMeter } from './pixel/PixelMeter';
import { PixelSphere } from './pixel/PixelSphere';
import { DitherField } from './pixel/DitherField';
import type { Session, Supervisor, Decision, Escalation } from '../lib/bridge';
import { pad2 } from '../lib/format';
import type { Status } from '../lib/tokens';

const WAVE_PAL: Record<Status, PaletteName> = { working: 'ocean', failed: 'sunset', done: 'mono', idle: 'mono' };
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

export function Panel({ open, top, wcount, eps, load, booted, sessions, supervisors, decisions, escalations,
  killingIds, sweepSent, epsRef, waveRef,
  logs, reprompt, focus, kill, onCleanup,
  openDashboard, onMessageSupervisor, onTogglePause, onKillSupervisor, onOpenTarget }: {
  open: boolean;
  top: Status;
  wcount: number;
  eps: number;
  load: number;
  booted: boolean;
  sessions: Session[];
  supervisors: Supervisor[];
  decisions?: Decision[];
  escalations?: Escalation[];
  killingIds: Set<string>;
  sweepSent: boolean;
  epsRef: MutableRefObject<number>;
  waveRef: Ref<DistortionWaveHandle>;
  logs: (s: Session) => void;
  reprompt: (s: Session) => void;
  focus: (id: string) => void;
  kill: (id: string) => void;
  onCleanup: () => void;
  openDashboard: () => void;
  onMessageSupervisor: (id: string) => void;
  onTogglePause: (sup: Supervisor) => void;
  onKillSupervisor: (id: string) => void;
  onOpenTarget: (kind: 'supervisor' | 'session', id: string) => void;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!open) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [open]);

  const escCount = escalations?.length ?? 0;

  return (
    <div className="panel">
      <header className="p-head">
        <span className="p-brand">
          <span className="p-mark" aria-hidden="true">
            <span className="brand-logo" />
          </span>
          <span className="p-status">
            <i className={`live-dot live-dot--${top}`} />
            <span>{cap(top)}</span>
          </span>
        </span>
        <span className="p-head-right">
          {escCount > 0 && (
            <button className="escalation-chip" onClick={openDashboard} aria-label={`${escCount} escalation${escCount === 1 ? '' : 's'} — open dashboard`}>
              <i className="live-dot live-dot--failed" />
              <span className="num">{escCount}</span>
            </button>
          )}
          <span className="p-eps">
            <NumberRoll value={Math.min(eps, 99.9)} format={(n) => n.toFixed(1)} className="num" />
            <span className="p-eps-unit">ev/s</span>
          </span>
        </span>
      </header>

      {/* state: the wave passes across both cards when the top status flips */}
      <DistortionWave
        ref={waveRef}
        type="center"
        palette={WAVE_PAL[top]}
        cornerRadius={0}
        prewarm={false}
        style={{ height: 150, flex: 'none' }}
      >
        <div className="state">
          <div className="card state-num">
            <DitherField className="state-num-crown" mode="linear" angle={0} cell={2} floor={0.22} />
            <PixelSphere size={40} cell={2} live={open} className="state-num-mark" />
            <div className="state-num-read">
              <NumberRoll value={wcount} format={(n) => pad2(Math.round(n))} className="num-xl" />
              <span className="label">In flight</span>
            </div>
          </div>
          <div className="card state-signal">
            <div className="row-between">
              <span className="label">Events / sec</span>
              <span className="label label--pale num">{eps.toFixed(1)}</span>
            </div>
            <HeartbeatField epsRef={epsRef} active={open} />
            <div className="row-between">
              <span className="label">Load</span>
              <span className="label num">{Math.round(load * 100)}%</span>
            </div>
            <PixelMeter value={load} cells={22} tone="hot" label="Load" />
          </div>
        </div>
      </DistortionWave>

      {/* supervisors */}
      <section className="card sup-card">
        <div className="card-head">
          <span className="label">Supervisors</span>
          <span className="label num">{supervisors.length}</span>
        </div>
        <div className="list-scroll sup-list">
          {supervisors.length === 0 ? (
            <p className="muted" style={{ padding: '6px 8px' }}>None yet</p>
          ) : supervisors.map((sv) => (
            <SupervisorRow key={sv.id} sup={sv} sessions={sessions} decisions={decisions} now={now}
              variant="popover"
              onMessage={onMessageSupervisor}
              onTogglePause={onTogglePause}
              onKill={onKillSupervisor}
              onSelect={(id) => {
                onOpenTarget('supervisor', id);
                const target = sessions.find((s) => s.sup === id && s.status === 'working')
                  ?? sessions.find((s) => s.sup === id);
                if (target) focus(target.id);
              }} />
          ))}
        </div>
      </section>

      {/* sessions */}
      <section className="sess">
        <div className="sec-head">
          <span className="label">Sessions</span>
          <span className="label num">{sessions.length}</span>
        </div>
        <div className="sess-body">
          {!booted ? (
            <div className="empty"><span className="muted">Connecting to notchd…</span></div>
          ) : sessions.length === 0 ? (
            <EmptyState onNewTask={openDashboard} />
          ) : (
            <TreeList sessions={sessions} killingIds={killingIds} onLogs={logs} onReprompt={reprompt} onFocus={focus} onKill={kill} />
          )}
        </div>
      </section>

      <Drift sessions={sessions} />

      <footer className="p-foot">
        <button className="text-btn ripple" onClick={openDashboard}>
          <PixelIcon name="window" size={15} />Open dashboard
        </button>
        <button className="text-btn ripple" onClick={onCleanup}>
          <PixelIcon name="trash" size={14} />{sweepSent ? 'Sweep sent' : 'Sweep orphans'}
        </button>
      </footer>
    </div>
  );
}