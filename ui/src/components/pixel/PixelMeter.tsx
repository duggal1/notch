// PixelMeter.tsx — segmented square meter. The only progress instrument in the app.
// Cells are literal squares; the filled ones carry the dither checkerboard.
export function PixelMeter({
  value, cells = 20, tone = 'hot', label, className = '',
}: {
  value: number;
  cells?: number;
  tone?: 'hot' | 'pale' | 'dim';
  label?: string;
  className?: string;
}) {
  const v = Math.max(0, Math.min(1, Number.isFinite(value) ? value : 0));
  const lit = Math.round(v * cells);
  return (
    <div
      className={`pixel-meter pixel-meter--${tone} ${className}`}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(v * 100)}
      aria-label={label}
    >
      {Array.from({ length: cells }, (_, i) => (
        <i key={i} className={i < lit ? 'pm-cell pm-cell--on' : 'pm-cell'} />
      ))}
    </div>
  );
}
