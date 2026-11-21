// Heartbeat.tsx — dot-matrix events/s field (DESIGN.md §9.2). Samples the latest eps every 240 ms while open.
import { useEffect, useRef } from 'react';
import { Heartbeat } from '../lib/heartbeat';
import { reduced } from '../lib/motion';
import { normalizeEps } from '../lib/format';

export function HeartbeatField({ epsRef, active }: { epsRef: React.MutableRefObject<number>; active: boolean }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const beatRef = useRef<Heartbeat | null>(null);

  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    const beat = new Heartbeat(cv, 11, 5);
    beatRef.current = beat;
    beat.drawOnce(); // paint the empty field immediately (dots at 22%)
  }, []);

  useEffect(() => {
    if (!active) return;
    beatRef.current?.push(normalizeEps(epsRef.current));
    if (reduced) return;
    const id = window.setInterval(() => beatRef.current?.push(normalizeEps(epsRef.current)), 240);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  return <canvas ref={canvasRef} className="heartbeat block w-full" aria-hidden="true" />;
}
