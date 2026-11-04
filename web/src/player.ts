// The player around the engine (spec/player.md): one play-time clock, one
// wake timer, two persistent video slots, a still, sounds and a preload
// cache. It carries out the engine's commands and reports what the video
// does; it never decides what plays.

import { ClipCache } from './cache.ts';
import { PlayClock } from './clock.ts';
import {
  chooseFiles,
  clipDelivery,
  expiresAt,
  fetchedAt,
  indexDelivery,
  readDelivery,
  sameRelease,
  type ClipFiles,
  type Delivery,
  type DeliveryEdge,
  type DeliveryInput,
  type DeliveryNode,
  type FileChoice
} from './delivery.ts';
import { Engine, LoadError, loadWasm, type ClipKind, type Command, type Value } from './engine.ts';
import type { Host, Sound } from './media.ts';

/** What the app hears (spec/player.md, "What the app sees"). */
export interface MaskoEvents {
  /** The mascot entered a pose. */
  pose: { pose: string };
  /** A clip, or a pose's still (`still:<pose>`), is now on screen. */
  clip: { clip: string; kind: ClipKind | 'still'; rate: number };
  /** A fire is done: used by a start, or spent unused. `name` is the full name, as `action::boop`. */
  reactionDone: { name: string; used: boolean };
  /** A move with `movement` is now on screen: the app may move the mascot. */
  movement: { edge: string; movement: unknown; rate: number; seconds: number };
  /**
   * Something went wrong, with a code from rules.md or the player and what it is about: the clip
   * of a clip error, the name of a refused value, the pose of a pose error.
   */
  error: { code: string; detail?: string };
}

/** Options of `set` and `fire`. */
export interface ValueOptions {
  /**
   * What this value starts happens now: an Any State start goes straight into its target pose,
   * skipping every transition clip and route, and a move plays its own clip (rules.md 5.8). Off
   * by default.
   */
  instant?: boolean;
}

export type MaskoEventName = keyof MaskoEvents;
export type MaskoListener<K extends MaskoEventName> = (event: MaskoEvents[K]) => void;

export interface PlayerOptions {
  /** The release delivery, its JSON, or the API response around it. */
  delivery?: DeliveryInput;
  /** One animation's files, played on its own in a loop, in place of `delivery`. */
  clip?: ClipFiles;
  /** Returns a fresh delivery of the same release, for signed links that expire. */
  getDelivery?: () => DeliveryInput | Promise<DeliveryInput>;
  /** Lets clips with their own voice (`hasAudio`) be heard. On by default; false mutes them. */
  sound?: boolean;
  /** Starts at once. Defaults to the canvas's `autoPlay`, else true. */
  autoPlay?: boolean;
  /**
   * What the player downloads ahead. `'next'` (the default): every clip that
   * can play next from the pose on screen. `'all'`: those first, then, once
   * the mascot shows, every other clip of the release, kept for the visit.
   */
  preload?: 'next' | 'all';
  /** Seeds the extra-clip rolls; random by default. */
  seed?: number;
}

/** The clip on screen and its clip time, in seconds of its file. */
export interface Position {
  clip: string | null;
  time: number;
}

type Call = (engine: Engine, now: number) => Command[];
type Hold = 'app' | 'blocked' | 'hidden';

/**
 * A call kept for later, with the name it sets (`set:<name>`) or fires (`fire:<name>`, or
 * `fire:<name>:instant`), if any.
 */
interface Kept {
  fn: Call;
  key?: string;
}

/** One showing of one clip in one slot. */
interface Showing {
  edge: string;
  kind: ClipKind;
  rate: number;
  slot: 0 | 1;
  /** The show command it answers (`showToken`); 0 when the engine did not ask for it. */
  ask: number;
  files: FileChoice[];
  tried: number;
  /** The file of this attempt is in the slot (`load` may wait for the cache first). */
  loaded: boolean;
  muted: boolean;
  /** The browser refused this clip's voice: its loops stay muted until the app turns sound on again. */
  voiceRefused: boolean;
  /** Its end was reported for this loop. */
  ended: boolean;
  cancelled: boolean;
  off: (() => void)[];
}

type Screen = { type: 'clip'; showing: Showing } | { type: 'still'; pose: string } | { type: 'none' };

/** Retry delay after a failed refresh, and the soonest a refresh repeats. */
const REFRESH_RETRY_MS = 30_000;
/** Refresh this long before signed links expire, or halfway when they expire sooner than twice this. */
const REFRESH_LEAD_MS = 5 * 60_000;
/** The most calls kept while the engine loads or the mascot is paused; the oldest go first. */
const MAX_KEPT = 1000;
/** App calls that event handlers chain in one go; the rest waits for the next task. */
const MAX_CHAINED = 100;

