// Shell.tsx — the notch shell (v4): Diether style — a darker, louder pill with a
// pulsing Diether blue dot at its center, matching Jupiter's palette.
// Rest/peek content hugs the notch; open is a full black box.
import { type ReactNode, useEffect, useRef, useState } from 'react';
import { type Mode } from '../lib/shell-geometry';
import { reduced } from '../lib/motion';

const DITHER_KEYFRAMES = `
@keyframes notchDitherBeat {
  0%, 100% { opacity: 0.5; transform: scale(0.9); }
  50%      { opacity: 1;   transform: scale(1.1); }
}`;

export function Shell({ mode, shellRef, children, onEnter, onLeave, onTap }: {
  mode: Mode;
  shellRef: React.RefObject<HTMLDivElement | null>;
  children: ReactNode;
  onEnter: () => void;
  onLeave: () => void;
  onTap: () => void;
}) {
  const [dotId] = useState('notch-dither-beat');
  useEffect(() => {
    const existing = document.getElementById(dotId);
    if (existing) return;
    const style = document.createElement('style');
    style.id = dotId;
    style.textContent = DITHER_KEYFRAMES;
    document.head.appendChild(style);
    return () => style.remove();
  }, [dotId]);

  return (
    <div
      ref={shellRef}
      className="shell"
      data-mode={mode}
      role={mode === 'open' ? undefined : 'button'}
      tabIndex={mode === 'open' ? undefined : 0}
      aria-label={mode === 'open' ? 'Notch control plane — expanded' : 'Notch control plane — hover to peek, click to open'}
      onPointerEnter={onEnter}
      onPointerLeave={onLeave}
      onClick={onTap}
      onKeyDown={(ev) => { if (mode !== 'open' && ev.target === ev.currentTarget && (ev.key === 'Enter' || ev.key === ' ')) { ev.preventDefault(); onTap(); } }}
      style={{ minHeight: 40, background: 'var(--color-f0)' }}
    >
      {/* macOS passes fully transparent pixels through; keep a near-zero-alpha hit layer. */}
      <span className="hit" aria-hidden="true" />
      {children}
      {/* Diether blue pulse dot — the pixelation signature at the notch center. */}
      {!reduced && (
        <span
          aria-hidden="true"
          style={{
            position: 'absolute',
            inset: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            pointerEvents: 'none',
          }}
        >
          <span
            style={{
              width: 6,
              height: 6,
              borderRadius: 999,
              background: 'var(--color-accent)',
              opacity: 0.95,
              animation: 'notchDitherBeat 1.1s ease-in-out infinite',
            }}
          />
        </span>
      )}
    </div>
  );
}
