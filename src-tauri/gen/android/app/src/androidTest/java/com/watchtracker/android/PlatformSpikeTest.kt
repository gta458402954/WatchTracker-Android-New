package com.watchtracker.android

import android.content.Context
import android.content.Intent
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.net.Uri
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.io.File
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.json.JSONObject

/** M0 executable proof for capabilities that must stay behind Android adapters. */
@RunWith(AndroidJUnit4::class)
class PlatformSpikeTest {
    @Test
    fun appPrivateDirectoryAndKeystoreRoundTrip() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val marker = File(context.filesDir, "watchtracker-m0-spike.bin")
        val plaintext = "WatchTracker M0".toByteArray()
        marker.writeBytes(plaintext)
        assertTrue(marker.canonicalPath.startsWith(context.filesDir.canonicalPath))

        val alias = "watchtracker-m0-spike"
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        try {
            val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            generator.init(
                KeyGenParameterSpec.Builder(
                    alias,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setRandomizedEncryptionRequired(true)
                    .build(),
            )
            generator.generateKey()

            val encrypt = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.ENCRYPT_MODE, keyStore.getKey(alias, null))
            }
            val ciphertext = encrypt.doFinal(plaintext)
            val decrypt = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.DECRYPT_MODE, keyStore.getKey(alias, null), encrypt.parameters)
            }
            assertArrayEquals(plaintext, decrypt.doFinal(ciphertext))
        } finally {
            keyStore.deleteEntry(alias)
            marker.delete()
        }
    }

    @Test
    fun safJsonIntentIsScopedToOpenableDocuments() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("application/json")
        assertEquals(Intent.ACTION_OPEN_DOCUMENT, intent.action)
        assertTrue(intent.categories?.contains(Intent.CATEGORY_OPENABLE) == true)
        assertEquals("application/json", intent.type)
    }

    @Test
    fun safJsonExportIntentCreatesOpenableDocument() {
        val fileName = "WatchTracker-backup-2026-08-29-173500.json"
        val intent = AndroidDocumentExporter.buildCreateJsonDocumentIntent(fileName)
        assertEquals(Intent.ACTION_CREATE_DOCUMENT, intent.action)
        assertTrue(intent.categories?.contains(Intent.CATEGORY_OPENABLE) == true)
        assertEquals("application/json", intent.type)
        assertEquals(fileName, intent.getStringExtra(Intent.EXTRA_TITLE))
        assertTrue(AndroidDocumentExporter.isContentDocumentUri(Uri.parse("content://documents/export.json")))
        assertTrue(!AndroidDocumentExporter.isContentDocumentUri(Uri.parse("file:///data/user/0/com.watchtracker.android.debug/watchtracker.db")))
    }

    @Test
    fun productionSafJsonImportIntentOpensOpenableDocument() {
        val intent = AndroidDocumentImporter.buildOpenJsonDocumentIntent()
        assertEquals(Intent.ACTION_OPEN_DOCUMENT, intent.action)
        assertTrue(intent.categories?.contains(Intent.CATEGORY_OPENABLE) == true)
        assertEquals("application/json", intent.type)
        assertTrue(AndroidDocumentImporter.isContentDocumentUri(Uri.parse("content://documents/backup.json")))
        assertTrue(!AndroidDocumentImporter.isContentDocumentUri(Uri.parse("file:///sdcard/backup.json")))
    }

    @Test
    fun productionSecretStoreEncryptsAtRestRejectsTamperingAndDeletes() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val target = "WatchTracker/v1/webdav/${"a".repeat(64)}"
        val plaintext = "m14-device-password-${System.nanoTime()}".toByteArray()
        try {
            assertEquals(0, AndroidSecretStore.write(context, target, "device-user", plaintext.copyOf()))
            val targetDigest = MessageDigest.getInstance("SHA-256").digest(target.toByteArray())
                .joinToString("") { "%02x".format(it) }
            val stored = File(context.filesDir, "secure-vault-v1").listFiles()
                ?.singleOrNull { it.name == "$targetDigest.json" }
            assertTrue(stored?.isFile == true)
            val atRest = stored!!.readBytes()
            assertTrue(!atRest.toString(Charsets.UTF_8).contains(plaintext.toString(Charsets.UTF_8)))

            val restored = AndroidSecretStore.read(context, target)
            assertEquals(1, restored.first().toInt())
            assertArrayEquals(plaintext, restored.copyOfRange(1, restored.size))

            val envelope = JSONObject(stored.readText())
            envelope.put("target", "tampered-aad-binding")
            stored.writeText(envelope.toString())
            assertEquals(2, AndroidSecretStore.read(context, target).first().toInt())

            assertEquals(0, AndroidSecretStore.delete(context, target))
            assertEquals(0, AndroidSecretStore.read(context, target).first().toInt())
        } finally {
            AndroidSecretStore.delete(context, target)
            plaintext.fill(0)
        }
    }
}
