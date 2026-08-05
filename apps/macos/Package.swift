// swift-tools-version: 6.0
import PackageDescription

// The macOS surface, and the library the JSON contract is decoded in. Built against the Command
// Line Tools SDK — no Xcode project, no asset catalog, no bundle — because nothing here needs one
// yet and a project file is a second place for build settings to live.
//
// `CsKit` is a product rather than a target private to the app, because the decoder is written
// once (`chat-search-me9.36`): every field added after the first non-Rust decoder ships would
// otherwise ship twice. `poc/swift` consumes it, which points the instrument at the product and
// keeps `swift run -c release cs-spike contract` — the only thing in the repo that catches a
// contract break, since `cargo test --workspace` structurally cannot see it — checking the same
// decoder the app runs on.
let package = Package(
    name: "chat-search-macos",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "CsKit", targets: ["CsKit"])
    ],
    targets: [
        .target(name: "CsKit", swiftSettings: [.swiftLanguageMode(.v6)])
    ]
)
