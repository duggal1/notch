import { useEffect, type RefObject } from 'react';

export function useRipple(rootRef: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const preference = matchMedia('(prefers-reduced-motion: reduce)');
    const onPress = (event: PointerEvent) => {
      if (preference.matches || event.button !== 0 || !(event.target instanceof Element)) return;
      const button = event.target.closest<HTMLElement>('button, [data-ripple]');
      if (!button || !root.contains(button) || button.matches(':disabled, [aria-disabled="true"]')) return;
      const bounds = button.getBoundingClientRect();
      const ripple = document.createElement('span');
      ripple.className = 'press-ripple';
      ripple.style.left = `${event.clientX - bounds.left}px`;
      ripple.style.top = `${event.clientY - bounds.top}px`;
      ripple.style.width = `${Math.max(bounds.width, bounds.height) * 2}px`;
      ripple.setAttribute('aria-hidden', 'true');
      ripple.addEventListener('animationend', () => ripple.remove(), { once: true });
      button.append(ripple);
    };
    root.addEventListener('pointerdown', onPress);
    return () => root.removeEventListener('pointerdown', onPress);
  }, [rootRef]);
}
