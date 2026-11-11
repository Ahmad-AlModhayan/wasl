import '@kahf/design/styles/kahf.css';
import '@kahf/design/styles/desktop.css';
import './styles/app.css';
import './styles/chrome-mac.css';
import './styles/sheets.css';
import './styles/settings.css';
import './styles/states.css';
import { App } from './app';
import { mountChrome } from './chrome';
import { platformOf, type Platform } from './platform';
import { isTrayWindow } from './tray/model';

/** The Tauri window this page runs in; undefined in a plain browser. */
function windowLabel(): string | undefined {
  const internals = (window as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } }).__TAURI_INTERNALS__;
  return internals?.metadata?.currentWindow?.label;
}

async function main(): Promise<void> {
  const root = document.documentElement;
  root.dataset.kfPlatform = platformOf(navigator.userAgent);
  // The tray's popover and flyout (#69) load this same bundle in their own window.
  const tray = isTrayWindow(location.search, windowLabel());
  // macOS opens Settings as its own window on the same page (#68, src-tauri/src/settings_window.rs).
  const settings = !tray && new URLSearchParams(location.search).get('window') === 'settings';
  // Development in a plain browser: answer the app's commands from captured data (src/dev).
  if (import.meta.env.DEV && !('__TAURI_INTERNALS__' in window)) {
    const mock = await import('./dev/mock');
    // ?platform=mac|windows and ?theme=light|dark, so a browser can show every combination.
    const { platform, theme, state } = mock.devOverrides(location.search);
    if (platform) root.dataset.kfPlatform = platform;
    if (theme) document.head.append(Object.assign(document.createElement('meta'), { name: 'color-scheme', content: theme }));
    // ?state=stopped|notfound|connecting|offline|auth|quota|missing|first|empty: the app's states (#70).
    if (state) mock.setMockState(state);
    mock.installMock(root.dataset.kfPlatform === 'mac' ? 'mac' : 'windows', tray ? 'tray' : settings ? 'settings' : 'main');
    if (tray) root.classList.add('tray-preview');
    else if (root.dataset.kfPlatform === 'mac') mock.fakeMacWindow();
  }
  if (tray) {
    const { mountTray } = await import('./tray/popover');
    await mountTray();
    return;
  }
  if (settings) {
    root.classList.add('settings-root');
    const { SettingsWindow } = await import('./settingsWindow');
    await new SettingsWindow().start();
    return;
  }
  await mountChrome(root.dataset.kfPlatform as Platform);
  await new App().start();
}

void main();
