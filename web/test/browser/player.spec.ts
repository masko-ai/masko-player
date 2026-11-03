// The player on Chromium's real decoder (spec/player.md, test 3), with
// transparent VP9 clips: generated ones whose colour tells which clip is on
// screen, and real Masko clips.

import { expect, test, type Page } from '@playwright/test';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

interface Sample {
  t: number;
  visible: string[];
  still: boolean;
  ready: number[];
  presented: number[];
  src: string[];
  time: number[];
  opaque: number;
  color: string | null;
}

interface Event {
  event: string;
  t: number;
  clip?: string;
  pose?: string;
  code?: string;
  [key: string]: unknown;
}

type Win = Window & {
  ready: boolean;
  start: (o?: unknown) => boolean;
  handle: { set(n: string, v: unknown): void; pause(): void; play(): void; position(): { clip: string | null; time: number } };
  events: Event[];
  watcher: { samples: Sample[]; stop(): void };
};

async function open(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await page.waitForFunction(() => (window as unknown as Win).ready === true);
}

async function start(page: Page, options: Record<string, unknown> = {}): Promise<void> {
  await open(page, '/pages/esm.html');
  await page.evaluate((o) => (window as unknown as Win).start(o), options);
}

async function waitClip(page: Page, clip: string, count = 1, timeout = 10_000): Promise<void> {
  await page.waitForFunction(
    ([c, n]) => (window as unknown as Win).events.filter((e) => e.event === 'clip' && e.clip === c).length >= (n as number),
    [clip, count] as const,
    { timeout }
  );
}

async function result(page: Page): Promise<{ samples: Sample[]; events: Event[] }> {
  return page.evaluate(() => {
    const w = window as unknown as Win;
    w.watcher.stop();
    return { samples: w.watcher.samples, events: w.events };
  });
}

/**
 * From the first frame with a mascot on, at every animation frame: exactly
 * one layer visible, the visible slot has presented a decoded frame of its
 * file (a video frame callback since its source was set, counted before
 * this frame), and the frame drawn shows the mascot. (`readyState` is not a
 * test: Chromium updates it after the frame is presented, and it drops while
 * a loop seeks back to 0 with the mascot still on screen.)
 */
function expectAlwaysAMascot(samples: Sample[], minOpaque = 30): Sample[] {
  const first = samples.findIndex((s) => s.visible.length === 1 || s.still);
  expect(first, 'the mascot shows').toBeGreaterThanOrEqual(0);
  const after = samples.slice(first);
  const bad = after.filter(
    (s) =>
      s.visible.length + (s.still ? 1 : 0) !== 1 ||
      (s.visible.length === 1 && s.presented[0] < 1) ||
      s.opaque < minOpaque
  );
  expect(bad.slice(0, 5), `${bad.length} of ${after.length} frames without a decoded mascot`).toEqual([]);
  return after;
}

function collapse<T>(list: T[]): T[] {
  return list.filter((x, i) => i === 0 || x !== list[i - 1]);
}

