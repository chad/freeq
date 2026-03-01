//! Safe wrapper around a C function pointer for event dispatch.

use std::ffi::{c_char, c_void, CString};

/// C callback signature: receives a UTF-8 JSON string (pointer + length) and opaque user data.
pub type EventCallback =
    unsafe extern "C" fn(json_ptr: *const c_char, json_len: usize, user_data: *mut c_void);

/// Wraps a C event callback with its user_data pointer.
///
/// The C# side is responsible for ensuring the callback and user_data remain valid
/// for the lifetime of the subscription.
pub struct CallbackSink {
    cb: EventCallback,
    user_data: *mut c_void,
}

// Safety: The C# consumer guarantees thread-safe access to user_data.
// The callback may be invoked from the tokio event-pump thread.
unsafe impl Send for CallbackSink {}
unsafe impl Sync for CallbackSink {}

impl CallbackSink {
    /// Create a new callback sink from a C function pointer and user data.
    pub fn new(cb: EventCallback, user_data: *mut c_void) -> Self {
        Self { cb, user_data }
    }

    /// Dispatch a JSON string to the C callback.
    ///
    /// Converts the Rust string into a CString and invokes the callback.
    /// If the string contains interior NUL bytes, the event is silently dropped
    /// (this should never happen with well-formed JSON).
    pub fn dispatch(&self, json: &str) {
        let Ok(cstr) = CString::new(json) else {
            tracing::warn!("event JSON contained interior NUL byte, dropping");
            return;
        };
        let ptr = cstr.as_ptr();
        let len = json.len();
        unsafe {
            (self.cb)(ptr, len, self.user_data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
    static LAST_LEN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn test_cb(_ptr: *const c_char, len: usize, _user_data: *mut c_void) {
        CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        LAST_LEN.store(len, Ordering::SeqCst);
    }

    #[test]
    fn test_callback_dispatch() {
        CALL_COUNT.store(0, Ordering::SeqCst);
        LAST_LEN.store(0, Ordering::SeqCst);

        let sink = CallbackSink::new(test_cb, std::ptr::null_mut());
        sink.dispatch(r#"{"type":"connected"}"#);

        assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(LAST_LEN.load(Ordering::SeqCst), 20);
    }
}
