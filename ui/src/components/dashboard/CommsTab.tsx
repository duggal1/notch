import { useMemo, useState } from 'react';
import type { Snapshot } from '../../lib/bridge';
import { PixelIcon } from '../PixelIcon';

type RouteFilter = 'same' | 'cross';

export function CommsTab({ snapshot, onJumpToTree }: {
  snapshot: Snapshot | null;
  onJumpToTree: (sessionId: string) => void;
}) {
  const [filter, setFilter] = useState('');
  const [route, setRoute] = useState<RouteFilter | null>(null);
  const supervisors = useMemo(() => new Map(snapshot?.sessions.map((session) => [session.id, session.sup])), [snapshot]);
  const messages = snapshot?.busMessages ?? [];
  const scope = (from: string, to: string) => {
    const sender = supervisors.get(from);
    const receiver = supervisors.get(to);
    return sender && receiver ? (sender === receiver ? 'same' : 'cross') : null;
  };
  const query = filter.trim().toLowerCase();
  const filtered = messages.filter((message) =>
    (!route || scope(message.from, message.to) === route)
    && [message.from, message.to, message.type, message.preview].some((value) => value.toLowerCase().includes(query)));

  return (
    <section className="comms">
      {messages.length > 0 && (
        <div className="comms-bar">
          <label className="comms-filter">
            <PixelIcon name="search" size={12} />
            <input value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Filter messages" aria-label="Filter messages" spellCheck={false} />
          </label>
          <button className="route-chip" aria-pressed={route === 'same'} onClick={() => setRoute(route === 'same' ? null : 'same')}>Same supervisor</button>
          <button className="route-chip" aria-pressed={route === 'cross'} onClick={() => setRoute(route === 'cross' ? null : 'cross')}>Cross supervisor</button>
        </div>
      )}
      <div className="comms-list">
        <div className="timeline">
          {filtered.length === 0 ? (
            <div className="timeline-entry">
              <span className="comms-time">{messages.length === 0 ? 'Waiting' : 'Filtered'}</span>
              <div className="timeline-empty">
                <h2 className="title">{messages.length === 0 ? 'The bus is silent' : 'No matching messages'}</h2>
                <p className="empty-line">{messages.length === 0 ? 'Messages between sessions appear here.' : 'Clear the filters to see all messages.'}</p>
                {messages.length > 0 && <button className="route-chip" onClick={() => { setFilter(''); setRoute(null); }}>Clear filters</button>}
              </div>
            </div>
          ) : filtered.map((message) => (
            <div key={message.id} className={`timeline-entry${scope(message.from, message.to) === 'cross' ? ' timeline-entry--cross' : ''}`}>
              <time className="comms-time" dateTime={new Date(message.at).toISOString()}>{new Date(message.at).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit', second: '2-digit' })}</time>
              <button className="comms-row" onClick={() => onJumpToTree(message.from)}>
                <span className="comms-line">
                  <span className="comms-pair mono">{message.from} → {message.to}</span>
                  <span className="type-chip">{message.type.replaceAll('_', ' ')}</span>
                </span>
                <span className="comms-preview">{message.preview}</span>
              </button>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
