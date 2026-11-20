// swift-tools-version:5.9
// The MaskoPlayer Swift package: the Masko engine (MaskoPlayerFFI) and the
// player around it, for iPhone, iPad, Mac, Mac Catalyst and Apple Vision Pro.
import Foundation
import PackageDescription

// The engine is a binary target. In a clone of this repository with the
// engine built (scripts/build-apple.sh), it is that local xcframework. An app
// that adds this package by URL gets no build there (the xcframework is
// git-ignored), so it downloads the release zip; scripts/release-apple.sh
// writes its URL and checksum here.
let releaseURL = "https://github.com/masko-ai/masko-player/releases/download/0.1.22/MaskoPlayerFFI.xcframework.zip"
let releaseChecksum = "c1c133cb796fdf3ec9b280c0043d88cf8ba3a0ae34354d317afce7d3f3ff5c4b"

let localEngine = "apple/MaskoPlayerFFI.xcframework"
let engine: Target =
    FileManager.default.fileExists(atPath: Context.packageDirectory + "/" + localEngine + "/Info.plist")
    ? .binaryTarget(name: "MaskoPlayerFFI", path: localEngine)
    : .binaryTarget(name: "MaskoPlayerFFI", url: releaseURL, checksum: releaseChecksum)

// dav1d decodes AV1 on devices without an AV1 decoder. Like the engine, it
// is the local xcframework when built (scripts/build-dav1d-apple.sh), else its
// release zip; scripts/release-apple.sh writes that zip's URL and checksum.
let dav1dReleaseURL = "https://github.com/masko-ai/masko-player/releases/download/0.1.22/dav1d.xcframework.zip"
let dav1dReleaseChecksum = "7350930e95b666fa2864b83db039afe9a824ae3b5614de705b023447426d1a98"

let localDav1d = "apple/dav1d.xcframework"
let dav1d: Target =
    FileManager.default.fileExists(atPath: Context.packageDirectory + "/" + localDav1d + "/Info.plist")
    ? .binaryTarget(name: "Dav1d", path: localDav1d)
    : .binaryTarget(name: "Dav1d", url: dav1dReleaseURL, checksum: dav1dReleaseChecksum)

let package = Package(
    name: "MaskoPlayer",
    platforms: [.iOS(.v15), .macOS(.v13), .macCatalyst(.v15), .visionOS(.v1)],
    products: [
        .library(name: "MaskoPlayer", targets: ["MaskoPlayer"]),
        // A Mac window for a mascot on the desktop that stays still while desktops swipe.
        // Separate, because it uses a private macOS framework that the Mac App Store refuses.
        .library(name: "MaskoOverlay", targets: ["MaskoOverlay"]),
    ],
    targets: [
        engine,
        dav1d,
        .target(
            name: "MaskoPlayer",
            dependencies: ["MaskoPlayerFFI", "Dav1d"],
            path: "apple/Sources/MaskoPlayer",
            resources: [.copy("PrivacyInfo.xcprivacy")]
        ),
        .target(
            name: "MaskoOverlay",
            dependencies: ["MaskoPlayer"],
            path: "apple/Sources/MaskoOverlay"
        ),
        .testTarget(
            name: "MaskoOverlayTests",
            dependencies: ["MaskoOverlay"],
            path: "apple/Tests/MaskoOverlayTests"
        ),
        .testTarget(
            name: "MaskoPlayerTests",
            dependencies: ["MaskoPlayer"],
            path: "apple/Tests/MaskoPlayerTests",
            resources: [.copy("Clips")]
        ),
    ]
)
