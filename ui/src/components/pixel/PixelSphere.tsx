// PixelSphere.tsx — the halftone core. Same 8×8 Bayer engine as the dither field, drawn as
// a shaded ball. `live` rotates the light; reduced motion gets one static frame.
import { useEffect, useRef } from 'react';
import { paintSphere } from '../../lib/pixel';
import { reduced } from '../../lib/motion';

export function PixelSphere({ size = 96, cell = 3, live = true, className = '' }: {
  size?: number;
  cell?: number;
  live?: boolean;
  className?: string;
}) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const cv = ref.current;
    if (!cv) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    cv.width = Math.round(size * dpr);
    cv.height = Math.round(size * dpr);
    const ctx = cv.getContext('2d');
    if (!ctx) return;

    let phase = 0;
    const draw = () => {
      paintSphere(ctx, size, phase, { cell, dpr });
    };
    draw();
    if (!live || reduced) return;
    const id = window.setInterval(() => {
      phase += 0.18;
      draw();
    }, 120);
    return () => window.clearInterval(id);
  }, [size, cell, live]);

  return (
    <canvas
      ref={ref}
      className={`pixel-sphere ${className}`}
      style={{ width: size, height: size }}
      aria-hidden="true"
    />
  );
}
