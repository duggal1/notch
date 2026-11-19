// SessionGrid.tsx — dot matrix, rows = supervisors (DESIGN.md §9.3). Redraws at 30 fps while open.
import { useEffect, useMemo, useRef } from 'react';
import { drawSessionGrid, gridFromSnapshot, type Cell } from '../lib/session-grid';
import { reduced, ticker } from '../lib/motion';
import type { Session, Supervisor } from '../lib/bridge';

export function SessionGrid({ supervisors, sessions, active, cols = 8 }: {
  supervisors: Supervisor[];
  sessions: Session[];
  active: boolean;
  cols?: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const tRef = useRef(0);
  const grid: Cell[][] = useMemo(
    () => gridFromSnapshot(supervisors, sessions, cols),
    [supervisors, sessions, cols],
  );
  const rows = Math.max(grid.length, 1);

  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    const pitch = 20, dpr = Math.min(window.devicePixelRatio || 1, 2);
    cv.width = cols * pitch * dpr; cv.height = rows * pitch * dpr;
    cv.style.width = `${cols * pitch}px`; cv.style.height = `${rows * pitch}px`;
    const ctx = cv.getContext('2d');
    if (!ctx) return;
    ctx.scale(dpr, dpr);
    drawSessionGrid(ctx, grid, tRef.current, pitch, 4.2);
    if (!active || reduced) return;
    return ticker.add((dt) => {
      tRef.current += dt;
      drawSessionGrid(ctx, grid, tRef.current, pitch, 4.2);
      return true;
    });
  }, [grid, active, cols, rows]);

  return <canvas ref={canvasRef} aria-hidden="true" />;
}
