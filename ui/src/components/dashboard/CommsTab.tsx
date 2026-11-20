// CommsTab.tsx — the wire. Newest first, filter by from/to/type/body.
// Each message is one soft card: route on top, payload underneath. Same-supervisor traffic
// carries a dark fuchsia spine; cross-supervisor carries the hot one. Click jumps to the sender.
import { useMemo, useState } from 'react';
import type { Snapshot } from '../../lib/bridge';
import { PixelIcon } from '../PixelIcon';
import { TYPE_COLOR } from '../../lib/bus';

export function CommsTab({ snapshot, onJumpToTree }: {
  snapshot: Snapshot | null;
  onJumpToTree: (sessionId: string) => void;
}) {
  const [filter, setFilter] = useState('');
  const sessions = snapshot?.sessions ?? [];
  const supOf = useMemo(() => new Map(sessions.map((s) => [s.id, s.sup])), [sessions]);
  const messages = snapshot?.busMessages ?? [];

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return messages;
    return messages.filter((m) =>
      m.from.toLowerCase().includes(q) || m.to.toLowerCase().includes(q)
      || m.type.includes(q) || m.preview.toLowerCase().includes(q));
  }, [filter, messages]);

  return (
    <div className="comms">
      <div className="comms-bar">
        <span className="label">The wire</span>
        <span className="view-head-count">{String(messages.length).padStart(2, '0')}</span>
        <div className="comms-legend">
          <span>Same sup <i className="marker marker--dim" /></span>
          <span>Cross sup <i className="marker marker--lilac" /></span>
        </div>
        <label className="comms-filter">
          <PixelIcon name="search" size={12} />
          <input value={filter} onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter from, to, type, body" aria-label="Filter messages" spellCheck={false} />
        </label>
        <span className="comms-legend-note">Newest first · the bus is the only channel</span>
      </div>

      <div className="comms-list">
        {filtered.length === 0 ? (
          <div className="empty-state">
            <span className="display">{messages.length === 0 ? 'The bus is silent.' : 'No match.'}</span>
            <p className="empty-line">
              {messages.length === 0
                ? 'Nothing has been said yet. Every cross-supervisor message lands here, newest first.'
                : `Nothing matches “${filter.trim()}”. Clear the filter to see all ${messages.length} messages.`}
            </p>
            {messages.length > 0 && (
              <button className="esc-btn esc-btn--primary ripple" onClick={() => setFilter('')}>Clear filter</button>
            )}
          </div>
        ) : filtered.map((m, i) => {
          const cross = supOf.get(m.from) !== supOf.get(m.to);
          return (
            <button
              key={m.id}
              style={{ animationDelay: `${Math.min(i, 10) * 26}ms` }}
              className={`comms-row${cross ? ' comms-row--cross' : ''}`}
              onClick={() => onJumpToTree(m.from)}
            >
              <span className="comms-line">
                <span className="comms-time">{new Date(m.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}</span>
                <span className="comms-pair">{m.from} <span className="comms-arrow">→</span> {m.to}</span>
                <span className="type-chip" style={{ color: TYPE_COLOR[m.type] }}>
                  <i className="type-dot" style={{ background: TYPE_COLOR[m.type] }} />
                  {m.type}
                </span>
              </span>
              <span className="comms-preview">{m.preview}</span>
            </button>
          );
        })}
        {filtered.length > 0 && <div className="wire-end"><span>end of wire</span></div>}
      </div>
    </div>
  );
}
