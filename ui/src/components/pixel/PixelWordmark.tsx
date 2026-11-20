// PixelWordmark.tsx — NOTCH in a hand-built 5×7 bitmap. Crisp edges at any scale.
import { wordmarkCells } from '../../lib/pixel';

export function PixelWordmark({ scale = 2, className = '' }: { scale?: number; className?: string }) {
  const cells = wordmarkCells();
  const w = 5 * 6 - 1;
  return (
    <svg
      className={`pixel-wordmark ${className}`}
      width={w * scale}
      height={7 * scale}
      viewBox={`0 0 ${w} 7`}
      shapeRendering="crispEdges"
      role="img"
      aria-label="Notch"
    >
      {cells.map((c, i) => (
        <rect key={i} x={c.x} y={c.y} width={1} height={1} fill="currentColor" />
      ))}
    </svg>
  );
}
