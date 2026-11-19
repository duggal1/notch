// Icons.tsx — one hand-authored family. 24×24, 1.75px stroke, round caps/joins, currentColor.
// Outline only: actions render exclusively on the white-hot row (ink on #FFFFFF),
// so a separate filled variant adds nothing. Never mixed with another family. No emoji.
type P = { size?: number; className?: string; strokeWidth?: number };
const base = (size = 16, sw = 1.75) => ({
  width: size, height: size, viewBox: '0 0 24 24', fill: 'none',
  stroke: 'currentColor', strokeWidth: sw, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const,
  'aria-hidden': true as const,
});

export function OrchestratorMark({ size = 14, className }: P) {
  return (
    <svg {...base(size, 1.5)} className={className}>
      <path d="M12 4.6 19.4 17.4H4.6Z" strokeDasharray="2.7 2.5" />
      <circle cx="12" cy="4.6" r="1.9" fill="currentColor" stroke="none" />
      <circle cx="19.4" cy="17.4" r="1.9" fill="currentColor" stroke="none" />
      <circle cx="4.6" cy="17.4" r="1.9" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function TerminalIcon({ size = 14, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <rect x="3.5" y="5" width="17" height="14" rx="2.4" />
      <path d="M7.5 10l3 3-3 3" />
      <path d="M12.5 16h4" />
    </svg>
  );
}

export function LogsIcon({ size = 14, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M6.5 8h11" /><path d="M6.5 12.25h11" /><path d="M6.5 16.5h7" />
    </svg>
  );
}

export function RepromptIcon({ size = 14, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M17.75 12a5.75 5.75 0 1 1-1.69-4.06" />
      <path d="M17.9 3.8v4.35h-4.35" />
    </svg>
  );
}

export function FocusIcon({ size = 14, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M10.25 5H5v14h14v-5.25" />
      <path d="M13.5 10.5 19 5" />
      <path d="M13.6 5H19v5.4" />
    </svg>
  );
}

export function KillIcon({ size = 14, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M7 7l10 10" /><path d="M17 7 7 17" />
    </svg>
  );
}

export function CopyIcon({ size = 13, className }: P) {
  return (
    <svg {...base(size, 1.6)} className={className}>
      <rect x="8.5" y="8.5" width="11" height="11" rx="2" />
      <path d="M15.5 5.5v-.6a2 2 0 0 0-2-2h-9a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h.6" />
    </svg>
  );
}

export function CheckIcon({ size = 13, className }: P) {
  return (
    <svg {...base(size, 1.9)} className={className}>
      <path d="M5.5 12.5l4.5 4.5L18.5 7" />
    </svg>
  );
}
