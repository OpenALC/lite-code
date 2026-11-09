//! C ABI for the OHOS NAPI bridge (docs/plans/ohos-shell.md §5.3).
//!
//! `lc_ohos_init` returns an opaque handle (a boxed `Host`) or 0 on failure.
//! `lc_ohos_call` is synchronous and is meant to run on a native worker thread;
//! results/errors are heap C strings owned by the caller and freed with
//! `lc_ohos_free`.
//!
//! The event sink callback is invoked on arbitrary Rust worker threads. The
//! envelope pointer is allocated with `CString::into_raw`; the callback must
//! hand it back to `lc_ohos_free` (the C++ bridge copies then frees it) or leak.

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;

use lc_proto::{ErrorBody, ErrorCode};
use serde_json::Value;

use crate::{Host, HostConfig, invalid};

pub type LcEventCallback = Option<unsafe extern "C" fn(json: *const c_char, user: *mut c_void)>;

fn c_string(raw: *const c_char) -> Result<String, ErrorBody> {
    if raw.is_null() {
        return Err(invalid("null C string"));
    }
    // SAFETY: the caller guarantees a NUL-terminated string valid for this call.
    let text = unsafe { CStr::from_ptr(raw) };
    text.to_str()
        .map(str::to_owned)
        .map_err(|error| invalid(format!("C string is not UTF-8: {error}")))
}

fn path_arg(value: &Value, key: &str) -> Result<PathBuf, ErrorBody> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|item| !item.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| invalid(format!("missing string argument `{key}`")))
}

fn config_from_json(raw: &str) -> Result<HostConfig, ErrorBody> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| invalid(format!("invalid config json: {error}")))?;
    if !value.is_object() {
        return Err(invalid("config must be a JSON object"));
    }
    Ok(HostConfig {
        files_dir: path_arg(&value, "files_dir")?,
        workspace_root: path_arg(&value, "workspace_root")?,
        providers_file: path_arg(&value, "providers_file")?,
    })
}

fn error_json(error: &ErrorBody) -> String {
    serde_json::to_string(error).unwrap_or_else(|_| {
        format!(
            "{{\"code\":\"internal\",\"message\":\"serialize ErrorBody failed\",\"detail\":null}}"
        )
    })
}

fn write_out(slot: *mut *mut c_char, text: String) {
    let pointer = CString::new(text)
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut());
    // SAFETY: the caller passes a valid out pointer (checked by `lc_ohos_call`).
    unsafe { *slot = pointer };
}

/// Initializes the host. Returns 0 on failure (details go to stderr/hilog).
///
/// # Safety
/// `config_json` must be a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_ohos_init(config_json: *const c_char) -> u64 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let raw = c_string(config_json)?;
        let config = config_from_json(&raw)?;
        std::fs::create_dir_all(&config.files_dir)
            .map_err(|error| invalid(format!("create files_dir failed: {error}")))?;
        let tmp = config.files_dir.join("tmp");
        let _ = std::fs::create_dir_all(&tmp);
        // SAFETY: called during single-threaded native startup before workers spawn.
        unsafe {
            std::env::set_var("LC_PROVIDERS_FILE", &config.providers_file);
            std::env::set_var("TMPDIR", &tmp);
        }
        Host::init(config)
    }));

    match result {
        Ok(Ok(host)) => Box::into_raw(Box::new(host)) as u64,
        Ok(Err(error)) => {
            eprintln!("lc_ohos_init failed: {}", error_json(&error));
            0
        }
        Err(_) => {
            eprintln!("lc_ohos_init panicked");
            0
        }
    }
}

/// Synchronously dispatches one command. Returns 0 on success (`out_json` set)
/// and 1 on failure (`out_error` holds an `ErrorBody` JSON).
///
/// # Safety
/// `handle` must come from `lc_ohos_init`; `cmd`/`args_json` must be valid
/// NUL-terminated strings; `out_json`/`out_error` must be writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_ohos_call(
    handle: u64,
    cmd: *const c_char,
    args_json: *const c_char,
    out_json: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> i32 {
    if out_json.is_null() || out_error.is_null() {
        return 1;
    }
    // SAFETY: checked above.
    unsafe {
        *out_json = ptr::null_mut();
        *out_error = ptr::null_mut();
    }
    if handle == 0 {
        write_out(
            out_error,
            error_json(&invalid("native host is not initialized")),
        );
        return 1;
    }

    let result = catch_unwind(AssertUnwindSafe(|| -> Result<String, ErrorBody> {
        // SAFETY: handle was produced by `lc_ohos_init` and is still alive.
        let host = unsafe { &*(handle as *const Host) };
        let cmd = c_string(cmd)?;
        let args = c_string(args_json)?;
        host.dispatch(&cmd, &args)
    }));

    match result {
        Ok(Ok(json)) => {
            write_out(out_json, json);
            0
        }
        Ok(Err(error)) => {
            write_out(out_error, error_json(&error));
            1
        }
        Err(_) => {
            write_out(
                out_error,
                error_json(&ErrorBody::new(
                    ErrorCode::Internal,
                    "native dispatch panicked",
                )),
            );
            1
        }
    }
}

/// Registers the event callback. Passing `None` detaches the current sink.
///
/// # Safety
/// `handle` must be a live host handle; `user` is passed back verbatim.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_ohos_set_event_sink(
    handle: u64,
    callback: LcEventCallback,
    user: *mut c_void,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: handle was produced by `lc_ohos_init` and is still alive.
    let host = unsafe { &*(handle as *const Host) };
    let user = user as usize;
    host.set_event_sink(Arc::new(move |json| {
        let Some(callback) = callback else { return };
        let Ok(json) = CString::new(json) else { return };
        // SAFETY: `callback`/`user` were supplied by the bridge and remain valid
        // until `lc_ohos_dispose`; the callee frees the pointer with `lc_ohos_free`.
        unsafe { callback(json.into_raw(), user as *mut c_void) };
    }));
}

/// Frees a string returned through `out_json`/`out_error` or an event envelope.
///
/// # Safety
/// `raw` must be null or a pointer allocated by this module and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_ohos_free(raw: *mut c_char) {
    if raw.is_null() {
        return;
    }
    // SAFETY: contract above.
    drop(unsafe { CString::from_raw(raw) });
}

/// Drops the host. Safe to call with 0 or an already-disposed handle (0 only);
/// callers must not reuse a disposed handle.
///
/// # Safety
/// `handle` must be a live handle returned by `lc_ohos_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_ohos_dispose(handle: u64) {
    if handle == 0 {
        return;
    }
    // SAFETY: handle came from `Box::into_raw` in `lc_ohos_init`.
    drop(unsafe { Box::from_raw(handle as *mut Host) });
}
