// Dashboard.tsx — the macOS window. Rail (navigation) · top strip (live instruments) · view.
// The dither is the brand: a 3px ordered-dither rule under the strip, dithered card crowns,
// one halftone sphere in the empty states. Nothing is rounded, nothing glows.
// Cmd+1..4 tabs · Up/Down on the rail · Cmd+K composer · two-step kill switch.
import { useCallback, useEffect, useRef, useState } from 'react';
import { AnimatePresence, MotionConfig, motion } from 'motion/react';
import { DistortionWave, type PaletteName } from '../vendor/distortion-waves/src';
import { useSnapshot } from '../lib/use-snapshot';
import { send, submitTask } from '../lib/bridge';
import type { Supervisor } from '../lib/bridge';
import { topStatus, normalizeEps, workingCount } from '../lib/format';
import { TreeTab, type Selection } from './dashboard/TreeTab';
import { MultiplexerTab } from './dashboard/MultiplexerTab';
import { CommsTab } from './dashboard/CommsTab';
import { EscalationsTab } from './dashboard/EscalationsTab';
import { PixelIcon, type PixelIconName } from './PixelIcon';
import { useWhiteHot } from '../lib/hot';
import { PixelMeter } from './pixel/PixelMeter';
import { DitherField } from './pixel/DitherField';
import { FleetDeck } from './fleet/FleetDeck';

type Tab = 'tree' | 'multiplexer' | 'comms' | 'escalations';

