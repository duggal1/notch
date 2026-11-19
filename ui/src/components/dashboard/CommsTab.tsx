// CommsTab.tsx — the bus log. Newest first, filter by from/to/type/body.
// Same-supervisor traffic carries a dim marker; cross-supervisor carries a lilac one.
// Clicking a message jumps the tree selection to the sender.
import { useMemo, useState } from 'react';
import type { Snapshot } from '../../lib/bridge';
import { SearchIcon } from '../Icons';
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
      <div className="comms-legend">
        <span>Same supervisor <i className="marker marker--dim" /></span>
        <span>Cross-supervisor <i className="marker marker--lilac" /></span>
        <span className="comms-legend-note">Newest first · bus is the only channel</span>
      </div>
      <label className="comms-filter">
        <SearchIcon size={13} />
        <input value={filter} onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter by from, to, type or preview…" aria-label="Filter messages" spellCheck={false} />
      </label>
      <div className="comms-list">
        {filtered.length === 0 ? (
          <p className="comms-empty">{messages.length === 0 ? 'The bus is silent — nothing has been said.' : 'Nothing matches that filter.'}</p>
        ) : filtered.map((m) => {
          const cross = supOf.get(m.from) !== supOf.get(m.to);
          return (
            <button key={m.id} className={`comms-row${cross ? ' comms-row--cross' : ''}`} onClick={() => onJumpToTree(m.from)}>
              <span className="comms-time">{new Date(m.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}</span>
              <span className="comms-pair mono">{m.from} <span className="comms-arrow">→</span> {m.to}</span>
              <span className="type-chip" style={{ color: TYPE_COLOR[m.type] }}>
                <i className="type-dot" style={{ background: TYPE_COLOR[m.type] }} />
                {m.type}
              </span>
              <span className="comms-preview">{m.preview}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
