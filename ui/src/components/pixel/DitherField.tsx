// DitherField.tsx — the brand texture. A canvas of square pixels ordered-dithered through
// the fuchsia ramp. Selective by design: header bands, empty states, meters, card crowns.
import { useEffect, useRef } from 'react';
import { paintDither, type DitherMode } from '../../lib/pixel';
import { reduced } from '../../lib/motion';

export function DitherField({
  mode = 'linear',
  angle = 90,
  cell = 2,
  floor = 0,
  ceil = 1,
  cx,
  cy,
  animate = false,
  period = 2600,
  className = '',
}: {
  mode?: DitherMode;
  angle?: number;
  cell?: number;
  floor?: number;
  ceil?: number;
  cx?: number;
  cy?: number;
  animate?: boolean;
  period?: number;
  className?: string;
}) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const cv = ref.current;
    if (!cv) return;
    let raf = 0;
    let t0 = performance.now();
    let lastW = 0;
    let lastH = 0;

    const paint = (now: number) => {
      const rect = cv.getBoundingClientRect();
      const w = Math.max(1, Math.round(rect.width));
      const h = Math.max(1, Math.round(rect.height));
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      if (w !== lastW || h !== lastH) {
        lastW = w; lastH = h;
        cv.width = Math.round(w * dpr);
        cv.height = Math.round(h * dpr);
      }
      const ctx = cv.getContext('2d');
      if (ctx) {
        paintDither(ctx, w, h, {
          mode, angle, cell, floor, ceil, cx, cy, dpr,
          phase: animate && !reduced ? ((now - t0) / period) % 1 : 0,
        });
      }
      if (animate && !reduced) raf = requestAnimationFrame(paint);
    };

    raf = requestAnimationFrame(paint);
    const ro = new ResizeObserver(() => {
      if (animate && !reduced) return; // the loop already tracks size
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(paint);
    });
    ro.observe(cv);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      t0 = 0;
    };
  }, [mode, angle, cell, floor, ceil, cx, cy, animate, period]);

  return <canvas ref={ref} className={`dither-field ${className}`} aria-hidden="true" />;
}
