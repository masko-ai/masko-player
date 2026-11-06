# MaskoPlayer for iPhone, iPad and Mac

The `MaskoPlayer` Swift package plays a Masko mascot release on iOS 15,
macOS 13, Mac Catalyst 15 and visionOS 1 or later. The Rust engine
(`engine/`) decides which clip plays and when; this package keeps the
play-time clock, shows the clips on two `AVPlayerLayer` slots and reports
when they end, as `spec/player.md` says.

## Install

Add the package by its URL; the engine comes built, so the app needs no
Rust.

- Xcode: File > Add Package Dependencies, enter
  `https://github.com/masko-ai/masko-player`, choose Up to Next Major
  Version from `0.1.0`, and add the `MaskoPlayer` product to the app target.
- `Package.swift`:

  ```swift
  dependencies: [
      .package(url: "https://github.com/masko-ai/masko-player", from: "0.1.0")
  ],
  targets: [
      .target(name: "MyApp", dependencies: [
          .product(name: "MaskoPlayer", package: "masko-player")
      ])
  ]
  ```

The engine is the binary target `MaskoPlayerFFI`: SwiftPM downloads
`MaskoPlayerFFI.xcframework.zip` from the GitHub release of that version
and checks it against the checksum in `Package.swift`. The package ships a
privacy manifest (`PrivacyInfo.xcprivacy`): no tracking; the system boot time
(`ProcessInfo.systemUptime`, reason `35F9.1`) for the play-time clock; and,
for a canvas read from its URL only, a random id the installed app keeps
(`Masko-Install-Id`), not linked to the person, so the mascot's owner can
count active copies.

A Mac app built for Apple silicon and Intel at once from the command line,
`swift build --arch arm64 --arch x86_64`, fails to link with Xcode 16's
linker (an assertion in `ObjCClassReadOnlyDataRef`): that build merges dav1d,
whose ARM assembly is not split by symbol, into the player's object file.
Xcode and `xcodebuild` builds, for both architectures too, link dav1d only
into the app and are not affected. From the command line, build each
architecture on its own and join them:

```bash
swift build -c release --arch arm64
swift build -c release --arch x86_64
lipo -create "$(swift build -c release --arch arm64 --show-bin-path)/MyApp" \
  "$(swift build -c release --arch x86_64 --show-bin-path)/MyApp" -output MyApp
```

## Use

```swift
import MaskoPlayer

let player = MaskoPlayer(delivery: json)   // the release delivery object
player.onEvent = { event in print(event) } // pose, clip, reactionDone, movement, error
player.set("working", true)
player.fire("celebrate")
player.set("heldContact", true, instant: true) // now; an Any State start cuts straight in

MaskoView(player: player)                  // SwiftUI
MaskoPlayerView(player: player)            // UIView on iOS, NSView on macOS
```

An app without a backend plays its canvas from its URL with an app key of
its mascot, which can only read that mascot's released canvases:

```swift
let player = MaskoPlayer(
    url: URL(string: "https://api.masko.ai/v1/canvases/CANVAS_ID/delivery")!,
    appKey: "masko_app_...")
```

