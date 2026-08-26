#!/usr/bin/env node
/**
 * M1.3 device smoke. Drives the installed Android WebView over bounded CDP
 * calls and invokes the real Tauri episode-history commands. No browser mock
 * or application test hook participates in this check.
 */
import { execFileSync } from 'node:child_process';

const adb = process.env.ADB ?? 'C:/Users/markp/AppData/Local/Android/Sdk/platform-tools/adb.exe';
const pkg = process.env.M13_PACKAGE ?? 'com.watchtracker.android.debug';
const port = Number(process.env.M13_CDP_PORT ?? 9224);
const timeoutMs = Number(process.env.M13_CDP_TIMEOUT_MS ?? 10000);
let forwarded = false;
let activeWebSocket = null;

function runAdb(args) {
  return execFileSync(adb, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function safeError(error) {
  return String(error)
    .replace(/https?:\/\/[^\s"']+/gi, '<url-redacted>')
    .replace(/password|secret|token/gi, '<redacted>');
}

function sleep(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }

async function waitFor(probe, attempts = 60, intervalMs = 500) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const value = await probe();
      if (value) return value;
    } catch { /* target is still starting or being re-enumerated */ }
    await sleep(intervalMs);
  }
  throw new Error('timed out waiting for Android/WebView readiness');
}

async function openCdp() {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const target = targets.find(item => item.type === 'page' && item.webSocketDebuggerUrl);
  if (!target) throw new Error('No WebView page target');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { cleanup(); reject(new Error('CDP open timeout')); }, timeoutMs);
      const cleanup = () => { clearTimeout(timer); ws.removeEventListener('open', opened); ws.removeEventListener('error', failed); ws.removeEventListener('close', failed); };
      const opened = () => { cleanup(); resolve(); };
      const failed = () => { cleanup(); reject(new Error('CDP socket closed during connect')); };
      ws.addEventListener('open', opened); ws.addEventListener('error', failed); ws.addEventListener('close', failed);
    });
  } catch (error) {
    try { ws.close(); } catch { /* best effort */ }
    throw error;
  }
  return ws;
}

function cdpEvaluate(ws, expression, id) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (error, value) => { if (settled) return; settled = true; cleanup(); error ? reject(error) : resolve(value); };
    const cleanup = () => { clearTimeout(timer); ws.removeEventListener('message', onMessage); ws.removeEventListener('error', onError); ws.removeEventListener('close', onClose); };
    const onMessage = event => {
      let message;
      try { message = JSON.parse(String(event.data)); } catch (error) { finish(error); return; }
      if (message.id !== id) return;
      if (message.error) finish(new Error(message.error.message)); else finish(null, message.result);
    };
    const onError = () => finish(new Error('CDP socket error'));
    const onClose = () => finish(new Error('CDP socket closed'));
    const timer = setTimeout(() => finish(new Error('CDP evaluate timeout')), timeoutMs);
    ws.addEventListener('message', onMessage); ws.addEventListener('error', onError); ws.addEventListener('close', onClose);
    try { ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } })); } catch (error) { finish(error); }
  });
}

