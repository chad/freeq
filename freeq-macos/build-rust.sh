#!/bin/bash
set -euo pipefail

# Build the Rust SDK for macOS (arm64) WITH the AV (voice/video) feature and
# generate Swift bindings + an xcframework. Mirrors freeq-ios/build-rust.sh.
# Run from anywhere: ./freeq-macos/build-rust.sh

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.cargo/bin:$PATH"
if rustc --print sysroot 2>/dev/null | grep -q Cellar; then
    for tc in "$HOME/.rustup"/toolchains/stable-*; do
        if [ -x "$tc/bin/rustc" ]; then
            export RUSTC="$tc/bin/rustc"
            echo "==> Overriding Homebrew rustc with: $RUSTC"
            break
        fi
    done
fi
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer

FEATURES="--features av"

echo "==> Building for macOS (aarch64-apple-darwin) WITH AV..."
MACOSX_DEPLOYMENT_TARGET=14.0 cargo rustc -p freeq-sdk-ffi $FEATURES --release \
    --target aarch64-apple-darwin --lib --crate-type staticlib

echo "==> Building host dylib for bindgen (AV interface is proc-macro defined)..."
cargo build -p freeq-sdk-ffi $FEATURES --lib --release
cargo build -p freeq-sdk-ffi --bin uniffi-bindgen

echo "==> Generating Swift bindings from the compiled library..."
cargo run -p freeq-sdk-ffi --bin uniffi-bindgen -- generate \
    --library target/release/libfreeq_sdk_ffi.dylib \
    --language swift \
    --out-dir freeq-macos/Generated

echo "==> Assembling xcframework..."
rm -rf freeq-macos/FreeqSDK.xcframework
mkdir -p freeq-macos/FreeqSDK.xcframework/macos-arm64/Headers

cp freeq-macos/Generated/freeqFFI.h freeq-macos/FreeqSDK.xcframework/macos-arm64/Headers/
cp freeq-macos/Generated/freeqFFI.modulemap freeq-macos/FreeqSDK.xcframework/macos-arm64/Headers/module.modulemap
cp target/aarch64-apple-darwin/release/libfreeq_sdk_ffi.a freeq-macos/FreeqSDK.xcframework/macos-arm64/

# Keep the legacy Libraries/ copy in sync (some setups link it directly).
cp target/aarch64-apple-darwin/release/libfreeq_sdk_ffi.a freeq-macos/Libraries/ 2>/dev/null || true

cat > freeq-macos/FreeqSDK.xcframework/Info.plist << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AvailableLibraries</key>
	<array>
		<dict>
			<key>HeadersPath</key>
			<string>Headers</string>
			<key>LibraryIdentifier</key>
			<string>macos-arm64</string>
			<key>LibraryPath</key>
			<string>libfreeq_sdk_ffi.a</string>
			<key>SupportedArchitectures</key>
			<array>
				<string>arm64</string>
			</array>
			<key>SupportedPlatform</key>
			<string>macos</string>
		</dict>
	</array>
	<key>CFBundlePackageType</key>
	<string>XFWK</string>
	<key>XCFrameworkFormatVersion</key>
	<string>1.0</string>
</dict>
</plist>
EOF

echo "==> Done! xcframework at freeq-macos/FreeqSDK.xcframework"
echo "    Swift bindings at freeq-macos/Generated/freeq.swift"
