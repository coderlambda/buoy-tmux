import type { LinkMatch, PluginRegistry } from './plugins.js';

interface TerminalLink extends LinkMatch { range: XtermLink['range'] }

// Match a complete logical line, including soft wraps, then map string offsets back to xterm's
// 1-based, inclusive cell coordinates. UTF-16 offsets alone are not terminal columns (CJK/emoji).
export function findTerminalLinks(
  buffer: Pick<XtermBuffer, 'getLine'>,
  lineNumber: number,
  registry: PluginRegistry,
): TerminalLink[] {
  let first = lineNumber - 1;
  if (first < 0 || !buffer.getLine(first)) return [];
  while (first > 0 && buffer.getLine(first)?.isWrapped) first--;

  const rows: Array<{ line: XtermBufferLine; y: number; offset: number; length: number }> = [];
  let text = '';
  for (let y = first; ; y++) {
    const line = buffer.getLine(y);
    if (!line) break;
    const wraps = !!buffer.getLine(y + 1)?.isWrapped;
    const content = line.translateToString(!wraps);
    rows.push({ line, y: y + 1, offset: text.length, length: content.length });
    text += content;
    if (!wraps) break;
  }

  const position = (index: number, end: boolean): { x: number; y: number } | null => {
    const row = rows.find(row => index >= row.offset && index < row.offset + row.length);
    if (!row) return null;
    let offset = row.offset;
    for (let x = 0; ; x++) {
      const cell = row.line.getCell(x);
      if (!cell) return null;
      const width = cell.getWidth();
      if (!width) continue;
      const length = cell.getChars().length || 1;
      if (index < offset + length) return { x: x + (end ? width : 1), y: row.y };
      offset += length;
    }
  };
  const links: TerminalLink[] = [];
  for (const match of registry.findMatches(text)) {
    const start = position(match.start, false);
    const end = position(match.end - 1, true);
    if (start && end && start.y <= lineNumber && end.y >= lineNumber) {
      links.push({ ...match, range: { start, end } });
    }
  }
  return links;
}