async function main() {
  if (runAdb(['get-state']) !== 'device') throw new Error('No Android emulator/device is online; M1.3 smoke is blocked');
  runAdb(['shell', 'am', 'force-stop', pkg]);
  runAdb(['shell', 'monkey', '-p', pkg, '1']);
  const pid = await waitFor(() => runAdb(['shell', 'pidof', pkg]).split(/\s+/)[0]);
  const socketName = `webview_devtools_remote_${pid}`;
  await waitFor(() => runAdb(['shell', 'cat', '/proc/net/unix']).includes(socketName));
  runAdb(['forward', `tcp:${port}`, `localabstract:${socketName}`]);
  forwarded = true;
  activeWebSocket = await waitFor(() => openCdp());
  let nextId = 0;
  const evaluate = async expression => {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try { return await cdpEvaluate(activeWebSocket, expression, ++nextId); } catch (error) {
        if (attempt > 0) throw error;
        try { activeWebSocket?.close(); } catch { /* best effort */ }
        activeWebSocket = await waitFor(() => openCdp(), 20);
      }
    }
    throw new Error('CDP evaluation failed');
  };
  await waitFor(async () => (await evaluate('Boolean(window.__TAURI_INTERNALS__?.invoke)'))?.result?.value === true);
  await waitFor(async () => (await evaluate("Boolean(document.querySelector('#mobile-library-title, [data-empty-state=\"library\"], [role=\"alert\"]'))"))?.result?.value === true, 120);

  const expression = `(async () => {
    const invoke = window.__TAURI_INTERNALS__?.invoke;
    if (!invoke) throw new Error('Tauri bridge unavailable');
    const id = 'm13-cdp-' + Date.now();
    const now = new Date().toISOString();
    const record = {id, originalName:'M1.3 Episode Smoke', chineseName:'M1.3逐集烟测', progress:'旧进度 E01', totalEpisodes:5,
      episodeTrackingEnabled:false, nextEpisode:null, movieProgress:null, movieDuration:null, releaseYear:'2026', posterPath:null,
      status:'未看', platform:'', rating:null, startDate:null, endDate:null, notes:'m13-bridge', createdAt:now, updatedAt:null,
      imdbId:null, isLocked:false, genres:null, originCountry:null, imdbRating:null, tmdbStatus:null, interestLevel:null,
      episodeRuntime:45, mediaType:'剧集', contentTags:null, tmdbMediaKind:null, tmdbId:null, tmdbParentId:null,
      tmdbSeasonNumber:null, seriesRecordKind:null, rev:0, revActor:''};
    let finalTracking = null;
    let evidence = null;
    let created = false;
    try {
      const inserted = await invoke('insert_record', {r:record});
      created = true;
      const enabled = await invoke('enable_episode_tracking', {recordId:id, initialNextEpisode:2, expectedRev:inserted.rev});
      if (enabled.record.progress !== '旧进度 E01' || enabled.record.nextEpisode !== 2 || enabled.completions.length !== 0) throw new Error('enable invariant failed');
      const advanced = await invoke('set_next_episode', {recordId:id, nextEpisode:4, expectedRev:enabled.record.rev});
      if (advanced.record.nextEpisode !== 4 || advanced.completions.map(x => x.episodeNumber).join(',') !== '2,3' || advanced.completions[0].completedAt !== null || !advanced.completions[1].completedAt) throw new Error('advance invariant failed');
      const historyBeforeRetreat = JSON.stringify(advanced.completions);
      const retreated = await invoke('set_next_episode', {recordId:id, nextEpisode:2, expectedRev:advanced.record.rev});
      if (retreated.record.nextEpisode !== 2 || JSON.stringify(retreated.completions) !== historyBeforeRetreat) throw new Error('retreat history invariant failed');
      const finished = await invoke('set_next_episode', {recordId:id, nextEpisode:null, expectedRev:retreated.record.rev});
      if (finished.record.status !== '已看' || finished.record.nextEpisode !== null || !finished.record.endDate) throw new Error('atomic finish invariant failed');
      finalTracking = await invoke('get_episode_tracking', {recordId:id});
      const persisted = (await invoke('get_all_records')).find(item => item.id === id);
      const episodes = finalTracking.completions.map(item => item.episodeNumber).join(',');
      if (!persisted || persisted.rev !== finalTracking.record.rev || persisted.progress !== '旧进度 E01' || episodes !== '2,3,4,5') throw new Error('persisted episode result mismatch');
      evidence = {enabled:true, advanced:true, retreated:true, finished:true, persisted:true, rev:persisted.rev, episodes};
    } finally {
      if (created) {
        await invoke('delete_record', {id});
        if ((await invoke('get_all_records')).some(item => item.id === id)) throw new Error('fixture cleanup failed');
      }
    }
    return {...evidence, cleanup:true};
  })()`;
  const result = await evaluate(expression);
  if (result?.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? 'M1.3 page evaluation failed');
  const value = result?.result?.value;
  if (!value?.enabled || !value.advanced || !value.retreated || !value.finished || !value.persisted || !value.cleanup || value.episodes !== '2,3,4,5') throw new Error(`M1.3 episode assertion failed: ${JSON.stringify(value)}`);
  console.log(`OK M1.3 bridge=true enable=true advance=true retreat=true finish=true persisted=true episodes=${value.episodes} rev=${value.rev} cleanup=true`);
}

try {
  await main();
} catch (error) {
  console.error(`FAIL ${safeError(error)}`);
  process.exitCode = 1;
} finally {
  try { activeWebSocket?.close(); } catch { /* best effort */ }
  if (forwarded) {
    try { runAdb(['forward', '--remove', `tcp:${port}`]); } catch { /* best effort */ }
  }
}