const TABS: { id: Tab; label: string; title: string; icon: PixelIconName }[] = [
  { id: 'tree', label: 'Tree', title: 'Control plane', icon: 'orchestrator' },
  { id: 'multiplexer', label: 'Mux', title: 'Session array', icon: 'grid' },
  { id: 'comms', label: 'Comms', title: 'The wire', icon: 'bubble' },
  { id: 'escalations', label: 'Alarms', title: 'Alarms', icon: 'alert' },
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
  const [fired, setFired] = useState(false);
  const [killBtn, setKillBtn] = useState<HTMLButtonElement | null>(null);
  // W2-5: opt-in fleet surface. Default is absent from the DOM, so every
  // existing tab renders the exact same pixels as before this change.
  const [fleetOpen, setFleetOpen] = useState(() =>
    new URLSearchParams(window.location.search).get('fleet') === '1');
  const armTimer = useRef(0);
  const initialPicked = useRef(false);
  const dashRef = useRef<HTMLDivElement>(null);
  useWhiteHot(dashRef);

  const sessions = snapshot?.sessions ?? [];
  const supervisors = snapshot?.supervisors ?? [];
  const escalations = snapshot?.escalations ?? [];
  const top = topStatus(sessions);
  const active = workingCount(sessions);
  const load = normalizeEps(snapshot?.eps ?? 0);

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
      if ((e.metaKey || e.ctrlKey) && e.key === '5') {
        e.preventDefault();
        setFleetOpen((v) => !v);
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

  // kill switch: first click arms (2.6 s), second click fires the one-shot wave
  const killSwitch = useCallback(() => {
    if (!armed) {
      setArmed(true);
      armTimer.current = window.setTimeout(() => setArmed(false), 2600);
      return;
    }
    window.clearTimeout(armTimer.current);
    setArmed(false);
    setFired(true);
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

  const view = TABS.find((t) => t.id === tab) ?? TABS[0]!;
  const sub = snapshot
    ? `${supervisors.length} supervisor${supervisors.length === 1 ? '' : 's'} · ${sessions.length} session${sessions.length === 1 ? '' : 's'} · ${snapshot.eps.toFixed(1)} ev/s`
    : 'Connecting to notchd…';

  return (
    <MotionConfig reducedMotion="user">
      <div className="dash" ref={dashRef}>
        {/* ---------- rail ---------- */}
        <nav className="rail" aria-label="Dashboard">
          <div className="rail-mark" title="notch">
            <DitherField mode="linear" angle={135} cell={2} floor={0.08} ceil={0.62} />
            <span className="brand-logo" aria-hidden="true" />
          </div>
          <div className="rail-nav" role="tablist" aria-label="Dashboard tabs" aria-orientation="vertical">
            {TABS.map((t) => {
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
                  title={t.label}
                  onKeyDown={(e) => {
                    const order = TABS.map((x) => x.id);
                    const i = order.indexOf(tab);
                    if (e.key === 'ArrowDown') { e.preventDefault(); switchTab(order[(i + 1) % order.length]!); }
                    if (e.key === 'ArrowUp') { e.preventDefault(); switchTab(order[(i - 1 + order.length) % order.length]!); }
                  }}
                >
                  <span className="rail-glyph"><PixelIcon name={t.icon} size={17} /></span>
                  <span>{t.label}</span>
                  {isEsc && <span className="rail-badge" aria-label={`${escalations.length} unresolved escalations`}>{escalations.length}</span>}
                </button>
              );
            })}
          </div>
          <div className="rail-foot">
            <span className="rail-live"><i className={`dot dot--${top}`} aria-hidden="true" />{top}</span>
            <span className="rail-rate">{snapshot?.eps.toFixed(1) ?? '0.0'}<em>ev/s</em></span>
          </div>
        </nav>

        {/* ---------- stage ---------- */}
        <div className="stage">
          <header className="topbar" data-tauri-drag-region>
            <div className="topbar-titles">
              <div className="topbar-title">
                <h1>{view.title}</h1>
              </div>
              <div className="topbar-sub">{sub}</div>
            </div>
            <div className="topbar-spacer" data-tauri-drag-region />

            <div className="stats" aria-label="Live counters">
              <div className="stat">
                <span className="label">In flight</span>
                <span className="stat-v">{String(active).padStart(2, '0')}</span>
              </div>
              <div className="stat stat--meter">
                <span className="label">Events / sec</span>
                <span className="stat-v">
                  {snapshot?.eps.toFixed(1) ?? '0.0'}
                  <PixelMeter value={load} cells={14} tone="hot" label="Event rate" />
                </span>
              </div>
              <div className="stat stat--optional">
                <span className="label">Supervisors</span>
                <span className="stat-v">{supervisors.length}</span>
              </div>
              <div className="stat stat--optional">
                <span className="label">Sessions</span>
                <span className="stat-v">{sessions.length}</span>
              </div>
            </div>

            <DistortionWave
              className="kill-wave"
              revealed={fired}
              type="center"
              palette={'sunset' as PaletteName}
              scale="auto"
              cornerRadius={0}
              prewarm={false}
              source={killBtn}
              overlay={
                <button
                  ref={setKillBtn}
                  className="killswitch ripple"
                  data-armed={armed ? '' : undefined}
                  onClick={killSwitch}
                  title="Close every managed pane, mark all sessions failed, clean up"
                >
                  <PixelIcon name="trash" size={13} />
                  <span>{armed ? 'Confirm — kill all' : 'Kill switch'}</span>
                </button>
              }
              revealedContent={<div className="kill-done" role="status">All sessions stopped</div>}
            >
              <span className="killswitch killswitch--ghost">
                <PixelIcon name="trash" size={13} />
                <span>Kill switch</span>
              </span>
            </DistortionWave>
            <DitherField className="topbar-rule" mode="linear" angle={90} cell={2} floor={0.42} />
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

          <footer className="statusline" aria-label="Status">
            <span className="sl-item"><i className={`dot dot--${top}`} aria-hidden="true" />{top}</span>
            <span className="sl-item">{active} in flight</span>
            <span className="sl-item">{sessions.length} sessions</span>
            <span className="sl-item">{supervisors.length} supervisors</span>
            <span className="sl-dither"><DitherField mode="linear" angle={0} cell={2} floor={0.18} /></span>
            <span className="sl-keys">⌘1–4 views · ⌘K compose</span>
          </footer>
        </div>

        {/* W2-5: fleet management surface, composed around the four owned tabs. */}
        {fleetOpen && <FleetDeck snapshot={snapshot} onClose={() => setFleetOpen(false)} />}
      </div>
    </MotionConfig>
  );
}
