import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';

// Run against `npm run dev` so React reports attribute mismatches.
// CHROME_BIN=/path/to/chromium SITE_URL=http://127.0.0.1:3000 npm run test:hydration
const origin = process.env.SITE_URL ?? 'http://127.0.0.1:3000';
const port = 9335;
const browser = spawn(process.env.CHROME_BIN ?? 'chromium', [
  '--headless', '--no-sandbox', '--disable-gpu', `--remote-debugging-port=${port}`, 'about:blank',
], { stdio: 'ignore' });
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let launchError;
browser.on('error', error => { launchError = error; });
let socket;
try {
  let tabs;
  for (let attempt = 0; attempt < 50; attempt++) {
    if (launchError) throw launchError;
    try { tabs = await (await fetch(`http://127.0.0.1:${port}/json`)).json(); break; }
    catch { await sleep(100); }
  }
  assert.ok(tabs, 'Chromium did not start; set CHROME_BIN to its executable.');
  socket = new WebSocket(tabs.find(tab => tab.type === 'page').webSocketDebuggerUrl);
  await new Promise(resolve => { socket.onopen = resolve; });
  let sequence = 0;
  const pending = new Map();
  let errors = [];
  socket.onmessage = message => {
    const data = JSON.parse(message.data);
    if (data.id) { pending.get(data.id)?.(data); pending.delete(data.id); }
    if (data.method === 'Runtime.consoleAPICalled' && data.params.type === 'error') {
      errors.push(data.params.args.map(arg => arg.value ?? arg.description ?? '').join(' '));
    }
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, result => result.error ? reject(result.error) : resolve(result.result));
    socket.send(JSON.stringify({ id, method, params }));
  });
  await send('Runtime.enable');
  await send('Page.enable');
  for (const scenario of ['clean', 'extension', 'descendant']) {
    errors = [];
    let script;
    if (scenario !== 'clean') {
      script = await send('Page.addScriptToEvaluateOnNewDocument', { source: `
        const observer = new MutationObserver(() => {
          const target = document.querySelector(${JSON.stringify(scenario === 'extension' ? 'body' : 'h1')});
          if (!target) return;
          ${scenario === 'extension' ? "target.classList.add('vc-init');" : "target.setAttribute('data-hydration-probe', 'injected');"}
          observer.disconnect();
        });
        observer.observe(document, { childList: true, subtree: true });
      ` });
    }
    const navigation = await send('Page.navigate', { url: origin });
    assert.ok(!navigation.errorText, navigation.errorText);
    await sleep(1500);
    const readiness = await send('Runtime.evaluate', { expression: `new Promise(resolve => {
      const deadline = Date.now() + 15000;
      const check = () => {
        if (document.querySelector('[data-reveal]')) resolve(true);
        else if (Date.now() > deadline) resolve(false);
        else setTimeout(check, 50);
      };
      check();
    })`, awaitPromise: true });
    assert.equal(readiness.result.value, true, 'The app did not hydrate in time.');
    await sleep(600);
    if (script) await send('Page.removeScriptToEvaluateOnNewDocument', { identifier: script.identifier });
    const mismatches = errors.filter(error => /hydrated|hydration|server rendered HTML/i.test(error));
    if (scenario === 'descendant') {
      assert.ok(mismatches.length, 'Control failed: descendant mismatches must still be reported.');
      console.log('PASS: unrelated descendant mismatches remain visible.');
    } else {
      assert.equal(mismatches.length, 0, `${scenario}: ${mismatches.join('\n')}`);
      console.log(`PASS: ${scenario} load has no hydration warning.`);
    }
  }
} finally { socket?.close(); browser.kill(); }
