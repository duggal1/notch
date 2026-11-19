// Shell.tsx — the notch shell (DESIGN.md §5). The element box is FIXED at the open frame
// (560×640); only the clip-path silhouette animates on springs — zero layout per morph frame.
import { useEffect, useLayoutEffect, useRef, type ReactNode } from 'react';
import { Spring, ticker, reduced } from '../lib/motion';
import { SPRING } from '../lib/tokens';
import { frameFor, notchPath, type Mode } from '../lib/shell-geometry';

export function Shell({ mode, shellRef, children, onEnter, onLeave, onTap }: {
  mode: Mode;
  shellRef: React.RefObject<HTMLDivElement | null>;
  children: ReactNode;
  onEnter: () => void;
  onLeave: () => void;
  onTap: () => void;
}) {
  const rest = frameFor('rest');
  const w = useRef(new Spring(rest.w, SPRING.open));
  const h = useRef(new Spring(rest.h, SPRING.open));
  const r = useRef(new Spring(rest.r, SPRING.open));
  const e = useRef(rest.e);

  const paint = (width = w.current.x, height = h.current.x, radius = r.current.x) => {
    const el = shellRef.current;
    if (!el) return;
    // the fixed shell box is wider than the silhouette — centre the silhouette inside it
    const x0 = (560 - width) / 2;
    el.style.clipPath = `path('${notchPath(width, height, Math.max(0, Math.min(radius, height - e.current)), e.current, x0)}')`;
  };

  useLayoutEffect(() => {
    paint();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const f = frameFor(mode);
    e.current = f.e;
    if (reduced) {
      w.current.x = f.w; h.current.x = f.h; r.current.x = f.r; w.current.v = h.current.v = r.current.v = 0;
      paint();
      return;
    }
    w.current.target = f.w; h.current.target = f.h; r.current.target = f.r;
    return ticker.add((dt) => {
      paint(w.current.step(dt), h.current.step(dt), r.current.step(dt));
      return !(w.current.settled && h.current.settled && r.current.settled);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);

  return (
    <div
      ref={shellRef}
      className="shell"
      data-mode={mode}
      role="button"
      tabIndex={0}
      aria-label={mode === 'open' ? 'Notch control plane — expanded' : 'Notch control plane — hover to peek, click to open'}
      onPointerEnter={onEnter}
      onPointerLeave={onLeave}
      onPointerDown={(ev) => { ev.preventDefault(); onTap(); }}
      onKeyDown={(ev) => { if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); onTap(); } }}
    >
      {children}
    </div>
  );
}
