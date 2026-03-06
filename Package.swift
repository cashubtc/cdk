// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Cdk",
    platforms: [.iOS(.v14), .macOS(.v13)],
    products: [
        .library(
            name: "Cdk",
            targets: ["Cdk", "CashuDevKitFFI"]
        ),
    ],
    targets: [
        .target(
            name: "Cdk",
            dependencies: ["CashuDevKitFFI"],
            path: "bindings/swift/Sources/Cdk"
        ),
        .binaryTarget(
            name: "CashuDevKitFFI",
            path: "bindings/swift/build/xcframework/CashuDevKitFFI.xcframework"
        ),
    ]
)
