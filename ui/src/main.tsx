import '@fontsource-variable/inter';
import './styles/tokens.css';
import './styles/base.css';
import './styles/panel.css';
import './styles/dashboard.css';
import { createRoot } from 'react-dom/client';
import App from './App';
import { Dashboard } from './components/Dashboard';
import { isTauri } from './lib/bridge';
import { installProbe } from './lib/probe';

const surface = new URLSearchParams(window.location.search).get('surface') ?? 'popover';

if (!isTauri()) document.body.classList.add('dev-stage');
document.body.dataset.surface = surface;
document.documentElement.style.background = 'transparent';
installProbe();

const el = document.getElementById('root');
if (!el) throw new Error('#root missing');
createRoot(el).render(surface === 'dashboard' ? <Dashboard /> : <App />);
(window as unknown as { __ready?: boolean }).__ready = true;