use super::{Error, Result};
use jni::{errors::ThrowRuntimeExAndDefault, objects::JClass, Env, EnvUnowned};
use std::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

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
