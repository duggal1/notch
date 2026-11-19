// TreeList.tsx — Zone 3. ID-ordered sessions, white-hot wipe from pointer-entry (DESIGN.md §8).
import { useEffect, useRef, useState } from 'react';
import type { Session } from '../lib/bridge';
import { SessionRow } from './WhiteHotRow';

export function TreeList({ sessions, killingIds, onLogs, onReprompt, onFocus, onKill }: {
  sessions: Session[];
  killingIds: Set<string>;
  onLogs: (s: Session) => void;
  onReprompt: (s: Session) => void;
  onFocus: (id: string) => void;
  onKill: (id: string) => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  // white-hot delegation — the fill is a clip-path circle anchored to where the pointer entered,
  // and retracts toward where it left. Imperative: React state would blur the entry marker.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const origin = (el: HTMLElement, e: PointerEvent) => {
      const r = el.getBoundingClientRect();
      el.style.setProperty('--hx', (((e.clientX - r.left) / r.width) * 100).toFixed(1) + '%');
      el.style.setProperty('--hy', (((e.clientY - r.top) / r.height) * 100).toFixed(1) + '%');
    };
    const over = (e: PointerEvent) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>('.item');
      if (!el || el.contains(e.relatedTarget as Node | null)) return;
      origin(el, e);
      el.toggleAttribute('data-hot', true);
    };
    const out = (e: PointerEvent) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>('.item');
      if (!el || el.contains(e.relatedTarget as Node | null)) return;
      origin(el, e);
      el.toggleAttribute('data-hot', false);
    };
    const fin = (e: FocusEvent) => (e.target as HTMLElement).closest('.item')?.toggleAttribute('data-hot', true);
    const fout = (e: FocusEvent) => (e.target as HTMLElement).closest('.item')?.toggleAttribute('data-hot', false);
    root.addEventListener('pointerover', over);
    root.addEventListener('pointerout', out);
    root.addEventListener('focusin', fin);
    root.addEventListener('focusout', fout);
    return () => {
      root.removeEventListener('pointerover', over);
      root.removeEventListener('pointerout', out);
      root.removeEventListener('focusin', fin);
      root.removeEventListener('focusout', fout);
    };
  }, []);

  return (
    <div ref={rootRef} className="list-scroll h-full min-h-0 px-2 py-1" role="table" aria-label="Sessions">
      {sessions.map((s) => (
        <SessionRow
          key={s.id}
          session={s}
          now={now}
          killing={killingIds.has(s.id)}
          onLogs={onLogs}
          onReprompt={onReprompt}
          onFocus={onFocus}
          onKill={onKill}
        />
      ))}
    </div>
  );
}