test('set() plays the move, then the target loop; swaps wait for a decoded frame; clip ends drive the loop', async ({ page }) => {
  const requests: string[] = [];
  page.on('request', (r) => requests.push(new URL(r.url()).pathname));
  await start(page, { sound: true });
  await waitClip(page, 'idle-loop');
  await page.waitForTimeout(500);
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop');
  const position = await page.evaluate(() => (window as unknown as Win).handle.position());
  expect(position.clip).toBe('work-loop');
  expect(position.time).toBeGreaterThanOrEqual(0);
  expect(position.time).toBeLessThan(2.1);
  await waitClip(page, 'work-loop', 3);
  const { samples, events } = await result(page);

  const after = expectAlwaysAMascot(samples);
  expect(collapse(after.map((s) => s.color))).toEqual(['idle-loop', 'to-work', 'work-loop']);
  // The move took the other slot; the target loop the free one, then stayed in it.
  const slotOf = (c: string) => [...new Set(after.filter((s) => s.color === c).map((s) => s.visible[0]))];
  expect(slotOf('idle-loop')).toHaveLength(1);
  expect(slotOf('to-work')).toHaveLength(1);
  expect(slotOf('work-loop')).toHaveLength(1);
  expect(slotOf('to-work')[0]).not.toBe(slotOf('idle-loop')[0]);
  expect(slotOf('work-loop')[0]).not.toBe(slotOf('to-work')[0]);

  const names = collapse(events.map((e) => `${e.event}:${e.clip ?? e.pose ?? e.code ?? e.edge ?? ''}`));
  expect(names).toEqual([
    'pose:idle',
    'clip:idle-loop',
    'clip:to-work',
    'movement:to-work',
    'pose:working',
    'clip:work-loop'
  ]);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
  // clipEnded on the real end: a new loop every 2 s, no safety net.
  const loops = events.filter((e) => e.event === 'clip' && e.clip === 'work-loop').map((e) => e.t);
  for (let i = 1; i < loops.length; i++) {
    expect(loops[i] - loops[i - 1]).toBeGreaterThan(1900);
    expect(loops[i] - loops[i - 1]).toBeLessThan(2400);
  }
  // The move was preloaded at the pose entry, and its sound played.
  expect(requests.filter((p) => p === '/clips/to-work.webm').length).toBe(1);
  expect(requests).toContain('/clips/tick.wav');
  expect(requests.indexOf('/clips/to-work.webm')).toBeLessThan(requests.indexOf('/clips/tick.wav'));
});

test('a loop slower than its preload: the move holds its last frame until the loop has a decoded frame', async ({ page }) => {
  // The loop is preloaded when the move starts; this one takes longer than the move.
  await start(page, { query: { 'work-loop': '?delay=2500' } });
  await waitClip(page, 'idle-loop');
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop', 1, 15_000);
  await page.waitForTimeout(300);
  const { samples } = await result(page);
  const after = expectAlwaysAMascot(samples);
  expect(collapse(after.map((s) => s.color))).toEqual(['idle-loop', 'to-work', 'work-loop']);
  const held = after.filter((s) => s.color === 'to-work' && s.time[0] >= 0.95);
  expect(held.length, 'frames on the move last frame while the loop loads').toBeGreaterThan(20);
  expect(held.at(-1)!.t - held[0].t).toBeGreaterThan(500);
});

test('a WebM that fails: the other file plays, once', async ({ page }) => {
  const failed: string[] = [];
  page.on('response', (r) => r.status() === 404 && failed.push(new URL(r.url()).pathname + new URL(r.url()).search));
  await start(page, { broken: ['to-work'] });
  await waitClip(page, 'idle-loop');
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop');
  const { samples, events } = await result(page);
  expectAlwaysAMascot(samples);
  expect(failed).toEqual(['/clips/to-work.webm?status=404']);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
  expect(collapse(samples.map((s) => s.color)).filter(Boolean)).toEqual(['idle-loop', 'to-work', 'work-loop']);
});

test('a move whose files all fail is skipped: clip_failed, then the target loop', async ({ page }) => {
  await start(page, { dead: ['to-work'] });
  await waitClip(page, 'idle-loop');
  await page.waitForTimeout(300);
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop');
  const { samples, events } = await result(page);
  expectAlwaysAMascot(samples);
  expect(collapse(samples.map((s) => s.color)).filter(Boolean)).toEqual(['idle-loop', 'work-loop']);
  expect(events.map((e) => `${e.event}:${e.clip ?? e.pose ?? e.code}`)).toContain('error:clip_failed');
  expect(events.map((e) => `${e.event}:${e.pose ?? ''}`)).toContain('pose:working');
});

