#!/bin/bash
set -euo pipefail

# Build the Rust SDK for Android targets and generate Kotlin bindings.
# Run from the repo root: ./freeq-android/build-rust.sh
#
# Prerequisites:
#   - Android NDK installed (set ANDROID_NDK_HOME or use default path)
#   - cargo-ndk: cargo install cargo-ndk
#   - Rust Android targets: rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

JNILIBS_DIR="freeq-android/freeq/src/main/jniLibs"

echo "==> Building for Android targets..."
cargo ndk \
    -t arm64-v8a \
    -t armeabi-v7a \
    -t x86_64 \
    -t x86 \
    -o "$JNILIBS_DIR" \
    build -p freeq-sdk-ffi --lib --release

echo "==> Building host binary for bindgen..."
cargo build -p freeq-sdk-ffi --lib --release
cargo build -p freeq-sdk-ffi --bin uniffi-bindgen

echo "==> Generating Kotlin bindings..."
cargo run -p freeq-sdk-ffi --bin uniffi-bindgen -- generate \
    --library target/release/libfreeq_sdk_ffi.dylib \
    --language kotlin \
    --out-dir freeq-android/Generated

echo "==> Done!"
echo "    Native libs at $JNILIBS_DIR"
echo "    Kotlin bindings at freeq-android/Generated/"
echo ""
echo "Next steps:"
echo "  1. Copy Generated/*.kt into freeq/src/main/java/com/freeq/ffi/"
echo "     (replacing the stub files)"
echo "  2. Add JNA dependency to freeq/build.gradle.kts:"
echo "     implementation(\"net.java.dev.jna:jna:5.13.0@aar\")"