export class Player {
  private readonly host: Host;
  private readonly options: PlayerOptions;
  private readonly clock: PlayClock;
  private readonly cache: ClipCache;
  private readonly listeners = new Map<MaskoEventName, Set<(event: never) => void>>();

  private delivery: Delivery | null = null;
  private json = '';
  private edges = new Map<string, DeliveryEdge>();
  private nodes = new Map<string, DeliveryNode>();
  private engine: Engine | null = null;

  private readonly seed: number;
  private started = false;
  private wantStart: boolean | null = null;
  private destroyed = false;
  /** Nothing can play (the delivery was refused, or the engine cannot run): calls are dropped. */
  private dead = false;
  private readonly holds = new Set<Hold>();
  /** Calls kept until the engine loads, or until the mascot plays again. */
  private queue: Kept[] = [];
  /** The app's calls before the start, for the first frame (rules.md 11.1). */
  private early: Kept[] = [];
  /** With `autoPlay` false: the first frame is on screen, and which command it came from. */
  private firstFrame = false;
  private firstKey: string | null = null;
  /** Steps running now (`step`): app calls their event handlers make wait in `deferred`. */
  private depth = 0;
  private deferred: (() => void)[] = [];
  private draining = false;
  private wakeTimer: unknown = null;
  private refreshTimer: unknown = null;
  /** When the refresh timer is due, on the wall clock: timers can sleep with the device. */
  private refreshAt: number | null = null;
  /** When `getDelivery` was last asked, on the monotonic clock. */
  private lastAsk: number | null = null;
  private asking = false;
  /** When the current delivery was taken, on the wall clock. */
  private takenAt = 0;

  private screen: Screen = { type: 'none' };
  private pending: Showing | null = null;
  /** Counts show commands: a still or a `clipShown` report that a later one overtook is stale. */
  private showToken = 0;
  /** A pose clip restarted in place by the commands running now, reported shown after them. */
  private restarted: Showing | null = null;
  private readonly slotKeys: [string | null, string | null] = [null, null];
  private sounds: { sound: Sound; loop: boolean }[] = [];
  /** Edges the engine was told cannot play. */
  private readonly unplayable = new Set<string>();
  /** Files that failed to load, tried last until the links are refreshed. */
  private readonly failedKeys = new Set<string>();
  private voices: boolean;
  /** Waiting for the person's next gesture, after the browser refused a voice. */
  private voiceGesture: (() => void) | null = null;
  /** `preload: 'all'` has asked for every clip. */
  private preloadedAll = false;
  /** Waits for the person's next gesture to start a blocked autoplay. */
  private blockedGesture: (() => void) | null = null;

  readonly preview = {
    /** Editors and tools: cut to a pose at once (rules.md 13). */
    showPose: (pose: string): void => this.appCall((e, now) => e.previewShowPose(pose, now)),
    /** Editors and tools: play one move from the current pose (rules.md 13). */
    playMove: (edge: string): void => this.appCall((e, now) => e.previewPlayMove(edge, now))
  };

  constructor(host: Host, options: PlayerOptions) {
    this.host = host;
    this.options = options;
    this.clock = new PlayClock(() => host.now());
    // With every clip kept, the cache never drops one.
    this.cache = new ClipCache(host.files, options.preload === 'all' ? Infinity : undefined);
    this.voices = options.sound ?? true;
    this.seed = options.seed ?? Math.floor(Math.random() * 2 ** 52) + 1;
    if (options.autoPlay !== undefined) this.wantStart = options.autoPlay;
    if (options.clip) {
      // Its delivery waits for the file's length (boot).
      void this.boot();
      return;
    }
    try {
      const { delivery, json } = readDelivery(options.delivery as DeliveryInput);
      this.useDelivery(delivery);
      this.json = json;
    } catch (error) {
      this.dead = true;
      this.later(() => this.emit('error', { code: 'unreadable', detail: message(error) }));
      return;
    }
    void this.boot();
  }

  // ------------------------------------------------------------- app calls

  set(name: string, value: Value, options?: ValueOptions): void {
    if (typeof value !== 'boolean' && typeof value !== 'number') {
      this.later(() => this.emit('error', { code: 'bad_value', detail: `set(${name}) takes a boolean or a number` }));
      return;
    }
    const instant = options?.instant === true;
    this.appCall((e, now) => e.set(name, value, instant, now), `set:${name}`);
  }

  fire(name: string, options?: ValueOptions): void {
    const instant = options?.instant === true;
    // An instant fire is kept beside a plain one of the same name: it makes that fire instant.
    this.appCall((e, now) => e.fire(name, instant, now), instant ? `fire:${name}:instant` : `fire:${name}`);
  }

  reset(): void {
    this.appCall((e, now) => e.reset(now));
  }

