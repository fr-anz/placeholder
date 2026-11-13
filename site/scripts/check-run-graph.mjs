import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
// Run against a local site with CHROME_BIN pointing to a Chromium executable.
const bin = process.env.CHROME_BIN ?? 'chromium';
const browser = spawn(bin, ['--headless', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', '--remote-debugging-port=9339', 'about:blank'], { stdio: 'ignore' });
const sleep = ms => new Promise(r => setTimeout(r, ms));
let ws;
try {
  let tabs;
  for (let i = 0; i < 30; i++) { try { tabs = await (await fetch('http://127.0.0.1:9339/json')).json(); break; } catch { await sleep(100); } }
  ws = new WebSocket(tabs.find(t => t.type === 'page').webSocketDebuggerUrl);
  await new Promise(r => ws.onopen = r);
  let id = 0;
  const pending = new Map();
  const exceptions = [];
  ws.onmessage = m => { const d = JSON.parse(m.data); if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); } if (d.method === 'Runtime.exceptionThrown') exceptions.push(d.params.exceptionDetails.text); };
  const send = (method, params = {}) => new Promise((resolve, reject) => { const request = ++id; pending.set(request, d => d.error ? reject(d.error) : resolve(d.result)); ws.send(JSON.stringify({ id: request, method, params })); });
  const ev = async expression => { const response = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (response.exceptionDetails) throw new Error(JSON.stringify(response.exceptionDetails)); return response.result.value; };
  await send('Runtime.enable');
  await send('Page.enable');


  const origin=process.env.SITE_URL ?? 'http://localhost:3000';
  const settled=()=>ev(`new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
  const offsets=()=>ev(`[...document.querySelectorAll('.g-progress')].map(path=>Number.parseFloat(getComputedStyle(path).strokeDashoffset))`);
  for(const scenario of [{name:'desktop',width:1440,height:1000},{name:'initially-visible',width:1440,height:1000,hash:'#pipeline'},{name:'mobile',width:390,height:844}]) {
    await send('Emulation.setDeviceMetricsOverride',{width:scenario.width,height:scenario.height,deviceScaleFactor:1,mobile:scenario.width<600});
    const navigation=await send('Page.navigate',{url:origin+(scenario.hash??'')});
    assert.ok(!navigation.errorText,navigation.errorText);
    await sleep(1000);
    await ev(`document.fonts.ready`);
    const geometry=await ev(`(()=>{const element=document.querySelector('.graph-scroll');return {top:element.getBoundingClientRect().top+scrollY,height:innerHeight}})()`);
    assert.equal(await ev(`document.querySelectorAll('.g-flow,.g-flow-dot,.g-flow-halo').length`),0,'Moving green dots must be removed.');
    assert.equal(await ev(`document.querySelectorAll('.g-progress').length`),5);
    assert.equal(await ev(`document.querySelector('.graph').getAnimations({subtree:true}).length`),0,'The diagram must not have timed animations.');
    assert.equal(await ev(`(()=>{const base=[...document.querySelectorAll('.g-int,.g-lane')],fills=[...document.querySelectorAll('.g-progress')];return fills.every((fill,index)=>fill.getAttribute('d')===base[index].getAttribute('d')&&getComputedStyle(fill).stroke==='rgb(51, 204, 107)')})()`),true,'Solid green fills must follow the original lines.');
    let previous;
    let middle;
    for(const stage of [{name:'start',top:1.1},{name:'early',top:0.65},{name:'middle',top:0.4},{name:'complete',top:-0.1},{name:'reverse',top:0.4}]) {
      await ev(`scrollTo({top:${geometry.top-geometry.height*stage.top},behavior:'instant'})`);
      await settled();
      const current=await offsets();
      if(stage.name==='start') assert.ok(current.every(value=>Math.abs(value-1)<0.001));
      if(stage.name==='middle') {
        assert.ok(current.some(value=>value>0&&value<1),'Scrolling should reveal a partial green fill.');
        middle=current;
      }
      if(stage.name==='complete') assert.ok(current.every(value=>Math.abs(value)<0.001));
      if(stage.name==='reverse') assert.ok(current.every((value,index)=>Math.abs(value-middle[index])<0.005),'Scrolling back must reverse the fill.');
      else if(previous) assert.ok(current.every((value,index)=>value<=previous[index]+0.005),'The solid green fill must grow as the page scrolls down.');
      previous=current;
    }
    const stopped=await offsets();
    await sleep(250);
    assert.deepEqual(await offsets(),stopped,'The fill must stop when scrolling stops.');
    console.log('PASS',scenario.name,'solid fills follow scrolling, hold when idle, and reverse when scrolling back.');
  }
  await send('Emulation.setEmulatedMedia',{features:[{name:'prefers-reduced-motion',value:'reduce'}]});
  await send('Page.navigate',{url:origin});
  await sleep(1000);
  assert.ok((await offsets()).every(value=>value===0));
  await ev(`document.querySelector('.graph-scroll').scrollIntoView({block:'center',behavior:'instant'})`);
  await settled();
  assert.ok((await offsets()).every(value=>value===0));
  console.log('PASS reduced motion: complete, static green lines.');
  assert.deepEqual(exceptions,[]);
} finally {ws?.close();browser.kill();}
