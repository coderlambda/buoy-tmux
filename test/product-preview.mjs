// Local-only screenshot fixture: the shipped renderer and xterm, with the existing test
// command/event bridge. Does not connect to SSH, read user sessions, or ship in the app/site.
// Run: node test/product-preview.mjs, then open http://127.0.0.1:4174/product-preview
import { readFileSync } from 'node:fs';
import { createServer } from 'vite';
import ts from 'typescript';

const bridge = 'window.__TAURI__ = { core: { invoke: async () => null } };\n' +
  ts.transpileModule(readFileSync(new URL('../src-tauri/ui_test_init.ts', import.meta.url), 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2019, module: ts.ModuleKind.None },
  }).outputText;

function fixture() {
  const scene = new URLSearchParams(location.search).get('productScene') || 'workspace';
  const session = (id, title, host, order, extra = {}) => ({
    id, title, host, order, session: `buoy-demo-${id}`, transport: host ? 'ssh' : 'local',
    mode: 'control', tmuxPath: '/usr/bin/tmux', tmuxVersion: [3, 6],
    color: null, lastTab: null, tabOrder: [], tabColors: {}, ...extra,
  });
  const markdown = '# Atlas workspace\n\nEverything you need, next to your terminal.\n\n## Today’s work\n\n- Keep the development server running in tmux.\n- Preview the generated HTML report.\n- Share a screenshot with the terminal agent.\n\n## Remote preview\n\nThe app runs at **localhost:3000** on the remote machine. Click its address in the terminal to open it through an SSH tunnel.\n\n## Project files\n\n`docs/plan.md` — notes and next steps\n\n`reports/preview.html` — a self-contained page\n\n`assets/` — images and reference files\n';
  const html = '<!doctype html><html lang="en"><meta charset="utf-8"><title>Atlas — build report</title><style>body{margin:0;padding:44px;font:16px/1.6 -apple-system,BlinkMacSystemFont,sans-serif;background:#f5f7fc;color:#25334d}small{color:#3464b4;letter-spacing:.12em}h1{font-size:38px;line-height:1.2;letter-spacing:-1.4px;margin:18px 0}p{color:#55657e;max-width:560px}.status{display:inline-block;background:#e4f0e8;color:#226240;padding:6px 12px;border-radius:5px}.row{display:flex;gap:45px;border-block:1px solid #d5deed;padding:24px 0;margin:30px 0}.row b{font-size:24px;display:block}h2{font-size:21px}li{margin:9px 0}</style><small>ATLAS / BUILD REPORT</small><h1>Ready for the next iteration.</h1><p>A generated HTML page, opened right beside the terminal that created it.</p><span class="status">Preview ready</span><div class="row"><div><b>3</b>Routes</div><div><b>12</b>Components</div><div><b>0</b>Broken links</div></div><h2>What changed</h2><ul><li>Updated the workspace navigation</li><li>Added the project overview</li><li>Prepared reference images for review</li></ul></html>';
  const file = text => ({ data_b64: btoa(unescape(encodeURIComponent(text))), size: new TextEncoder().encode(text).length, truncated: false });
  const seed = {
    token: 'product-preview-v0.1.4',
    sessions: [session('s1', 'Atlas workspace', 'dev@workstation.example', 0, { color: '#89b4fa' }),
      session('s2', 'Infrastructure', 'ops@staging.example', 1, { color: '#a6e3a1' }),
      session('s3', 'Local tools', '', 2, { color: '#cba6f7' }),
      session('s4', 'API prototype', 'dev@staging.example', 3, {
        archived: true, archivedAt: Date.UTC(2026, 8, 20), restorePending: true,
        recoveryTabs: [{ window: '@0', title: 'editor', cwd: '/srv/api', lastCommand: 'nvim' }, { window: '@1', title: 'server', cwd: '/srv/api', lastCommand: 'npm run dev' }],
      })],
    config: { loopbackHosts: ['localhost', '127.0.0.1'], lastActive: 's1' },
    backend: {
      tunnels: { s1: [{ remote: 3000, local: 3000, active: true }] },
      files: { 'docs/plan.md': file(markdown), '/srv/atlas/docs/plan.md': file(markdown),
        'reports/preview.html': file(html), '/srv/atlas/reports/preview.html': file(html) },
      discovery: { tmuxPath: '/usr/bin/tmux', tmuxVersion: [3, 6], sessions: [
        { name: 'atlas-api', windows: 3, attached: 1, created: 20 },
        { name: 'build-tools', windows: 2, attached: 0, created: 10 },
      ] },
      uploadReport: { directory: '/srv/atlas', cancelled: false, warnings: [], items: [
        { name: 'reference.png', status: 'uploaded', detail: '', inserted: true },
        { name: 'assets', status: 'uploaded', detail: '', inserted: true },
        { name: 'notes.md', status: 'uploaded', detail: '', inserted: true },
      ] },
    },
  };
  localStorage.setItem('buoy.theme', 'dark');
  const timer = setInterval(async () => {
    if (!window.__testReset) return;
    clearInterval(timer);
    window.__BUOY_UI_TEST__.setFixture(seed);
    await window.__testReset();
    const fire = window.__fire;
    for (const [win, name] of [['@0', 'shell'], ['@1', 'server'], ['@2', 'agent']]) {
      fire('window', { id: 's1', action: 'add', window: win, name, order: ['@0', '@1', '@2'] });
      fire('window', { id: 's1', action: 'rename', window: win, name });
    }
    const active = scene === 'tunnel' ? '@1' : '@0';
    fire('window', { id: 's1', action: 'active', window: active, order: ['@0', '@1', '@2'] });
    for (const id of ['s1', 's2', 's3']) fire('state', { id, state: 'connected' });
    fire('ready', { id: 's1' });
    const shell = [
      '\x1b[2J\x1b[H', '\x1b[38;5;110mAtlas workspace\x1b[0m',
      '\x1b[38;5;244mdev@workstation  /srv/atlas\x1b[0m', '',
      '$ ls', 'assets/  docs/  reports/  src/  package.json', '',
      '$ cat docs/plan.md', 'Keep the server running. Review the output. Keep moving.', '',
      '\x1b[38;5;110mProject files\x1b[0m',
      '  \x1b]8;;file:///srv/atlas/docs/plan.md\x07docs/plan.md\x1b]8;;\x07',
      '  \x1b]8;;file:///srv/atlas/reports/preview.html\x07reports/preview.html\x1b]8;;\x07', '',
      '\x1b[38;5;110mDevelopment server\x1b[0m',
      '  http://localhost:3000/', '',
      '\x1b[38;5;244mClick a file to preview it. Click localhost to open the service.\x1b[0m', '', '$ ',
    ].join('\r\n');
    const server = [
      '\x1b[2J\x1b[H', '\x1b[38;5;244mdev@workstation  /srv/atlas\x1b[0m', '', '$ npm run dev', '',
      '\x1b[38;5;110m  ATLAS  development server\x1b[0m', '',
      '  Local:   \x1b[38;5;75mhttp://localhost:3000/\x1b[0m',
      '  Network: listening on remote loopback', '',
      '\x1b[32m  Ready for connections\x1b[0m', '',
      '  GET /                 200', '  GET /assets/app.css    200', '  GET /api/workspaces    200', '',
      '\x1b[38;5;244mThe development server stays in this tmux window.\x1b[0m',
    ].join('\r\n');
    fire('data', { id: 's1', window: '@0', data: shell });
    fire('data', { id: 's1', window: '@1', data: server });
    if (scene === 'upload') setTimeout(() => fire('files:drop', { kind: 'drop', token: 'product-upload', count: 3 }), 400);
    if (scene === 'drop') setTimeout(() => fire('files:drop', { kind: 'enter', count: 3 }), 400);
    document.documentElement.dataset.productScene = scene;
  }, 30);
}

