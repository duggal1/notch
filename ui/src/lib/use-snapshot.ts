// use-snapshot.ts — one subscription per surface.
import { useEffect, useState } from 'react';
import { subscribe, type Snapshot } from './bridge';

export function useSnapshot(): Snapshot | null {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  useEffect(() => {
    let alive = true;
    const unsub = subscribe({ onSnapshot: (s) => { if (alive) setSnapshot(s); } });
    return () => { alive = false; unsub.then((fn) => fn?.()).catch(() => undefined); };
  }, []);
  return snapshot;
}