  /** Starts the mascot, or resumes it where it stopped. */
  play(): void {
    if (this.destroyed) return;
    if (this.depth > 0) {
      this.deferred.push(() => this.play());
      return;
    }
    if (!this.started) {
      this.wantStart = true;
      if (this.engine) this.start();
      return;
    }
    this.unblock();
    this.setHold('app', false);
  }

  /**
   * Lifts a blocked autoplay, from inside a gesture: the slot that is free
   * now is primed too, so the next clip starts without another one. The
   * caller syncs.
   */
  private unblock(): void {
    if (!this.holds.has('blocked')) return;
    this.blockedGesture?.();
    this.blockedGesture = null;
    this.host.blocked(false);
    this.holds.delete('blocked');
    const busy = new Set<number>();
    if (this.screen.type === 'clip') busy.add(this.screen.showing.slot);
    if (this.pending) busy.add(this.pending.slot);
    this.host.slots.forEach((slot, i) => {
      if (!busy.has(i)) slot.prime();
    });
  }

  /** Stops play time, clips, timers and sounds; `set` and `fire` wait for `play`. */
  pause(): void {
    if (this.destroyed) return;
    if (this.depth > 0) {
      this.deferred.push(() => this.pause());
      return;
    }
    if (!this.started) {
      this.wantStart = false;
      return;
    }
    this.setHold('app', true);
  }

  /** The page is hidden or shown again: play time stops while hidden. */
  setHidden(hidden: boolean): void {
    if (this.destroyed) return;
    if (this.depth > 0) {
      this.deferred.push(() => this.setHidden(hidden));
      return;
    }
    this.setHold('hidden', hidden);
    if (!hidden) this.checkLinks();
  }

  /** Lets clip voices be heard, from the next clip or loop on. */
  setSound(on: boolean): void {
    this.voices = on;
    // The app asks again: a voice the browser refused may be allowed now.
    const screen = this.screen;
    if (screen.type === 'clip') screen.showing.voiceRefused = false;
    if (this.pending) this.pending.voiceRefused = false;
  }

  /**
   * Checks the signed links again, for when the page is shown again or the
   * device is back online: the refresh timer may have slept with the device.
   */
  checkLinks(): void {
    if (this.destroyed || this.asking || this.refreshAt === null) return;
    this.refreshIn(this.refreshAt - this.host.wallNow());
  }

  position(): Position {
    const s = this.screen;
    if (s.type === 'clip') return { clip: s.showing.edge, time: this.host.slots[s.showing.slot].time };
    if (s.type === 'still') return { clip: `still:${s.pose}`, time: 0 };
    return { clip: null, time: 0 };
  }

  on<K extends MaskoEventName>(name: K, listener: MaskoListener<K>): () => void {
    let set = this.listeners.get(name);
    if (!set) this.listeners.set(name, (set = new Set()));
    set.add(listener as (event: never) => void);
    return () => set.delete(listener as (event: never) => void);
  }

  /** Swaps in a fresh delivery of the same release: its URLs only. */
  updateDelivery(input: DeliveryInput): boolean {
    if (this.destroyed || !this.delivery) return false;
    let fresh: Delivery;
    try {
      fresh = readDelivery(input).delivery;
    } catch (error) {
      this.emit('error', { code: 'refresh_failed', detail: message(error) });
      return false;
    }
    if (!sameRelease(this.delivery, fresh)) {
      this.emit('error', { code: 'refresh_mismatch', detail: 'getDelivery returned another release' });
      return false;
    }
    this.useDelivery(fresh);
    this.failedKeys.clear();
    // Clips that failed on expired links may load now.
    for (const edge of [...this.unplayable]) this.preload(edge);
    this.scheduleRefresh();
    return true;
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.cancelWake();
    this.cancelRefresh();
    for (const s of this.sounds) s.sound.stop();
    this.sounds = [];
    if (this.pending) this.cancel(this.pending);
    if (this.screen.type === 'clip') this.cancel(this.screen.showing);
    this.pending = null;
    this.screen = { type: 'none' };
    for (const slot of this.host.slots) {
      slot.pause();
      slot.setVisible(false);
      slot.unload();
    }
    this.host.still.setVisible(false);
    this.cache.clear();
    this.engine?.free();
    this.engine = null;
    this.queue = [];
    this.early = [];
    this.deferred = [];
    this.listeners.clear();
    this.voiceGesture?.();
    this.voiceGesture = null;
    this.blockedGesture?.();
    this.blockedGesture = null;
    this.host.dispose();
  }

  // ------------------------------------------------------------- engine

