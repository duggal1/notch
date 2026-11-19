// Dashboard.tsx — the macOS window. No gray title bar: the native bar is overlaid and this header IS the top edge.
// Cmd+1..4 tabs · arrows on the tablist · Cmd+K composer · two-step kill switch.
import { useCallback, useEffect, useRef, useState } from 'react';
import { AnimatePresence, MotionConfig, motion } from 'motion/react';
import { useSnapshot } from '../lib/use-snapshot';
import { send, submitTask } from '../lib/bridge';
import type { Supervisor } from '../lib/bridge';
import { TreeTab, type Selection } from './dashboard/TreeTab';
import { MultiplexerTab } from './dashboard/MultiplexerTab';
import { CommsTab } from './dashboard/CommsTab';
import { EscalationsTab } from './dashboard/EscalationsTab';
import { TrashIcon } from './Icons';
import { Sym } from './Symbol';
import { SPRING_REACT } from '../lib/tokens';
import { useWhiteHot } from '../lib/hot';

type Tab = 'tree' | 'multiplexer' | 'comms' | 'escalations';
const TABS: { id: Tab; label: string }[] = [
  { id: 'tree', label: 'Tree' },
  { id: 'multiplexer', label: 'Multiplexer' },
  { id: 'comms', label: 'Comms' },
  { id: 'escalations', label: 'Escalations' },
];

function readStorage<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch { return null; }
}

