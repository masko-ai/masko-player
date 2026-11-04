// The player against a scripted fake video layer (spec/player.md, test 2):
// two slots, swap only after the first frame, loop restart, end and failure
// reports, the preload cache, sounds, events, position, pause and play, and
// signed-link refresh.

import { test, type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Engine, loadWasmSync } from '../src/engine.ts';
import { Player, type MaskoEventName, type PlayerOptions } from '../src/player.ts';
import { FakeHost, settle } from './fakes.ts';

loadWasmSync(readFileSync(join(import.meta.dirname, '..', 'src', 'wasm', 'masko_player_wasm_bg.wasm')));

const files = (id: string, sig = '1') => ({
  webm: `https://cdn.test/${id}.webm?sig=${sig}`,
  hevc: `https://cdn.test/${id}.mov?sig=${sig}`
});

/** The test delivery; `expires` signs its links, `fetched` is when the server made it. */
function delivery(sig = '1', expires?: string, fetched: string | null = '2026-10-23T10:00:00Z') {
  return {
    deliverySchemaVersion: 1,
    playerContract: 1,
    requires: ['any-state'],
    target: 'web',
    mode: 'selected',
    ...(expires
      ? {
          deliveryAccess: {
            urlMode: 'signed' as const,
            signedUrlExpiresAt: expires,
            ...(fetched ? { lastFetchedAt: fetched } : {})
          }
        }
      : {}),
    runtimeGraph: {
      runtimeGraphSchemaVersion: 1,
      version: '2.1',
      name: 'Test',
      initialNode: 'idle',
      autoPlay: true,
      nodes: [
        { id: 'idle', name: 'Idle', transparentThumbnailUrl: `https://cdn.test/idle.png?sig=${sig}` },
        { id: 'working', name: 'Working' }
      ],
      inputs: [{ name: 'behavior::working', type: 'boolean', default: false }],
      edges: [
        {
          id: 'idle-loop',
          source: 'idle',
          target: 'idle',
          isLoop: true,
          duration: 2,
          videos: files('idle-loop', sig),
          sound: { url: `https://cdn.test/hum.mp3?sig=${sig}`, loop: true, volume: 0.5 }
        },
        {
          id: 'work-loop',
          source: 'working',
          target: 'working',
          isLoop: true,
          duration: 2,
          hasAudio: true,
          videos: files('work-loop', sig)
        },
        {
          id: 'to-work',
          source: 'idle',
          target: 'working',
          isLoop: false,
          duration: 1,
          conditions: [{ input: 'behavior::working' }],
          videos: files('to-work', sig),
          sound: { url: `https://cdn.test/whoosh.mp3?sig=${sig}` },
          movement: { x: 120 }
        },
        {
          id: 'to-idle',
          source: 'working',
          target: 'idle',
          isLoop: false,
          duration: 1,
          conditions: [{ input: 'behavior::working', value: false }],
          videos: files('to-idle', sig)
        },
        {
          id: 'poke',
          source: 'idle',
          target: 'working',
          isLoop: false,
          duration: 1,
          conditions: [{ input: 'action::click' }],
          videos: files('poke', sig)
        }
      ]
    }
  };
}

const EVENTS: MaskoEventName[] = ['pose', 'clip', 'reactionDone', 'movement', 'error'];

async function mount(options: Partial<PlayerOptions> = {}, setup?: (host: FakeHost) => void) {
  const host = new FakeHost();
  setup?.(host);
  const player = new Player(host, { delivery: delivery(), seed: 1, ...options });
  const events: string[] = [];
  for (const name of EVENTS) {
    player.on(name, (e) => {
      const ev = e as Record<string, unknown>;
      events.push(
        name === 'pose'
          ? `pose:${ev.pose}`
          : name === 'clip'
            ? `clip:${ev.clip}`
            : name === 'error'
              ? `error:${ev.code}`
              : name === 'reactionDone'
                ? `done:${ev.name}:${ev.used}`
                : `movement:${ev.edge}`
      );
    });
  }
  await settle();
  const [a, b] = host.slots;
  return { host, player, events, a, b };
}

/** Starts and shows the idle loop in slot 0. */
async function started(options: Partial<PlayerOptions> = {}, setup?: (host: FakeHost) => void) {
  const m = await mount(options, setup);
  m.a.frame();
  await settle();
  return m;
}

/** Records each `clipShown` the player reports, as `edge@now`. */
function shownReports(t: TestContext): () => string[] {
  const spy = t.mock.method(Engine.prototype, 'clipShown');
  return () => spy.mock.calls.map((c) => `${c.arguments[0]}@${c.arguments[1]}`);
}

const blob = (id: string, ext = 'webm', sig = '1') => `blob:https://cdn.test/${id}.${ext}?sig=${sig}`;
const url = (id: string, ext = 'webm', sig = '1') => `https://cdn.test/${id}.${ext}?sig=${sig}`;

test('start: the first loop loads in a hidden slot and shows on its first frame', async () => {
  const { host, events, a, b } = await mount();
  assert.deepEqual(a.loads, [url('idle-loop')]);
  assert.equal(a.playing, true);
  assert.equal(a.muted, true);
  assert.equal(a.visible, false, 'nothing shows before the first frame');
  assert.deepEqual(events, ['pose:idle']);
  a.frame();
  await settle();
  assert.equal(a.visible, true);
  assert.equal(b.visible, false);
  assert.deepEqual(events, ['pose:idle', 'clip:idle-loop']);
  // Entering idle preloaded its moves, most important first.
  assert.deepEqual(host.files.requested, [url('to-work'), url('poke')]);
});

