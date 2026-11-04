# What every player does

The engine (`engine/`) holds every rule of `rules.md`. Each platform's player
(`web/`, `apple/`, `android/`, `react-native/`) only plays video and carries
out the engine's commands, the same way everywhere. This page is that
contract.

## The clock

A player keeps one **play time** in whole milliseconds: 0 when the mascot
starts, moving with real time, stopped while the player is paused (rules
11.2). It pauses itself while the mascot cannot be seen: a hidden page on
the web, a view out of its window or an app in the background on iPhone and
iPad, a view off screen on Android, a view out of its window on the Mac.
Every engine call takes the current play time. A player never computes a
rule itself.

## Calls into the engine

| When                                         | Call                                  |
| -------------------------------------------- | ------------------------------------- |
| The delivery is loaded                       | `new(delivery JSON)`; a refusal is an `error` event with its code |
| Before starting, the app sets or fires values | `set` / `fire` with play time 0       |
| The mascot starts (`autoPlay`, or `play()`)  | `start(0)`                            |
| The app calls `set(name, value)`             | `set(name, value, instant, now)`      |
| The app calls `fire(name)`                   | `fire(name, instant, now)`            |
| The engine's `nextWake()` time arrives       | `advance(now)`; then read `nextWake()` again |
| A clip's first frame is on screen            | `clipShown(edge, now)`                |
| The clip on screen plays its last frame      | `clipEnded(edge, now)`                |
| A clip being shown cannot play               | `clipFailed(edge, now)`               |
| A preload fails                              | `markUnplayable(edge)`                |
| A clip marked unplayable loads later         | `markPlayable(edge)`                  |
| The app calls `reset()`                      | `reset(now)`                          |
| An editor calls `preview.showPose` / `playMove` | `previewShowPose` / `previewPlayMove` |

After every call, run the returned commands in order, then schedule a wake
for `nextWake()` (cancel the previous one). A late wake is fine: the engine
runs everything that was due first.

## Commands from the engine

- **`showClip { edge, kind, rate }`**: show this edge's clip from its first
  frame, now, at playback rate `rate`, never backwards.
  - If the same edge is already on screen as a pose clip, this is its next
    loop: restart the same video without a visible cut (seek to 0 and play,
    or let a native loop run on).
  - Otherwise use the other of two persistent video slots: load the clip (or
    take it from the preload cache), wait until its **first frame is
    decoded**, then make it visible and hide the old slot in the same frame.
    No crossfade, ever. Pause the old slot after the swap.
  - Report `clipShown(edge, now)` when this clip's first frame is on screen:
    at the swap, or at once when a pose clip restarts in place. Report it
    once per showing; the engine counts the clip's safety net from it, and
    a clip not reported shown within 10 s counts as failed (rules 9.3).
  - Report `clipEnded(edge, now)` when this clip reaches its end, once per
    showing. Report `clipFailed(edge, now)` if it cannot load or decode.
  - For `kind: move`, tell the app the clip's `movement` (from the delivery
    edge) so it can move the mascot; the player never moves itself.
- **`showStill { pose }`**: show the pose's `transparentThumbnailUrl`
  image, or nothing, in place of video. The engine ends a still by itself.
- **`preload { edge }`**: load that edge's file into a small cache (a few
  clips) so a later `showClip` swaps without waiting.
- **`playSound { edge }`**: play the edge's `sound.url` at `sound.volume`,
  looping when `sound.loop` is true. The engine never asks again for a
  looping sound that is still playing (rules 9.6).
- **`stopSounds { onlyLooping }`**: stop looping sounds, or every sound.
- **`poseEntered { pose }`**: tell the app (`pose` event).
- **`interrupted { edge }`**: the move on screen is cut, by an instant start
  (rules 10.1) or by `reset`, a second `start` or `preview.showPose` (rules
  11.4); the next `showClip` or `showStill` replaces it at once.
- **`spent { name, used }`**: tell the app a fire is done (`reactionDone`
  event, with `used`).
- **`error { code, detail? }`**: tell the app (`error` event). The detail
  says what it is about: the edge of a clip error, the name of a refused
  value, the pose of a pose error.

## The file and the size

From the edge's `videos`, a player picks the file its platform plays with
transparency:

