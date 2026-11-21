import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';

type FixtureInput = { snapshot: unknown };
export function install({ snapshot }: FixtureInput) {
  mockWindows('dashboard', 'main');
  mockIPC((command, args) => {
    window.dispatchEvent(new CustomEvent('test-ipc', { detail: { command, args } }));
    switch (command) {
      case 'snapshot': return snapshot;
      case 'logs': {
        const id = typeof args === 'object' && args && 'id' in args ? args.id : '';
        return { id, lines: ['Reading session output', 'Unit tests completed', 'Waiting for the next command'] };
      }
      case 'list_models': return { models: ['auto', 'opencode/example'] };
      case 'submit_task': return { ok: true, sessionId: 'test-assignment' };
      case 'reprompt': return { ok: true };
      case 'cleanup': return 1;
      case 'open_dashboard': return { ok: true };
      case 'list_harnesses': return { harnesses: [] };
      case 'list_approvals': return { approvals: [] };
      default: return undefined;
    }
  }, { shouldMockEvents: true });
}
