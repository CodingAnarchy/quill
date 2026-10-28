// swift-tools-version:6.0
// QuillKit: the iPhone app's Swift layer over Quill's Rust core (#1230).
//
//   QuillFFI   the core as a static library (ios/build/QuillFFI.xcframework)
//   QuillCore  the Swift bindings UniFFI generates for it
//   QuillKit   the app's model layer, which the SwiftUI views observe
//
// QuillFFI and QuillCore's source are build products: run ios/build-core.sh
// (or `ios/build-core.sh --host` for `swift test` on a Mac without Xcode)
// before building this package.
import PackageDescription

let package = Package(
    name: "QuillKit",
    platforms: [.iOS(.v18), .macOS(.v14)],
    products: [
        .library(name: "QuillKit", targets: ["QuillKit"]),
    ],
    targets: [
        .binaryTarget(name: "QuillFFI", path: "../build/QuillFFI.xcframework"),
        .target(
            name: "QuillCore",
            dependencies: ["QuillFFI"],
            // What the Rust core links from the system: the keychain and
            // network stack (Security, SystemConfiguration), zlib, and iconv.
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
                .linkedLibrary("z"),
                .linkedLibrary("iconv"),
            ]
        ),
        .target(name: "QuillKit", dependencies: ["QuillCore"]),
        .testTarget(name: "QuillKitTests", dependencies: ["QuillKit"]),
    ],
    // The generated bindings are written for Swift 5's concurrency checking.
    swiftLanguageModes: [.v5]
)
