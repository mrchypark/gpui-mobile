use super::{Error, Result};
use jni::{errors::ThrowRuntimeExAndDefault, objects::JClass, Env, EnvUnowned};
use std::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static SCAN_ERRORS: std::sync::OnceLock<super::state::ScanErrors> = std::sync::OnceLock::new();

pub(super) fn scan_errors() -> futures::channel::mpsc::Receiver<i32> {
    SCAN_ERRORS.get_or_init(Default::default).subscribe()
}

#[no_mangle]
pub extern "system" fn Java_org_gpui_mobile_ble_Ble_nativeScanFailed(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    code: i32,
) {
    SCAN_ERRORS.get_or_init(Default::default).send(code);
}

pub(super) fn adapter_state() -> Result<super::CentralState> {
    ensure_initialized()?;
    jni::JavaVM::singleton()?.attach_current_thread(|env| -> Result<_> {
        let adapter = env
            .call_static_method(
                jni::jni_str!("android/bluetooth/BluetoothAdapter"),
                jni::jni_str!("getDefaultAdapter"),
                jni::jni_sig!("()Landroid/bluetooth/BluetoothAdapter;"),
                &[],
            )?
            .l()?;
        if adapter.is_null() {
            return Err(Error::NoAdapterAvailable);
        }
        let enabled = env.call_method(
            &adapter,
            jni::jni_str!("isEnabled"),
            jni::jni_sig!("()Z"),
            &[],
        );
        match enabled {
            Ok(value) => Ok(if value.z()? {
                super::CentralState::PoweredOn
            } else {
                super::CentralState::PoweredOff
            }),
            Err(error) => {
                // Permission revocation is a native error, never a fabricated radio state.
                env.exception_clear();
                Err(error.into())
            }
        }
    })
}

/// Invoke from Java or with the application's class loader on the calling thread.
pub fn init_android(env: &mut Env<'_>) -> Result<()> {
    btleplug::platform::init(env)?;
    INITIALIZED.store(true, Ordering::Release);
    Ok(())
}

pub(super) fn ensure_initialized() -> Result<()> {
    if INITIALIZED.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(Error::RuntimeError(
            "call org.gpui.mobile.ble.Ble.initialize() first".into(),
        ))
    }
}

#[no_mangle]
pub extern "system" fn Java_org_gpui_mobile_ble_Ble_initialize<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        if let Err(error) = init_android(env) {
            env.throw(format!("BLE initialization failed: {error}"))?;
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
