export type ThemePreference = 'dark' | 'light' | 'system';
export type Theme = 'dark' | 'light';
const storageKey = 'buoy.theme';
let systemAppearance: MediaQueryList;
const listeners = new Set<() => void>();
let preference: ThemePreference = 'dark';

export function getThemePreference(): ThemePreference { return preference; }
export function getTheme(): Theme {
  return preference === 'system' ? (systemAppearance.matches ? 'light' : 'dark') : preference;
}
function applyTheme(): void {
  document.documentElement.dataset.theme = getTheme();
  document.documentElement.style.colorScheme = getTheme();
  for (const listener of listeners) listener();
}
export function setThemePreference(value: ThemePreference): void {
  preference = value;
  try { localStorage.setItem(storageKey, value); } catch (_) {}
  applyTheme();
}
export function onThemeChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
// Initialize from the renderer entry point so importing terminal utilities has no DOM side effects.
export function initializeTheme(): void {
  if (systemAppearance) return;
  systemAppearance = window.matchMedia('(prefers-color-scheme: light)');
  try {
    const saved = localStorage.getItem(storageKey);
    if (saved === 'dark' || saved === 'light' || saved === 'system') preference = saved;
  } catch (_) { /* Theme changes still work when storage is unavailable. */ }
  systemAppearance.addEventListener('change', () => { if (preference === 'system') applyTheme(); });
  window.addEventListener('storage', event => {
    if (event.key !== storageKey && event.key !== null) return;
    const saved = event.newValue;
    preference = saved === 'light' || saved === 'system' ? saved : 'dark';
    applyTheme();
  });
  applyTheme();
}

export function terminalTheme(): NonNullable<XtermTerminalOptions['theme']> {
  return getTheme() === 'light' ? {
    background: '#eff1f5', foreground: '#4c4f69', cursor: '#1e66f5', cursorAccent: '#eff1f5',
    selectionBackground: '#bccdf3',
    black: '#4c4f69', red: '#d20f39', green: '#2a7533', yellow: '#8a5d15',
    blue: '#1e66f5', magenta: '#8839ef', cyan: '#087f8c', white: '#bcc0cc',
    brightBlack: '#6c6f85', brightRed: '#d20f39', brightGreen: '#2a7533', brightYellow: '#8a5d15',
    brightBlue: '#1e66f5', brightMagenta: '#8839ef', brightCyan: '#087f8c', brightWhite: '#ffffff',
  } : {
    background: '#1e1e2e', foreground: '#cdd6f4', cursor: '#89b4fa', cursorAccent: '#1e1e2e',
    selectionBackground: '#45475a',
    black: '#45475a', red: '#f38ba8', green: '#a6e3a1', yellow: '#f9e2af',
    blue: '#89b4fa', magenta: '#cba6f7', cyan: '#94e2d5', white: '#bac2de',
    brightBlack: '#585b70', brightRed: '#f38ba8', brightGreen: '#a6e3a1', brightYellow: '#f9e2af',
    brightBlue: '#89b4fa', brightMagenta: '#cba6f7', brightCyan: '#94e2d5', brightWhite: '#a6adc8',
  };
}
