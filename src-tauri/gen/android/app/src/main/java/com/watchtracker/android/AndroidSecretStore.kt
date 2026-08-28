package com.watchtracker.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.util.AtomicFile
import android.util.Base64
import androidx.annotation.Keep
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.security.GeneralSecurityException
import java.security.InvalidAlgorithmParameterException
import java.security.InvalidKeyException
import java.security.KeyStoreException
import java.security.KeyStore
import java.security.MessageDigest
import java.security.NoSuchAlgorithmException
import java.security.UnrecoverableKeyException
import java.security.cert.CertificateException
import javax.crypto.AEADBadTagException
import javax.crypto.BadPaddingException
import javax.crypto.Cipher
import javax.crypto.IllegalBlockSizeException
import javax.crypto.KeyGenerator
import javax.crypto.NoSuchPaddingException
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONObject

/**
 * Android-only raw vault used by Rust's SecretStore contract.
 *
 * The Keystore key is non-exportable. Only IV and ciphertext are written to
 * app-private storage, and the logical target name is authenticated as AAD.
 * Status bytes keep expected failures out of exception/log text crossing JNI.
 */
@Keep
object AndroidSecretStore {
    private const val VERSION = 1
    private const val STATUS_MISSING: Byte = 0
    private const val STATUS_OK: Byte = 1
    private const val STATUS_REENTRY_REQUIRED: Byte = 2
    private const val STATUS_UNAVAILABLE: Byte = 3
    private const val MAX_VAULT_BYTES = 16 * 1024

    private fun digest(value: String): String = MessageDigest.getInstance("SHA-256")
        .digest(value.toByteArray(Charsets.UTF_8))
        .joinToString("") { "%02x".format(it) }

    private fun alias(targetName: String) = "WatchTrackerVault.v1.${digest(targetName).take(32)}"

