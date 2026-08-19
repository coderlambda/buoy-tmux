import { iconButton } from './uiControls.js';
import { getTheme, onThemeChange } from './theme.js';

const HIGHLIGHT_LIMIT = 1000;

export function isTerminalFindShortcut(event: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'isComposing'>, mac: boolean): boolean {
  if (event.isComposing || event.altKey || event.key.toLowerCase() !== 'f') return false;
  // Preserve macOS Ctrl+F (shell cursor movement); Ctrl+Shift+F also works on every platform.
  return (event.metaKey && !event.ctrlKey) || (event.ctrlKey && !event.metaKey && (!mac || event.shiftKey));
}

/** One query, set of options, and search addon per terminal. Hidden tabs do no search work. */
export function createTerminalSearch(term: XtermTerminal, host: HTMLElement, focusTerminal: () => void) {
  let addon: SearchAddon.SearchAddon | undefined;
  let opened = false, active = false, disposed = false;
  let caseSensitive = false, wholeWord = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const panel = document.createElement('div');
  panel.className = 'terminal-find';
  panel.setAttribute('role', 'search');
  panel.setAttribute('aria-label', 'Find in terminal');
  const input = document.createElement('input');
  input.type = 'text'; input.className = 'terminal-find-input';
  input.placeholder = 'Find in terminal'; input.setAttribute('aria-label', 'Find in terminal');
  input.autocomplete = 'off'; input.spellcheck = false;
  input.setAttribute('autocapitalize', 'off');
  const count = document.createElement('span');
  count.className = 'terminal-find-count'; count.setAttribute('role', 'status');
  count.setAttribute('aria-live', 'polite'); count.setAttribute('aria-atomic', 'true');
  const matchCase = iconButton('case', 'Match case', () => {
    caseSensitive = !caseSensitive; matchCase.setAttribute('aria-pressed', String(caseSensitive));
    run(true); input.focus();
  });
  matchCase.setAttribute('aria-pressed', 'false');
  const matchWord = iconButton('word', 'Match whole word', () => {
    wholeWord = !wholeWord; matchWord.setAttribute('aria-pressed', String(wholeWord));
    run(true); input.focus();
  });
  matchWord.setAttribute('aria-pressed', 'false');
  const previous = iconButton('up', 'Previous match · Shift+Enter', () => { run(false, true); input.focus(); });
  const next = iconButton('down', 'Next match · Enter', () => { run(); input.focus(); });
  const closeButton = iconButton('x', 'Close search · Escape', close);
  const actions = document.createElement('div'); actions.className = 'terminal-find-actions';
  actions.append(matchCase, matchWord, previous, next, closeButton);
  panel.append(input, count, actions);

  function results(index: number, total: number): void {
    count.textContent = !input.value ? '' : total >= HIGHLIGHT_LIMIT ? `${HIGHLIGHT_LIMIT}+` : `${index + 1} / ${total}`;
    const label = !input.value ? 'Type to search' : total === 0 ? 'No matches'
      : total >= HIGHLIGHT_LIMIT ? `At least ${HIGHLIGHT_LIMIT} matches` : `Match ${index + 1} of ${total}`;
    count.title = label; count.setAttribute('aria-label', label);
    panel.classList.toggle('no-matches', !!input.value && total === 0);
    previous.disabled = next.disabled = !input.value || total === 0;
  }

  function engine(): SearchAddon.SearchAddon {
    if (!addon) {
      addon = new SearchAddon.SearchAddon({ highlightLimit: HIGHLIGHT_LIMIT });
      term.loadAddon(addon);
      addon.onDidChangeResults(({ resultIndex, resultCount }) => {
        if (opened && active && !disposed) results(resultIndex, resultCount);
      });
    }
    return addon;
  }

  function run(incremental = false, reverse = false): void {
    clearTimeout(timer);
    if (!opened || !active || disposed) return;
    if (!input.value) {
      addon?.clearDecorations(); term.clearSelection(); results(-1, 0); return;
    }
    const light = getTheme() === 'light';
    const options: XtermSearchOptions = {
      caseSensitive, wholeWord, incremental,
      decorations: {
        matchBackground: light ? '#dce7fd' : '#2b3854', matchBorder: light ? '#1e66f5' : '#89b4fa',
        matchOverviewRuler: light ? '#1e66f5' : '#89b4fa',
        activeMatchBackground: light ? '#bccdf3' : '#45475a', activeMatchBorder: light ? '#8a5d15' : '#f9e2af',
        activeMatchColorOverviewRuler: light ? '#8a5d15' : '#f9e2af',
      },
    };
    // SearchAddon 0.15 advances on a repeated term even with incremental=true. Clearing its
    // cached decorations keeps the current selection's start as the anchor when editing options.
    if (incremental) addon?.clearDecorations();
    if (reverse) engine().findPrevious(input.value, options);
    else engine().findNext(input.value, options);
  }

  function clear(): void {
    clearTimeout(timer);
    addon?.clearDecorations();
    if (addon) term.clearSelection();
  }
  function close(): void {
    opened = false; clear(); panel.remove();
    if (active && !disposed) focusTerminal();
  }
  input.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(() => run(true), 80);
  });
  panel.addEventListener('keydown', event => {
    if (event.isComposing) return;
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
    else if (event.key === 'Enter' && event.target === input) {
      event.preventDefault(); event.stopPropagation(); run(false, event.shiftKey);
    }
  });
  const unsubscribeTheme = onThemeChange(() => {
    if (opened && active) { addon?.clearDecorations(); run(true); }
  });
  results(-1, 0);

  return {
    get isOpen() { return opened && active; },
    open() {
      if (!active || disposed) return;
      if (!opened) {
        const selection = term.getSelection();
        if (selection && !/[\r\n]/.test(selection)) input.value = selection;
      }
      opened = true; host.append(panel); run(true); input.focus(); input.select();
    },
    close,
    next(reverse = false) { run(false, reverse); },
    focus() { if (opened && active) input.focus(); },
    setActive(value: boolean) {
      if (active === value || disposed) return;
      active = value;
      if (active && opened) { host.append(panel); run(true); }
      else if (!active) { panel.remove(); clear(); }
    },
    dispose() {
      disposed = true; clear(); panel.remove(); unsubscribeTheme();
      // Terminal.dispose() owns the loaded addon's lifetime.
    },
  };
}
