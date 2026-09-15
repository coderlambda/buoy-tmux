import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, session, screenshotIfRequested } from './tauri-ui-harness.js';

const previewRoot = '#term .fv-root:not([style*="display: none"])';
const path = '/tmp/report.html';
const html = '<h1>Preview</h1><button onclick="this.textContent=\'Running\'">Run JavaScript</button>';
const order = () => js(`Array.from(document.querySelectorAll('#tabs .tab:not(.plus) .ttext'), el => el.textContent)`);
const active = () => js(`document.querySelector('#tabs .tab.active .ttext')?.textContent ?? null`);
const calls = (name: string) => js(`window.__invocations.filter(([name]) => name === ${JSON.stringify(name)})`);
const select = async (title: string) => { await $(`#tabs .tlabel[title^="${title} ·"]`).click(); };
const close = async (title: string) => { await $(`#tabs .tab:has(.tlabel[title^="${title} ·"])`).$('.tclose').click(); };
const windowEvent = (action: string, window: string, extra = {}) => fire('window', { id: 's1', action, window, ...extra });

async function fixture(customOrder = ['@0', '@1', '@2'], backend = {}): Promise<void> {
  await loadFixture([{ ...session(1, 'Preview workspace'), tabOrder: customOrder }], {}, {
    files: { [path]: { data_b64: Buffer.from(html).toString('base64'), size: Buffer.byteLength(html), truncated: false } },
    ...backend,
  });
  for (const [id, name] of [['@0', 'First'], ['@1', 'Source'], ['@2', 'Last']]) {
    await windowEvent('add', id!, { order: ['@0', '@1', '@2'] });
    await windowEvent('rename', id!, { name });
  }
  await windowEvent('active', '@1');
  await fire('state', { id: 's1', state: 'connected' });
  await fire('ready', { id: 's1' });
  const E = '\x1b';
  await fire('data', { id: 's1', window: '@1', data: `${E}]8;;file://${path}${E}\\OPEN_REPORT${E}]8;;${E}\\\r\n` });
  await browser.waitUntil(async () => !!await js(`window.__testFindText('OPEN_REPORT')`));
}

async function clickLink(chooser = false, label = 'OPEN_REPORT', opens = true): Promise<void> {
  const point = await js(`(() => {
    const cell = window.__testFindText(${JSON.stringify(label)});
    const screen = Array.from(document.querySelectorAll('#term .xterm-screen')).find(el => el.getBoundingClientRect().width > 0).getBoundingClientRect();
    const size = window.__testTerminalState();
    return { x: Math.round(screen.left + (cell.x + 2) * screen.width / size.cols),
      y: Math.round(screen.top + (cell.y + .5) * screen.height / size.rows) };
  })()`);
  await browser.action('pointer').move({ ...point, origin: 'viewport' }).perform();
  await browser.pause(150);
  if (chooser) {
    // The embedded driver drops held modifiers on mouse events, as it does for Shift+Enter.
    // Keep native hover/hit-testing, then dispatch the combined gesture to the same DOM target.
    await js(`(() => {
      const target = document.elementFromPoint(${point.x}, ${point.y});
      for (const type of ['mousedown', 'mouseup']) target.dispatchEvent(new MouseEvent(type, {
        bubbles: true, cancelable: true, button: 0, shiftKey: true, clientX: ${point.x}, clientY: ${point.y}
      }));
    })()`);
  } else {
    await browser.action('pointer').move({ ...point, origin: 'viewport' }).down({ button: 0 }).up({ button: 0 }).perform();
  }
  if (!opens) { await browser.pause(100); return; }
  await browser.waitUntil(async () => chooser ? await $('.chooser-title').isExisting() : (await active()) === 'report.html');
}

async function preview(): Promise<void> {
  await clickLink();
  await $(`${previewRoot} .fv-scripts`).waitForDisplayed();
}

async function closeTerminal(title: string, id: string, remaining: string[]): Promise<void> {
  await close(title);
  await $('.action-dialog [data-confirm="accept"]').click();
  await browser.waitUntil(async () => (await calls('tab_close')).some((call: UiTestInvocation) => call[1]?.win === id));
  await windowEvent('close', id, { order: remaining });
}

