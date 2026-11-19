// Specular.tsx — pointer specular (DESIGN.md §7.4). Spring-lagged white light following the cursor.
import { useEffect, useRef } from 'react';
import { Spring, ticker, reduced } from '../lib/motion';
import { SPRING } from '../lib/tokens';

export function Specular({ shellRef }: { shellRef: React.RefObject<HTMLElement | null> }) {
  const specRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const shell = shellRef.current, spec = specRef.current;
    if (!shell || !spec || reduced) return;
    const x = new Spring(0.5, SPRING.hover), y = new Spring(0.0, SPRING.hover);
    const tick = (dt: number) => {
      spec.style.setProperty('--sx', (x.step(dt) * 100).toFixed(2) + '%');
      spec.style.setProperty('--sy', (y.step(dt) * 100).toFixed(2) + '%');
      return !(x.settled && y.settled);
    };
    const move = (e: PointerEvent) => {
      const r = shell.getBoundingClientRect();
      x.target = (e.clientX - r.left) / r.width;
      y.target = (e.clientY - r.top) / r.height;
      ticker.add(tick);
    };
    const enter = () => spec.style.setProperty('--spec', '1');
    const leave = () => spec.style.setProperty('--spec', '0');
    shell.addEventListener('pointermove', move);
    shell.addEventListener('pointerenter', enter);
    shell.addEventListener('pointerleave', leave);
    return () => {
      shell.removeEventListener('pointermove', move);
      shell.removeEventListener('pointerenter', enter);
      shell.removeEventListener('pointerleave', leave);
    };
  }, [shellRef]);

  return <div ref={specRef} className="spec" aria-hidden="true" />;
}
