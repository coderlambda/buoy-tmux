import {
  createElement, Plus, X, Equal, ExternalLink, PanelLeftClose, PanelLeft, History,
  RefreshCw, Keyboard, Ellipsis, Terminal, FileText, Globe, Laptop, ArrowRight,
  ArrowLeft, ArrowUp, ArrowDown, Download, Copy, Code, Info, Pencil, Unplug,
  Power, Trash2, RotateCcw, ChevronDown, ChevronUp, Search, Check, CircleAlert,
  ArrowRightLeft, Palette, LoaderCircle,
} from 'lucide';

const icons = {
  plus: Plus, x: X, equal: Equal, open: ExternalLink, collapse: PanelLeftClose,
  sidebar: PanelLeft, history: History, refresh: RefreshCw, keyboard: Keyboard,
  more: Ellipsis, terminal: Terminal, file: FileText, globe: Globe, laptop: Laptop,
  next: ArrowRight, back: ArrowLeft, up: ArrowUp, down: ArrowDown, download: Download,
  copy: Copy, code: Code, info: Info, rename: Pencil, detach: Unplug, end: Power,
  delete: Trash2, restore: RotateCcw, expand: ChevronDown, fold: ChevronUp,
  search: Search, check: Check, error: CircleAlert, ports: ArrowRightLeft,
  palette: Palette, loading: LoaderCircle,
};
export type IconName = keyof typeof icons;

export function icon(name: IconName): string {
  const svg = createElement(icons[name]);
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', 'ui-icon');
  svg.setAttribute('focusable', 'false');
  return svg.outerHTML;
}

export function labelControl(el: HTMLElement, label: string): void {
  el.title = label;
  el.setAttribute('aria-label', label);
}

export function setIcon(el: HTMLElement, name: IconName, label: string): void {
  el.innerHTML = icon(name);
  labelControl(el, label);
}

export function iconButton(name: IconName, label: string, action?: () => unknown): HTMLButtonElement {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'icon-button';
  setIcon(button, name, label);
  if (action) button.onclick = () => { void action(); };
  return button;
}

/** Native dialogs provide focus containment, Escape and focus restoration. */
export function openPanel(title: string, help = '') {
  const dialog = document.createElement('dialog');
  dialog.className = 'action-dialog';
  dialog.setAttribute('aria-label', title);
  const head = document.createElement('header');
  head.className = 'dialog-head';
  const heading = document.createElement('h2');
  heading.textContent = title;
  head.append(heading);
  if (help) head.append(iconButton('info', help));
  const close = iconButton('x', 'Close', () => dialog.close());
  close.classList.add('dialog-close');
  head.append(close);
  const content = document.createElement('div');
  content.className = 'dialog-content';
  dialog.append(head, content);
  dialog.addEventListener('click', (event) => {
    if (event.target !== dialog) return;
    const r = dialog.getBoundingClientRect();
    if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) dialog.close();
  });
  dialog.addEventListener('close', () => dialog.remove(), { once: true });
  document.body.append(dialog);
  // Callers populate the content synchronously before the first paint.
  dialog.showModal();
  return { dialog, content };
}

export function confirmAction(title: string, detail: string, action: string, danger = false, content?: HTMLElement): Promise<boolean> {
  const panel = openPanel(title);
  const description = document.createElement('p');
  description.className = 'confirm-detail';
  description.textContent = detail;
  panel.content.append(description);
  if (content) panel.content.append(content);
  const footer = document.createElement('footer');
  footer.className = 'dialog-footer';
  const cancel = iconButton('x', 'Cancel', () => panel.dialog.close());
  const accept = iconButton(danger ? 'delete' : 'restore', action, () => panel.dialog.close('accept'));
  accept.classList.add(danger ? 'danger-fill' : 'primary');
  accept.dataset.confirm = 'accept';
  footer.append(cancel, accept);
  panel.content.append(footer);
  cancel.focus();
  return new Promise(resolve => panel.dialog.addEventListener('close', () => resolve(panel.dialog.returnValue === 'accept'), { once: true }));
}

export function hydrateIcons(): void {
  document.querySelectorAll<HTMLElement>('[data-icon]').forEach(el => {
    const name = el.dataset.icon as IconName;
    if (name in icons) setIcon(el, name, el.getAttribute('aria-label') || el.title);
  });
}
