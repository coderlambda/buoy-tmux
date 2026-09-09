import { strict as assert } from 'node:assert';
import { createChecks, fire, js, loadFixture, session } from './tauri-ui-harness.js';

describe('Tauri UI: durable session lifecycle', () => {
  it('closes an unopened or detached background session without attaching or selecting it', async () => {
    for (const detached of [false, true]) {
      await loadFixture([session(1, 'Current workspace'), { ...session(2, 'Missing remote session'), detached }]);
      await fire('state', { id: 's1', state: 'connected' });
      const before = await js(`window.__invocations.filter(([name]) => name === 'create_session').length`);
      await $('.session[data-id="s2"] .workspace-menu').click();
      await $('.act.kill').click();
      await $('[data-confirm="accept"]').click();
      await browser.waitUntil(async () => js(`!!document.querySelector('#history .session[data-id="s2"]')`));
      assert.equal(await js(`window.__invocations.filter(([name]) => name === 'create_session').length`), before);
      assert.equal(await js(`document.querySelector('#sessions .session.active').dataset.id`), 's1');
      assert.equal(await js(`document.querySelector('#sessions .session[data-id="s2"]') === null`), true);
      assert.equal(await js(`window.__invocations.some(([name, args]) => name === 'session_close' && args.id === 's2')`), true);
    }
  });

  it('removes a failed initial connection through History without another connect attempt', async () => {
    await loadFixture([session(1, 'Missing remote session')], {}, { reject: { create_session: 'Remote session is gone' } });
    await $('.workspace-menu').click();
    await $('.act.kill').click();
    await $('[data-confirm="accept"]').click();
    await browser.waitUntil(async () => js(`document.querySelectorAll('#history .session').length === 1`));
    assert.equal(await js(`window.__invocations.filter(([name]) => name === 'create_session').length`), 1);
    await $('#history .delete').click();
    await $('[data-confirm="accept"]').click();
    await browser.waitUntil(async () => js(`document.querySelectorAll('.session').length === 0`));
    assert.equal(await js(`window.__BUOY_UI_TEST__.fixture.sessions.length`), 0);
    assert.deepEqual(await js('window.__errs'), []);
  });

  it('retains the entry after a close or delete error and allows retry', async () => {
    await loadFixture([session(1, 'Unreachable host')], {}, { reject: { session_close: 'Permission denied (publickey).' } });
    await $('.workspace-menu').click();
    await $('.act.kill').click();
    await $('[data-confirm="accept"]').click();
    await browser.waitUntil(async () => js(`document.querySelector('#status').textContent.includes('Permission denied')`));
    assert.equal(await $$('#sessions .session').length, 1);
    assert.equal(await $$('#history .session').length, 0);
    assert.equal(await $('.session-reconnect').isEnabled(), true);
    await js(`delete window.__BUOY_UI_TEST__.fixture.backend.reject.session_close`);
    await $('.workspace-menu').click();
    await $('.act.kill').click();
    await $('[data-confirm="accept"]').click();
    await browser.waitUntil(async () => js(`document.querySelectorAll('#history .session').length === 1`));
    await js(`window.__BUOY_UI_TEST__.fixture.backend.reject.session_kill = 'Could not delete saved entry'`);
    await $('#history .delete').click();
    await $('[data-confirm="accept"]').click();
    await browser.waitUntil(async () => js(`document.querySelector('#status').textContent.includes('could not delete')`));
    assert.equal(await $$('#history .session').length, 1);
    assert.deepEqual(await js('window.__errs'), []);
  });

  it('distinguishes Detach from Close and reconstructs a closed session from History', async () => {
    const checks = createChecks();
    await loadFixture([session(1, 'Work session')]);
    await fire('window', { id: 's1', action: 'add', window: '@0', name: 'shell', order: ['@0'] });
    await fire('window', { id: 's1', action: 'active', window: '@0', order: ['@0'] });
    await fire('ready', { id: 's1' });

    await js(`window.__testType('echo recover-this-tab\\r')`);
    await $('.workspace-menu').click();
    await $('.act.detach').click();
    await browser.waitUntil(async () => js(`document.querySelector('#sessions .session .sub').textContent.includes('detached')`));
    const detached = await js(`({
      active: document.querySelectorAll('#sessions .session').length,
      history: document.querySelectorAll('#history .session').length,
      invoked: window.__invocations.some(([name]) => name === 'session_detach'),
    })`);
    checks.check(detached.active === 1 && detached.history === 0 && detached.invoked,
      `TC-SH1 Detach leaves tmux in Sessions (got ${JSON.stringify(detached)})`);

    await $('#recover').click();
    await browser.waitUntil(async () => js(`document.querySelector('.chooser-title')?.textContent === 'Open detached sessions'`));
    const found = await js(`({
      label: document.querySelector('.chooser-item')?.textContent,
      checked: window.__invocations.some(([name]) => name === 'check_open_sessions'),
    })`);
    checks.check(found.checked && found.label?.includes('Work session'),
      `TC-SH2 Check open sessions offers the detached tmux session (got ${JSON.stringify(found)})`);
    await $('.chooser-item').click();
    await fire('window', { id: 's1', action: 'add', window: '@0', name: 'shell', order: ['@0'] });
    await fire('window', { id: 's1', action: 'active', window: '@0', order: ['@0'] });
    await fire('ready', { id: 's1' });

    await $('.workspace-menu').click();
    await $('.act.kill').click();
    await $('[data-confirm="accept"]').click();
    await browser.waitUntil(async () => js(`document.querySelectorAll('#history .session').length === 1`));
    const closed = await js(`(() => {
      const call = window.__invocations.filter(([name]) => name === 'session_close').pop();
      return {
        active: document.querySelectorAll('#sessions .session').length,
        history: document.querySelectorAll('#history .session').length,
        lastCommand: call?.[1]?.tabs?.[0]?.lastCommand,
      };
    })()`);
    checks.check(closed.active === 0 && closed.history === 1 && closed.lastCommand === 'echo recover-this-tab',
      `TC-SH3 Close archives a per-tab recovery snapshot (got ${JSON.stringify(closed)})`);

    await $('#history .resume').click();
    await $('[data-confirm="accept"]').click();
    await browser.waitUntil(async () => js(`document.querySelectorAll('#sessions .session').length === 1 && document.querySelectorAll('#history .session').length === 0`));
    const resumed = await js(`({
      resumed: window.__invocations.some(([name]) => name === 'session_resume'),
      reconnects: window.__invocations.filter(([name]) => name === 'create_session').length,
    })`);
    checks.check(resumed.resumed && resumed.reconnects >= 3,
      `TC-SH4 Resume reconstructs and reconnects the closed session (got ${JSON.stringify(resumed)})`);
    checks.finish();
  });

  it('restores archived rows into History without reconnecting them at launch', async () => {
    const checks = createChecks();
    const archived = {
      ...session(1, 'Archived session'),
      archived: true,
      archivedAt: 1234,
      restorePending: true,
      recoveryTabs: [{ window: '@0', title: 'shell', cwd: '/tmp', lastCommand: 'pwd' }],
    };
    await loadFixture([archived]);
    const restored = await js(`({
      active: document.querySelectorAll('#sessions .session').length,
      history: document.querySelectorAll('#history .session').length,
      creates: window.__invocations.filter(([name]) => name === 'create_session').length,
    })`);
    checks.check(restored.active === 0 && restored.history === 1,
      `TC-SH5 archived rows launch in History (got ${JSON.stringify(restored)})`);
    checks.check(restored.creates === 0,
      `TC-SH5 archived rows do not auto-connect (got ${JSON.stringify(restored)})`);
    checks.finish();
  });
});