  private async boot(): Promise<void> {
    const clip = this.options.clip ? this.readClip(this.options.clip) : null;
    try {
      await loadWasm();
    } catch (error) {
      if (this.destroyed) return;
      // It never loads now: calls are dropped rather than kept.
      this.dead = true;
      this.queue = [];
      this.early = [];
      this.emit('error', { code: 'engine_unavailable', detail: message(error) });
      return;
    }
    if (clip && !(await clip)) return;
    if (this.destroyed) return;
    let engine: Engine;
    try {
      engine = Engine.load(this.json);
    } catch (error) {
      const e = error instanceof LoadError ? error : new LoadError('unreadable', message(error));
      this.dead = true;
      this.queue = [];
      this.early = [];
      this.emit('error', { code: e.code, detail: e.detail });
      return;
    }
    this.engine = engine;
    engine.seed(this.seed);
    // Values set or fired before the start, at play time 0 (rules.md 11.1).
    const early = this.queue;
    this.queue = [];
    for (const k of early) this.exec(this.ask(k.fn, engine, 0));
    const autoPlay = this.wantStart ?? this.delivery?.runtimeGraph.autoPlay ?? true;
    if (autoPlay) this.start();
    else this.showFirstFrame();
    this.scheduleRefresh();
  }

  /**
   * A lone clip's delivery, once its file's length is known: the preferred
   * format first, then the other one. When neither can be read nothing
   * plays, with `error` `clip_failed`. True when the delivery is ready.
   */
  private async readClip(files: ClipFiles): Promise<boolean> {
    let failure = 'the clip has no webm or hevc file';
    for (const format of this.host.formats) {
      const url = files[format];
      if (!url) continue;
      try {
        const seconds = await this.host.clipLength(url);
        const delivery = clipDelivery(files, seconds);
        this.useDelivery(delivery);
        this.json = JSON.stringify(delivery);
        return true;
      } catch (error) {
        failure = message(error);
      }
    }
    if (this.destroyed) return false;
    this.dead = true;
    this.queue = [];
    this.early = [];
    this.emit('error', { code: 'clip_failed', detail: failure });
    return false;
  }

  private start(): void {
    if (this.started || this.destroyed || !this.engine) return;
    this.started = true;
    this.early = [];
    this.firstFrame = false;
    if (this.holds.size === 0) this.clock.play();
    this.exec(this.engine.start(0));
    this.sync();
  }

  /**
   * With `autoPlay` false: the first pose's first frame, from a throwaway
   * engine that heard the same calls before the start. Shown again when a
   * later call changes it.
   */
  private showFirstFrame(): void {
    let first: Command | undefined;
    try {
      const probe = Engine.load(this.json);
      try {
        probe.seed(this.seed);
        for (const k of this.early) k.fn(probe, 0);
        first = probe.start(0).find((c) => c.type === 'showClip' || c.type === 'showStill');
      } finally {
        probe.free();
      }
    } catch {
      return;
    }
    this.firstFrame = true;
    const key = first ? JSON.stringify(first) : null;
    if (!first || key === this.firstKey) return;
    this.firstKey = key;
    const shown = first;
    this.step(() => {
      this.run(shown);
      // The engine that plays did not ask for it: its first frame is not reported.
      this.restarted = null;
      if (this.pending) this.pending.ask = 0;
    });
  }

  private running(): boolean {
    return this.started && !this.destroyed && this.holds.size === 0;
  }

  /** An app call. Before the start, it also decides the first frame (rules.md 11.1). */
  private appCall(fn: Call, key?: string): void {
    if (this.destroyed || this.dead) return;
    if (this.depth > 0) {
      this.deferred.push(() => this.appCall(fn, key));
      return;
    }
    if (!this.started) keep(this.early, fn, key);
    this.call(fn, key);
    if (!this.started && this.firstFrame) this.showFirstFrame();
  }

  /**
   * Runs an engine call now, or keeps it: until the step running now is done
   * (`inOrder` runs it at once, at the end of a step), or for when the mascot
   * plays.
   */
  private call(fn: Call, key?: string, inOrder = false): void {
    if (this.destroyed || this.dead) return;
    if (this.depth > 0 && !inOrder) {
      this.deferred.push(() => this.call(fn, key));
      return;
    }
    const engine = this.engine;
    if (!engine || (this.started && !this.running())) {
      keep(this.queue, fn, key);
      return;
    }
    this.exec(this.ask(fn, engine, this.started ? this.clock.now() : 0));
  }

  /** One engine call; one that throws is reported outside and gives no commands. */
  private ask(fn: Call, engine: Engine, now: number): Command[] {
    try {
      return fn(engine, now);
    } catch (error) {
      this.later(() => {
        throw error;
      });
      return [];
    }
  }

  private exec(commands: Command[]): void {
    this.step(() => {
      for (const c of commands) {
        if (this.destroyed) return;
        try {
          this.run(c);
        } catch (error) {
          // One command that throws must not skip the rest: report it outside.
          this.later(() => {
            throw error;
          });
        }
      }
      // A pose clip restarted in place is on screen at once, after the commands that restarted it.
      const restarted = this.restarted;
      this.restarted = null;
      if (restarted) this.reportShown(restarted);
      this.scheduleWake();
    });
  }

