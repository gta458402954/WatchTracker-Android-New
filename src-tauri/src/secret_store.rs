use crate::db_atomic_helpers::{get_setting_tx, set_setting_tx};
use crate::error::AppError;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const WINDOWS_SECRET_REFERENCE: &str = "wincred:v1";
pub const ANDROID_SECRET_REFERENCE: &str = "androidkeystore:v1";
// Kept as a compatibility name for existing Windows-focused tests.
#[cfg(test)]
pub const SECRET_REFERENCE: &str = WINDOWS_SECRET_REFERENCE;
const MIGRATION_JOURNAL_KEY: &str = "credential_migration_v1";
const SECURITY_STATE_KEY: &str = "credential_security_state_v1";
const MAX_SECRET_BYTES: usize = 2048;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MigrationJournal {
    version: u8,
    logical_id: String,
    source_format: &'static str,
    source_fingerprint: String,
    phase: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SecurityState {
    version: u8,
    protected_by: String,
    history_warning_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalSecret {
    WebDav(String),
    Tmdb,
}

impl LogicalSecret {
    pub fn logical_id(&self) -> String {
        match self {
            Self::WebDav(id) => format!("webdav/{id}"),
            Self::Tmdb => "tmdb/default".to_string(),
        }
    }

    pub fn target_name(&self) -> String {
        format!("WatchTracker/v1/{}", self.logical_id())
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::WebDav(_) => "webdav",
            Self::Tmdb => "tmdb",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecretEnvelope {
    version: u8,
    kind: String,
    logical_id: String,
    secret: String,
}

pub trait SecretStore {
    fn write_raw(&self, target_name: &str, username: &str, value: &[u8]) -> Result<(), String>;
    fn read_raw(&self, target_name: &str) -> Result<Option<Vec<u8>>, String>;
    fn delete_raw(&self, target_name: &str) -> Result<(), String>;
}

pub fn write_secret(
    store: &impl SecretStore,
    logical: &LogicalSecret,
    username: &str,
    secret: &str,
) -> Result<(), String> {
    if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
        return Err("credential_invalid_length".into());
    }
    let envelope = SecretEnvelope {
        version: 1,
        kind: logical.kind().into(),
        logical_id: logical.logical_id(),
        secret: secret.into(),
    };
    let bytes =
        Zeroizing::new(serde_json::to_vec(&envelope).map_err(|_| "credential_store_unavailable")?);
    store.write_raw(&logical.target_name(), username, &bytes)?;
    let verified = read_secret(store, logical)?.ok_or("credential_write_unverified")?;
    if verified.as_str() != secret {
        return Err("credential_write_unverified".into());
    }
    Ok(())
}

pub fn read_secret(
    store: &impl SecretStore,
    logical: &LogicalSecret,
) -> Result<Option<Zeroizing<String>>, String> {
    let Some(raw) = store.read_raw(&logical.target_name())? else {
        return Ok(None);
    };
    let raw = Zeroizing::new(raw);
    let envelope: SecretEnvelope =
        serde_json::from_slice(&raw).map_err(|_| "credential_legacy_corrupt")?;
    if envelope.version != 1
        || envelope.kind != logical.kind()
        || envelope.logical_id != logical.logical_id()
        || envelope.secret.is_empty()
        || envelope.secret.len() > MAX_SECRET_BYTES
    {
        return Err("credential_legacy_corrupt".into());
    }
    Ok(Some(Zeroizing::new(envelope.secret)))
}

pub fn delete_secret(store: &impl SecretStore, logical: &LogicalSecret) -> Result<(), String> {
    store.delete_raw(&logical.target_name())
}

fn app_error(code: impl Into<String>) -> AppError {
    AppError::General(code.into())
}

fn platform_secret_reference() -> &'static str {
    #[cfg(target_os = "android")]
    return ANDROID_SECRET_REFERENCE;
    #[cfg(not(target_os = "android"))]
    WINDOWS_SECRET_REFERENCE
}

fn platform_protection_name() -> &'static str {
    #[cfg(target_os = "android")]
    return "android-keystore-aes-gcm";
    #[cfg(not(target_os = "android"))]
    "windows-credential-manager"
}

#[derive(Debug, PartialEq, Eq)]
enum SecretReferenceKind {
    Current,
    ForeignPlatform,
    Legacy,
}

fn classify_secret_reference(value: &str, current: &str) -> SecretReferenceKind {
    if value == current {
        SecretReferenceKind::Current
    } else if value == WINDOWS_SECRET_REFERENCE || value == ANDROID_SECRET_REFERENCE {
        SecretReferenceKind::ForeignPlatform
    } else {
        SecretReferenceKind::Legacy
    }
}

fn source_format(value: &str) -> &'static str {
    if value.starts_with("portable:v1:") {
        "portable:v1"
    } else if value.starts_with("machine_bound:v1:") {
        "machine_bound:v1"
    } else {
        "unknown"
    }
}

