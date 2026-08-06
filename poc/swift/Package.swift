// swift-tools-version: 6.0
import PackageDescription

// Built against the Command Line Tools SDK — no Xcode project, no asset catalog, no bundle.
// That is itself part of what the spike is measuring: what a Swift surface costs to build.
//
// The one dependency points at the product, which is the direction that matters. `CsKit` holds
// the decoder and the transport, and the spike consumes them rather than owning them, so
// `cs-spike contract` checks the same bytes the app is built on instead of a copy that has
// drifted (`chat-search-me9.8.1`). Nothing in `apps/` points back here.
//
// The floor is the app's floor and not a judgement of its own. A package declares one set of
// platforms for every product in it, so `apps/macos` raising itself to macOS 15 for
// `onScrollGeometryChange` (`chat-search-me9.8.27`) raises `CsKit` with it, and a consumer below
// that floor does not build. Nothing here needs macOS 15; it needs the same `CsKit` the app runs
// on, which is the whole reason this dependency points where it does.
let package = Package(
    name: "cs-spike",
    platforms: [.macOS(.v15)],
    dependencies: [.package(path: "../../apps/macos")],
    targets: [
        .executableTarget(
            name: "cs-spike",
            dependencies: [.product(name: "CsKit", package: "macos")],
            path: "Sources/cs-spike",
            swiftSettings: [.swiftLanguageMode(.v6)]
        )
    ]
)
