// Drift.tsx — DESIGN.md §9.5 / ref 3. Range bars from rolling min (A) to each session's avg.
// Caption is derived, never invented: with < 2 timed sessions we say so.
import { drift, fmtMs } from '../lib/format';
import type { Session } from '../lib/bridge';

export function Drift({ sessions }: { sessions: Session[] }) {
  const d = drift(sessions);
  const timed = sessions.filter((s) => typeof s.latencyMs === 'number');
  const sentence =
    d.n < 2
      ? 'Collecting step latency — first sample pending.'
      : `Step latency never drifted more than ${(d.pct ?? 0).toFixed(2)}% across ${d.n} sessions.`;

  const W = 208, H = 72;
  const lo = d.best ?? 0, hi = d.worst ?? 1;
  const y = (v: number) => +(H - 12 - ((v - lo) / (hi - lo || 1)) * (H - 24)).toFixed(1);
  const barW = timed.length > 0 ? Math.min(14, (W - 84) / timed.length - 6) : 0;
  const last = timed.length - 1;

  return (
    <div className="tile flex items-stretch gap-3 px-4 py-2.5">
      <div className="flex min-w-0 flex-1 flex-col justify-center gap-1">
        <span className="label">Step latency</span>
        <p className="m-0 max-w-[244px] text-[13px] leading-snug" style={{ color: 'rgba(255,255,255,.9)' }}>
          {sentence}
        </p>
        {d.n >= 2 && d.best !== null && d.worst !== null && (
          <p className="m-0 font-mono text-[10px]" style={{ color: 'rgba(255,255,255,.38)' }}>
            best {fmtMs(d.best)} · worst {fmtMs(d.worst)} · rolling avg
          </p>
        )}
      </div>
      <div className="flex flex-col justify-center" aria-hidden="true">
        {d.n < 2 ? (
          <div className="flex h-[72px] w-[208px] flex-col items-center justify-center gap-1">
            <span className="label" style={{ color: 'rgba(255,255,255,.4)' }}>Collecting</span>
            <span className="font-mono text-[10.5px]" style={{ color: 'rgba(255,255,255,.32)' }}>{d.n}/2 sessions timed</span>
          </div>
        ) : (
          <svg viewBox={`0 0 ${W} ${H}`} width={W} height={H} className="drift block">
            <g stroke="rgba(255,255,255,.28)" strokeDasharray="2 3">
              <line x1="0" x2={W - 58} y1={y(hi)} y2={y(hi)} />
              <line x1="0" x2={W - 58} y1={y(lo)} y2={y(lo)} />
            </g>
            <g fill="rgba(255,255,255,.8)" fontFamily="ui-monospace, 'SF Mono', monospace" fontSize="9.5" letterSpacing="1.2">
              <text x={W - 52} y={y(hi) + 3.5}>B {(d.pct ?? 0).toFixed(2)}%</text>
              <text x={W - 52} y={y(lo) + 3.5}>A 0.00%</text>
            </g>
            {timed.map((s, i) => {
              const v = s.latencyMs as number;
              const x = 18 + ((W - 84) / Math.max(timed.length, 1)) * (i + 0.5);
              const isLast = i === last, o = isLast ? 1 : 0.45;
              return (
                <g key={s.id}>
                  <line x1={x} x2={x} y1={y(v)} y2={y(lo)} stroke={`rgba(255,255,255,${o})`} strokeWidth={isLast ? 2 : 1.25} />
                  <circle cx={x} cy={y(v)} r={isLast ? 4.5 : 3.6} fill={`rgba(255,255,255,${o})`} />
                  <circle cx={x} cy={y(lo)} r={isLast ? 4 : 3.2} fill={`rgba(255,255,255,${o * 0.8})`} />
                </g>
              );
            })}
          </svg>
        )}
      </div>
    </div>
  );
}
