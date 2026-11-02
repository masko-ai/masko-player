// mountMascot: the player in a page element, with the tap-to-play overlay
// (rules.md 14). The player does not watch the person: the app sets
// `behavior::pointerHover` and fires `action::click` itself (rules.md 3.5).

import { playCanvas } from './canvas-url.ts';
import { preferredFormats, type ClipFiles, type DeliveryInput } from './delivery.ts';
import { DomSlot, DomSounds, DomStill, FetchLoader, clipLength, onGesture } from './dom.ts';
import type { Value } from './engine.ts';
import type { Host } from './media.ts';
import {
  Player,
  type MaskoEventName,
  type MaskoListener,
  type PlayerOptions,
  type Position,
  type ValueOptions
} from './player.ts';

/**
 * What the player plays: a release `delivery`; a canvas from its `url` with
 * an `appKey`, whose newest release it asks for and follows; or one
 * animation's `clip` files (`webm` and `hevc`), looped on its own.
 */
export type MountOptions = MountCommon &
  (
    | { delivery: DeliveryInput; clip?: undefined; url?: undefined }
    | {
        /** The canvas's URL: `https://api.masko.ai/v1/canvases/{canvasId}/delivery`. */
        url: string;
        /** An app key of the canvas's mascot (`masko_app_...`), safe in a page. */
        appKey: string;
        delivery?: undefined;
        clip?: undefined;
      }
    | { clip: ClipFiles; delivery?: undefined; url?: undefined }
  );

interface MountCommon {
  /** Returns a fresh delivery of the same release, before signed links expire. */
  getDelivery?: () => DeliveryInput | Promise<DeliveryInput>;
  /** Lets clips with their own voice be heard. On by default; false mutes them. Edge sounds play either way. */
  sound?: boolean;
  /** Starts at once. Defaults to the canvas's `autoPlay`, else true. */
  autoPlay?: boolean;
  /**
   * @deprecated Ignored: the player no longer watches the pointer. The app
   * sets `behavior::pointerHover` and fires `action::click` itself.
   */
  pointer?: boolean;
}

/** What `mountMascot` returns: the same calls on every platform. */
export interface MaskoHandle {
  /** `{ instant: true }`: what it starts happens now; an Any State start cuts straight in. */
  set(name: string, value: Value, options?: ValueOptions): void;
  fire(name: string, options?: ValueOptions): void;
  reset(): void;
  play(): void;
  pause(): void;
  destroy(): void;
  /** The clip on screen and its clip time in seconds. */
  position(): Position;
  /**
   * Swaps in fresh links of the release that is playing; the mascot keeps
   * playing. Another release is refused with `error` `refresh_mismatch`, a
   * delivery that cannot be read with `refresh_failed`. True when taken.
   */
  updateDelivery(delivery: DeliveryInput): boolean;
  /** Lets clips' own voices be heard, or not, from the next clip or loop on. */
  setSound(on: boolean): void;
  /** Listens to `pose`, `clip`, `reactionDone`, `movement` or `error`; returns the unsubscribe call. */
  on<K extends MaskoEventName>(event: K, handler: MaskoListener<K>): () => void;
  preview: { showPose(id: string): void; playMove(id: string): void };
}

const STYLE_ID = 'masko-player-style';
const CSS = `
.masko-player{position:relative;width:100%;height:100%;overflow:visible;-webkit-tap-highlight-color:transparent;user-select:none;-webkit-user-select:none}
.masko-player>video,.masko-player>img.masko-still{position:absolute;inset:0;width:100%;height:100%;object-fit:contain;pointer-events:none;margin:0;padding:0;border:0;background:transparent;transition:none}
.masko-player>.masko-tap{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;margin:0;padding:0;border:0;background:transparent;cursor:pointer}
.masko-player>.masko-tap[hidden]{display:none}
.masko-player>.masko-tap>span{display:flex;align-items:center;justify-content:center;width:48px;height:48px;border-radius:50%;background:rgba(0,0,0,.55);box-shadow:0 2px 8px rgba(0,0,0,.25)}
`;

function ensureStyle(doc: Document): void {
  if (doc.getElementById(STYLE_ID)) return;
  const style = doc.createElement('style');
  style.id = STYLE_ID;
  style.textContent = CSS;
  (doc.head ?? doc.documentElement).appendChild(style);
}

/**
 * Plays a mascot in `element`, which sets its size. Returns at once; the
 * engine loads in the background and calls made meanwhile are kept.
 */
export function mountMascot(element: HTMLElement, options: MountOptions): MaskoHandle {
  if (options.url) return mountCanvas(element, options.url, options.appKey, options);
  return handleFor(createPlayer(element, options).player);
}

const INSTALL_KEY = 'masko-install-id';

/**
 * A canvas from its URL: one player per release, each in its own root. A
 * newer release's root shows over the one on screen until its first clip is
 * there, then takes its place.
 */
