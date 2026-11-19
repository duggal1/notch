// EmptyState.tsx — zero sessions. The first action, not an apology (TASTE.md §4.4).
// The wire has no runPlan message from UI, so the honest affordance is the literal command.
import { useState } from 'react';
import { CopyIcon, CheckIcon } from './Icons';

const CMD = 'notchd run-plan examples/two-by-two.json';

export function EmptyState() {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(CMD);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch {
      // clipboard unavailable (headless / insecure context) — show the command anyway, select-all fallback
      setCopied(false);
      const el = document.createElement('textarea');
      el.value = CMD; document.body.append(el); el.select();
      try { document.execCommand('copy'); } catch { /* nothing else to do */ }
      el.remove();
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    }
  };
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center">
      <span className="label" style={{ color: 'rgba(255,255,255,.44)' }}>Nothing in flight</span>
      <p className="title m-0 max-w-[340px]" style={{ color: 'rgba(255,255,255,.92)' }}>
        Point notch at a plan and it runs.
      </p>
      <button className="cmd-chip mt-0.5" onClick={copy} aria-label="Copy run-plan command">
        <span>{CMD}</span>
        {copied ? <CheckIcon size={13} className="text-lilac" /> : <CopyIcon size={13} className="opacity-60" />}
      </button>
      <span className="mono" style={{ fontSize: 8.5, color: 'rgba(255,255,255,.32)' }}>
        {copied ? 'COPIED' : 'CLICK TO COPY'}
      </span>
    </div>
  );
}
