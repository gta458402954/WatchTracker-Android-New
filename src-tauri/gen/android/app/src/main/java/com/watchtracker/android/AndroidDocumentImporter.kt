package com.watchtracker.android

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID

/** Narrow SAF adapter: one selected content URI to one app-private staged JSON file. */
object AndroidDocumentImporter {
  private const val STAGING_DIRECTORY = "import-staging"
  private const val MAX_IMPORT_BYTES = 128L * 1024L * 1024L
  private val tokenPattern = Regex(
    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
  )

  data class Selection(val token: String, val fileName: String, val sizeBytes: Long)

  @JvmStatic
  fun buildOpenJsonDocumentIntent(): Intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
    .addCategory(Intent.CATEGORY_OPENABLE)
    .setType("application/json")

  @JvmStatic
  fun isContentDocumentUri(uri: Uri): Boolean = uri.scheme == ContentResolver.SCHEME_CONTENT

  @Throws(IOException::class)
  fun stageSelectedDocument(context: Context, uri: Uri): Selection {
    if (!isContentDocumentUri(uri)) throw IOException("invalid_import_document_uri")
    val directory = stagingDirectory(context)
    if (!directory.exists() && !directory.mkdirs()) {
      throw IOException("import_staging_unavailable")
    }
    val token = UUID.randomUUID().toString()
    val temporary = stageFile(directory, token, ".tmp")
    val target = stageFile(directory, token, ".json")
    var copied = 0L
    try {
      val source = context.contentResolver.openInputStream(uri)
        ?: throw IOException("import_input_unavailable")
      source.buffered().use { input ->
        FileOutputStream(temporary).buffered().use { output ->
          val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
          while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            copied += count
            if (copied > MAX_IMPORT_BYTES) throw IOException("import_file_too_large")
            output.write(buffer, 0, count)
          }
          output.flush()
        }
      }
      FileOutputStream(temporary, true).use { it.fd.sync() }
      if (target.exists() || !temporary.renameTo(target)) {
        throw IOException("import_stage_rename_failed")
      }
      return Selection(token, displayName(context, uri), copied)
    } catch (error: Exception) {
      temporary.delete()
      target.delete()
      if (error is IOException) throw error
      throw IOException("document_read_failed", error)
    }
  }

  fun cleanupStale(context: Context) {
    stagingDirectory(context).listFiles()?.forEach { file ->
      val name = file.name
      val token = name.removeSuffix(".json").takeIf { name.endsWith(".json") }
        ?: name.removeSuffix(".tmp").takeIf { name.endsWith(".tmp") }
      if (file.isFile && token != null && tokenPattern.matches(token)) file.delete()
    }
  }

  private fun displayName(context: Context, uri: Uri): String {
    val providerName = runCatching {
      context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        ?.use { cursor ->
          if (!cursor.moveToFirst()) null
          else cursor.getString(cursor.getColumnIndexOrThrow(OpenableColumns.DISPLAY_NAME))
        }
    }.getOrNull()
    return providerName
      ?.trim()
      ?.takeIf { it.isNotEmpty() && it.length <= 256 && it.none(Char::isISOControl) }
      ?.replace('/', '_')
      ?.replace('\\', '_')
      ?: "WatchTracker-backup.json"
  }

  private fun stageFile(directory: File, token: String, suffix: String): File {
    require(tokenPattern.matches(token)) { "invalid_import_stage_token" }
    val canonicalDirectory = directory.canonicalFile
    val file = File(canonicalDirectory, "$token$suffix").canonicalFile
    require(file.parentFile == canonicalDirectory) { "invalid_import_stage_path" }
    return file
  }

  // Tauri's Android app_data_dir resolves to ApplicationInfo.dataDir.
  private fun stagingDirectory(context: Context): File =
    File(context.applicationInfo.dataDir, STAGING_DIRECTORY)
}