| Where                                       | File |
| ------------------------------------------- | ---- |
| Safari, and any browser on iPhone and iPad   | `hevc` |
| Chrome, Firefox, Edge                        | `webm` (HEVC only on Safari, because some Chrome setups decode HEVC but draw a box) |
| iPhone, iPad and Mac apps                    | `av1` when the clip has one, else `hevc` |
| Android apps                                 | `av1` when the clip has one, else `stacked` |

`stacked` and `av1` are the same picture: colour above, transparency below,
drawn with a shader; `stacked` is encoded with H.264, `av1` with AV1, two to
three times lighter at the same look. A clip's sizes are of the same kind as
its base file. With `videos.variants` (manifest mode), take the smallest size
at least as large as the mascot on screen times the screen density, else the
base file.
If a file fails, a web player tries the other format once before reporting
`clipFailed`.

## Asking for AV1

A delivery names `av1` files only when it is asked for them. Each app player
has `formats`, the formats to add to the delivery request
(`formats=<formats>`): `av1` on Apple devices, which decode AV1 with the
device's AV1 decoder or with dav1d inside the player, and `av1` on Android
phones that have an AV1 decoder (Android 10 and later ship one), else
nothing. The server then gives each clip that has an AV1 file its `av1` in
place of `stacked` or `hevc`, so the player never chooses between files or
tries one after another. A delivery asked without `formats` is the one
players without AV1 have always read.

## One clip

A player also plays one animation on its own, from its file links in place
of a delivery: `webm` and `hevc` on the web, `hevc` or a stacked AV1 file on
iPhone, iPad and Mac, `stacked` or a stacked AV1 file on Android. It reads the file's length from its metadata (on the
web, the preferred format first, then the other one), then plays this
delivery as any other:

```json
{
  "deliverySchemaVersion": 1,
  "playerContract": 1,
  "requires": [],
  "runtimeGraph": {
    "runtimeGraphSchemaVersion": 1,
    "initialNode": "clip",
    "nodes": [{ "id": "clip" }],
    "edges": [{ "id": "clip", "source": "clip", "target": "clip", "isLoop": true,
                "duration": <the file's length in seconds>, "videos": { <the files> },
                "hasAudio": <whether the file has a sound track> }],
    "inputs": []
  }
}
```

The pose and the clip are both `clip`, so the app hears `pose` `clip` and
`clip` `clip` on every loop. Apple and Android read whether the file has a
sound track, so a silent file never takes the device's audio; a web player
cannot tell before it plays, marks `hasAudio: true`, and the browser plays a
file without a sound track silently. Sound on, the file's own voice is heard. The length matters: the engine's safety net
(rules.md 9.3) ends a loop whose end is not reported `duration + 1 s` after
it showed. When no length can be read, nothing plays and the player reports
`error` `clip_failed`. Calls made before the length is read are dropped,
except `play` and `pause`, which decide whether it starts. `getDelivery`
does not apply: the links are the app's.

## A canvas from its URL

An app without a backend gives the player its canvas's URL,
`https://api.masko.ai/v1/canvases/{canvasId}/delivery`, and an **app key**
of its mascot (`masko_app_...`), which can only read that mascot's released
canvases. The player then does what a backend would:

1. **Asks.** `GET` the URL with `target` for its platform and its
   `formats`, `Authorization: Bearer <app key>`, and `Masko-Install-Id`, a
   random UUID the installed app keeps, so the owner can count active copies.
   The answer's `data.delivery` is the canvas's latest release.
2. **Keeps.** Every link of the release's graph (clips, stills, sounds) is
   downloaded once and kept for good, named after the link's path: the path
   never changes, only the signature does. A file is complete or not there.
   The release is kept, for the next launch too, once all its files are.
3. **Plays.** At launch, the kept release plays at once, also offline, and
   the files no kept release plays are deleted first. With nothing kept, the
   first delivery plays from its links while its files are kept; values set
   before it comes choose the first pose, fires are dropped, and a delivery
   that does not come is asked for again 30 seconds later, then twice as
   long each time, up to an hour.
4. **Updates.** It asks again at once and every hour. The same release only
   renews its links (`updateDelivery`). A newer release is checked by the
   engine (a refusal is reported, the release playing goes on) and kept
   whole, then takes over where the initial pose's loop would start again:
   the picture on screen stays until the new release's first clip has
   decoded, its engine starts at play time 0, and the values the app set are
   set again. Before the start, it simply gives the first frame.

