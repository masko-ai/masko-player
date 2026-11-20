// The browser side (dom.ts) against small fake elements: playback rates and
// volumes a browser refuses, stills that decode out of order, and edge
// sounds when the browser blocks sound until a gesture.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DomSlot, DomSounds, DomStill } from '../src/dom.ts';
import { settle } from './fakes.ts';

class Target {
  private readonly listeners = new Map<string, Set<(e: unknown) => void>>();

  addEventListener(type: string, fn: (e: unknown) => void): void {
    let set = this.listeners.get(type);
    if (!set) this.listeners.set(type, (set = new Set()));
    set.add(fn);
  }

  removeEventListener(type: string, fn: (e: unknown) => void): void {
    this.listeners.get(type)?.delete(fn);
  }

  dispatch(type: string): void {
    for (const fn of [...(this.listeners.get(type) ?? [])]) fn({ type });
  }

  get listening(): number {
    let n = 0;
    for (const set of this.listeners.values()) n += set.size;
    return n;
  }
}

function domError(name: string): Error {
  const e = new Error(name);
  e.name = name;
  return e;
}

/** An `<audio>` that refuses to play while its document blocks sound, and throws on a volume out of range, as browsers do. */
class FakeAudio extends Target {
  private readonly doc: FakeDoc;
  src = '';
  preload = '';
  loop = false;
  ended = false;
  paused = true;
  plays = 0;
  private v = 1;

  constructor(doc: FakeDoc) {
    super();
    this.doc = doc;
  }

  get volume(): number {
    return this.v;
  }

  set volume(v: number) {
    if (!Number.isFinite(v)) throw new TypeError('The provided double value is non-finite.');
    if (v < 0 || v > 1) throw domError('IndexSizeError');
    this.v = v;
  }

  play(): Promise<void> {
    this.plays++;
    if (this.doc.blocked) return Promise.reject(domError('NotAllowedError'));
    this.paused = false;
    return Promise.resolve();
  }

  pause(): void {
    this.paused = true;
  }

  removeAttribute(): void {
    this.src = '';
  }

  load(): void {}
}

/** An image whose decode the test finishes, in any order. */
class FakeImage {
  readonly ownerDocument: FakeDoc;
  src = '';
  alt = '';
  decoding = '';
  draggable = true;
  style: Record<string, string> = {};
  dataset: Record<string, string> = {};
  finish: () => void = () => undefined;

  constructor(doc: FakeDoc) {
    this.ownerDocument = doc;
  }

  decode(): Promise<void> {
    return new Promise((resolve) => (this.finish = resolve));
  }
}

class FakeDoc extends Target {
  blocked = true;
  audios: FakeAudio[] = [];
  images: FakeImage[] = [];

  createElement(tag: string): unknown {
    if (tag === 'audio') {
      const a = new FakeAudio(this);
      this.audios.push(a);
      return a;
    }
    const img = new FakeImage(this);
    this.images.push(img);
    return img;
  }
}

function sounds(doc: FakeDoc): DomSounds {
  return new DomSounds(doc as unknown as Document);
}

test('sounds: a one-shot sound blocked until a gesture is over, and never plays late', async () => {
  const doc = new FakeDoc();
  const out = sounds(doc);
  let ended = 0;
  const tick = out.play('tick.mp3', 1, false);
  tick.onEnded(() => ended++);
  await settle();
  assert.equal(ended, 1, 'over at once, so the player drops it');
  doc.blocked = false;
  doc.dispatch('pointerdown');
  tick.resume();
  await settle();
  assert.equal(doc.audios[0].plays, 1, 'not tried again after a gesture or a resume');
  assert.equal(doc.listening, 0);
  // Ended before the player listened: it still hears it.
  doc.blocked = true;
  const late = out.play('tick.mp3', 1, false);
  await settle();
  late.onEnded(() => ended++);
  assert.equal(ended, 2);
});

test('sounds: a looping sound blocked at load starts on the next gesture, unless paused or stopped', async () => {
  const doc = new FakeDoc();
  const out = sounds(doc);
  const hum = out.play('hum.mp3', 0.5, true);
  hum.onEnded(() => assert.fail('a loop never ends'));
  await settle();
  const audio = doc.audios[0];
  assert.equal(audio.paused, true);
  doc.blocked = false;
  doc.dispatch('click');
  await settle();
  assert.equal(audio.paused, false, 'heard after the first click');
  assert.equal(doc.listening, 0, 'the gesture listeners are gone');
  // Blocked again, then paused by the player: a gesture does not start it, resume does.
  doc.blocked = true;
  const held = out.play('hum.mp3', 0.5, true);
  await settle();
  held.pause();
  doc.blocked = false;
  doc.dispatch('keydown');
  await settle();
  assert.equal(doc.audios[1].paused, true, 'the player holds it');
  held.resume();
  await settle();
  assert.equal(doc.audios[1].paused, false);
  // Stopped while waiting for a gesture.
  doc.blocked = true;
  const gone = out.play('hum.mp3', 0.5, true);
  await settle();
  gone.stop();
  doc.blocked = false;
  doc.dispatch('pointerdown');
  await settle();
  assert.equal(doc.audios[2].plays, 1);
  assert.equal(doc.listening, 0);
});