test("preload 'all': once the mascot shows, every other clip of the release loads too, and stays", async () => {
  const { host, a } = await mount({ preload: 'all' });
  a.frame();
  await settle();
  // What can play next first, as by default, then every other clip.
  assert.deepEqual(host.files.requested, [url('to-work'), url('poke'), url('idle-loop'), url('work-loop'), url('to-idle')]);
  assert.deepEqual(host.files.freed, [], 'nothing is dropped from the cache');
});

test('swap only after the next clip has a decoded frame, in one step, no frame without a mascot', async () => {
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(1999);
  assert.equal(b.loads.length, 0, 'an app move waits for the end of the loop');
  await host.advance(1);
  a.end();
  await settle();
  assert.deepEqual(b.loads, [blob('to-work')], 'the move comes from the preload cache');
  assert.equal(b.playing, true);
  assert.equal(b.visible, false);
  assert.equal(a.visible, true, 'the old clip stays on screen while the next one decodes');
  await host.advance(40);
  assert.equal(b.visible, false);
  b.frame();
  assert.equal(b.visible, true);
  assert.equal(a.visible, false);
  assert.equal(a.playing, false, 'the old slot is paused after the swap');
  const swap = host.log.slice(host.log.indexOf('slot1:show') - 0, host.log.indexOf('slot1:show') + 2);
  assert.deepEqual(swap, ['slot1:show', 'slot0:hide'], 'show the new slot, then hide the old one, in the same step');
  assert.equal(host.emptyScreens, 0);
  assert.ok(events.includes('clip:to-work'));
});

test('a pose clip shown again restarts in its slot, with no swap', async () => {
  const { host, events, a, b } = await started();
  await host.advance(2000);
  a.time = 2;
  a.end();
  await settle();
  assert.equal(a.seeks, 1, 'seek to the first frame');
  assert.equal(a.loads.length, 1, 'no new load');
  assert.equal(b.loads.length, 0, 'the other slot is untouched');
  assert.equal(a.visible, true);
  assert.equal(a.playing, true);
  assert.deepEqual(events.filter((e) => e.startsWith('clip:')), ['clip:idle-loop', 'clip:idle-loop']);
  assert.ok(!events.some((e) => e.startsWith('error:')), 'no missing_end: the end was reported');
});

test('clipEnded is reported once per showing, on the real end; stale ends are ignored', async () => {
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  b.frame();
  await host.advance(300);
  a.end(); // the old slot: stale
  await settle();
  assert.ok(!events.includes('pose:working'), 'a stale end does not end the move');
  await host.advance(700);
  b.end();
  b.end(); // the same showing again
  await settle();
  assert.equal(events.filter((e) => e === 'pose:working').length, 1);
  assert.deepEqual(a.loads.at(-1), blob('work-loop'), 'the target loop, preloaded with the move, loads in the free slot');
  assert.ok(!events.some((e) => e.startsWith('error:')));
});

test('a late end report is covered by the engine safety net, counted from the swap', async () => {
  const { host, events, a } = await mount();
  await host.advance(500);
  a.frame();
  await host.advance(2999);
  assert.ok(!events.includes('error:missing_end'));
  await host.advance(1);
  assert.ok(events.includes('error:missing_end'), 'the safety net runs on the wake timer, 2 s + 1 s after the first frame');
});

test('clipShown: at the swap, with the play time of the first frame, and at once on a loop restart', async (t) => {
  const reports = shownReports(t);
  const { host, player, a, b } = await mount();
  await host.advance(120);
  assert.deepEqual(reports(), [], 'not before the first frame');
  a.frame();
  await settle();
  assert.deepEqual(reports(), ['idle-loop@120']);
  await host.advance(2000);
  a.end();
  await settle();
  assert.equal(a.seeks, 1, 'the loop restarted in place');
  assert.deepEqual(reports(), ['idle-loop@120', 'idle-loop@2120'], 'a restart in place is on screen at once');
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  await host.advance(80);
  assert.deepEqual(reports().length, 2, 'the move is not shown while it decodes');
  b.frame();
  b.frame();
  await settle();
  assert.deepEqual(reports().slice(2), ['to-work@4200'], 'once per showing');
});

test('clipShown is never reported for a showing a later command overtook', async (t) => {
  const reports = shownReports(t);
  const { host, player, b } = await started();
  player.fire('action::click', { instant: true });
  await settle();
  assert.deepEqual(b.loads, [blob('poke')]);
  await host.advance(300);
  player.preview.showPose('idle');
  await settle();
  b.frame();
  await settle();
  assert.deepEqual(reports(), ['idle-loop@0', 'idle-loop@300'], 'the dropped move is not reported');
  assert.equal(b.visible, false);
});

