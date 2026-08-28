#!/usr/bin/env node
/**
 * M1.4 bounded device smoke. It uses the installed app, real Tauri IPC/Rust,
 * Android Keystore, the mobile coordinator and a controlled local WebDAV.
 * Every readiness/network/CDP wait is bounded and every failed assertion exits
 * non-zero. No public WebDAV service is involved.
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const adb = process.env.ADB ?? 'C:/Users/markp/AppData/Local/Android/Sdk/platform-tools/adb.exe';
const pkg = process.env.M14_PACKAGE ?? 'com.watchtracker.android.debug';
const cdpPort = Number(process.env.M14_CDP_PORT ?? 9225);
const mockPort = Number(process.env.M14_MOCK_PORT ?? 18144);
const timeoutMs = Number(process.env.M14_TIMEOUT_MS ?? 15000);
const password = `m14-keystore-${Date.now()}`;
const targetUrl = `http://127.0.0.1:${mockPort}/dav/`;
const apk = process.env.M14_APK ?? 'src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk';
let ws = null;
let forwarded = false;
let nextCdpId = 0;
let server = null;

const now = '2026-08-27T00:00:00.000Z';
const recordId = 'm14-remote-series';
const state = {
  etagSequence: 1,
  etag: '"m14-1"',
  requestCount: 0,
  authenticatedRequests: 0,
  authenticationFailures: 0,
  conditionalPuts: 0,
  unconditionalPuts: 0,
  preconditions: 0,
  force412: 0,
  mutateOn412: null,
  payload: {
    schemaVersion: 4, documentId: 'm14-device-document', revision: 1,
    commitId: 'm14-desktop-seed', parentCommitId: null, writerId: 'desktop-device', committedAt: now,
    records: [{
      id: recordId, originalName: 'M1.4 Remote Series', chineseName: 'M1.4跨端剧集', progress: '', totalEpisodes: 3,
      episodeTrackingEnabled: true, nextEpisode: 1, movieProgress: null, movieDuration: null, releaseYear: '2026',
      posterPath: null, status: '在看', platform: '', rating: null, startDate: '2026-08-27', endDate: null,
      notes: '', createdAt: now, updatedAt: now, imdbId: null, isLocked: false, genres: null,
      originCountry: null, imdbRating: null, tmdbStatus: null, interestLevel: null, episodeRuntime: 45,
      mediaType: '剧集', contentTags: null, tmdbMediaKind: null, tmdbId: null, tmdbParentId: null,
      tmdbSeasonNumber: null, seriesRecordKind: null, rev: 1, revActor: 'desktop-device',
    }],
    tombstones: [], episodeCompletions: [],
  },
};

function runAdb(args, options = {}) {
  return execFileSync(adb, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 30000, maxBuffer: 20 * 1024 * 1024, ...options }).trim();
}

function runAdbBuffer(args) {
  return execFileSync(adb, args, { encoding: null, stdio: ['ignore', 'pipe', 'pipe'], timeout: 30000, maxBuffer: 100 * 1024 * 1024 });
}

function sha256(value) { return createHash('sha256').update(value).digest('hex').toUpperCase(); }

function installVerifiedApk() {
  const localBytes = readFileSync(apk);
  execFileSync(adb, ['install', '-r', apk], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 120000 });
  const remote = runAdb(['shell', 'pm', 'path', pkg]).replace(/^package:/, '').trim();
  if (!remote) throw new Error('installed base.apk path missing');
  const directory = mkdtempSync(join(tmpdir(), 'watchtracker-m14-apk-'));
  const pulled = join(directory, 'base.apk');
  try {
    execFileSync(adb, ['pull', remote, pulled], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 120000 });
    const localHash = sha256(localBytes); const installedHash = sha256(readFileSync(pulled));
    if (localHash !== installedHash) throw new Error('installed APK SHA256 mismatch');
    return { localHash, installedHash, remote };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function safeError(error) {
  return String(error).replaceAll(password, '<redacted>').replace(/https?:\/\/[^\s"']+/gi, '<url-redacted>').replace(/password|secret|token|authorization/gi, '<redacted>');
}

function stage(label) { console.log(`M1.4 smoke: ${label}`); }

function sleep(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }

async function waitFor(probe, label, attempts = 60, intervalMs = 250) {
  let lastError = null;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try { const value = await probe(); if (value) return value; } catch (error) { lastError = error; }
    await sleep(intervalMs);
  }
  throw new Error(`${label} timeout${lastError ? `: ${safeError(lastError)}` : ''}`);
}

function bumpRemote(writer = 'desktop-device') {
  state.etagSequence += 1;
  state.etag = `"m14-${state.etagSequence}"`;
  state.payload.revision += 1;
  state.payload.parentCommitId = state.payload.commitId;
  state.payload.commitId = `m14-${writer}-${state.etagSequence}`;
  state.payload.writerId = writer;
  state.payload.committedAt = new Date().toISOString();
}

function requestBody(request, limit = 5 * 1024 * 1024) {
  return new Promise((resolve, reject) => {
    const chunks = []; let length = 0; let settled = false;
    const finish = (error, value) => { if (settled) return; settled = true; clearTimeout(timer); error ? reject(error) : resolve(value); };
    const timer = setTimeout(() => finish(new Error('mock request body timeout')), timeoutMs);
    request.on('data', chunk => { length += chunk.length; if (length > limit) { request.destroy(); finish(new Error('mock request too large')); } else chunks.push(chunk); });
    request.on('end', () => finish(null, Buffer.concat(chunks).toString('utf8')));
    request.on('error', error => finish(error));
  });
}

function hasExactBasicCredential(request) {
  const header = String(request.headers.authorization || '');
  if (!header.startsWith('Basic ')) {
    state.authenticationFailures += 1;
    return false;
  }
  const decoded = Buffer.from(header.slice('Basic '.length), 'base64').toString('utf8');
  const separator = decoded.indexOf(':');
  const valid = separator >= 0
    && decoded.slice(0, separator) === 'm14-user'
    && decoded.slice(separator + 1) === password;
  if (valid) state.authenticatedRequests += 1;
  else state.authenticationFailures += 1;
  return valid;
}

function buildServer() {
  return createServer(async (request, response) => {
    try {
      state.requestCount += 1;
      if (!hasExactBasicCredential(request)) { response.writeHead(401); response.end(); return; }
      if (request.method === 'MKCOL') { response.writeHead(405); response.end(); return; }
      if (request.method === 'GET' && request.url?.endsWith('/records.json')) { response.writeHead(404); response.end(); return; }
      if (request.method === 'GET' && request.url?.endsWith('/records-v3.json')) {
        response.writeHead(200, { 'Content-Type': 'application/json', ETag: state.etag }); response.end(JSON.stringify(state.payload)); return;
      }
      if (request.method === 'PROPFIND' && request.url?.endsWith('/records-v3.json')) {
        const escaped = state.etag.replaceAll('&', '&amp;').replaceAll('"', '&quot;');
        response.writeHead(207, { 'Content-Type': 'application/xml' });
        response.end(`<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop><d:getetag>${escaped}</d:getetag></d:prop></d:propstat></d:response></d:multistatus>`); return;
      }
      if (request.method === 'PUT' && request.url?.endsWith('/records-v3.json')) {
        const body = await requestBody(request);
        const conditional = request.headers['if-match'] === state.etag
          || request.headers['if-none-match'] === '*'
          || String(request.headers.if || '').includes(state.etag);
        if (conditional) state.conditionalPuts += 1; else state.unconditionalPuts += 1;
        if (!conditional) { response.writeHead(412, { ETag: state.etag }); response.end(); return; }
        if (state.force412 > 0) {
          state.force412 -= 1; state.preconditions += 1;
          state.mutateOn412?.(); state.mutateOn412 = null;
          response.writeHead(412, { ETag: state.etag }); response.end(); return;
        }
        state.payload = JSON.parse(body);
        state.etagSequence += 1; state.etag = `"m14-${state.etagSequence}"`;
        response.writeHead(204, { ETag: state.etag }); response.end(); return;
      }
      response.writeHead(404); response.end();
    } catch {
      response.writeHead(500); response.end();
    }
  });
}

async function startServer() {
  if (server) return;
  server = buildServer();
  await Promise.race([
    new Promise((resolve, reject) => { server.once('error', reject); server.listen(mockPort, '127.0.0.1', resolve); }),
    new Promise((_, reject) => setTimeout(() => reject(new Error('mock listen timeout')), timeoutMs)),
  ]);
}

async function stopServer() {
  const current = server; server = null;
  if (!current) return;
  await Promise.race([
    new Promise(resolve => current.closeAllConnections ? (current.closeAllConnections(), current.close(resolve)) : current.close(resolve)),
    new Promise((_, reject) => setTimeout(() => reject(new Error('mock close timeout')), timeoutMs)),
  ]);
}

async function openCdp() {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  let targets;
  try { targets = await (await fetch(`http://127.0.0.1:${cdpPort}/json`, { signal: controller.signal })).json(); }
  finally { clearTimeout(timer); }
  const target = targets.find(item => item.type === 'page' && item.webSocketDebuggerUrl);
  if (!target) throw new Error('No WebView page target');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { cleanup(); reject(new Error('CDP open timeout')); }, timeoutMs);
    const cleanup = () => { clearTimeout(timeout); socket.removeEventListener('open', opened); socket.removeEventListener('error', failed); socket.removeEventListener('close', failed); };
    const opened = () => { cleanup(); resolve(); }; const failed = () => { cleanup(); reject(new Error('CDP open failed')); };
    socket.addEventListener('open', opened); socket.addEventListener('error', failed); socket.addEventListener('close', failed);
  });
  return socket;
}

function evaluateOnce(expression) {
  const socket = ws; const id = ++nextCdpId;
  return new Promise((resolve, reject) => {
    if (!socket || socket.readyState !== WebSocket.OPEN) { reject(new Error('CDP socket not open')); return; }
    let settled = false;
    const finish = (error, value) => { if (settled) return; settled = true; cleanup(); error ? reject(error) : resolve(value); };
    const cleanup = () => { clearTimeout(timer); socket.removeEventListener('message', onMessage); socket.removeEventListener('error', onError); socket.removeEventListener('close', onClose); };
    const onMessage = event => { let message; try { message = JSON.parse(String(event.data)); } catch (error) { finish(error); return; } if (message.id !== id) return; if (message.error) finish(new Error(message.error.message)); else finish(null, message.result); };
    const onError = () => finish(new Error('CDP socket error')); const onClose = () => finish(new Error('CDP socket closed'));
    const timer = setTimeout(() => finish(new Error('CDP evaluate timeout')), timeoutMs);
    socket.addEventListener('message', onMessage); socket.addEventListener('error', onError); socket.addEventListener('close', onClose);
    socket.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
  });
}

async function evaluate(expression) {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const result = await evaluateOnce(expression);
      if (result.exceptionDetails) {
        const error = new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? 'page evaluation failed');
        error.pageEvaluation = true;
        throw error;
      }
      return result.result?.value;
    } catch (error) {
      if (error?.pageEvaluation) throw error;
      if (attempt) throw error;
      try { ws?.close(); } catch { /* best effort */ }
      ws = await waitFor(openCdp, 'CDP reconnect', 20, 250);
    }
  }
  throw new Error('CDP evaluation failed');
}

