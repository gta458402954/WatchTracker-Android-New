package com.watchtracker.android

import android.os.Bundle
import android.os.Build
import android.webkit.WebView
import android.window.OnBackInvokedCallback
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  private var appWebView: WebView? = null
  private var systemBackCallback: OnBackInvokedCallback? = null
  private var backInFlight = false

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
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
    super.onWebViewCreate(webView)
  }

  override fun onResume() {
    super.onResume()
    appWebView?.evaluateJavascript(
      "window.dispatchEvent(new Event('watchtracker:android-resume'))",
      null,
    )
  }

  override fun onDestroy() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      systemBackCallback?.let { onBackInvokedDispatcher.unregisterOnBackInvokedCallback(it) }
    }
    systemBackCallback = null
    appWebView = null
    super.onDestroy()
  }
}
