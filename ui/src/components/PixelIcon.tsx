// PixelIcon.tsx — the single icon primitive for the whole UI.
//
// Every glyph comes from @hugeicons/core-free-icons, rasterised by
// tools/build_pixel_icons.py into a native-CSS-pixel PNG grid with quantised
// alpha. A CSS mask paints that alpha with `currentColor`, so the white-hot
// row flip (ink on #FFFFFF) keeps working with zero per-icon code. The PNG is
// already at the display size, and `image-rendering: pixelated` keeps every
// cell a hard square on retina. One family, one stroke weight, one pixel size.
import type { CSSProperties } from 'react';

const urls = import.meta.glob<string>('../assets/pixel-icons/*.png', {
  eager: true,
  query: '?url',
  import: 'default',
});

export const PIXEL_ICON_NAMES = [
  'orchestrator', 'logs', 'reprompt', 'focus', 'kill', 'copy', 'check',
  'trash', 'bubble', 'pause', 'play', 'chevron-down', 'close', 'search',
  'grid', 'list', 'alert', 'plus', 'window',
] as const;
export type PixelIconName = (typeof PIXEL_ICON_NAMES)[number];

const SIZES = [9, 10, 11, 12, 13, 14, 15, 16, 17] as const;

function nearestSize(size: number): number {
  let best: number = SIZES[0]!;
  for (const s of SIZES) if (Math.abs(s - size) < Math.abs(best - size)) best = s;
  return best;
}

export function PixelIcon({ name, size = 16, className }: {
  name: PixelIconName;
  size?: number;
  className?: string;
}) {
  const assetSize = nearestSize(size);
  const url = urls[`../assets/pixel-icons/${name}-${assetSize}.png`];
  const style: CSSProperties = {
    display: 'inline-block',
    width: size,
    height: size,
    background: 'currentColor',
    imageRendering: 'pixelated',
    WebkitMaskImage: `url("${url}")`,
    maskImage: `url("${url}")`,
    WebkitMaskPosition: 'center',
    maskPosition: 'center',
    WebkitMaskRepeat: 'no-repeat',
    maskRepeat: 'no-repeat',
    WebkitMaskSize: '100% 100%',
    maskSize: '100% 100%',
  };
  return (
    <i
      className={`pxicon${className ? ` ${className}` : ''}`}
      style={style}
      data-icon={name}
      data-icon-size={size}
      aria-hidden="true"
    />
  );
}
