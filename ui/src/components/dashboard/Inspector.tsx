// Inspector.tsx — right pane. Session anatomy with the DoD contract in quotes; no invented controls.
// A single DistortionWave marks a REAL status change of the selected session (one-shot).
import { useEffect, useRef, useState } from 'react';
import { sendMessage, type Session, type Supervisor, type Snapshot } from '../../lib/bridge';
import { middleTruncate, fmtElapsed } from '../../lib/format';
import { PixelIcon } from '../PixelIcon';
import { PixelMeter } from '../pixel/PixelMeter';

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
  const [draft, setDraft] = useState('');
  const [sendState, setSendState] = useState<'idle' | 'sent' | 'failed'>('idle');
  const msgRef = useRef<HTMLInputElement>(null);
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

  if (supervisor) {
    const own = sessions.filter((s) => s.sup === supervisor.id);
    return (
      <aside className="inspector" aria-label="Supervisor inspector">
        <div className="inspector-head">
          <span className="label">Supervisor</span>
          <span className="label">{supervisor.paused ? 'Paused' : 'Active'}</span>
        </div>
        <div className="inspector-body">
          <div className="inspector-id">{supervisor.id}</div>
          {supervisor.title && <div className="label dim inspector-title">{supervisor.title}</div>}
          <div className="status-wave">
            <div className="inspector-status">
              <i className={`dot dot--${supervisor.status}`} />
              <span>{supervisor.status}</span>
            </div>
          </div>
          <div className="anatomy meta-grid">
            <div className="meta-row"><span>sessions</span><b>{own.length}</b></div>
            <div className="meta-row"><span>working</span><b>{supervisor.working}</b></div>
            <div className="meta-row"><span>idle</span><b>{supervisor.idle}</b></div>
            <div className="meta-row"><span>failed</span><b>{supervisor.failed}</b></div>
          </div>
          <div className="iter-block">
            <div className="iter-head"><span className="label">Load</span><span>{supervisor.working}/{Math.max(1, own.length)}</span></div>
            <PixelMeter value={own.length ? supervisor.working / own.length : 0} cells={16} tone="hot" label="Working sessions" />
          </div>
          <div className="control-row">
            <button className="ctl ripple" onClick={() => msgRef.current?.focus()} title="Message this supervisor">
              <PixelIcon name="bubble" size={12} /><span>Message</span>
            </button>
            <button className="ctl ripple" onClick={() => onTogglePause(supervisor)} title={supervisor.paused ? 'Resume supervisor' : 'Pause supervisor'}>
              {supervisor.paused ? <PixelIcon name="play" size={12} /> : <PixelIcon name="pause" size={12} />}
              <span>{supervisor.paused ? 'Resume' : 'Pause'}</span>
            </button>
            <button className="ctl ctl--kill ripple" onClick={() => onKill(supervisor.id)} title="Kill every session under this supervisor">
              <PixelIcon name="kill" size={12} /><span>Kill all</span>
            </button>
          </div>
          <form className="inspector-message" onSubmit={async (e) => {
            e.preventDefault();
            const body = draft.trim();
            if (!body) return;
            const res = await sendMessage('ORCH', supervisor.id, 'task', body);
            setSendState(res.ok ? 'sent' : 'failed');
            if (res.ok) setDraft('');
          }}>
            <input ref={msgRef} value={draft} onChange={(e) => { setDraft(e.target.value); setSendState('idle'); }}
              placeholder={`Message ${supervisor.id}`} aria-label={`Message ${supervisor.id}`} spellCheck={false} />
            <button type="submit" className="ctl ripple" disabled={!draft.trim()} aria-label="Send message">
              <PixelIcon name="send" size={12} />
            </button>
          </form>
          {sendState !== 'idle' && (
            <p role="status" className={`inspector-send-${sendState}`}>
              {sendState === 'sent' ? `Message delivered to ${supervisor.id}` : 'Message failed — try again'}
            </p>
          )}
          <div className="inspector-list">
            <span className="label">Sessions under {supervisor.id}</span>
            {own.length === 0 && <span className="inspector-empty">None yet.</span>}
            {own.map((s) => (
              <button key={s.id} className="inspector-session-row ripple" onClick={() => { onSelectSession(s.id); onOpenHerdr(s.id); }}>
                <i className={`dot dot--${s.status}`} />
                <span className="tabular-nums">{s.id}</span>
                <span className="truncate">{s.title}</span>
              </button>
            ))}
          </div>
        </div>
      </aside>
    );
  }

  // session inspector
  if (!session) return null;
  const s = session;
  const maxIter = s.maxIterations ?? 8;
  const iter = s.iterations ?? 0;
  const iterPct = Math.min(1, iter / maxIter);
  const myFiles = attribution.filter((a) => a.session === s.id);
  const myConflicts = conflicts.filter((c) => c.sessions.includes(s.id));
  const ownSup = supervisors.find((sv) => sv.id === s.sup);
  const statusBlock = (st: string) => (
    <div className="inspector-status">
      <i className={`dot dot--${st}`} />
      <span>{st}</span>
    </div>
  );

  return (
    <aside className="inspector" aria-label={`Inspector for ${s.id}`}>
      <div className="inspector-head">
        <span className="label">Session</span>
        <span className="label">{s.status}</span>
      </div>
      <div className="inspector-body">
        <div className="inspector-id tabular-nums">{s.id}</div>
        {statusBlock(s.status)}
        {(s.goal || s.task) && (s.goal || s.task) !== s.title && <div className="inspector-goal"><span className="label">Goal</span><p>{s.goal || s.task}</p></div>}
        <div className="anatomy meta-grid">
          <div className="meta-row"><span>Model</span><b title={s.model ?? undefined}>{s.model ?? '—'}</b></div>
          <div className="meta-row"><span>supervisor</span><b>{s.sup}</b></div>
          <div className="meta-row"><span>cwd</span><b title={s.cwd}>{middleTruncate(s.cwd, 28)}</b></div>
          <div className="meta-row"><span>started</span><b>{new Date(s.startedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</b></div>
          <div className="meta-row"><span>elapsed</span><b>{fmtElapsed(s.startedAt, now)}</b></div>
          {s.paneId && <div className="meta-row"><span>pane</span><b title={s.paneId}>{middleTruncate(s.paneId, 24)}</b></div>}
          {s.wallClockDeadline && (
            <div className="meta-row"><span>deadline</span><b title={new Date(s.wallClockDeadline).toLocaleString()}>
              {fmtElapsed(now, s.wallClockDeadline)}
            </b></div>
          )}
        </div>

        <div className="iter-block">
          <div className="iter-head">
            <span className="label">Iterations</span>
            <span>{iter} / {maxIter}</span>
          </div>
          <PixelMeter value={iterPct} cells={16} tone={iterPct > 0.85 ? 'pale' : 'hot'} label="Iteration budget used" />
        </div>

        {s.definitionOfDone && (
          <blockquote className="dod-quote">“{s.definitionOfDone}”</blockquote>
        )}

        <div className="inspector-list">
          <span className="label">Files touched</span>
          {myFiles.length === 0 && <span className="inspector-empty">No files attributed yet.</span>}
          {myFiles.slice(0, 8).map((f, i) => (
            <div key={i} className="file-row"><span className="tabular-nums truncate">{f.file}</span></div>
          ))}
          {myFiles.length > 8 && <span className="inspector-empty">+{myFiles.length - 8} more</span>}
        </div>

        {myConflicts.length > 0 && (
          <div className="inspector-list conflict-list">
            <span className="label" style={{ paddingBottom: 6, color: 'var(--f4)' }}>Conflicts</span>
            {myConflicts.map((c, i) => (
              <div key={i} className="conflict-row">
                <span className="tabular-nums truncate">{c.file}</span>
                <span className="conflict-note">claimed by {c.sessions.filter((x) => x !== s.id).join(', ')}</span>
              </div>
            ))}
          </div>
        )}

        <div className="control-row">
          <button className="ctl ripple" onClick={() => onReprompt(s.id)} title="Re-prompt this session">
            <PixelIcon name="reprompt" size={12} /><span>Re-prompt</span>
          </button>
          <button className="ctl ripple" onClick={() => onTogglePause(ownSup!)} disabled={!ownSup} title={ownSup?.paused ? 'Resume supervisor' : 'Pause supervisor'}>
            {ownSup?.paused ? <PixelIcon name="play" size={12} /> : <PixelIcon name="pause" size={12} />}
            <span>{ownSup?.paused ? 'Resume' : 'Pause'}</span>
          </button>
          <button className="ctl ctl--kill ripple" onClick={() => onKill(s.id)} title="Kill session">
            <PixelIcon name="kill" size={12} /><span>Kill</span>
          </button>
          <button className="ctl ripple" onClick={() => onOpenHerdr(s.id)} title="Open this session in herdr">
            <PixelIcon name="focus" size={12} /><span>Herdr</span>
          </button>
        </div>
      </div>
    </aside>
  );
}
