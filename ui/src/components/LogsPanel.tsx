// LogsPanel.tsx — in-panel log sheet. Solid near-black, spring in.
import { AnimatePresence, motion } from 'motion/react';
import type { LogsPayload } from '../lib/bridge';
import { PixelIcon } from './PixelIcon';

export function LogsPanel({ open, payload, onClose }: { open: boolean; payload: LogsPayload | null; onClose: () => void }) {
  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="logs"
          className="logs-sheet"
          initial={{ y: '100%', opacity: 0.4 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: '100%', opacity: 0.4 }}
          transition={{ type: 'spring', stiffness: 380, damping: 34 }}
          role="dialog" aria-modal="true" aria-label={`Logs for ${payload?.id ?? ''}`}
        >
          <div className="logs-head">
            <span className="label" style={{ color: 'var(--t2)' }}>{payload?.id ?? '—'} · Logs</span>
            <button className="text-btn" onClick={onClose} aria-label="Close logs">
              <span className="dim">esc</span>
              <PixelIcon name="close" size={14} />
            </button>
          </div>
          <pre className="logs-pre">{payload?.lines.join('\n') ?? 'Waiting for log lines…'}</pre>
        </motion.div>
      )}
    </AnimatePresence>
  );
}