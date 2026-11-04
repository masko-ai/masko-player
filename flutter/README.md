# masko_player

The Masko player for Flutter apps: a `MaskoView` widget that plays a mascot
release with transparency, driven with `set` and `fire`. On iOS it is the
`MaskoPlayer` Swift package (`../apple`), on Android the `ai.masko:player`
Kotlin library (`../android`); every canvas rule runs in the shared Rust
engine. This package only bridges them: it never plays video or decides a
clip itself.

iOS 15 and Android minSdk 24, Flutter 3.24 or later. iOS apps build it with
Swift Package Manager or CocoaPods.

```dart
import 'package:masko_player/masko_player.dart';

final mascot = MaskoController();

SizedBox(
  width: 200,
  height: 200,
  child: MaskoView(
    controller: mascot,
    delivery: delivery,                     // the release delivery, its JSON, or the API response
    getDelivery: () => api.delivery(),      // fresh signed links for the same release
    onPose: (pose) {},
    onError: (error) {},
  ),
);

mascot.set('working', true);
mascot.fire('celebrate');
mascot.set('heldContact', true, instant: true); // now; an Any State start cuts straight in
```

One animation on its own: `MaskoView(clip: MaskoClipFiles(hevc: ..., stacked: ...))`
in place of `delivery`. iOS plays the HEVC file, Android the stacked MP4;
each loops as one pose named `clip`, with its voice when the file has one. A
clip without the file for the platform, or whose length cannot be read,
reports `clip_failed`.

- `delivery` is the `delivery` object of
  `GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery` as a `Map`, its
  JSON, or the response around it. Fetch it on your backend with your API
  key; never put the key in an app.
- The controller: `set(name, value)` with a `bool` or a number,
  `fire(name)` (both take `instant: true`: what that value starts happens
  now; an Any State start goes straight into its target pose, skipping
  every transition clip and route, and a move plays its own clip,
  `spec/rules.md` 5.8), `reset()`, `play()`, `pause()`, `position()` (a
  future of `MaskoPosition`, the clip on screen and its clip time in
  seconds, with `clip` null before anything shows) and
  `preview.showPose(id)` / `preview.playMove(id)` (editors only). Calls
  reach the native player on the main thread, in the order they were made.
  Calls made before the view first builds reach the player before it
  starts, so the values set then choose the first pose (`spec/rules.md`
  11.1); calls made while no view shows wait for the next one. A controller
  drives one view at a time. There is no `destroy()`: removing the view
  destroys its player.
- Events, as callbacks, with the same data as the other players:
  `onPose(pose)`, `onClip(MaskoClipEvent)` (`clip` is an edge id, or
  `still:<pose>`; `kind` is `pose`, `move` or `still`),
  `onReactionDone(MaskoReactionDoneEvent)`,
  `onMovement(MaskoMovementEvent)` (a move's `movement`, for the app to
  move the mascot over `seconds`; the player never moves itself) and
  `onError(MaskoErrorEvent)`. A delivery that is not JSON gives `error`
  `unreadable` on both platforms; other platforms than iOS and Android show
  nothing and give `unsupported_platform`.
- `sound` is on by default: a clip's own voice is heard; `false` mutes it.
  Edge sounds play either way. On iOS the app chooses its `AVAudioSession`
  category; the Android player never takes audio focus.
- `autoPlay` is read when a release loads. With `false` the mascot shows its
  first frame and waits for `play()`. It wins over the graph's `autoPlay`;
  left out, the graph decides.
- The mascot does not watch touches. It takes hits like a picture, so a
  `GestureDetector` around it gets them, and the app sends what the person
  does, as it does for dragging and physics: `mascot.fire('action::click')`,
  `set('behavior::pointerHover', on)`, and `heldContact`, `heldShake` and
  `falling`. Each waits for the end of the loop on screen unless the app
  passes `instant: true`.
- `onError`'s `detail` says what the error is about: the edge of a clip
  error, the name of a refused value, the pose of a pose error.
- A new release in `delivery` loads a new player. A delivery of the same
  release with new links keeps the mascot playing and gives the native
  player the new links (its `updateDelivery`). Giving or taking away
  `getDelivery` loads the release again with the latest links. That
  restarts the mascot: it starts from its first pose, without the values the
  app set.
- Signed links: pass `getDelivery`. The native player calls it five minutes
  before `deliveryAccess.signedUrlExpiresAt` (halfway there when less than
  ten minutes are left) and swaps the links only; files already downloaded
  keep playing. A `getDelivery` that throws or returns null gives `error`
  `refresh_failed`, another release `refresh_mismatch`, and the player asks
  again 30 seconds later.
- Links can point anywhere the device can read: Masko's CDN, your own
  storage, or files on the device (`file:` links, and plain file paths for
  a lone clip).
