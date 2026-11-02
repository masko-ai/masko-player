// A canvas from its URL (spec/player.md, "A canvas from its URL"), on the
// web: the player asks for the canvas's newest release with an app key,
// renews its links from that release's own URL, and moves to a newer release
// where the initial pose's loop starts again. Browsers keep the files in
// their own cache, so nothing is kept for offline play.

import type { Delivery, DeliveryInput } from './delivery.ts';
import type { Value } from './engine.ts';
import type {
  MaskoEventName,
  MaskoEvents,
  MaskoListener,
  Position,
  ValueOptions
} from './player.ts';

/** What the canvas needs from one release's player. */
export interface ReleasePlayer {
  set(name: string, value: Value, options?: ValueOptions): void;
  fire(name: string, options?: ValueOptions): void;
  reset(): void;
  play(): void;
  pause(): void;
  destroy(): void;
  position(): Position;
  updateDelivery(delivery: DeliveryInput): boolean;
  setSound(on: boolean): void;
  on<K extends MaskoEventName>(event: K, handler: MaskoListener<K>): () => void;
  preview: { showPose(id: string): void; playMove(id: string): void };
}

export interface CanvasSource {
  /** `https://api.masko.ai/v1/canvases/{canvasId}/delivery` */
  url: string;
  /** An app key of the canvas's mascot (`masko_app_...`). */
  appKey: string;
}

export interface CanvasDeps {
  fetch: (url: string, init: { headers: Record<string, string> }) => Promise<Response>;
  setTimer(fn: () => void, ms: number): unknown;
  clearTimer(id: unknown): void;
  /** The random UUID this browser keeps, or null when it cannot keep one. */
  installId(): string | null;
  /**
   * A player for one release. `over` is true when it takes over from the one
   * on screen: it shows on top until `settle()`, once the old one is gone.
   */
  makePlayer(
    delivery: Delivery,
    getDelivery: () => Promise<Delivery>,
    over: boolean
  ): { player: ReleasePlayer; settle(): void };
}

const HOUR = 3_600_000;
const FIRST_RETRY = 30_000;
/** How often a waiting switch looks for the initial pose's loop start. */
const WATCH = 200;
/** A switch that finds no loop start in this time happens anyway. */
const MAX_WAIT = 600_000;
/** A new release whose first clip does not show by then is given up. */
const SHOW_WAIT = 30_000;

interface Release {
  id: string;
  delivery: Delivery;
}
interface Playing {
  player: ReleasePlayer;
  release: string;
  /** The initial pose's own loop: where a newer release may take over. */
  loop: string | null;
  settle(): void;
}
interface Subscription {
  event: MaskoEventName;
  listener: MaskoListener<MaskoEventName>;
  offs: Map<ReleasePlayer, () => void>;
}

/** The initial pose's loop: a move from that pose to itself. */
function initialLoop(delivery: Delivery): string | null {
  const graph = delivery.runtimeGraph;
  const edge = graph.edges.find(
    (e) => e.source === graph.initialNode && e.target === graph.initialNode && e.isLoop
  );
  return edge?.id ?? null;
}

/** The URL of one release, from the canvas's URL. */
export function releaseUrl(url: string, release: string): string {
  const u = new URL(url);
  u.pathname = u.pathname.replace(/\/delivery\/?$/, `/releases/${encodeURIComponent(release)}/delivery`);
  return u.toString();
}

