// GlowOrb.tsx — rim-bloom WebGL2 orb (DESIGN.md §7.2). rAF lives only while the panel is open.
import { useEffect, useRef, type ReactNode } from 'react';
import { GlowOrb } from '../lib/glow-orb';
import type { Status } from '../lib/tokens';

export function Orb({ status, running, children }: { status: Status; running: boolean; children: ReactNode }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const orbRef = useRef<GlowOrb | null>(null);

  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    let orb: GlowOrb | null = null;
    try {
      orb = new GlowOrb(cv, { radius: 36, seed: 1.3, fps: 60 });
      orb.resize(216, 216);
      orbRef.current = orb;
    } catch (err) {
      console.warn('[notch] webgl2 unavailable — orb falls back to static CSS halo', err);
      cv.style.display = 'none';
      cv.parentElement?.classList.add('orb--glowless');
    }
    const lost = (e: Event) => { e.preventDefault(); orb?.stop(); };
    const restore = () => { if (running) orb?.start(); };
    cv.addEventListener('webglcontextlost', lost);
    cv.addEventListener('webglcontextrestored', restore);
    return () => {
      cv.removeEventListener('webglcontextlost', lost);
      cv.removeEventListener('webglcontextrestored', restore);
      orb?.stop();
      orbRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const orb = orbRef.current;
    if (!orb) return;
    orb.setStatus(status);
    if (running) { orb.setFps(60); orb.start(); }
    else { orb.stop(); orb.renderOnce(); } // rest: one static render per status change
  }, [status, running]);

  return (
    <div className="orb" aria-hidden="true">
      <canvas ref={canvasRef} />
      <div className="orb-content">{children}</div>
    </div>
  );
}