  /**
   * Runs one step: a batch of commands, or a swap and its report. App calls
   * the event handlers make meanwhile wait until it is done, then run in
   * order, so they never run inside the batch that told them.
   */
  private step(fn: () => void): void {
    this.depth++;
    try {
      fn();
    } finally {
      this.depth--;
    }
    if (this.depth === 0) this.drain();
  }

  private drain(): void {
    if (this.draining) return;
    this.draining = true;
    try {
      let chained = 0;
      while (this.deferred.length > 0 && !this.destroyed) {
        if (++chained > MAX_CHAINED) {
          // Handlers that keep calling back: the rest waits, the page goes on.
          this.later(() => this.drain());
          return;
        }
        this.deferred.shift()!();
      }
    } finally {
      this.draining = false;
    }
  }

  private run(c: Command): void {
    switch (c.type) {
      case 'showClip':
        this.showClip(c.edge, c.kind, c.rate);
        break;
      case 'showStill':
        this.showStill(c.pose);
        break;
      case 'preload':
        this.preload(c.edge);
        break;
      case 'playSound':
        this.playSound(c.edge);
        break;
      case 'stopSounds':
        this.stopSounds(c.onlyLooping);
        break;
      case 'poseEntered':
        this.cache.nextBatch();
        this.emit('pose', { pose: c.pose });
        break;
      case 'interrupted':
        // The next showClip replaces the move on screen.
        break;
      case 'spent':
        this.emit('reactionDone', { name: c.name, used: c.used });
        break;
      case 'error':
        if (c.code === 'clip_timeout' && this.pending) {
          // The clip asked for did not show in time and the engine counts it unplayable
          // (rules.md 9.3): drop it, and let a later preload of its file mark it playable.
          this.unplayable.add(this.pending.edge);
          this.dropPending();
        }
        this.emit('error', { code: c.code, detail: c.detail });
        break;
    }
  }

  // ------------------------------------------------------------- clock

  private scheduleWake(): void {
    this.cancelWake();
    if (!this.running() || !this.engine) return;
    const at = this.engine.nextWake();
    if (at === undefined) return;
    this.wakeTimer = this.host.setTimer(() => {
      this.wakeTimer = null;
      this.call((e, now) => e.advance(now));
    }, Math.max(0, at - this.clock.now()));
  }

  private cancelWake(): void {
    if (this.wakeTimer !== null) this.host.clearTimer(this.wakeTimer);
    this.wakeTimer = null;
  }

  private setHold(hold: Hold, on: boolean): void {
    if (on) this.holds.add(hold);
    else this.holds.delete(hold);
    this.sync();
  }

  /** Brings the clock, clips and sounds in line with the holds. */
  private sync(): void {
    if (!this.started || this.destroyed) return;
    if (this.running()) {
      if (this.clock.running) return;
      this.clock.play();
      // Calls kept while paused first, then the clips and sounds go on.
      const queued = this.queue;
      this.queue = [];
      while (queued.length > 0 && this.running() && this.engine) {
        this.exec(this.ask(queued.shift()!.fn, this.engine, this.clock.now()));
      }
      // Paused again by a handler: the rest stays first in line.
      if (queued.length > 0) this.queue = [...queued, ...this.queue];
      if (!this.running()) return;
      const screen = this.screen;
      if (screen.type === 'clip' && !screen.showing.ended) this.playShowing(screen.showing);
      // A clip still waiting for its file plays once it is in its slot.
      if (this.pending?.loaded) this.playShowing(this.pending);
      for (const s of this.sounds) s.sound.resume();
      this.scheduleWake();
    } else {
      if (!this.clock.running) return;
      this.clock.pause();
      this.cancelWake();
      if (this.screen.type === 'clip') this.host.slots[this.screen.showing.slot].pause();
      if (this.pending) {
        const p = this.pending;
        this.host.slots[p.slot].pause();
        // Shows its first frame while paused, once its file is in the slot: until
        // then the slot still holds an older clip (`load` waits for it otherwise).
        if (p.loaded) p.off.push(this.host.slots[p.slot].onReady(() => this.swapIn(p)));
      }
      for (const s of this.sounds) s.sound.pause();
    }
  }

  // ------------------------------------------------------------- clips

