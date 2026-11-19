// Symbol.tsx — SF Symbols rasterised on-device by the host (invoke('symbol')), painted via mask + currentColor.
// If the host command fails you get the hand-drawn fallback AND one console warning saying why.
import { useEffect, useState, type ComponentType, type CSSProperties } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauri } from '../lib/bridge';

const cache = new Map<string, string>();
let warned = false;

function useSymbolUri(name: string, pt: number, weight: 'regular' | 'medium' | 'semibold'): string {
  const [uri, setUri] = useState<string>(() => cache.get(`${name}|${pt}|${weight}`) ?? '');
  useEffect(() => {
    const key = `${name}|${pt}|${weight}`;
    const cached = cache.get(key);
    if (cached !== undefined) { setUri(cached); return; }
    if (!isTauri()) { cache.set(key, ''); setUri(''); return; }
    let alive = true;
    invoke<string>('symbol', { name, pt, weight })
      .then((u) => { const v = u ?? ''; cache.set(key, v); if (alive) setUri(v); })
      .catch((err) => {
        if (!warned) { warned = true; console.warn('[notch] SF Symbol bridge failed — using fallback icons. Is the `symbol` command registered and returning a data:image/png URI?', err); }
        cache.set(key, ''); if (alive) setUri('');
      });
    return () => { alive = false; };
  }, [name, pt, weight]);
  return uri;
}

export function Sym({ name, pt = 16, weight = 'semibold', className, fallback: Fallback }: {
  name: string;
  pt?: number;
  weight?: 'regular' | 'medium' | 'semibold';
  className?: string;
  fallback: ComponentType<{ size?: number; className?: string }>;
}) {
  const uri = useSymbolUri(name, pt, weight);
  if (uri) {
    return <i className={`sym ${className ?? ''}`} style={{ '--pt': `${pt}px`, '--sym': `url("${uri}")` } as CSSProperties} aria-hidden="true" />;
  }
  return <Fallback size={pt} className={className} />;
}