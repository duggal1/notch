// CommsTab.tsx — the wire. One vertical timeline, one card per real bus message.
// Type gets a bare pixel glyph; route gets a labelled chip; nothing is invented.
import { useMemo, useState } from 'react';
import type { BusMessage, Snapshot } from '../../lib/bridge';
import { PixelIcon, type PixelIconName } from '../PixelIcon';

type RouteFilter = 'same' | 'cross';

const ICON: Record<BusMessage['type'], PixelIconName> = {
  task: 'plus',
  result: 'check',
  question: 'bubble',
  handoff: 'chevron',
  escalate: 'alert',
};

export function CommsTab({ snapshot, onJumpToTree }: {
  snapshot: Snapshot | null;
  onJumpToTree: (sessionId: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [route, setRoute] = useState<RouteFilter | null>(null);
  const owner = useMemo(() => new Map<string, string>([
    ...(snapshot?.supervisors ?? []).map((s): [string, string] => [s.id, s.id]),
    ...(snapshot?.sessions ?? []).map((s): [string, string] => [s.id, s.sup]),
  ]), [snapshot]);
  const scope = (m: BusMessage) => {
    const a = owner.get(m.from), b = owner.get(m.to);
    return a && b ? (a === b ? 'same' : 'cross') : null;
  };
  const messages = snapshot?.busMessages ?? [];
  const q = query.trim().toLowerCase();
  const visible = messages.filter((m) =>
    (!route || scope(m) === route)
    && [m.from, m.to, m.type, m.preview].some((v) => v.toLowerCase().includes(q)));

  return (
    <section className="wire-root">
      {messages.length > 0 && (
        <header className="wire-head">
          <label className="wire-filter">
            <PixelIcon name="search" size={12} />
            <input value={query} onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter messages" aria-label="Filter messages" spellCheck={false} />
          </label>
          <button className={`wire-chip${route === 'same' ? ' is-active' : ''}`} aria-pressed={route === 'same'}
            onClick={() => setRoute(route === 'same' ? null : 'same')}>
            <PixelIcon name="check" size={12} />Same supervisor
          </button>
          <button className={`wire-chip${route === 'cross' ? ' is-active' : ''}`} aria-pressed={route === 'cross'}
            onClick={() => setRoute(route === 'cross' ? null : 'cross')}>
            <PixelIcon name="chevron" size={12} />Cross supervisor
          </button>
        </header>
      )}

      <div className="wire-scroll">
        <div className="wire-track">
          {visible.length === 0 ? (
            <div className="wire-entry">
              <time className="wire-time">{messages.length === 0 ? 'Waiting' : 'Filtered'}</time>
              <span className="wire-node" aria-hidden="true" />
              <div className="wire-card wire-card--quiet">
                <span className="wire-title">{messages.length === 0 ? 'The bus is silent' : 'No matching messages'}</span>
                <span className="wire-text">{messages.length === 0
                  ? 'Messages between sessions appear here.'
                  : 'Clear the filters to see all messages.'}</span>
                {messages.length > 0 && <button className="wire-chip" onClick={() => { setQuery(''); setRoute(null); }}>Clear filters</button>}
              </div>
            </div>
          ) : visible.map((m) => {
            const cross = scope(m) === 'cross';
            return (
              <div key={m.id} className="wire-entry">
                <time className="wire-time" dateTime={new Date(m.at).toISOString()}>
                  {new Date(m.at).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit', second: '2-digit' })}
                </time>
                <span className={`wire-node${cross ? ' wire-node--cross' : ''}`} aria-hidden="true" />
                <button className="wire-card" onClick={() => onJumpToTree(m.from)}
                  aria-label={`${m.from} to ${m.to}: ${m.preview}`}>
                  <span className="wire-line">
                    <PixelIcon name={ICON[m.type]} size={12} className="wire-icon" />
                    <span className="wire-route">{m.from} → {m.to}</span>
                    <span className="wire-type">{m.type.replaceAll('_', ' ')}</span>
                  </span>
                  <span className="wire-text">{m.preview}</span>
                </button>
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}
