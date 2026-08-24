#!/usr/bin/env node
/**
 * External Android M0 smoke. Requires Node 24 (native WebSocket), adb, and an
 * already-built/installed debug APK. It drives the real Tauri WebView through
 * Chrome DevTools Protocol; no application test hook or manual DevTools step
 * is involved.
 */
import { execFileSync } from 'node:child_process';

const adb = process.env.ADB ?? 'C:/Users/markp/AppData/Local/Android/Sdk/platform-tools/adb.exe';
const pkg = process.env.M0_PACKAGE ?? 'com.watchtracker.android.debug';
const localPort = Number(process.env.M0_CDP_PORT ?? 9222);
const webdavUrl = process.env.M0_WEBDAV_URL ?? 'https://httpbin.org/etag/watchtracker-m0';
let forwarded = false;

function runAdb(args) {
  return execFileSync(adb, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function safeError(error) {
  return String(error).replace(/https?:\/\/[^\s"']+/gi, '<url-redacted>').replace(/password|secret|token/gi, '<redacted>');
}

function sleep(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }

async function waitForSocket(pid) {
  const name = `webview_devtools_remote_${pid}`;
  for (let i = 0; i < 30; i += 1) {
    const unix = runAdb(['shell', 'cat', '/proc/net/unix']);
    if (unix.includes(name)) return name;
    await sleep(500);
  }
  throw new Error('WebView DevTools socket not found');
}

async function waitForPid() {
  for (let i = 0; i < 30; i += 1) {
    try {
      const pid = runAdb(['shell', 'pidof', pkg]).split(/\s+/)[0];
      if (pid) return pid;
    } catch { /* process is still starting */ }
    await sleep(500);
  }
  throw new Error(`Package ${pkg} did not start`);
}

function cdpRequest(ws, method, params, nextId) {
  const id = nextId();
  return new Promise((resolve, reject) => {
    const onMessage = event => {
      const message = JSON.parse(String(event.data));
      if (message.id !== id) return;
      ws.removeEventListener('message', onMessage);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    };
    ws.addEventListener('message', onMessage);
    ws.send(JSON.stringify({ id, method, params }));
  });
}

async function main() {
  runAdb(['shell', 'monkey', '-p', pkg, '1']);
  const pid = await waitForPid();
  const socket = await waitForSocket(pid);
  runAdb(['forward', `tcp:${localPort}`, `localabstract:${socket}`]);
  forwarded = true;
  const targets = await (await fetch(`http://127.0.0.1:${localPort}/json`)).json();
  const target = targets.find(item => item.type === 'page' && item.webSocketDebuggerUrl);
  if (!target) throw new Error('No WebView page target found');

  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true });
    ws.addEventListener('error', reject, { once: true });
  });
  let id = 0;
  const nextId = () => ++id;
  // The WebView DevTools socket can appear before Tauri has injected its
  // bridge. Wait on the bridge itself so a cold emulator launch is reliable.
  let bridgeReady = false;
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const probe = await cdpRequest(ws, 'Runtime.evaluate', {
      expression: 'Boolean(window.__TAURI_INTERNALS__?.invoke && window.__TAURI_INTERNALS__?.convertFileSrc)',
      returnByValue: true,
    }, nextId);
    if (probe.result?.value === true) {
      bridgeReady = true;
      break;
    }
    await sleep(500);
  }
  if (!bridgeReady) throw new Error('Tauri bridge did not become ready');
  const expression = `
    (async () => {
      const invoke = window.__TAURI_INTERNALS__?.invoke;
      const convertFileSrc = window.__TAURI_INTERNALS__?.convertFileSrc;
      if (!invoke || !convertFileSrc) throw new Error('Tauri bridge unavailable');
      const id = 'm0-cdp-' + Date.now();
      const now = new Date().toISOString();
      const record = {id, originalName:'M0 CDP Smoke', chineseName:'M0 CDP烟测', progress:'', totalEpisodes:null,
        episodeTrackingEnabled:false, nextEpisode:null, movieProgress:null, movieDuration:null, releaseYear:'2026',
        posterPath:null, status:'未看', platform:'', rating:null, startDate:null, endDate:null, notes:'cdp-insert',
        createdAt:now, updatedAt:null, imdbId:null, isLocked:false, genres:null, originCountry:null, imdbRating:null,
        tmdbStatus:null, interestLevel:null, episodeRuntime:null, mediaType:'电影', contentTags:null,
        tmdbMediaKind:null, tmdbId:null, tmdbParentId:null, tmdbSeasonNumber:null, seriesRecordKind:null, rev:0, revActor:''};
      await invoke('insert_record', {r:record});
      let rows = await invoke('get_all_records');
      if (!rows.some(item => item.id === id)) throw new Error('insert/get failed');
      await invoke('update_record', {id, updates:{notes:'cdp-update'}, actorId:null});
      rows = await invoke('get_all_records');
      if (rows.find(item => item.id === id)?.notes !== 'cdp-update') throw new Error('update failed');
      await invoke('delete_record', {id});
      rows = await invoke('get_all_records');
      if (rows.some(item => item.id === id)) throw new Error('delete failed');
      const dav = await invoke('probe_webdav_request', {request:{method:'GET', url:${JSON.stringify(webdavUrl)}, username:'m0', password:'not-used', body:null, proxy:null, ifMatch:null, ifNoneMatch:null, ifDavEtag:null}});
      if (dav.status !== 200 || !dav.body || !dav.etag) throw new Error('WebDAV status/body/etag assertion failed');
      const file = await invoke('m0_prepare_poster');
      const img = new Image();
      const loaded = new Promise((resolve, reject) => { img.onload=resolve; img.onerror=()=>reject(new Error('poster load failed')); });
      img.src = convertFileSrc(file, 'poster'); await loaded;
      if (img.naturalWidth <= 0) throw new Error('poster naturalWidth assertion failed');
      const bad = new Image();
      const rejected = new Promise(resolve => { bad.onload=()=>resolve(false); bad.onerror=()=>resolve(true); });
      bad.src = convertFileSrc('../watchtracker.db', 'poster');
      if (!(await rejected)) throw new Error('poster traversal accepted');
      return {crud:true, webdav:{status:dav.status, etag:dav.etag}, poster:{loaded:true, traversalRejected:true}};
    })()
  `;
  const result = await cdpRequest(ws, 'Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, nextId);
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text ?? 'Runtime exception');
  const value = result.result?.value;
  if (!value?.crud || !value?.webdav || !value?.poster) throw new Error('Smoke returned incomplete result');
  console.log(`OK CRUD=true WebDAV status=${value.webdav.status} etag=${String(value.webdav.etag).slice(0, 80)} poster=true traversalRejected=true`);
  ws.close();
}

try {
  await main();
} catch (error) {
  console.error(`FAIL ${safeError(error)}`);
  process.exitCode = 1;
} finally {
  if (forwarded) {
    try { runAdb(['forward', '--remove', `tcp:${localPort}`]); } catch { /* best effort cleanup */ }
  }
}
