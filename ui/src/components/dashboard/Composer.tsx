// Composer.tsx — the assignment surface. Enter sends; Shift+Enter newlines; Cmd+K focuses.
// A prompt gutter and a hard 1px well: this is a terminal you type work into, not a chat box.
// The dither appears only during `sending`: pixels leave the Assign button and fly into the beam.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { VoiceBeam } from '../../vendor/voice-glow/src';
import { submitTask, listModels, type Session } from '../../lib/bridge';
import { PixelIcon } from '../PixelIcon';

export type ComposerTarget = { kind: 'supervisor' | 'session'; id: string } | null;

// `auto` uses whatever model the operator configured in OpenCode. No house
// default is forced on anyone; real models come from the host's `list_models`.
const FALLBACK_MODELS = ['auto'];

export function Composer({ selection, sessions, prefill, focusSignal }: {
  selection: ComposerTarget;
  sessions: Session[];
  prefill: { text: string; at: number } | null;
  focusSignal: number;
}) {
  const [task, setTask] = useState(() => {
    try {
      const draft = localStorage.getItem('notch.draft');
      if (draft) { localStorage.removeItem('notch.draft'); return draft; }
    } catch { /* storage may be unavailable */ }
    return '';
  });
  const [dod, setDod] = useState('');
  const [dodOpen, setDodOpen] = useState(false);
  const [cwd, setCwd] = useState(() => { try { return localStorage.getItem('notch.cwd') ?? '/Users/harshitduggal'; } catch { return '/Users/harshitduggal'; } });
  const [cwdHint, setCwdHint] = useState(false);
  const [model, setModel] = useState(() => { try { return localStorage.getItem('notch.model') ?? 'auto'; } catch { return 'auto'; } });
  const [models, setModels] = useState<string[]>(FALLBACK_MODELS);
  const [modelOpen, setModelOpen] = useState(false);
  const [state, setState] = useState<'idle' | 'sending' | { ok: boolean; error?: string; id?: string; ms?: number }>('idle');
  const [targetSession, setTargetSession] = useState<string | null>(selection?.kind === 'session' ? selection.id : null);
  const [assignBtn, setAssignBtn] = useState<HTMLButtonElement | null>(null);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const sendingRef = useRef(false);
  sendingRef.current = state === 'sending';
  const getLevel = useCallback(() => (sendingRef.current ? 0.35 : 0), []);

  // follow explicit selection: clicking a session targets it; clicking a supervisor retargets to it
  useEffect(() => {
    setTargetSession(selection?.kind === 'session' ? selection.id : null);
  }, [selection?.kind, selection?.id]);

  // prefills (escalations "Re-prompt fresh"): replace contents and focus
  useEffect(() => {
    if (prefill) {
      setTask((p) => (p.trim() ? p : prefill.text));
      taRef.current?.focus();
    }
  }, [prefill?.at]); // eslint-disable-line react-hooks/exhaustive-deps

  // focus signal (inspector "Re-prompt", Cmd+K, sidebar buttons)
  useEffect(() => {
    if (focusSignal > 0) taRef.current?.focus();
  }, [focusSignal]);

  // models from the host once per mount
  useEffect(() => {
    let alive = true;
    listModels().then((r) => { if (alive && r.models?.length) setModels(r.models); });
    return () => { alive = false; };
  }, []);

  // click-outside closes the model menu
  useEffect(() => {
    if (!modelOpen) return;
    const onDown = (e: PointerEvent) => { if (!menuRef.current?.contains(e.target as Node)) setModelOpen(false); };
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setModelOpen(false); };
    window.addEventListener('pointerdown', onDown);
    window.addEventListener('keydown', onKey);
    return () => { window.removeEventListener('pointerdown', onDown); window.removeEventListener('keydown', onKey); };
  }, [modelOpen]);

  const targetSupervisorId = useMemo(() => {
    if (targetSession) return sessions.find((s) => s.id === targetSession)?.sup;
    return selection?.kind === 'supervisor' ? selection.id : undefined;
  }, [targetSession, sessions, selection]);

  const targetChipLabel = targetSession ?? (selection?.kind === 'supervisor' ? selection.id : null);

  const autosize = () => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 5 * 22)}px`;
  };

  // a staged draft from the popover opens at full height
  useEffect(() => { if (task) autosize(); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const send = async () => {
    const text = task.trim();
    if (!text || state === 'sending') return;
    const cwdClean = cwd.trim();
    if (cwdClean && !cwdClean.startsWith('/')) { setCwdHint(true); return; }
    const t0 = performance.now();
    setState('sending');
    try { localStorage.setItem('notch.cwd', cwdClean); localStorage.setItem('notch.model', model); } catch { /* storage may be unavailable */ }
    const res = await submitTask({
      task: text,
      cwd: cwdClean || undefined,
      definitionOfDone: dod.trim() || undefined,
      model: model || undefined,
      supervisorId: targetSupervisorId,
    });
    const ms = Math.round(performance.now() - t0);
    if (res.ok) {
      setTask(''); setDod(''); setCwdHint(false);
      setState({ ok: true, id: res.sessionId, ms });
      window.setTimeout(autosize, 0);
      window.setTimeout(() => setState('idle'), 2400);
    } else {
      setState({ ok: false, error: res.error ?? 'submit_task failed — see logs', ms });
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); e.currentTarget.focus(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); send(); }
  };

  const done = state !== 'idle' && state !== 'sending' && state.ok;
  const error = state !== 'idle' && state !== 'sending' && !state.ok ? state.error : null;
  const roundtrip = state !== 'idle' && state !== 'sending' ? state.ms : undefined;

  return (
    <div className="composer">
      <VoiceBeam
        className="composer-beam"
        look="dither"
        type="default"
        theme="dark"
        colorVariant={error ? 'sunset' : 'ice'}
        level={getLevel}
        strength={0.25}
        pixelGlow={0.08}
        active={state === 'sending'}
        pixelExitTarget={error ? null : assignBtn}
        idle={0}
      >
        {targetChipLabel && (
          <div className="composer-target">
            <span>→ {targetChipLabel}</span>
            <button className="chip-clear" aria-label="Clear target — assign to a new supervisor" title="Clear target"
              onClick={() => setTargetSession(null)}>
              <PixelIcon name="close" size={12} />
            </button>
          </div>
        )}
        <div className="composer-box">
          <span className="composer-prompt" aria-hidden="true">›</span>
          <textarea
            ref={taRef}
            className="composer-input"
            rows={1}
            placeholder="Assign work — Enter to assign, Shift+Enter for a new line"
            value={task}
            onChange={(e) => { setTask(e.target.value); autosize(); }}
            onKeyDown={onKeyDown}
            aria-label="Assign work to a session"
          />
        </div>
        <div className="composer-dod">
          <button className="linklike" onClick={() => setDodOpen((v) => !v)} aria-expanded={dodOpen}>
            <PixelIcon name="chevron_down" size={12} className={dodOpen ? 'rotate-180' : ''} />
            Done when…
          </button>
          {dodOpen && (
            <input className="dod-input" value={dod} onChange={(e) => setDod(e.target.value)}
              placeholder="tests pass, docs updated, evidence pasted" aria-label="Definition of done" />
          )}
        </div>
        <div className="composer-row">
          <label className="cwd-field">
            <span>cwd</span>
            <input value={cwd} onChange={(e) => { setCwd(e.target.value); setCwdHint(false); }}
              onBlur={() => setCwdHint(cwd.trim().length > 0 && !cwd.trim().startsWith('/'))}
              spellCheck={false} aria-label="Working directory" />
          </label>
          <div ref={menuRef} className="model-picker">
            <button className="model-btn" onClick={() => setModelOpen((v) => !v)} aria-haspopup="listbox" aria-expanded={modelOpen}>
              <span className="tabular-nums">{model}</span>
              <PixelIcon name="chevron_down" size={12} />
            </button>
            {modelOpen && (
              <ul className="model-menu" role="listbox" aria-label="Model">
                {models.map((m) => (
                  <li key={m} role="option" aria-selected={m === model}>
                    <button className="model-item" onClick={() => { setModel(m); setModelOpen(false); }}>
                      <span className="tabular-nums">{m}</span>
                      {m === model && <PixelIcon name="check" size={12} />}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
          <button ref={setAssignBtn} className="assign-btn ripple" onClick={send} disabled={state === 'sending' || !task.trim()}>
            {state === 'sending' ? 'Assigning…' : 'Assign'}
          </button>
        </div>
      </VoiceBeam>
      {cwdHint && <div className="composer-hint">absolute path expected (e.g. /Users/you/project)</div>}
      {error && <div className="composer-error" role="alert">{error}{roundtrip !== undefined ? ` · ${roundtrip} ms` : ''}</div>}
      {done && state.id && <div className="composer-ok"><PixelIcon name="check" size={12} /> Submitted {state.id}{roundtrip !== undefined ? ` · ${roundtrip} ms` : ''}</div>}
    </div>
  );
}
