# freeq Android ProGuard rules
# Keep FFI bindings (when real UniFFI .so is integrated)
-keep class com.freeq.ffi.** { *; }
