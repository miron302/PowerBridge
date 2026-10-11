// swift-tools-version:5.7
import PackageDescription
import Foundation

// The Rust static library (libppcx_ffi.a) is built by scripts/bundle.sh into <repo>/target/release.
let repoRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path
let rustLibDir = repoRoot + "/target/release"

let package = Package(
    name: "PowerBridge",
    platforms: [.macOS(.v12)],
    products: [.executable(name: "PowerBridge", targets: ["PowerBridge"])],
    targets: [
        // C header for the Rust core (module map is generated from include/)
        .target(name: "CPPCX", path: "Sources/CPPCX"),
        // Foundation-only Swift wrapper around the C ABI
        .target(name: "PPCXKit", dependencies: ["CPPCX"], path: "Sources/PPCXKit"),
        // SwiftUI app
        .executableTarget(
            name: "PowerBridge",
            dependencies: ["PPCXKit"],
            path: "Sources/PowerBridge",
            linkerSettings: [
                .unsafeFlags(["-L", rustLibDir]),
                .linkedLibrary("ppcx_ffi"),
                .linkedLibrary("iconv"),
            ]
        ),
    ]
)