async function connectApp(clear = false) {
  try { ws?.close(); } catch { /* best effort */ }
  if (forwarded) { try { runAdb(['forward', '--remove', `tcp:${cdpPort}`]); } catch { /* best effort */ } forwarded = false; }
  if (clear) runAdb(['shell', 'pm', 'clear', pkg]);
  runAdb(['shell', 'monkey', '-p', pkg, '1']);
  const pid = await waitFor(() => runAdb(['shell', 'pidof', pkg]).split(/\s+/)[0], 'app pid');
  const socketName = `webview_devtools_remote_${pid}`;
  await waitFor(() => runAdb(['shell', 'cat', '/proc/net/unix']).includes(socketName), 'WebView socket');
  runAdb(['forward', `tcp:${cdpPort}`, `localabstract:${socketName}`]); forwarded = true;
  ws = await waitFor(openCdp, 'CDP page');
  await waitFor(() => evaluate('Boolean(window.__TAURI_INTERNALS__?.invoke)'), 'Tauri bridge');
  await waitFor(() => evaluate("Boolean(document.querySelector('#mobile-library-title, [data-empty-state=\"library\"], [role=\"alert\"]'))"), 'mobile library', 120, 500);
}

function assert(condition, message) { if (!condition) throw new Error(message); }

async function main() {
  stage('install and verify APK');
  if (runAdb(['get-state']) !== 'device') throw new Error('No Android emulator/device is online; M1.4 smoke is blocked');
  const apkEvidence = installVerifiedApk();
  runAdb(['logcat', '-c']);
  await startServer();
  runAdb(['reverse', `tcp:${mockPort}`, `tcp:${mockPort}`]);
  await connectApp(true);

  stage('case 1 local-first cold start');
  const cold = await evaluate(`(async()=>{const invoke=window.__TAURI_INTERNALS__.invoke;const records=await Promise.race([invoke('get_all_records'),new Promise((_,r)=>setTimeout(()=>r(new Error('local read timeout')),5000))]);return {array:Array.isArray(records),localReady:Boolean(document.querySelector('#mobile-library-title,[data-empty-state="library"]'))};})()`);
  assert(cold?.array && cold.localReady, 'Case 1 local-first cold start failed');

  stage('cases 2-3 secure activation and initial pull');
  const configured = await evaluate(`(async()=>{
    window.__M14_CONSOLE__=[]; for(const level of ['log','warn','error']){const original=console[level].bind(console);console[level]=(...args)=>{window.__M14_CONSOLE__.push(args.map(String).join(' '));original(...args);};}
    const wait=async fn=>{for(let i=0;i<80;i++){const value=await fn();if(value)return value;await new Promise(r=>setTimeout(r,250));}throw new Error('settings action timeout');};
    const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent.trim()===text||item.textContent.trim().endsWith(text));
    button('设置').click(); await wait(()=>document.querySelector('#mobile-sync-title'));
    const input=label=>[...document.querySelectorAll('label')].find(item=>item.childNodes[0]?.textContent?.trim()===label)?.querySelector('input');
    const fill=(element,value)=>{const setter=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;setter.call(element,value);element.dispatchEvent(new Event('input',{bubbles:true}));};
    fill(input('WebDAV URL'),${JSON.stringify(targetUrl)});fill(input('用户名'),'m14-user');fill(input('密码'),${JSON.stringify(password)});
    button('测试连接').click();await wait(()=>document.querySelector('[aria-label="目标检查结果"]'));
    button('确认激活并首次同步').click();
    const connection=await wait(async()=>{const value=await window.__TAURI_INTERNALS__.invoke('get_active_sync_connection');return value?.credentialAvailable?value:null;});
    await wait(async()=>{const rows=await window.__TAURI_INTERNALS__.invoke('get_all_records');return rows.some(item=>item.id===${JSON.stringify(recordId)});});
    return {available:connection.credentialAvailable,passwordEmpty:input('密码').value==='',body:document.body.innerText,connection};
  })()`);
  assert(configured?.available && configured.passwordEmpty, 'Case 2 secure activation failed');
  assert(!configured.body.includes(password) && !JSON.stringify(configured.connection).includes(password), 'Case 2 password returned to WebView');

  stage('case 4 episode write, publish and acknowledgement');
  const progress = await evaluate(`(async()=>{const wait=async fn=>{for(let i=0;i<80;i++){const value=await fn();if(value)return value;await new Promise(r=>setTimeout(r,250));}throw new Error('episode UI timeout');};const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent.trim()===text||item.textContent.trim().endsWith(text));button('片库').click();await wait(()=>button('完成第 1 集'));button('完成第 1 集').click();await wait(async()=>{const rows=await window.__TAURI_INTERNALS__.invoke('get_all_records');return rows.find(item=>item.id===${JSON.stringify(recordId)})?.nextEpisode===2;});const before=await window.__TAURI_INTERNALS__.invoke('get_sync_runtime_state');return {pending:before.outbox.pending};})()`);
  assert(progress?.pending, 'Case 4 local episode write did not enter outbox');
  await waitFor(() => state.payload.records[0]?.nextEpisode === 2 && state.payload.episodeCompletions?.some(item => item.recordId === recordId && item.episodeNumber === 1), 'Case 4 local-write debounce publish', 180, 250);
  const acknowledged = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_sync_runtime_state')`);
  assert(!acknowledged.outbox.pending, 'Case 4 outbox was not acknowledged');

  const appArchive = runAdbBuffer(['exec-out', 'run-as', pkg, 'sh', '-c', 'tar -cf - files databases shared_prefs 2>/dev/null']);
  assert(!appArchive.includes(Buffer.from(password)), 'Case 10 plaintext password found in app-private persisted data');

  stage('case 5 process restart and Keystore reuse');
  state.payload.records[0].notes = 'restart-authenticated-pull';
  state.payload.records[0].rev += 1;
  state.payload.records[0].revActor = 'desktop-device';
  bumpRemote();
  const authenticatedBeforeRestart = state.authenticatedRequests;
  runAdb(['shell', 'am', 'force-stop', pkg]); await connectApp(false);
  const restart = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_active_sync_connection')`);
  assert(restart?.credentialAvailable, 'Case 5 Keystore credential did not survive process restart');
  const restartSync = await waitFor(async () => {
    const records = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_all_records')`);
    const runtime = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_sync_runtime_state')`);
    return records.find(item => item.id === recordId)?.notes === 'restart-authenticated-pull'
      && runtime.scheduler.lastSuccessAt ? { records, runtime } : null;
  }, 'Case 5 startup authenticated sync', 100, 250);
  assert(state.authenticatedRequests > authenticatedBeforeRestart && state.authenticationFailures === 0, 'Case 5 did not authenticate with the saved exact credential');
  assert(restartSync.records.find(item => item.id === recordId)?.nextEpisode === 2, 'Case 5 startup sync lost Android progress');

  stage('case 6 foreground lifecycle pull and merge');
  state.payload.records[0].platform = 'desktop-field-change'; state.payload.records[0].rev += 1; state.payload.records[0].revActor = 'desktop-device'; bumpRemote();
  runAdb(['shell', 'input', 'keyevent', 'KEYCODE_HOME']); await sleep(500); await connectApp(false);
  await waitFor(async () => (await evaluate(`window.__TAURI_INTERNALS__.invoke('get_all_records')`)).find(item => item.id === recordId)?.platform === 'desktop-field-change', 'Case 6 lifecycle pull/merge', 80, 250);
  const merged = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_all_records')`);
  assert(merged.find(item => item.id === recordId)?.nextEpisode === 2 && merged.find(item => item.id === recordId)?.notes === 'restart-authenticated-pull', 'Case 6 remote merge lost Android progress or restart pull');

  stage('case 7 conditional 412 retry');
  state.force412 = 1;
  state.mutateOn412 = () => { state.payload.records[0].platform = 'desktop-platform'; state.payload.records[0].rev += 1; state.payload.records[0].revActor = 'desktop-device'; bumpRemote(); };
  await evaluate(`(async()=>{const wait=async fn=>{for(let i=0;i<80;i++){const v=await fn();if(v)return v;await new Promise(r=>setTimeout(r,250));}throw new Error('second episode timeout');};const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent.trim()===text||item.textContent.trim().endsWith(text));button('片库')?.click();await wait(()=>button('完成第 2 集'));button('完成第 2 集').click();await wait(async()=>{const rows=await window.__TAURI_INTERNALS__.invoke('get_all_records');return rows.find(item=>item.id===${JSON.stringify(recordId)})?.nextEpisode===3;});window.dispatchEvent(new Event('online'));return true;})()`);
  await waitFor(() => state.preconditions === 1 && state.payload.records[0]?.nextEpisode === 3 && state.payload.records[0]?.platform === 'desktop-platform', 'Case 7 conditional retry', 100, 250);
  assert(state.conditionalPuts > 0 && state.unconditionalPuts === 0, 'Case 7 performed an unconditional PUT');

  stage('case 8 offline local write and retained outbox');
  await stopServer();
  await evaluate(`(async()=>{const wait=async fn=>{for(let i=0;i<80;i++){const v=await fn();if(v)return v;await new Promise(r=>setTimeout(r,250));}throw new Error('offline episode timeout');};const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent.trim()===text||item.textContent.trim().endsWith(text));await wait(()=>button('完成第 3 集'));button('完成第 3 集').click();await wait(async()=>{const rows=await window.__TAURI_INTERNALS__.invoke('get_all_records');return rows.find(item=>item.id===${JSON.stringify(recordId)})?.nextEpisode===null;});window.dispatchEvent(new Event('online'));return true;})()`);
  const failedRuntime = await waitFor(async () => { const runtime = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_sync_runtime_state')`); return runtime.scheduler.lastErrorCode ? runtime : null; }, 'Case 8 failed runtime', 80, 250);
  assert(failedRuntime.outbox.pending, 'Case 8 network failure cleared outbox');
  const localAfterFailure = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_all_records')`);
  assert(localAfterFailure.find(item => item.id === recordId)?.status === '已看', 'Case 8 network failure blocked local write');

  stage('case 9 clear credential and preserve scoped state');
  const activeBeforeClear = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_active_sync_connection')`);
  await evaluate(`(async()=>{window.confirm=()=>true;const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent.trim()===text||item.textContent.trim().endsWith(text));button('设置')?.click();for(let i=0;i<40&&!button('清除凭据');i++)await new Promise(r=>setTimeout(r,250));button('清除凭据').click();for(let i=0;i<40;i++){if(!(await window.__TAURI_INTERNALS__.invoke('get_active_sync_connection')))return true;await new Promise(r=>setTimeout(r,250));}throw new Error('credential clear timeout');})()`);
  await startServer(); const requestsBeforeClearTrigger = state.requestCount;
  await evaluate(`window.dispatchEvent(new Event('online'))`); await sleep(1500);
  assert(state.requestCount === requestsBeforeClearTrigger, 'Case 9 automatic sync continued after credential clear');
  const scopedOutbox = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_setting',{key:${JSON.stringify(`sync_target::${activeBeforeClear.targetId}::outbox_v1`)}})`);
  assert(JSON.parse(scopedOutbox).pending, 'Case 9 clear discarded the target-scoped pending outbox');
  assert(activeBeforeClear?.targetId && (await evaluate(`window.__TAURI_INTERNALS__.invoke('get_all_records')`)).some(item => item.id === recordId), 'Case 9 clear removed local data');

  stage('case 10 sensitive data absence');
  const consoleMessages = await evaluate(`window.__M14_CONSOLE__||[]`);
  const logcat = runAdb(['logcat', '-d']);
  assert(state.authenticationFailures === 0, 'Case 10 mock observed a missing or mismatched Basic credential');
  assert(!JSON.stringify(consoleMessages).includes(password), 'Case 10 password found in WebView console');
  assert(!logcat.includes(password) && !/Authorization:\s*Basic/i.test(logcat), 'Case 10 secret or Basic authorization found in logcat');

  const device = runAdb(['shell', 'getprop', 'ro.product.model']); const android = runAdb(['shell', 'getprop', 'ro.build.version.release']);
  console.log(`OK M1.4 cold=true keystore=true pull=true episode=true localWriteDebounce=true restart=true startupSync=true exactBasicAuth=true lifecycle=true merge=true precondition412=${state.preconditions} offlineOutbox=true clear=true sensitive=false conditionalPuts=${state.conditionalPuts} unconditionalPuts=${state.unconditionalPuts} apkSha256=${apkEvidence.localHash} installedSha256=${apkEvidence.installedHash} device=${device} android=${android}`);
}

try {
  await main();
} catch (error) {
  console.error(`FAIL ${safeError(error)}`);
  process.exitCode = 1;
} finally {
  try { ws?.close(); } catch { /* best effort */ }
  try { await stopServer(); } catch { /* best effort */ }
  if (forwarded) { try { runAdb(['forward', '--remove', `tcp:${cdpPort}`]); } catch { /* best effort */ } }
  try { runAdb(['reverse', '--remove', `tcp:${mockPort}`]); } catch { /* best effort */ }
}
