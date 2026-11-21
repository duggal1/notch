// FleetDeck.tsx — opt-in fleet management surface, composed AROUND the existing
// dashboard tabs. Structure only is stolen from agentbox (one derived fleet,
// one honest line per unit, one decision on stage, display-only activity);
// every class and CSS variable here is already ours. No stylesheet changes.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Snapshot } from '../../lib/bridge';
import { isTauri, send } from '../../lib/bridge';
import { PixelIcon } from '../PixelIcon';
import {
  loadFleet, registerFleetEvents, sendFleet, EMPTY_FLEET_STATE,
  type FleetCommand, type FleetData, type FleetUnit,
} from '../../lib/fleet-wiring';
import { DitherField } from '../pixel/DitherField';
import {
  deriveActivity, firstUnitOf, fmtStageAge, stageQueue, stepStage,
} from './derive';

export function FleetDeck({ snapshot, onClose }: { snapshot: Snapshot | null; onClose: () => void }) {
  const [data, setData] = useState<FleetData>(() => ({
    state: EMPTY_FLEET_STATE, approvals: [], harnesses: [],
  }));
  const [actionError, setActionError] = useState('');
  const [stageIndex, setStageIndex] = useState(0);
  const deckRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let alive = true;
    let receivedEvent = false;
    const stop = registerFleetEvents((state, approvals) => {
      if (!alive) return;
      receivedEvent = true;
      setData((prev) => ({ ...prev, state, approvals: approvals ?? prev.approvals }));
    });
    loadFleet().then((next) => {
      if (alive) {
        setData((prev) => receivedEvent ? { ...prev, harnesses: next.harnesses } : next);
      }
    });
    return () => { alive = false; stop(); };
  }, []);

  const units = data.state.units;
  const activity = useMemo(() => deriveActivity(snapshot), [snapshot]);
  const cards = useMemo(() => stageQueue(data.approvals, units), [data.approvals, units]);
  const card = cards[Math.min(stageIndex, Math.max(0, cards.length - 1))] ?? null;
  const supervisor = snapshot?.supervisors[0]?.id;
  const warning = actionError || snapshot?.warnings.at(-1);
  const harness = data.harnesses[0];

  const act = useCallback((command: FleetCommand) => {
    setActionError('');
    void sendFleet(command).catch((error: unknown) => {
      setActionError(typeof error === 'string' ? error : 'Fleet command failed.');
    });
  }, []);

  const answer = useCallback((allow: boolean) => {
    if (!card) return;
    act({ cmd: 'answer_approval', id: card.id, allow, digest: card.digest });
  }, [act, card]);

  const advance = useCallback((dir: 1 | -1) => {
    setStageIndex((i) => stepStage(cards, i, dir));
  }, [cards]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === 'Escape') { e.preventDefault(); onClose(); return; }
      if (e.key === 'ArrowRight') { e.preventDefault(); advance(1); return; }
      if (e.key === 'ArrowLeft') { e.preventDefault(); advance(-1); return; }
      if (e.key === '1') { e.preventDefault(); answer(true); return; }
      if (e.key === '2') { e.preventDefault(); answer(false); return; }
      if (e.key === '3') {
        e.preventDefault();
        if (card) send({ cmd: 'kill', id: card.session });
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [advance, answer, card, onClose]);

  const groups: { label: string; rows: FleetUnit[] }[] = [
    { label: 'Working', rows: units.filter((u) => u.word === 'Working') },
    { label: 'Stopped', rows: units.filter((u) => u.word === 'Stopped') },
    { label: 'Idle', rows: units.filter((u) => u.word === 'Idle') },
    { label: 'Done', rows: units.filter((u) => u.word === 'Done') },
  ];

  return (
    <div
      className="fleet-deck"
      ref={deckRef}
      role="region"
      aria-label="Fleet control"
      style={{ position: 'absolute', inset: 0, zIndex: 50, background: 'var(--void)', overflow: 'hidden', display: 'flex', flexDirection: 'column' }}
    >
      <div className="tree-grid" style={{ flex: 1, minHeight: 0, height: 'auto' }}>
        {/* roster — one honest line per unit */}
        <aside className="sidebar" aria-label="Fleet roster">
          <div className="sidebar-head">
            <span className="label label--hot">Fleet</span>
            <span className="view-head-count">{String(units.length).padStart(2, '0')}</span>
          </div>
          <div className="sidebar-body">
            <div className="sidebar-scroll" style={{ overflowX: 'hidden' }}>
              {units.length === 0 ? (
                <p className="sidebar-empty">Waiting for the first session.</p>
              ) : groups.map((g) => g.rows.length > 0 && (
                <div className="sidebar-group" key={g.label}>
                  <span className="label">{g.label}</span>
                  {g.rows.map((u) => (
                    <button
                      key={u.id}
                      type="button"
                      className="inspector-session-row fleet-unit"
                      data-unit={u.id}
                      title={u.detail}
                      style={{ minWidth: 0, width: '100%', overflow: 'hidden' }}
                      onClick={() => send({ cmd: 'focus', id: u.id })}
                    >
                      <i className={`dot dot--${u.status}`} aria-hidden="true" />
                      <span className="mono">{u.id}</span>
                      <span className="truncate">{u.title}</span>
                      <span className="label">{u.word}</span>
                    </button>
                  ))}
                </div>
              ))}
            </div>
          </div>
          <div className="sidebar-actions">
            <button className="sidebar-btn" type="button" onClick={onClose}>Close fleet · ⌘5</button>
          </div>
        </aside>

        {/* live activity — structured frames, never evidence */}
        <section className="center-pane" aria-label="Live activity">
          <div className="view-head">
            <span className="label label--hot">Live activity</span>
            <span className="view-head-count">{String(activity.length).padStart(2, '0')}</span>
            <span className="view-head-rule" />
            <span className="view-head-note">Display only · the snapshot is the source of truth</span>
          </div>
          <div className="transcript fleet-activity">
            <div className="transcript-inner">
              {warning && (
                <p className="body esc-reason" role="alert">
                  <PixelIcon name="alert" size={12} /> {warning}
                </p>
              )}
              {activity.length === 0 ? (
                <p className="transcript-empty">Nothing has moved yet.</p>
              ) : activity.map((a) => (
                <div className="transcript-line" key={`${a.kind}-${a.session ?? 'orch'}-${a.seq}`} style={{ animation: 'none' }}>
                  <span className="tl-n fleet-seq">{String(a.seq).padStart(3, '0')}</span>
                  <span className="tl-mark" aria-hidden="true">{a.kind === 'message' ? '→' : a.kind === 'decision' ? '•' : a.kind === 'output' ? '›' : '·'}</span>
                  <span className="tl-text">{a.text}</span>
                </div>
              ))}
            </div>
          </div>
        </section>

        {/* one decision at a time — answer in place, then step */}
        <aside className="inspector" aria-label="Decision deck">
          <div className="inspector-head">
            <span className="label label--hot">One decision</span>
            <span className="view-head-count">{String(Math.max(0, cards.length)).padStart(2, '0')}</span>
          </div>
          <div className="inspector-body">
            {!card ? (
              <p className="inspector-empty">Nothing needs an answer right now.</p>
            ) : (
              <>
                <div className="esc-card fleet-decision" role="article" aria-label={`Decision ${card.id} — ${card.session}`}>
                  <div className="esc-rail"><DitherField mode="linear" angle={90} cell={2} floor={0} /></div>
                  <div className="esc-body">
                    <div className="esc-head">
                      <span className="esc-kind"><i className="esc-kind-dot" />{card.kind === 'blocked' ? 'Waiting on you' : card.kind}</span>
                      <span className="esc-age tabular-nums">{fmtStageAge(card.age)}</span>
                    </div>
                    <div className="esc-meta">
                      <span className="tabular-nums">{card.session}</span>
                      <span><em>under</em> {card.supervisor}</span>
                    </div>
                    <p className="esc-reason">{card.reason}</p>
                    <div className="esc-actions">
                      <button type="button" className="esc-btn esc-btn--primary" data-answer="allow" onClick={() => answer(true)}>1 · Allow</button>
                      <button type="button" className="esc-btn" data-answer="deny" onClick={() => answer(false)}>2 · Needs you</button>
                      <button type="button" className="esc-btn esc-btn--kill" data-answer="kill" onClick={() => send({ cmd: 'kill', id: card.session })}>3 · Kill</button>
                    </div>
                  </div>
                </div>
                <span className="label">{Math.min(stageIndex + 1, cards.length)} of {cards.length} · arrows step · {card.signed ? 'signed' : 'digest'} {card.digest.slice(0, 8)}</span>
              </>
            )}
          </div>
        </aside>
      </div>

      {/* capability strip: declared harness, ledger fence, storm guard */}
      <div className="statusline" aria-label="Fleet capabilities">
        {harness && <span className="sl-item">{harness.label}</span>}
        {(harness?.capabilities ?? []).filter((c, i) => i < 2 || !c.supported).map((c) => (
          <span className="sl-item fleet-cap" key={c.name}>{c.name} {c.supported ? 'yes' : 'no'}</span>
        ))}
        {data.state.source !== 'empty' && (
          <>
            <span className="sl-item fleet-ledger tabular-nums" title={data.state.ledger.fence}>
              seq {data.state.ledger.seq} · fence {data.state.ledger.fence.slice(-8)}
            </span>
            <span className="sl-item">storm {data.state.storm.guard} · {data.state.storm.holds} held</span>
          </>
        )}
        {isTauri() && supervisor && (
          <>
            <button type="button" className="esc-btn" data-storm="hold" onClick={() => act({ cmd: 'fleet_control', action: 'hold', id: supervisor })}>Hold</button>
            <button type="button" className="esc-btn" data-storm="release" onClick={() => act({ cmd: 'fleet_control', action: 'release', id: supervisor })}>Release</button>
          </>
        )}
      </div>
    </div>
  );
}