test('a report that waits for play is dropped when a call kept from before overtakes it', async (t) => {
  const reports = shownReports(t);
  const { host, player, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  player.pause();
  player.reset();
  b.frame();
  await settle();
  player.play();
  await settle();
  assert.deepEqual(reports(), ['idle-loop@0'], 'the reset asked for the idle loop before the move was reported');
  assert.deepEqual(a.loads.at(-1), url('idle-loop'));
  a.frame();
  await settle();
  assert.deepEqual(reports(), ['idle-loop@0', 'idle-loop@2000']);
});

test('a first frame decoded while paused is reported when play resumes, at its play time', async (t) => {
  const reports = shownReports(t);
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  await host.advance(100);
  player.pause();
  b.frame();
  await settle();
  assert.equal(b.visible, true, 'the first frame shows while paused');
  assert.deepEqual(reports(), ['idle-loop@0'], 'the engine hears nothing while paused');
  await host.advance(60_000);
  player.play();
  await settle();
  assert.deepEqual(reports(), ['idle-loop@0', 'to-work@2100']);
  // The move lasts 1 s: its safety net is at 2100 + 1000 + 1000.
  await host.advance(1999);
  assert.ok(!events.includes('error:missing_end'));
  await host.advance(1);
  assert.ok(events.includes('error:missing_end'));
});

test('a slow first frame does not trip missing_end: the safety net counts from the swap', async () => {
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  // 2.5 s to the first frame: longer than the move's 1 s and its 1 s safety net.
  await host.advance(2500);
  assert.ok(!events.some((e) => e.startsWith('error:')));
  assert.ok(!events.includes('pose:working'), 'the engine waits for the move to show');
  assert.equal(a.visible, true, 'the idle loop stays on screen meanwhile');
  b.frame();
  await host.advance(1000);
  b.end();
  await settle();
  assert.ok(events.includes('pose:working'));
  assert.ok(!events.some((e) => e.startsWith('error:')), 'the move played in full');
});

test('a clip that never shows a frame counts as failed 10 s after it was asked for (clip_timeout)', async () => {
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  assert.deepEqual(b.loads, [blob('to-work')]);
  await host.advance(9999);
  assert.ok(!events.some((e) => e.startsWith('error:')));
  assert.ok(!events.includes('pose:working'));
  await host.advance(1);
  assert.ok(events.includes('error:clip_timeout'));
  assert.ok(events.includes('pose:working'), 'a move that times out skips to its target (rules 9.4)');
  assert.deepEqual(b.loads.at(-1), blob('work-loop'), 'the target loop takes the hidden slot');
  assert.equal(a.visible, true, 'the idle loop stays until then');
  b.frame();
  await settle();
  assert.equal(b.visible, true);
  assert.equal(a.visible, false);
  assert.equal(host.emptyScreens, 0);
  // Back to idle: the move's file is in the cache, so the move plays again.
  player.set('working', false);
  await host.advance(2000);
  b.end();
  await settle();
  a.frame();
  await host.advance(1000);
  a.end();
  await settle();
  b.frame();
  events.length = 0;
  player.set('working', true);
  await host.advance(2000);
  b.end();
  await settle();
  assert.ok(!events.some((e) => e.startsWith('error:')), 'the timed-out move was marked playable again');
  assert.deepEqual(a.loads.at(-1), blob('to-work'));
});

test('a file that fails before it shows: the other format once, then clipFailed', async () => {
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  assert.deepEqual(b.loads, [blob('to-work')]);
  b.fail();
  await settle();
  assert.deepEqual(b.loads, [blob('to-work'), url('to-work', 'mov')], 'HEVC after WebM, same hidden slot');
  assert.equal(a.visible, true, 'the old clip stays meanwhile');
  b.fail();
  await settle();
  assert.ok(events.includes('error:clip_failed'));
  assert.ok(events.includes('pose:working'), 'a failed move skips to its target (rules 9.4)');
  assert.deepEqual(b.loads.at(-1), blob('work-loop'), 'the target loop, preloaded with the move, takes the hidden slot');
  assert.equal(a.visible, true, 'the idle loop stays until then');
  b.frame();
  await settle();
  assert.equal(b.visible, true);
  assert.equal(a.visible, false);
  assert.equal(host.emptyScreens, 0);
});

test('the other format plays when the first fails', async () => {
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  b.fail();
  await settle();
  b.frame();
  await settle();
  assert.equal(b.visible, true);
  assert.ok(!events.includes('error:clip_failed'));
  assert.ok(events.includes('clip:to-work'));
});

test('one failed file reported twice (error event and play refusal) moves on one format only', async () => {
  const { a, events } = await mount({}, (h) => {
    const slot = h.slots[0];
    slot.refuseNext = 'NotSupportedError';
    slot.onLoad = () => {
      slot.onLoad = null;
      queueMicrotask(() => slot.fail());
    };
  });
  assert.deepEqual(a.loads, [url('idle-loop'), url('idle-loop', 'mov')], 'WebM, then HEVC once');
  a.frame();
  await settle();
  assert.equal(a.visible, true);
  assert.ok(!events.includes('error:clip_failed'));
});

test('a pose clip that fails on screen becomes the pose still', async () => {
  const { host, events, a } = await started();
  a.fail();
  await settle();
  assert.ok(events.includes('error:clip_failed'));
  assert.equal(host.still.visible, true);
  assert.equal(host.still.url, url('idle', 'png'));
  assert.equal(a.visible, false);
  assert.ok(events.includes('clip:still:idle'));
  assert.equal(host.emptyScreens, 0);
});

test('a preload that fails marks the clip unplayable, and a later load marks it playable', async () => {
  const { host, player, events, a, b } = await started({}, (h) => {
    h.files.broken.add(url('to-work'));
    h.files.broken.add(url('to-work', 'mov'));
  });
  assert.deepEqual(host.files.requested.slice(0, 3), [url('to-work'), url('poke'), url('to-work', 'mov')]);
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  assert.ok(events.includes('error:clip_failed'), 'the engine skips the move it was told cannot play');
  assert.ok(events.includes('pose:working'));
  assert.deepEqual(b.loads, [url('work-loop')], 'the broken move never loaded in a slot');
  b.frame();
  // Back to idle: its preloads run again and the file loads this time.
  host.files.broken.clear();
  player.set('working', false);
  await host.advance(2000);
  b.end();
  await settle();
  assert.deepEqual(a.loads.at(-1), blob('to-idle'));
  a.frame();
  await host.advance(1000);
  a.end();
  await settle();
  assert.equal(events.filter((e) => e === 'pose:idle').length, 2);
  assert.deepEqual(b.loads.at(-1), blob('idle-loop'), 'preloaded when the move back started');
  b.frame();
  events.length = 0;
  player.set('working', true);
  await host.advance(2000);
  b.end();
  await settle();
  assert.ok(!events.includes('error:clip_failed'), 'markPlayable: the move plays again');
  assert.deepEqual(a.loads.at(-1), blob('to-work'));
});

test('stills: shown in place of video once decoded, or nothing', async () => {
  const d = delivery();
  d.runtimeGraph.edges = d.runtimeGraph.edges.filter((e) => e.id !== 'idle-loop');
  const { host, events, a } = await mount({ delivery: d });
  assert.equal(a.loads.length, 0);
  assert.equal(host.still.visible, true);
  assert.ok(events.includes('clip:still:idle'));
  assert.deepEqual(
    (await mount({ delivery: d }, (h) => h.still.broken.add(url('idle', 'png')))).host.still.visible,
    false,
    'an image that cannot load shows nothing'
  );
});

test('sounds: looping and one-shot, with the stop rules of 9.6', async () => {
  const { host, player, a, b } = await started();
  const sounds = host.sounds.played;
  assert.equal(sounds.length, 1);
  assert.deepEqual([sounds[0].url, sounds[0].loop, sounds[0].volume], [url('hum', 'mp3'), true, 0.5]);
  await host.advance(2000);
  a.end();
  await settle();
  assert.equal(sounds.length, 1, 'the looping sound is not started again on the next loop');
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  assert.equal(sounds[0].state, 'stopped', 'a move stops every sound before its own');
  assert.equal(sounds.length, 2);
  assert.deepEqual([sounds[1].url, sounds[1].loop], [url('whoosh', 'mp3'), false]);
  b.frame();
  player.pause();
  assert.equal(sounds[1].state, 'paused');
  player.play();
  assert.equal(sounds[1].state, 'playing');
  await host.advance(1000);
  b.end();
  await settle();
  assert.equal(sounds[1].state, 'playing', 'entering a pose lets one-shot sounds finish');
  sounds[1].finish();
  // Back to idle: the hum starts again and entering working stops it.
  player.set('working', false);
  a.frame();
  await host.advance(2000);
  a.end();
  await settle();
  b.frame();
  await host.advance(1000);
  b.end();
  await settle();
  const hum = sounds.at(-1)!;
  assert.equal(hum.url, url('hum', 'mp3'));
  assert.equal(hum.state, 'playing');
});

test('events: pose, clip, movement, reactionDone, error', async () => {
  const { host, player, events, a, b } = await started();
  player.fire('nothing-listens');
  await host.advance(2000);
  a.end();
  await settle();
  assert.ok(events.includes('done:action::nothing-listens:false'), 'an unused fire is spent at the loop end');
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  assert.ok(!events.includes('movement:to-work'), 'movement waits for the move to be on screen');
  b.frame();
  assert.ok(events.includes('movement:to-work'));
  const all: unknown[] = [];
  player.on('movement', (e) => all.push(e));
  player.on('error', (e) => all.push(e));
  player.set('node::loopCount', 1);
  assert.deepEqual(all, [{ code: 'read_only', detail: 'node::loopCount' }]);
});

test('movement carries the edge data, its rate and how long it lasts', async () => {
  const { host, player, a, b } = await started();
  const got: unknown[] = [];
  player.on('movement', (e) => got.push(e));
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  b.frame();
  assert.deepEqual(got, [{ edge: 'to-work', movement: { x: 120 }, rate: 1, seconds: 1 }]);
});

test('a refused delivery is an error event with its code, and nothing plays', async () => {
  const d = { ...delivery(), deliverySchemaVersion: 2 };
  const { a, events } = await mount({ delivery: d });
  assert.deepEqual(events, ['error:unsupported_delivery']);
  assert.equal(a.loads.length, 0);
  const bad = await mount({ delivery: '{not json' });
  await bad.host.advance(1);
  assert.deepEqual(bad.events, ['error:unreadable']);
});

test('position: the clip on screen and its clip time', async () => {
  const { player, a } = await mount();
  assert.deepEqual(player.position(), { clip: null, time: 0 });
  a.frame();
  a.time = 0.75;
  assert.deepEqual(player.position(), { clip: 'idle-loop', time: 0.75 });
});

test('pause stops the clock, clips and timers; set waits; play resumes where it stopped', async () => {
  const { host, player, events, a } = await started();
  await host.advance(1000);
  player.pause();
  assert.equal(a.playing, false);
  assert.equal(host.pendingTimers, 0, 'no wake timer while paused');
  player.set('working', true);
  await host.advance(60_000);
  assert.ok(!events.includes('error:missing_end'), 'play time stopped: no safety net');
  player.play();
  assert.equal(a.playing, true);
  await host.advance(1999);
  assert.ok(!events.includes('error:missing_end'));
  await host.advance(1);
  // 1000 ms before the pause + 2000 after: the safety net at play time 3000.
  assert.ok(events.includes('error:missing_end'));
  assert.deepEqual(host.slots[1].loads, [blob('to-work')], 'the set made while paused was applied on play');
});

test('a hidden page holds play time like pause', async () => {
  const { host, player, events, a } = await started();
  player.setHidden(true);
  assert.equal(a.playing, false);
  await host.advance(10_000);
  assert.ok(!events.some((e) => e.startsWith('error:')));
  player.setHidden(false);
  assert.equal(a.playing, true);
});

test('autoplay blocked: first frame, tap-to-play overlay, clock held until play', async () => {
  const { host, player, events, a } = await mount({}, (h) => (h.slots[0].refuse = 'always'));
  assert.equal(host.overlay, true);
  assert.equal(a.visible, false);
  a.frame();
  await settle();
  assert.equal(a.visible, true, 'the first frame shows under the overlay');
  await host.advance(10_000);
  assert.ok(!events.includes('error:missing_end'), 'the clock waits for the tap');
  a.refuse = 'none';
  player.play();
  assert.equal(host.overlay, false);
  assert.equal(a.playing, true);
  await host.advance(3000);
  assert.ok(events.includes('error:missing_end'), 'the clock runs after the tap');
});

test('autoplay blocked: the next gesture anywhere starts it and primes the other slot', async () => {
  const { host, events, a, b } = await mount({}, (h) => (h.slots[0].refuse = 'always'));
  assert.equal(host.overlay, true);
  a.frame();
  await settle();
  // A gesture the browser does not count (a touch starting): still blocked, still waiting.
  host.gesture();
  await settle();
  assert.equal(host.overlay, true, 'refused again: the overlay stays');
  assert.equal(a.playing, false);
  a.refuse = 'none';
  host.gesture();
  assert.equal(host.overlay, false);
  assert.equal(a.playing, true, 'plays inside the gesture');
  assert.equal(b.primed, 2, 'the free slot is primed in each gesture');
  await host.advance(3000);
  assert.ok(events.includes('error:missing_end'), 'the clock runs after the gesture');
  host.gesture();
  assert.equal(b.primed, 2, 'no more priming once it plays');
});

test('voices: unmuted only with sound on and hasAudio, muted when the browser refuses', async () => {
  for (const [sound, refuse, muted] of [
    [false, 'none', true],
    [true, 'none', false],
    [true, 'unmuted', true]
  ] as const) {
    const { host, player, a, b } = await started({ sound }, (h) => (h.slots[0].refuse = refuse));
    player.set('working', true);
    await host.advance(2000);
    a.end();
    await settle();
    b.frame();
    await host.advance(1000);
    b.end();
    await settle();
    assert.equal(a.src, blob('work-loop'), 'the voiced loop plays in slot 0');
    assert.equal(a.muted, muted, `sound ${sound}, refuse ${refuse}`);
    assert.equal(a.playing, true);
    assert.equal(host.overlay, false, 'a refused voice is not a blocked autoplay');
  }
});

test('autoPlay false: the first frame waits, play() starts on the same slot', async (t) => {
  const reports = shownReports(t);
  const { host, player, events, a, b } = await mount({ autoPlay: false });
  assert.deepEqual(a.loads, [url('idle-loop')]);
  assert.equal(a.playing, false);
  a.frame();
  await settle();
  assert.equal(a.visible, true, 'the first pose first frame');
  assert.ok(!events.includes('pose:idle'), 'not started');
  assert.deepEqual(reports(), [], 'the engine did not ask for the first frame');
  await host.advance(5000);
  assert.ok(!events.some((e) => e.startsWith('error:')), 'no clock before play');
  player.play();
  await settle();
  assert.ok(events.includes('pose:idle'));
  assert.equal(a.loads.length, 1, 'the start restarts the clip already shown');
  assert.deepEqual(reports(), ['idle-loop@0'], 'and reports it shown at once');
  assert.equal(a.playing, true);
  assert.equal(b.loads.length, 0);
});

test('values set before play are read by the start, at play time 0', async () => {
  const { player, events, a } = await mount({ autoPlay: false });
  player.set('working', true);
  player.play();
  await settle();
  assert.deepEqual(events.slice(0, 1), ['pose:idle']);
  assert.deepEqual(a.loads.at(-1), blob('to-work'), 'the move starts on entry (rules 11.1, 5.1)');
});

test('a click the app fires waits for the end of the loop unless instant', async () => {
  const { player, events, b } = await started();
  player.fire('action::click');
  await settle();
  assert.deepEqual(b.loads, [], 'it waits for the end of the loop (rules 5.2, 5.3)');
  player.fire('action::click', { instant: true });
  await settle();
  assert.deepEqual(b.loads, [blob('poke')], 'instant starts it now (rules 5.8)');
  b.frame();
  assert.ok(events.includes('clip:poke'));
  assert.ok(!events.some((e) => e.startsWith('error:')));
});

test('signed links: getDelivery before expiry swaps URLs only; cached files keep playing', async () => {
  let calls = 0;
  const start = Date.parse('2026-10-23T10:00:00Z');
  const expires = (ms: number) => new Date(start + ms).toISOString();
  const { host, player, events, a, b } = await started({
    delivery: delivery('1', expires(20 * 60_000)),
    getDelivery: async () => {
      calls++;
      return { data: { delivery: delivery('2', expires(40 * 60_000), expires(15 * 60_000)) } };
    }
  });
  await host.advance(14 * 60_000);
  assert.equal(calls, 0);
  events.length = 0;
  await host.advance(60_000 + 1);
  assert.equal(calls, 1, 'asked five minutes before expiry');
  assert.ok(
    !events.some((e) => e.startsWith('pose:') || e.startsWith('error:refresh')),
    'the engine keeps running on the same pose'
  );
  // The move was cached under the old link: it still plays from the cache.
  player.set('working', true);
  a.end();
  await settle();
  assert.deepEqual(b.loads.at(-1), blob('to-work', 'webm', '1'));
  b.frame();
  b.end();
  await settle();
  assert.deepEqual(a.loads.at(-1), blob('work-loop', 'webm', '2'), 'new loads use the fresh links');
  // The next refresh follows the new expiry: five minutes before 10:40.
  await host.advance(19 * 60_000);
  assert.equal(calls, 1);
  await host.advance(60_000);
  assert.equal(calls, 2);
});

test('signed links: another release or a failure is an error, and it tries again', async () => {
  const start = Date.parse('2026-10-23T10:00:00Z');
  const other = delivery('2');
  other.runtimeGraph.edges = other.runtimeGraph.edges.filter((e) => e.id !== 'poke');
  let fail = true;
  let calls = 0;
  const { host, events } = await started({
    delivery: delivery('1', new Date(start + 15 * 60_000).toISOString()),
    getDelivery: () => {
      calls++;
      if (fail) throw new Error('offline');
      return other;
    }
  });
  await host.advance(10 * 60_000);
  assert.ok(events.includes('error:refresh_failed'));
  assert.equal(calls, 1);
  fail = false;
  await host.advance(30_000);
  assert.ok(events.includes('error:refresh_mismatch'), 'tried again 30 s later');
  assert.equal(calls, 2);
});

test('reset and preview calls go through the engine', async () => {
  const { player, events, b } = await started();
  events.length = 0;
  player.preview.playMove('to-work');
  await settle();
  assert.deepEqual(b.loads, [blob('to-work')]);
  player.preview.showPose('nowhere');
  assert.ok(events.includes('error:unknown_pose'));
  player.reset();
  await settle();
  assert.ok(events.includes('pose:idle'));
});

test('destroy: timers, sounds, slots and cache go; nothing fires after', async () => {
  const { host, player, events, a } = await started();
  const n = events.length;
  player.destroy();
  assert.equal(host.disposed, true);
  assert.equal(host.pendingTimers, 0);
  assert.equal(host.sounds.played[0].state, 'stopped');
  assert.equal(a.src, null);
  assert.deepEqual(host.files.freed.sort(), [blob('poke'), blob('to-work')]);
  a.end();
  player.set('working', true);
  await host.advance(10_000);
  assert.equal(events.length, n);
});

test('an app call made in an event handler runs after the batch that told it, never inside it', async () => {
  const d = delivery();
  (d.runtimeGraph.edges as unknown[]).push({
    id: 'drop',
    source: 'working',
    target: 'idle',
    isLoop: false,
    duration: 1,
    priority: 5,
    conditions: [{ input: 'behavior::heldContact' }],
    videos: files('drop')
  });
  const { host, player, events, a, b } = await started({ delivery: d });
  // An instant value set as the pose is entered starts its move at once (rules 5.8).
  player.on('pose', ({ pose }) => {
    if (pose === 'working') player.set('heldContact', true, { instant: true });
  });
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  b.frame();
  await host.advance(1000);
  b.end();
  await settle();
  // The batch of the move's end asked for the working loop after the pose event:
  // the handler's call comes after it, so its move wins.
  assert.deepEqual(a.loads, [url('idle-loop'), blob('drop')], 'the move the handler started, not the loop asked for before');
  a.frame();
  await settle();
  assert.ok(events.includes('clip:drop'));
  assert.ok(!events.includes('clip:work-loop'));
});

test('pause while the next clip waits for its cached file: no stale frame, no early clip event', async () => {
  const { host, player, events, a, b } = await started();
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  b.frame();
  await settle();
  // The preloads of the working pose are slow from here on.
  host.files.hold = true;
  await host.advance(1000);
  b.end();
  await settle();
  a.frame();
  await settle();
  player.set('working', false);
  await host.advance(2000);
  a.end();
  await settle();
  // The move back waits for its file; the hidden slot still holds the old move.
  assert.equal(b.src, blob('to-work'));
  player.pause();
  await settle();
  assert.equal(b.visible, false, 'the old move is not shown as the new one');
  assert.equal(a.visible, true);
  assert.ok(!events.includes('clip:to-idle'));
  host.files.release();
  await settle();
  assert.equal(b.src, blob('to-idle'));
  assert.equal(b.playing, false, 'loaded while paused, not played');
  assert.equal(b.visible, false, 'not before its first frame');
  b.frame();
  await settle();
  assert.equal(b.visible, true, 'its first frame shows while paused');
  assert.ok(events.includes('clip:to-idle'));
  assert.equal(host.emptyScreens, 0);
});

test('signed links: a device clock a day ahead of the server never makes the refresh loop', async () => {
  for (const withFetched of [true, false]) {
    const host = new FakeHost();
    const asked: number[] = [];
    // The server's clock is 25 h behind the device's; its links last 24 h.
    const fresh = () => {
      const server = host.wall - 25 * 3600_000;
      const sig = String(asked.length + 1);
      return delivery(sig, new Date(server + 24 * 3600_000).toISOString(), withFetched ? new Date(server).toISOString() : null);
    };
    const getDelivery = () => {
      asked.push(host.t);
      return fresh();
    };
    const player = new Player(host, { delivery: fresh(), seed: 1, getDelivery });
    await settle();
    // Only the refresh timer runs.
    player.pause();
    if (withFetched) {
      await host.advance(23 * 3600_000 + 54 * 60_000);
      assert.deepEqual(asked, [], 'lastFetchedAt measures the 24 h on the server clock');
      await host.advance(60_000);
      assert.equal(asked.length, 1, 'five minutes before they expire');
    } else {
      await host.advance(5 * 60_000);
      assert.ok(asked.length >= 2 && asked.length <= 11, `links that look expired: ${asked.length} asks in 5 minutes`);
      for (let i = 1; i < asked.length; i++) assert.ok(asked[i] - asked[i - 1] >= 30_000, 'never sooner than 30 s after the last');
    }
    player.destroy();
  }
});

test('signed links: a file cached without CORS (its URL, not a copy) plays from the fresh link after a refresh', async () => {
  const start = Date.parse('2026-10-23T10:00:00Z');
  const at = (ms: number) => new Date(start + ms).toISOString();
  const { host, player, a, b } = await started(
    {
      delivery: delivery('1', at(20 * 60_000)),
      getDelivery: () => delivery('2', at(40 * 60_000), at(15 * 60_000))
    },
    (h) => {
      // FetchLoader without CORS: the browser's cache holds the file, the loader returns its URL.
      h.files.load = (u: string) => {
        h.files.requested.push(u);
        return Promise.resolve(u);
      };
    }
  );
  assert.ok(host.files.requested.includes(url('to-work')), 'cached under the old link');
  await host.advance(15 * 60_000 + 10);
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await settle();
  assert.equal(b.src, url('to-work', 'webm', '2'), 'the fresh link, not the one that expires');
});

test('sound turned on reaches a looping pose clip at its next loop; a refused voice is not asked again each loop', async (t) => {
  const d = delivery();
  (d.runtimeGraph.edges[0] as Record<string, unknown>).hasAudio = true;
  const { host, player, a } = await started({ delivery: d, sound: false });
  const plays = t.mock.method(a, 'play');
  const loop = async () => {
    await host.advance(2000);
    a.end();
    await settle();
  };
  assert.equal(a.muted, true);
  player.setSound(true);
  await loop();
  assert.equal(a.loads.length, 1, 'the same clip restarted in place');
  assert.equal(a.muted, false, 'the next loop is heard');
  player.setSound(false);
  await loop();
  assert.equal(a.muted, true, 'and muted again');
  // The browser refuses sound without a gesture.
  a.refuse = 'unmuted';
  player.setSound(true);
  await loop();
  assert.equal(a.muted, true);
  assert.equal(a.playing, true, 'played muted');
  plays.mock.resetCalls();
  await loop();
  await loop();
  assert.deepEqual(
    plays.mock.calls.map((c) => c.arguments[1]),
    [true, true],
    'no unmuted try on every loop'
  );
  a.refuse = 'none';
  player.setSound(true);
  await loop();
  assert.equal(a.muted, false, 'the app asks again: heard');
});

test('autoPlay false: the first frame follows the values set before the start, before or after the engine loads', async () => {
  const d = delivery();
  (d.runtimeGraph.edges as unknown[]).push({
    id: 'any-work',
    source: '*',
    target: 'working',
    isLoop: false,
    duration: 1,
    conditions: [{ input: 'behavior::working' }],
    videos: files('any-work')
  });
  // Set while the engine loads.
  const host = new FakeHost();
  const early = new Player(host, { delivery: d, seed: 1, autoPlay: false });
  early.set('working', true);
  await settle();
  const [a, b] = host.slots;
  assert.deepEqual(a.loads, [url('work-loop')], 'the pose the value chose (rules 11.1)');
  a.frame();
  await settle();
  early.play();
  await settle();
  assert.deepEqual(a.loads, [url('work-loop')], 'the start restarts the clip already shown');
  assert.equal(b.loads.length, 0);
  // Set after the engine loaded, before play.
  const m = await mount({ delivery: d, autoPlay: false });
  assert.deepEqual(m.a.loads, [url('idle-loop')]);
  m.a.frame();
  await settle();
  m.player.set('working', true);
  await settle();
  assert.deepEqual(m.b.loads, [url('work-loop')], 'the first frame follows the value');
  m.b.frame();
  await settle();
  assert.equal(m.b.visible, true);
  assert.equal(m.a.visible, false);
  m.player.set('working', true);
  await settle();
  assert.deepEqual([m.a.loads.length, m.b.loads.length], [1, 1], 'the same first frame is not shown again');
  m.player.play();
  await settle();
  assert.ok(m.events.includes('pose:working'));
  assert.deepEqual(m.b.loads, [url('work-loop')], 'the start plays the clip on screen');
});

test('signed links: after the device slept past the refresh time, the links are checked when the page shows or comes online', async () => {
  const start = Date.parse('2026-10-23T10:00:00Z');
  let calls = 0;
  const fresh = (sig: string) => {
    const now = start + calls * 2 * 3600_000;
    return delivery(sig, new Date(now + 3600_000).toISOString(), new Date(now).toISOString());
  };
  for (const wake of ['shown', 'online'] as const) {
    calls = 0;
    const { host, player } = await started({
      delivery: fresh('1'),
      getDelivery: () => {
        calls++;
        return fresh(String(calls + 1));
      }
    });
    player.setHidden(true);
    // Asleep for 2 hours: the wall clock moves on, timers do not.
    host.wall += 2 * 3600_000;
    await host.advance(0);
    assert.equal(calls, 0);
    if (wake === 'shown') player.setHidden(false);
    else player.checkLinks();
    await host.advance(0);
    assert.equal(calls, 1, `asked at once when ${wake}`);
    player.destroy();
  }
});

test('one bad value never skips the rest of a batch: a volume that is not a number, a host call that throws', async () => {
  for (const [volume, heard] of [
    ['loud', 1],
    [2, 1],
    [-1, 0],
    [null, 1]
  ] as const) {
    const d = delivery();
    d.runtimeGraph.edges[0].sound!.volume = volume as unknown as number;
    const { host } = await started({ delivery: d });
    assert.equal(host.sounds.played[0].volume, heard, `volume ${volume}`);
  }
  const { host, player, a, b } = await started();
  const play = host.sounds.play.bind(host.sounds);
  host.sounds.play = (u, v, l) => {
    if (u.includes('whoosh')) throw new Error('setter refused');
    return play(u, v, l);
  };
  player.set('working', true);
  await host.advance(2000);
  a.end();
  await assert.rejects(host.advance(0), /setter refused/, 'reported outside');
  await settle();
  assert.deepEqual(b.loads, [blob('to-work')], 'the move still shows');
  assert.ok(host.files.requested.includes(url('work-loop')), 'and its target loop still preloads');
});

test('calls kept while paused: the last set of a name, a repeated fire once, at most 1000', async (t) => {
  const sets = t.mock.method(Engine.prototype, 'set');
  const fires = t.mock.method(Engine.prototype, 'fire');
  const { player } = await started();
  player.pause();
  sets.mock.resetCalls();
  for (let i = 0; i < 3000; i++) player.set(`score${i}`, i);
  for (let i = 0; i < 5000; i++) player.set('working', i % 2 === 0);
  for (let i = 0; i < 100; i++) player.fire('celebrate');
  player.play();
  await settle();
  const names = sets.mock.calls.map((c) => c.arguments[0]);
  assert.equal(names.length + fires.mock.calls.length, 1000, 'the oldest go first');
  assert.equal(names.filter((n) => n === 'working').length, 1);
  assert.equal(sets.mock.calls.find((c) => c.arguments[0] === 'working')!.arguments[1], false, 'its last value');
  assert.equal(fires.mock.calls.length, 1);
  assert.equal(names[0], 'score2002');
});

const waveFiles = { webm: url('wave'), hevc: url('wave', 'mov') };

test('one clip: its file plays in a loop as one pose, from the length it reads', async () => {
  const { host, events, a } = await mount({ delivery: undefined, clip: waveFiles }, (h) => h.lengths.set(url('wave'), 3));
  assert.deepEqual(a.loads, [url('wave')]);
  a.frame();
  await settle();
  assert.deepEqual(events, ['pose:clip', 'clip:clip']);
  await host.advance(3000);
  a.end();
  await settle();
  assert.equal(a.seeks, 1, 'the loop restarts in its slot');
  assert.ok(!events.some((e) => e.startsWith('error:')));
});

test('one clip: the safety net counts the length read from the file', async () => {
  const { host, events, a } = await mount({ delivery: undefined, clip: waveFiles }, (h) => h.lengths.set(url('wave'), 3));
  a.frame();
  await host.advance(3999);
  assert.ok(!events.includes('error:missing_end'));
  await host.advance(1);
  assert.ok(events.includes('error:missing_end'), '3 s + 1 s after the first frame');
});

test('one clip: the other format gives the length when the preferred one cannot be read', async () => {
  const { host, events, a } = await mount({ delivery: undefined, clip: waveFiles }, (h) =>
    h.lengths.set(url('wave', 'mov'), 2)
  );
  assert.ok(host.log.includes(`length:${url('wave')}`));
  assert.deepEqual(a.loads, [url('wave')], 'the preferred file still plays first');
  assert.deepEqual(events, ['pose:clip']);
});

test('one clip: nothing plays when no file length can be read (clip_failed)', async () => {
  const { events, a } = await mount({ delivery: undefined, clip: waveFiles });
  assert.deepEqual(a.loads, []);
  assert.deepEqual(events, ['error:clip_failed']);
});

test('one clip: its own voice is heard with sound on, muted by default', async () => {
  for (const sound of [false, true]) {
    const { a } = await mount({ delivery: undefined, clip: waveFiles, sound }, (h) => h.lengths.set(url('wave'), 3));
    assert.equal(a.playing, true);
    assert.equal(a.muted, !sound, `sound ${sound}`);
  }
});

test("sound is on by default; a voice the browser refused is heard after the person's first gesture", async () => {
  const d = delivery();
  (d.runtimeGraph.edges[0] as Record<string, unknown>).hasAudio = true;
  const { host, a } = await mount({ delivery: d }, (h) => (h.slots[0].refuse = 'unmuted'));
  a.frame();
  await settle();
  assert.equal(a.muted, true, 'refused: played muted');
  assert.equal(a.playing, true);
  a.refuse = 'none';
  host.gesture();
  await host.advance(2000);
  a.end();
  await settle();
  assert.equal(a.muted, false, 'the next loop is heard');
});

/** The test delivery with an Any State edge to working on `input`, ranked above the moves. */
function withAnyWork(input: string) {
  const d = delivery();
  (d.runtimeGraph.edges as unknown as Record<string, unknown>[]).push({
    id: 'go-work',
    source: '*',
    target: 'working',
    isLoop: false,
    duration: 0,
    priority: 1,
    conditions: [{ input }],
    videos: {}
  });
  return d;
}

test('instant: an Any State start enters its pose at once, with no move clip, movement or move sound', async () => {
  const { host, player, events, b } = await started({ delivery: withAnyWork('behavior::working') });
  await host.advance(500);
  player.set('working', true, { instant: true });
  await settle();
  assert.equal(events.at(-1), 'pose:working', 'the pose is entered mid-loop');
  assert.equal(b.loads.length, 1);
  assert.ok(b.loads[0].includes('work-loop'), 'the next clip is the pose clip, not the move');
  b.frame();
  await settle();
  assert.equal(events.at(-1), 'clip:work-loop');
  assert.ok(!events.includes('clip:to-work') && !events.includes('movement:to-work'));
  const sounds = host.sounds.played;
  assert.deepEqual(
    sounds.map((s) => [s.url, s.state]),
    [[url('hum', 'mp3'), 'stopped']],
    'no move sound; entering stops the looping one'
  );
});

test('instant: a move set with instant plays its own clip at once, with its movement and sound', async () => {
  const { host, player, events, b } = await started();
  await host.advance(500);
  player.set('working', true, { instant: true });
  await settle();
  assert.ok(b.loads.at(-1)!.includes('to-work'));
  b.frame();
  await settle();
  assert.ok(events.includes('clip:to-work') && events.includes('movement:to-work'), 'mid-loop, not at its end');
  assert.ok(!events.includes('pose:working'));
  assert.deepEqual(
    host.sounds.played.map((s) => [s.url, s.state]),
    [
      [url('hum', 'mp3'), 'stopped'],
      [url('whoosh', 'mp3'), 'playing']
    ]
  );
});

test('instant: a fire kept while paused becomes instant when it is fired again with instant', async () => {
  const { player, events, b } = await started({ delivery: withAnyWork('action::beam') });
  player.pause();
  player.fire('beam');
  player.fire('beam', { instant: true });
  player.play();
  await settle();
  // Straight into working with no clip on the way: its loop is the next clip.
  assert.equal(events.at(-1), 'pose:working');
  assert.ok(!events.includes('clip:to-work'), 'the route did not play');
  assert.ok(b.loads.at(-1)!.includes('work-loop'));
});
