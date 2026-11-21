// Panel.tsx — header (logo + status only) · state · supervisors · sessions · latency · footer.
// Idle is a separate, short layout: sphere + "Idle" + the first-task composer. No zeros, no empty cards.
import { useEffect, useState, type MutableRefObject } from 'react';
import { NumberRoll } from './NumberRoll';
import { HeartbeatField } from './HeartbeatField';
import { Drift } from './Drift';
import { SupervisorRow } from './SupervisorRow';
import { TreeList } from './TreeList';
import { EmptyState } from './EmptyState';
import { PixelIcon } from './PixelIcon';
import { PixelMeter } from './pixel/PixelMeter';
import { DitherField } from './pixel/DitherField';
import { PixelSphere } from './pixel/PixelSphere';
import type { Session, Supervisor, Decision, Escalation } from '../lib/bridge';
import type { Status } from '../lib/tokens';

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

type PanelProps = {
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
};

/** The short idle form. The composer is the hero; nothing else earns space. */
function IdlePanel({ openDashboard }: { openDashboard: () => void }) {
  return (
    <div className="panel panel--idle">
      <header className="p-head">
        <span className="p-brand">
          <span className="p-mark" aria-hidden="true"><span className="brand-logo" /></span>
          <span className="p-status"><i className="live-dot live-dot--idle" /><span>Idle</span></span>
        </span>
      </header>
      <div className="idle-body">
        <div className="idle-hero" aria-hidden="true">
          <DitherField mode="radial" cell={4} floor={0} ceil={0.24} />
          <PixelSphere size={56} cell={3} live={false} />
        </div>
        <EmptyState onNewTask={openDashboard} />
      </div>
    </div>
  );
}

export function Panel(props: PanelProps) {
  const { open, top, wcount, eps, load, booted, sessions, supervisors, decisions, escalations,
    killingIds, sweepSent, epsRef,
    logs, reprompt, focus, kill, onCleanup,
    openDashboard, onMessageSupervisor, onTogglePause, onKillSupervisor, onOpenTarget } = props;

  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!open) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [open]);

  const idle = booted && sessions.length === 0;
  const escCount = escalations?.length ?? 0;

  if (!booted) {
    return (
      <div className="panel">
        <header className="p-head">
          <span className="p-brand">
            <span className="p-mark" aria-hidden="true"><span className="brand-logo" /></span>
            <span className="p-status"><i className="live-dot live-dot--idle" /><span>Connecting</span></span>
          </span>
        </header>
        <div className="empty"><span className="muted">Establishing a session snapshot…</span></div>
      </div>
    );
  }

  if (idle) return <IdlePanel openDashboard={openDashboard} />;

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

        <div className="state">
          <div className="card state-num">
            <PixelSphere size={40} cell={3} live={open && wcount > 0} className="state-num-mark" />
            <div className="state-num-read">
              <NumberRoll value={wcount} format={(n) => String(Math.round(n))} className="num-xl" />
              <span className="label">Agents running</span>
            </div>
          </div>
          <div className="card state-signal">
            <div className="row-between">
              <span className="label">Events / sec</span>
              <span className="label label--pale num">{eps.toFixed(1)}</span>
            </div>
            {eps > 0 && <HeartbeatField epsRef={epsRef} active={open} />}
            <div className="row-between">
              <span className="label">Load</span>
              <span className="label num">{Math.round(load * 100)}%</span>
            </div>
            <PixelMeter value={load} cells={22} tone="hot" label="Load" />
          </div>
        </div>


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
      <section className="sess card">
        <div className="sec-head">
          <span className="label">Sessions</span>
          <span className="label num">{sessions.length}</span>
        </div>
        <div className="sess-body">
          <TreeList sessions={sessions} killingIds={killingIds} onLogs={logs} onReprompt={reprompt} onFocus={focus} onKill={kill} />
        </div>
      </section>

      <Drift sessions={sessions} />

      <footer className="p-foot">
        <button className="btn-primary text-btn ripple" onClick={openDashboard}>
          <PixelIcon name="window" size={12} />Open dashboard
        </button>
        <button className="btn-secondary text-btn ripple" onClick={onCleanup}>
          <PixelIcon name="trash" size={12} />{sweepSent ? 'Sweep sent' : 'Sweep orphans'}
        </button>
      </footer>
    </div>
  );
}
