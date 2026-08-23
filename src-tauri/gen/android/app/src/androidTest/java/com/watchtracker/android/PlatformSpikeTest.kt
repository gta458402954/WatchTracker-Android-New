package com.watchtracker.android

import android.content.Context
import android.content.Intent
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

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
}
