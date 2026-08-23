use aes_gcm::aead::{Aead, KeyInit};
#[cfg(test)]
use aes_gcm::aead::{AeadCore, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose, Engine as _};
#[cfg(not(target_os = "android"))]
use sha2::{Digest, Sha256};

fn get_derived_key() -> Result<Key<Aes256Gcm>, String> {
    // Android does not expose a stable machine UID to Rust.  Deliberately
    // refuse legacy machine-bound migration there: credentials must be
    // entered again and then stored by the Android Keystore adapter.
    #[cfg(target_os = "android")]
    return Err("credential_reentry_required".to_string());

    #[cfg(not(target_os = "android"))]
    let uid = machine_uid::get().map_err(|e| format!("Failed to get machine UID: {}", e))?;

    #[cfg(not(target_os = "android"))]
    {
        // 使用 SHA-256 对 UID 散列，生成 32 字节的密钥
        let mut hasher = Sha256::new();
        hasher.update(uid.as_bytes());
        // 为了增加复杂度，可以在此处混入应用的固定 Salt
        hasher.update(b"WatchTracker::Secure::Salt::2026");
        let result = hasher.finalize();

        Ok(*Key::<Aes256Gcm>::from_slice(&result))
    }
}

#[cfg(test)]
pub fn encrypt(text: &str, _tag: &str) -> Result<String, String> {
    if text.is_empty() {
        return Ok(String::new());
    }

    let key = get_derived_key()?;
    let cipher = Aes256Gcm::new(&key);

    // 生成 12 字节随机 Nonce
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    // 加密
    let ciphertext = cipher
        .encrypt(&nonce, text.as_bytes())
        .map_err(|e| format!("Encryption failed: {:?}", e))?;

    // 组合 Nonce 和 Ciphertext
    let mut payload = nonce.to_vec();
    payload.extend_from_slice(&ciphertext);

    let encoded = general_purpose::STANDARD.encode(&payload);
    Ok(format!("machine_bound:v1:{}", encoded))
}

pub fn decrypt(id: &str) -> Result<String, String> {
    if id.is_empty() {
        return Ok(String::new());
    }

    // 向下兼容旧版便携明文加密
    if let Some(encoded) = id.strip_prefix("portable:v1:") {
        let bytes = general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| error.to_string())?;
        return String::from_utf8(bytes).map_err(|error| error.to_string());
    }

    // 解析新版机器绑定加密
    let encoded = id
        .strip_prefix("machine_bound:v1:")
        .ok_or_else(|| "Unsupported credential format".to_string())?;

    let payload = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| error.to_string())?;

    if payload.len() < 12 {
        return Err("Invalid payload length".to_string());
    }

    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let key = get_derived_key()?;
    let cipher = Aes256Gcm::new(&key);

    let decrypted_bytes = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed (machine mismatch or corrupted data)".to_string())?;

    String::from_utf8(decrypted_bytes).map_err(|error| error.to_string())
}