export function playCanvas(source: CanvasSource, deps: CanvasDeps, options: { sound?: boolean } = {}): ReleasePlayer {
  let current: Playing | null = null;
  let incoming: Playing | null = null;
  let waiting: Release | null = null;
  let destroyed = false;
  let paused = false;
  let sound = options.sound;
  let retry = FIRST_RETRY;
  let checkTimer: unknown = null;
  const held = new Map<string, { value: Value; options?: ValueOptions }>();
  const subscriptions = new Set<Subscription>();

  const emit = <K extends MaskoEventName>(event: K, data: MaskoEvents[K]) => {
    for (const s of subscriptions) if (s.event === event) (s.listener as MaskoListener<K>)(data);
  };

  async function ask(url: string): Promise<Release> {
    const u = new URL(url);
    if (!u.searchParams.has('target')) u.searchParams.set('target', 'web');
    const headers: Record<string, string> = { Authorization: `Bearer ${source.appKey}` };
    const install = deps.installId();
    if (install) headers['Masko-Install-Id'] = install;
    const res = await deps.fetch(u.toString(), { headers });
    const body = (await res.json().catch(() => null)) as {
      data?: { release_id?: string; delivery?: Delivery };
      error?: { message?: string };
    } | null;
    const delivery = body?.data?.delivery;
    if (!res.ok || !delivery) throw new Error(body?.error?.message ?? `HTTP ${res.status}`);
    return { id: body?.data?.release_id ?? delivery.release?.id ?? '', delivery };
  }

  function make(release: Release, over: boolean): Playing {
    const { player, settle } = deps.makePlayer(
      release.delivery,
      () => ask(releaseUrl(source.url, release.id)).then((r) => r.delivery),
      over
    );
    for (const s of subscriptions) s.offs.set(player, player.on(s.event, s.listener));
    for (const [name, v] of held) player.set(name, v.value, v.options);
    if (sound !== undefined) player.setSound(sound);
    if (paused) player.pause();
    return { player, release: release.id, loop: initialLoop(release.delivery), settle };
  }

  function drop(playing: Playing) {
    for (const s of subscriptions) {
      s.offs.get(playing.player)?.();
      s.offs.delete(playing.player);
    }
    playing.player.destroy();
  }

  function schedule(ms: number) {
    if (destroyed) return;
    if (checkTimer !== null) deps.clearTimer(checkTimer);
    checkTimer = deps.setTimer(() => {
      checkTimer = null;
      void check();
    }, ms);
  }

  async function check() {
    let release: Release;
    try {
      release = await ask(source.url);
    } catch (error) {
      if (destroyed) return;
      emit('error', { code: 'refresh_failed', detail: error instanceof Error ? error.message : String(error) });
      if (current) schedule(HOUR);
      else {
        schedule(retry);
        retry = Math.min(retry * 2, HOUR);
      }
      return;
    }
    if (destroyed) return;
    if (!current) current = make(release, false);
    else if (release.id === current.release) current.player.updateDelivery(release.delivery);
    else if (release.id !== incoming?.release) switchTo(release);
    schedule(HOUR);
  }

  /** Waits for the initial pose's loop to start again, then lets the new release take over. */
  function switchTo(release: Release) {
    const first = waiting === null;
    waiting = release;
    if (!first) return;
    let waited = 0;
    const watch = () => {
      if (destroyed || !current || !waiting) return;
      const at = current.player.position();
      const atStart = current.loop !== null && at.clip === current.loop && at.time < 0.3;
      if (paused || atStart || waited >= MAX_WAIT) {
        const next = waiting;
        waiting = null;
        takeOver(next);
        return;
      }
      waited += WATCH;
      deps.setTimer(watch, WATCH);
    };
    watch();
  }

  function takeOver(release: Release) {
    if (incoming) drop(incoming);
    const next = make(release, true);
    incoming = next;
    let off: (() => void) | null = null;
    const giveUp = deps.setTimer(() => {
      off?.();
      if (incoming !== next) return;
      incoming = null;
      drop(next);
      emit('error', { code: 'refresh_failed', detail: 'the new release did not start' });
    }, SHOW_WAIT);
    off = next.player.on('clip', () => {
      off?.();
      deps.clearTimer(giveUp);
      if (incoming !== next || destroyed) return;
      const old = current;
      current = next;
      incoming = null;
      if (old) drop(old);
      next.settle();
    });
  }

  void check();

  const both = (fn: (player: ReleasePlayer) => void) => {
    if (current) fn(current.player);
    if (incoming) fn(incoming.player);
  };

  return {
    set(name, value, valueOptions) {
      held.set(name, { value, options: valueOptions });
      both((p) => p.set(name, value, valueOptions));
    },
    // Before the first release comes, a fire has nothing to start: it is dropped.
    fire: (name, valueOptions) => current?.player.fire(name, valueOptions),
    reset() {
      held.clear();
      both((p) => p.reset());
    },
    play() {
      paused = false;
      both((p) => p.play());
    },
    pause() {
      paused = true;
      both((p) => p.pause());
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      if (checkTimer !== null) deps.clearTimer(checkTimer);
      if (incoming) drop(incoming);
      if (current) drop(current);
      current = incoming = null;
      subscriptions.clear();
    },
    position: () => current?.player.position() ?? { clip: null, time: 0 },
    updateDelivery: (delivery) => current?.player.updateDelivery(delivery) ?? false,
    setSound(on) {
      sound = on;
      both((p) => p.setSound(on));
    },
    on(event, handler) {
      const s: Subscription = {
        event,
        listener: handler as MaskoListener<MaskoEventName>,
        offs: new Map()
      };
      subscriptions.add(s);
      both((p) => s.offs.set(p, p.on(event, handler)));
      return () => {
        subscriptions.delete(s);
        for (const off of s.offs.values()) off();
      };
    },
    preview: {
      showPose: (id) => current?.player.preview.showPose(id),
      playMove: (id) => current?.player.preview.playMove(id)
    }
  };
}
