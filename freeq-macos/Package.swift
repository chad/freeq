// swift-tools-version: 5.10
// This is a helper for building/testing outside Xcode.
// The actual app should be opened as an Xcode project.

import PackageDescription

let package = Package(
    name: "freeq-macos",
    platforms: [.macOS(.v14)],
    products: [],
    targets: []
)
