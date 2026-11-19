// session-grid.ts — DESIGN.md §9.3. Rows = supervisors, cols = slots. Pure draw fn; React owns the canvas.
import { C } from './tokens';
import { breathe } from './glow-orb';
import type { Status } from './tokens';

export type Cell = Status | null;

export function drawSessionGrid(
  ctx: CanvasRenderingContext2D,
  grid: Cell[][],
  t: number,
  pitch = 20,
  r = 4.2,
): void {
  ctx.clearRect(0, 0, 4096, 4096); // ctx is pre-scaled by dpr
  for (let j = 0; j < grid.length; j++) {
    const row = grid[j]!;
    for (let i = 0; i < row.length; i++) {
      const s = row[i];
      if (!s) continue;
      const x = pitch * (i + 0.5), y = pitch * (j + 0.5), edge = Math.min(1, (i + 1) / 3); // left columns fade in
      if (s === 'working') {
        const b = Math.pow(Math.max(0, Math.sin((i + j) * 0.35 - t * 2.2)), 8); // travelling diagonal bump
        ctx.fillStyle = `rgba(255,255,255,${((0.78 + 0.22 * b) * edge).toFixed(3)})`;
        ctx.beginPath(); ctx.arc(x, y, r, 0, 7); ctx.fill();
      } else if (s === 'idle') {
        ctx.fillStyle = `rgba(255,255,255,${(0.3 * edge).toFixed(3)})`;
        ctx.beginPath(); ctx.arc(x, y, r, 0, 7); ctx.fill();
      } else if (s === 'done') {
        ctx.fillStyle = C.lilac; ctx.globalAlpha = 0.95 * edge;
        ctx.beginPath(); ctx.arc(x, y, r, 0, 7); ctx.fill(); ctx.globalAlpha = 1;
      } else { // failed: hollow
        ctx.strokeStyle = C.pink; ctx.lineWidth = 1.5;
        ctx.beginPath(); ctx.arc(x, y, r * (0.86 + (0.14 * (breathe('failed', t) - 1)) / 0.18), 0, 7); ctx.stroke();
      }
    }
  }
}

export function gridFromSnapshot(supervisors: { id: string }[], sessions: { sup: string; status: Status }[], cols = 8): Cell[][] {
  return supervisors.map((sv) => {
    const own = sessions.filter((s) => s.sup === sv.id);
    const row: Cell[] = [];
    for (let i = 0; i < cols; i++) row.push(own[i]?.status ?? null);
    return row;
  });
}
