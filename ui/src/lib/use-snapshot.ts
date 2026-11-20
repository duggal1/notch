// use-snapshot.ts — one subscription per surface.
import { useEffect, useState } from 'react';
import { subscribe, type Snapshot } from './bridge';
import { FLEET_EVENT } from './ws-transport';

export function useSnapshot(): Snapshot | null {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  useEffect(() => {
    let alive = true;
    // Additive live-fleet path: a single `fleet` event carries the same §1.3 Snapshot
    // shape, so existing selectors, filters and layouts need no change. Identical
    // objects from both paths are deduped by React's Object.is bailout.
    const onFleetEvent = (e: Event) => {
      const s = (e as CustomEvent<Snapshot>).detail;
      if (alive && s && Array.isArray(s.sessions)) setSnapshot(s);
    };
    window.addEventListener(FLEET_EVENT, onFleetEvent);
    const unsub = subscribe({ onSnapshot: (s) => { if (alive) setSnapshot(s); } });
    return () => {
      alive = false;
      window.removeEventListener(FLEET_EVENT, onFleetEvent);
      unsub.then((fn) => fn?.()).catch(() => undefined);
    };
  }, []);
  return snapshot;
}
