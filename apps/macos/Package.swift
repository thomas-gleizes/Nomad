// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Nomad",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "Nomad",
            path: "Sources/Nomad"
        )
    ]
)
