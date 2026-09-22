import { iconButton, labelControl, openPanel, textButton } from './uiControls.js';
import type { TerminalAPI, UploadReport } from './types.js';

export interface UploadTarget { id: string; win: string; label: string }
export function installFileDrop(api: TerminalAPI, target: () => UploadTarget | string) {
  const preferenceKey = 'buoy.fileDropAction';
  const attachEnabled = () => { try { return localStorage.getItem(preferenceKey) !== 'upload'; } catch (_) { return true; } };
  const options = document.getElementById('file-drop-options');
  const refreshOption = () => {
    if (options) labelControl(options, `File drops: ${attachEnabled() ? 'Upload & attach' : 'Upload only'}`);
  };
  refreshOption();
  if (options) options.onclick = () => {
    const panel = openPanel('File drops');
    const choices = document.createElement('div'); choices.className = 'drop-action-choices';
    for (const [value, label] of [['attach', 'Upload & attach'], ['upload', 'Upload only']] as const) {
      const button = textButton(label, () => {
        try { localStorage.setItem(preferenceKey, value); } catch (_) {}
        refreshOption(); panel.dialog.close();
      });
      button.dataset.dropAction = value;
      button.setAttribute('aria-pressed', String(attachEnabled() === (value === 'attach')));
      choices.append(button);
    }
    const help = document.createElement('p'); help.className = 'confirm-detail';
    help.textContent = 'Upload & attach adds successful file paths to the original terminal input. It never sends Enter. Existing names are skipped.';
    panel.content.append(choices, help);
  };
  const overlay = document.createElement('div');
  overlay.className = 'file-drop-overlay'; overlay.hidden = true;
  const heading = document.createElement('strong'); heading.textContent = 'Drop files or folders to upload';
  const destination = document.createElement('span');
  overlay.append(heading, destination); document.body.append(overlay);
  const panel = document.createElement('section'); panel.className = 'file-upload-panel'; panel.hidden = true;
  panel.setAttribute('aria-label', 'File upload');
  const title = document.createElement('strong'); title.setAttribute('role', 'status');
  const owner = document.createElement('div'); owner.className = 'upload-owner';
  const detail = document.createElement('div'); detail.className = 'upload-detail';
  const meter = document.createElement('progress'); meter.max = 1; meter.setAttribute('aria-label', 'Upload progress');
  const results = document.createElement('ul'); results.className = 'upload-results';
  const cancel = iconButton('x', 'Cancel upload', async () => {
    if (!token) return;
    cancel.disabled = true; title.textContent = 'Cancelling upload…';
    try { await api.cancelFileUpload(token); }
    catch (error) { detail.textContent = String(error); cancel.disabled = false; }
  });
  const close = iconButton('x', 'Close upload result', () => { panel.hidden = true; });
  close.hidden = true;
  const head = document.createElement('header'); head.append(title, cancel, close);
  panel.append(head, owner, detail, meter, results); document.body.append(panel);
  let token: string | null = null;
  const error = (message: string) => {
    panel.hidden = false; title.textContent = 'Could not upload'; detail.textContent = message;
    owner.textContent = ''; meter.hidden = true; cancel.hidden = true; close.hidden = false; results.replaceChildren();
  };
  const finish = (report: UploadReport) => {
    const uploaded = report.items.filter(item => item.status === 'uploaded').length;
    const skipped = report.items.filter(item => item.status === 'skipped').length;
    const failed = report.items.filter(item => item.status === 'failed').length;
    const inserted = report.items.filter(item => item.inserted).length;
    const notAdded = report.items.filter(item => item.status === 'uploaded' && !item.inserted && item.detail).length;
    title.textContent = report.cancelled ? 'Upload cancelled' : failed || notAdded ? 'Upload finished with errors' : 'Upload complete';
    detail.textContent = `${uploaded} uploaded · ${skipped} skipped · ${failed} failed`;
    if (inserted) detail.textContent += ` · ${inserted} added to terminal`;
    if (notAdded) detail.textContent += ` · ${notAdded} not added`;
    owner.textContent += ` → ${report.directory}`;
    for (const item of report.items) {
      const li = document.createElement('li'); li.className = `upload-${item.status}`;
      li.textContent = `${item.name} — ${item.inserted ? 'uploaded, added to terminal' : item.status}${item.detail ? ': ' + item.detail : ''}`; results.append(li);
    }
    for (const warning of report.warnings) { const li = document.createElement('li'); li.textContent = warning; results.append(li); }
  };
  api.onFileDrop(async event => {
    if (event.kind === 'leave') { overlay.hidden = true; return; }
    if (token) {
      overlay.hidden = true;
      if (event.kind === 'drop') detail.textContent = 'An upload is already running. Drop again when it finishes.';
      return;
    }
    const selected = target();
    if (event.kind === 'enter') {
      overlay.hidden = false;
      heading.textContent = typeof selected === 'string' ? 'Cannot upload here' : attachEnabled() ? 'Drop to upload & attach' : 'Drop files or folders to upload';
      destination.textContent = typeof selected === 'string' ? selected : `${selected.label} · Current directory`;
      return;
    }
    overlay.hidden = true;
    if (!event.token || !event.count) return;
    if (typeof selected === 'string') { error(selected); return; }
    token = event.token; const started = token;
    panel.hidden = false; results.replaceChildren(); meter.hidden = false; meter.removeAttribute('value');
    title.textContent = 'Preparing upload…'; owner.textContent = selected.label;
    detail.textContent = 'Existing files or folders with the same name will be skipped.';
    cancel.hidden = false; cancel.disabled = false; close.hidden = true;
    try {
      const report = await api.uploadDroppedFiles(selected.id, selected.win, token, attachEnabled());
      if (token === started) finish(report);
    }
    catch (failure) { if (token !== started) return; title.textContent = cancel.disabled ? 'Upload cancelled' : 'Upload failed'; detail.textContent = failure instanceof Error ? failure.message : String(failure); }
    finally {
      if (token === started) { token = null; meter.hidden = true; cancel.hidden = true; close.hidden = false; }
    }
  });
  api.onUploadProgress(progress => {
    if (progress.token !== token || cancel.disabled) return;
    title.textContent = progress.phase === 'attach' ? 'Adding to terminal…' : `Uploading ${Math.min(progress.completed + 1, progress.count)} of ${progress.count}`;
    detail.textContent = progress.item ? `${progress.item} → ${progress.directory}` : `Preparing → ${progress.directory}`;
    if (progress.total) { meter.value = Math.min(progress.sent / progress.total, 1); }
    else meter.removeAttribute('value');
  });
  // Browser default drops must never navigate away from the terminal. File paths are accepted
  // solely through Tauri's native grant; HTML drags/terminal text do not initiate a transfer.
  for (const event of ['dragover', 'drop']) document.addEventListener(event, e => e.preventDefault());
  return { reset() { if (token) void api.cancelFileUpload(token).catch(() => {}); token = null; overlay.hidden = true; panel.hidden = true; refreshOption(); } };
}
