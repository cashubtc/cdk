// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "CdkExample",
    platforms: [.iOS(.v17), .macOS(.v14)],
    dependencies: [
        // Point to the local CDK Swift bindings
        .package(name: "Cdk", path: "../../.."),
    ],
    targets: [
        .executableTarget(
            name: "CdkExample",
            dependencies: ["Cdk"],
            path: "CdkExample"
        ),
    ]
)