test('the one-file script build plays on a plain page, with the engine inlined', async ({ page }) => {
  const requests: string[] = [];
  page.on('request', (r) => requests.push(new URL(r.url()).pathname));
  await open(page, '/pages/script.html');
  await page.evaluate(() => {
    const w = window as unknown as Win & { testDelivery(): unknown; observe(): boolean; start(d: unknown): boolean };
    w.start(w.testDelivery());
    return w.observe();
  });
  await waitClip(page, 'idle-loop');
  await page.evaluate(() => (window as unknown as { mascot: { set(n: string, v: boolean): void } }).mascot.set('working', true));
  await waitClip(page, 'work-loop', 2);
  const { samples, events } = await result(page);
  const after = expectAlwaysAMascot(samples);
  expect(collapse(after.map((s) => s.color))).toEqual(['idle-loop', 'to-work', 'work-loop']);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
  expect(requests.filter((p) => p.endsWith('.wasm'))).toEqual([]);
  expect(requests.filter((p) => p.startsWith('/dist/'))).toEqual(['/dist/masko-player.js']);
  // Video time moves on in the visible slot.
  const times = after.filter((s) => s.color === 'work-loop').map((s) => s.time[0]);
  expect(Math.max(...times)).toBeGreaterThan(1);
  expect(await page.evaluate(() => typeof (window as unknown as { MaskoPlayer: { version: string } }).MaskoPlayer.version)).toBe(
    'string'
  );
});

test('one clip: a single file plays on its own and loops, with a mascot on every frame', async ({ page }) => {
  await open(page, '/pages/esm.html');
  await page.evaluate(() => {
    const clip = { webm: `${location.origin}/clips/idle-loop.webm`, hevc: `${location.origin}/clips/idle-loop.mov` };
    return (window as unknown as Win).start({ clip });
  });
  await waitClip(page, 'clip', 3);
  const { samples, events } = await result(page);
  const after = expectAlwaysAMascot(samples);
  expect(collapse(after.map((s) => s.color))).toEqual(['idle-loop']);
  expect(events.filter((e) => e.event === 'pose').map((e) => e.pose)).toEqual(['clip']);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
});

test('React: <MaskoPlayer> with a ref and event props, one player under StrictMode, new links swap without a remount', async ({
  page
}) => {
  const requests: string[] = [];
  page.on('request', (r) => requests.push(new URL(r.url()).pathname + new URL(r.url()).search));
  await open(page, '/pages/react.html');
  await waitClip(page, 'idle-loop');
  expect(await page.locator('.masko-player').count()).toBe(1);
  await page.evaluate(() => {
    const w = window as unknown as { setDelivery(d: unknown): void; testDelivery(o: unknown): unknown };
    // The same release with new links.
    w.setDelivery(w.testDelivery({ version: '?v=2' }));
  });
  await page.waitForTimeout(200);
  await page.evaluate(() => (window as unknown as { mascot: { set(n: string, v: boolean): void } }).mascot.set('working', true));
  await waitClip(page, 'work-loop');
  const events = await page.evaluate(() => (window as unknown as Win).events);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
  expect(events.filter((e) => e.event === 'pose').map((e) => e.pose)).toEqual(['idle', 'working']);
  expect(await page.locator('.masko-player').count()).toBe(1);
  expect(requests).toContain('/clips/work-loop.webm?v=2');
});

test('the player does not watch the person: the app fires the click', async ({ page }) => {
  await start(page);
  await waitClip(page, 'idle-loop');
  const root = page.locator('.masko-player');
  expect(await root.getAttribute('role')).toBeNull();
  expect(await root.getAttribute('tabindex')).toBeNull();
  await page.locator('#m').hover();
  await page.locator('#m').click();
  await page.keyboard.press('Enter');
  // Two loops later, only the idle loop has played (rules 3.5).
  await waitClip(page, 'idle-loop', 3);
  const poked = await page.evaluate(() =>
    (window as unknown as Win).events.some((e) => e.event === 'clip' && e.clip === 'poke')
  );
  expect(poked).toBe(false);
  await page.evaluate(() =>
    (window as unknown as { handle: { fire(n: string, o: { instant: boolean }): void } }).handle.fire(
      'action::click',
      { instant: true }
    )
  );
  await waitClip(page, 'poke');
  const { events } = await result(page);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
});

