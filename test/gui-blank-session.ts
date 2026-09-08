import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, session } from './tauri-ui-harness.js';

// Inspect actual text-layer pixels, independently of xterm's buffer and cursor. A moving cursor
// alone is not proof that an idle shell prompt was painted (issue #13).
async function paintedPixels(): Promise<number> {
  return js(`(() => {
    const canvas = document.querySelector('#term .xterm-text-layer');
    const state = window.__testTerminalState();
    const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, Math.ceil(canvas.height/state.rows)).data;
    const hex = state.theme.background.slice(1);
    const bg = [0,2,4].map(offset => parseInt(hex.slice(offset,offset+2),16));
    let ink = 0;
    for(let i=0;i<pixels.length;i+=4) {
      if(pixels[i+3] && Math.max(...bg.map((value,c) => Math.abs(pixels[i+c]-value))) > 40) ink++;
    }
    return ink;
  })()`);
}

describe('Tauri UI: blank session regression', () => {
  before(async () => {
    await browser.setWindowSize(1100, 700);
    await loadFixture([session(1, 'Empty shell')], {}, { echoInput: true });
  });

  for (const [index, phase] of ['new session', 'reconnect without a prior command', 'server restart'].entries()) {
    it(`paints the first prompt after ${phase}, without extra input or resize`, async () => {
      const win = index === 2 ? '@1' : '@0';
      await fire('state', { id: 's1', state: 'reconnecting' });
      if (index !== 1) await fire('window', { id: 's1', action: 'add', window: win, order: [win] });
      await fire('window', { id: 's1', action: 'active', window: win, order: [win] });
      if (index === 2) await fire('window', { id: 's1', action: 'close', window: '@0', order: [win] });
      await browser.pause(50);
      // A capture taken before the shell prints anything has an empty screen AND a zero cursor.
      const empty = '\x1b[H\x1b[2J' + '\r\n'.repeat((await js('window.__testTerminalState()')).rows - 1) + '\x1b[1;1H';
      await fire('data', { id: 's1', window: win, data: empty, repaint: true });
      await fire('state', { id: 's1', state: 'connected' });
      await fire('ready', { id: 's1' });
      const prompt = 'dev@host ~/work > ';
      await fire('data', { id: 's1', window: win, data: prompt });
      await browser.pause(150); // no further writes to accidentally hide an idle-render failure
      const state = await js('window.__testTerminalState()');
      assert.equal(state.line, prompt);
      assert.equal(state.cursorY, 0);
      assert.equal(state.cursorX, prompt.length);
      assert.ok(await paintedPixels() > 50, 'the text canvas contains visible prompt glyphs');
      assert.equal(await js(`document.querySelector('#term').classList.contains('gated')`), false);
      assert.deepEqual(await js('window.__errs'), []);
    });
  }
});
