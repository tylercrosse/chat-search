// swift-tools-version: 6.0
import PackageDescription

// Built against the Command Line Tools SDK — no Xcode project, no asset catalog, no bundle.
// That is itself part of what the spike is measuring: what a Swift surface costs to build.
let package = Package(
    name: "cs-spike",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "cs-spike",
            path: "Sources/cs-spike",
            swiftSettings: [.swiftLanguageMode(.v6)]
        )
    ]
)
