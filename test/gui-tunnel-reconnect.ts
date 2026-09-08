import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, session } from './tauri-ui-harness.js';

const calls = (name: string): Promise<UiTestInvocation[]> => js(`window.__invocations.filter(([name]) => name === ${JSON.stringify(name)})`);
const port = '.session[data-id="s2"] .tunnel[data-remote="3000"]';
const ready = () => fire('state', { id: 's2', state: 'connected' });
const idle = () => browser.pause(180);
const opened = () => browser.waitUntil(async () => (await calls('open_forwarded_url')).length > 0);

async function fixture(detached = false, backend = {}): Promise<void> {
  await loadFixture([session(1, 'Current workspace'), { ...session(2, 'Background workspace'), detached }], {}, {
    tunnels: { s2: [{ remote: 3000, local: 53000, active: true, scheme: 'https' }, { remote: 8080, local: null, active: false }] }, ...backend,
  });
  await fire('state', { id: 's1', state: 'connected' });
}

describe('Tauri UI: clicking a tunnel reconnects its workspace', () => {
  beforeEach(async () => { await browser.setWindowSize(1100, 700); await fixture(); });

  it('starts an unopened workspace without switching terminals and waits before opening', async () => {
    await $(`${port} .tunnel-link`).click();
    await idle();
    const creates = (await calls('create_session')).filter(call => call[1]?.meta?.id === 's2');
    assert.equal(creates.length, 1);
    assert.equal(creates[0]?.[1]?.meta?.session, 'dt-s2');
    assert.equal((await calls('open_forwarded_url')).length, 0);
    assert.equal(await $(`${port} .topen`).isEnabled(), false);
    await ready(); await opened();
    assert.deepEqual((await calls('open_forwarded_url'))[0]?.[1], { id: 's2', url: 'https://localhost:3000/' });
    assert.equal((await calls('open_external')).length, 0, 'cached active status must never bypass backend validation');
    assert.equal(await js(`document.querySelector('.session.active').dataset.id`), 's1');
  });

  it('reattaches a detached workspace directly from its retained port', async () => {
    await fixture(true);
    await $(`${port} .topen`).click();
    await ready(); await opened();
    assert.equal((await calls('create_session')).filter(call => call[1]?.meta?.id === 's2').length, 1);
    assert.equal((await calls('session_force_reconnect')).length, 0);
    assert.equal(await js(`document.querySelector('.session.active').dataset.id`), 's1');
  });

  it('shares one reconnect across ports and ignores double clicks', async () => {
    await $('.session[data-id="s2"] .name').click();
    await ready();
    await $('.session[data-id="s1"] .name').click();
    await fire('state', { id: 's2', state: 'dead' });
    await $('.session[data-id="s2"] .tunnels-toggle').click();
    await js(`(() => { const button = document.querySelector('${port} .topen'); button.click(); button.click(); document.querySelector('.session[data-id="s2"] .tunnel[data-remote="8080"] .topen').click(); })()`);
    await idle();
    assert.equal((await calls('session_force_reconnect')).length, 1);
    assert.equal((await calls('open_forwarded_url')).length, 0);
    await ready();
    await browser.waitUntil(async () => (await calls('open_forwarded_url')).length === 2);
    assert.equal(await js(`document.querySelector('.session.active').dataset.id`), 's1');
  });

  it('joins an automatic reconnect instead of restarting it', async () => {
    await $('.session[data-id="s2"] .name').click();
    await fire('state', { id: 's2', state: 'reconnecting' });
    await $(`${port} .topen`).click();
    await idle();
    assert.equal((await calls('session_force_reconnect')).length, 0);
    assert.equal((await calls('open_forwarded_url')).length, 0);
    await ready(); await opened();
  });

  it('shows reconnect failures and lets a later port click retry', async () => {
    await fixture(false, { reject: { create_session: 'SSH authentication failed' } });
    await $(`${port} .topen`).click();
    await browser.waitUntil(async () => await $(`${port} .tunnel-error`).isExisting());
    assert.match(await $(`${port} .tunnel-error`).getAttribute('title') || '', /authentication failed/);
    assert.equal((await calls('open_forwarded_url')).length, 0);
    assert.equal(await $(`${port} .topen`).isEnabled(), true);
    await js(`delete window.__BUOY_UI_TEST__.fixture.backend.reject.create_session`);
    await $(`${port} .topen`).click();
    await ready(); await opened();
    assert.deepEqual(await js('window.__errs'), []);
  });

  it('keeps tunnel failures visible and retains the port for retry', async () => {
    await fixture(false, { delay: { open_forwarded_url: 350 }, reject: { open_forwarded_url: 'Remote service did not respond' } });
    await $(`${port} .topen`).click(); await ready(); await opened();
    assert.equal(await $(`${port} .topen`).isEnabled(), false);
    await browser.waitUntil(async () => await $(`${port} .tunnel-error`).isExisting());
    assert.match(await $(`${port} .tunnel-error`).getAttribute('title') || '', /did not respond/);
    assert.equal(await $(`${port} .topen`).isEnabled(), true);
    assert.equal((await calls('open_external')).length, 0);
    assert.deepEqual(await js('window.__errs'), []);
  });

  it('cancels a pending open on detach and leaves its port available', async () => {
    await $(`${port} .topen`).click();
    await $('.session[data-id="s2"] .workspace-menu').click();
    await $('.action-dialog .detach').click();
    await idle(); await ready(); await idle();
    assert.equal((await calls('open_forwarded_url')).length, 0);
    assert.equal(await $(`${port} .tunnel-link`).isExisting(), true);
    assert.equal(await $(`${port} .topen`).isEnabled(), true);
  });
  it('times out a reconnect without opening the browser or leaving controls stuck', async () => {
    await $(`${port} .topen`).click();
    await idle();
    await js(`(() => { window.__realNow = Date.now; Date.now = () => window.__realNow() + 31000; })()`);
    try {
      await browser.waitUntil(async () => await $(`${port} .tunnel-error`).isExisting());
      assert.match(await $(`${port} .tunnel-error`).getAttribute('title') || '', /timed out/);
      assert.equal((await calls('open_forwarded_url')).length, 0);
      assert.equal(await $(`${port} .topen`).isEnabled(), true);
    } finally { await js(`Date.now = window.__realNow`); }
  });

  it('ignores a stale port query that arrives after a newer tunnel event', async () => {
    await js(`window.__BUOY_UI_TEST__.fixture.backend.delay = { list_tunnels: 350 }`);
    await $('.session[data-id="s2"] .name').click();
    await fire('tunnels', { id: 's2', tunnels: [{ remote: 3000, local: 3000, active: true }] });
    await browser.pause(450);
    assert.equal(await $(`${port} .tunnel-link`).getText(), '3000');
  });

  it('does not open on an early Ready event before backend creation finishes', async () => {
    await js(`window.__BUOY_UI_TEST__.fixture.backend.delay = { create_session: 500 }`);
    await $('.session[data-id="s2"] .name').click();
    await $(`${port} .topen`).click();
    await ready(); await idle();
    assert.equal((await calls('open_forwarded_url')).length, 0);
    await opened();
    assert.equal((await calls('create_session')).filter(call => call[1]?.meta?.id === 's2').length, 1);
  });

  it('cancels while backend creation is pending without starting another session', async () => {
    await js(`window.__BUOY_UI_TEST__.fixture.backend.delay = { create_session: 500 }`);
    await $('.session[data-id="s2"] .name').click();
    await $(`${port} .topen`).click();
    await $('.session[data-id="s2"] .workspace-menu').click();
    await $('.action-dialog .detach').click();
    await browser.pause(600);
    assert.equal((await calls('create_session')).filter(call => call[1]?.meta?.id === 's2').length, 1);
    assert.equal((await calls('open_forwarded_url')).length, 0);
    assert.equal(await $(`${port} .topen`).isEnabled(), true);
  });

});
