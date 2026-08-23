package com.watchtracker.android

import android.app.Activity
import android.content.Intent
import android.webkit.WebView
import androidx.test.platform.app.InstrumentationRegistry
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertTrue
import org.junit.Test

/** Executes the M0 smoke through the installed Tauri WebView/IPC bridge. */
class DeviceSmokeTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()

    @Test
    fun tauriIpcCrudWebdavAndPosterSmoke() {
        val activity = instrumentation.startActivitySync(
            Intent(instrumentation.targetContext, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        val webView = waitForWebView(activity)
        // The Tauri bridge is injected after the initial document bootstrap.
        Thread.sleep(5_000)
        val result = evaluateSmoke(webView)
        assertTrue("M0 smoke failed: $result", result.startsWith("ok:"))
        activity.finishAndRemoveTask()
    }

    private fun waitForWebView(activity: Activity): WebView {
        repeat(60) {
            var found: WebView? = null
            activity.runOnUiThread { found = findWebView(activity.window.decorView) }
            if (found != null) return found!!
            Thread.sleep(500)
        }
        throw AssertionError("Tauri WebView was not found")
    }

    private fun findWebView(view: android.view.View): WebView? {
        if (view is WebView) return view
        if (view is android.view.ViewGroup) {
            for (index in 0 until view.childCount) {
                findWebView(view.getChildAt(index))?.let { return it }
            }
        }
        return null
    }

    private fun evaluateSmoke(webView: WebView): String {
        var value = "timeout"
        val script = """
            (async function() {
              try {
                const invoke = window.__TAURI_INTERNALS__?.invoke;
                if (!invoke) throw new Error('tauri invoke bridge missing');
                const id = 'm0-device-' + Date.now();
                const now = new Date().toISOString();
                const record = {
                  id, originalName:'M0 Device Smoke', chineseName:'M0设备烟测', progress:'',
                  totalEpisodes:null, episodeTrackingEnabled:false, nextEpisode:null,
                  movieProgress:null, movieDuration:null, releaseYear:'2026', posterPath:null,
                  status:'未看', platform:'', rating:null, startDate:null, endDate:null,
                  notes:'inserted-by-tauri-ipc', createdAt:now, updatedAt:null, imdbId:null,
                  isLocked:false, genres:null, originCountry:null, imdbRating:null,
                  tmdbStatus:null, interestLevel:null, episodeRuntime:null, mediaType:'电影',
                  contentTags:null, tmdbMediaKind:null, tmdbId:null, tmdbParentId:null,
                  tmdbSeasonNumber:null, seriesRecordKind:null, rev:0, revActor:''
                };
                await invoke('insert_record', {r:record});
                let rows = await invoke('get_all_records');
                if (!rows.some(x => x.id === id)) throw new Error('insert/get failed');
                await invoke('update_record', {id, updates:{notes:'updated-by-tauri-ipc'}, actorId:null});
                rows = await invoke('get_all_records');
                if (rows.find(x => x.id === id)?.notes !== 'updated-by-tauri-ipc') throw new Error('update failed');
                await invoke('delete_record', {id});
                rows = await invoke('get_all_records');
                if (rows.some(x => x.id === id)) throw new Error('delete failed');
                const dav = await invoke('probe_webdav_request', {request:{method:'GET', url:'https://httpbin.org/etag/watchtracker-m0', username:'m0', password:'not-a-secret', body:null, proxy:null, ifMatch:null, ifNoneMatch:null, ifDavEtag:null}});
                if (dav.status !== 200 || !dav.etag || !dav.body) throw new Error('webdav GET status/body/etag failed:'+JSON.stringify(dav));
                const file = await invoke('m0_prepare_poster');
                const src = window.__TAURI_INTERNALS__.convertFileSrc ? window.__TAURI_INTERNALS__.convertFileSrc(file,'poster') : null;
                if (!src) throw new Error('convertFileSrc bridge missing');
                const image = new Image();
                const loaded = new Promise((resolve,reject)=>{ image.onload=()=>resolve(true); image.onerror=()=>reject(new Error('poster load failed')); });
                image.src = src; await loaded;
                if (image.naturalWidth <= 0) throw new Error('poster naturalWidth invalid');
                const bad = new Image();
                const rejected = new Promise(resolve=>{ bad.onload=()=>resolve(false); bad.onerror=()=>resolve(true); });
                bad.src = window.__TAURI_INTERNALS__.convertFileSrc('../watchtracker.db','poster');
                if (!(await rejected)) throw new Error('poster traversal accepted');
                return 'ok:'+JSON.stringify({crud:true,webdav:{status:dav.status,etag:dav.etag},poster:true});
              } catch (e) { return 'error:'+(e?.message || String(e)); }
            })()
        """.trimIndent()
        webView.post {
            webView.evaluateJavascript("window.__m0_smoke_result='started'", null)
            webView.evaluateJavascript(
                "(async()=>{try{window.__m0_smoke_result=await ($script)}catch(e){window.__m0_smoke_result='error:'+e}})()",
                null,
            )
        }
        repeat(180) {
            val pollLatch = CountDownLatch(1)
            webView.post {
                webView.evaluateJavascript("window.__m0_smoke_result || ''") { raw ->
                    val cleaned = raw.trim('"').replace("\\\"", "\"")
                    if (cleaned.startsWith("ok:") || cleaned.startsWith("error:")) value = cleaned
                    pollLatch.countDown()
                }
            }
            pollLatch.await(500, TimeUnit.MILLISECONDS)
            if (value.startsWith("ok:") || value.startsWith("error:")) return value
        }
        return value
    }
}
