import { useCallback, useEffect, useRef, useState } from 'react';
import { useSnapshot } from '../lib/use-snapshot';
import { send, submitTask } from '../lib/bridge';
import type { Supervisor } from '../lib/bridge';
import { topStatus, workingCount } from '../lib/format';
import { TreeTab, type Selection } from './dashboard/TreeTab';
import { MultiplexerTab } from './dashboard/MultiplexerTab';
import { CommsTab } from './dashboard/CommsTab';
import { EscalationsTab } from './dashboard/EscalationsTab';
import { PixelIcon, type PixelIconName } from './PixelIcon';
import { FleetDeck } from './fleet/FleetDeck';

type Tab = 'tree' | 'multiplexer' | 'comms' | 'escalations';

const TABS: { id: Tab; label: string; title: string; icon: PixelIconName }[] = [
  { id: 'tree', label: 'Tree', title: 'Tree', icon: 'tree' },
  { id: 'multiplexer', label: 'Mux', title: 'Mux', icon: 'grid' },
  { id: 'comms', label: 'Comms', title: 'Comms', icon: 'bubble' },
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
  // W2-5: opt-in fleet surface. Default is absent from the DOM, so every
  // existing tab renders the exact same pixels as before this change.
  const [fleetOpen, setFleetOpen] = useState(() =>
    new URLSearchParams(window.location.search).get('fleet') === '1');
  const armTimer = useRef(0);
  useEffect(() => () => window.clearTimeout(armTimer.current), []);
  const initialPicked = useRef(false);

  const sessions = snapshot?.sessions ?? [];
  const supervisors = snapshot?.supervisors ?? [];
  const escalations = snapshot?.escalations ?? [];
  const top = topStatus(sessions);
  const active = workingCount(sessions);


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
    if (!snapshot) return;
    const recoverTarget = () => {
      const handoff = readStorage<Selection>('notch.handoff');
      if (!handoff) return;
      try { localStorage.removeItem('notch.handoff'); } catch { /* storage may be unavailable */ }
      if ((handoff.kind === 'session' && snapshot.sessions.some(({ id }) => id === handoff.id))
        || (handoff.kind === 'supervisor' && snapshot.supervisors.some(({ id }) => id === handoff.id))) {
        select(handoff);
        switchTab('tree');
      }
    };
    recoverTarget();
    const onStorage = (event: StorageEvent) => { if (event.key === 'notch.handoff') recoverTarget(); };
    window.addEventListener('storage', onStorage);
    window.addEventListener('focus', recoverTarget);
    return () => {
      window.removeEventListener('storage', onStorage);
      window.removeEventListener('focus', recoverTarget);
    };
  }, [snapshot, select, switchTab]);

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

  const togglePause = useCallback((sup: Supervisor) =>
    send({ cmd: sup.paused ? 'resume_supervisor' : 'pause_supervisor', id: sup.id }), []);
  const killSession = useCallback((id: string) => send({ cmd: 'kill', id }), []);
  const killSupervisor = useCallback((id: string) => {
    for (const s of sessions) if (s.sup === id) send({ cmd: 'kill', id: s.id });
  }, [sessions]);
  const openHerdr = useCallback((id: string) => send({ cmd: 'focus', id }), []);

  // kill switch: first click arms (2.6 s), second click fires the one-shot wave
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
    { value: active, unit: 'in flight' },
    { value: rate, unit: 'ev/s', precision: 1 },
    { value: supervisors.length, unit: supervisors.length === 1 ? 'supervisor' : 'supervisors' },
  ].filter(({ value }) => value > 0);


  return (
    <div className="dash">
      <header className="dashboard-masthead" data-tauri-drag-region>
        <div className="dashboard-brand" aria-label="Notch">
          <span className="brand-logo" aria-hidden="true" />
          <span>Notch</span>
        </div>
        <nav className="dashboard-tabs" role="tablist" aria-label="Dashboard tabs">
          {TABS.map((item, index) => (
            <button key={item.id} role="tab" aria-selected={tab === item.id}
              tabIndex={tab === item.id ? 0 : -1} className="dashboard-tab"
              onClick={() => { switchTab(item.id); setFleetOpen(false); }}
              title={`${item.label} · ⌘${index + 1}`}
              onKeyDown={(event) => {
                const delta = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
                if (!delta) return;
                const next = (index + delta + TABS.length) % TABS.length;
                const target = TABS[next];
                if (!target) return;
                event.preventDefault();
                switchTab(target.id);
                event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role=tab]')[next]?.focus();
              }}>
              <PixelIcon name={item.icon} size={24} />
              <span>{item.label}</span>
              {item.id === 'escalations' && escalations.length > 0 && <span className="dashboard-tab-count" aria-label={`${escalations.length} unresolved alarms`}>{escalations.length}</span>}
            </button>
          ))}
        </nav>
        <div className="dashboard-controls">
          <span className="dashboard-status"><i className={`dot dot--${top}`} />{top[0]?.toUpperCase()}{top.slice(1)}</span>
          <button className="dashboard-fleet" aria-pressed={fleetOpen} onClick={() => setFleetOpen((current) => !current)} title={snapshot?.warnings.length ? "Fleet has warnings · ⌘5" : "Fleet · ⌘5"}>
            <PixelIcon name={snapshot?.warnings.length ? "alert" : "orchestrator"} size={12} />Fleet
          </button>
        </div>
      </header>
      <div className="stage">
          <header className="topbar" data-tauri-drag-region>
            <h1>{view.title}</h1>
            <div className="topbar-spacer" data-tauri-drag-region />
            {sessions.length > 0 && readouts.length > 0 && (
              <div className="stats" aria-label="Live counters">
                {readouts.map(({ value, unit, precision }) => (
                  <span className="stat" key={unit}>
                    <span className="stat-v" key={value}>{value.toFixed(precision ?? 0)}</span>
                    <span className="label">{unit}</span>
                  </span>
                ))}
              </div>
            )}
            <button className="killswitch" disabled={active === 0}
              onClick={killSwitch} aria-pressed={armed && active > 0}
              title="Close all managed panes and stop running sessions">
              <PixelIcon name="power" size={12} />
              {armed && active > 0 ? 'Confirm kill' : 'Kill switch'}
            </button>
          </header>

          <main key={tab} className={`view view--${tab}`} role="tabpanel" aria-label={`${view.label} view`}>
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
                  if (sessions.some((session) => session.id === id)) {
                    jumpToSession(id, id);
                  } else if (supervisors.some((supervisor) => supervisor.id === id)) {
                    switchTab('tree');
                    select({ kind: 'supervisor', id });
                  }
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
                  onOpenSession={jumpToSession}
                />
              )}
          </main>


        </div>

        {/* W2-5: fleet management surface, composed around the four owned tabs. */}
        {fleetOpen && <FleetDeck snapshot={snapshot} onClose={() => setFleetOpen(false)} />}
      </div>

  );
}