  private showClip(edge: string, kind: ClipKind, rate: number): void {
    this.showToken++;
    const screen = this.screen;
    if (
      kind === 'pose' &&
      screen.type === 'clip' &&
      screen.showing.edge === edge &&
      screen.showing.kind === 'pose' &&
      !screen.showing.cancelled
    ) {
      // The next loop of the clip on screen: restart it, no swap.
      if (this.pending) this.dropPending();
      const s = screen.showing;
      s.ask = this.showToken;
      s.ended = false;
      s.rate = rate;
      // Sound turned on or off since the last loop.
      s.muted = this.muted(edge) || s.voiceRefused;
      this.restarted = s;
      this.host.slots[s.slot].load(this.host.slots[s.slot].src ?? s.files[s.tried].url);
      if (this.running()) this.playShowing(s);
      this.emit('clip', { clip: edge, kind, rate });
      return;
    }
    const busy = screen.type === 'clip' ? screen.showing.slot : null;
    if (this.pending) this.dropPending();
    const files = chooseFiles(this.edges.get(edge), this.host.formats, this.host.pixels());
    files.sort((a, b) => Number(this.failedKeys.has(a.key)) - Number(this.failedKeys.has(b.key)));
    const s: Showing = {
      edge,
      kind,
      rate,
      slot: busy === 0 ? 1 : 0,
      ask: this.showToken,
      files,
      tried: 0,
      loaded: false,
      muted: this.muted(edge),
      voiceRefused: false,
      ended: false,
      cancelled: false,
      off: []
    };
    this.pending = s;
    void this.load(s);
  }

  /** Whether this edge's clip plays muted: its own voice only with sound on (rules.md 9.6). */
  private muted(edge: string): boolean {
    return !(this.voices && this.edges.get(edge)?.hasAudio === true);
  }

  private async load(s: Showing): Promise<void> {
    s.loaded = false;
    const file = s.files[s.tried];
    if (!file) {
      this.fail(s, s.tried);
      return;
    }
    const slot = this.host.slots[s.slot];
    let src = file.url;
    const cached = this.cache.acquire(file.key, file.url);
    if (cached) {
      try {
        src = await cached;
      } catch {
        src = file.url;
      }
      if (s.cancelled || this.destroyed) {
        this.cache.release(file.key);
        return;
      }
    }
    const previous = this.slotKeys[s.slot];
    if (previous !== null) this.cache.release(previous);
    this.slotKeys[s.slot] = cached ? file.key : null;
    slot.load(src);
    s.loaded = true;
    const attempt = s.tried;
    s.off.push(
      slot.onFrame(() => this.swapIn(s)),
      slot.onEnded(() => this.ended(s)),
      slot.onError(() => this.fail(s, attempt))
    );
    if (this.running()) this.playShowing(s);
    else s.off.push(slot.onReady(() => this.swapIn(s)));
  }

  private playShowing(s: Showing): void {
    const slot = this.host.slots[s.slot];
    const attempt = s.tried;
    slot.play(s.rate, s.muted).catch((error: unknown) => {
      if (s.cancelled || this.destroyed || attempt !== s.tried) return;
      const name = error instanceof Error || isDomError(error) ? (error as { name: string }).name : '';
      if (name === 'AbortError') return;
      if (name === 'NotAllowedError') {
        if (!s.muted) {
          // The browser refuses sound without a gesture: play muted, and ask
          // again from the next clip or loop after the person's first gesture.
          s.muted = true;
          s.voiceRefused = true;
          this.voiceGesture ??= this.host.onGesture(() => {
            this.voiceGesture = null;
            const screen = this.screen;
            if (screen.type === 'clip') screen.showing.voiceRefused = false;
            if (this.pending) this.pending.voiceRefused = false;
          });
          if (this.running()) this.playShowing(s);
          return;
        }
        // Autoplay is blocked (iPhones in Low Power Mode refuse even muted
        // video): the first frame with a tap-to-play overlay. A tap on it, or
        // the person's next gesture anywhere on the page, starts it.
        this.host.blocked(true);
        this.setHold('blocked', true);
        this.blockedGesture ??= this.host.onGesture(() => {
          this.blockedGesture = null;
          if (this.destroyed || !this.holds.has('blocked')) return;
          this.unblock();
          this.sync();
        });
        return;
      }
      this.fail(s, attempt);
    });
  }

  /** The showing's first frame is decoded: make it visible and hide the old one, in one frame. */
  private swapIn(s: Showing): void {
    if (this.pending !== s || s.cancelled || this.destroyed) return;
    this.step(() => this.swap(s));
  }

  private swap(s: Showing): void {
    this.pending = null;
    const old = this.screen;
    this.host.slots[s.slot].setVisible(true);
    if (old.type === 'clip') {
      const o = old.showing;
      this.host.slots[o.slot].setVisible(false);
      this.host.slots[o.slot].pause();
      this.detach(o);
      o.cancelled = true;
    } else if (old.type === 'still') {
      this.host.still.setVisible(false);
    }
    this.screen = { type: 'clip', showing: s };
    if (this.unplayable.delete(s.edge)) this.engine?.markPlayable(s.edge);
    this.emit('clip', { clip: s.edge, kind: s.kind, rate: s.rate });
    const edge = this.edges.get(s.edge);
    if (s.kind === 'move' && edge?.movement !== undefined) {
      this.emit('movement', { edge: s.edge, movement: edge.movement, rate: s.rate, seconds: edge.duration / s.rate });
    }
    this.reportShown(s);
    if (this.options.preload === 'all' && !this.preloadedAll) {
      this.preloadedAll = true;
      this.preloadAll();
    }
  }

