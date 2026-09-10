import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, screenshotIfRequested, session } from './tauri-ui-harness.js';

const input = '.terminal-find-input';
const count = '.terminal-find-count';
const next = '.terminal-find [aria-label="Next match · Enter"]';
const previous = '.terminal-find [aria-label="Previous match · Shift+Enter"]';
const close = '.terminal-find [aria-label="Close search · Escape"]';
const state = () => js('window.__testTerminalState()');
const waitCount = (value: string) => browser.waitUntil(async () => await $(count).getText() === value);
const terminalKeys = (key: string, modifiers = {}) => js(`(() => {
  const el = [...document.querySelectorAll('.xterm-helper-textarea')].find(el => el.getBoundingClientRect().height > 0);
  el.focus(); el.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(key)}, bubbles: true, cancelable: true, ...${JSON.stringify(modifiers)} }));
})()`);
const write = (data: string, window = '@0', id = 's1') => fire('data', { id, window, data });
async function query(text: string): Promise<void> {
  await $(input).setValue(text);
  await browser.pause(120);
}
async function selectTab(title: string): Promise<void> {
  await js(`([...document.querySelectorAll('#tabs .tlabel')].find(el => el.textContent === ${JSON.stringify(title)})).click()`);
  await browser.pause(100);
}
async function fixture(): Promise<void> {
  await loadFixture([session(1, 'Search workspace'), session(2, 'Other workspace')]);
  for (const [index, name] of ['shell', 'logs'].entries()) {
    await fire('window', { id: 's1', action: 'add', window: '@' + index, name, order: ['@0', '@1'] });
    await fire('window', { id: 's1', action: 'rename', window: '@' + index, name });
  }
  await fire('window', { id: 's1', action: 'active', window: '@0', order: ['@0', '@1'] });
  await fire('state', { id: 's1', state: 'connected' });
  await fire('ready', { id: 's1' });
  await write('alpha first\r\nbeta\r\nalpha second\r\nalpha third\r\n');
  await browser.waitUntil(async () => (await js('window.__testReadBuffer()')).includes('alpha third'));
}

