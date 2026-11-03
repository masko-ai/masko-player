// A scripted host for the player: virtual time, timers, video slots whose
// frames, ends and failures the test decides, a still, sounds and files.

import type { Format } from '../src/delivery.ts';
import type { FileLoader, Host, Sound, SoundOut, StillLayer, VideoSlot } from '../src/media.ts';

export type Log = string[];

function domError(name: string): Error {
  const e = new Error(name);
  e.name = name;
  return e;
}

export class FakeSlot implements VideoSlot {
  readonly id: number;
  private readonly log: Log;
  private readonly host: FakeHost;
  src: string | null = null;
  time = 0;
  visible = false;
  playing = false;
  rate = 1;
  muted = true;
  decoded = false;
  /** Every new source loaded, in order. */
  loads: string[] = [];
  /** Seeks back to 0 of the same source. */
  seeks = 0;
  /** What the next `play` calls do. */
  refuse: 'none' | 'always' | 'unmuted' = 'none';
  /** The next `play` call rejects with this error name, once. */
  refuseNext: string | null = null;
  /** Called inside `load`, for races. */
  onLoad: (() => void) | null = null;
  private frames = new Set<() => void>();
  private readies = new Set<() => void>();
  private ends = new Set<() => void>();
  private errors = new Set<() => void>();

  constructor(id: number, log: Log, host: FakeHost) {
    this.id = id;
    this.log = log;
    this.host = host;
  }

  load(src: string): void {
    if (this.src === src) {
      this.time = 0;
      this.seeks++;
      this.log.push(`slot${this.id}:seek0`);
      return;
    }
    this.src = src;
    this.loads.push(src);
    this.decoded = false;
    this.time = 0;
    this.log.push(`slot${this.id}:load:${src}`);
    this.onLoad?.();
  }

  play(rate: number, muted: boolean): Promise<void> {
    this.rate = rate;
    this.muted = muted;
    if (this.refuseNext) {
      const name = this.refuseNext;
      this.refuseNext = null;
      return Promise.reject(domError(name));
    }
    if (this.refuse === 'always' || (this.refuse === 'unmuted' && !muted)) {
      return Promise.reject(domError('NotAllowedError'));
    }
    this.playing = true;
    return Promise.resolve();
  }

  pause(): void {
    this.playing = false;
  }

  /** Times the slot was primed inside a gesture. */
  primed = 0;

  prime(): void {
    if (this.playing || this.visible) return;
    this.primed++;
    this.log.push(`slot${this.id}:prime`);
  }

  setVisible(on: boolean): void {
    this.visible = on;
    this.log.push(`slot${this.id}:${on ? 'show' : 'hide'}`);
    this.host.checkScreen();
  }

  onFrame(fn: () => void): () => void {
    this.frames.add(fn);
    return () => this.frames.delete(fn);
  }

  onReady(fn: () => void): () => void {
    if (this.decoded) {
      let live = true;
      queueMicrotask(() => live && fn());
      return () => (live = false);
    }
    this.readies.add(fn);
    return () => this.readies.delete(fn);
  }

  onEnded(fn: () => void): () => void {
    this.ends.add(fn);
    return () => this.ends.delete(fn);
  }

  onError(fn: () => void): () => void {
    this.errors.add(fn);
    return () => this.errors.delete(fn);
  }

  unload(): void {
    this.src = null;
    this.decoded = false;
  }

  /** The decoder has the first frame of the loaded file. */
  frame(): void {
    this.decoded = true;
    const fns = [...this.readies, ...(this.playing ? this.frames : [])];
    this.readies.clear();
    if (this.playing) this.frames.clear();
    for (const fn of fns) fn();
  }

  /** The file plays its last frame. */
  end(): void {
    for (const fn of [...this.ends]) fn();
  }

  /** The file cannot load or decode. */
  fail(): void {
    for (const fn of [...this.errors]) fn();
  }

  get listeners(): number {
    return this.frames.size + this.readies.size + this.ends.size + this.errors.size;
  }
}

export class FakeStill implements StillLayer {
  private readonly log: Log;
  private readonly host: FakeHost;
  visible = false;
  url: string | null = null;
  broken = new Set<string>();

  constructor(log: Log, host: FakeHost) {
    this.log = log;
    this.host = host;
  }

  load(url: string): Promise<void> {
    if (this.broken.has(url)) return Promise.reject(new Error('broken image'));
    this.url = url;
    return Promise.resolve();
  }

  setVisible(on: boolean): void {
    this.visible = on;
    this.log.push(`still:${on ? 'show' : 'hide'}`);
    this.host.checkScreen();
  }
}

export interface FakeSound extends Sound {
  url: string;
  volume: number;
  loop: boolean;
  state: 'playing' | 'paused' | 'stopped' | 'ended';
  finish(): void;
}

