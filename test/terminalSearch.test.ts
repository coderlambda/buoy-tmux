import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { isTerminalFindShortcut } from '../ui/src/terminalSearch.js';

const key = { key: 'f', metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false };
test('find shortcuts support each platform without consuming macOS shell Ctrl+F', () => {
  assert.equal(isTerminalFindShortcut({ ...key, metaKey: true }, true), true);
  assert.equal(isTerminalFindShortcut({ ...key, ctrlKey: true }, false), true);
  assert.equal(isTerminalFindShortcut({ ...key, ctrlKey: true }, true), false);
  for (const mac of [true, false]) {
    assert.equal(isTerminalFindShortcut({ ...key, key: 'F', ctrlKey: true, shiftKey: true }, mac), true);
    for (const event of [key, { ...key, metaKey: true, altKey: true }, { ...key, ctrlKey: true, metaKey: true }, { ...key, metaKey: true, isComposing: true }, { ...key, key: 'r', ctrlKey: true }]) {
      assert.equal(isTerminalFindShortcut(event, mac), false);
    }
  }
});
