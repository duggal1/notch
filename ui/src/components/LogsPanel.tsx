// LogsPanel.tsx — in-panel log sheet (AnimatePresence). Real lines from the host via listen('logs').
import { AnimatePresence, motion } from 'motion/react';
import type { LogsPayload } from '../lib/bridge';
import { KillIcon } from './Icons';

export function LogsPanel({ open, payload, onClose }: { open: boolean; payload: LogsPayload | null; onClose: () => void }) {
  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="logs"
          className="absolute inset-x-0 bottom-0 z-30 flex flex-col overflow-hidden rounded-t-2xl border-t border-white/15"
          style={{ background: '#14153A', height: '62%' }}
          initial={{ y: '100%', opacity: 0.4 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: '100%', opacity: 0.4 }}
          transition={{ type: 'spring', stiffness: 380, damping: 34 }}
          role="dialog" aria-modal="true" aria-label={`Logs for ${payload?.id ?? ''}`}
        >
          <div className="flex items-center justify-between border-b border-white/10 px-5 py-2.5">
            <span className="label" style={{ color: 'rgba(255,255,255,.62)' }}>{payload?.id ?? '—'} — Logs</span>
            <button className="text-btn flex items-center gap-2" onClick={onClose} aria-label="Close logs">
              <span style={{ fontSize: 9 }}>esc</span>
              <KillIcon size={13} />
            </button>
          </div>
          <pre className="m-0 flex-1 overflow-auto whitespace-pre-wrap px-5 py-3 font-mono text-[10px] leading-[1.7]" style={{ color: 'rgba(255,255,255,.78)' }}>
            {payload?.lines.join('\n') ?? 'Waiting for log lines…'}
          </pre>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
