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

export function TrashIcon({ size = 14, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M5.5 7h13" />
      <path d="M10 7V5.25A1.25 1.25 0 0 1 11.25 4h1.5A1.25 1.25 0 0 1 14 5.25V7" />
      <path d="M7.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.4h4.4a1.5 1.5 0 0 0 1.5-1.4l.8-12" />
      <path d="M10.6 11v5.5" /><path d="M13.9 11v5.5" />
    </svg>
  );
}

export function BubbleIcon({ size = 14, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5v7a1.5 1.5 0 0 1-1.5 1.5H12l-4.2 3.6A.5.5 0 0 1 7 18.2V15H6.5A1.5 1.5 0 0 1 5 13.5Z" />
    </svg>
  );
}

export function PauseIcon({ size = 13, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M10 5.5v13" /><path d="M14.5 5.5v13" />
    </svg>
  );
}

export function PlayIcon({ size = 13, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M8.5 6.3v11.4a.8.8 0 0 0 1.22.68l9.3-5.7a.8.8 0 0 0 0-1.36l-9.3-5.7A.8.8 0 0 0 8.5 6.3Z" />
    </svg>
  );
}

export function ChevronDownIcon({ size = 12, className }: P) {
  return (
    <svg {...base(size, 1.9)} className={className}>
      <path d="M6 9.5l6 6 6-6" />
    </svg>
  );
}

export function CloseIcon({ size = 10, className }: P) {
  return (
    <svg {...base(size, 1.9)} className={className}>
      <path d="M7 7l10 10M17 7L7 17" />
    </svg>
  );
}

export function SearchIcon({ size = 13, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <circle cx="11" cy="11" r="6.5" />
      <path d="M16 16l4.5 4.5" />
    </svg>
  );
}

export function GridIcon({ size = 13, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <rect x="4" y="4" width="6.5" height="6.5" rx="1.2" /><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.2" />
      <rect x="4" y="13.5" width="6.5" height="6.5" rx="1.2" /><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.2" />
    </svg>
  );
}

export function ListIcon({ size = 13, className }: P) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M5 6.5h14M5 12h14M5 17.5h14" />
    </svg>
  );
}

export function PlusIcon({ size = 12, className }: P) {
  return (
    <svg {...base(size, 2)} className={className}>
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}
