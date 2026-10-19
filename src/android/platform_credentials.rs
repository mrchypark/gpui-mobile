//! Private, non-backed-up credential records authenticated by an Android Keystore key.

use crate::android::jni as bridge;
use anyhow::{anyhow, bail, Result};
use jni::{
    objects::{JByteArray, JObject, JValue},
    Env,
};
use std::{
    fs,
    io::Write,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
};

static STORE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
const KEY_ALIAS: &str = "dev.gpui.platform.credentials.v1";
const HEADER: &[u8; 4] = b"GPC1";

fn with_env<T>(f: impl FnOnce(&mut Env) -> Result<T>) -> Result<T> {
    bridge::with_env(|env| {
        let result = f(env).map_err(|error| error.to_string());
        if result.is_err() {
            let _ = env.exception_clear();
        }
        result
    })
    .map_err(anyhow::Error::msg)
}

fn bytes(env: &Env, value: JObject) -> Result<Vec<u8>> {
    if value.is_null() {
        bail!("Keystore returned no bytes");
    }
    let array = unsafe { JByteArray::from_raw(env, value.into_raw()) };
    Ok(env.convert_byte_array(&array)?)
}

fn path(env: &mut Env, service: &str) -> Result<PathBuf> {
    let activity = bridge::activity(env).map_err(anyhow::Error::msg)?;
    let directory = env
        .call_method(
            &activity,
            jni::jni_str!("getNoBackupFilesDir"),
            jni::jni_sig!("()Ljava/io/File;"),
            &[],
        )?
        .l()?;
    if directory.is_null() {
        bail!("Private credential directory unavailable");
    }
    let directory = env
        .call_method(
            &directory,
            jni::jni_str!("getAbsolutePath"),
            jni::jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()?;
    let directory = bridge::get_string(env, &directory);
    if directory.is_empty() {
        bail!("Private credential path unavailable");
    }
    let algorithm = env.new_string("SHA-256")?;
    let digest = env
        .call_static_method(
            jni::jni_str!("java/security/MessageDigest"),
            jni::jni_str!("getInstance"),
            jni::jni_sig!("(Ljava/lang/String;)Ljava/security/MessageDigest;"),
            &[JValue::Object(&algorithm)],
        )?
        .l()?;
    let input = env.byte_array_from_slice(service.as_bytes())?;
    let hash = env
        .call_method(
            &digest,
            jni::jni_str!("digest"),
            jni::jni_sig!("([B)[B"),
            &[JValue::Object(&input)],
        )?
        .l()?;
    let filename: String = bytes(env, hash)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(PathBuf::from(directory)
        .join("gpui-credentials-v1")
        .join(filename))
}

fn key<'local>(env: &mut Env<'local>, create: bool) -> Result<JObject<'local>> {
    let provider = env.new_string("AndroidKeyStore")?;
    let store = env
        .call_static_method(
            jni::jni_str!("java/security/KeyStore"),
            jni::jni_str!("getInstance"),
            jni::jni_sig!("(Ljava/lang/String;)Ljava/security/KeyStore;"),
            &[JValue::Object(&provider)],
        )?
        .l()?;
    env.call_method(
        &store,
        jni::jni_str!("load"),
        jni::jni_sig!("(Ljava/security/KeyStore$LoadStoreParameter;)V"),
        &[JValue::Object(&JObject::null())],
    )?;
    let alias = env.new_string(KEY_ALIAS)?;
    let key = env
        .call_method(
            &store,
            jni::jni_str!("getKey"),
            jni::jni_sig!("(Ljava/lang/String;[C)Ljava/security/Key;"),
            &[JValue::Object(&alias), JValue::Object(&JObject::null())],
        )?
        .l()?;
    if !key.is_null() {
        return Ok(key);
    }
    if !create {
        bail!("Credential key is missing; encrypted records cannot be recovered");
    }
    let algorithm = env.new_string("AES")?;
    let generator = env
        .call_static_method(
            jni::jni_str!("javax/crypto/KeyGenerator"),
            jni::jni_str!("getInstance"),
            jni::jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;"),
            &[JValue::Object(&algorithm), JValue::Object(&provider)],
        )?
        .l()?;
    let builder = env.new_object(
        jni::jni_str!("android/security/keystore/KeyGenParameterSpec$Builder"),
        jni::jni_sig!("(Ljava/lang/String;I)V"),
        &[JValue::Object(&alias), JValue::Int(3)],
    )?;
    let gcm = env.new_string("GCM")?;
    let modes = env.new_object_array(1, jni::jni_str!("java/lang/String"), &gcm)?;
    env.call_method(
        &builder,
        jni::jni_str!("setBlockModes"),
        jni::jni_sig!(
            "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;"
        ),
        &[JValue::Object(&modes)],
    )?;
    let padding = env.new_string("NoPadding")?;
    let paddings = env.new_object_array(1, jni::jni_str!("java/lang/String"), &padding)?;
    env.call_method(
        &builder,
        jni::jni_str!("setEncryptionPaddings"),
        jni::jni_sig!(
            "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;"
        ),
        &[JValue::Object(&paddings)],
    )?;
    env.call_method(
        &builder,
        jni::jni_str!("setKeySize"),
        jni::jni_sig!("(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;"),
        &[JValue::Int(256)],
    )?;
    let spec = env
        .call_method(
            &builder,
            jni::jni_str!("build"),
            jni::jni_sig!("()Landroid/security/keystore/KeyGenParameterSpec;"),
            &[],
        )?
        .l()?;
    env.call_method(
        &generator,
        jni::jni_str!("init"),
        jni::jni_sig!("(Ljava/security/spec/AlgorithmParameterSpec;)V"),
        &[JValue::Object(&spec)],
    )?;
    Ok(env
        .call_method(
            &generator,
            jni::jni_str!("generateKey"),
            jni::jni_sig!("()Ljavax/crypto/SecretKey;"),
            &[],
        )?
        .l()?)
}

