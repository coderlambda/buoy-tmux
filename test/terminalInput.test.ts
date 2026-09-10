import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { createTerminalInput, INPUT_CHUNK_UNITS, trackCommandText } from '../ui/src/terminalInput.js';

const tick = () => new Promise(resolve => setTimeout(resolve, 5));
async function until(predicate: () => boolean): Promise<void> {
  for (let i = 0; i < 1000 && !predicate(); i++) await tick();
  assert.ok(predicate(), 'input queue completed');
}

test('large Unicode paste streams exact ordered chunks with one native write in flight', async () => {
  const original = '\x1b[200~' + ('a'.repeat(4088) + '🙂\r\n中文$HOME').repeat(80) + '\x1b[201~';
  const sent: string[] = [], errors: unknown[] = [];
  let release: (() => void) | undefined;
  let active = 0, maximum = 0, timerRan = false;
  const input = createTerminalInput(async data => {
    sent.push(data); maximum = Math.max(maximum, ++active);
    if (sent.length === 1) await new Promise<void>(resolve => { release = resolve; });
    active--;
  }, () => true, error => errors.push(error));
  input.push(original); input.push('after');
  await tick();
  assert.equal(sent.length, 1, 'backpressure must stop the next IPC call');
  setTimeout(() => { timerRan = true; }, 0);
  release!();
  await until(() => sent.at(-1) === 'after');
  assert.equal(sent.join(''), original + 'after');
  assert.equal(maximum, 1);
  assert.ok(timerRan, 'paste yields to timers and rendering');
  assert.ok(sent.every(s => s.length <= INPUT_CHUNK_UNITS));
  assert.ok(sent.every(s => !/[\uD800-\uDBFF]$/.test(s) && !/^[\uDC00-\uDFFF]/.test(s)));
  assert.deepEqual(errors, []);
});

test('disconnect cancellation never replays a paste tail after reconnect', async () => {
  const sent: string[] = [];
  let release!: () => void;
  const input = createTerminalInput(data => {
    sent.push(data);
    if (sent.length === 1) return new Promise<void>(resolve => { release = resolve; });
  }, () => true, error => assert.fail(String(error)));
  input.push('x'.repeat(20000));
  input.cancel();
  input.push('new connection');
  release(); await tick();
  assert.deepEqual(sent, ['x'.repeat(4096), 'new connection']);
  input.dispose(); input.push('disposed'); await tick();
  assert.equal(sent.length, 2);
});

test('write failures stop queued input, report once, and allow later explicit input', async () => {
  const sent: string[] = [], errors: unknown[] = [];
  const input = createTerminalInput(async data => {
    sent.push(data);
    if (sent.length === 1) throw new Error('broken pipe');
  }, () => true, error => errors.push(error));
  input.push('x'.repeat(20000)); input.push('queued');
  await tick();
  assert.equal(sent.length, 1); assert.equal(errors.length, 1);
  input.push('new'); await tick(); assert.equal(sent.at(-1), 'new');
});

test('a disconnected terminal drops unsent input and a cancelled error stays silent', async () => {
  let connected = false, reject!: (error: Error) => void;
  const sent: string[] = [], errors: unknown[] = [];
  const input = createTerminalInput(data => {
    sent.push(data);
    return new Promise<void>((_, fail) => { reject = fail; });
  }, () => connected, error => errors.push(error));
  input.push('offline'); assert.equal(sent.length, 0);
  connected = true; input.push('online'); input.cancel();
  reject(new Error('closed')); await tick();
  assert.deepEqual(errors, []);
});

test('recovery command tracking handles large text runs, controls, and Unicode deletion', () => {
  const state: { commandDraft?: string; lastCommand?: string } = {};
  trackCommandText(state, 'x'.repeat(2_000_000));
  assert.equal(state.commandDraft, 'x'.repeat(4096));
  trackCommandText(state, '\x03echo 中文🙂\x7f\r');
  assert.equal(state.lastCommand, 'echo 中文');
  assert.equal(state.commandDraft, '');
  trackCommandText(state, '\x1b[200~first\rsecond\x1b[201~');
  assert.equal(state.lastCommand, 'first'); assert.equal(state.commandDraft, 'second');
  trackCommandText(state, '\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\\x15');
  assert.equal(state.commandDraft, '');
});
