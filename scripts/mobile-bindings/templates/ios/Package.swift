// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "cdk-swift-local",
    platforms: [.iOS("18.0")],
    products: [
        .library(name: "Cdk", targets: ["Cdk"])
    ],
    targets: [
        .binaryTarget(
            name: "CashuDevKitFFI",
            path: "CashuDevKitFFI.xcframework"
        ),
        .target(
            name: "Cdk",
            dependencies: ["CashuDevKitFFI"],
            linkerSettings: [
                .linkedFramework("Network"),
                .linkedFramework("SystemConfiguration")
            ]
        )
    ]
)