  /** `preload: 'all'`: every clip of the release with a file this browser plays, after the engine's own preloads. */
  private preloadAll(): void {
    for (const [edge, e] of this.edges) {
      if (chooseFiles(e, this.host.formats, this.host.pixels()).length > 0) this.preload(edge);
    }
  }

  /**
   * Tells the engine the showing's first frame is on screen, at the play time
   * of now (rules.md 9.3). Called once per showing: at the swap, or after a
   * pose clip restarts in place. A showing a later show command overtook is
   * not reported, also when the report waits for play to resume.
   */
  private reportShown(s: Showing): void {
    const ask = s.ask;
    const current = () => ask === this.showToken && !s.cancelled;
    if (!current()) return;
    // At the end of the step that put it on screen, before the app calls it led to.
    this.call((e, now) => (current() ? e.clipShown(s.edge, now) : []), undefined, true);
  }

  private ended(s: Showing): void {
    if (s.cancelled || s.ended || this.destroyed) return;
    this.step(() => {
      // A clip that ends before its frame callback is on screen by now.
      if (this.pending === s) this.swapIn(s);
      if (this.screen.type !== 'clip' || this.screen.showing !== s) return;
      s.ended = true;
      this.call((e, now) => e.clipEnded(s.edge, now), undefined, true);
    });
  }

  /** The file of this attempt failed: try the other format once, then report the clip failed. */
  private fail(s: Showing, attempt: number): void {
    // The error event and the play() refusal can both report one attempt.
    if (s.cancelled || this.destroyed || attempt !== s.tried) return;
    this.detach(s);
    const file = s.files[s.tried];
    if (file) this.failedKeys.add(file.key);
    s.tried++;
    // A file that fails before it is shown: the other format, in the same hidden slot.
    if (this.pending === s && s.tried < s.files.length) {
      void this.load(s);
      return;
    }
    s.cancelled = true;
    if (this.pending === s) this.pending = null;
    this.unplayable.add(s.edge);
    this.call((e, now) => e.clipFailed(s.edge, now));
  }

  private showStill(pose: string): void {
    const token = ++this.showToken;
    if (this.pending) this.dropPending();
    const url = this.nodes.get(pose)?.transparentThumbnailUrl;
    const reveal = (shown: boolean) => {
      if (token !== this.showToken || this.destroyed) return;
      this.step(() => {
        this.host.still.setVisible(shown);
        if (this.screen.type === 'clip') {
          const o = this.screen.showing;
          this.host.slots[o.slot].setVisible(false);
          this.host.slots[o.slot].pause();
          this.detach(o);
          o.cancelled = true;
        }
        this.screen = { type: 'still', pose };
        this.emit('clip', { clip: `still:${pose}`, kind: 'still', rate: 1 });
      });
    };
    if (url) {
      this.host.still.load(url).then(
        () => reveal(true),
        () => reveal(false)
      );
    } else reveal(false);
  }

  private dropPending(): void {
    const p = this.pending;
    if (!p) return;
    this.pending = null;
    this.cancel(p);
    this.host.slots[p.slot].pause();
  }

  private cancel(s: Showing): void {
    s.cancelled = true;
    this.detach(s);
  }

  private detach(s: Showing): void {
    for (const off of s.off) off();
    s.off = [];
  }

  // ------------------------------------------------------------- preload and sound

  private preload(edge: string): void {
    const files = chooseFiles(this.edges.get(edge), this.host.formats, this.host.pixels());
    const attempt = (i: number): void => {
      if (this.destroyed || !this.engine) return;
      const file = files[i];
      if (!file) {
        if (!this.unplayable.has(edge)) {
          this.unplayable.add(edge);
          this.engine.markUnplayable(edge);
        }
        return;
      }
      this.cache.preload(file.key, file.url).then(
        (cached) => {
          if (cached && this.unplayable.delete(edge)) this.engine?.markPlayable(edge);
        },
        () => {
          this.failedKeys.add(file.key);
          attempt(i + 1);
        }
      );
    };
    attempt(0);
  }

  private playSound(edgeId: string): void {
    const s = this.edges.get(edgeId)?.sound;
    if (!s?.url) return;
    const loop = s.loop === true;
    // A browser throws on a volume that is not a number from 0 to 1.
    const volume = typeof s.volume === 'number' && Number.isFinite(s.volume) ? Math.min(1, Math.max(0, s.volume)) : 1;
    const entry = { sound: this.host.sounds.play(s.url, volume, loop), loop };
    this.sounds.push(entry);
    if (!loop) entry.sound.onEnded(() => (this.sounds = this.sounds.filter((x) => x !== entry)));
    if (!this.running()) entry.sound.pause();
  }

