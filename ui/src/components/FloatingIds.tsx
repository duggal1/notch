// FloatingIds.tsx — real session IDs drifting behind content (DESIGN.md §9.6). Click → focus herdr pane.
import type { CSSProperties } from 'react';
import { computeFloatingIds } from '../lib/floating-ids';

export function FloatingIds({ ids, active, onFocus }: {
  ids: { id: string; live: boolean }[];
  active: boolean;
  onFocus: (id: string) => void;
}) {
  if (!active || ids.length === 0) return null;
  return (
    <>
      {computeFloatingIds(ids, 7).map((f) => (
        <button
          key={f.id}
          className="fid"
          data-id={f.id}
          data-live={f.live ? '' : undefined}
          style={{ left: `${f.left}%`, top: `${f.top}%`, '--o': f.o, '--d': `${f.d}s`, '--dx': `${f.dx}px` } as CSSProperties}
          onClick={() => onFocus(f.id)}
          aria-label={`Focus session ${f.id}`}
        >
          {f.id}
        </button>
      ))}
    </>
  );
}
