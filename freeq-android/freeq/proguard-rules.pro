# freeq Android ProGuard rules
# Keep FFI bindings (UniFFI-generated + JNA)
-keep class com.freeq.ffi.** { *; }
-dontwarn com.sun.jna.**
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.Callback { *; }
