#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Building freeq-sdk-ffi for macOS (arm64)..."
cargo build --release --target aarch64-apple-darwin -p freeq-sdk-ffi

echo "==> Copying static library..."
cp target/aarch64-apple-darwin/release/libfreeq_sdk_ffi.a freeq-macos/Libraries/

echo "==> Generating Swift bindings..."
cargo run -p freeq-sdk-ffi --bin uniffi-bindgen -- generate \
    freeq-sdk-ffi/src/freeq.udl \
    --language swift \
    --out-dir freeq-macos/Generated/

echo "==> Done! Open freeq-macos/freeq-macos.xcodeproj in Xcode."
echo "    Or create the project via: File → New → Project → macOS → App"
echo "    Then add all Swift files + Libraries/libfreeq_sdk_ffi.a"