describe('Tauri UI: preview actions and neighbouring tabs', () => {
  beforeEach(async () => { await js(`document.querySelectorAll('.action-dialog').forEach(el => el.close())`); await browser.setWindowSize(1100, 700); await fixture(); });

  it('inserts a linked preview immediately right of its source in custom display order', async () => {
    await fixture(['@2', '@1', '@0']);
    await preview();
    assert.deepEqual(await order(), ['Last', 'Source', 'report.html', 'First']);
    assert.equal((await calls('tab_new')).length, 0);
    await close('report.html');
    assert.equal(await active(), 'Source');
    assert.equal((await calls('tab_select')).at(-1)[1].win, '@1');
    assert.equal((await calls('tab_close')).length, 0, 'preview close stays local');
  });

  it('also inserts a detected plain file path beside its source', async () => {
    await fire('data', { id: 's1', window: '@1', data: '/tmp/report.html\r\n' });
    await browser.waitUntil(async () => !!await js(`window.__testFindText('/tmp/report.html')`));
    await clickLink(false, '/tmp/report.html');
    assert.deepEqual(await order(), ['First', 'Source', 'report.html', 'Last']);
  });

  it('rejects unsupported OSC 8 schemes even with the chooser modifier', async () => {
    await fire('data', { id: 's1', window: '@1', data: '\x1b]8;;javascript:alert(1)\x07UNSAFE_LINK\x1b]8;;\x07\r\n' });
    await browser.waitUntil(async () => !!await js(`window.__testFindText('UNSAFE_LINK')`));
    await clickLink(true, 'UNSAFE_LINK', false);
    assert.equal(await active(), 'Source');
    assert.equal(await $('.action-dialog').isExisting(), false);
    assert.equal((await calls('open_external')).length, 0);
    assert.equal((await calls('read_remote_file')).length, 0);
  });

  it('keeps the original link source when the chooser is confirmed after a backend tab switch', async () => {
    await clickLink(true);
    await windowEvent('active', '@2');
    await $('.chooser-item[aria-label="Preview in app"]').click();
    assert.deepEqual(await order(), ['First', 'Source', 'report.html', 'Last']);
  });

  it('opens another linked preview beside the source and preserves focus on background close', async () => {
    await preview();
    await select('Source'); await preview();
    assert.deepEqual(await order(), ['First', 'Source', 'report.html', 'report.html', 'Last']);
    await select('Last'); await close('report.html');
    assert.equal(await active(), 'Last');
    assert.deepEqual(await order(), ['First', 'Source', 'report.html', 'Last']);
  });

  it('retains preview positions after dragging and persists only terminal ids', async () => {
    await preview();
    // Exercise production pointer handlers with actual strip geometry; the native driver emits
    // MouseEvents rather than PointerEvents (the full gui-reorder suite covers native drags).
    await js(`(() => {
      const tabs = document.querySelectorAll('#tabs .tab:not(.plus)');
      const source = tabs[2];
      const from = source.getBoundingClientRect(), to = tabs[1].getBoundingClientRect();
      const options = { bubbles: true, cancelable: true, button: 0, pointerId: 42,
        clientX: from.left + from.width / 2, clientY: from.top + from.height / 2 };
      source.dispatchEvent(new PointerEvent('pointerdown', options));
      options.clientX = to.left + to.width / 2 - 8;
      window.dispatchEvent(new PointerEvent('pointermove', options));
      window.dispatchEvent(new PointerEvent('pointerup', options));
    })()`);
    assert.deepEqual(await order(), ['First', 'report.html', 'Source', 'Last']);
    assert.deepEqual((await calls('set_tab_prefs')).at(-1)[1].tabOrder, ['@0', '@1', '@2']);
    await select('Source');
    await closeTerminal('Source', '@1', ['@0', '@2']);
    assert.equal(await active(), 'report.html', 'close uses the dragged display order');
  });

  it('selects the left terminal after close and resists tmux choosing the first tab', async () => {
    await select('Last');
    await closeTerminal('Last', '@2', ['@0', '@1']);
    assert.equal(await active(), 'Source');
    await windowEvent('active', '@0', { afterClose: true, order: ['@0', '@1'] });
    assert.equal(await active(), 'Source');
    assert.equal((await calls('tab_select')).at(-1)[1].win, '@1');
    await windowEvent('active', '@0');
    assert.equal(await active(), 'First', 'ordinary external tmux switches still work');
  });

  it('keeps a preview selected when it is the closed terminal’s left neighbour', async () => {
    await preview(); await select('Last');
    await closeTerminal('Last', '@2', ['@0', '@1']);
    assert.equal(await active(), 'report.html');
    await windowEvent('active', '@0', { afterClose: true });
    assert.equal(await active(), 'report.html');
    assert.ok((await calls('tab_select')).every((call: UiTestInvocation) => String(call[1]?.win).startsWith('@')));
  });

  it('chooses the right neighbour for the first tab and retains selection on background close', async () => {
    await select('First');
    await closeTerminal('First', '@0', ['@1', '@2']);
    assert.equal(await active(), 'Source');
    await windowEvent('active', '@2', { afterClose: true });
    assert.equal(await active(), 'Source');
    await closeTerminal('Last', '@2', ['@1']);
    assert.equal(await active(), 'Source');
  });

  it('shows readable JavaScript actions, requires confirmation, and keeps opt-in per preview', async () => {
    await preview();
    assert.deepEqual(await js(`Array.from(document.querySelectorAll('.fv-bar button'), el => el.textContent)`), ['Copy path', 'Enable JavaScript', 'Download']);
    assert.equal(await js(`document.querySelectorAll('.fv-bar button svg').length`), 0);
    assert.equal(await $(`${previewRoot} .fv-html`).getAttribute('sandbox'), '');
    await screenshotIfRequested('preview-toolbar.png');
    await $(`${previewRoot} .fv-scripts`).click();
    assert.deepEqual(await js(`Array.from(document.querySelectorAll('.action-dialog .dialog-footer button'), el => el.textContent)`), ['Cancel', 'Enable JavaScript']);
    assert.equal(await js(`document.querySelectorAll('.action-dialog .dialog-footer svg').length`), 0);
    await screenshotIfRequested('enable-javascript.png');
    await $('.action-dialog .dialog-footer button:not([data-confirm])').click();
    assert.equal((await calls('enable_html_scripts')).length, 0);
    assert.equal(await $(`${previewRoot} .fv-scripts`).isEnabled(), true);
    await $(`${previewRoot} .fv-scripts`).click(); await $('.action-dialog [data-confirm="accept"]').click();
    await $(`${previewRoot} .fv-script-status`).waitForDisplayed();
    assert.equal(await $(`${previewRoot} .fv-script-status`).getText(), 'JavaScript enabled');
    assert.equal(await $(`${previewRoot} .fv-html`).getAttribute('sandbox'), 'allow-scripts');
    await close('report.html'); await preview();
    assert.equal(await $(`${previewRoot} .fv-html`).getAttribute('sandbox'), '');
    assert.equal(await $(`${previewRoot} .fv-scripts`).isEnabled(), true);
  });

  it('keeps the enabling label visible while pending and lets a failed opt-in retry', async () => {
    await fixture(undefined, { delay: { enable_html_scripts: 500 }, reject: { enable_html_scripts: 'Preview unavailable' } });
    await preview(); await $(`${previewRoot} .fv-scripts`).click(); await $('.action-dialog [data-confirm="accept"]').click();
    assert.equal(await $(`${previewRoot} .fv-scripts`).getText(), 'Enabling JavaScript…');
    assert.equal(await $(`${previewRoot} .fv-scripts`).isEnabled(), false);
    await browser.waitUntil(async () => await $(`${previewRoot} .fv-scripts`).isEnabled());
    assert.equal(await $(`${previewRoot} .fv-scripts`).getText(), 'Enable JavaScript');
    assert.equal(await $(`${previewRoot} .fv-html`).getAttribute('sandbox'), '');
    await js(`delete window.__BUOY_UI_TEST__.fixture.backend.reject.enable_html_scripts`);
    await $(`${previewRoot} .fv-scripts`).click(); await $('.action-dialog [data-confirm="accept"]').click();
    await $(`${previewRoot} .fv-script-status`).waitForDisplayed();
  });

  it('can close a preview while its file request is still pending', async () => {
    await fixture(undefined, { delay: { read_remote_file: 500 }, reject: { read_remote_file: 'File disappeared' } });
    await clickLink(); await close('report.html');
    await browser.pause(600);
    assert.equal(await active(), 'Source');
    assert.deepEqual(await js('window.__errs'), []);
  });
});
