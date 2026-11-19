// Inspector.tsx — right pane. Session anatomy with the DoD contract in quotes; no invented controls.
import { useEffect, useState } from 'react';
import type { Session, Supervisor, Snapshot } from '../../lib/bridge';
import { middleTruncate, fmtElapsed } from '../../lib/format';
import { RepromptIcon, PauseIcon, PlayIcon, KillIcon, FocusIcon, BubbleIcon } from '../Icons';
import { Sym } from '../Symbol';

export function Inspector({ selection, snapshot, onReprompt, onTogglePause, onKill, onOpenHerdr, onFocusComposer, onSelectSession }: {
  selection: { kind: 'supervisor' | 'session'; id: string } | null;
  snapshot: Snapshot | null;
  onReprompt: (sessionId: string) => void;
  onTogglePause: (sup: Supervisor) => void;
  onKill: (targetId: string) => void;
  onOpenHerdr: (id: string) => void;
  onFocusComposer: () => void;
  onSelectSession: (sessionId: string) => void;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const sessions = snapshot?.sessions ?? [];
  const supervisors = snapshot?.supervisors ?? [];
  const attribution = snapshot?.attribution ?? [];
  const conflicts = snapshot?.conflicts ?? [];

  const session = selection?.kind === 'session' ? sessions.find((s) => s.id === selection.id) ?? null : null;
  const supervisor = selection?.kind === 'supervisor' ? supervisors.find((s) => s.id === selection.id) ?? null : null;

  if (!session && !supervisor) {
    return (
      <aside className="inspector" aria-label="Inspector">
        <span className="label">Inspector</span>
        <p className="inspector-empty">Select a session — its anatomy lands here.</p>
      </aside>
    );
  }

  if (supervisor) {
    const own = sessions.filter((s) => s.sup === supervisor.id);
    return (
      <aside className="inspector" aria-label="Supervisor inspector">
        <span className="label">Supervisor</span>
        <div className="inspector-id">{supervisor.id}</div>
        <div className="inspector-status">
          <i className={`dot dot--${supervisor.status}`} />
          <span>{capitalize(supervisor.status)}</span>
        </div>
        <div className="meta-grid">
          <span>sessions</span><b>{own.length}</b>
          <span>working</span><b>{supervisor.working}</b>
          <span>idle</span><b>{supervisor.idle}</b>
          <span>failed</span><b>{supervisor.failed}</b>
        </div>
        <div className="control-row">
          <button className="ctl" onClick={onFocusComposer} title="Message this supervisor">
            <BubbleIcon size={13} /><span>Message</span>
          </button>
          <button className="ctl" onClick={() => onTogglePause(supervisor)} title={supervisor.paused ? 'Resume supervisor' : 'Pause supervisor'}>
            {supervisor.paused ? <PlayIcon size={12} /> : <PauseIcon size={12} />}
            <span>{supervisor.paused ? 'Resume' : 'Pause'}</span>
          </button>
          <button className="ctl ctl--kill" onClick={() => onKill(supervisor.id)} title="Kill every session under this supervisor">
            <KillIcon size={13} /><span>Kill all</span>
          </button>
        </div>
        <div className="inspector-list">
          <span className="label" style={{ marginBottom: 4 }}>Sessions under {supervisor.id}</span>
          {own.length === 0 && <span className="inspector-empty">None yet.</span>}
          {own.map((s) => (
            <button key={s.id} className="inspector-session-row" onClick={() => onSelectSession(s.id)}>
              <i className={`dot dot--${s.status}`} />
              <span className="mono">{s.id}</span>
              <span className="truncate">{s.title}</span>
            </button>
          ))}
        </div>
      </aside>
    );
  }

  // session inspector
  const s = session!;
  const maxIter = s.maxIterations ?? 8;
  const iter = s.iterations ?? 0;
  const iterPct = Math.min(1, iter / maxIter);
  const myFiles = attribution.filter((a) => a.session === s.id);
  const myConflicts = conflicts.filter((c) => c.sessions.includes(s.id));
  const ownSup = supervisors.find((sv) => sv.id === s.sup);

  return (
    <aside className="inspector" aria-label={`Inspector for ${s.id}`}>
      <span className="label">Session</span>
      <div className="inspector-id">{s.id}</div>
      <div className="inspector-status">
        <i className={`dot dot--${s.status}`} />
        <span>{capitalize(s.status)}</span>
      </div>
      <div className="meta-grid">
        <span>model</span><b className="mono">{s.model ?? '—'}</b>
        <span>supervisor</span><b className="mono">{s.sup}</b>
        <span>cwd</span><b className="mono" title={s.cwd}>{middleTruncate(s.cwd, 30)}</b>
        <span>started</span><b className="mono">{new Date(s.startedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</b>
        <span>elapsed</span><b className="mono">{fmtElapsed(s.startedAt, now)}</b>
      </div>

      <div className="iter-block">
        <div className="iter-head">
          <span className="label">Iterations</span>
          <span className="mono" style={{ color: 'rgba(255,255,255,.5)' }}>{iter} / {maxIter}</span>
        </div>
        <div className="iter-track" role="progressbar" aria-valuemin={0} aria-valuemax={maxIter} aria-valuenow={iter} aria-label="Iteration budget used">
          <div className="iter-fill" style={{ transform: `scaleX(${iterPct})` }} />
        </div>
      </div>

      {s.definitionOfDone && (
        <blockquote className="dod-quote">“{s.definitionOfDone}”</blockquote>
      )}

      <div className="inspector-list">
        <span className="label" style={{ marginBottom: 2 }}>Files touched</span>
        {myFiles.length === 0 && <span className="inspector-empty">No files attributed yet.</span>}
        {myFiles.slice(0, 8).map((f, i) => (
          <div key={i} className="file-row"><span className="mono">{f.file}</span></div>
        ))}
        {myFiles.length > 8 && <span className="inspector-empty">+{myFiles.length - 8} more</span>}
      </div>

      {myConflicts.length > 0 && (
        <div className="inspector-list conflict-list">
          <span className="label" style={{ marginBottom: 2, color: '#FFB3C1' }}>Conflicts</span>
          {myConflicts.map((c, i) => (
            <div key={i} className="conflict-row">
              <span className="mono">{c.file}</span>
              <span className="conflict-note">claimed by {c.sessions.filter((x) => x !== s.id).join(', ')}</span>
            </div>
          ))}
        </div>
      )}

      <div className="control-row">
        <button className="ctl" onClick={() => onReprompt(s.id)} title="Re-prompt this session">
          <RepromptIcon size={13} /><span>Re-prompt</span>
        </button>
        <button className="ctl" onClick={() => onTogglePause(ownSup!)} disabled={!ownSup} title={ownSup?.paused ? 'Resume supervisor' : 'Pause supervisor'}>
          {ownSup?.paused ? <PlayIcon size={12} /> : <PauseIcon size={12} />}
          <span>{ownSup?.paused ? 'Resume' : 'Pause'}</span>
        </button>
        <button className="ctl ctl--kill" onClick={() => onKill(s.id)} title="Kill session">
          <KillIcon size={13} /><span>Kill</span>
        </button>
        <button className="ctl" onClick={() => onOpenHerdr(s.id)} title="Open this session in herdr">
          <FocusIcon size={13} /><span>Open in herdr</span>
        </button>
      </div>
    </aside>
  );
}

const capitalize = (s: string) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);