export class FakeSounds implements SoundOut {
  played: FakeSound[] = [];

  play(url: string, volume: number, loop: boolean): Sound {
    let ended: (() => void) | null = null;
    const s: FakeSound = {
      url,
      volume,
      loop,
      state: 'playing',
      stop: () => {
        s.state = 'stopped';
      },
      pause: () => {
        if (s.state === 'playing') s.state = 'paused';
      },
      resume: () => {
        if (s.state === 'paused') s.state = 'playing';
      },
      onEnded: (fn) => {
        ended = fn;
      },
      finish: () => {
        s.state = 'ended';
        ended?.();
      }
    };
    this.played.push(s);
    return s;
  }
}

export class FakeFiles implements FileLoader {
  requested: string[] = [];
  freed: string[] = [];
  /** URLs that fail to load. */
  broken = new Set<string>();
  /** Loads that wait until `release()`. */
  hold = false;
  private waiting: (() => void)[] = [];

  load(url: string, signal: AbortSignal): Promise<string> {
    this.requested.push(url);
    const finish = (resolve: (s: string) => void, reject: (e: Error) => void) => {
      if (signal.aborted) reject(new Error('aborted'));
      else if (this.broken.has(url)) reject(new Error(`404 ${url}`));
      else resolve(`blob:${url}`);
    };
    return new Promise((resolve, reject) => {
      if (this.hold) this.waiting.push(() => finish(resolve, reject));
      else finish(resolve, reject);
    });
  }

  release(): void {
    const w = this.waiting;
    this.waiting = [];
    for (const fn of w) fn();
  }

  free(src: string): void {
    this.freed.push(src);
  }
}

interface Timer {
  id: number;
  at: number;
  fn: () => void;
}

export class FakeHost implements Host {
  log: Log = [];
  t = 0;
  wall = Date.parse('2026-10-23T10:00:00Z');
  private timers: Timer[] = [];
  private nextId = 1;
  slots: [FakeSlot, FakeSlot];
  still: FakeStill;
  sounds = new FakeSounds();
  files = new FakeFiles();
  formats: Format[] = ['webm', 'hevc'];
  size = 360;
  overlay = false;
  disposed = false;
  /** Set when a frame shows no mascot after the first one did. */
  emptyScreens = 0;
  private everShown = false;

  constructor() {
    this.slots = [new FakeSlot(0, this.log, this), new FakeSlot(1, this.log, this)];
    this.still = new FakeStill(this.log, this);
  }

  now(): number {
    return this.t;
  }

  wallNow(): number {
    return this.wall;
  }

  setTimer(fn: () => void, ms: number): unknown {
    const id = this.nextId++;
    this.timers.push({ id, at: this.t + Math.max(0, ms), fn });
    return id;
  }

  clearTimer(id: unknown): void {
    this.timers = this.timers.filter((t) => t.id !== id);
  }

  pixels(): number {
    return this.size;
  }

  /** Callbacks waiting for the person's next gesture (`gesture`). */
  private gestures = new Set<() => void>();

  onGesture(fn: () => void): () => void {
    this.gestures.add(fn);
    return () => this.gestures.delete(fn);
  }

  /** The person clicks or presses a key somewhere on the page. */
  gesture(): void {
    const waiting = [...this.gestures];
    this.gestures.clear();
    for (const fn of waiting) fn();
  }

  /** File lengths for `clipLength`, by URL; a URL not here cannot be read. */
  lengths = new Map<string, number>();

  clipLength(url: string): Promise<number> {
    this.log.push(`length:${url}`);
    const seconds = this.lengths.get(url);
    return seconds === undefined ? Promise.reject(new Error(`cannot read ${url}`)) : Promise.resolve(seconds);
  }

  blocked(on: boolean): void {
    this.overlay = on;
    this.log.push(`overlay:${on}`);
  }

  dispose(): void {
    this.disposed = true;
  }

  get pendingTimers(): number {
    return this.timers.length;
  }

  /** Records a moment with nothing on screen after the mascot first showed. */
  checkScreen(): void {
    const shown = this.slots.some((s) => s.visible) || this.still.visible;
    if (shown) this.everShown = true;
    else if (this.everShown && !this.disposed) this.emptyScreens++;
  }

  /** Moves time on by `ms`, running every timer due, in order; the wall clock moves too. */
  async advance(ms: number): Promise<void> {
    const until = this.t + ms;
    for (;;) {
      await settle();
      const due = this.timers.filter((t) => t.at <= until).sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (!due) break;
      this.timers = this.timers.filter((t) => t !== due);
      this.wall += due.at - this.t;
      this.t = Math.max(this.t, due.at);
      due.fn();
    }
    this.wall += until - this.t;
    this.t = until;
    await settle();
  }
}

/** Lets promise callbacks run. */
export async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
  await new Promise((r) => setImmediate(r));
}
