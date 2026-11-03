// The browser side of media.ts: video slots, the still, sounds and the file
// loader for the preload cache.

import type { FileLoader, Sound, SoundOut, StillLayer, VideoSlot } from './media.ts';

const HAVE_CURRENT_DATA = 2;
/** The playback rates Chromium takes; it throws outside them. */
const MIN_RATE = 0.0625;
const MAX_RATE = 16;

type FrameVideo = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: () => void) => number;
  cancelVideoFrameCallback?: (id: number) => void;
};

/** A persistent `<video>`: muted, inline, hidden until swapped in. */
export class DomSlot implements VideoSlot {
  readonly el: FrameVideo;
  src: string | null = null;

  constructor(el: HTMLVideoElement) {
    this.el = el;
    el.muted = true;
    el.defaultMuted = true;
    el.playsInline = true;
    el.preload = 'auto';
    el.loop = false;
    el.setAttribute('muted', '');
    el.setAttribute('playsinline', '');
    el.setAttribute('disablepictureinpicture', '');
    el.setAttribute('disableremoteplayback', '');
    el.setAttribute('aria-hidden', 'true');
    // Page CSS must not turn the swap into a crossfade.
    el.style.transition = 'none';
    this.setVisible(false);
  }

  get time(): number {
    return this.el.currentTime || 0;
  }

  load(src: string): void {
    if (this.src === src) {
      this.el.currentTime = 0;
      return;
    }
    this.src = src;
    this.el.src = src;
  }

  play(rate: number, muted: boolean): Promise<void> {
    try {
      const r = Number.isFinite(rate) && rate > 0 ? Math.min(MAX_RATE, Math.max(MIN_RATE, rate)) : 1;
      this.el.defaultPlaybackRate = r;
      this.el.playbackRate = r;
      this.el.muted = muted;
      return this.el.play() ?? Promise.resolve();
    } catch (error) {
      return Promise.reject(error);
    }
  }

  pause(): void {
    this.el.pause();
  }

  prime(): void {
    const el = this.el;
    if (!el.paused || el.dataset.visible === 'true') return;
    try {
      // Safari lifts its gesture rule for this element on a play() inside a
      // gesture; the pause right after keeps it still.
      void el.play()?.catch(() => undefined);
      el.pause();
    } catch {
      // Nothing to unlock.
    }
  }

  setVisible(on: boolean): void {
    this.el.style.opacity = on ? '1' : '0';
    this.el.dataset.visible = on ? 'true' : 'false';
  }

  onFrame(fn: () => void): () => void {
    const el = this.el;
    const frames = typeof el.requestVideoFrameCallback === 'function';
    let done = false;
    let raf = 0;
    let id = 0;
    const off = () => {
      if (frames) el.cancelVideoFrameCallback?.(id);
      for (const e of events) el.removeEventListener(e, check);
      if (raf) cancelAnimationFrame(raf);
    };
    const fire = () => {
      if (done) return;
      done = true;
      off();
      fn();
    };
    // Without frame callbacks (older Firefox), a frame is decoded once the
    // element plays with current data. With them, the same test once the
    // play position has moved is a backstop for a browser that skips the
    // callback on a hidden video. Either way the swap waits for the next
    // animation frame.
    const check = () => {
      if (raf || el.paused || el.readyState < HAVE_CURRENT_DATA) return;
      if (frames && !(el.currentTime > 0)) return;
      raf = requestAnimationFrame(fire);
    };
    const events = frames ? ['timeupdate'] : ['playing', 'timeupdate', 'loadeddata'];
    for (const e of events) el.addEventListener(e, check);
    if (frames) {
      // Runs in the rendering step that presents the frame: the swap lands
      // in that same frame.
      id = el.requestVideoFrameCallback!(fire);
    } else {
      check();
    }
    return () => {
      done = true;
      off();
    };
  }

  onReady(fn: () => void): () => void {
    const el = this.el;
    let done = false;
    const fire = () => {
      if (done) return;
      done = true;
      el.removeEventListener('loadeddata', fire);
      fn();
    };
    if (el.readyState >= HAVE_CURRENT_DATA) queueMicrotask(fire);
    else el.addEventListener('loadeddata', fire);
    return () => {
      done = true;
      el.removeEventListener('loadeddata', fire);
    };
  }

  onEnded(fn: () => void): () => void {
    this.el.addEventListener('ended', fn);
    return () => this.el.removeEventListener('ended', fn);
  }

  onError(fn: () => void): () => void {
    this.el.addEventListener('error', fn);
    return () => this.el.removeEventListener('error', fn);
  }

  unload(): void {
    this.src = null;
    this.el.removeAttribute('src');
    this.el.load();
  }
}

/** The pose's still image, decoded before it shows. */
export class DomStill implements StillLayer {
  readonly el: HTMLImageElement;
  /** Counts loads: an older image that decodes after a newer one asked for is not shown. */
  private loads = 0;

  constructor(el: HTMLImageElement) {
    this.el = el;
    el.alt = '';
    el.decoding = 'async';
    el.draggable = false;
    el.style.transition = 'none';
    this.setVisible(false);
  }

