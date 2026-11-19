// Composer.tsx — the assignment surface. Enter sends; Shift+Enter newlines; Cmd+K always focuses;
// the goal→definitionOfDone contract from the autonomous supervisor, exposed honestly.
import { useEffect, useMemo, useRef, useState } from 'react';
import { submitTask, listModels, type Session } from '../../lib/bridge';
import { CheckIcon, ChevronDownIcon, CloseIcon } from '../Icons';

export type ComposerTarget = { kind: 'supervisor' | 'session'; id: string } | null;

const FALLBACK_MODELS = ['opencode/hy4-flash-free', 'auto'];

export function Composer({ selection, sessions, prefill, focusSignal }: {
  selection: ComposerTarget;
  sessions: Session[];
  prefill: { text: string; at: number } | null;
  focusSignal: number;
}) {
  const [task, setTask] = useState('');
  const [dod, setDod] = useState('');
  const [dodOpen, setDodOpen] = useState(false);
  const [cwd, setCwd] = useState(() => { try { return localStorage.getItem('notch.cwd') ?? '/Users/harshitduggal'; } catch { return '/Users/harshitduggal'; } });
  const [cwdHint, setCwdHint] = useState(false);
  const [model, setModel] = useState(() => { try { return localStorage.getItem('notch.model') ?? 'opencode/hy4-flash-free'; } catch { return 'opencode/hy4-flash-free'; } });
  const [models, setModels] = useState<string[]>(FALLBACK_MODELS);
  const [modelOpen, setModelOpen] = useState(false);
  const [state, setState] = useState<'idle' | 'sending' | { ok: boolean; error?: string }>('idle');
  const [targetSession, setTargetSession] = useState<string | null>(selection?.kind === 'session' ? selection.id : null);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

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

  const send = async () => {
    const text = task.trim();
    if (!text || state === 'sending') return;
    const cwdClean = cwd.trim();
    if (cwdClean && !cwdClean.startsWith('/')) { setCwdHint(true); return; }
    setState('sending');
    try { localStorage.setItem('notch.cwd', cwdClean); localStorage.setItem('notch.model', model); } catch { /* storage may be unavailable */ }
    const res = await submitTask({
      task: text,
      cwd: cwdClean || undefined,
      definitionOfDone: dod.trim() || undefined,
      model: model || undefined,
      supervisorId: targetSupervisorId,
    });
    if (res.ok) {
      setTask(''); setDod(''); setCwdHint(false);
      setState({ ok: true });
      window.setTimeout(() => setState('idle'), 1500);
      window.setTimeout(autosize, 0);
    } else {
      setState({ ok: false, error: res.error ?? 'submit_task failed — see logs' });
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); e.currentTarget.focus(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); send(); }
  };

  const assigned = state !== 'idle' && state !== 'sending' && state.ok;
  const error = state !== 'idle' && state !== 'sending' && !state.ok ? state.error : null;

  return (
    <div className="composer">
      {targetChipLabel && (
        <div className="composer-target">
          <span className="mono text-[10.5px]" style={{ color: 'rgba(255,255,255,.5)' }}>→ {targetChipLabel}</span>
          <button className="chip-clear" aria-label="Clear target — assign to a new supervisor" title="Clear target"
            onClick={() => setTargetSession(null)}>
            <CloseIcon size={10} />
          </button>
        </div>
      )}
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
      <div className="composer-dod">
        <button className="linklike" onClick={() => setDodOpen((v) => !v)} aria-expanded={dodOpen}>
          <ChevronDownIcon size={10} className={dodOpen ? 'rotate-180' : ''} />
          Done when…
        </button>
        {dodOpen && (
          <input className="dod-input" value={dod} onChange={(e) => setDod(e.target.value)}
            placeholder="tests pass, docs updated, evidence pasted" aria-label="Definition of done" />
        )}
      </div>
      <div className="composer-row">
        <label className="cwd-field">
          <span className="mono" style={{ color: 'rgba(255,255,255,.34)' }}>cwd</span>
          <input value={cwd} onChange={(e) => { setCwd(e.target.value); setCwdHint(false); }}
            onBlur={() => setCwdHint(cwd.trim().length > 0 && !cwd.trim().startsWith('/'))}
            spellCheck={false} aria-label="Working directory" />
        </label>
        <div ref={menuRef} className="model-picker">
          <button className="model-btn" onClick={() => setModelOpen((v) => !v)} aria-haspopup="listbox" aria-expanded={modelOpen}>
            <span className="mono">{model}</span>
            <ChevronDownIcon size={11} />
          </button>
          {modelOpen && (
            <ul className="model-menu" role="listbox" aria-label="Model">
              {models.map((m) => (
                <li key={m} role="option" aria-selected={m === model}>
                  <button className="model-item" onClick={() => { setModel(m); setModelOpen(false); }}>
                    <span className="mono">{m}</span>
                    {m === model && <CheckIcon size={11} />}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <button className="assign-btn" onClick={send} disabled={state === 'sending' || !task.trim()}>
          {state === 'sending' ? 'Assigning…' : 'Assign'}
        </button>
      </div>
      {cwdHint && <div className="composer-hint">absolute path expected (e.g. /Users/you/project)</div>}
      {error && <div className="composer-error" role="alert">{error}</div>}
      {assigned && <div className="composer-ok"><CheckIcon size={12} /> Assigned — supervisor is on it</div>}
    </div>
  );
}