const scenes = ['workspace', 'tunnel', 'upload', 'drop'];
const server = await createServer({
  server: { host: '127.0.0.1', port: 4174, strictPort: true },
  plugins: [{
    name: 'product-screenshot-fixture',
    transformIndexHtml() {
      return [
        { tag: 'script', attrs: { src: '/__product/bridge.js' }, injectTo: 'head-prepend' },
        { tag: 'script', attrs: { src: '/__product/fixture.js' }, injectTo: 'head-prepend' },
      ];
    },
    configureServer(vite) {
      vite.middlewares.use((req, res, next) => {
        const url = new URL(req.url, 'http://127.0.0.1:4174');
        if (url.pathname === '/__product/bridge.js' || url.pathname === '/__product/fixture.js') {
          res.setHeader('Content-Type', 'text/javascript');
          res.end(url.pathname.endsWith('bridge.js') ? bridge : `(${fixture.toString()})();`);
        } else if (url.pathname === '/product-preview') {
          const scene = scenes.includes(url.searchParams.get('scene')) ? url.searchParams.get('scene') : 'workspace';
          res.setHeader('Content-Type', 'text/html');
          res.end(`<!doctype html><html lang="en"><meta charset="utf-8"><title>Buoy v0.1.4 product screenshots</title><style>body{margin:24px;background:#e4e7ed;font:14px system-ui;color:#24314b}nav{height:40px;display:flex;gap:24px}a{color:inherit}iframe{display:block;width:1200px;height:760px;border:0}</style><nav>${scenes.map(s => `<a href="/product-preview?scene=${s}">${s}</a>`).join('')}<span>Buoy v0.1.4 · demonstration data</span></nav><iframe title="Buoy screenshot workspace" src="/?productScene=${scene}"></iframe></html>`);
        } else next();
      });
    },
  }],
});
await server.listen();
console.log('Product preview: http://127.0.0.1:4174/product-preview');
