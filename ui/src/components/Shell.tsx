import type { ReactNode, RefObject } from 'react';
import { frameFor, type Mode } from '../lib/shell-geometry';

export function Shell({ mode, contentH = 640, shellRef, children, onEnter, onLeave, onTap }: {
  mode: Mode;
  contentH?: number;
  shellRef: RefObject<HTMLDivElement | null>;
  children: ReactNode;
  onEnter: () => void;
  onLeave: () => void;
  onTap: () => void;
}) {
  const frame = frameFor(mode, contentH);
  const expanded = mode === 'open';
  return (
    <div ref={shellRef} className="shell" data-mode={mode}
      style={{ width: frame.w, height: frame.h, borderRadius: `0 0 ${frame.r}px ${frame.r}px` }}
      onPointerEnter={onEnter} onPointerLeave={onLeave}>
      {expanded ? children : (
        <button className="notch-trigger" aria-label="Open Notch" onClick={onTap}>{children}</button>
      )}
    </div>
  );
}
