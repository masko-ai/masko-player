# ai.masko:player for Android

The Masko player for Android apps: a `MaskoPlayerView` that plays a mascot
release with transparency, and a `MaskoPlayer` that apps drive with `set` and
`fire`. Every canvas rule runs in the shared Rust engine (`../engine`) through
its Kotlin binding; this library only plays video and carries out the
engine's commands, as `../spec/player.md` describes.

## Install

Minimum SDK 24 (Android 7.0), Java 17, a hardware-accelerated window (the
default). From Maven Central:

```kotlin
// settings.gradle.kts: repositories { google(); mavenCentral() }
dependencies {
    implementation("ai.masko:player:0.1.12")
}
```

The engine library is built for `arm64-v8a`, `armeabi-v7a` and `x86_64`. JNA
also ships its own library for other ABIs, so an app should keep only these:

```kotlin
android { defaultConfig { ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") } } }
```

The library brings its ProGuard rules for JNA and the bindings. Its licenses
ship inside it: `META-INF/ai.masko.player/LICENSE` (MIT) and
`META-INF/ai.masko.player/THIRD_PARTY_NOTICES.md` (every Rust crate compiled
into the engine, MPL-2.0 uniffi among them).

## Android files

Android plays the `stacked` or `av1` file of each edge (colour above,
transparency below), drawn with a shader. Fetch the release delivery with
`target=android` and `MaskoPlayer.formats`
(`GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery?target=android&formats=${MaskoPlayer.formats}`
on the Masko API): it lists only those files, AV1 where the phone has an AV1
decoder (Android 10 and later), two to three times lighter than the H.264
stacked files (`../spec/player.md`, "Asking for AV1"). A release made before the API
put stacked files in releases has none; there every clip reports
`clipFailed`, so the engine reports `error` `clip_failed`, skips each move
straight to its target pose, and shows each pose's still
(`transparentThumbnailUrl`) in place of its clips. Releasing the canvas again
fixes it. The demo in `../examples/android` plays a small bundled delivery
that has stacked files.

## Use

```kotlin
val view = MaskoPlayerView(context)            // add it to your layout
val player = view.load(deliveryJson, MaskoPlayer.Options(
    getDelivery = { reply -> api.fetchDelivery { json -> reply(json) } },
))
player.addListener(object : MaskoPlayer.Listener {
    override fun onPose(pose: String) { }
    override fun onError(code: String, detail: String?) { }
})
player.set("working", true)
player.fire("celebrate")
player.set("heldContact", true, instant = true) // now; an Any State start cuts straight in
player.sound = false                           // mutes clips' own voices; on by default
player.pause(); player.play(); player.reset()
player.position()                              // clip on screen and its clip time
player.updateDelivery(freshJson)               // new links for the same release
player.destroy()                               // when you are done with it
```

`set` and `fire` take `instant = true` last: what that value starts then
happens now. An Any State start goes straight into its target pose, skipping
every transition clip and route; a move plays its own clip
(`spec/rules.md` 5.8).

One animation on its own, from its stacked MP4 (a link, or a file inside the
app: `file://`, `asset:///`): `view.loadClip(stackedUrl)`. The player reads
the file's length and sound track and loops it as one pose named `clip`, with
its voice when it has one; a file it cannot read plays nothing and reports
`clip_failed`.

`deliveryJson` is the `delivery` object of
`GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery`. Call everything
on the main thread: a call from another thread throws an
`IllegalStateException` that names the method and the thread. Events arrive
on the main thread from the next main-thread turn on, so listeners added
right after `load` hear the first pose.

