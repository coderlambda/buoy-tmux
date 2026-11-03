import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { builtinLinkPlugins } from '../apps/desktop/ui/src/builtinPlugins.js';
import { PluginRegistry } from '../apps/desktop/ui/src/plugins.js';
import { findTerminalLinks } from '../apps/desktop/ui/src/terminalLinks.js';

const registry = new PluginRegistry();
builtinLinkPlugins().forEach(plugin => registry.registerLink(plugin));

// Fixed cell fixtures model soft-wrap flags and wide-character continuation cells. The mapping
// assertions below use terminal columns, not JavaScript string offsets.
function buffer(rows: Array<[string, boolean]>): Pick<XtermBuffer, 'getLine'> {
  const lines: XtermBufferLine[] = rows.map(([text, isWrapped]) => {
    const cells: Array<{ getChars(): string; getWidth(): number; isUnderline(): boolean }> = [];
    for (const char of text) {
      const width = /[报告😀]/u.test(char) ? 2 : 1;
      cells.push({ getChars: () => char, getWidth: () => width, isUnderline: () => false });
      if (width === 2) cells.push({ getChars: () => '', getWidth: () => 0, isUnderline: () => false });
    }
    return { isWrapped, translateToString: trim => trim ? text.trimEnd() : text, getCell: x => cells[x] };
  });
  return { getLine: y => lines[y] };
}

test('soft-wrapped file and URL targets are complete from either terminal row', () => {
  for (const [first, second, target] of [
    ['See /yitong/report', '.html.', '/yitong/report.html'],
    ['See https://example.com/', 'report.html?q=a,b#part.', 'https://example.com/report.html?q=a,b#part'],
  ] as const) {
    const buf = buffer([[first, false], [second, true], ['Unrelated /other.txt.', false]]);
    const links = findTerminalLinks(buf, 1, registry);
    assert.deepEqual(links.map(link => link.text), [target]);
    assert.deepEqual(links[0]!.range, { start: { x: 5, y: 1 }, end: { x: second.length - 1, y: 2 } });
    assert.deepEqual(findTerminalLinks(buf, 2, registry), links);
    assert.deepEqual(findTerminalLinks(buf, 3, registry).map(link => link.text), ['/other.txt']);
  }
});

test('a period wrapped onto the next row is not a link or part of its target', () => {
  const buf = buffer([['See /tmp/report.html', false], ['. Done', true]]);
  const link = findTerminalLinks(buf, 1, registry)[0]!;
  assert.equal(link.text, '/tmp/report.html');
  assert.deepEqual(link.range, { start: { x: 5, y: 1 }, end: { x: 20, y: 1 } });
  assert.deepEqual(findTerminalLinks(buf, 2, registry), []);
});

test('link hit regions account for CJK and surrogate-pair display widths', () => {
  const buf = buffer([['报告 /tmp/a', false], ['.md。', true], ['😀 /tmp/a.md.', false]]);
  assert.deepEqual(findTerminalLinks(buf, 1, registry)[0]!.range,
    { start: { x: 6, y: 1 }, end: { x: 3, y: 2 } });
  assert.deepEqual(findTerminalLinks(buf, 3, registry)[0]!.range,
    { start: { x: 4, y: 3 }, end: { x: 12, y: 3 } });
});

test('hard line breaks never combine distinct targets; missing rows have no links', () => {
  const buf = buffer([['/tmp/first.html.', false], ['/tmp/second.html.', false]]);
  assert.deepEqual(findTerminalLinks(buf, 1, registry).map(link => link.text), ['/tmp/first.html']);
  assert.deepEqual(findTerminalLinks(buf, 2, registry).map(link => link.text), ['/tmp/second.html']);
  assert.deepEqual(findTerminalLinks(buf, 0, registry), []);
  assert.deepEqual(findTerminalLinks(buf, 3, registry), []);
});
