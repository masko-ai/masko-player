// A canvas from its URL on the web (spec/player.md): asking with the app key,
// values kept for the first release, links renewed from the release's own
// URL, retries, and a newer release taking over at the initial loop's start.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { playCanvas, releaseUrl, type ReleasePlayer } from '../src/canvas-url.ts';
import type { Delivery } from '../src/delivery.ts';
import type { MaskoEventName, MaskoListener, Position } from '../src/player.ts';

const URL_ = 'https://api.masko.ai/v1/canvases/c1/delivery';
const KEY = 'masko_app_test';

const delivery = (tag: string): Delivery => ({
  deliverySchemaVersion: 1,
  release: { id: `sha256:${tag}` },
  runtimeGraph: {
    initialNode: 'idle',
    nodes: [{ id: 'idle' }, { id: 'cheer' }],
    edges: [
      { id: 'idle-loop', source: 'idle', target: 'idle', isLoop: true, duration: 4 },
      { id: 'idle-cheer', source: 'idle', target: 'cheer', isLoop: false, duration: 1 }
    ]
  }
});

class FakePlayer implements ReleasePlayer {
  calls: string[] = [];
  at: Position = { clip: null, time: 0 };
  destroyed = false;
  settled = false;
  private listeners = new Map<string, Set<(e: unknown) => void>>();
  readonly tag: string;
  readonly over: boolean;
  readonly getDelivery: () => Promise<Delivery>;
  constructor(tag: string, over: boolean, getDelivery: () => Promise<Delivery>) {
    this.tag = tag;
    this.over = over;
    this.getDelivery = getDelivery;
  }
  set(name: string, value: unknown) { this.calls.push(`set ${name}=${value}`); }
  fire(name: string) { this.calls.push(`fire ${name}`); }
  reset() { this.calls.push('reset'); }
  play() { this.calls.push('play'); }
  pause() { this.calls.push('pause'); }
  destroy() { this.destroyed = true; }
  position() { return this.at; }
  updateDelivery(d: unknown) { this.calls.push(`update ${(d as Delivery).release?.id}`); return true; }
  setSound(on: boolean) { this.calls.push(`sound ${on}`); }
  on<K extends MaskoEventName>(event: K, handler: MaskoListener<K>) {
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(handler as (e: unknown) => void);
    return () => set.delete(handler as (e: unknown) => void);
  }
  emit(event: MaskoEventName, data: unknown) {
    for (const l of [...(this.listeners.get(event) ?? [])]) l(data);
  }
  preview = { showPose() {}, playMove() {} };
}

function setup(answers: (() => { status: number; body: unknown })[]) {
  const asked: { url: string; headers: Record<string, string> }[] = [];
  const timers: { fn: () => void; ms: number; id: number }[] = [];
  const players: FakePlayer[] = [];
  let id = 0;
  const handle = playCanvas(
    { url: URL_, appKey: KEY },
    {
      fetch: async (url, init) => {
        asked.push({ url, headers: init.headers });
        const answer = answers[Math.min(asked.length - 1, answers.length - 1)]();
        return new Response(JSON.stringify(answer.body), { status: answer.status });
      },
      setTimer: (fn, ms) => {
        timers.push({ fn, ms, id: ++id });
        return id;
      },
      clearTimer: (t) => {
        const i = timers.findIndex((x) => x.id === t);
        if (i >= 0) timers.splice(i, 1);
      },
      installId: () => 'install-1',
      makePlayer: (d, getDelivery, over) => {
        const p = new FakePlayer(d.release?.id ?? '?', over, getDelivery);
        players.push(p);
        return { player: p, settle: () => (p.settled = true) };
      }
    }
  );
  /** Runs the next timer of `ms` (or the next one at all). */
  const tick = async (ms?: number) => {
    const i = ms === undefined ? 0 : timers.findIndex((t) => t.ms === ms);
    assert.ok(i >= 0, `no timer of ${ms} ms`);
    const [t] = timers.splice(i, 1);
    t.fn();
    await flush();
  };
  return { handle, asked, timers, players, tick };
}
const flush = () => new Promise((r) => setTimeout(r, 0));
const ok = (tag: string, release = tag) => () => ({
  status: 200,
  body: { data: { release_id: release, delivery: delivery(tag) } }
});

