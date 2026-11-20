// EmptyState.tsx — zero sessions: one halftone core, one line, one action. Nothing else.
import { PixelSphere } from './pixel/PixelSphere';

export function EmptyState({ onNewTask }: { onNewTask: () => void }) {
  return (
    <div className="empty">
      <PixelSphere size={64} cell={3} live />
      <span className="empty-title">Nothing running</span>
      <button className="new-task-btn" onClick={onNewTask}>New task</button>
    </div>
  );
}