fn crypt(
    env: &mut Env,
    service: &str,
    input: &[u8],
    iv: Option<&[u8]>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let key = key(env, iv.is_none())?;
    let transformation = env.new_string("AES/GCM/NoPadding")?;
    let cipher = env
        .call_static_method(
            jni::jni_str!("javax/crypto/Cipher"),
            jni::jni_str!("getInstance"),
            jni::jni_sig!("(Ljava/lang/String;)Ljavax/crypto/Cipher;"),
            &[JValue::Object(&transformation)],
        )?
        .l()?;
    if let Some(iv) = iv {
        let iv = env.byte_array_from_slice(iv)?;
        let spec = env.new_object(
            jni::jni_str!("javax/crypto/spec/GCMParameterSpec"),
            jni::jni_sig!("(I[B)V"),
            &[JValue::Int(128), JValue::Object(&iv)],
        )?;
        env.call_method(
            &cipher,
            jni::jni_str!("init"),
            jni::jni_sig!("(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V"),
            &[JValue::Int(2), JValue::Object(&key), JValue::Object(&spec)],
        )?;
    } else {
        // AndroidKeyStore generates the nonce; never reuse or supply one for encryption.
        env.call_method(
            &cipher,
            jni::jni_str!("init"),
            jni::jni_sig!("(ILjava/security/Key;)V"),
            &[JValue::Int(1), JValue::Object(&key)],
        )?;
    }
    let aad = env.byte_array_from_slice(service.as_bytes())?;
    env.call_method(
        &cipher,
        jni::jni_str!("updateAAD"),
        jni::jni_sig!("([B)V"),
        &[JValue::Object(&aad)],
    )?;
    let input = env.byte_array_from_slice(input)?;
    let output = env
        .call_method(
            &cipher,
            jni::jni_str!("doFinal"),
            jni::jni_sig!("([B)[B"),
            &[JValue::Object(&input)],
        )?
        .l()?;
    let output = bytes(env, output)?;
    let nonce = env
        .call_method(&cipher, jni::jni_str!("getIV"), jni::jni_sig!("()[B"), &[])?
        .l()?;
    Ok((bytes(env, nonce)?, output))
}

#[path = "../credential_payload.rs"]
mod credential_payload;
use credential_payload::{decode, encode};

fn split_record(record: &[u8]) -> Result<(&[u8], &[u8])> {
    if record.len() < 4 + 12 + 16 || &record[..4] != HEADER {
        bail!("Invalid encrypted credential record");
    }
    Ok((&record[4..16], &record[16..]))
}

fn replace(path: &Path, record: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("Missing credential directory"))?;
    fs::create_dir_all(directory)?;
    let temporary = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(record)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::warn!("Could not remove encrypted credential staging file: {error}"),
        }
    }
    result
}

fn process_lock(path: &Path) -> Result<fs::File> {
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("Missing credential directory"))?;
    fs::create_dir_all(directory)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join(".lock"))?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
}