describe('Tauri UI: search within a terminal tab', () => {
  beforeEach(async () => { await browser.setWindowSize(1400, 900); await fixture(); });
  afterEach(async () => { assert.deepEqual(await js('window.__errs'), []); });

  it('opens from the active tab, highlights matches, and wraps in both directions', async () => {
    assert.equal(await $$('#tabs .tsearch').length, 1);
    await $('#tabs .tsearch').click();
    await query('alpha'); await waitCount('1 / 3');
    assert.equal((await state()).selection, 'alpha');
    assert.ok(await $$('.xterm-find-result-decoration').length > 0);
    await $(next).click(); await waitCount('2 / 3');
    await $(next).click(); await waitCount('3 / 3');
    await $(next).click(); await waitCount('1 / 3');
    await $(previous).click(); await waitCount('3 / 3');
    await screenshotIfRequested('terminal-search.png', 'BUOY_SEARCH_SCREENSHOTS');
    await $(close).click();
    assert.equal(await $(input).isExisting(), false);
    assert.equal((await state()).selection, '');
    assert.equal(await $$('.xterm-find-result-decoration').length, 0);
    assert.equal(await js(`document.activeElement.classList.contains('xterm-helper-textarea')`), true);
  });

  it('captures find and navigation shortcuts without sending search input to the shell', async () => {
    const before = await js('window.__inputs.length');
    await terminalKeys('f', { metaKey: true });
    assert.equal(await $(input).isDisplayed(), true);
    await query('alpha'); await waitCount('1 / 3');
    await browser.keys('Enter'); await waitCount('2 / 3');
    // The embedded driver's modifier keyDown does not set shiftKey on the subsequent Enter.
    // Dispatch that combined event in the real webview; unmodified keys still use native input.
    await js(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key:'Enter', shiftKey:true, bubbles:true, cancelable:true }))`);
    await waitCount('1 / 3');
    await browser.keys('F3'); await waitCount('2 / 3');
    await browser.keys('Escape');
    assert.equal(await $(input).isExisting(), false);
    assert.equal(await js('window.__inputs.length'), before);
    await terminalKeys('F', { ctrlKey: true, shiftKey: true });
    assert.equal(await $(input).getValue(), 'alpha');
  });

  it('keeps queries and options separate for each tab and session', async () => {
    await $('#tabs .tsearch').click(); await query('alpha'); await waitCount('1 / 3');
    await $('.terminal-find [aria-label="Match case"]').click();
    await selectTab('logs');
    assert.equal(await $(input).isExisting(), false);
    await write('beta logs\r\n', '@1');
    await $('#tabs .tsearch').click();
    assert.equal(await $(input).getValue(), '');
    await query('beta'); await waitCount('1 / 1');
    await selectTab('shell');
    assert.equal(await $(input).getValue(), 'alpha');
    assert.equal(await $('.terminal-find [aria-label="Match case"]').getAttribute('aria-pressed'), 'true');
    await waitCount('1 / 3');
    await $('.session[data-id="s2"] .name').click();
    assert.equal(await $(input).isExisting(), false);
    await $('.session[data-id="s1"] .name').click();
    assert.equal(await $(input).getValue(), 'alpha');
  });

  it('supports case and whole-word filters, literal punctuation, and no-match recovery', async () => {
    await write('\r\nALPHA alphabet [a-z] literal\r\n');
    await $('#tabs .tsearch').click(); await query('alpha'); await waitCount('1 / 5');
    await $('.terminal-find [aria-label="Match case"]').click(); await waitCount('1 / 4');
    await $('.terminal-find [aria-label="Match whole word"]').click(); await waitCount('1 / 3');
    await query('[a-z]'); await waitCount('1 / 1');
    assert.equal((await state()).selection, '[a-z]');
    await query('not-present'); await waitCount('0 / 0');
    assert.equal(await $(next).isEnabled(), false);
    assert.equal((await state()).selection, '');
    await query('beta'); await waitCount('1 / 1');
    await query('');
    assert.equal(await $(count).getText(), '');
    assert.equal(await $$('.xterm-find-result-decoration').length, 0);
  });

  it('finds wrapped Unicode text and scrollback outside the viewport', async () => {
    const cols = (await state()).cols;
    const wrapped = '跨行🙂café終点';
    await write('\r\n' + 'x'.repeat(cols - 3) + wrapped + '\r\n');
    await write(Array.from({ length: 90 }, (_, i) => `output ${i}\r\n`).join(''));
    await browser.waitUntil(async () => (await state()).baseY > 20);
    await $('#tabs .tsearch').click(); await query(wrapped); await waitCount('1 / 1');
    assert.equal((await state()).selection, wrapped);
    assert.ok((await state()).viewportY < (await state()).baseY);
  });

  it('updates matches for new output, preserves focus on reconnect, and clears safely on close', async () => {
    await $('#tabs .tsearch').click(); await query('alpha'); await waitCount('1 / 3');
    await write('alpha fresh\r\n');
    await browser.waitUntil(async () => (await $(count).getText()).endsWith('/ 4'));
    await fire('state', { id: 's1', state: 'reconnecting' });
    assert.equal(await $(input).isDisplayed(), true);
    await fire('state', { id: 's1', state: 'connected' }); await fire('ready', { id: 's1' });
    assert.equal(await $(input).getValue(), 'alpha');
    assert.equal(await js(`document.activeElement.matches('${input}')`), true);
    await write('alpha pending\r\n');
    await fire('window', { id: 's1', action: 'close', window: '@0', order: ['@1'] });
    await browser.pause(300);
    assert.equal(await $(input).isExisting(), false);
    await $('#tabs .tsearch').click();
    assert.equal(await $(input).getValue(), '');
  });

  it('hides search in History and dialogs do not steal find shortcuts', async () => {
    await $('#tabs .tsearch').click(); await query('alpha');
    await $('#show-history').click();
    assert.equal(await $(input).isExisting(), false);
    await $('#history-back').click();
    assert.equal(await $(input).getValue(), 'alpha');
    await $('#new').click();
    await js(`(() => { document.querySelector('#f-host').focus(); document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key:'f', metaKey:true, bubbles:true, cancelable:true })); })()`);
    assert.equal(await js(`document.activeElement.id`), 'f-host');
    await browser.keys('Escape');
  });

  it('works in a plain terminal and follows theme changes', async () => {
    await loadFixture([session(1, 'Plain terminal', 'plain')]);
    await write('plain search\r\n');
    await $('#find-terminal').click(); await query('plain'); await waitCount('1 / 1');
    for (const theme of ['light', 'dark']) {
      await $('#show-theme').click(); await $(`[data-theme-choice="${theme}"]`).click();
      await waitCount('1 / 1');
      assert.equal((await state()).selection, 'plain');
    }
  });

  it('searches an alternate-screen application without mixing in normal scrollback', async () => {
    await write('\u001b[?1049h\u001b[2J\u001b[Halternate needle\r\nneedle second');
    await $('#tabs .tsearch').click(); await query('needle');
    await waitCount('1 / 2');
    assert.equal((await state()).selection, 'needle');
    await $(next).click(); await waitCount('2 / 2');
    await query('alpha'); await waitCount('0 / 0');
    await write('\u001b[?1049l');
    await browser.waitUntil(async () => (await $(count).getText()).endsWith('/ 3'));
  });

  it('keeps search usable after reflow and sidebar resizing', async () => {
    await $('#tabs .tsearch').click(); await query('alpha'); await waitCount('1 / 3');
    await browser.setWindowSize(850, 700);
    await $('#collapse-sidebar').click();
    await browser.pause(350);
    assert.equal(await $(input).getValue(), 'alpha');
    assert.equal((await state()).selection, 'alpha');
    await $(next).click();
    assert.equal((await state()).selection, 'alpha');
    const bounds = await js(`({ width: innerWidth, rect: document.querySelector('.terminal-find').getBoundingClientRect().toJSON() })`);
    assert.ok(bounds.rect.left >= 0 && bounds.rect.right <= bounds.width);
  });

  it('bounds highlighting for large scrollback and cancels stale queries on close', async () => {
    await write('bulk-match\r\n'.repeat(1200));
    await browser.waitUntil(async () => (await state()).baseY > 1000);
    await $('#tabs .tsearch').click(); await query('bulk-match'); await waitCount('1000+');
    assert.equal((await state()).selection, 'bulk-match');
    await $(previous).click();
    assert.equal((await state()).selection, 'bulk-match');
    await js(`(() => { const input = document.querySelector('${input}'); input.value = 'queued-change'; input.dispatchEvent(new Event('input', { bubbles:true })); document.querySelector('${close}').click(); })()`);
    await browser.pause(350);
    assert.equal(await $(input).isExisting(), false);
    assert.equal(await $$('.xterm-find-result-decoration').length, 0);
  });
});
