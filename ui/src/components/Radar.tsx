// Radar.tsx — DESIGN.md §9.4. Orchestrator center, supervisors on the middle ring.
// Hottest = most live work (failed > working > idle > done), dashed tether to it.
import type { Supervisor } from '../lib/bridge';
import type { Status } from '../lib/tokens';

const HEAT: Record<Status, number> = { failed: 3, working: 2, idle: 1, done: 0 };

export function Radar({ supervisors, hot }: { supervisors: Supervisor[]; hot?: number }) {
  const n = supervisors.length;
  const hotIndex = hot ?? supervisors.reduce((best, s, i) => (HEAT[s.status] > HEAT[supervisors[best]?.status ?? 'done'] ? i : best), 0);
  const R = 52;
  const pts = supervisors.map((s, i) => {
    const a = (n === 0 ? 0 : (i / n) * Math.PI * 2) - Math.PI / 2.35;
    return { s, x: +(Math.cos(a) * R).toFixed(1), y: +(Math.sin(a) * R).toFixed(1) };
  });
  const hotPt = pts[hotIndex];
  const fill = (st: Status) => (st === 'failed' ? '#FFB3C1' : st === 'idle' ? 'rgba(255,255,255,.4)' : st === 'done' ? '#DCD5FA' : '#fff');
  return (
    <svg viewBox="-100 -100 200 200" className="radar h-full w-full" role="img" aria-label={`${n} supervisors on ring`}>
      <g fill="none" stroke="rgba(255,255,255,.32)" strokeWidth=".75">
        <circle r="30" /><circle r="52" /><circle r="82" opacity=".5" />
        <circle r="52" className="ping" stroke="rgba(255,255,255,.5)" />
      </g>
      {hotPt && pts.length > 0 && (
        <line x1="0" y1="0" x2={hotPt.x} y2={hotPt.y} stroke="#fff" strokeWidth="1" strokeDasharray="2 3" className="tether" />
      )}
      <circle r="3.4" fill="#fff" />
      {pts.map((p) => (
        <g key={p.s.id}>
          <circle cx={p.x} cy={p.y} r="9" fill="rgba(255,255,255,.14)" />
          <circle cx={p.x} cy={p.y} r="4.6" fill={fill(p.s.status)} />
        </g>
      ))}
    </svg>
  );
}