fn write_journal(
    conn: &Connection,
    logical: &LogicalSecret,
    source: &str,
    phase: &'static str,
) -> Result<(), AppError> {
    let journal = MigrationJournal {
        version: 1,
        logical_id: logical.logical_id(),
        source_format: source_format(source),
        source_fingerprint: format!("{:x}", Sha256::digest(source.as_bytes())),
        phase,
    };
    let value = serde_json::to_string(&journal)
        .map_err(|_| app_error("credential_migration_journal_invalid"))?;
    set_setting_tx(conn, MIGRATION_JOURNAL_KEY, &value)?;
    Ok(())
}

fn finish_migration(conn: &Connection) -> Result<(), AppError> {
    let state = serde_json::to_string(&SecurityState {
        version: 1,
        protected_by: platform_protection_name().to_string(),
        history_warning_required: true,
    })
    .map_err(|_| app_error("credential_security_state_invalid"))?;
    set_setting_tx(conn, SECURITY_STATE_KEY, &state)?;
    set_setting_tx(conn, MIGRATION_JOURNAL_KEY, "")?;
    Ok(())
}

pub fn resolve_or_migrate<F>(
    conn: &Connection,
    key: &str,
    logical: &LogicalSecret,
    username: &str,
    extract: F,
) -> Result<Option<Zeroizing<String>>, AppError>
where
    F: FnOnce(&str) -> Result<String, AppError>,
{
    resolve_or_migrate_with_store(
        &PlatformSecretStore,
        conn,
        key,
        logical,
        username,
        extract,
        platform_secret_reference(),
    )
}

fn resolve_or_migrate_with_store<F>(
    store: &impl SecretStore,
    conn: &Connection,
    key: &str,
    logical: &LogicalSecret,
    username: &str,
    extract: F,
    secret_reference: &str,
) -> Result<Option<Zeroizing<String>>, AppError>
where
    F: FnOnce(&str) -> Result<String, AppError>,
{
    let Some(value) = get_setting_tx(conn, key)?.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    match classify_secret_reference(&value, secret_reference) {
        SecretReferenceKind::Current => {
            let secret = read_secret(store, logical)
                .map_err(app_error)?
                .ok_or_else(|| app_error("credential_missing"))?;
            if get_setting_tx(conn, MIGRATION_JOURNAL_KEY)?.is_some_and(|value| !value.is_empty()) {
                finish_migration(conn)?;
            }
            return Ok(Some(secret));
        }
        SecretReferenceKind::ForeignPlatform => {
            return Err(app_error("credential_reentry_required"));
        }
        SecretReferenceKind::Legacy => {}
    }
    write_journal(conn, logical, &value, "planned")?;
    let decrypted = crate::auth::decrypt(&value).map_err(|_| {
        app_error(if value.starts_with("machine_bound:v1:") {
            "credential_reentry_required"
        } else {
            "credential_legacy_corrupt"
        })
    })?;
    let decrypted = Zeroizing::new(decrypted);
    let secret = Zeroizing::new(extract(&decrypted)?);
    write_secret(store, logical, username, &secret).map_err(app_error)?;
    write_journal(conn, logical, &value, "vault_verified")?;
    // The old value is replaced only after a verified vault round-trip.
    set_setting_tx(conn, key, secret_reference)?;
    finish_migration(conn)?;
    Ok(Some(secret))
}

pub fn save_setting_secret(
    conn: &Connection,
    key: &str,
    logical: &LogicalSecret,
    username: &str,
    secret: &str,
) -> Result<(), AppError> {
    let store = PlatformSecretStore;
    write_secret(&store, logical, username, secret).map_err(app_error)?;
    set_setting_tx(conn, key, platform_secret_reference())?;
    Ok(())
}

pub fn clear_setting_secret(
    conn: &Connection,
    key: &str,
    logical: &LogicalSecret,
) -> Result<(), AppError> {
    let store = PlatformSecretStore;
    delete_secret(&store, logical).map_err(app_error)?;
    set_setting_tx(conn, key, "")?;
    Ok(())
}

#[cfg(windows)]
pub struct PlatformSecretStore;

