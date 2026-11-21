import '@fontsource-variable/instrument-sans';
import './styles/tokens.css';
import './styles/base.css';
import './styles/panel.css';
import './styles/dashboard.css';
import { createRoot } from 'react-dom/client';
import { lazy, Suspense } from 'react';
import App from './App';
import { isTauri } from './lib/bridge';
import { installProbe } from './lib/probe';

// The dashboard window (motion, all tabs, fleet deck, WebGL wave) is a
// separate surface: never load it for the popover.
const Dashboard = lazy(() =>
  import('./components/Dashboard').then((m) => ({ default: m.Dashboard })),
);

const surface = new URLSearchParams(window.location.search).get('surface') ?? 'popover';

if (!isTauri()) document.body.classList.add('dev-stage');
document.body.dataset.surface = surface;
document.documentElement.style.background = 'transparent';
installProbe();

const el = document.getElementById('root');
if (!el) throw new Error('#root missing');
createRoot(el).render(
  <Suspense fallback={null}>
    {surface === 'dashboard' ? <Dashboard /> : <App />}
  </Suspense>,
);
(window as unknown as { __ready?: boolean }).__ready = true;