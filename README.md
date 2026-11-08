# Masko player

[![CI](https://github.com/masko-ai/masko-player/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/masko-ai/masko-player/actions/workflows/ci.yml)

One player for Masko mascots on every platform. One Rust engine holds every
canvas rule; each platform adds only its video players and its view. Apps
call `set` and `fire`, and the canvas decides which clip plays.

| Platform              | Package                      | Folder          |
| --------------------- | ---------------------------- | --------------- |
| Web (any framework)   | `@masko/player`              | `web/`          |
| iPhone, iPad and Mac  | `MaskoPlayer` Swift package  | `apple/`        |
| Android               | `ai.masko:player`            | `android/`      |
| React Native and Expo | `@masko/player-react-native` | `react-native/` |
| Flutter               | `masko_player`               | `flutter/`      |
| Windows (Rust)        | `masko-player-windows` crate | `windows/`      |

Install:

- **Web**: `npm install @masko/player` ([web/README.md](web/README.md)). It
  runs in Electron too ([examples/electron](examples/electron)).
- **Apple**: add `https://github.com/masko-ai/masko-player` from `0.1.0` in
  Xcode or `Package.swift`, product `MaskoPlayer`
  ([apple/README.md](apple/README.md)).
- **React Native and Expo**: `npm install @masko/player-react-native`
  ([react-native/README.md](react-native/README.md)); it carries the iOS and
  Android players.
- **Flutter**: `flutter pub add masko_player`
  ([flutter/README.md](flutter/README.md)); it carries the iOS and Android
  players.
- **Android**: `implementation("ai.masko:player:0.1.16")` from Maven Central
  ([android/README.md](android/README.md)).
- **Windows**: the Rust crate in `windows/` (not on crates.io yet): it keeps
  a canvas's stacked AV1 clips on the computer, decodes them with dav1d and
  draws them in a transparent window. The Masko desktop pets for Windows use
  it. Build dav1d first with `scripts/build-dav1d-windows.sh`. Its clock is
  awake time, like the Mac's: the hours the PC sleeps, and any stop of the
  program longer than 5 seconds, do not run the mascot's clips on.

The React Native and Flutter packages wrap the Swift and Kotlin players;
they add no rules of their own.

Every player reads a release delivery: the `delivery` object of the Masko
API's `GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery`, with
`target=web`, `target=apple` or `target=android` for the files that
platform plays.

On iPhone, iPad and Mac, an app without a backend can instead give the
player its canvas's URL, `https://api.masko.ai/v1/canvases/{canvasId}/delivery`,
and an app key of its mascot: the player asks for the latest release, keeps
every file for offline play and switches to newer releases by itself
(`spec/player.md`, "A canvas from its URL").

A player also plays one animation on its own, from its file links (`webm`
and `hevc` on the web, `hevc` on Apple, `stacked` on Android), looped as one
pose (`spec/player.md`, "One clip").

## File size

Apps that send `formats` (from 0.1.6, 0.1.7 in React Native) get each
clip's stacked AV1 file: colour above, transparency below, made at a
lighter setting that keeps the look at mascot size. One 5-second animation
at 720 px is about 0.24 MB in AV1, against 2.2 MB in HEVC on iPhone and
2.4 MB in the stacked MP4 on Android. It plays on iPhone, iPad, Mac and
Android 10 and later; on Apple the player carries dav1d (about 0.7 MB) for
devices without an AV1 chip. Animations made from October 27, 2026 have
it. Browsers keep WebM or HEVC.

Every format's size, and a calculator for your own mascot:
[masko.ai/docs/generate/file-sizes](https://masko.ai/docs/generate/file-sizes).

## What is here

- `spec/rules.md`: the rulebook every player follows.
- `spec/player.md`: what every platform's player does with the engine's
  commands, the file it picks, what the app sees, and the tests it runs.
- `spec/fixtures/`: the scenarios that prove the rulebook, run by the engine
  and by every platform through its own binding.
- `spec/release.schema.json`: the JSON Schema of the release delivery.
- `engine/`: the Rust engine; `engine/SIZE.md` has its size.
- `bindings/wasm/`: the engine for the web (wasm-bindgen).
- `bindings/uniffi/`: the engine for Swift and Kotlin (UniFFI).
- `examples/`: a small app per platform.
- `scripts/`: `setup.sh` and `env.sh` (the Rust toolchain, the Android SDK),
  the engine builds for each platform, the copies the React Native and
  Flutter packages take (`build-react-native.sh`, `build-flutter.sh`), the
  Swift engine's release zip
  (`release-apple.sh`, `zip-xcframework.sh`), and `third-party-notices.mjs`.
- `windows/`: the Windows player (its own Cargo workspace), with
  `examples/desktop_check.rs`, which plays test clips in a real transparent
  window and checks the screen.
- `.github/workflows/ci.yml`: the checks below, on GitHub Actions;
  `.github/workflows/windows.yml`: the Windows player on Windows machines.

## Set up

You need, for the platforms you build:

- curl, and a C compiler and linker: Xcode or its Command Line Tools on
  macOS.
- Node.js 22.18 or later, with npm: web and React Native.
- ffmpeg with libvpx-vp9: the web browser tests make their own clips with it.
- Xcode 16 or later, with an iOS simulator: Apple, and React Native on iOS.
- Java 17, and the Android SDK with NDK 27.1.12297006 at `ANDROID_HOME`
  (Android Studio's folder by default): Android, and React Native on
  Android.
- Flutter 3.24 or later: Flutter (CI and the device tests use 3.47.9).

Then install Rust into `.toolchain/`, inside this repository:

```bash
scripts/setup.sh
```

It installs rustup, the Rust version and targets of `rust-toolchain.toml`,
and the wasm-bindgen CLI the web build needs, without touching your own
Rust. Then it lists which of the tools above this machine has.
`scripts/env.sh` uses `.toolchain/` when it exists, and your own rustup
otherwise.

Each check below runs from the repository root. GitHub Actions runs them
on every push to `main` and every pull request (`.github/workflows/ci.yml`):
the engine, web, Android and React Native on Ubuntu, Apple on macOS 15. The
iOS simulator tests and each platform's device checks run by hand.

The engine, with the scenarios and the engine tests, its formatting and
its lints:

```bash
source scripts/env.sh
cargo test -p masko-player-engine
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Web:

```bash
(cd web && npm install && npx playwright install chromium)
scripts/build-web.sh --package
(cd web && npm run typecheck && npm test && npx playwright test)
```

iPhone, iPad and Mac:

```bash
scripts/build-apple.sh
swift test
scripts/test-zip-xcframework.sh
xcodebuild test -scheme MaskoPlayer -destination 'platform=iOS Simulator,name=iPhone 16 Pro'
```

Android:

```bash
scripts/build-android.sh
(cd android && ./gradlew :player:testDebugUnitTest)
```

React Native (these checks need no native build; the example app needs
`scripts/build-react-native.sh` first):

```bash
(cd react-native && npm install && npm run typecheck && npm run lint && npm test)
```

Flutter (these checks need no native build; the example app and its device
tests need `scripts/build-flutter.sh` first):

```bash
(cd flutter && flutter pub get && flutter analyze && flutter test)
```

Each platform's README has its own details and device checks.

## Releases

[RELEASING.md](RELEASING.md) has every step, from the command line:
`scripts/set-version.sh <version>` sets one version in every package, and a
version tag (`0.2.0`, no `v`) publishes `@masko/player` and
`@masko/player-react-native` to npm, `masko_player` to pub.dev and
`ai.masko:player` to Maven Central from `.github/workflows/release.yml`.
The Swift package is the tag, with its engine zips on a GitHub release when
the engine changed. It also lists the credentials, the checks after a
release, and what to do when a job fails.

## License

MIT. See `LICENSE`. The compiled engine contains Rust crates under their
own licenses, listed with their texts in `THIRD_PARTY_NOTICES.md`
(`node scripts/third-party-notices.mjs` writes it). Every package built from
this repository ships both `LICENSE` and `THIRD_PARTY_NOTICES.md`.