test('autoplay blocked: the first frame under a tap-to-play overlay; a tap starts it', async ({ page }) => {
  // The browser's policy, as on an iPhone in Low Power Mode: no playback
  // before the person's first real tap. (Playwright's evaluate counts as a
  // user gesture, so the policy watches trusted input events only.)
  await page.addInitScript(() => {
    let tapped = false;
    addEventListener('pointerdown', (e) => (tapped ||= e.isTrusted), true);
    const play = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = function (this: HTMLMediaElement) {
      if (!tapped) return Promise.reject(new DOMException('autoplay blocked', 'NotAllowedError'));
      return play.call(this);
    };
  });
  await start(page);
  const tap = page.locator('.masko-tap');
  await expect(tap).toBeVisible();
  await page.waitForTimeout(3500);
  let state = await page.evaluate(() => {
    const w = window as unknown as Win;
    return { samples: w.watcher.samples.slice(-5), events: w.events };
  });
  expect(state.samples.every((s) => s.visible.length === 1 && s.opaque > 30 && s.time[0] === 0)).toBe(true);
  expect(state.events.filter((e) => e.event === 'error'), 'play time held: no safety net').toEqual([]);
  await tap.click();
  await expect(tap).toBeHidden();
  await waitClip(page, 'idle-loop', 2);
  state = await page.evaluate(() => ({ samples: [], events: (window as unknown as Win).events }));
  expect(state.events.filter((e) => e.event === 'error')).toEqual([]);
});

test('autoplay blocked per video, as in Safari: a tap anywhere starts it, and the next clip needs no second tap', async ({ page }) => {
  // Safari in Low Power Mode: a video plays only once a play() of that same
  // element ran inside a real gesture.
  await page.addInitScript(() => {
    let gesture = false;
    for (const type of ['pointerup', 'click', 'keydown'])
      addEventListener(type, (e) => {
        if (!e.isTrusted) return;
        gesture = true;
        setTimeout(() => (gesture = false), 0);
      }, true);
    const unlocked = new WeakSet<HTMLMediaElement>();
    const play = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = function (this: HTMLMediaElement) {
      if (gesture) unlocked.add(this);
      if (!unlocked.has(this)) return Promise.reject(new DOMException('autoplay blocked', 'NotAllowedError'));
      return play.call(this);
    };
  });
  await start(page);
  const tap = page.locator('.masko-tap');
  await expect(tap).toBeVisible();
  // Not on the overlay: somewhere else on the page.
  await page.mouse.click(500, 400);
  await expect(tap).toBeHidden();
  await waitClip(page, 'idle-loop', 2);
  await page.evaluate(() =>
    (window as unknown as { handle: { fire(n: string, o: { instant: boolean }): void } }).handle.fire(
      'action::click',
      { instant: true }
    )
  );
  await waitClip(page, 'poke');
  await expect(tap).toBeHidden();
  const { events } = await result(page);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
});

test('pause stops the clip and the clock; play resumes it', async ({ page }) => {
  await start(page);
  await waitClip(page, 'idle-loop');
  await page.waitForTimeout(400);
  await page.evaluate(() => (window as unknown as Win).handle.pause());
  const t0 = await page.evaluate(() => (window as unknown as Win).handle.position().time);
  await page.waitForTimeout(2500);
  const paused = await page.evaluate(() => ({
    time: (window as unknown as Win).handle.position().time,
    clips: (window as unknown as Win).events.filter((e) => e.event === 'clip').length
  }));
  expect(paused.time).toBe(t0);
  expect(paused.clips).toBe(1);
  await page.evaluate(() => (window as unknown as Win).handle.play());
  await waitClip(page, 'idle-loop', 2);
  const { events } = await result(page);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
});

test('without requestVideoFrameCallback (older Firefox): the readyState fallback still swaps on a decoded frame', async ({ page }) => {
  await page.addInitScript(() => {
    const proto = HTMLVideoElement.prototype as unknown as Record<string, unknown>;
    proto.hiddenFrameCallback = proto.requestVideoFrameCallback;
    delete proto.requestVideoFrameCallback;
  });
  await start(page);
  await waitClip(page, 'idle-loop');
  expect(await page.evaluate(() => 'requestVideoFrameCallback' in HTMLVideoElement.prototype)).toBe(false);
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop', 2);
  const { samples, events } = await result(page);
  const after = expectAlwaysAMascot(samples);
  expect(collapse(after.map((s) => s.color))).toEqual(['idle-loop', 'to-work', 'work-loop']);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
});