test('asks with the app key and keeps values set before the first release', async () => {
  const { handle, asked, players } = setup([ok('r1')]);
  handle.set('behavior::working', true);
  handle.fire('action::cheer');
  await flush();
  assert.equal(asked.length, 1);
  assert.equal(asked[0].url, `${URL_}?target=web`);
  assert.equal(asked[0].headers.Authorization, `Bearer ${KEY}`);
  assert.equal(asked[0].headers['Masko-Install-Id'], 'install-1');
  assert.equal(players.length, 1);
  assert.deepEqual(players[0].calls, ['set behavior::working=true']);
  handle.fire('action::cheer');
  assert.deepEqual(players[0].calls.at(-1), 'fire action::cheer');
});

test("renews links from the release's own URL, and the same release only swaps links", async () => {
  const { players, asked, tick } = setup([ok('r1', 'rel-1')]);
  await flush();
  await players[0].getDelivery();
  assert.equal(asked[1].url, 'https://api.masko.ai/v1/canvases/c1/releases/rel-1/delivery?target=web');
  await tick(3_600_000);
  assert.equal(players.length, 1);
  assert.equal(players[0].calls.at(-1), 'update sha256:r1');
  assert.equal(releaseUrl(`${URL_}?target=web`, 'a b'), 'https://api.masko.ai/v1/canvases/c1/releases/a%20b/delivery?target=web');
});

test('a first release that does not come is asked again, 30 seconds then twice as long', async () => {
  let fail = true;
  const { handle, timers, players, tick } = setup([
    () => (fail ? { status: 503, body: { error: { message: 'busy' } } } : ok('r1')())
  ]);
  const errors: unknown[] = [];
  handle.on('error', (e) => errors.push(e));
  await flush();
  assert.deepEqual(errors, [{ code: 'refresh_failed', detail: 'busy' }]);
  assert.equal(timers[0].ms, 30_000);
  await tick(30_000);
  assert.equal(timers[0].ms, 60_000);
  fail = false;
  await tick(60_000);
  assert.equal(players.length, 1);
});

test('a newer release takes over at the start of the initial loop, with the values and listeners', async () => {
  let release = 'r1';
  const { handle, players, tick } = setup([() => ok(release)()]);
  const poses: unknown[] = [];
  handle.on('pose', (e) => poses.push(e));
  handle.set('context::level', 3);
  await flush();
  const old = players[0];
  old.at = { clip: 'idle-cheer', time: 0.5 };
  release = 'r2';
  await tick(3_600_000);
  // It waits while another clip plays.
  assert.equal(players.length, 1);
  await tick(200);
  assert.equal(players.length, 1);
  old.at = { clip: 'idle-loop', time: 0.1 };
  await tick(200);
  assert.equal(players.length, 2);
  const fresh = players[1];
  assert.equal(fresh.over, true);
  assert.deepEqual(fresh.calls, ['set context::level=3']);
  // The old one plays on until the new one shows its first clip.
  assert.equal(old.destroyed, false);
  fresh.emit('clip', { clip: 'idle-loop', kind: 'pose', rate: 1 });
  assert.equal(old.destroyed, true);
  assert.equal(fresh.settled, true);
  fresh.emit('pose', { pose: 'idle' });
  assert.deepEqual(poses, [{ pose: 'idle' }]);
  handle.fire('action::cheer');
  assert.equal(fresh.calls.at(-1), 'fire action::cheer');
});

test('a newer release that never shows is given up and the old one plays on', async () => {
  let release = 'r1';
  const { handle, players, tick } = setup([() => ok(release)()]);
  const errors: unknown[] = [];
  handle.on('error', (e) => errors.push(e));
  await flush();
  players[0].at = { clip: 'idle-loop', time: 0 };
  release = 'r2';
  await tick(3_600_000);
  assert.equal(players.length, 2);
  await tick(30_000);
  assert.equal(players[1].destroyed, true);
  assert.equal(players[0].destroyed, false);
  assert.deepEqual(errors, [{ code: 'refresh_failed', detail: 'the new release did not start' }]);
});

test('destroy stops asking', async () => {
  const { handle, timers, players } = setup([ok('r1')]);
  await flush();
  handle.destroy();
  assert.equal(players[0].destroyed, true);
  assert.equal(timers.length, 0);
});