export function Dashboard() {
  const snapshot = useSnapshot();
  const [tab, setTab] = useState<Tab>(() => readStorage<Tab>('notch.dashboardTab') ?? 'tree');
  const [selection, setSelection] = useState<Selection>(null);
  const [composerPrefill, setComposerPrefill] = useState<{ text: string; at: number } | null>(null);
  const [composerFocusSignal, setComposerFocusSignal] = useState(0);
  const [flashId, setFlashId] = useState<string | null>(null);
  const [armed, setArmed] = useState(false);
  const armTimer = useRef(0);
  const initialPicked = useRef(false);
  const dashRef = useRef<HTMLDivElement>(null);
  useWhiteHot(dashRef);

  const sessions = snapshot?.sessions ?? [];
  const supervisors = snapshot?.supervisors ?? [];
  const escalations = snapshot?.escalations ?? [];

  // target handoff from the popover: consume once
  useEffect(() => {
    if (!snapshot || initialPicked.current) return;
    initialPicked.current = true;
    const t = readStorage<{ kind: 'supervisor' | 'session'; id: string }>('notch.target');
    if (t?.kind === 'session' && sessions.some((s) => s.id === t.id)) { setSelection(t); return; }
    if (t?.kind === 'supervisor' && supervisors.some((s) => s.id === t.id)) { setSelection(t); return; }
    const hot = sessions.find((s) => s.status === 'working');
    setSelection(hot ? { kind: 'session', id: hot.id } : null);
  }, [snapshot, sessions, supervisors]);

  const select = useCallback((sel: Selection) => {
    setSelection(sel);
    try { localStorage.setItem('notch.target', JSON.stringify(sel ?? { kind: 'none' })); } catch { /* storage may be unavailable */ }
  }, []);

  const switchTab = useCallback((t: Tab) => {
    setTab(t);
    try { localStorage.setItem('notch.dashboardTab', JSON.stringify(t)); } catch { /* storage may be unavailable */ }
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && ['1', '2', '3', '4'].includes(e.key)) {
        e.preventDefault();
        switchTab(TABS[+e.key - 1]?.id ?? 'tree');
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setComposerFocusSignal((n) => n + 1);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [switchTab]);

  const togglePause = useCallback((sup: Supervisor) =>
    send({ cmd: sup.paused ? 'resume_supervisor' : 'pause_supervisor', id: sup.id }), []);
  const killSession = useCallback((id: string) => send({ cmd: 'kill', id }), []);
  const killSupervisor = useCallback((id: string) => {
    for (const s of sessions) if (s.sup === id) send({ cmd: 'kill', id: s.id });
  }, [sessions]);
  const openHerdr = useCallback((id: string) => send({ cmd: 'focus', id }), []);

  // kill switch: first click arms (2.6 s), second click fires
  const killSwitch = useCallback(() => {
    if (!armed) {
      setArmed(true);
      armTimer.current = window.setTimeout(() => setArmed(false), 2600);
      return;
    }
    window.clearTimeout(armTimer.current);
    setArmed(false);
    send({ cmd: 'kill_switch' });
  }, [armed]);

  const jumpToSession = useCallback((sessionId: string, flash?: string) => {
    switchTab('tree');
    select({ kind: 'session', id: sessionId });
    if (flash) { setFlashId(flash); window.setTimeout(() => setFlashId(null), 1200); }
  }, [select, switchTab]);

  const onReassign = useCallback(async (sup: string, goal: string, dod: string | undefined, model?: string) => {
    const res = await submitTask({ task: goal, supervisorId: sup, definitionOfDone: dod, model });
    return res.ok ? (res.sessionId ?? 'assigned') : null;
  }, []);

  return (
    <MotionConfig reducedMotion="user">
      <div className="dash" ref={dashRef}>
        <header className="dash-head" data-tauri-drag-region>
          <div className="seg" role="tablist" aria-label="Dashboard tabs">
            {TABS.map((t) => {
              const active = tab === t.id;
              const isEsc = t.id === 'escalations' && escalations.length > 0;
              return (
                <button
                  key={t.id}
                  role="tab"
                  aria-selected={active}
                  tabIndex={active ? 0 : -1}
                  className={`seg-item${active ? ' is-active' : ''}`}
                  onClick={() => switchTab(t.id)}
                  onKeyDown={(e) => {
                    const order = TABS.map((x) => x.id);
                    const i = order.indexOf(tab);
                    if (e.key === 'ArrowRight') { e.preventDefault(); switchTab(order[(i + 1) % order.length]!); }
                    if (e.key === 'ArrowLeft') { e.preventDefault(); switchTab(order[(i - 1 + order.length) % order.length]!); }
                  }}
                >
                  {active && <motion.span layoutId="seg-pill" className="seg-pill" transition={{ type: 'spring', ...SPRING_REACT.hover }} />}
                  <span className="seg-label">{t.label}</span>
                  {isEsc && (
                    <span className="seg-badge" aria-label={`${escalations.length} unresolved`}>
                      <i className="live-dot live-dot--failed" />{escalations.length}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
          <div className="dash-spacer" data-tauri-drag-region />
          <div className="dash-head-right">
            <span className="dash-eps">{snapshot?.eps.toFixed(1) ?? '0.0'} ev/s</span>
            <button className="killswitch" data-armed={armed ? '' : undefined} onClick={killSwitch}
              title="Close every managed pane, mark all sessions failed, clean up">
              <Sym name="trash" pt={14} fallback={TrashIcon} />
              <span>{armed ? 'Click again to kill all' : 'Kill switch'}</span>
            </button>
          </div>
        </header>

        <AnimatePresence mode="wait" initial={false}>
          <motion.main
            key={tab}
            className="dash-body"
            role="tabpanel"
            aria-label={`${TABS.find((t) => t.id === tab)?.label} tab`}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={{ duration: 0.18, ease: [0.16, 1, 0.3, 1] }}
          >
            {tab === 'tree' && (
              <TreeTab
                snapshot={snapshot}
                selection={selection}
                onSelect={select}
                onTogglePause={togglePause}
                onKillSupervisor={killSupervisor}
                onKillSession={killSession}
                onOpenHerdr={openHerdr}
                onFocusComposerRequest={() => setComposerFocusSignal((n) => n + 1)}
                composerPrefill={composerPrefill}
                composerFocusSignal={composerFocusSignal}
                flashId={flashId}
              />
            )}
            {tab === 'multiplexer' && <MultiplexerTab snapshot={snapshot} onJumpToTree={(id) => jumpToSession(id)} />}
            {tab === 'comms' && (
              <CommsTab snapshot={snapshot} onJumpToTree={(id) => {
                const toFlash = sessions.find((s) => s.id === id)?.sup;
                jumpToSession(id, toFlash ? id : undefined);
              }} />
            )}
            {tab === 'escalations' && (
              <EscalationsTab
                snapshot={snapshot}
                onRepromptFresh={(sessionId, goal) => {
                  switchTab('tree');
                  select({ kind: 'session', id: sessionId });
                  setComposerPrefill({ text: `Fresh take on the goal: ${goal}`, at: Date.now() });
                  setComposerFocusSignal((n) => n + 1);
                }}
                onReassign={onReassign}
                onKill={killSession}
              />
            )}
          </motion.main>
        </AnimatePresence>
      </div>
    </MotionConfig>
  );
}