test('frame callbacks that never come for a hidden video: the play-position backstop swaps, still on a decoded frame', async ({
  page
}) => {
  await page.addInitScript(() => {
    const proto = HTMLVideoElement.prototype as unknown as Record<string, unknown>;
    proto.hiddenFrameCallback = proto.requestVideoFrameCallback;
    // A browser whose callback never fires for the player.
    proto.requestVideoFrameCallback = () => 1;
    proto.cancelVideoFrameCallback = () => undefined;
  });
  await start(page);
  await waitClip(page, 'idle-loop');
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop', 2);
  const { samples, events } = await result(page);
  const after = expectAlwaysAMascot(samples);
  expect(collapse(after.map((s) => s.color))).toEqual(['idle-loop', 'to-work', 'work-loop']);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
});

test('a pose without a clip that plays shows its still; a move then swaps in over it', async ({ page }) => {
  await start(page, { dead: ['idle-loop'] });
  await page.waitForFunction(() => (window as unknown as Win).events.some((e) => e.event === 'clip' && e.clip === 'still:idle'));
  await page.waitForTimeout(300);
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop');
  const { samples, events } = await result(page);
  const after = expectAlwaysAMascot(samples);
  expect(after[0].still).toBe(true);
  expect(collapse(after.map((s) => (s.still ? 'still' : s.color)))).toEqual(['still', 'to-work', 'work-loop']);
  expect(events.filter((e) => e.event === 'error').map((e) => e.code)).toContain('clip_failed');
});

test('destroy removes the mascot and stops its events', async ({ page }) => {
  await start(page);
  await waitClip(page, 'idle-loop');
  const left = await page.evaluate(async () => {
    const w = window as unknown as Win & { handle: { destroy(): void } };
    const n = w.events.length;
    w.handle.destroy();
    w.handle.set('working', true);
    await new Promise((r) => setTimeout(r, 2500));
    return { players: document.querySelectorAll('.masko-player').length, videos: document.querySelectorAll('video').length, newEvents: w.events.length - n };
  });
  expect(left).toEqual({ players: 0, videos: 0, newEvents: 0 });
});

const realDir = join(import.meta.dirname, '.cache');
const REAL = ['idle-settle', 'laptop-working', 'panic-tremble', 'dumpling-groove'];

test('real Masko clips: Dumple plays, moves and loops with a mascot on every frame', async ({ page }) => {
  test.skip(!REAL.every((id) => existsSync(join(realDir, `${id}.webm`))), 'real clips not downloaded');
  await open(page, '/pages/esm.html');
  const origin = new URL(page.url()).origin;
  const clip = (id: string) => ({ webm: `${origin}/real/${id}.webm` });
  const edge = (id: string, file: string, source: string, target: string, conditions?: unknown) => ({
    id,
    source,
    target,
    isLoop: source === target,
    duration: 4.042,
    videos: clip(file),
    ...(conditions ? { conditions } : {})
  });
  const delivery = {
    deliverySchemaVersion: 1,
    playerContract: 1,
    requires: [],
    target: 'web',
    mode: 'selected',
    runtimeGraph: {
      runtimeGraphSchemaVersion: 1,
      version: '2.1',
      name: 'Dumple',
      initialNode: 'idle',
      autoPlay: true,
      nodes: [
        { id: 'idle', name: 'Idle' },
        { id: 'working', name: 'Working' }
      ],
      inputs: [{ name: 'behavior::working', type: 'boolean', default: false }],
      edges: [
        edge('idle-loop', 'idle-settle', 'idle', 'idle'),
        edge('work-loop', 'laptop-working', 'working', 'working'),
        edge('to-work', 'panic-tremble', 'idle', 'working', [{ input: 'behavior::working' }]),
        edge('to-idle', 'dumpling-groove', 'working', 'idle', [{ input: 'behavior::working', value: false }])
      ]
    }
  };
  await page.evaluate((d) => (window as unknown as Win).start({ delivery: d }), delivery);
  await waitClip(page, 'idle-loop');
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop', 1, 20_000);
  await page.waitForTimeout(1000);
  const { samples, events } = await result(page);
  // Real clips fill less of the frame: a few opaque pixels of 576 mean a mascot.
  const after = expectAlwaysAMascot(samples, 8);
  // Idle loop, the move in the other slot, the working loop back in the first.
  expect(collapse(after.map((s) => s.visible[0]))).toHaveLength(3);
  expect(collapse(events.filter((e) => e.event === 'clip').map((e) => e.clip))).toEqual(['idle-loop', 'to-work', 'work-loop']);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
  expect(events.filter((e) => e.event === 'pose').map((e) => e.pose)).toEqual(['idle', 'working']);
});

