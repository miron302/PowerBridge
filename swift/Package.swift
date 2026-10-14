// swift-tools-version:5.7
import PackageDescription
import Foundation

let repoRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path
// scripts/bundle.sh merges the per-architecture Rust libraries into target/universal (Apple silicon + Intel).
let universalDir = repoRoot + "/target/universal"
let rustLibDir = FileManager.default.fileExists(atPath: universalDir + "/libppcx_ffi.a")
    ? universalDir
    : repoRoot + "/target/release"

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
