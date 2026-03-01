//! FFI error codes returned by all `freeq_win_*` functions.

/// Error codes for the C ABI surface.
///
/// Every `freeq_win_*` function that returns `i32` uses these values.
/// C# consumers should check for `Ok` (0) and handle errors accordingly.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiResult {
    /// Success.
    Ok = 0,
    /// The handle does not exist in the global handle table.
    InvalidHandle = 1,
    /// A required argument was null or not valid UTF-8.
    InvalidArgument = 2,
    /// The client is not connected (no active SDK handle).
    NotConnected = 3,
    /// An internal error occurred (logged via tracing).
    Internal = 4,
}
