import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, screenshotIfRequested, session } from './tauri-ui-harness.js';

async function choose(theme: string): Promise<void> {
  await $('#show-theme').click();
  await $(`[data-theme-choice="${theme}"]`).click();
  await browser.pause(100);
}

async function appearance(): Promise<{ app: string; background: string; terminal: string; stored: string; systemLight: boolean }> {
  return js(`({app:document.documentElement.dataset.theme,
    background:getComputedStyle(document.body).backgroundColor,
    terminal:window.__testTerminalState()?.theme.background,
    stored:localStorage.getItem('buoy.theme'),
    systemLight:matchMedia('(prefers-color-scheme: light)').matches})`);
}

describe('Tauri UI: appearance', () => {
  let original: string | null;
  before(async () => {
    original = await js(`localStorage.getItem('buoy.theme')`);
    await js(`localStorage.removeItem('buoy.theme')`);
    await browser.refresh();
    await browser.setWindowSize(1100, 700);
    await loadFixture([session(1, 'Buoy development')]);
    await fire('window', { id: 's1', action: 'add', window: '@0', order: ['@0'] });
    await fire('window', { id: 's1', action: 'active', window: '@0', order: ['@0'] });
    await fire('state', { id: 's1', state: 'connected' });
    await fire('ready', { id: 's1' });
    await fire('data', { id: 's1', window: '@0', data: '\x1b[34mBuoy\x1b[0m\r\n\r\n$ npm test\r\n\x1b[32mTests passed\x1b[0m\r\n\r\n$ ' });
    await browser.pause(150);
  });
  after(async () => {
    await js(`(() => {
      const value = ${JSON.stringify(original)};
      if(value === null) localStorage.removeItem('buoy.theme'); else localStorage.setItem('buoy.theme',value);
      window.dispatchEvent(new StorageEvent('storage',{key:'buoy.theme',newValue:value}));
    })()`);
  });

  it('defaults to dark blue, synchronizes live terminals, and persists the choice', async () => {
    assert.equal((await appearance()).app, 'dark');
    assert.equal((await appearance()).terminal, '#1e1e2e');
    await js(`window.__themeTerminalElement = document.querySelector('#term .xterm')`);
    const state = await js('window.__testTerminalState()');
    const buffer = await js('window.__testReadBuffer()');
    await choose('light');
    assert.equal((await appearance()).app, 'light');
    assert.equal((await appearance()).terminal, '#eff1f5');
    assert.equal((await appearance()).stored, 'light');
    assert.equal(await js('window.__themeTerminalElement === document.querySelector("#term .xterm")'), true);
    assert.equal(await js('window.__testReadBuffer()'), buffer);
    assert.equal((await js('window.__testTerminalState()')).cursorX, state.cursorX);
    assert.equal((await js('window.__testTerminalState()')).rows, state.rows);
    await screenshotIfRequested('theme-light.png');
    await choose('system');
    const system = await appearance();
    assert.equal(system.app, system.systemLight ? 'light' : 'dark');
    assert.equal(system.terminal, system.systemLight ? '#eff1f5' : '#1e1e2e');
    assert.equal(await js(`window.__invocations.filter(([name]) => name === 'set_theme').at(-1)[1].theme`), null);
    await choose('dark');
    assert.equal((await appearance()).app, 'dark');
    await screenshotIfRequested('theme-dark.png');
    await $('#show-theme').click();
    assert.equal(await $('[data-theme-choice="dark"]').getAttribute('aria-pressed'), 'true');
    await screenshotIfRequested('theme-picker.png');
    await browser.keys('Escape');
    assert.equal(await js('document.querySelectorAll("dialog[open]").length'), 0);
    await browser.refresh();
    await browser.waitUntil(async () => js('document.documentElement.dataset.theme === "dark"'));
    assert.equal((await appearance()).stored, 'dark');
    assert.deepEqual(await js('window.__errs'), []);
  });
});
