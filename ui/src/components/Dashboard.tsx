import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { AnimatePresence, MotionConfig, motion } from 'motion/react';
import { useSnapshot } from '../lib/use-snapshot';
import { send, submitTask } from '../lib/bridge';
import type { Supervisor } from '../lib/bridge';
import { topStatus, workingCount } from '../lib/format';
import { TreeTab, type Selection } from './dashboard/TreeTab';
import { MultiplexerTab } from './dashboard/MultiplexerTab';
import { CommsTab } from './dashboard/CommsTab';
import { EscalationsTab } from './dashboard/EscalationsTab';
import { PixelIcon, type PixelIconName } from './PixelIcon';
import { useRipple } from '../lib/use-ripple';
import { FleetDeck } from './fleet/FleetDeck';

type Tab = 'tree' | 'multiplexer' | 'comms' | 'escalations';

const TABS: { id: Tab; label: string; icon: PixelIconName }[] = [
  { id: 'tree', label: 'Tree', icon: 'tree' },
  { id: 'multiplexer', label: 'Mux', icon: 'grid' },
  { id: 'comms', label: 'Comms', icon: 'bubble' },
  { id: 'escalations', label: 'Alarms', icon: 'alert' },
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
  const [killNotice, setKillNotice] = useState<string | null>(null);
  const [fleetOpen, setFleetOpen] = useState(() =>
    new URLSearchParams(window.location.search).get('fleet') === '1');
  const armTimer = useRef(0);
  const noticeTimer = useRef(0);
  useEffect(() => () => {
    window.clearTimeout(armTimer.current);
    window.clearTimeout(noticeTimer.current);
  }, []);
  const initialPicked = useRef(false);
  const dashRef = useRef<HTMLDivElement>(null);
  useRipple(dashRef);

  const sessions = snapshot?.sessions ?? [];
  const supervisors = snapshot?.supervisors ?? [];
  const escalations = snapshot?.escalations ?? [];
  const top = topStatus(sessions);
  const active = workingCount(sessions);

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
    if (t !== tab) send({ cmd: 'haptic', kind: 'levelChange' });
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
        switchTab('tree');
        setComposerFocusSignal((n) => n + 1);
      }
      if ((e.metaKey || e.ctrlKey) && e.key === '5') {
        e.preventDefault();
        setFleetOpen((v) => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [switchTab]);

  // Re-prompt changes the running session in place; it is not a new assignment.
  const repromptSession = useCallback((id: string, overrideText?: string) => {
    const s = sessions.find((x) => x.id === id);
    send({ cmd: 'reprompt', id, text: overrideText ?? s?.goal ?? s?.task ?? s?.title ?? 'continue' });
  }, [sessions]);

  const togglePause = useCallback((sup: Supervisor) =>
    send({ cmd: sup.paused ? 'resume_supervisor' : 'pause_supervisor', id: sup.id }), []);
  const killSession = useCallback((id: string) => send({ cmd: 'kill', id }), []);
  const killSupervisor = useCallback((id: string) => {
    for (const s of sessions) if (s.sup === id) send({ cmd: 'kill', id: s.id });
  }, [sessions]);
  const openHerdr = useCallback((id: string) => send({ cmd: 'focus', id }), []);

  const killSwitch = useCallback(() => {
    if (active === 0) return;
    if (!armed) {
      setArmed(true);
      armTimer.current = window.setTimeout(() => setArmed(false), 2600);
      return;
    }
    window.clearTimeout(armTimer.current);
    setArmed(false);
    send({ cmd: 'kill_switch' });
    setKillNotice('Kill switch sent — stopping all sessions');
    window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setKillNotice(null), 3200);
  }, [active, armed]);

  const jumpToSession = useCallback((sessionId: string, flash?: string) => {
    switchTab('tree');
    select({ kind: 'session', id: sessionId });
    if (flash) { setFlashId(flash); window.setTimeout(() => setFlashId(null), 1200); }
  }, [select, switchTab]);

  const onReassign = useCallback(async (sup: string, goal: string, dod: string | undefined, model?: string) => {
    const res = await submitTask({ task: goal, supervisorId: sup, definitionOfDone: dod, model });
    return res.ok ? (res.sessionId ?? 'assigned') : null;
  }, []);

  const view = TABS.find((t) => t.id === tab) ?? TABS[0]!;
  const rate = snapshot?.eps ?? 0;
  const readouts = [
    { value: active, unit: 'in flight', key: 'inflight' },
    { value: rate, unit: 'ev/s', precision: 1, key: 'evs' },
    { value: supervisors.length, unit: supervisors.length === 1 ? 'supervisor' : 'supervisors', key: 'sup' },
  ].filter(({ value }) => value > 0);
  const railIndex = TABS.findIndex((item) => item.id === tab);

  return (
    <MotionConfig reducedMotion="user">
      <div className="dash" ref={dashRef}>
        <nav className="rail" aria-label="Dashboard">
          <div className="rail-mark" title="notch">
            <span className="brand-logo" aria-hidden="true" />
          </div>
          <div className="rail-nav" role="tablist" aria-label="Dashboard tabs" aria-orientation="vertical">
            <span className="rail-indicator" style={{ "--i": railIndex } as CSSProperties} aria-hidden="true" />
            {TABS.map((t, index) => {
              const isActive = tab === t.id;
              const isEsc = t.id === 'escalations' && escalations.length > 0;
              return (
                <button
                  key={t.id}
                  role="tab"
                  aria-selected={isActive}
                  tabIndex={isActive ? 0 : -1}
                  className={`rail-item ripple${isActive ? ' is-active' : ''}`}
                  onClick={() => switchTab(t.id)}
                  title={`${t.label} · ⌘${index + 1} · ⌘K compose`}
                  data-ripple
                  onKeyDown={(e) => {
                    const order = TABS.map((x) => x.id);
                    const i = order.indexOf(tab);
                    if (e.key === 'ArrowDown') { e.preventDefault(); switchTab(order[(i + 1) % order.length]!); }
                    if (e.key === 'ArrowUp') { e.preventDefault(); switchTab(order[(i - 1 + order.length) % order.length]!); }
                  }}
                >
                  <PixelIcon name={t.icon} size={24} />
                  <span className="rail-label">{t.label}</span>
                  {isEsc && <span className="rail-badge" aria-label={`${escalations.length} unresolved escalations`}>{escalations.length}</span>}
                </button>
              );
            })}
          </div>
          <div className="rail-foot" data-tauri-drag-region>
            <span className="rail-live" aria-label="Status">
              <i className={`dot dot--${top}`} aria-hidden="true" />
              {top === 'idle' ? 'Idle' : top[0]?.toUpperCase() + top.slice(1)}
            </span>
          </div>
        </nav>

        <div className="stage">
          <header className="topbar" data-tauri-drag-region>
            <h1>{view.label}</h1>
            {(snapshot?.warnings?.length ?? 0) > 0 && (
              <span className="topbar-warn" role="status" title={snapshot!.warnings.join(' · ')}
                aria-label={`${snapshot!.warnings.length} warnings`}>
                <PixelIcon name="alert" size={12} />{snapshot!.warnings.length}
              </span>
            )}
            <div className="topbar-spacer" data-tauri-drag-region />
            {sessions.length === 0 ? (
              <span className="topbar-idle" aria-label="Idle">
                <i className="live-dot live-dot--idle" aria-hidden="true" />
                Idle
              </span>
            ) : readouts.length > 0 ? (
              <div className="stats" aria-label="Live counters">
                {readouts.map(({ value, unit, precision, key }) => (
                  <span className="stat" key={key}>
                    <span className="stat-num tabular-nums" key={value}>{value.toFixed(precision ?? 0)}</span>
                    <span className="stat-unit">{unit}</span>
                  </span>
                ))}
              </div>
            ) : null}
            <button className="killswitch ripple" disabled={active === 0}
              onClick={killSwitch} aria-pressed={armed && active > 0}
              title="Close all managed panes and stop running sessions">
              <PixelIcon name="power" size={24} />
              {armed && active > 0 ? 'Confirm kill' : 'Kill switch'}
            </button>
          </header>

          <AnimatePresence mode="wait" initial={false}>
            <motion.main
              key={tab}
              className="view"
              role="tabpanel"
              aria-label={`${view.label} view`}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -3 }}
              transition={{ duration: 0.14, ease: [0.16, 1, 0.3, 1] }}
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
                  onReprompt={repromptSession}
                  onFocusComposerRequest={() => setComposerFocusSignal((n) => n + 1)}
                  onPrefill={(text) => {
                    setComposerPrefill({ text, at: Date.now() });
                    setComposerFocusSignal((n) => n + 1);
                  }}
                  composerPrefill={composerPrefill}
                  composerFocusSignal={composerFocusSignal}
                  flashId={flashId}
                />
              )}
              {tab === 'multiplexer' && <MultiplexerTab snapshot={snapshot} onJumpToTree={(id) => jumpToSession(id)} onAssign={() => { switchTab('tree'); setComposerFocusSignal((n) => n + 1); }} />}
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
                    repromptSession(sessionId, `Fresh take on the goal: ${goal}`);
                    setComposerPrefill({ text: `Fresh take on the goal: ${goal}`, at: Date.now() });
                  }}
                  onReassign={onReassign}
                  onKill={killSession}
                  onOpenSession={jumpToSession}
                />
              )}
            </motion.main>
          </AnimatePresence>
        </div>

        {fleetOpen && <FleetDeck snapshot={snapshot} onClose={() => setFleetOpen(false)} />}
        {killNotice && <div className="dash-notice" role="status">{killNotice}</div>}
      </div>
    </MotionConfig>
  );
}