#[cfg(windows)]
impl SecretStore for PlatformSecretStore {
    fn write_raw(&self, target_name: &str, username: &str, value: &[u8]) -> Result<(), String> {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Security::Credentials::{
            CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
        };
        let mut target: Vec<u16> = target_name.encode_utf16().chain(Some(0)).collect();
        let mut user: Vec<u16> = username.encode_utf16().chain(Some(0)).collect();
        let mut blob = Zeroizing::new(value.to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: blob
                .len()
                .try_into()
                .map_err(|_| "credential_invalid_length")?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user.as_mut_ptr(),
            ..Default::default()
        };
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            let _code = unsafe { GetLastError() };
            return Err("credential_store_unavailable".into());
        }
        Ok(())
    }

    fn read_raw(&self, target_name: &str) -> Result<Option<Vec<u8>>, String> {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
        use windows_sys::Win32::Security::Credentials::{
            CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
        };
        let target: Vec<u16> = target_name.encode_utf16().chain(Some(0)).collect();
        let mut pointer: *mut CREDENTIALW = std::ptr::null_mut();
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pointer) } == 0 {
            let code = unsafe { GetLastError() };
            return if code == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err("credential_store_unavailable".into())
            };
        }
        if pointer.is_null() {
            return Err("credential_store_unavailable".into());
        }
        let value = unsafe {
            let credential = &*pointer;
            let bytes = std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
            .to_vec();
            CredFree(pointer.cast());
            bytes
        };
        Ok(Some(value))
    }

    fn delete_raw(&self, target_name: &str) -> Result<(), String> {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
        use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
        let target: Vec<u16> = target_name.encode_utf16().chain(Some(0)).collect();
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let code = unsafe { GetLastError() };
            if code != ERROR_NOT_FOUND {
                return Err("credential_store_unavailable".into());
            }
        }
        Ok(())
    }
}

#[cfg(all(not(windows), not(target_os = "android")))]
pub struct PlatformSecretStore;

#[cfg(all(not(windows), not(target_os = "android")))]
impl SecretStore for PlatformSecretStore {
    fn write_raw(&self, _: &str, _: &str, _: &[u8]) -> Result<(), String> {
        Err("credential_store_unsupported".into())
    }
    fn read_raw(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
        Err("credential_store_unsupported".into())
    }
    fn delete_raw(&self, _: &str) -> Result<(), String> {
        Err("credential_store_unsupported".into())
    }
}

#[cfg(target_os = "android")]
pub struct PlatformSecretStore;

#[cfg(target_os = "android")]
impl PlatformSecretStore {
    const JNI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    fn with_env<T>(
        operation: impl FnOnce(&mut jni::JNIEnv<'_>, &jni::objects::JObject<'_>) -> Result<T, String>
            + Send
            + 'static,
    ) -> Result<T, String>
    where
        T: Send + 'static,
    {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let queued = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wry::prelude::dispatch(move |env, activity, _webview| {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    operation(env, activity)
                }))
                .unwrap_or_else(|_| Err("credential_store_unavailable".to_string()));
                if result.is_err() {
                    Self::clear_exception(env);
                }
                let _ = sender.send(result);
            });
        }));
        if queued.is_err() {
            return Err("credential_store_unavailable".to_string());
        }
        receiver
            .recv_timeout(Self::JNI_TIMEOUT)
            .map_err(|_| "credential_store_unavailable".to_string())?
    }

    fn clear_exception(env: &mut jni::JNIEnv<'_>) {
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    }

    fn adapter_class<'local, 'context>(
        env: &mut jni::JNIEnv<'local>,
        context: &jni::objects::JObject<'context>,
    ) -> Result<jni::objects::JClass<'local>, String> {
        use jni::objects::{JClass, JValue};
        // Native async commands may run on a thread whose FindClass loader is
        // not the application loader. Resolve through Context explicitly.
        let loader = env
            .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|value| value.l())
            .map_err(|_| "credential_store_unavailable".to_string())?;
        let name = env
            .new_string("com.watchtracker.android.AndroidSecretStore")
            .map_err(|_| "credential_store_unavailable".to_string())?;
        let class = env
            .call_method(
                loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&name)],
            )
            .and_then(|value| value.l())
            .map_err(|_| "credential_store_unavailable".to_string())?;
        Ok(JClass::from(class))
    }
}