    private fun vaultFile(context: Context, targetName: String): File {
        val directory = File(context.filesDir, "secure-vault-v1")
        if (!directory.exists() && !directory.mkdirs()) {
            throw IllegalStateException("credential_store_unavailable")
        }
        return File(directory, "${digest(targetName)}.json")
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun createKey(targetName: String): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                alias(targetName),
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .build(),
        )
        return generator.generateKey()
    }

    @Keep
    @JvmStatic
    @Synchronized
    fun write(context: Context, targetName: String, username: String, value: ByteArray): Int {
        // Username remains part of the cross-platform store contract, but is
        // deliberately neither persisted nor used as key material on Android.
        @Suppress("UNUSED_VARIABLE") val ignoredUsername = username
        return try {
            val store = keyStore()
            fun replacementKey(): SecretKey {
                if (store.containsAlias(alias(targetName))) store.deleteEntry(alias(targetName))
                return createKey(targetName)
            }
            fun currentKey(): SecretKey = try {
                (store.getKey(alias(targetName), null) as? SecretKey) ?: createKey(targetName)
            } catch (_: UnrecoverableKeyException) {
                replacementKey()
            }
            fun encrypt(key: SecretKey): Pair<ByteArray, ByteArray> {
                val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
                    init(Cipher.ENCRYPT_MODE, key)
                    updateAAD(targetName.toByteArray(Charsets.UTF_8))
                }
                return cipher.iv to cipher.doFinal(value)
            }
            fun encryptedPayload(): Pair<ByteArray, ByteArray> {
                return try {
                    encrypt(currentKey())
                } catch (_: KeyPermanentlyInvalidatedException) {
                    // A user who is explicitly re-entering a credential may
                    // replace an invalidated per-secret key without affecting
                    // any other logical secret.
                    encrypt(replacementKey())
                } catch (_: UnrecoverableKeyException) {
                    encrypt(replacementKey())
                }
            }
            val (iv, ciphertext) = encryptedPayload()
            val payload = JSONObject()
                .put("version", VERSION)
                .put("target", digest(targetName))
                .put("iv", Base64.encodeToString(iv, Base64.NO_WRAP))
                .put("ciphertext", Base64.encodeToString(ciphertext, Base64.NO_WRAP))
                .toString()
                .toByteArray(Charsets.UTF_8)
            val destination = vaultFile(context, targetName)
            val atomic = AtomicFile(destination)
            var output: FileOutputStream? = null
            try {
                output = atomic.startWrite()
                output.write(payload)
                output.fd.sync()
                atomic.finishWrite(output)
                output = null
            } catch (error: Throwable) {
                output?.let { atomic.failWrite(it) }
                throw error
            }
            0
        } catch (_: Throwable) {
            STATUS_UNAVAILABLE.toInt()
        } finally {
            value.fill(0)
        }
    }

    @Keep
    @JvmStatic
    @Synchronized
    fun read(context: Context, targetName: String): ByteArray {
        val file = try { vaultFile(context, targetName) } catch (_: Throwable) {
            return byteArrayOf(STATUS_UNAVAILABLE)
        }
        if (!file.isFile) return byteArrayOf(STATUS_MISSING)
        if (file.length() !in 1..MAX_VAULT_BYTES.toLong()) {
            return byteArrayOf(STATUS_REENTRY_REQUIRED)
        }
        return try {
            val store = keyStore()
            val key = store.getKey(alias(targetName), null) as? SecretKey
                ?: return byteArrayOf(STATUS_REENTRY_REQUIRED)
            val payload = JSONObject(file.readText(Charsets.UTF_8))
            if (payload.getInt("version") != VERSION || payload.getString("target") != digest(targetName)) {
                return byteArrayOf(STATUS_REENTRY_REQUIRED)
            }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(
                    Cipher.DECRYPT_MODE,
                    key,
                    GCMParameterSpec(128, Base64.decode(payload.getString("iv"), Base64.NO_WRAP)),
                )
                updateAAD(targetName.toByteArray(Charsets.UTF_8))
            }
            val plaintext = cipher.doFinal(Base64.decode(payload.getString("ciphertext"), Base64.NO_WRAP))
            val result = ByteArray(plaintext.size + 1)
            result[0] = STATUS_OK
            plaintext.copyInto(result, destinationOffset = 1)
            plaintext.fill(0)
            result
        } catch (_: KeyPermanentlyInvalidatedException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: UnrecoverableKeyException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: AEADBadTagException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: InvalidKeyException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: InvalidAlgorithmParameterException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: BadPaddingException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: IllegalBlockSizeException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: org.json.JSONException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: IllegalArgumentException) {
            byteArrayOf(STATUS_REENTRY_REQUIRED)
        } catch (_: IOException) {
            byteArrayOf(STATUS_UNAVAILABLE)
        } catch (_: KeyStoreException) {
            byteArrayOf(STATUS_UNAVAILABLE)
        } catch (_: NoSuchAlgorithmException) {
            byteArrayOf(STATUS_UNAVAILABLE)
        } catch (_: NoSuchPaddingException) {
            byteArrayOf(STATUS_UNAVAILABLE)
        } catch (_: CertificateException) {
            byteArrayOf(STATUS_UNAVAILABLE)
        } catch (_: GeneralSecurityException) {
            byteArrayOf(STATUS_UNAVAILABLE)
        } catch (_: Throwable) {
            byteArrayOf(STATUS_UNAVAILABLE)
        }
    }

    @Keep
    @JvmStatic
    @Synchronized
    fun delete(context: Context, targetName: String): Int = try {
        val file = vaultFile(context, targetName)
        if (file.exists() && !file.delete()) throw IllegalStateException()
        val store = keyStore()
        if (store.containsAlias(alias(targetName))) store.deleteEntry(alias(targetName))
        0
    } catch (_: Throwable) {
        STATUS_UNAVAILABLE.toInt()
    }
}
