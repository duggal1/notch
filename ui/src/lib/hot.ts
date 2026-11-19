// hot.ts — white-hot wipe delegation (DESIGN.md §8). The fill enters from where the pointer
// went in and retracts toward where it left; ink flips with it. Directionality is the physicality.
import { useEffect, type RefObject } from 'react';

export function whiteHover(root: HTMLElement): () => void {
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
}

export function useWhiteHot(ref: RefObject<HTMLElement | null>): void {
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    return whiteHover(root);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