  private stopSounds(onlyLooping: boolean): void {
    this.sounds = this.sounds.filter((s) => {
      if (onlyLooping && !s.loop) return true;
      s.sound.stop();
      return false;
    });
  }

  // ------------------------------------------------------------- delivery

  private useDelivery(delivery: Delivery): void {
    this.takenAt = this.host.wallNow();
    if (!delivery || typeof delivery !== 'object' || !delivery.runtimeGraph) {
      // The engine refuses it with its own code.
      this.delivery = delivery ?? null;
      return;
    }
    this.delivery = delivery;
    const index = indexDelivery(delivery);
    this.edges = index.edges;
    this.nodes = index.nodes;
  }

  /**
   * How long the signed links last from now, or null when they do not
   * expire. With `lastFetchedAt`, both times are the server's, so a device
   * clock that is off does not matter.
   */
  private linksLeft(): number | null {
    const d = this.delivery;
    const at = d ? expiresAt(d) : null;
    if (!d || at === null) return null;
    const fetched = fetchedAt(d);
    if (fetched !== null && fetched <= at) return at - fetched - (this.host.wallNow() - this.takenAt);
    return at - this.host.wallNow();
  }

  /** Asks `getDelivery` for fresh links before the signed ones expire. */
  private scheduleRefresh(): void {
    this.cancelRefresh();
    if (this.destroyed || !this.options.getDelivery) return;
    const left = this.linksLeft();
    if (left === null) return;
    this.refreshIn(left > 2 * REFRESH_LEAD_MS ? left - REFRESH_LEAD_MS : left / 2);
  }

  private refreshIn(delay: number): void {
    this.cancelRefresh();
    // Never sooner than 30 s after the last ask: links that look expired at once cannot loop.
    const soonest = this.lastAsk === null ? 0 : this.lastAsk + REFRESH_RETRY_MS - this.host.now();
    const wait = Math.min(Math.max(0, delay, soonest), 2 ** 31 - 1);
    this.refreshAt = this.host.wallNow() + wait;
    this.refreshTimer = this.host.setTimer(() => {
      this.refreshTimer = null;
      this.refreshAt = null;
      void this.refresh();
    }, wait);
  }

  private cancelRefresh(): void {
    if (this.refreshTimer !== null) this.host.clearTimer(this.refreshTimer);
    this.refreshTimer = null;
    this.refreshAt = null;
  }

  private async refresh(): Promise<void> {
    const get = this.options.getDelivery;
    if (!get || this.destroyed || !this.delivery) return;
    const before = expiresAt(this.delivery);
    this.lastAsk = this.host.now();
    this.asking = true;
    let input: DeliveryInput;
    try {
      input = await get();
    } catch (error) {
      this.asking = false;
      if (this.destroyed) return;
      this.emit('error', { code: 'refresh_failed', detail: message(error) });
      this.refreshIn(REFRESH_RETRY_MS);
      return;
    }
    this.asking = false;
    if (this.destroyed) return;
    if (!this.updateDelivery(input)) {
      this.refreshIn(REFRESH_RETRY_MS);
      return;
    }
    // Links that do not last longer are asked for again, not in a tight loop.
    const after = expiresAt(this.delivery);
    if (after !== null && before !== null && after <= before) this.refreshIn(REFRESH_RETRY_MS);
  }

  // ------------------------------------------------------------- events

  private emit<K extends MaskoEventName>(name: K, event: MaskoEvents[K]): void {
    const set = this.listeners.get(name);
    if (!set) return;
    for (const listener of [...set]) {
      try {
        (listener as MaskoListener<K>)(event);
      } catch (error) {
        // An app's handler must not stop the mascot; report it outside.
        this.later(() => {
          throw error;
        });
      }
    }
  }

  private later(fn: () => void): void {
    this.host.setTimer(fn, 0);
  }
}

/**
 * Keeps a call for later. Calls kept are applied at one play time, so among
 * the app's calls since the last other one, a `set` of a name replaces the
 * one kept before and a name already fired stays fired once (rules.md 3.2,
 * 3.3). Past `MAX_KEPT`, the oldest go first.
 */
function keep(list: Kept[], fn: Call, key?: string): void {
  if (key) {
    for (let i = list.length - 1; i >= 0 && list[i].key; i--) {
      if (list[i].key !== key) continue;
      if (key.startsWith('fire:')) return;
      list.splice(i, 1);
      break;
    }
  }
  list.push({ fn, key });
  if (list.length > MAX_KEPT) list.splice(0, list.length - MAX_KEPT);
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isDomError(error: unknown): boolean {
  return typeof error === 'object' && error !== null && typeof (error as { name?: unknown }).name === 'string';
}