#[cfg(target_os = "android")]
impl SecretStore for PlatformSecretStore {
    fn write_raw(&self, target_name: &str, username: &str, value: &[u8]) -> Result<(), String> {
        use jni::objects::JValue;
        let target_name = target_name.to_string();
        let username = username.to_string();
        let value = Zeroizing::new(value.to_vec());
        Self::with_env(move |env, context| {
            let class = Self::adapter_class(env, context)?;
            let target = env
                .new_string(target_name)
                .map_err(|_| "credential_store_unavailable".to_string())?;
            let username = env
                .new_string(username)
                .map_err(|_| "credential_store_unavailable".to_string())?;
            let bytes = env
                .byte_array_from_slice(&value)
                .map_err(|_| "credential_store_unavailable".to_string())?;
            let result = env.call_static_method(
                class,
                "write",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;[B)I",
                &[
                    JValue::Object(context),
                    JValue::Object(&target),
                    JValue::Object(&username),
                    JValue::Object(&bytes),
                ],
            );
            match result.and_then(|value| value.i()) {
                Ok(0) => Ok(()),
                _ => {
                    Self::clear_exception(env);
                    Err("credential_store_unavailable".into())
                }
            }
        })
    }

    fn read_raw(&self, target_name: &str) -> Result<Option<Vec<u8>>, String> {
        use jni::objects::{JByteArray, JValue};
        let target_name = target_name.to_string();
        Self::with_env(move |env, context| {
            let class = Self::adapter_class(env, context)?;
            let target = env
                .new_string(target_name)
                .map_err(|_| "credential_store_unavailable".to_string())?;
            let result = env.call_static_method(
                class,
                "read",
                "(Landroid/content/Context;Ljava/lang/String;)[B",
                &[JValue::Object(context), JValue::Object(&target)],
            );
            let object = match result.and_then(|value| value.l()) {
                Ok(value) => value,
                Err(_) => {
                    Self::clear_exception(env);
                    return Err("credential_store_unavailable".into());
                }
            };
            let array = JByteArray::from(object);
            let bytes = Zeroizing::new(
                env.convert_byte_array(&array)
                    .map_err(|_| "credential_store_unavailable".to_string())?,
            );
            let zeros = vec![0_i8; bytes.len()];
            let _ = env.set_byte_array_region(&array, 0, &zeros);
            match bytes.split_first() {
                Some((&0, _)) => Ok(None),
                Some((&1, value)) => Ok(Some(value.to_vec())),
                Some((&2, _)) => Err("credential_reentry_required".into()),
                _ => Err("credential_store_unavailable".into()),
            }
        })
    }

