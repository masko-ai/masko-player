# @masko/player for the web

Plays a Masko mascot release in any web page, with transparency. Every
canvas rule runs in the shared Rust engine
([`engine/`](https://github.com/masko-ai/masko-player/tree/main/engine)),
compiled to WebAssembly; this package only plays video and carries out the
engine's commands, as
[`spec/player.md`](https://github.com/masko-ai/masko-player/blob/main/spec/player.md)
describes.

```bash
npm install @masko/player
```

It runs in Chrome and Edge 87, Firefox 79, and Safari 15 (iOS 15) or later:
the engine's WebAssembly needs bulk memory and non-trapping float-to-int
conversions.

It runs unchanged in Electron apps, including in a transparent window as a
desktop mascot:
[`examples/electron`](https://github.com/masko-ai/masko-player/tree/main/examples/electron)
shows one, with the Content Security Policy the player needs
(`'wasm-unsafe-eval'`, and the clips' host in `connect-src` and `media-src`
with `blob:`).

## Use

```ts
import { mountMascot } from '@masko/player';

const mascot = mountMascot(document.getElementById('mascot')!, {
  delivery, // the release delivery object, its JSON, or the API response around it
  getDelivery: () => fetch('/my-api/delivery').then((r) => r.json())
});
mascot.on('pose', ({ pose }) => console.log(pose));
mascot.set('working', true);
mascot.fire('celebrate');
mascot.set('heldContact', true, { instant: true }); // now; an Any State start cuts straight in
```

The element sets the mascot's size; one without a height gets a square
mascot as wide as it is. `mountMascot` returns at once; the engine loads in
the background and calls made meanwhile are kept.

With React (17 or later):

```tsx
import { MaskoPlayer, type MaskoHandle } from '@masko/player/react';

const ref = useRef<MaskoHandle>(null);
<MaskoPlayer ref={ref} delivery={delivery} style={{ width: 240, height: 240 }} onPose={(e) => setPose(e.pose)} />;
ref.current?.fire('celebrate');
```

A `delivery` with new links of the same release swaps the links and keeps
playing. A delivery of another release mounts a new mascot: it starts again
from the canvas defaults, so set your values again (in `onPose`, for
example). Event props and listeners added with `ref.current.on` keep
hearing the new mascot.

Without a build step, one file with the engine inlined,
`dist/masko-player.js`: from a CDN, or copied from
`node_modules/@masko/player/dist/` next to your page. It is a classic
script that defines `window.MaskoPlayer`, not a module to import.

```html
<script src="https://cdn.jsdelivr.net/npm/@masko/player@0.1.13/dist/masko-player.js"></script>
<script>
  const mascot = MaskoPlayer.mount(document.getElementById('mascot'), { delivery });
</script>
```


### A canvas from its URL

A page or an app without a backend gives the player its canvas's URL and an
**app key** of its mascot (`masko_app_...`). The key can only read that
mascot's released canvases, so it is safe in a page:

```ts
mountMascot(element, {
  url: 'https://api.masko.ai/v1/canvases/CANVAS_ID/delivery',
  appKey: 'masko_app_...'
});
// React: <MaskoPlayer url="https://api.masko.ai/v1/canvases/CANVAS_ID/delivery" appKey="masko_app_..." />
// Script tag: MaskoPlayer.mount(element, { url, appKey })
```

The player asks for the canvas's newest release (`target=web`, with a
`Masko-Install-Id` the browser keeps, so the owner can count active copies),
plays it, renews its links from that release's own URL, and asks again every
hour. A newer release takes over where the initial pose's loop starts again:
its mascot shows over the one on screen until its first clip is there.
Values set before the first release comes choose its first pose; fires are
dropped. When the first ask fails, it asks again after 30 seconds, then
twice as long each time, up to an hour, reporting `error` `refresh_failed`.
The browser keeps the files in its own cache: nothing is kept for offline
play on the web.

### One animation

To play one animation on its own, give its files in place of a delivery:

```ts
mountMascot(element, { clip: { webm: 'https://…/wave.webm', hevc: 'https://…/wave.mov' } });
// React: <MaskoPlayer clip={{ webm, hevc }} />; script tag: MaskoPlayer.mount(element, { clip })
```

The player picks the file the browser shows with transparency, reads its
length, and loops it as one pose named `clip`, with its own voice when the
file has one. A clip whose length cannot be read plays nothing and reports
`error` `clip_failed`.

## What the app can call

- `set(name, value)` with a boolean or a number, `fire(name)`. Values set
  before the start choose the first pose, also the first frame shown with
  `autoPlay` false. Both take `{ instant: true }` last: what that value
  starts then happens now; an Any State start goes straight into its target
  pose, skipping every transition clip and route, and a move plays its own
  clip (`spec/rules.md` 5.8).
- `play()`, `pause()`, `reset()`, `destroy()`, `position()`, and
  `preview.showPose(id)` / `preview.playMove(id)` (editors only).
- `updateDelivery(delivery)`: fresh links of the release that is playing.
  Another release is refused with `error` `refresh_mismatch`.
- `setSound(on)`: lets clips' own voices be heard, or not, from the next
  clip or loop on. Browsers refuse sound before the person's first click or
  key press on the page: the clip then plays muted, and its voice is heard
  from the next clip or loop after that.
- Events: `pose`, `clip`, `reactionDone`, `movement`, `error` (its `detail`
  says what it is about: the edge of a clip error, the name of a refused
  value, the pose of a pose error).
- Options: `sound` (on by default: a clip's own voice is heard; false mutes
  it; edge sounds play either way) and `autoPlay` (defaults to the canvas).
- The person: the player does not watch the pointer, clicks or keys. The app
  sends what the person does to the mascot, like any other value, and adds
  `{ instant: true }` when it should cut the loop:

  ```js
  el.addEventListener('pointerenter', () => mascot.set('behavior::pointerHover', true));
  el.addEventListener('pointerleave', () => mascot.set('behavior::pointerHover', false));
  el.addEventListener('click', () => mascot.fire('action::click'));
  ```

Calls made inside an event handler run once the player has finished what
told the handler. While the mascot is paused or the page is hidden, calls
wait for play: the last `set` of each name and one `fire` of each name (one
more when it is fired with `instant`) are kept, at most 1000 calls.

## What the player does by itself

- Picks the file: HEVC on Safari, in WebKit views of Mac apps (WKWebView, as
  in Tauri) and on every iPhone and iPad browser, WebM elsewhere, and the
  other one once if a file fails. With size variants, the smallest one at
  least as large as the mascot on screen.
- Two video elements: the next clip swaps in only once its first frame is
  decoded, so there is always a mascot on screen and never a crossfade.
- Preloads what the engine asks for, plays edge sounds, and pauses while the
  page is hidden. When the browser blocks sound until the person interacts,
  a one-shot sound is skipped and a looping one starts on their first click
  or key press.
- Shows a tap-to-play button when the browser blocks autoplay.
- Asks `getDelivery` for fresh signed links before they expire and swaps the
  links only; the mascot keeps playing. It checks again when the page shows
  after the device slept and when it comes back online, and never asks more
  often than every 30 seconds.

## The engine's WebAssembly

The ES module build loads `masko_player_wasm_bg.wasm` next to itself;
bundlers such as Vite, webpack and Next.js copy it into your build. To host it elsewhere, call
`setWasmSource(url)` before the first mascot mounts.

When the file next to it does not load, the player loads a copy of the
engine inlined in a JavaScript chunk instead (about 100 KB more, gzipped).
That happens in Vite's dev server, which pre-bundles dependencies away from
their files and answers with its page. To load the `.wasm` there too:

```js
// vite.config.js
export default { optimizeDeps: { exclude: ['@masko/player'] } };
```

With a Content Security Policy, allow:

- `script-src 'wasm-unsafe-eval'` for the engine (`'unsafe-eval'` before
  Chrome 97, Firefox 102 and Safari 16);
- `style-src 'unsafe-inline'` for the small `<style id="masko-player-style">`
  element the player adds to the page;
- your clip host and `blob:` in `media-src`, the clip host in `connect-src`
  (preloading), and the still images' host in `img-src`.

## Build and test

From a clone of [masko-player](https://github.com/masko-ai/masko-player)
([Set up](https://github.com/masko-ai/masko-player#set-up) first):

```bash
npm install
npx playwright install chromium     # once, for the browser tests
../scripts/build-web.sh --package   # the engine, then dist/
npm run typecheck
npm test                            # the scenarios through WebAssembly, player and unit tests
npx playwright test                 # real transparent video in Chromium
npm pack                            # a .tgz to install in an app
```

The browser tests make their clips with ffmpeg, which needs libvpx-vp9.
