// NumberRoll.tsx — DESIGN.md §9.1. Per-digit spring columns + velocity blur. Never NumberFlow.
import { useEffect, useRef } from 'react';
import { Spring, ticker, reduced } from '../lib/motion';
import { SPRING } from '../lib/tokens';

type Col = { digit: boolean; node: HTMLSpanElement; strip?: HTMLSpanElement; sp: Spring };

export function NumberRoll({ value, format = (n: number) => n.toFixed(1), className }: {
  value: number;
  format?: (n: number) => string;
  className?: string;
}) {
  const rootRef = useRef<HTMLSpanElement>(null);
  const colsRef = useRef<Col[]>([]);
  const fmtRef = useRef(format);
  fmtRef.current = format;
  const tickRef = useRef<((dt: number) => boolean) | null>(null);

  if (!tickRef.current) {
    tickRef.current = (dt: number) => {
      let busy = false;
      for (const c of colsRef.current) {
        if (!c.digit || !c.strip) continue;
        const x = c.sp.step(dt);
        c.strip.style.transform = `translateY(${-x}em)`;
        c.strip.style.filter = `blur(${Math.min(1.4, Math.abs(c.sp.v) * 0.09).toFixed(2)}px)`;
        busy ||= !c.sp.settled;
      }
      return busy; // ticker unsubscribes us when idle; set() re-adds
    };
  }

  const build = (s: string) => {
    const root = rootRef.current;
    if (!root) return;
    root.textContent = '';
    const cols: Col[] = [];
    for (const ch of s) {
      const node = document.createElement('span');
      const digit = /\d/.test(ch);
      if (!digit) {
        node.textContent = ch;
        node.style.cssText = 'display:block;height:1em;line-height:1';
        root.append(node);
        cols.push({ digit, node, sp: new Spring(0, SPRING.digit) });
        continue;
      }
      node.style.cssText = 'display:block;height:1em;overflow:hidden;line-height:1';
      const strip = document.createElement('span');
      strip.style.cssText = 'display:block;will-change:transform';
      strip.innerHTML = Array.from({ length: 10 }, (_, d) => `<i style="display:block;height:1em;font-style:normal">${d}</i>`).join('');
      node.append(strip);
      root.append(node);
      cols.push({ digit, node, strip, sp: new Spring(+ch, SPRING.digit) });
    }
    colsRef.current = cols;
  };

  // mount: build at rest, no animation
  const valueRef = useRef(value);
  useEffect(() => {
    build(fmtRef.current(valueRef.current));
    ticker.add(tickRef.current!);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    valueRef.current = value;
    const s = fmtRef.current(value);
    if (s.length !== colsRef.current.length) build(s);
    const cols = colsRef.current;
    [...s].forEach((ch, i) => {
      const c = cols[i];
      if (!c) return;
      if (c.digit) c.sp.target = +ch; else c.node.textContent = ch;
    });
    if (reduced) {
      for (const c of cols) {
        if (!c.digit || !c.strip) continue;
        c.sp.x = c.sp.target; c.sp.v = 0;
        c.strip.style.transform = `translateY(${-c.sp.x}em)`; c.strip.style.filter = 'none';
      }
      return;
    }
    ticker.add(tickRef.current!);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  return (
    <span
      ref={rootRef}
      className={`inline-flex overflow-hidden leading-none ${className ?? ''}`}
      style={{ height: '1em' }}
      aria-label={fmtRef.current(value)}
    />
  );
}
