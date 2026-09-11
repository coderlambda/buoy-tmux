// Exercise the shipped xterm and native scrollbar without typing: user input repairs xterm's
// stale scroll geometry itself and would hide this regression.
import assert from 'node:assert/strict';
import { fire, js, loadFixture, session } from './tauri-ui-harness.js';

async function select(win: string): Promise<void> {
  await fire('window', { id: 's1', action: 'active', window: win, order: ['@0', '@1'] });
  await browser.pause(80);
}

async function output(data: string, win = '@0'): Promise<void> {
  await fire('data', { id: 's1', window: win, data });
  await browser.pause(100);
}

function lines(count: number, prefix = 'history'): string {
  return Array.from({ length: count }, (_, i) => `${prefix} ${i}\r\n`).join('');
}

async function metrics() {
  return js(`(() => {
    const viewport = [...document.querySelectorAll('#term .xterm-viewport')]
      .find(element => element.offsetParent);
    const screen = viewport.parentElement.querySelector('.xterm-screen');
    const { baseY, viewportY, cols, rows, line } = window.__testTerminalState();
    return { baseY, viewportY, cols, rows, line, scrollTop: viewport.scrollTop,
      maxScroll: viewport.scrollHeight - viewport.clientHeight,
      rowHeight: screen.getBoundingClientRect().height / window.__testTerminalState().rows };
  })()`);
}

async function scrollTo(top: number | 'bottom'): Promise<void> {
  await js(`(() => {
    const viewport = [...document.querySelectorAll('#term .xterm-viewport')]
      .find(element => element.offsetParent);
    viewport.scrollTop = ${top === 'bottom' ? 'viewport.scrollHeight' : top};
  })()`);
  await browser.pause(80);
}

async function assertGeometry() {
  const state = await metrics();
  assert.ok(Math.abs(state.maxScroll - state.baseY * state.rowHeight) <= 1,
    `scrollbar reaches the final buffer row: ${JSON.stringify(state)}`);
  assert.ok(Math.abs(state.scrollTop - state.viewportY * state.rowHeight) <= 1,
    `scrollbar and rendered rows agree: ${JSON.stringify(state)}`);
  return state;
}