test('a dev server that moved the package away from its .wasm (Vite pre-bundling): the inlined engine plays', async ({ page }) => {
  const wasm: string[] = [];
  page.on('response', (r) => {
    const path = new URL(r.url()).pathname;
    if (path === '/moved/masko_player_wasm_bg.wasm') wasm.push(`wasm ${r.headers()['content-type']}`);
    if (path.startsWith('/moved/inline-')) wasm.push(`inlined ${r.headers()['content-type']}`);
  });
  await open(page, '/pages/moved.html');
  await page.evaluate(() => (window as unknown as Win).start());
  await waitClip(page, 'idle-loop');
  const { samples, events } = await result(page);
  expectAlwaysAMascot(samples);
  expect(events.filter((e) => e.event === 'error')).toEqual([]);
  expect(wasm).toEqual(['wasm text/html; charset=utf-8', 'inlined text/javascript; charset=utf-8']);
});

test('setWasmSource pointing at a web page: engine_unavailable says so', async ({ page }) => {
  await open(page, '/pages/moved.html?wasm=/pages/esm.html');
  await page.evaluate(() => (window as unknown as Win).start());
  await page.waitForFunction(() => (window as unknown as Win).events.some((e) => e.event === 'error'));
  const errors = await page.evaluate(() => (window as unknown as Win).events.filter((e) => e.event === 'error'));
  expect(errors).toHaveLength(1);
  expect(errors[0].code).toBe('engine_unavailable');
  expect(errors[0].detail).toContain('/pages/esm.html is a web page');
  expect(errors[0].detail).toContain('setWasmSource');
});

test('mounted in a hidden element: measured once it shows, not a square for good', async ({ page }) => {
  await open(page, '/pages/esm.html');
  const heights = await page.evaluate(async () => {
    const { mountMascot } = await import('/dist/index.js' as string);
    const { testDelivery } = await import('/pages/harness.js' as string);
    const box = (style: string) => {
      const el = document.createElement('div');
      el.style.cssText = `display:none;${style}`;
      document.body.appendChild(el);
      mountMascot(el, { delivery: testDelivery() });
      return el;
    };
    const sized = box('width:300px;height:120px');
    const auto = box('width:200px');
    await new Promise((r) => requestAnimationFrame(r));
    sized.style.display = 'block';
    auto.style.display = 'block';
    await new Promise((r) => setTimeout(r, 100));
    const h = (el: HTMLElement) => Math.round(el.querySelector('.masko-player')!.getBoundingClientRect().height);
    return { sized: h(sized), auto: h(auto) };
  });
  expect(heights).toEqual({ sized: 120, auto: 200 });
});

test('page CSS cannot turn the swap into a crossfade', async ({ page }) => {
  await open(page, '/pages/esm.html');
  await page.addStyleTag({ content: '#m video, #m img { transition: opacity 1s linear; }' });
  await page.evaluate(() => (window as unknown as Win).start());
  await waitClip(page, 'idle-loop');
  const durations = await page.evaluate(() =>
    [...document.querySelectorAll('#m video, #m img')].map((el) => getComputedStyle(el).transitionDuration)
  );
  expect(durations).toEqual(['0s', '0s', '0s']);
});

