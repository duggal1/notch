// EmptyState.tsx — zero sessions is not a poster, it is the first action.
// The composer is the hero; three real task templates seed it on click.
import { useState } from 'react';

const TEMPLATES = ['Fix a failing test', 'Review a pull request', 'Draft a changelog'];

export function EmptyState({ onNewTask }: { onNewTask: () => void }) {
  const [draft, setDraft] = useState('');

  const submit = () => {
    const text = draft.trim();
    if (text) { try { localStorage.setItem('notch.draft', text); } catch { /* storage may be unavailable */ } }
    onNewTask();
  };

  return (
    <div className="empty">
      <p className="muted">Describe the first task — it opens in the dashboard composer.</p>
      <div className="start-row">
        <span className="label">Start from</span>
        <div className="start-chips">
          {TEMPLATES.map((t) => (
            <button key={t} type="button" className="start-chip ripple" onClick={() => setDraft(t)}>{t}</button>
          ))}
        </div>
      </div>
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
        <button className="btn-primary empty-go ripple" type="submit" aria-label="Assign the first task">
          Assign
        </button>
      </form>
    </div>
  );
}
