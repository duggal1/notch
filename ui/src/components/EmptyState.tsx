// EmptyState.tsx — zero sessions is not a poster, it is the first action: one field, one key.
// Typing here stages the task and opens the dashboard with it in the composer.
import { useState } from 'react';

export function EmptyState({ onNewTask }: { onNewTask: () => void }) {
  const [draft, setDraft] = useState('');

  const submit = () => {
    const text = draft.trim();
    if (text) { try { localStorage.setItem('notch.draft', text); } catch { /* storage may be unavailable */ } }
    onNewTask();
  };

  return (
    <div className="empty">
      <span className="empty-title">Nothing running</span>
      <p className="muted">Describe the first task — it opens in the dashboard composer.</p>
      <form
        className="empty-form"
        onSubmit={(e) => { e.preventDefault(); submit(); }}
      >
        <input
          className="empty-input"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="What should the first agent do?"
          aria-label="First task"
          spellCheck={false}
        />
        <button className="empty-go ripple" type="submit" aria-label="Assign the first task">
          Assign
        </button>
      </form>
    </div>
  );
}
