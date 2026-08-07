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
// The floor is macOS 15 and the commitment is per-API rather than per-release
// (`chat-search-me9.8.27`). What bought it is `onScrollGeometryChange(for:of:action:)`, which is
// the only way a `List` says where it actually is — before it, the minimap's viewport box was
// drawn from which rows had reported themselves through `onAppear`, which is a superset of what a
// reader can see and moves in message-sized jumps. Going back below 15 now means `#available`
// guards around that and around `onGeometryChange`, so adopt deliberately.
//
// Fifteen and not twenty-six, which is what this machine runs: there is no macOS 16 through 25, so
// one release of headroom is a year of it.
//
// What it does not buy is any distance from Liquid Glass, which this line claimed until
// `chat-search-me9.8.28` photographed it. Adoption keys off the SDK the binary is *linked against*
// — `vtool -show-build` reports `sdk 26.2` on what this manifest produces — and never off the
// platform it is built *for*. The app has been drawn by the macOS 26 design system since the day
// the toolchain updated. ADR 27 records what is being done about that; this list has nothing to do
// with it and only ever bought API headroom.
let package = Package(
    name: "chat-search-macos",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "CsKit", targets: ["CsKit"]),
        .executable(name: "chat-search", targets: ["ChatSearch"]),
    ],
    targets: [
        .target(name: "CsKit", swiftSettings: [.swiftLanguageMode(.v6)]),
        // The token layer, and a target of its own rather than a folder inside the app: the app
        // has to be able to read tokens and must not be able to author them, and a module boundary
        // is the only place that distinction can actually be enforced. Not a product — nothing
        // outside this package draws anything yet.
        .target(name: "CsTheme", swiftSettings: [.swiftLanguageMode(.v6)]),
        .executableTarget(
            name: "ChatSearch",
            dependencies: ["CsKit", "CsTheme"],
            swiftSettings: [.swiftLanguageMode(.v6)]
        ),
    ]
)