  async load(url: string): Promise<void> {
    const n = ++this.loads;
    const img = this.el.ownerDocument.createElement('img');
    img.decoding = 'async';
    img.src = url;
    await img.decode();
    if (n !== this.loads) return;
    this.el.src = url;
    // The visible element decodes from the image cache.
    await this.el.decode().catch(() => undefined);
  }

  setVisible(on: boolean): void {
    this.el.style.opacity = on ? '1' : '0';
    this.el.dataset.visible = on ? 'true' : 'false';
  }
}

/** Events that let a page play sound (user activation). */
const GESTURES = ['pointerdown', 'pointerup', 'mousedown', 'keydown', 'touchend', 'click'];

/** Calls `fn` once, on the person's next gesture on the page; returns the cancel. */
export function onGesture(doc: Document, fn: () => void): () => void {
  const off = () => {
    for (const e of GESTURES) doc.removeEventListener(e, once, true);
  };
  const once = () => {
    off();
    fn();
  };
  for (const e of GESTURES) doc.addEventListener(e, once, true);
  return off;
}

/**
 * Edge sounds through `<audio>` elements. When the browser blocks sound
 * until a gesture, a one-shot sound is over (it belongs to its moment, it
 * never plays late), and a looping one starts on the person's next gesture.
 */
export class DomSounds implements SoundOut {
  private readonly doc: Document;

  constructor(doc: Document) {
    this.doc = doc;
  }

  play(url: string, volume: number, loop: boolean): Sound {
    const doc = this.doc;
    const audio = doc.createElement('audio');
    audio.preload = 'auto';
    audio.src = url;
    audio.volume = Number.isFinite(volume) ? Math.min(1, Math.max(0, volume)) : 1;
    audio.loop = loop;
    let stopped = false;
    let paused = false;
    let over = false;
    let ended: (() => void) | null = null;
    const finish = () => {
      if (over) return;
      over = true;
      ended?.();
    };
    const retry = () => {
      disarm();
      if (!stopped && !paused) start();
    };
    const arm = () => {
      for (const e of GESTURES) doc.addEventListener(e, retry, true);
    };
    const disarm = () => {
      for (const e of GESTURES) doc.removeEventListener(e, retry, true);
    };
    const start = () => {
      let playing: Promise<void> | undefined;
      try {
        playing = audio.play();
      } catch (error) {
        playing = Promise.reject(error);
      }
      void playing?.catch((error: unknown) => {
        // Paused or stopped before it started: `resume` starts it again, `stop` ends it.
        if (stopped || paused) return;
        if (!loop) finish();
        else if ((error as { name?: unknown } | null)?.name === 'NotAllowedError') arm();
      });
    };
    audio.addEventListener('ended', finish);
    start();
    return {
      stop: () => {
        stopped = true;
        disarm();
        audio.pause();
        audio.removeAttribute('src');
        audio.load();
      },
      pause: () => {
        paused = true;
        audio.pause();
      },
      resume: () => {
        paused = false;
        if (!stopped && !over && !audio.ended) start();
      },
      onEnded: (fn) => {
        ended = fn;
        if (over) fn();
      }
    };
  }
}

/**
 * Loads whole clip files into memory and plays them from object URLs. When
 * a host does not allow `fetch` from this page (no CORS), it checks that the
 * file loads in a detached `<video>` and lets the browser cache it instead.
 */
export class FetchLoader implements FileLoader {
  async load(url: string, signal: AbortSignal): Promise<string> {
    let response: Response;
    try {
      response = await fetch(url, { signal, credentials: 'omit' });
    } catch (error) {
      if (signal.aborted) throw error;
      await probe(url, signal);
      return url;
    }
    if (!response.ok) throw new Error(`${response.status} ${url}`);
    const blob = await response.blob();
    return URL.createObjectURL(blob);
  }

  free(src: string): void {
    if (src.startsWith('blob:')) URL.revokeObjectURL(src);
  }
}

/**
 * A video file's length in seconds, read from its metadata in a detached
 * `<video>`. Rejects when the file cannot be read within `ms`.
 */
export function clipLength(doc: Document, url: string, ms = 10_000): Promise<number> {
  return new Promise((resolve, reject) => {
    const v = doc.createElement('video');
    v.muted = true;
    v.preload = 'metadata';
    const timer = setTimeout(() => done(NaN), ms);
    const done = (seconds: number) => {
      clearTimeout(timer);
      v.removeAttribute('src');
      v.load();
      if (Number.isFinite(seconds) && seconds > 0) resolve(seconds);
      else reject(new Error(`cannot read the length of ${url}`));
    };
    v.addEventListener('loadedmetadata', () => done(v.duration), { once: true });
    v.addEventListener('error', () => done(NaN), { once: true });
    v.src = url;
  });
}

function probe(url: string, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const v = document.createElement('video');
    v.muted = true;
    v.preload = 'auto';
    const done = (ok: boolean) => {
      v.removeAttribute('src');
      v.load();
      if (ok) resolve();
      else reject(new Error(`cannot load ${url}`));
    };
    v.addEventListener('loadeddata', () => done(true), { once: true });
    v.addEventListener('error', () => done(false), { once: true });
    signal.addEventListener('abort', () => done(false), { once: true });
    v.src = url;
  });
}