test('sounds: a volume a browser would throw on plays clamped', () => {
  const doc = new FakeDoc();
  doc.blocked = false;
  const out = sounds(doc);
  for (const [volume, heard] of [
    [Number.NaN, 1],
    [2, 1],
    [-1, 0],
    [0.3, 0.3]
  ]) {
    out.play('s.mp3', volume, false);
    assert.equal(doc.audios.at(-1)!.volume, heard, `volume ${volume}`);
  }
});

test('still: an older image that decodes after a newer one asked for is not shown', async () => {
  const doc = new FakeDoc();
  const el = new FakeImage(doc);
  const still = new DomStill(el as unknown as HTMLImageElement);
  assert.equal(el.style.transition, 'none');
  const older = still.load('old.png');
  const newer = still.load('new.png');
  const [oldImg, newImg] = doc.images;
  newImg.finish();
  await settle();
  el.finish();
  await newer;
  assert.equal(el.src, 'new.png');
  oldImg.finish();
  await older;
  assert.equal(el.src, 'new.png', 'the slow old image does not replace it');
});

/** A `<video>` that throws on playback rates outside Chromium's 0.0625 to 16. */
class FakeVideo extends Target {
  muted = false;
  defaultMuted = false;
  playsInline = false;
  preload = '';
  loop = true;
  style: Record<string, string> = {};
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>();
  private rate = 1;
  private defaultRate = 1;

  private check(r: number): number {
    if (!(r >= 0.0625 && r <= 16)) throw domError('NotSupportedError');
    return r;
  }

  get playbackRate(): number {
    return this.rate;
  }

  set playbackRate(r: number) {
    this.rate = this.check(r);
  }

  get defaultPlaybackRate(): number {
    return this.defaultRate;
  }

  set defaultPlaybackRate(r: number) {
    this.defaultRate = this.check(r);
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  play(): Promise<void> {
    return Promise.resolve();
  }
}

test('slot: a playback rate the browser refuses is clamped, never thrown', async () => {
  const el = new FakeVideo();
  const slot = new DomSlot(el as unknown as HTMLVideoElement);
  assert.equal(el.style.transition, 'none', 'page CSS cannot crossfade the swap');
  for (const [rate, played] of [
    [40, 16],
    [0.01, 0.0625],
    [Number.NaN, 1],
    [-2, 1],
    [1.5, 1.5]
  ]) {
    await slot.play(rate, true);
    assert.equal(el.playbackRate, played, `rate ${rate}`);
    assert.equal(el.defaultPlaybackRate, played);
  }
});

/** A window whose animation frames run when the test says. */
function frames() {
  let queue: Array<() => void> = [];
  const view = {
    requestAnimationFrame(fn: () => void): number {
      queue.push(fn);
      return queue.length;
    }
  };
  return {
    doc: { defaultView: view },
    next(): void {
      const run = queue;
      queue = [];
      for (const fn of run) fn();
    }
  };
}

test('slot: a slot swapped out stays drawn two frames, so a swap never shows an empty frame', () => {
  const f = frames();
  const el = Object.assign(new FakeVideo(), { ownerDocument: f.doc, src: '', currentTime: 0 });
  const slot = new DomSlot(el as unknown as HTMLVideoElement);
  assert.equal(el.style.opacity, '0', 'a new slot starts hidden, at once');
  slot.setVisible(true);
  assert.equal(el.style.opacity, '1');
  slot.setVisible(false);
  assert.equal(el.dataset.visible, 'false', 'hidden for the player at once');
  f.next();
  assert.equal(el.style.opacity, '1', 'still drawn one frame later');
  f.next();
  assert.equal(el.style.opacity, '0', 'gone two frames later');

  slot.setVisible(false);
  slot.setVisible(true);
  f.next();
  f.next();
  assert.equal(el.style.opacity, '1', 'shown again before the hide: it stays');

  slot.setVisible(false);
  slot.load('next.webm');
  assert.equal(el.style.opacity, '0', 'a slot loading its next clip hides at once');
});
