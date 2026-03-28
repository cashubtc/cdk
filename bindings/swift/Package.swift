// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "cdk-swift",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "Cdk", targets: ["Cdk"]),
    ],
    targets: [
        .systemLibrary(
            name: "cdkFFI",
            path: "cdkFFI"
        ),
        .target(
            name: "Cdk",
            dependencies: ["cdkFFI"],
            path: "Sources/Cdk",
            sources: ["cdk.swift"],
            linkerSettings: [
                .unsafeFlags([
                    "-L", ".build/macos",
                    "-lcdk_ffi_swift",
                ]),
            ]
        ),
        .testTarget(
            name: "CdkTests",
            dependencies: ["Cdk"],
            path: "Tests"
        ),
    ]
)
