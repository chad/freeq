// swift-tools-version: 5.10
//
// SwiftPM harness for unit-testing the macOS app's pure-Foundation
// helpers without booting Xcode. The actual app target lives in
// freeq-macos.xcodeproj and isn't built here — this package only
// compiles `Models/Validation.swift` so its assertions can run under
// `swift test` from the command line.
//
// Run:  cd freeq-macos && swift test

import PackageDescription

let package = Package(
    name: "freeq-macos",
    platforms: [.macOS("14.4")],
    products: [
        .library(name: "FreeqMacosCore", targets: ["FreeqMacosCore"])
    ],
    targets: [
        // Single-source the validation helpers — the file lives where
        // the app expects it (inside the Xcode group), and we point
        // SwiftPM at the same file. No copy, no drift.
        .target(
            name: "FreeqMacosCore",
            path: "freeq-macos/Models",
            sources: ["Validation.swift"]
        ),
        .testTarget(
            name: "FreeqMacosCoreTests",
            dependencies: ["FreeqMacosCore"],
            path: "Tests/FreeqMacosCoreTests"
        ),
    ]
)
