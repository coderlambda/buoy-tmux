import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, session, screenshotIfRequested } from './tauri-ui-harness.js';
const call = (name: string) => js(`window.__invocations.filter(([name]) => name === ${JSON.stringify(name)})`);
const drop = (token = 'drop-test', count = 1) => fire('files:drop', { kind: 'drop', token, count });
const panel = '.file-upload-panel';
async function fixture(backend = {}, mode: 'control' | 'local' = 'control') {
  await loadFixture([session(1, 'Workspace', mode)], {}, backend);
  if (mode === 'control') {
    for (const [window, name] of [['@0', 'First'], ['@1', 'Target']]) {
      await fire('window', { id: 's1', action: 'add', window, order: ['@0', '@1'] });
      await fire('window', { id: 's1', action: 'rename', window, name });
    }
    await fire('window', { id: 's1', action: 'active', window: '@1' });
  }
  await fire('state', { id: 's1', state: 'connected' }); await fire('ready', { id: 's1' });
}
const completed = () => $(`${panel} [aria-label="Close upload result"]`).waitForDisplayed();

describe('Tauri UI: desktop file and folder drops', () => {
  beforeEach(async () => { await browser.setWindowSize(1100, 700); await js("localStorage.removeItem('buoy.fileDropAction')"); await fixture(); });

  it('shows the target during drag hover and dismisses on leave without uploading', async () => {
    await fire('files:drop', { kind: 'enter', count: 2 });
    assert.match(await $('.file-drop-overlay').getText(), /Workspace › Target/);
    await screenshotIfRequested('file-drop-overlay.png');
    await fire('files:drop', { kind: 'leave' });
    assert.equal(await $('.file-drop-overlay').isDisplayed(), false);
    assert.equal((await call('upload_dropped_files')).length, 0);
  });

  it('captures the tab at drop time, stays responsive, and ignores progress from other uploads', async () => {
    await fixture({ delay: { upload_dropped_files: 1200 } });
    await drop('drop-one', 2);
    await $('#tabs .tlabel[title^="First"]').click();
    await fire('files:progress', { token: 'drop-one', directory: '/work/中文 project', item: 'folder/file.txt', completed: 0, count: 2, sent: 512, total: 1024 });
    assert.deepEqual((await call('upload_dropped_files'))[0][1], { id: 's1', win: '@1', token: 'drop-one', attach: true });
    assert.match(await $(`${panel} .upload-detail`).getText(), /folder\/file.txt/);
    assert.equal(Number(await $(`${panel} progress`).getAttribute('value')), .5);
    await fire('files:progress', { token: 'stale', directory: '/wrong', item: 'wrong', completed: 0, count: 1, sent: 0, total: 1 });
    assert.doesNotMatch(await $(`${panel} .upload-detail`).getText(), /wrong/);
    assert.equal(await $('#tabs .tab.active .ttext').getText(), 'First');
    await screenshotIfRequested('file-upload-progress.png');
    await completed();
    assert.match(await $(`${panel} .upload-owner`).getText(), /Workspace › Target/);
  });

  it('lists uploaded and skipped files/folders without interpreting their names as HTML', async () => {
    await fixture({ uploadReport: { directory: '/work', cancelled: false, warnings: ['Skipped link: symlink'], items: [
      { name: 'folder', status: 'uploaded', detail: '' },
      { name: '<img src=x onerror=alert(1)>', status: 'skipped', detail: 'Already exists.' },
    ] } });
    await drop(); await completed();
    assert.match(await $(`${panel} .upload-detail`).getText(), /1 uploaded · 1 skipped/);
    assert.equal(await $(`${panel} .upload-results img`).isExisting(), false);
    assert.match(await $(`${panel} .upload-results`).getText(), /Already exists/);
    await screenshotIfRequested('file-upload-result.png');
    await $(`${panel} [aria-label="Close upload result"]`).click();
    assert.equal(await $(panel).isDisplayed(), false);
  });

  it('cancels the captured upload and rejects additional drops while it is running', async () => {
    await fixture({ delay: { upload_dropped_files: 600 }, uploadReport: { directory: '/work', cancelled: true, warnings: [], items: [] } });
    await drop('drop-cancel'); await drop('drop-extra');
    assert.equal((await call('upload_dropped_files')).length, 1);
    await $(`${panel} [aria-label="Cancel upload"]`).click();
    assert.deepEqual((await call('cancel_file_upload'))[0][1], { token: 'drop-cancel' });
    await completed();
    assert.equal(await $(`${panel} strong`).getText(), 'Upload cancelled');
    await drop('drop-next'); await completed();
    assert.equal((await call('upload_dropped_files')).length, 2);
  });

  it('shows SSH failures and permits a new drop to retry', async () => {
    await fixture({ reject: { upload_dropped_files: 'SSH authentication failed' } });
    await drop(); await completed();
    assert.match(await $(`${panel} .upload-detail`).getText(), /SSH authentication failed/);
    await js(`delete window.__BUOY_UI_TEST__.fixture.backend.reject.upload_dropped_files`);
    await drop('retry'); await completed();
    assert.equal(await $(`${panel} strong`).getText(), 'Upload complete');
  });

  it('does not upload to a disconnected terminal, a local fallback shell, or an open dialog', async () => {
    await fire('state', { id: 's1', state: 'reconnecting' }); await drop();
    assert.match(await $(`${panel} .upload-detail`).getText(), /Reconnect/);
    assert.equal((await call('upload_dropped_files')).length, 0);
    await fixture({}, 'local'); await drop();
    assert.match(await $(`${panel} .upload-detail`).getText(), /tmux terminal/);
    assert.equal((await call('upload_dropped_files')).length, 0);
    await fixture(); await $('#show-theme').click(); await drop();
    assert.equal((await call('upload_dropped_files')).length, 0);
    await $('.action-dialog .dialog-close').click();
  });

  it('switches between persisted upload-only and attachment modes', async () => {
    await $('#file-drop-options').click();
    await $('[data-drop-action="upload"]').click();
    await fire('files:drop', { kind: 'enter', count: 1 });
    assert.match(await $('.file-drop-overlay').getText(), /Drop files or folders to upload/);
    await drop(); await completed();
    assert.equal((await call('upload_dropped_files'))[0][1].attach, false);
    await $('#file-drop-options').click();
    await $('[data-drop-action="attach"]').click();
    await drop('attach-next'); await completed();
    assert.equal((await call('upload_dropped_files'))[1][1].attach, true);
  });

  it('reports inserted paths and attachment failures without claiming skipped files were attached', async () => {
    await fixture({ uploadReport: { directory: '/work', cancelled: false, warnings: [], items: [
      { name: 'image.png', status: 'uploaded', detail: '', inserted: true },
      { name: 'old.png', status: 'skipped', detail: 'Already exists.', inserted: false },
      { name: 'late.png', status: 'uploaded', detail: 'Not added to terminal: program changed', inserted: false },
    ] } });
    await drop('result', 3); await completed();
    assert.match(await $(`${panel} .upload-detail`).getText(), /1 added to terminal/);
    assert.match(await $(`${panel} .upload-results`).getText(), /program changed/);
    assert.equal((await call('session_input')).length, 0);
  });

  it('does not treat a web page drop or an empty native drop as a local file grant', async () => {
    const prevented = await js(`(() => {
      const event = new Event('drop', { bubbles: true, cancelable: true }); document.body.dispatchEvent(event); return event.defaultPrevented;
    })()`);
    assert.equal(prevented, true);
    await drop('empty', 0);
    assert.equal((await call('upload_dropped_files')).length, 0);
    assert.deepEqual(await js('window.__errs'), []);
  });
});