for (const renderer of ['canvas', 'dom']) describe(`Tauri UI: terminal scroll recovery (${renderer})`, () => {
  before(async () => {
    if (renderer === 'dom') await js(`(() => {
      window.__scrollSavedCanvas = window.CanvasAddon;
      window.CanvasAddon = { CanvasAddon: class { constructor() { throw new Error('test DOM fallback'); } } };
    })()`);
  });

  after(async () => {
    if (renderer === 'dom') await js(`(() => {
      window.CanvasAddon = window.__scrollSavedCanvas;
      delete window.__scrollSavedCanvas;
    })()`);
  });

  beforeEach(async () => {
    await browser.setWindowSize(1000, 700);
    await loadFixture([session(1, 'scroll recovery')]);
    for (const win of ['@0', '@1']) {
      await fire('window', { id: 's1', action: 'add', window: win, order: ['@0', '@1'] });
    }
    await select('@0');
    await fire('state', { id: 's1', state: 'connected' });
    await fire('ready', { id: 's1' });
    await output(lines(400) + 'initial prompt');
    assert.equal(await js('window.__testRendererKind()'), renderer);
  });

  it('can scroll all the way down after output arrives in a hidden tab', async () => {
    await select('@1');
    await output(lines(120, 'background') + 'LIVE_BOTTOM');
    await select('@0');
    const revealed = await assertGeometry();
    assert.equal(revealed.viewportY, revealed.baseY, 'live output stays at the bottom on reveal');
    // Moving away and back exercises the DOM scrollbar path, not xterm.scrollToBottom/input.
    await scrollTo(revealed.scrollTop - 10 * revealed.rowHeight);
    assert.equal((await metrics()).viewportY, revealed.baseY - 10, 'first scroll after reveal works');
    await scrollTo('bottom');
    const bottom = await assertGeometry();
    assert.equal(bottom.viewportY, bottom.baseY);
    assert.equal(bottom.line, 'LIVE_BOTTOM');
    assert.deepEqual(await js('window.__inputs'), [], 'scroll recovery sends no remote input');
  });

  it('preserves a scrollback reading position and accepts the first scroll after reveal', async () => {
    const initial = await metrics();
    await scrollTo(initial.scrollTop - 100 * initial.rowHeight);
    const reading = await metrics();
    assert.equal(reading.viewportY, initial.baseY - 100);
    await select('@1');
    await output(lines(120, 'background') + 'NEW_PROMPT');
    await select('@0');
    const revealed = await assertGeometry();
    assert.equal(revealed.viewportY, reading.viewportY, 'background output does not pull the reader down');
    await scrollTo(revealed.scrollTop + 5 * revealed.rowHeight);
    assert.equal((await metrics()).viewportY, reading.viewportY + 5, 'first scroll is not swallowed');
    await output('\r\nmore live output');
    assert.equal((await metrics()).viewportY, reading.viewportY + 5, 'ordinary output preserves scrollback');
    await scrollTo('bottom');
    const bottom = await assertGeometry();
    assert.equal(bottom.viewportY, bottom.baseY);
  });

  it('still accumulates small trackpad scrolls instead of snapping each delta back to a row', async () => {
    const initial = await metrics();
    // The wheel handler updates scrollTop in pixel units; its scroll event then rounds to a row.
    // An unconditional sync in xterm's internal row refresh would undo every one-pixel movement.
    await scrollTo(initial.scrollTop - 100 * initial.rowHeight);
    const start = await metrics();
    for (let step = 1; step <= initial.rowHeight + 2; step++) {
      await js(`(() => {
        const viewport = [...document.querySelectorAll('#term .xterm-viewport')]
          .find(element => element.offsetParent);
        viewport.scrollTop += 1;
      })()`);
      await browser.pause(20);
    }
    const moved = await metrics();
    assert.ok(moved.scrollTop >= start.scrollTop + initial.rowHeight,
      `small pixel deltas accumulate: ${JSON.stringify({ start, moved })}`);
    assert.ok(moved.viewportY > start.viewportY);
  });

  it('recovers when a tab is hidden between parsing output and the queued scroll refresh', async () => {
    const initial = await metrics();
    await scrollTo(initial.scrollTop - 100 * initial.rowHeight);
    const reading = await metrics();
    // Hide in the public write callback: xterm has scheduled its viewport frame but that frame
    // has not run yet. Keep the real parser and scrollbar; only control the tab-switch timing.
    await js(`(() => {
      const write = window.Terminal.prototype.write;
      window.Terminal.prototype.write = function(data, callback) {
        window.Terminal.prototype.write = write;
        return write.call(this, data, () => {
          callback?.();
          window.__fire('window', {id:'s1',action:'active',window:'@1',order:['@0','@1']});
        });
      };
      window.__fire('data', {id:'s1',window:'@0',data:${JSON.stringify(lines(100) + 'FRAME_RACE_BOTTOM')}});
    })()`);
    await browser.pause(100);
    await select('@0');
    const revealed = await assertGeometry();
    assert.equal(revealed.viewportY, reading.viewportY);
    await scrollTo('bottom');
    const bottom = await assertGeometry();
    assert.equal(bottom.viewportY, bottom.baseY);
    assert.equal(bottom.line, 'FRAME_RACE_BOTTOM');
  });

  it('keeps the bottom reachable at the scrollback limit after hidden output and resize', async () => {
    await select('@1');
    await browser.setWindowSize(880, 560);
    await browser.pause(250);
    await output(lines(5500, 'long-running agent') + 'CAPPED_BOTTOM');
    await select('@0');
    const revealed = await assertGeometry();
    assert.equal(revealed.baseY, 5000, 'fixture reaches the configured scrollback cap');
    await scrollTo(revealed.scrollTop - 50 * revealed.rowHeight);
    await scrollTo('bottom');
    const bottom = await assertGeometry();
    assert.equal(bottom.viewportY, 5000);
    assert.equal(bottom.line, 'CAPPED_BOTTOM');
  });

  it('restores normal scrollback after a hidden full-screen TUI switches buffers', async () => {
    const initial = await metrics();
    await output('\x1b[?1049h\x1b[H\x1b[2JFull-screen agent'
      + `\x1b[${initial.rows};1HFOOTER`);
    await select('@1');
    await output('\x1b[?1049l\r\n' + lines(60, 'agent result') + 'NORMAL_BOTTOM');
    await select('@0');
    const normal = await assertGeometry();
    assert.ok(normal.baseY > 0);
    await scrollTo(normal.scrollTop - 10 * normal.rowHeight);
    await scrollTo('bottom');
    const bottom = await assertGeometry();
    assert.equal(bottom.viewportY, bottom.baseY);
    assert.equal(bottom.line, 'NORMAL_BOTTOM');
    await select('@1');
    await output('\x1b[?1049h\x1b[H\x1b[2JALT_ACTIVE');
    await select('@0');
    const alternate = await assertGeometry();
    assert.equal(alternate.baseY, 0, 'alternate buffer has no phantom scrollback');
    assert.equal(alternate.maxScroll, 0);
  });
});