test('sound blocked until a gesture: one-shot sounds never play late, a looping one starts on the first click', async ({ page }) => {
  // The browser's policy: muted video plays, sound waits for the person's first real tap.
  await page.addInitScript(() => {
    const w = window as unknown as { audioPlays: { src: string; ok: boolean; t: number }[]; audios: HTMLMediaElement[] };
    w.audioPlays = [];
    w.audios = [];
    let tapped = false;
    addEventListener('pointerdown', (e) => (tapped ||= e.isTrusted), true);
    const play = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = function (this: HTMLMediaElement) {
      if (this instanceof HTMLAudioElement) {
        if (!w.audios.includes(this)) w.audios.push(this);
        w.audioPlays.push({ src: new URL(this.src).search, ok: tapped, t: Math.round(performance.now()) });
      }
      if (!this.muted && !tapped) return Promise.reject(new DOMException('sound blocked', 'NotAllowedError'));
      return play.call(this);
    };
  });
  type Audio = Window & { audioPlays: { src: string; ok: boolean; t: number }[]; audios: HTMLMediaElement[] };
  await start(page, { hum: true, tick: true });
  await waitClip(page, 'idle-loop');
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  // Three working loops, each with a tick the browser blocks.
  await waitClip(page, 'work-loop', 3, 15_000);
  // A tap outside the mascot lets the page play sound; then pause and play.
  await page.mouse.click(600, 400);
  await page.evaluate(() => {
    const w = window as unknown as Win;
    w.handle.pause();
    w.handle.play();
  });
  await page.waitForTimeout(300);
  const ticks = await page.evaluate(() => (window as unknown as Audio).audioPlays.filter((p) => p.src === '?tick'));
  expect(ticks.filter((p) => !p.ok).length, 'ticks blocked').toBeGreaterThanOrEqual(2);
  expect(ticks.filter((p) => p.ok).length, 'none of them played late').toBeLessThanOrEqual(1);
  // A looping sound blocked at load starts on the first click.
  await start(page, { hum: true });
  await waitClip(page, 'idle-loop');
  await page.waitForTimeout(300);
  const before = await page.evaluate(() => (window as unknown as Audio).audios.map((a) => a.paused));
  expect(before).toEqual([true]);
  await page.mouse.click(600, 400);
  await page.waitForTimeout(300);
  const after = await page.evaluate(() => (window as unknown as Audio).audios.map((a) => a.paused));
  expect(after).toEqual([false]);
});

test('the handle swaps fresh links and refuses another release; setSound is there', async ({ page }) => {
  const requests: string[] = [];
  page.on('request', (r) => requests.push(new URL(r.url()).pathname + new URL(r.url()).search));
  await start(page);
  await waitClip(page, 'idle-loop');
  const taken = await page.evaluate(async () => {
    const w = window as unknown as Win & { handle: { updateDelivery(d: unknown): boolean; setSound(on: boolean): void } };
    const { testDelivery } = await import('/pages/harness.js' as string);
    w.handle.setSound(true);
    return [w.handle.updateDelivery(testDelivery({ version: '?v=2' })), w.handle.updateDelivery(testDelivery({ name: 'Another bot' }))];
  });
  expect(taken).toEqual([true, false]);
  await page.evaluate(() => (window as unknown as Win).handle.set('working', true));
  await waitClip(page, 'work-loop');
  const { events } = await result(page);
  expect(events.filter((e) => e.event === 'error').map((e) => e.code)).toEqual(['refresh_mismatch']);
  expect(requests).toContain('/clips/work-loop.webm?v=2');
});

test('React: listeners added with ref.on hear the player of a new release', async ({ page }) => {
  await open(page, '/pages/react.html');
  await waitClip(page, 'idle-loop');
  await page.evaluate(() => {
    const w = window as unknown as {
      mascot: { on(e: string, f: (e: { pose: string }) => void): () => void };
      refPoses: string[];
      setDelivery(d: unknown): void;
      testDelivery(o: unknown): unknown;
    };
    w.refPoses = [];
    w.mascot.on('pose', (e) => w.refPoses.push(e.pose));
    // Another release: a new player.
    w.setDelivery(w.testDelivery({ name: 'Another bot' }));
  });
  await page.waitForFunction(() => (window as unknown as { refPoses: string[] }).refPoses.length > 0);
  expect(await page.evaluate(() => (window as unknown as { refPoses: string[] }).refPoses)).toEqual(['idle']);
  expect(await page.locator('.masko-player').count()).toBe(1);
});
