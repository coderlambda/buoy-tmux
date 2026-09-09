import { strict as assert } from 'node:assert';
import { fire, js, loadFixture, session } from './tauri-ui-harness.js';

const card = (id = 's1') => `.session[data-id="${id}"]`;
const reconnect = (id = 's1') => `${card(id)} .session-reconnect`;
const calls = (command: string): Promise<UiTestInvocation[]> => js(`window.__invocations.filter(([name]) => name === ${JSON.stringify(command)})`);

describe('Tauri UI: direct session reconnect controls', () => {
  beforeEach(async () => {
    await browser.setWindowSize(1100, 700);
    await loadFixture([session(1, 'Current workspace'), session(2, 'Background workspace')]);
    await fire('state', { id: 's1', state: 'connected' });
    await fire('ready', { id: 's1' });
  });

  it('shows a labeled icon on disconnected cards without hover and hides it after recovery', async () => {
    assert.equal(await $(reconnect()).isExisting(), false);
    for (const state of ['dead', 'closed', 'reconnecting']) {
      await fire('state', { id: 's1', state });
      await $('#status').moveTo();
      assert.equal(await $(reconnect()).isDisplayed(), true, state);
      assert.equal(await $(reconnect()).isEnabled(), true, state);
      assert.equal(await $(reconnect()).getAttribute('title'), 'Reconnect');
      assert.equal(await $(reconnect()).getAttribute('aria-label'), 'Reconnect');
      assert.equal(await $(reconnect()).getText(), '');
      assert.equal(await $('#term .gate-badge').isEnabled(), true, state);
    }
    await fire('state', { id: 's1', state: 'connected' });
    await fire('ready', { id: 's1' });
    assert.equal(await $(reconnect()).isExisting(), false);
    assert.equal(await $('#term').getAttribute('class'), '');
  });

  it('starts an unopened or detached background session without selecting it', async () => {
    for (const detached of [false, true]) {
      await loadFixture([session(1, 'Current workspace'), { ...session(2, 'Background workspace'), detached }]);
      await $(reconnect('s2')).click();
      await browser.waitUntil(async () => (await calls('create_session')).some(call => call[1]?.meta?.id === 's2'));
      const creates = (await calls('create_session')).filter(call => call[1]?.meta?.id === 's2');
      assert.equal(creates.length, 1);
      assert.equal(creates[0]?.[1]?.meta?.session, 'dt-s2');
      assert.equal((await calls('session_force_reconnect')).length, 0);
      assert.equal(await js(`document.querySelector('.session.active').dataset.id`), 's1');
      await fire('state', { id: 's2', state: 'connected' });
      assert.equal(await $(reconnect('s2')).isExisting(), false);
    }
  });

  it('shares one in-flight request across the card and terminal even with an early state event', async () => {
    await fire('state', { id: 's1', state: 'dead' });
    await js(`window.__BUOY_UI_TEST__.fixture.backend.delay = { session_force_reconnect: 800 }`);
    await js(`(() => {
      document.querySelector('${reconnect()}').click();
      window.__fire('state', { id: 's1', state: 'reconnecting' });
      document.querySelector('${reconnect()}').click();
      document.querySelector('#term .gate-badge').click();
    })()`);
    assert.equal(await $(reconnect()).isEnabled(), false);
    assert.equal(await $(reconnect()).getAttribute('aria-busy'), 'true');
    assert.equal((await calls('session_force_reconnect')).length, 1);
    await browser.waitUntil(async () => await $(reconnect()).isEnabled());
    assert.equal((await calls('session_force_reconnect')).length, 1);
    await $(reconnect()).click();
    await browser.waitUntil(async () => (await calls('session_force_reconnect')).length === 2);
    await browser.pause(850);
    await fire('state', { id: 's1', state: 'connected' });
  });

  it('recreates a closed backend from the terminal reconnect button', async () => {
    await fire('state', { id: 's1', state: 'closed' });
    await $('#term .gate-badge').click();
    await browser.waitUntil(async () => (await calls('create_session')).length === 2);
    assert.equal((await calls('session_retry')).length, 0);
    assert.equal((await calls('session_force_reconnect')).length, 0);
    assert.equal((await calls('create_session'))[1]?.[1]?.meta?.session, 'dt-s1');
  });

  it('reenables both entry points after initial creation or forced reconnect fails', async () => {
    await loadFixture([session(1, 'Failed workspace')], {}, { reject: { create_session: 'SSH authentication failed' } });
    await browser.waitUntil(async () => await $(reconnect()).isEnabled());
    assert.equal(await $('#term .gate-badge').isEnabled(), true);
    await js(`delete window.__BUOY_UI_TEST__.fixture.backend.reject.create_session`);
    await $('#term .gate-badge').click();
    await browser.waitUntil(async () => (await calls('create_session')).length === 2);
    await fire('state', { id: 's1', state: 'dead' });
    await js(`window.__BUOY_UI_TEST__.fixture.backend.reject = { session_force_reconnect: 'Host unreachable' }`);
    await $(reconnect()).click();
    await browser.waitUntil(async () => (await calls('session_force_reconnect')).length === 1 && await $(reconnect()).isEnabled());
    assert.equal(await $('#term .gate-badge').isEnabled(), true);
    assert.match(await $('#status').getText(), /Host unreachable/);
    assert.deepEqual(await js('window.__errs'), []);
  });

  it('keeps force reconnect available in the connected session menu', async () => {
    await $(`${card()} .workspace-menu`).click();
    assert.equal(await $('.action-dialog .reconnect').isEnabled(), true);
    await $('.action-dialog .reconnect').click();
    await browser.waitUntil(async () => (await calls('session_force_reconnect')).length === 1);
    await fire('state', { id: 's1', state: 'connected' });
  });

  it('waits for a manual reconnect before validating and opening a tunnel', async () => {
    await fire('tunnels', { id: 's1', tunnels: [{ remote: 3000, local: 3000, active: true }] });
    await fire('state', { id: 's1', state: 'dead' });
    await js(`window.__BUOY_UI_TEST__.fixture.backend.delay = { session_force_reconnect: 800 }`);
    await $(reconnect()).click();
    await $(`${card()} .topen`).click();
    await fire('state', { id: 's1', state: 'connected' });
    assert.equal((await calls('open_forwarded_url')).length, 0);
    await browser.waitUntil(async () => (await calls('open_forwarded_url')).length === 1);
    assert.equal((await calls('session_force_reconnect')).length, 1);
    assert.deepEqual(await js('window.__errs'), []);
  });
});