The player asks for the latest release (`target=apple`, `formats=av1`),
downloads every file once and keeps it in
`Application Support/<bundle id>/MaskoPlayer/`, out of backups, so the mascot
plays offline. It asks again at launch and every hour, and switches to a
newer release once all its files are kept, where the first pose's loop starts
again, keeping the values the app set (`../spec/player.md`, "A canvas from
its URL"). Before the first delivery is in, values set choose the first pose
and fires are dropped.

One animation on its own, from its HEVC file: `MaskoPlayer(clip: url)`. The
player reads the file's length and sound track and loops it as one pose
named `clip`, with its voice when it has one; a file it cannot read plays
nothing and reports `clip_failed`.

- `set(_:_:instant:)` takes a `Bool` or a number, `fire(_:instant:)` a name.
  With `instant: true`, what that value starts happens now and goes straight
  into its target pose, skipping every transition clip and route
  (`spec/rules.md` 5.8). Values set right after the player is made choose
  its first pose. The player then loads the
  first frame and starts, at play time 0, once that frame is on screen; with
  `autoPlay` off it shows that frame and waits for `play()`.
- `play()`, `pause()`, `reset()`, `destroy()`, `position()` (a `Position`:
  `clip`, the edge on screen or `still:<pose>`, nil before anything shows,
  and `time`, its clip time in seconds) and `preview.showPose(_:)` /
  `preview.playMove(_:)` (editors only).
- `capabilities`: the graph's `capabilities` of the release on screen, as a
  `MaskoJSON` (its `actions` and `behaviors`, each with its `input`), or nil
  when it has none. A canvas read from its URL changes it when a newer
  release takes over, so an app that picks what to fire from it only names
  clips the mascot on screen has.
- `autoPlay` in the initializer wins over the graph's `autoPlay`; left nil,
  the graph decides, else the mascot starts at once.
- Events, the same data as on every platform: `.pose(pose:)`,
  `.clip(clip:kind:rate:)` (`kind` is `.pose`, `.move` or `.still`),
  `.reactionDone(name:used:)`, `.movement(edge:movement:rate:seconds:)`
  (`seconds` is the edge's duration at this rate) and `.error(code:detail:)`.
  A refused delivery gives the engine's own code (`unreadable` for text that
  is not JSON).
- `sound` is on by default: a clip's own voice is heard; `false` mutes it.
  A voiced stacked AV1 clip waits for its sound to reach the speakers before
  its picture moves, so the voice stays on the lips, also on Bluetooth
  headphones.
  Edge sounds play either way. The app chooses its `AVAudioSession` category.
- The view does not watch the person. The app sends taps, clicks and the
  pointer itself, with its own gestures, as it already does for dragging and
  physics: `player.set("behavior::pointerHover", on)`,
  `player.fire("action::click")`, and `heldContact`, `heldShake` and
  `falling`. Each waits for the end of the loop on screen unless the app
  passes `instant: true`.
- An `error` event's `detail` says what it is about: the edge of a clip
  error, the name of a refused value, the pose of a pose error.
- A player shows in one view at a time: the newest view given it that is in
  a window, else the newest. It pauses itself while that view is out of its
  window (or the app is in the background on iPhone and iPad), and when the
  last view lets go of it. A mascot never keeps the screen awake.
- Signed links: pass `getDelivery`, an async closure that fetches the same
  release again. The player calls it five minutes before
  `deliveryAccess.signedUrlExpiresAt` (halfway there when less than ten
  minutes are left) and swaps the links only; files already downloaded keep
  playing. When it throws or gives something the player cannot take, the
  player reports `refresh_failed` or `refresh_mismatch` and asks again 30
  seconds later.
- `updateDelivery(_:)` takes a fresh delivery of the same release from the
  app at any time, with the same rules: the links only, the engine keeps
  running, clips that failed are tried again. It returns false, and reports
  `refresh_mismatch` for another release or `refresh_failed` for a delivery
  it cannot read.
- The player picks each edge's `videos.av1` file (stacked AV1) when it has
  one, else its `videos.hevc` file (HEVC with alpha), or with
  `videos.variants` the smallest size at least as large as the view in
  pixels. Ask the release delivery for AV1 with `formats=` and
  `MaskoPlayer.formats`: AV1 files are two to three times lighter, and the
  player decodes them on every device, with the device's AV1 decoder
  (iPhone 15 Pro, M3 Macs and later) or with dav1d (`../spec/player.md`,
  "Asking for AV1"). A pose clip that started before the view had a size switches to
  the right file at its next loop.
- Downloads go in `tmp/MaskoPlayer/`. `destroy()` cancels the ones in
  flight and deletes the files; a launch deletes what earlier launches left.
  A canvas read from its URL keeps its files instead, and `destroy()` leaves
  them.

## Layout

- `Sources/MaskoPlayer/MaskoPlayer.swift`: the player: engine calls, commands,
  slots, stills, sounds, clock, link refresh. The hidden slot decodes the
  next clip ahead: the decided one, or during a move the target pose's loop.
- `Stage.swift`: the two-slot `AVPlayerLayer` stage. A clip swaps in only when
  its first frame is decoded and ready for display, in one transaction with
  implicit animations off.
- `Delivery.swift`: the delivery fields the player reads itself (files,
  sounds, movement, stills, the poses each edge joins, link expiry).
- `MediaLoader.swift`: downloads files into a small cache, keyed by edge and
  size so a refreshed link reuses them, or into a canvas's kept files.
- `KeptRelease.swift`: a canvas read from its URL: the request with the app
  key, the kept files and the kept release.
- `MaskoPlayerView.swift`: the view, and which view shows a player.
- `Sounds.swift`, `Clock.swift`, `MaskoView.swift`.
- `PrivacyInfo.xcprivacy`: the privacy manifest, a resource of the target.
- `Generated/masko_player_ffi.swift`: the UniFFI bindings, written by the build
  script. Do not edit it.
- `MaskoPlayerFFI.xcframework`: the engine as a static library for macOS
  (arm64, x86_64), iOS devices (arm64), the iOS simulator (arm64, x86_64),
  Mac Catalyst (arm64, x86_64), visionOS devices (arm64) and the visionOS
  simulator (arm64). It is built locally and git-ignored.
- `dav1d.xcframework`: dav1d, the AV1 decoder for devices without one, as a
  static library for the same platforms, built by
  `../scripts/build-dav1d-apple.sh`. It is built locally and git-ignored.
- `tools/make-test-clips.swift`: writes the HEVC-with-alpha test clips in
  `Tests/MaskoPlayerTests/Clips`; `tools/make-av1-test-clips.sh` writes the
  stacked AV1 ones.

`Package.swift` uses `apple/MaskoPlayerFFI.xcframework` when it exists, and
the release zip otherwise.

## Build and test

```bash
scripts/build-apple.sh   # the xcframework and the Swift bindings
swift test               # scenarios, player, file cache and real video on macOS
xcodebuild test -scheme MaskoPlayer \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro'
xcodebuild test -scheme MaskoPlayer \
  -destination 'platform=macOS,variant=Mac Catalyst'
```

The tests run every `spec/fixtures` scenario through the Swift binding, the
player against a fake video layer, and real HEVC-with-alpha clips on
AVFoundation.

Run `scripts/build-apple.sh` before the first `swift` or `xcodebuild` command
in a clone. SwiftPM caches what `Package.swift` says by its text, so a read
made before the local engine existed keeps using the release zip; the build
script warns when that happens, and `swift package purge-cache` clears it.

## Releases

```bash
scripts/release-apple.sh 0.1.0
```

It builds the engine and dav1d, writes
`target/apple-release/<version>/MaskoPlayerFFI.xcframework.zip` and
`dav1d.xcframework.zip` (each xcframework at the top of its zip, as SwiftPM
expects), and sets `releaseURL` and `releaseChecksum`, and
`dav1dReleaseURL` and `dav1dReleaseChecksum` (`swift package
compute-checksum`), in `Package.swift`, pointing at the GitHub release named
after the version. Every file in a zip is dated to the commit it builds
from, so the same commit gives the same zip (`scripts/zip-xcframework.sh`;
its check is `scripts/test-zip-xcframework.sh`). Then:

1. Commit `Package.swift` with `Sources/MaskoPlayer/Generated/` from the
   same build: the bindings must match the engine in the zip.
2. Tag that commit with the version itself (`0.1.0`, no `v`) and push it.
3. Make the GitHub release for the tag with both zips, under their own
   names: `gh release create <version> target/apple-release/<version>/*.zip`.

Run the script for a release whose engine, dav1d or bindings changed: the
zips, their checksums and the bindings go together. A release without such a
change keeps the earlier release's zips. [RELEASING.md](../RELEASING.md) has
the whole release, with the other packages.
