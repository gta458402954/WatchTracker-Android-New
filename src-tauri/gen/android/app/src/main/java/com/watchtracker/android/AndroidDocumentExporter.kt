package com.watchtracker.android

import android.content.Context
import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import java.io.File
import java.io.IOException

/** Narrow SAF adapter: one app-private staged JSON file to one chosen URI. */
object AndroidDocumentExporter {
  private const val STAGING_DIRECTORY = "export-staging"
  private val tokenPattern = Regex(
    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
  )

  @JvmStatic
  fun buildCreateJsonDocumentIntent(fileName: String): Intent {
    requireValidFileName(fileName)
    return Intent(Intent.ACTION_CREATE_DOCUMENT)
      .addCategory(Intent.CATEGORY_OPENABLE)
      .setType("application/json")
      .putExtra(Intent.EXTRA_TITLE, fileName)
  }

  @Throws(IOException::class)
  fun writeSelectedDocument(context: Context, uri: Uri, token: String) {
    if (!isContentDocumentUri(uri)) throw IOException("invalid_export_document_uri")
    val source = sourceFile(context, token)
    try {
      if (!source.isFile) throw IOException("export_stage_missing")
      source.inputStream().buffered().use { input ->
        val output = context.contentResolver.openOutputStream(uri, "w")
          ?: throw IOException("export_output_unavailable")
        output.buffered().use { buffered ->
          input.copyTo(buffered)
          buffered.flush()
        }
      }
    } finally {
      source.delete()
    }
  }

  @JvmStatic
  fun isContentDocumentUri(uri: Uri): Boolean =
    uri.scheme == ContentResolver.SCHEME_CONTENT

  fun discardStage(context: Context, token: String) {
    runCatching { sourceFile(context, token).delete() }
  }

  fun cleanupStale(context: Context) {
    val directory = stagingDirectory(context)
    directory.listFiles()?.forEach { file ->
      val name = file.name
      val token = name.removeSuffix(".json").takeIf { name.endsWith(".json") }
        ?: name.removeSuffix(".tmp").takeIf { name.endsWith(".tmp") }
      if (file.isFile && token != null && tokenPattern.matches(token)) file.delete()
    }
  }

  private fun sourceFile(context: Context, token: String): File {
    require(tokenPattern.matches(token)) { "invalid_export_stage_token" }
    val directory = stagingDirectory(context).canonicalFile
    val source = File(directory, "$token.json").canonicalFile
    require(source.parentFile == directory) { "invalid_export_stage_path" }
    return source
  }

  // Tauri's Android app_data_dir resolves to ApplicationInfo.dataDir.
  private fun stagingDirectory(context: Context): File =
    File(context.applicationInfo.dataDir, STAGING_DIRECTORY)

  private fun requireValidFileName(fileName: String) {
    require(fileName.endsWith(".json", ignoreCase = true)) { "invalid_export_file_name" }
    require(fileName.length in 6..128) { "invalid_export_file_name" }
    require(fileName.none { it == '/' || it == '\\' || it.isISOControl() }) {
      "invalid_export_file_name"
    }
  }
}
