import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, session } from './tauri-ui-harness.js';

async function fixture(backend: Record<string, unknown> = {}): Promise<void> {
  await loadFixture([session(1, 'Paste test')], {}, backend);
  for (const [i, name] of ['shell', 'other'].entries()) {
    await fire('window', { id: 's1', action: 'add', window: '@' + i, name, order: ['@0', '@1'] });
    await fire('window', { id: 's1', action: 'rename', window: '@' + i, name });
  }
  await fire('window', { id: 's1', action: 'active', window: '@0', order: ['@0', '@1'] });
  await fire('state', { id: 's1', state: 'connected' });
  await fire('ready', { id: 's1' });
  await fire('data', { id: 's1', window: '@0', data: '\x1b[?2004h' });
  await browser.pause(100);
}

// Exercise xterm's real paste handler (including newline normalization and bracket markers)
// in the native webview, without replacing the user's system clipboard.
async function paste(expression: string): Promise<void> {
  await js(`(() => {
    const el = [...document.querySelectorAll('.xterm-helper-textarea')].find(el => el.closest('.xterm').getBoundingClientRect().height > 0);
    el.focus();
    const event = new Event('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: { getData: () => (${expression}) } });
    el.dispatchEvent(event);
  })()`);
}

describe('Tauri UI: large terminal paste', () => {
  afterEach(async () => { assert.deepEqual(await js('window.__errs'), []); });

  it('pastes 50,000 characters on one line without truncation or a giant native call', async () => {
    await fixture({ echoInput: true });
    await paste("'abc $HOME;'.repeat(5000)");
    await browser.waitUntil(async () => await js(`window.__inputs.reduce((n,i) => n + i[1].length, 0) >= 50012`));
    assert.equal(await js(`window.__inputs.map(i => i[1]).join('') === '\x1b[200~' + 'abc $HOME;'.repeat(5000) + '\x1b[201~'`), true);
    assert.ok(await js('window.__inputs.every(i => i[1].length <= 4096)'));
  });

  it('streams a large Unicode clipboard paste exactly while keeping controls and the event loop responsive', async () => {
    await fixture({ delay: { session_input: 1000 }, echoInput: true });
    await js(`(() => {
      window.__pasteText = (${JSON.stringify('中文🙂 $HOME \\" code\r\n')} + 'x'.repeat(150)).repeat(10000);
    })()`);
    await paste('window.__pasteText');
    assert.equal(await js("window.__invocations.filter(i => i[0] === 'session_input').length"), 1, 'only one input is pending during a slow write');
    await $('#tabs .tsearch').click();
    assert.equal(await $('.terminal-find-input').isDisplayed(), true, 'search remains usable during paste');
    await $('.terminal-find [aria-label="Close search · Escape"]').click();
    // Add later input through xterm's real onData path while the paste is still pending.
    await js("window.__testSendInput('after')");
    await js(`([...document.querySelectorAll('#tabs .tlabel')].find(el => el.textContent === 'other')).click()`);
    await js('window.__BUOY_UI_TEST__.fixture.backend.delay.session_input = 0');
    try {
      await browser.waitUntil(async () => await js(`window.__inputs.at(-1)?.[1] === 'after'`), { timeout: 20000 });
    } catch (error) {
      console.log(await js(`({ chunks: window.__inputs.length, length: window.__inputs.reduce((n,i) => n + i[1].length, 0), tail: window.__inputs.at(-1)?.[1].slice(-40), status: document.querySelector('#status-message').textContent })`));
      throw error;
    }
    const result = await js(`(() => {
      const sent = window.__inputs.map(i => i[1]);
      return { exact: sent.join('') === '\x1b[200~' + window.__pasteText.replace(/\\r?\\n/g, '\\r') + '\x1b[201~after',
        chunks: sent.length, max: Math.max(...sent.map(s => s.length)), targets: [...new Set(window.__inputs.map(i => i[2]))] };
    })()`);
    assert.equal(result.exact, true, 'no truncation, reordering, or broken bracket markers');
    assert.ok(result.chunks > 100); assert.ok(result.max <= 4096);
    assert.deepEqual(result.targets, ['@0'], 'tab switching never redirects the remaining paste');
  });

  it('cancels the paste tail on disconnect and never resumes it in a reconnected shell', async () => {
    await fixture({ delay: { session_input: 40 } });
    await paste("'OLD_PASTE'.repeat(50000)");
    await browser.waitUntil(async () => await js('window.__inputs.length > 1'));
    await fire('state', { id: 's1', state: 'reconnecting' });
    await browser.pause(150); // the one already-in-flight chunk may finish
    const stopped = await js('window.__inputs.length');
    assert.ok(stopped < 20, 'unsent paste is discarded immediately');
    await fire('state', { id: 's1', state: 'connected' }); await fire('ready', { id: 's1' });
    await browser.pause(150);
    assert.equal(await js('window.__inputs.length'), stopped);
    await paste("'NEW_INPUT'");
    await browser.waitUntil(async () => await js('window.__inputs.length > ' + stopped));
    assert.equal(await js('window.__inputs.at(-1)[1]'), '\x1b[200~NEW_INPUT\x1b[201~');
  });

  it('closing a terminal disposes queued paste input', async () => {
    await fixture({ delay: { session_input: 40 } });
    await paste("'dispose'.repeat(50000)");
    await browser.waitUntil(async () => await js('window.__inputs.length > 1'));
    await fire('window', { id: 's1', action: 'close', window: '@0', order: ['@1'] });
    await browser.pause(150);
    const stopped = await js('window.__inputs.length');
    await browser.pause(150);
    assert.equal(await js('window.__inputs.length'), stopped);
  });

  it('reports native write failure without flooding retries or throwing an unhandled rejection', async () => {
    await fixture({ reject: { session_input: 'PTY closed' } });
    await paste("'failure'.repeat(50000)");
    await browser.waitUntil(async () => await js("document.querySelector('.toast')?.textContent.includes('Input failed: PTY closed')"));
    const calls = await js("window.__invocations.filter(i => i[0] === 'session_input').length");
    await browser.pause(100);
    assert.equal(await js("window.__invocations.filter(i => i[0] === 'session_input').length"), calls);
    assert.equal(calls, 1);
  });
});
