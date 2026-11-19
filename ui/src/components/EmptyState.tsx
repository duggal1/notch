// EmptyState.tsx — zero sessions: one line, one action. Nothing else.
export function EmptyState({ onNewTask }: { onNewTask: () => void }) {
  return (
    <div className="empty">
      <span className="empty-title">Nothing running</span>
      <button className="new-task-btn" onClick={onNewTask}>New task</button>
    </div>
  );
}