- **Events**, the same data as on every platform: `onPose(pose)`,
  `onClip(clip, kind, rate)` (an edge id, or `still:<pose>`; `kind` is
  `POSE`, `MOVE` or `STILL`), `onReactionDone(name, used)`,
  `onMovement(edge, movement, rate, seconds)` (a move's `movement`, for the
  app to move the mascot over `seconds`, the edge's duration at this rate)
  and `onError(code, detail)`.
- **Error codes**: the engine's (`../spec/rules.md`), a refused delivery's
  (`unreadable` for text that is not JSON, `unsupported_contract`,
  `invalid_graph`, ...), and four from the player: `engine_failed` (the
  engine library is missing, or the engine threw: a Rust panic in any call
  is reported, never thrown), `refresh_failed` (`getDelivery` threw, gave
  nothing, gave a delivery that cannot be read, or did not reply within 60
  seconds), `refresh_mismatch` (a delivery of another release) and
  `surface_unavailable` (the view gets no surface to draw into; the detail
  says why: no hardware acceleration, a size of 0, a parent in a software
  layer. The mascot starts if the view gets one later).
- **Position**: `position()` gives `Position(clip, time)`: the edge on
  screen or `still:<pose>` (null before anything shows) and its clip time in
  seconds.
- **The person**: the view does not watch taps or the pointer. The app
  sends them with its own listeners, as it does for dragging and physics:
  `player.set("behavior::pointerHover", on)`, `player.fire("action::click")`,
  and `heldContact`, `heldShake` and `falling`. Each waits for the end of
  the loop on screen unless the app passes `instant = true`.
- **Errors**: `onError`'s `detail` says what it is about: the edge of a clip
  error, the name of a refused value, the pose of a pose error.
- **Sound**: edge sounds play either way; a clip's own voice (`hasAudio`) is
  heard only with `sound` on. The player never takes audio focus.
- **Signed links**: five minutes before `deliveryAccess.signedUrlExpiresAt`
  (halfway there when less than ten minutes are left), the player asks
  `getDelivery` for the same release again and swaps the links only. Files
  are cached by link without its query, so cached files keep playing. After
  a failed refresh it asks again 30 seconds later; a reply that comes after
  60 seconds is ignored. The timer stops in deep sleep, so whenever the view
  is back on screen the player checks the expiry against the wall clock and
  refreshes at once when it is due.
- **New links from the app**: `updateDelivery(json)` takes a fresh delivery
  of the same release at any time, with the same rules: the links only, the
  engine keeps running, clips that failed are tried again. It returns false,
  and reports `refresh_mismatch` or `refresh_failed`, when it cannot take it.
- **Editors**: `player.preview.showPose(id)` and `player.preview.playMove(id)`.
- `autoPlay` false shows the first frame and waits for `play()`. The app's
  `Options.autoPlay` wins over the graph's; left null, the graph decides,
  else the mascot starts at once.
- **Extra clips** are rolled with a random seed of each player, so two
  mascots on one screen do not play the same extras.

## Lifecycle

- **Off screen**: while the view, or any parent, is `GONE` or `INVISIBLE`,
  its window is hidden, or it is detached, the player pauses itself, and
  resumes where it stopped when it is back.
- **Detached** (removed from its parent, or its Activity destroyed), the
  view also frees its two video decoders and the sounds' players, and keeps
  no timer queued. Attached again, it loads the clip it showed at the same
  frame and plays on; looping sounds start again, a one-shot sound that was
  playing is not heard again.
- **Destroy**: call `player.destroy()` when the mascot is gone for good, for
  example in `Activity.onDestroy` or `Fragment.onDestroyView`. It frees the
  engine and cancels preloads and still loads. If an app forgets it, a
  detached view holds nothing alive (no decoder, no thread, no queued task),
  so the Activity is not leaked on rotation; the engine is freed when the
  player is collected.
- A new `view.load(...)` destroys the player loaded before.

## How it plays

- `MaskoPlayer` keeps the play time (stopped while paused), one wake on the
  main thread for the engine's `nextWake()`, and carries out its commands.
- Two persistent slots, each a Media3 ExoPlayer drawing into its own
  `StackedVideoView` (a TextureView with a GL thread and the alpha shader,
  which reads transparency as the luma of the lower half). A new clip loads
  in the hidden slot, paused on its first frame; the swap happens once Media3
  reports `onRenderedFirstFrame` and that frame is drawn in the slot's view,
  and hides the old slot in the same frame. No crossfade. A pose clip shown
  again restarts in place, seeking inside the clip kept in memory (up to a
  minute of played video), so a loop never reads its file again.
- The hidden slot also loads the engine's decided next clip ahead of time
  (or, during a move, the target's main clip), so most swaps happen within a
  few milliseconds of the moment the engine asks.
- `STATE_ENDED` reports `clipEnded`; a player error reports `clipFailed`, or
  marks a clip loaded ahead of time unplayable. An error of the clip a slot
  held before is never blamed on the clip loading into it.
- `preload` puts remote files in a 200 MB disk cache, one folder per process
  (`cacheDir/masko-player/<process name>`). A cache that cannot write (a full
  disk) never fails a clip: the file plays from the network and stays in
  memory for its loops.
- Variants: the smallest `videos.variants` entry with a `stacked` file at
  least as large as the mascot fitted inside the view (its longer side, in
  pixels; square until a clip's size is known), else the base `stacked` file.

## Build

```bash
scripts/build-android.sh                 # from the repository root
cd android && ./gradlew :player:assembleRelease
```

`scripts/build-android.sh` sources `scripts/env.sh` (the Rust toolchain of
`rust-toolchain.toml`) and builds, in `target/android`:

- `libmasko_player_ffi.so` for the three ABIs with the NDK 27 clang (API 24,
  16 KB pages) into `player/src/main/jniLibs/`, with this machine's folders
  remapped (`--remap-path-prefix`), so the libraries name no local path;
- the host library for JVM tests into `player/src/test/engine/`;
- the Kotlin bindings (`uniffi.toml`, package `ai.masko.player.engine`) into
  `player/src/main/generated/`;
- `player/engine.stamp`: a hash of the engine sources (`engine/`,
  `bindings/uniffi*/`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
  `uniffi.toml` and the script), then the hash of each file built from them.

`--host-only` builds the host library and the bindings only, enough for the
JVM tests. These outputs are not in git. Every Gradle build checks the stamp
(`checkEngine`, and `checkEngineLibraries` before packaging and publishing):
files built from other sources, or changed since, fail the build with "Run
scripts/build-android.sh", so a stale engine is never packaged or published.
Rust panics unwind instead of aborting, so an engine bug reaches the player
as an exception (an `engine_failed` event), not a crash.

The build needs Java 17; Gradle downloads it when this machine has none (the
foojay toolchain resolver in `settings.gradle.kts`).

`./gradlew :player:publishReleasePublicationToLocalRepository` writes
`ai.masko:player:0.1.12` to `player/build/repo` (the React Native package
takes it from there; `../scripts/build-react-native.sh` runs it), with its
sources and javadoc jars, signed when a signing key is set.

## Publish to Maven Central

The library publishes through the Central Portal with the
`com.vanniktech.maven.publish.base` plugin: the POM (MIT, MPL-2.0 for the
uniffi crates, the third-party notices, developer Masko, the GitHub
repository), the AAR, sources and javadoc jars, all signed. Once:

1. Create an account on https://central.sonatype.com.
2. Add the namespace `ai.masko` (Publishing, Namespaces) and verify it: the
   portal gives a verification key to publish as a DNS TXT record on
   `masko.ai`; once it is found, the namespace is verified and the record
   can go.
3. Make a GPG key for signing, publish its public key to a key server the
   portal reads (for example `gpg --keyserver keyserver.ubuntu.com --send-keys <key id>`),
   and export the private key: `gpg --export-secret-keys --armor <key id>`.
4. Generate a user token (Account, Generate User Token).
5. Put these in `~/.gradle/gradle.properties` (never in this repository), or
   in the environment as `ORG_GRADLE_PROJECT_<name>`:

   ```properties
   mavenCentralUsername=<token username>
   mavenCentralPassword=<token password>
   signingInMemoryKey=<the exported private key>
   signingInMemoryKeyId=<the last 8 characters of the key id>
   signingInMemoryKeyPassword=<its passphrase>
   ```

   In `gradle.properties` the key goes on one line: the export without its
   first line and last two lines, line breaks removed. In the environment the
   whole export works:
   `export ORG_GRADLE_PROJECT_signingInMemoryKey="$(gpg --export-secret-keys --armor <key id>)"`.
   `signing.keyId`, `signing.password` and `signing.secretKeyRingFile` work
   too, instead of the in-memory key.

A version tag publishes it from `../.github/workflows/release.yml`, with the
other packages (`../RELEASING.md`): the job builds the
engine, runs the tests and lint, and runs `publishAndReleaseToMavenCentral`,
which leaves no step on the portal. It reads the repository secrets
`MAVEN_CENTRAL_USERNAME` and `MAVEN_CENTRAL_PASSWORD` (the user token),
`MAVEN_SIGNING_KEY` (the whole armored export) and
`MAVEN_SIGNING_KEY_PASSWORD`.

By hand:

```bash
scripts/build-android.sh
cd android && ./gradlew :player:testDebugUnitTest :player:lintRelease
./gradlew :player:publishToMavenCentral   # uploads a deployment, not yet public
```

Then check the deployment on https://central.sonatype.com/publishing/deployments
and press Publish (`publishAndReleaseToMavenCentral` publishes without that
step). Without a signing key a Central publication stops at
`signReleasePublication`, before anything is uploaded. A version can be
published once; raise `version` in `player/build.gradle.kts` for the next.

## Tests

```bash
cd android
./gradlew :player:testDebugUnitTest            # JVM, with the host engine library
./gradlew :player:connectedDebugAndroidTest    # a device or emulator
```

- `ScenariosTest`: every `../spec/fixtures/*.json` through the Kotlin
  binding, with a port of the ideal player of `engine/tests/scenarios.rs`.
  `-Pmasko.fixtures=<dir>` runs another folder.
- `MaskoPlayerTest`: the player against a fake video layer: the swap after
  the first frame, loop restarts, ends and failures, missing files, preload,
  sounds and voices, events, position, pause and play, `autoPlay`, signed
  link refresh (and a `getDelivery` that throws or never replies, deep
  sleep), variants, extra clips rolled per player, engine panics, calls off
  the main thread, a view without a surface, a detached view, destroy.
- `DeliveryTest`: what the player reads from a delivery, and the fitted size.
- `MediaFilesTest`: the cache folder per process, cache failures, the
  licenses in the library.
- `RealVideoTest` (instrumented): stacked MP4s from
  `player/src/androidTest/assets/clips` (`make-clips.sh`) play through Media3,
  the shader and both slots; it checks each swap waited for a rendered and
  drawn first frame, that `STATE_ENDED` drives the loops and the move's end,
  the colours and transparency on the real screen (transparency is luma), and
  a view taken off screen and back (decoders freed, then restored).
- `ViewHostTest` (instrumented): a `GONE` view or `INVISIBLE` parent pauses,
  a view with no height or in a software layer reports
  `surface_unavailable`, an earlier clip's error is not blamed on the next,
  an output made while the surface dies is freed, releasing the stage
  cancels preloads and stills.
- `MediaTest` (instrumented): a loop reads its file once (the back buffer),
  and a cache that cannot write still gives the file.

On a Mac without an emulator window:

```bash
$ANDROID_HOME/emulator/emulator -avd <avd> -no-window -no-audio -gpu swiftshader_indirect -no-snapshot-save
```
