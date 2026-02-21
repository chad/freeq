#!/bin/bash
set -euo pipefail

# Build the Rust SDK for iOS targets and generate Swift bindings.
# Run from the repo root: ./freeq-ios/build-rust.sh

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer

echo "==> Building for iOS device (aarch64-apple-ios)..."
cargo build -p freeq-sdk-ffi --lib --release --target aarch64-apple-ios

echo "==> Building for iOS simulator (aarch64-apple-ios-sim)..."
cargo build -p freeq-sdk-ffi --lib --release --target aarch64-apple-ios-sim

echo "==> Building host binary for bindgen..."
cargo build -p freeq-sdk-ffi --lib --release
cargo build -p freeq-sdk-ffi --bin uniffi-bindgen

echo "==> Generating Swift bindings..."
cargo run -p freeq-sdk-ffi --bin uniffi-bindgen -- generate \
    --library target/release/libfreeq_sdk_ffi.dylib \
    --language swift \
    --out-dir freeq-ios/Generated

echo "==> Assembling xcframework..."
rm -rf freeq-ios/FreeqSDK.xcframework
mkdir -p freeq-ios/FreeqSDK.xcframework/ios-arm64/Headers
mkdir -p freeq-ios/FreeqSDK.xcframework/ios-arm64_x86_64-simulator/Headers

# Headers
cp freeq-ios/Generated/freeqFFI.h freeq-ios/FreeqSDK.xcframework/ios-arm64/Headers/
cp freeq-ios/Generated/freeqFFI.modulemap freeq-ios/FreeqSDK.xcframework/ios-arm64/Headers/module.modulemap
cp freeq-ios/Generated/freeqFFI.h freeq-ios/FreeqSDK.xcframework/ios-arm64_x86_64-simulator/Headers/
cp freeq-ios/Generated/freeqFFI.modulemap freeq-ios/FreeqSDK.xcframework/ios-arm64_x86_64-simulator/Headers/module.modulemap

# Static libs
cp target/aarch64-apple-ios/release/libfreeq_sdk_ffi.a freeq-ios/FreeqSDK.xcframework/ios-arm64/
cp target/aarch64-apple-ios-sim/release/libfreeq_sdk_ffi.a freeq-ios/FreeqSDK.xcframework/ios-arm64_x86_64-simulator/

# Info.plist
cat > freeq-ios/FreeqSDK.xcframework/Info.plist << 'EOF'
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
			<string>ios-arm64</string>
			<key>LibraryPath</key>
			<string>libfreeq_sdk_ffi.a</string>
			<key>SupportedArchitectures</key>
			<array>
				<string>arm64</string>
			</array>
			<key>SupportedPlatform</key>
			<string>ios</string>
		</dict>
		<dict>
			<key>HeadersPath</key>
			<string>Headers</string>
			<key>LibraryIdentifier</key>
			<string>ios-arm64_x86_64-simulator</string>
			<key>LibraryPath</key>
			<string>libfreeq_sdk_ffi.a</string>
			<key>SupportedArchitectures</key>
			<array>
				<string>arm64</string>
			</array>
			<key>SupportedPlatform</key>
			<string>ios</string>
			<key>SupportedPlatformVariant</key>
			<string>simulator</string>
		</dict>
	</array>
	<key>CFBundlePackageType</key>
	<string>XFWK</string>
	<key>XCFrameworkFormatVersion</key>
	<string>1.0</string>
</dict>
</plist>
EOF

echo "==> Done! xcframework at freeq-ios/FreeqSDK.xcframework"
echo "    Swift bindings at freeq-ios/Generated/freeq.swift"
