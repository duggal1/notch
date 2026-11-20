// Shell.tsx — the notch shell. Opaque black in rest, peek and open.
// No clip-path morph, no blur, no full-surface dither slab, no looping decoration.
// The logo and the square status dot are the whole rest/peek silhouette.
import { type ReactNode } from 'react';
import { type Mode } from '../lib/shell-geometry';

export function Shell({ mode, content, shellRef, children, onEnter, onLeave, onTap }: {
  mode: Mode;
  content: 'idle' | 'active';
  shellRef: React.RefObject<HTMLDivElement | null>;
  children: ReactNode;
  onEnter: () => void;
  onLeave: () => void;
  onTap: () => void;
}) {
  return (
    <div
      ref={shellRef}
      className="shell"
      data-mode={mode}
      data-content={content}
      role={mode === 'open' ? undefined : 'button'}
      tabIndex={mode === 'open' ? undefined : 0}
      aria-label={mode === 'open' ? 'Notch control plane — expanded' : 'Notch control plane — hover to peek, click to open'}
      onPointerEnter={onEnter}
      onPointerLeave={onLeave}
      onClick={onTap}
      onKeyDown={(ev) => { if (mode !== 'open' && ev.target === ev.currentTarget && (ev.key === 'Enter' || ev.key === ' ')) { ev.preventDefault(); onTap(); } }}
      style={{ background: '#000000' }}
    >
      {/* macOS passes fully transparent pixels through; keep a near-zero-alpha hit layer. */}
      <span className="hit" aria-hidden="true" />
      {children}
    </div>
  );
}
