package com.watchtracker.android

import android.app.Activity
import android.os.Bundle
import android.os.Build
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.window.OnBackInvokedCallback
import androidx.activity.result.ActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.enableEdgeToEdge
import java.util.concurrent.Executors
import org.json.JSONObject

class MainActivity : TauriActivity() {
  private data class PendingExport(
    val requestId: String,
    val fileName: String,
    val token: String,
  )

  private var appWebView: WebView? = null
  private var systemBackCallback: OnBackInvokedCallback? = null
  private var backInFlight = false
  private var pendingExport: PendingExport? = null
  private var exportWriteInFlight = false
  private val exportExecutor = Executors.newSingleThreadExecutor()
  private val exportRequestPattern = Regex(
    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
  )
  private val createJsonDocumentLauncher = registerForActivityResult(
    ActivityResultContracts.StartActivityForResult(),
    ::handleCreateDocumentResult,
  )

  private val documentExportBridge = object {
    @JavascriptInterface
    fun exportJsonDocument(requestId: String, fileName: String, token: String) {
      runOnUiThread { launchDocumentExport(requestId, fileName, token) }
    }
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    AndroidDocumentExporter.cleanupStale(this)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      systemBackCallback = OnBackInvokedCallback { dispatchBack() }
      onBackInvokedDispatcher.registerOnBackInvokedCallback(
        android.window.OnBackInvokedDispatcher.PRIORITY_OVERLAY,
        systemBackCallback!!,
      )
    }
  }

  private fun dispatchBack() {
    if (backInFlight) return
    backInFlight = true
    val webView = appWebView
    if (webView == null) {
      finish()
      return
    }
    webView.evaluateJavascript(
      "window.__WATCHTRACKER_ANDROID_BACK__ ? window.__WATCHTRACKER_ANDROID_BACK__() : 'exit'"
    ) { action ->
      when (action) {
        "\"history\"" -> if (webView.canGoBack()) {
          webView.goBack()
          backInFlight = false
        } else finish()
        "\"consumed\"" -> backInFlight = false
        else -> finish()
      }
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    appWebView = webView
    webView.addJavascriptInterface(documentExportBridge, "watchTrackerDocumentExport")
    super.onWebViewCreate(webView)
  }

  private fun launchDocumentExport(requestId: String, fileName: String, token: String) {
    if (!exportRequestPattern.matches(requestId)) {
      AndroidDocumentExporter.discardStage(this, token)
      return
    }
    if (pendingExport != null || exportWriteInFlight) {
      AndroidDocumentExporter.discardStage(this, token)
      dispatchExportResult(requestId, "error", fileName, "export_already_running")
      return
    }
    try {
      val intent = AndroidDocumentExporter.buildCreateJsonDocumentIntent(fileName)
      pendingExport = PendingExport(requestId, fileName, token)
      createJsonDocumentLauncher.launch(intent)
    } catch (_: Exception) {
      pendingExport = null
      AndroidDocumentExporter.discardStage(this, token)
      dispatchExportResult(requestId, "error", fileName, "document_picker_unavailable")
    }
  }

  private fun handleCreateDocumentResult(result: ActivityResult) {
    val request = pendingExport ?: return
    pendingExport = null
    val uri = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || uri == null) {
      AndroidDocumentExporter.discardStage(this, request.token)
      dispatchExportResult(request.requestId, "cancelled", request.fileName)
      return
    }

    exportWriteInFlight = true
    exportExecutor.execute {
      val status = try {
        AndroidDocumentExporter.writeSelectedDocument(this, uri, request.token)
        "saved"
      } catch (_: Exception) {
        "error"
      }
      runOnUiThread {
        exportWriteInFlight = false
        dispatchExportResult(
          request.requestId,
          status,
          request.fileName,
          if (status == "error") "document_write_failed" else null,
        )
      }
    }
  }

  private fun dispatchExportResult(
    requestId: String,
    status: String,
    fileName: String,
    errorCode: String? = null,
  ) {
    val detail = JSONObject()
      .put("requestId", requestId)
      .put("status", status)
      .put("fileName", fileName)
      .apply { if (errorCode != null) put("errorCode", errorCode) }
      .toString()
    appWebView?.evaluateJavascript(
      "window.dispatchEvent(new CustomEvent('watchtracker:document-export-result',{detail:$detail}))",
      null,
    )
  }

  override fun onResume() {
    super.onResume()
    appWebView?.evaluateJavascript(
      "window.dispatchEvent(new Event('watchtracker:android-resume'))",
      null,
    )
  }

  override fun onDestroy() {
    pendingExport?.let { AndroidDocumentExporter.discardStage(this, it.token) }
    pendingExport = null
    appWebView?.removeJavascriptInterface("watchTrackerDocumentExport")
    exportExecutor.shutdownNow()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      systemBackCallback?.let { onBackInvokedDispatcher.unregisterOnBackInvokedCallback(it) }
    }
    systemBackCallback = null
    appWebView = null
    super.onDestroy()
  }
}