A failure to ask or to download is an `error` `refresh_failed`, with what
failed; the player goes on with what it has. `destroy()` stops asking and
leaves the kept files.

On the web the page is not installed, so the player keeps nothing for
offline play: the browser caches the files, a new page load asks again, and
a newer release's mascot shows over the one on screen until its first clip
is there (web/README.md, "A canvas from its URL").

## What the app sees

The same names on every platform:

- `set(name, value)`, `fire(name)`, `reset()`, `play()`, `pause()`,
  `destroy()`, and `position()`: `{ clip, time }`, the clip on screen (an
  edge, or `still:<pose>`) and its clip time in seconds, read only. Before
  anything shows, `clip` is null and `time` 0; a still's `time` is 0.
- `set` and `fire` take an `instant` option, off by default: what that
  value starts happens now; an Any State start goes straight into its target
  pose, skipping every transition clip and route, and a move plays its own
  clip (rules.md 5.8). On the web and in React
  Native it is `{ instant: true }` after the value or name, in Swift
  `instant: true`, in Kotlin `instant = true`.
- `autoPlay`: the app's value wins when it gives one, else the graph's
  `autoPlay`, else true (rules.md 11.1).
- `preload` (web only): `'next'` by default, the preloads the engine asks for
  (rules.md 9.7). `'all'`: once the first clip shows, the player also loads
  every other clip of the release with a file the browser plays, and its
  cache keeps them all. An app that plays a canvas from its URL already
  keeps every file of the release on the device.
- `sound`: on by default. A clip's own voice (`hasAudio`) is heard unless
  the app turns sound off; edge sounds play either way. A browser refuses
  sound before the person's first gesture on the page: a web player then
  plays the clip muted, and asks for its voice again from the next clip or
  loop after that gesture.
- Events, with the same data everywhere:
  - `pose { pose }`: the mascot entered this pose.
  - `clip { clip, kind, rate }`: a clip is on screen. `clip` is the edge, or
    `still:<pose>`; `kind` is `pose`, `move` or `still`; `rate` is its
    playback rate (1 for a still).
  - `reactionDone { name, used }`: a fire is done, used by a start or spent
    unused.
  - `movement { edge, movement, rate, seconds }`: a move whose edge has a
    `movement` is on screen; `seconds` is the edge's `duration` divided by
    `rate`, how long the move lasts.
  - `error { code, detail? }`: a code from rules.md or the player, and a
    detail when there is one. A refused delivery gives the engine's own
    code, so text that is not JSON is `unreadable` everywhere. Only an
    engine that cannot run gives the player's `engine_unavailable` (web) or
    `engine_failed` (Android).
- `preview.showPose(id)` and `preview.playMove(id)`, for editors and tools.
- Signed links expire (`deliveryAccess.signedUrlExpiresAt`). Five minutes
  before that (halfway there when less than ten minutes are left), the player
  asks the app's `getDelivery()` for a fresh delivery of the same release
  and takes it as `updateDelivery` does. When `getDelivery` fails, or gives
  nothing it can take, the player reports it and asks again 30 seconds
  later; links that last no longer than the old ones are asked for again 30
  seconds later too.
- `updateDelivery(delivery)`: the app gives a fresh delivery of the release
  that is playing. The player swaps the URLs only; the engine keeps running,
  files already cached keep playing, and clips that failed are tried again
  from the new links. Another release (another `release.id`, or another
  graph once links are left out) is refused with `error`
  `refresh_mismatch`, a delivery that cannot be read with `refresh_failed`.
  On the web and in React Native, the components call it when their
  `delivery` prop changes to new links of the same release.

## Tests every platform runs

1. **The scenarios, through its binding.** Every `spec/fixtures/*.json` runs
   through the platform's own engine binding with the ideal player of
   `fixtures/README.md`, and must give the same trace as `cargo test`.
2. **The player against a fake video layer.** Command handling (two slots,
   swap only after the first frame, loop restart, end and failure reports,
   preload cache, sounds, events) runs against a scripted fake decoder.
3. **Real video.** At least one real transparent clip plays and swaps on the
   platform's real decoder (a browser, the iOS simulator or macOS, an Android
   emulator or a JVM-side decoder check where an emulator is not available).
