# @masko/player-react-native

The Masko player for React Native and Expo apps: a `<MaskoView>` that plays
a mascot release with transparency, driven with `set` and `fire`. On iOS it
is the `MaskoPlayer` Swift package (`../apple`), on Android the
`ai.masko:player` Kotlin library (`../android`); every canvas rule runs in
the shared Rust engine. This package only bridges them: it never plays video
or decides a clip itself.

It is an Expo module, for Expo SDK 54 (React Native 0.81, new
architecture) and for bare React Native apps with Expo modules installed
(`npx install-expo-modules`). iOS 15.1, Android minSdk 24.

```tsx
import { MaskoView, type MaskoHandle } from '@masko/player-react-native';

const mascot = useRef<MaskoHandle>(null);

<MaskoView
  ref={mascot}
  style={{ width: 200, height: 200 }}
  delivery={delivery}                   // the release delivery, its JSON, or the API response
  getDelivery={() => api.delivery()}    // fresh signed links for the same release
  onPose={({ pose }) => {}}
  onError={({ code }) => {}}
/>;

mascot.current?.set('working', true);
mascot.current?.fire('celebrate');
mascot.current?.set('heldContact', true, { instant: true }); // now; an Any State start cuts straight in
```

One animation on its own: `<MaskoView clip={{ hevc, stacked }} />` in
place of `delivery`. iOS plays the HEVC file, Android the stacked MP4; each
loops as one pose named `clip`, with its voice when the file has one. A clip without the file for the platform, or
whose length cannot be read, reports `clip_failed`.

- `delivery` is the `delivery` object of
  `GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery`, its JSON, or
  the response around it.
- The ref: `set(name, value)` with a boolean or a number, `fire(name)`
  (both take `{ instant: true }` last: what that value starts happens now;
  an Any State start goes straight into its target pose, skipping every
  transition clip and route, and a move plays its own clip, `spec/rules.md`
  5.8),
  `reset()`, `play()`, `pause()`, `position()` (a promise of
  `{ clip, time }`, the clip on screen and its clip time in seconds, with
  `clip` null before anything shows) and
  `preview.showPose(id)` / `preview.playMove(id)` (editors only). Calls reach
  the native player on the main thread, in the order they were made. A call
  after the view is gone does nothing. There is no `destroy()`, which
  `../spec/player.md` lists for the other platforms: unmounting the view
  destroys its player.
- Events, as props, with the same data as `@masko/player`:
  `onPose({ pose })`, `onClip({ clip, kind, rate })` (an edge id, or
  `still:<pose>`; `kind` is `pose`, `move` or `still`),
  `onReactionDone({ name, used })`,
  `onMovement({ edge, movement, rate, seconds })` (a move's `movement`, for
  the app to move the mascot over `seconds`; the player never moves itself)
  and `onError({ code, detail })`. A delivery that is not JSON gives
  `error` `unreadable` on both platforms.
- `sound` is on by default: a clip's own voice is heard; `false` mutes it.
  Edge sounds play either way. On iOS the app chooses its `AVAudioSession`
  category; the Android player never takes audio focus.
- `autoPlay` is read when a release loads. With `false` the mascot shows its
  first frame and waits for `play()`. The prop wins over the graph's
  `autoPlay`; left out, the graph decides.
- The mascot does not watch touches or the pointer. The app sends them
  itself, from a `Pressable` around the mascot for example, as it does for
  dragging and physics: `ref.current?.fire('action::click')`,
  `set('behavior::pointerHover', on)`, and `heldContact`, `heldShake` and
  `falling`. Each waits for the end of the loop on screen unless the app
  passes `{ instant: true }`.
- `onError`'s `detail` says what the error is about: the edge of a clip
  error, the name of a refused value, the pose of a pose error.
- A new release in `delivery` loads a new player. A delivery of the same
  release with new links keeps the mascot playing and gives the native
  player the new links (its `updateDelivery`). Giving or taking away
  `getDelivery` loads the release again with the latest `delivery` prop's
  links. That restarts the mascot: it starts from its first pose, without
  the values the app set.
- Signed links: pass `getDelivery`. The native player calls it five minutes
  before `deliveryAccess.signedUrlExpiresAt` (halfway there when less than
  ten minutes are left) and swaps the links only; files already downloaded
  keep playing. A `getDelivery` that throws or returns nothing gives
  `error` `refresh_failed`, another release `refresh_mismatch`, and the
  player asks again 30 seconds later. A view that unmounts while
  `getDelivery` runs stops waiting for it and drops its answer; so does an
  iOS view that leaves its window, whose player asks again 30 seconds
  later.
- Unmounting the view destroys its player. While the view is off screen the
  player pauses itself, and resumes where it stopped when it is back: out
  of its window or with the app in the background on iOS, detached or with
  its window hidden on Android.