- iOS plays each edge's `videos.av1` (stacked AV1) when it has one, else its
  `videos.hevc` (HEVC with alpha); Android its `videos.av1` when it has one,
  else its `videos.stacked` (colour above, transparency below). Ask the
  delivery with `target=apple` on iOS, `target=android` on Android, and
  `formats=` the value of `maskoFormats()`: `av1` on every iPhone and on
  Android phones with an AV1 decoder, which makes each file two to three
  times lighter (`../spec/player.md`, "Asking for AV1").

## Install

```yaml
dependencies:
  masko_player: ^0.1.15
```

The package carries both native players: the Swift player with the
engine's iOS slices and the AV1 decoder dav1d, and the Kotlin player with
its engine libraries. iOS apps need iOS 15 or later: the Runner's
`IPHONEOS_DEPLOYMENT_TARGET`, and `platform :ios, '15.0'` in the `Podfile`
of an app that uses CocoaPods. Android apps need `minSdk = 24` or more.

To work on the package itself, copy the native players into it from this
repository, then depend on this folder by path, as `../examples/flutter`
does:

```bash
scripts/build-flutter.sh          # in this repository; again after a change to apple/, android/ or the engine
```

## Layout

- `lib/src/masko_view.dart`: the widget, its controller, `maskoFormats()`
  and the platform channels.
- `lib/src/delivery.dart`: the delivery's JSON for the native players, and
  which release it is.
- `lib/src/types.dart`: the events and values the app hears.
- `ios/masko_player/Sources/masko_player/`: the plugin and
  `MaskoFlutterView`, a `MaskoPlayerView` with its `MaskoPlayer`.
- `android/src/main/kotlin/ai/masko/player/flutter/`: the same for Android,
  around `ai.masko.player.MaskoPlayerView`.
- `test/`: tests of the Dart layer against a fake native view.

## Where the native players come from

`scripts/build-flutter.sh` copies them in, and the package publishes the
copies (`.pubignore`), so apps build them from the package alone:

- **iOS.** `ios/masko_player/vendor/`: the Swift player and one
  `MaskoPlayerFFI.xcframework` that merges the engine and dav1d. Swift
  Package Manager builds them from `ios/masko_player/Package.swift` (the
  Swift player as its own `MaskoPlayer` module), CocoaPods from
  `ios/masko_player.podspec` (the Swift player and the plugin in one pod).
- **Android.** `android/player/`: the Kotlin player's sources, bindings,
  engine libraries and R8 rules, which `android/build.gradle.kts` compiles
  into the plugin with the player's own dependencies (Media3, JNA). No Maven
  repository is involved.

## Build and test

```bash
cd flutter
flutter pub get
flutter analyze
flutter test                    # the Dart layer: props, controller calls and events against a fake native view
```

These checks need no native build; CI runs them on Ubuntu
(`../.github/workflows/ci.yml`). The device tests below run the whole bridge
with the native players, on the iOS simulator and an Android emulator. The
native players run their own tests (`../apple/README.md`,
`../android/README.md`).

## Example

`../examples/flutter` is an app with a mascot, buttons for `set`, `fire`,
`sound`, pause, reset and position, a tap on the mascot, and a log of every
event. It plays the small release that `../examples/react-native/server`
serves; its links expire after five and a half minutes, so `getDelivery`
runs too. It also holds the device tests,
`integration_test/player_test.dart`: the first pose, moves and movement,
used and dropped fires, values set before start, position, pause, autoPlay,
reset, preview calls, new links and new releases, `getDelivery` and its
failure, refused deliveries, lone clips, taps, a removed view, and
`maskoFormats()`.

```bash
scripts/build-flutter.sh                                   # once, from the repository root
node examples/react-native/server/serve.mjs                # http://localhost:47290/delivery.json
cd examples/flutter
flutter run                                                # the app (the emulator reaches the server at 10.0.2.2)
flutter test integration_test -d <simulator or emulator>   # the device tests
```

The server also relays commands to the app, so a terminal drives it without
touching the screen: `curl -X POST localhost:47290/do/<command>` with
`fire/celebrate`, `set/working/true`, `sound/on`, `pause`, `play`, `reset`,
`position` or `delivery` (a new delivery with fresh links, which the view
hands to `updateDelivery`). Each one shows in the app's log.

The example sits outside this folder on purpose: Xcode refuses a Swift
package whose folder name differs from its name (`flutter` here,
`masko_player` in the package) when the app that uses it is inside it.

## Releases

A version tag publishes it from `../.github/workflows/release.yml`, with the
other packages (`../RELEASING.md`): the job builds the native players,
copies them in with `scripts/build-flutter.sh`, runs the checks above and
publishes. pub.dev trusts that workflow for tags that match `{{version}}`
(the package's admin page, "Automated publishing"), so no token is needed.
`../scripts/set-version.sh` sets the version in `pubspec.yaml` and
`android/build.gradle.kts` with the other packages; `CHANGELOG.md` takes an
entry for each version, written by hand.

To see what a release would publish:

```bash
scripts/build-flutter.sh                                   # from the repository root
(cd flutter && dart pub publish --dry-run)                 # the vendor/ and player/ copies must be in
```
