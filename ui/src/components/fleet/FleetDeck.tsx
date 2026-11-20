// FleetDeck.tsx — opt-in fleet management surface, composed AROUND the existing
// dashboard tabs. Structure only is stolen from agentbox (one derived fleet,
// one honest line per unit, one decision on stage, display-only activity);
// every class and CSS variable here is already ours. No stylesheet changes.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Snapshot } from '../../lib/bridge';
import { send } from '../../lib/bridge';
import {
  loadFleet, registerFleetEvents, sendFleet, EMPTY_FLEET_STATE,
  type FleetData, type FleetUnit,
} from '../../lib/fleet-wiring';
import { DitherField } from '../pixel/DitherField';
import {
  deriveActivity, firstUnitOf, fmtStageAge, stageQueue, stepStage,
} from './derive';

export function FleetDeck({ snapshot, onClose }: { snapshot: Snapshot | null; onClose: () => void }) {
  const [data, setData] = useState<FleetData>(() => ({
    state: EMPTY_FLEET_STATE, approvals: [], harnesses: [],
  }));
  const [stageIndex, setStageIndex] = useState(0);
  const deckRef = useRef<HTMLDivElement>(null);

  // One load from the backend view; live `fleet` events take it from here.
  useEffect(() => {
    let alive = true;
    loadFleet().then((next) => { if (alive) setData(next); });
    return () => { alive = false; };
  }, []);

  useEffect(() => registerFleetEvents((state) => {
    setData((prev) => ({ ...prev, state }));
  }), []);

  const units = data.state.units;
  const activity = useMemo(() => deriveActivity(snapshot), [snapshot]);
  const cards = useMemo(() => stageQueue(data.approvals, units), [data.approvals, units]);
  const card = cards[Math.min(stageIndex, Math.max(0, cards.length - 1))] ?? null;
  const sup = snapshot?.supervisors?.[0]?.id ?? 'SV-1';
  const harness = data.harnesses[0];

  const answer = useCallback((allow: boolean) => {
    if (!card) return;
    sendFleet({ cmd: 'answer_approval', id: card.id, allow });
  }, [card]);

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
        <span className="sl-item"><i className="dot dot--working" aria-hidden="true" />{harness?.label ?? 'OpenCode'}</span>
        {(harness?.capabilities ?? []).filter((c, i) => i < 2 || !c.supported).map((c) => (
          <span className="sl-item fleet-cap" key={c.name}>{c.name} {c.supported ? 'yes' : 'no'}</span>
        ))}
        <span className="sl-item fleet-ledger">seq {data.state.ledger.seq} · fence {data.state.ledger.fence.slice(0, 8)}</span>
        <span className="sl-item">storm {data.state.storm.guard} · {data.state.storm.holds} held</span>
        <button type="button" className="esc-btn" data-storm="hold" onClick={() => sendFleet({ cmd: 'fleet_control', action: 'hold', id: sup })}>Hold</button>
        <button type="button" className="esc-btn" data-storm="release" onClick={() => sendFleet({ cmd: 'fleet_control', action: 'release', id: sup })}>Release</button>
        <span className="sl-dither"><DitherField mode="linear" angle={0} cell={2} floor={0.18} /></span>
      </div>
    </div>
  );
}
