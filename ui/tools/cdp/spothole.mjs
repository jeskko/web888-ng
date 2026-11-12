// SPDX-License-Identifier: GPL-3.0-or-later
// Local simulated receiver + synthetic feed. No live Spothole dependency.
// CHROME=... OUT=... node spothole.mjs http://127.0.0.1:28175/
import assert from "node:assert/strict";
import { launch, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const p = await launch({width: Number(process.env.W ?? 1400),height: Number(process.env.H ?? 900)});
const fixture = `
  window.__spotSent = []; window.__fetchUrls = [];
  const t = Date.now()/1000;
  window.__feed = {enabled:true,state:'ready',last_success:t,error:null,spots:[
    {id:'cw',source:'spothole',t,mode:'CW',reported_mode:'CW',band:'40m',hz:7030000,dial:7030000,call:'OH0CW',msg:'CQ park',activities:['POTA','WWFF'],refs:[{activity:'POTA',id:'FI-0001'}],upstream_source:'POTA'},
    {id:'lsb',source:'spothole',t:t-1,mode:'LSB',reported_mode:'SSB',band:'40m',hz:7150000,dial:7150000,call:'OH0LSB',msg:'CQ summit',activities:['SOTA'],refs:[{activity:'SOTA',id:'OH/XX-001'}],upstream_source:'SOTA'},
    {id:'usb',source:'spothole',t:t-2,mode:'USB',reported_mode:'SSB',band:'20m',hz:14285000,dial:14285000,call:'OH0USB',msg:'CQ',activities:[],refs:[],upstream_source:'Cluster'},
    {id:'60m',source:'spothole',t:t-3,mode:'USB',reported_mode:'SSB',band:'60m',hz:5354000,dial:5354000,call:'OH060M',msg:'CQ',activities:['WWFF'],refs:[],upstream_source:'WWFF'}
  ]};
  const originalFetch = window.fetch.bind(window);
  window.fetch = (input, options) => {
    const target = typeof input === 'string' ? input : input.url;
    window.__fetchUrls.push(new URL(target,location.href).href);
    if (target.endsWith('api/spots/external')) return Promise.resolve(new Response(JSON.stringify(window.__feed),{status:200,headers:{'Content-Type':'application/json'}}));
    return originalFetch(input,options);
  };
  const originalTimeout = window.setTimeout.bind(window);
  window.setTimeout = (fn, delay, ...args) => originalTimeout(fn, delay===30000 ? 100 : delay, ...args);
  const NativeSocket = window.WebSocket;
  window.WebSocket = class extends NativeSocket {
    send(data) { try { window.__spotSent.push(JSON.parse(data)); } catch {} super.send(data); }
    set onmessage(cb) { super.onmessage = (event) => {
      if (typeof event.data === 'string') {
        const message=JSON.parse(event.data);
        if(message.type==='hello') message.spots=false; // prove independent availability
        cb.call(this,new MessageEvent('message',{data:JSON.stringify(message)}));
      } else cb.call(this,event);
    }; }
    get onmessage() { return super.onmessage; }
  };
`;
try {
  await p.cdp("Page.addScriptToEvaluateOnNewDocument", {source: fixture});
  await p.goto(url);
  await sleep(1500);
  // Phone puts the existing controls in the More sheet.
  await p.ev(`([...document.querySelectorAll('button')].find(b=>b.textContent.includes('More')))?.click()`);
  await sleep(100);
  assert.equal(await p.ev(`!![...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='Spots')`), true, "external feed must work without local skimmers");
  await p.ev(`if(!document.querySelector('.spots')) [...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='Spots').click()`);
  await sleep(200);
  assert.equal(await p.ev(`document.querySelectorAll('.spots .spot-record').length`),4);
  if (Number(process.env.W ?? 1400) <= 640) assert.equal(await p.ev(`document.querySelectorAll(".spot-card").length`),4,"phone spots must be full-width cards");
  const phone = Number(process.env.W ?? 1400) <= 640;
  if (phone) {
    assert.equal(await p.ev(`document.querySelectorAll('.spots select').length`),0,"filters start collapsed");
    await p.ev(`document.querySelector('button[aria-controls="spots-filters"]').click()`);
    await sleep(70);
  }
  const filter = async (label,value) => {
    await p.ev(`(()=>{const e=document.querySelector('select[aria-label=${JSON.stringify(label)}]');e.value=${JSON.stringify(value)};e.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    await sleep(70);
  };
  await filter('Spot activity','POTA');
  assert.equal(await p.ev(`document.querySelectorAll('.spots .spot-record').length`),1);
  await filter('Spot activity','WWFF');
  assert.equal(await p.ev(`document.querySelectorAll('.spots .spot-record').length`),2,"dual activation must match WWFF as well as POTA");
  await filter('Spot activity','');
  const cases=[['OH0CW','cw',7030000],['OH0LSB','lsb',7150000],['OH0USB','usb',14285000],['OH060M','usb',5354000]];
  for (const [call,mode,hz] of cases) {
    await p.ev(`document.querySelector('button[aria-label^="Tune ${call} "]').click()`);
    await sleep(700);
    assert.ok((await p.ev(`location.hash`)).includes(`m=${mode}`),`${call}: receive mode`);
    assert.equal(await p.ev(`window.__spotSent.filter(m=>m.type==='tune').at(-1).hz`),hz);
  }
  assert.equal(await p.ev(`document.querySelector('.spots-list').textContent.includes('undefined')`),false);
  if (Number(process.env.W ?? 1400) > 600) assert.equal(await p.ev(`document.querySelectorAll('.spots tbody td:nth-child(4)')[0].textContent`),'—');
  const beforeKeyboard = await p.ev(`window.__spotSent.filter(m=>m.type==='tune').length`);
  await p.ev(`document.querySelector('.spot-tune').focus()`);
  await p.cdp('Input.dispatchKeyEvent',{type:'keyDown',key:' ',code:'Space',windowsVirtualKeyCode:32});
  await p.cdp('Input.dispatchKeyEvent',{type:'keyUp',key:' ',code:'Space',windowsVirtualKeyCode:32});
  await sleep(400);
  assert.ok(await p.ev(`window.__spotSent.filter(m=>m.type==='tune').length`) > beforeKeyboard,"keyboard activation must tune");
  assert.equal(await p.ev(`window.__spotSent.filter(m=>m.type==='tune').at(-1).hz`),7030000);
  if (phone) {
    await p.ev(`document.querySelector('button[aria-controls="spots-filters"]').click()`);
    await sleep(70);
    assert.equal(await p.ev(`document.querySelectorAll('.spots select').length`),0);
    const row = await p.ev(`document.querySelector('.spot-card').textContent`);
    assert.ok(!row.includes('CQ') && !row.includes('UTC') && !row.includes('Spothole') && !row.includes('FI-0001'),"mobile rows omit verbose metadata");
  }
  await p.shot(`${out}/spothole-${process.env.W ?? 1400}.png`);
  const overflow=await p.ev(`({page:document.documentElement.scrollWidth>innerWidth,panel:document.querySelector('.spots').scrollWidth>document.querySelector('.spots').clientWidth,list:document.querySelector('.spots-list').scrollWidth>document.querySelector('.spots-list').clientWidth,sheet:!!document.querySelector('.m-sheet')&&document.querySelector('.m-sheet').scrollWidth>document.querySelector('.m-sheet').clientWidth})`);
  assert.deepEqual(overflow,{page:false,panel:false,list:false,sheet:false});
  await p.ev(`window.__feed={...window.__feed,state:'stale',error:'Fixture upstream unavailable'}`);
  await sleep(200);
  assert.match(await p.ev(`document.querySelector('.spots-status').textContent`),/stale.*Fixture upstream unavailable/);
  await p.ev(`window.__feed={...window.__feed,spots:[]}`);
  await sleep(200);
  assert.match(await p.ev(`document.querySelector('.spots-list').textContent`),/No recent spots/);
  assert.equal(await p.ev(`window.__fetchUrls.some(u=>new URL(u).origin!==location.origin)`),false,"browsers must not contact an external feed");
  console.log(`Spothole browser checks passed at ${process.env.W ?? 1400}px: modes, activity, independent availability, stale/empty, keyboard and overflow`);
} finally { p.close(); }
