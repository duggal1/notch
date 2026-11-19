// Drift.tsx — step-latency honesty in one slim card. Derived, never invented.
import { drift, fmtMs } from '../lib/format';
import type { Session } from '../lib/bridge';

export function Drift({ sessions }: { sessions: Session[] }) {
  const d = drift(sessions);
  const timed = sessions.filter((s) => typeof s.latencyMs === 'number');
  const W = 148, H = 30;
  const lo = d.best ?? 0, hi = d.worst ?? 1;
  const n = Math.max(timed.length, 1);
  const slot = W / n;
  const bw = Math.max(2, Math.min(6, slot - 3));

  return (
    <div className="card drift">
      <div className="drift-text">
        <span className="label">Step latency</span>
        <span className="drift-line num">
          {d.n < 2 || d.best === null || d.worst === null
            ? 'Waiting for timings'
            : `${(d.pct ?? 0).toFixed(1)}% drift · ${fmtMs(d.best)} – ${fmtMs(d.worst)}`}
        </span>
      </div>
      {d.n >= 2 && (
        <svg className="drift-svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} aria-hidden="true">
          {timed.map((s, i) => {
            const v = s.latencyMs as number;
            const k = 0.18 + 0.82 * ((v - lo) / (hi - lo || 1));
            const last = i === timed.length - 1;
            return (
              <rect key={s.id} className="drift-bar" x={i * slot + (slot - bw) / 2} y={0} width={bw} height={H} rx={bw / 2}
                fill="#fff" opacity={last ? 1 : 0.38} style={{ transform: `scaleY(${k.toFixed(3)})` }} />
            );
          })}
        </svg>
      )}
    </div>
  );
}