function mountCanvas(element: HTMLElement, url: string, appKey: string, options: MountCommon): MaskoHandle {
  const win = element.ownerDocument.defaultView ?? window;
  let shown: HTMLElement | null = null;
  return playCanvas(
    { url, appKey },
    {
      fetch: (target, init) => win.fetch(target, init),
      setTimer: (fn, ms) => win.setTimeout(fn, ms),
      clearTimer: (id) => win.clearTimeout(id as number),
      installId: () => {
        try {
          const store = win.localStorage;
          let id = store.getItem(INSTALL_KEY);
          if (!id) {
            id = win.crypto.randomUUID();
            store.setItem(INSTALL_KEY, id);
          }
          return id;
        } catch {
          return null;
        }
      },
      makePlayer: (delivery, getDelivery, over) => {
        const below = over ? shown : null;
        const made = createPlayer(element, {
          delivery,
          getDelivery,
          sound: options.sound,
          autoPlay: options.autoPlay
        });
        const style = made.root.style;
        if (below) {
          style.position = 'absolute';
          style.left = `${below.offsetLeft}px`;
          style.top = `${below.offsetTop}px`;
          style.width = `${below.offsetWidth}px`;
          style.height = `${below.offsetHeight}px`;
        } else shown = made.root;
        // The size the old root found for an element without a height.
        const height = below?.style.height ?? '';
        const aspectRatio = below?.style.aspectRatio ?? '';
        return {
          player: made.player,
          settle: () => {
            if (!below) return;
            for (const name of ['position', 'left', 'top', 'width']) style.removeProperty(name);
            style.height = height;
            style.aspectRatio = aspectRatio;
            shown = made.root;
          }
        };
      }
    },
    { sound: options.sound }
  );
}

/** The player and its DOM, for the React component. */
export function createPlayer(element: HTMLElement, options: MountOptions): { player: Player; root: HTMLElement } {
  const doc = element.ownerDocument;
  const win = doc.defaultView ?? window;
  ensureStyle(doc);

  const root = doc.createElement('div');
  root.className = 'masko-player';
  const videos = [doc.createElement('video'), doc.createElement('video')];
  videos.forEach((v, i) => {
    v.dataset.slot = String(i);
    root.appendChild(v);
  });
  const img = doc.createElement('img');
  img.className = 'masko-still';
  root.appendChild(img);
  const tap = doc.createElement('button');
  tap.type = 'button';
  tap.className = 'masko-tap';
  tap.hidden = true;
  tap.setAttribute('aria-label', 'Play');
  tap.innerHTML =
    '<span><svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true"><path d="M6 4l10 6-10 6z" fill="#fff"/></svg></span>';
  root.appendChild(tap);
  element.appendChild(root);
  const cleanups: (() => void)[] = [];

  // An element without a height: a square mascot as wide as it is. A hidden
  // element has no size at all yet: it is measured once it shows.
  const square = (): boolean => {
    const r = root.getBoundingClientRect();
    if (r.width === 0) return false;
    if (r.height === 0) {
      root.style.height = 'auto';
      root.style.aspectRatio = '1 / 1';
    }
    return true;
  };
  if (!square() && typeof win.ResizeObserver === 'function') {
    const observer = new win.ResizeObserver(() => {
      if (square()) observer.disconnect();
    });
    observer.observe(root);
    cleanups.push(() => observer.disconnect());
  }

  const slots: [DomSlot, DomSlot] = [new DomSlot(videos[0]), new DomSlot(videos[1])];
  const nav = win.navigator;
  const timers = new Set<number>();
  const host: Host = {
    now: () => win.performance.now(),
    wallNow: () => Date.now(),
    setTimer: (fn, ms) => {
      const id = win.setTimeout(() => {
        timers.delete(id);
        fn();
      }, ms);
      timers.add(id);
      return id;
    },
    clearTimer: (id) => {
      win.clearTimeout(id as number);
      timers.delete(id as number);
    },
    slots,
    still: new DomStill(img),
    sounds: new DomSounds(doc),
    files: new FetchLoader(),
    formats: preferredFormats(nav.userAgent, nav.maxTouchPoints ?? 0),
    pixels: () => {
      const r = root.getBoundingClientRect();
      return Math.max(r.width, r.height) * (win.devicePixelRatio || 1);
    },
    clipLength: (url) => clipLength(doc, url),
    onGesture: (fn) => onGesture(doc, fn),
    blocked: (on) => {
      tap.hidden = !on;
    },
    dispose: () => {
      for (const c of cleanups) c();
      for (const id of timers) win.clearTimeout(id);
      timers.clear();
      root.remove();
    }
  };

  const playerOptions: PlayerOptions = {
    delivery: options.delivery,
    clip: options.clip,
    getDelivery: options.getDelivery,
    sound: options.sound,
    autoPlay: options.autoPlay
  };
  const player = new Player(host, playerOptions);

  const listen = (target: EventTarget, type: string, fn: (e: Event) => void) => {
    target.addEventListener(type, fn);
    cleanups.push(() => target.removeEventListener(type, fn));
  };
  listen(tap, 'click', (e) => {
    e.stopPropagation();
    player.play();
  });
  listen(doc, 'visibilitychange', () => player.setHidden(doc.visibilityState === 'hidden'));
  // Timers may have slept with the device: check the signed links again.
  listen(win, 'online', () => player.checkLinks());
  if (doc.visibilityState === 'hidden') player.setHidden(true);
  return { player, root };
}

/** The app's handle on a player. */
export function handleFor(player: Player): MaskoHandle {
  return {
    set: (name, value, options) => player.set(name, value, options),
    fire: (name, options) => player.fire(name, options),
    reset: () => player.reset(),
    play: () => player.play(),
    pause: () => player.pause(),
    destroy: () => player.destroy(),
    position: () => player.position(),
    updateDelivery: (delivery) => player.updateDelivery(delivery),
    setSound: (on) => player.setSound(on),
    on: (event, handler) => player.on(event, handler),
    preview: player.preview
  };
}
