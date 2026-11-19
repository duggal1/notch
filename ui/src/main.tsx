import '@fontsource-variable/inter';
import './styles/tokens.css';
import './styles/base.css';
import './styles/panel.css';
import { createRoot } from 'react-dom/client';
import App from './App';
import { isTauri } from './lib/bridge';

if (!isTauri()) document.body.classList.add('dev-stage');
if (isTauri()) document.documentElement.style.background = 'transparent';

const el = document.getElementById('root');
if (!el) throw new Error('#root missing');
createRoot(el).render(<App />);