pub(super) fn write(service: &str, username: &str, password: &[u8]) -> Result<()> {
    let _guard = STORE_LOCK.lock();
    with_env(|env| {
        let path = path(env, service)?;
        // Serialize key creation and record replacement across all app processes.
        let _process_lock = process_lock(&path)?;
        let mut payload = encode(username, password)?;
        let encrypted = crypt(env, service, &payload, None);
        payload.fill(0);
        let (iv, ciphertext) = encrypted?;
        if iv.len() != 12 {
            bail!("Unexpected GCM nonce length");
        }
        let mut record = HEADER.to_vec();
        record.extend(iv);
        record.extend(ciphertext);
        replace(&path, &record)
    })
}

pub(super) fn read(service: &str) -> Result<Option<(String, Vec<u8>)>> {
    let _guard = STORE_LOCK.lock();
    with_env(|env| {
        let _process_lock = process_lock(&path(env, service)?)?;
        read_record(env, service)
    })
}

fn read_record(env: &mut Env, service: &str) -> Result<Option<(String, Vec<u8>)>> {
    let record = match fs::read(path(env, service)?) {
        Ok(record) => record,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let (iv, ciphertext) = split_record(&record)?;
    let (_, mut payload) = crypt(env, service, ciphertext, Some(iv))?;
    let decoded = decode(&payload);
    payload.fill(0);
    Ok(Some(decoded?))
}

pub(super) fn delete(service: &str) -> Result<()> {
    delete_account(service, None)
}

pub(super) fn delete_account(service: &str, account: Option<&str>) -> Result<()> {
    let _guard = STORE_LOCK.lock();
    with_env(|env| {
        let path = path(env, service)?;
        let _process_lock = process_lock(&path)?;
        if let Some(account) = account {
            if !read_record(env, service)?.is_some_and(|(username, _)| username == account) {
                return Ok(());
            }
        }
        remove_record(&path)
    })
}

fn remove_record(path: &Path) -> Result<()> {
    remove_record_and_sync(path, |parent| fs::File::open(parent)?.sync_all())
}

fn remove_record_and_sync(
    path: &Path,
    sync: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync(
            path.parent()
                .ok_or_else(|| anyhow!("Missing credential directory"))?,
        )
        .map_err(Into::into),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_syncs_directory_and_propagates_durability_failure() {
        let dir = std::env::temp_dir().join(format!(
            "gpui-delete-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("record");
        let result = (|| -> Result<()> {
            fs::write(&path, b"ciphertext")?;
            let mut synced = false;
            let failure = remove_record_and_sync(&path, |parent| {
                assert_eq!(parent, dir);
                assert!(!path.exists());
                synced = true;
                Err(std::io::Error::other("injected directory sync failure"))
            });
            assert!(synced);
            assert!(failure.is_err());
            fs::write(&path, b"ciphertext")?;
            remove_record(&path)?;
            assert!(!path.exists());
            remove_record_and_sync(&path, |_| panic!("missing record must not sync"))?;
            Ok(())
        })();
        fs::remove_dir_all(&dir).unwrap();
        result.unwrap();
    }

    #[test]
    fn payload_handles_unicode_nul_empty_password_and_rejects_truncation() {
        let payload = encode("a\0한", b"\0\xff").unwrap();
        assert_eq!(
            decode(&payload).unwrap(),
            ("a\0한".into(), b"\0\xff".to_vec())
        );
        assert_eq!(
            decode(&encode("", b"").unwrap()).unwrap(),
            (String::new(), Vec::new())
        );
        for length in 0..9 {
            assert!(decode(&payload[..length]).is_err());
        }
        assert!(decode(&[255, 255, 255, 255]).is_err());
        assert!(decode(&[0, 0, 0, 1, 255]).is_err());
        assert!(split_record(b"GPC1").is_err());
        assert!(split_record(&[0; 32]).is_err());
    }

    #[test]
    #[ignore = "requires Android Activity, Keystore and private storage"]
    fn native_round_trip_persistence_and_authentication() {
        let service = format!("gpui-test-{:?}", std::time::SystemTime::now());
        let result = (|| -> Result<()> {
            write(&service, "one", b"secret")?;
            if read(&service)? != Some(("one".into(), b"secret".to_vec())) {
                bail!("Credential round trip mismatch");
            }
            write(&service, "two\0한", b"")?;
            if read(&service)? != Some(("two\0한".into(), Vec::new())) {
                bail!("Credential replacement mismatch");
            }
            with_env(|env| {
                let path = path(env, &service)?;
                let mut record = fs::read(&path)?;
                *record.last_mut().unwrap() ^= 1;
                fs::write(path, record)?;
                Ok(())
            })?;
            if read(&service).is_ok() {
                bail!("Tampering must not look like absence");
            }
            Ok(())
        })();
        delete(&service).unwrap();
        result.unwrap();
        assert!(read(&service).unwrap().is_none());
        delete(&service).unwrap();
    }
}
