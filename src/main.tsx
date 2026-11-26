import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauri } from './api';
import { MainApp } from './main/MainApp';
import { Overlay } from './overlay/Overlay';
import { initStore, reportError } from './store';
import './styles.css';
import './shortcuts.css';

function windowLabel(): string {
  if (isTauri) return getCurrentWindow().label;
  // In a plain browser, add ?window=overlay to preview the overlay.
  return new URLSearchParams(window.location.search).get('window') === 'overlay' ? 'overlay' : 'main';
}

const label = windowLabel();
document.documentElement.dataset.window = label;
document.documentElement.dataset.tauri = isTauri ? 'true' : 'false';

initStore().catch((e) => reportError('startup', e));

const root = document.getElementById('root');
if (root) {
  createRoot(root).render(<StrictMode>{label === 'overlay' ? <Overlay /> : <MainApp />}</StrictMode>);
}