    fn delete_raw(&self, target_name: &str) -> Result<(), String> {
        use jni::objects::JValue;
        let target_name = target_name.to_string();
        Self::with_env(move |env, context| {
            let class = Self::adapter_class(env, context)?;
            let target = env
                .new_string(target_name)
                .map_err(|_| "credential_store_unavailable".to_string())?;
            let result = env.call_static_method(
                class,
                "delete",
                "(Landroid/content/Context;Ljava/lang/String;)I",
                &[JValue::Object(context), JValue::Object(&target)],
            );
            match result.and_then(|value| value.i()) {
                Ok(0) => Ok(()),
                _ => {
                    Self::clear_exception(env);
                    Err("credential_store_unavailable".into())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};

    #[derive(Default)]
    struct MemoryStore(RefCell<HashMap<String, Vec<u8>>>);
    impl SecretStore for MemoryStore {
        fn write_raw(&self, target: &str, _: &str, value: &[u8]) -> Result<(), String> {
            self.0.borrow_mut().insert(target.into(), value.into());
            Ok(())
        }
        fn read_raw(&self, target: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.borrow().get(target).cloned())
        }
        fn delete_raw(&self, target: &str) -> Result<(), String> {
            self.0.borrow_mut().remove(target);
            Ok(())
        }
    }

    struct FailingStore;
    impl SecretStore for FailingStore {
        fn write_raw(&self, _: &str, _: &str, _: &[u8]) -> Result<(), String> {
            Err("credential_store_unavailable".into())
        }
        fn read_raw(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn delete_raw(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn database(value: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .unwrap();
        set_setting_tx(&conn, "secret", value).unwrap();
        conn
    }

    #[test]
    fn logical_items_are_isolated_and_envelopes_cannot_be_swapped() {
        let store = MemoryStore::default();
        let a = LogicalSecret::WebDav("a".repeat(64));
        let b = LogicalSecret::WebDav("b".repeat(64));
        write_secret(&store, &a, "user", "secret-a").unwrap();
        write_secret(&store, &b, "user", "secret-b").unwrap();
        assert_eq!(
            read_secret(&store, &a).unwrap().unwrap().as_str(),
            "secret-a"
        );
        let copied = store.0.borrow().get(&a.target_name()).unwrap().clone();
        store.0.borrow_mut().insert(b.target_name(), copied);
        assert_eq!(
            read_secret(&store, &b).unwrap_err(),
            "credential_legacy_corrupt"
        );
    }

    #[test]
    fn deleting_one_logical_secret_does_not_affect_another() {
        let store = MemoryStore::default();
        let webdav = LogicalSecret::WebDav("c".repeat(64));
        let tmdb = LogicalSecret::Tmdb;
        write_secret(&store, &webdav, "user", "webdav-secret").unwrap();
        write_secret(&store, &tmdb, "tmdb", "tmdb-secret").unwrap();
        delete_secret(&store, &webdav).unwrap();
        assert!(read_secret(&store, &webdav).unwrap().is_none());
        assert_eq!(
            read_secret(&store, &tmdb).unwrap().unwrap().as_str(),
            "tmdb-secret"
        );
    }

    #[test]
    fn portable_legacy_value_is_verified_before_database_switch() {
        let source = crate::auth::encrypt("legacy-secret", "test").unwrap();
        let conn = database(&source);
        let store = MemoryStore::default();
        let logical = LogicalSecret::Tmdb;
        let value = resolve_or_migrate_with_store(
            &store,
            &conn,
            "secret",
            &logical,
            "tmdb",
            |value| Ok(value.to_string()),
            SECRET_REFERENCE,
        )
        .unwrap()
        .unwrap();
        assert_eq!(value.as_str(), "legacy-secret");
        assert_eq!(
            get_setting_tx(&conn, "secret").unwrap().as_deref(),
            Some(SECRET_REFERENCE)
        );
        assert_eq!(
            get_setting_tx(&conn, MIGRATION_JOURNAL_KEY)
                .unwrap()
                .as_deref(),
            Some("")
        );
        assert!(!get_setting_tx(&conn, SECURITY_STATE_KEY)
            .unwrap()
            .unwrap()
            .contains("legacy-secret"));
    }

    #[test]
    fn failed_vault_write_preserves_source_and_journal_contains_no_secret() {
        let source = crate::auth::encrypt("never-log-this", "test").unwrap();
        let conn = database(&source);
        let error = resolve_or_migrate_with_store(
            &FailingStore,
            &conn,
            "secret",
            &LogicalSecret::Tmdb,
            "tmdb",
            |value| Ok(value.to_string()),
            SECRET_REFERENCE,
        )
        .unwrap_err();
        assert!(error.to_string().contains("credential_store_unavailable"));
        assert_eq!(
            get_setting_tx(&conn, "secret").unwrap().as_deref(),
            Some(source.as_str())
        );
        let journal = get_setting_tx(&conn, MIGRATION_JOURNAL_KEY)
            .unwrap()
            .unwrap();
        assert!(journal.contains("planned"));
        assert!(!journal.contains("never-log-this"));
        assert!(!journal.contains(&source));
    }

    #[test]
    fn referenced_missing_secret_never_falls_back() {
        let conn = database(SECRET_REFERENCE);
        let error = resolve_or_migrate_with_store(
            &MemoryStore::default(),
            &conn,
            "secret",
            &LogicalSecret::Tmdb,
            "tmdb",
            |value| Ok(value.to_string()),
            SECRET_REFERENCE,
        )
        .unwrap_err();
        assert!(error.to_string().contains("credential_missing"));
        assert_eq!(
            get_setting_tx(&conn, "secret").unwrap().as_deref(),
            Some(SECRET_REFERENCE)
        );
    }

    #[test]
    fn platform_references_are_explicit_and_windows_reference_remains_compatible() {
        assert_eq!(
            classify_secret_reference(WINDOWS_SECRET_REFERENCE, WINDOWS_SECRET_REFERENCE),
            SecretReferenceKind::Current
        );
        assert_eq!(
            classify_secret_reference(ANDROID_SECRET_REFERENCE, WINDOWS_SECRET_REFERENCE),
            SecretReferenceKind::ForeignPlatform
        );
        assert_eq!(
            classify_secret_reference(WINDOWS_SECRET_REFERENCE, ANDROID_SECRET_REFERENCE),
            SecretReferenceKind::ForeignPlatform
        );
        assert_eq!(
            classify_secret_reference("machine_bound:v1:value", ANDROID_SECRET_REFERENCE),
            SecretReferenceKind::Legacy
        );
    }
}