- iOS plays each edge's `videos.av1` (stacked AV1) when it has one, else its
  `videos.hevc` (HEVC with alpha); Android its `videos.av1` when it has one,
  else its `videos.stacked` (colour above, transparency below). AV1 files
  come in a delivery asked with `formats=av1` (`../spec/player.md`, "Asking
  for AV1"). On Android, fetch the
  release delivery with `target=android`; an older release without stacked
  files shows each pose's still until the canvas is released again
  (`../android/README.md`).

## A canvas from its URL

An app with no backend plays a canvas from one link: the native player
reads the canvas's latest release, keeps every file so the mascot also
plays offline, and moves to a newer release on its own. No `delivery` or
`getDelivery` is needed.

```tsx
<MaskoView
  ref={mascot}
  style={{ width: 200, height: 200 }}
  url="https://api.masko.ai/v1/canvases/CANVAS_ID/delivery?key=masko_app_..."
/>;

// Or with the app key apart:
<MaskoView url="https://api.masko.ai/v1/canvases/CANVAS_ID/delivery" appKey="masko_app_..." />;
```

`url` takes the place of `delivery`: give one or the other. The ref, the
events, `sound` and `autoPlay` work as with a delivery. A new `url` or
`appKey` loads a new player. `getDelivery` is ignored with `url`. On iOS
this is `MaskoPlayer(url:appKey:autoPlay:)`, on Android
`MaskoPlayerView.loadCanvas(url, appKey, options)` (`../spec/player.md`,
"A canvas from its URL").

## Install

```bash
npm install @masko/player-react-native
npx expo run:ios          # or run:android; prebuild links the native code
```

The package carries both native players: the Swift player with the
engine's iOS slices, and `ai.masko:player` as a local Maven repository.

To work on the package itself, build the native players from this
repository, then install this folder in an app, as
`../examples/react-native` does:

```bash
scripts/build-react-native.sh                      # in this repository
npm install <this repository>/react-native         # in your app
```

The Android engine is built for `arm64-v8a`, `armeabi-v7a` and `x86_64`.
JNA, which the Kotlin player calls the engine through, ships other ABIs too,
so an app should keep only these, for example with
`reactNativeArchitectures=armeabi-v7a,arm64-v8a,x86_64` in
`android/gradle.properties`.

## Layout

- `src/MaskoView.tsx`: the component, its ref and the events.
- `src/delivery.ts`: the delivery's JSON for the native players, and which
  release it is.
- `ios/MaskoPlayerModule.swift`: the view's props, events and calls.
  `ios/MaskoNativeView.swift`: a `MaskoPlayerView` with its `MaskoPlayer`.
- `android/src/main/java/ai/masko/player/reactnative/`: the same for
  Android, around `ai.masko.player.MaskoPlayerView`.
- `test/`: Jest tests of the JavaScript layer against a fake native view.
- `scripts/vendor.mjs`: copies the native players into the package before
  `npm pack`.

## Where the native players come from

- **iOS.** `ios/MaskoPlayerReactNative.podspec` builds the Swift player and
  this package's two Swift files into one pod, with the engine
  (`MaskoPlayerFFI.xcframework`), all from `ios/vendor/`. In this
  repository `pod install` links `ios/vendor/` to `../apple`, so apps build
  the current `apple/Sources`. `npm pack` copies them in instead, with the
  engine's iOS slices only.
- **Android.** `android/build.gradle` compiles the Kotlin player into this
  library, with its bindings, engine libraries and R8 rules, and declares
  the player's own dependencies (Media3, JNA). In this repository it reads
  `android/player/src/main`; `npm pack` copies it into `android/player/`.
  No Maven repository is involved, so an Expo build
  (`npx expo run:android`, which configures on demand) finds it too.

## Build and test

```bash
cd react-native
npm install
npm run typecheck
npm run lint
npm test                        # Jest: props, ref calls and events against a fake native view
```

These checks need no native build; CI runs them on Ubuntu
(`../.github/workflows/ci.yml`). The example and `npm pack` need
`scripts/build-react-native.sh` first (from the repository root: the
engines and `ai.masko:player`). The native players run their own tests
(`../apple/README.md`, `../android/README.md`). The example below runs the
whole bridge on the iOS simulator and an Android emulator.

## Example

`../examples/react-native` is an Expo app with a mascot, buttons for `set`,
`fire`, `sound`, pause, reset and position, and a log of every event. It
plays a small release served by `server/serve.mjs`, with clips made by
`server/make-clips.sh` (ffmpeg and Apple's encoder, so on a Mac). Its links
expire after five and a half minutes, so `getDelivery` runs too.

```bash
cd examples/react-native
npm install
npm run clips              # http://localhost:47290/delivery.json
npx expo run:ios           # or: npx expo run:android (the emulator reaches the server at 10.0.2.2)
```

The server also relays commands to the app, so a terminal or a test drives
it without touching the screen: `curl -X POST localhost:47290/do/<command>`
with `fire/celebrate`, `set/working/true`, `sound/on`, `pause`, `play`,
`reset`, `position` or `delivery` (a new delivery prop with fresh links,
which the view hands to `updateDelivery`). Each one shows in the app's log.

## Releases

A version tag publishes it from `.github/workflows/release.yml`, with the
other packages (`../RELEASING.md`). `npm publish` runs `prepack`, which
needs `apple/MaskoPlayerFFI.xcframework` (`scripts/build-apple.sh`) and the
published `ai.masko:player`
(`scripts/build-react-native.sh`): it copies the Swift player, the iOS engine
and the Maven repository into the package, and builds `build/` with `tsc`.
