import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, screenshotIfRequested, session } from './tauri-ui-harness.js';

const tunnels = () => ({ s1: [{ remote: 3000, local: 53000, active: true }, { remote: 5173, local: 5173, active: true }], s2: [{ remote: 8080, local: 58080, active: true }] });
const calls = (command: string) => js(`window.__invocations.filter(([name]) => name === ${JSON.stringify(command)})`);

async function workspace(reject = {}): Promise<void> {
  await loadFixture([session(1, 'Buoy development'), session(2, 'Production'), { ...session(3, 'Local tools'), host: '', transport: 'local' }], {}, { tunnels: tunnels(), reject });
  for (const [i, name] of ['shell', 'codex', 'logs'].entries()) {
    await fire('window', { id: 's1', action: 'add', window: '@' + i, order: ['@0', '@1', '@2'] });
    await fire('window', { id: 's1', action: 'rename', window: '@' + i, name });
  }
  await fire('window', { id: 's1', action: 'active', window: '@0', order: ['@0', '@1', '@2'] });
  await fire('state', { id: 's1', state: 'connected' });
  await fire('state', { id: 's2', state: 'connected' });
  await fire('ready', { id: 's1' });
  await fire('data', { id: 's1', window: '@0', data: '\u001b[32mdev@workstation\u001b[0m ~/projects/buoy\r\n$ git status --short\r\n M ui/src/renderer.ts\r\n\r\n$ npm test\r\n\u001b[32mTests passed\u001b[0m\r\n\r\n$ npm run dev\r\nLocal: http://localhost:3000\r\n\r\n$ ' });
}

describe('Tauri UI: compact blue workspace', () => {
  beforeEach(async () => { await browser.setWindowSize(1100, 700); await workspace(); });

  it('keeps icon actions visible, and refits the same terminal when the sidebar returns', async () => {
    assert.equal(await js(`document.querySelector('#sidebar h1') === null`), true);
    const geometry = await js(`({ width:innerWidth, sidebar:parseFloat(getComputedStyle(document.getElementById('sidebar')).width) })`);
    assert.equal(geometry.sidebar, geometry.width <= 560 ? 164 : geometry.width <= 760 ? 172 : 184);
    const controls = await js(`Array.from(document.querySelectorAll('.tforce,.topen,.tclose,.workspace-menu')).map(el => ({ visibility:getComputedStyle(el).visibility, label:el.getAttribute('aria-label'), width:el.getBoundingClientRect().width }))`);
    assert.ok(controls.every((c: { visibility: string; label: string; width: number }) => c.visibility === 'visible' && c.label && c.width > 0));
    await js(`window.__compactTerminal = document.querySelector('#term .xterm')`);
    await $('#collapse-sidebar').click();
    assert.equal(await $('#restore-sidebar').isDisplayed(), true);
    await $('#restore-sidebar').click();
    await browser.pause(150);
    assert.equal(await js(`window.__compactTerminal === document.querySelector('#term .xterm')`), true);
    await screenshotIfRequested('workspace-native.png', 'BUOY_COMPACT_SCREENSHOTS');
  });

  it('manages ports inline, preserves owner identity and ignores duplicate same-port clicks', async () => {
    const prefix = '#sessions .session[data-id="s1"]';
    await $(prefix + ' .tunnels-toggle').click();
    assert.equal(await $$(prefix + ' .tunnel').length, 2);
    await js(`(() => { const b = document.querySelector('${prefix} .tforce'); b.click(); b.click(); })()`);
    await browser.waitUntil(async () => (await calls('force_forward')).length === 1);
    await browser.waitUntil(async () => js(`document.querySelector('${prefix} .tunnel-link').textContent === '3000'`));
    assert.equal(await js(`document.querySelector('${prefix} .tforce').getAttribute('aria-disabled')`), 'true');
    assert.equal(await js(`document.querySelectorAll('dialog[open]').length`), 0);
    await $('#sessions .session[data-id="s2"] .topen').click();
    const opened = await calls('open_external');
    assert.equal(opened.at(-1)?.[1]?.url, 'http://localhost:58080/');
    assert.equal(await js(`document.querySelector('.session.active').dataset.id`), 's1');
    await $(prefix + ' .tunnel[data-remote="5173"] .tclose').click();
    await browser.waitUntil(async () => js(`document.querySelectorAll('${prefix} .tunnel').length === 1`));
    assert.equal((await calls('close_tunnel')).at(-1)?.[1]?.remote, 5173);
    assert.equal((await calls('session_close')).length, 0);
    assert.deepEqual(await js('window.__errs'), []);
  });

  it('keeps the previous mapping on conflict and requires reconnect after a dropped connection', async () => {
    await workspace({ force_forward: 'local port 3000 is already in use' });
    await $('.session[data-id="s1"] .tforce').click();
    await browser.waitUntil(async () => js(`!!document.querySelector('.tunnel-error')`));
    assert.equal(await $('.session[data-id="s1"] .tunnel-link').getText(), '3000→53000');
    assert.equal(await js(`document.querySelectorAll('dialog[open]').length`), 0);
    await fire('state', { id: 's1', state: 'reconnecting' });
    const before = (await calls('force_forward')).length;
    await $('.session[data-id="s1"] .tforce').click();
    assert.equal((await calls('force_forward')).length, before);
    assert.match(await $('.session[data-id="s1"] .tunnel-error').getAttribute('title') || '', /Reconnect/);
  });

  it('confirms terminal destruction and keeps History from resizing hidden terminals', async () => {
    await $('#tabs .tab .tclose').click();
    assert.equal((await calls('tab_close')).length, 0);
    await $('.action-dialog [aria-label="Cancel"]').click();
    assert.equal((await calls('tab_close')).length, 0);
    await $('#tabs .tab .tclose').click();
    await $('[data-confirm="accept"]').click();
    assert.equal((await calls('tab_close')).length, 1);
    await $('#show-history').click();
    assert.equal(await $('#history-panel').isDisplayed(), true);
    const resizes = (await calls('session_resize')).length;
    await fire('window', { id: 's1', action: 'add', window: '@3', order: ['@0', '@1', '@2', '@3'] });
    await fire('data', { id: 's1', window: '@0', data: '\u001b]9;Background work done\u0007' });
    assert.equal(await js(`!!document.querySelector('.session[data-id="s1"] .notification-dot')`), true);
    await browser.pause(150);
    assert.equal((await calls('session_resize')).length, resizes);
    await $('#history-back').click();
    assert.equal(await $('#term').isDisplayed(), true);
  });
});
