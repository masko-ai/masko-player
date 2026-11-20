// Every scenario of spec/fixtures through the WebAssembly binding, with the
// ideal player of spec/fixtures/README.md. A port of
// engine/tests/scenarios.rs: same delivery patches, same clock, same checks.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { Engine, LoadError, loadWasmSync, type Command, type Value } from '../src/engine.ts';

const root = join(import.meta.dirname, '..', '..');
const fixtures = join(root, 'spec', 'fixtures');
loadWasmSync(readFileSync(join(import.meta.dirname, '..', 'src', 'wasm', 'masko_player_wasm_bg.wasm')));

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type Obj = { [key: string]: Json };

const isObj = (v: Json | undefined): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);

function merge(base: Json, patch: Json): void {
  if (!isObj(base) || !isObj(patch)) return;
  for (const [k, v] of Object.entries(patch)) {
    if (v === null) delete base[k];
    else if (isObj(v) && isObj(base[k])) merge(base[k], v);
    else base[k] = structuredClone(v);
  }
}

function buildDelivery(fx: Obj): Obj {
  const delivery: Obj =
    typeof fx.canvas === 'string'
      ? JSON.parse(readFileSync(join(fixtures, 'canvases', `${fx.canvas}.json`), 'utf8'))
      : structuredClone(fx.canvas as Obj);
  const patch = (fx.patch ?? {}) as Obj;
  if (patch.delivery) merge(delivery, patch.delivery);
  const g = delivery.runtimeGraph as Obj;
  const edges = g.edges as Obj[];
  if (isObj(patch.edges)) {
    for (const [id, change] of Object.entries(patch.edges)) {
      const idx = edges.findIndex((e) => e.id === id);
      assert.ok(idx >= 0, `patched edge ${id}`);
      if (change === null) edges.splice(idx, 1);
      else merge(edges[idx], change);
    }
  }
  if (Array.isArray(patch.addEdges)) edges.push(...(structuredClone(patch.addEdges) as Obj[]));
  if (!('inputs' in g)) g.inputs = [];
  const inputs = g.inputs as Obj[];
  if (Array.isArray(patch.addInputs)) inputs.push(...(structuredClone(patch.addInputs) as Obj[]));
  if (isObj(patch.defaults)) {
    for (const [name, d] of Object.entries(patch.defaults)) {
      const input = inputs.find((i) => i.name === name);
      assert.ok(input, `input ${name}`);
      input.default = d;
    }
  }
  return delivery;
}

/** Whole milliseconds, halves to even, as every player computes them. */
function roundMs(x: number): number {
  const f = Math.floor(x);
  const d = x - f;
  const r = d > 0.5 ? f + 1 : d < 0.5 ? f : f % 2 === 0 ? f : f + 1;
  return Math.max(0, r);
}

/** The ideal player around the engine. */
class IdealPlayer {
  engine: Engine;
  files: Map<string, number>;
  durations: Map<string, number>;
  noEnd = new Set<string>();
  /** How long some edges' first frames take to show, in milliseconds. */
  load: Map<string, number>;
  /** The clip on screen: edge, shown at, rate. */
  clip: { edge: string; shown: number; rate: number } | null = null;
  /** A clip asked for and not yet reported shown: edge, when it shows. */
  showing: { edge: string; at: number } | null = null;
  events: string[] = [];
  preloaded: string[] = [];

  constructor(engine: Engine, files: Map<string, number>, durations: Map<string, number>, load: Map<string, number>) {
    this.engine = engine;
    this.files = files;
    this.durations = durations;
    this.load = load;
  }

  clock(): number {
    return this.engine.state().clock;
  }

  absorb(commands: Command[]): void {
    const now = this.clock();
    for (const c of commands) {
      switch (c.type) {
        case 'showClip': {
          this.events.push(`shown:${c.edge}`);
          const at = now + (this.load.get(c.edge) ?? 0);
          this.clip = { edge: c.edge, shown: at, rate: c.rate };
          this.showing = { edge: c.edge, at };
          break;
        }
        case 'showStill':
          this.events.push(`shown:still:${c.pose}`);
          this.clip = null;
          this.showing = null;
          break;
        case 'preload':
          this.preloaded.push(c.edge);
          break;
        case 'poseEntered':
          this.events.push(`pose:${c.pose}`);
          break;
        case 'interrupted':
          this.events.push(`interrupted:${c.edge}`);
          break;
        case 'spent':
          this.events.push(c.used ? `spent:${c.name}` : `spent:${c.name}:unused`);
          break;
        case 'error':
          this.events.push(`error:${c.code}`);
          break;
        case 'playSound':
        case 'stopSounds':
          break;
      }
    }
    // A clip that shows at once is reported shown at once.
    const showing = this.showing;
    if (showing && showing.at === now) {
      this.showing = null;
      this.absorb(this.engine.clipShown(showing.edge, now));
    }
  }

  /** When the player reports the end of its clip, unless a step took the end signal away. */
  clipEnd(): { edge: string; at: number } | null {
    if (!this.clip || this.noEnd.has(this.clip.edge)) return null;
    const { edge, shown, rate } = this.clip;
    const len = this.files.get(edge) ?? this.durations.get(edge)!;
    return { edge, at: shown + roundMs((len / rate) * 1000) };
  }

  advanceTo(until: number): void {
    for (let i = 0; i < 100_000; i++) {
      const showing = this.showing;
      if (showing && showing.at <= until) {
        const w = this.engine.nextWake();
        if (w === undefined || showing.at <= w) {
          this.showing = null;
          this.absorb(this.engine.clipShown(showing.edge, showing.at));
          continue;
        }
      }
      const end = this.clipEnd();
      const wake = this.engine.nextWake();
      const next = end ? (wake === undefined ? end.at : Math.min(end.at, wake)) : wake;
      if (next !== undefined && next <= until) {
        let commands: Command[];
        if (end && end.at <= (wake ?? Number.MAX_SAFE_INTEGER)) {
          assert.equal(
            this.engine.state().clip,
            end.edge,
            'the player and the engine disagree on the clip on screen'
          );
          commands = this.engine.clipEnded(end.edge, end.at);
        } else {
          commands = this.engine.advance(next);
        }
        this.absorb(commands);
      } else {
        this.absorb(this.engine.advance(until));
        return;
      }
    }
    throw new Error('the clock did not settle');
  }

  runToEnd(moving: boolean): void {
    const state = this.engine.state();
    assert.equal(state.moving, moving, 'loopEnded or clipEnded on the wrong kind of clip');
    const safety = state.shownAt + state.expectedMs + 1000;
    let until: number;
    if (state.clip?.startsWith('still:')) {
      until = state.shownAt + state.expectedMs;
    } else {
      const end = this.clipEnd();
      until = end ? Math.min(end.at, safety) : safety;
    }
    this.advanceTo(until);
  }
}

function act(p: IdealPlayer, step: Obj): void {
  const now = p.clock();
  const e = p.engine;
  let commands: Command[] = [];
  // `"instant": true` on a set or fire step (rules.md 5.8).
  const instant = step.instant === true;
  if ('start' in step) commands = e.start(now);
  else if ('set' in step) {
    const [name, value] = step.set as [string, Value];
    commands = e.set(name, value, instant, now);
  } else if ('fire' in step) commands = e.fire(step.fire as string, instant, now);
  else if ('advance' in step) p.advanceTo(now + (step.advance as number));
  else if ('loopEnded' in step) p.runToEnd(false);
  else if ('clipEnded' in step) p.runToEnd(true);
  else if ('failClip' in step) e.markUnplayable(step.failClip as string);
  else if ('loadsAgain' in step) e.markPlayable(step.loadsAgain as string);
  else if ('noEndSignal' in step) p.noEnd.add(step.noEndSignal as string);
  else if ('reset' in step) commands = e.reset(now);
  else if ('showPose' in step) commands = e.previewShowPose(step.showPose as string, now);
  else if ('playMove' in step) commands = e.previewPlayMove(step.playMove as string, now);
  else throw new Error(`unknown step ${JSON.stringify(step)}`);
  p.absorb(commands);
}

function observed(p: IdealPlayer, want: Obj): Obj {
  const s = p.engine.state();
  const out: Obj = {
    clock: s.clock,
    pose: s.pose,
    clip: s.clip,
    next: s.next ? { clip: s.next.clip, at: s.next.at } : null,
    events: [...p.events]
  };
  if (isObj(want.values)) {
    out.values = Object.fromEntries(Object.keys(want.values).map((n) => [n, p.engine.value(n)]));
  }
  if ('fired' in want) out.fired = p.engine.pendingFires();
  if ('preloaded' in want) out.preloaded = [...p.preloaded];
  return out;
}

/** Equality as the scenarios mean it: a boolean equals the number 1 or 0. */
function same(a: Json, b: Json): boolean {
  const num = (v: Json): number | null =>
    typeof v === 'boolean' ? Number(v) : typeof v === 'number' ? v : null;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((x, i) => same(x, b[i]));
  }
  if (isObj(a) && isObj(b)) {
    const ka = Object.keys(a);
    return ka.length === Object.keys(b).length && ka.every((k) => k in b && same(a[k], b[k]));
  }
  const [x, y] = [num(a), num(b)];
  if (x !== null && y !== null) return x === y;
  return a === b;
}

function run(path: string): string[] {
  const fx = JSON.parse(readFileSync(path, 'utf8')) as Obj;
  const delivery = buildDelivery(fx);
  const wantRefusal = isObj(fx.expectLoad) ? (fx.expectLoad.refused as string | undefined) : undefined;
  let engine: Engine;
  try {
    engine = Engine.load(JSON.stringify(delivery));
  } catch (error) {
    assert.ok(error instanceof LoadError, `not a load error: ${error}`);
    return wantRefusal === error.code
      ? []
      : [`load refused with ${error.code}, expected ${wantRefusal ?? 'None'}`];
  }
  if (wantRefusal) {
    engine.free();
    return [`loaded, expected refusal ${wantRefusal}`];
  }
  engine.useRolls((fx.random as number[] | undefined) ?? []);
  const files = new Map(Object.entries((fx.files ?? {}) as Record<string, number>));
  const durations = new Map(
    ((delivery.runtimeGraph as Obj).edges as Obj[]).map((e) => [e.id as string, e.duration as number])
  );
  const load = new Map(Object.entries((fx.loadMs ?? {}) as Record<string, number>));
  const player = new IdealPlayer(engine, files, durations, load);
  const failures: string[] = [];
  (fx.steps as Obj[]).forEach((step, i) => {
    player.events = [];
    player.preloaded = [];
    act(player, step);
    const want = (step.expect ?? {}) as Obj;
    const got = observed(player, want);
    for (const [k, v] of Object.entries(want)) {
      const g = got[k] ?? null;
      if (!same(g, v)) {
        failures.push(`step ${i + 1} ${k}: got ${JSON.stringify(g)}, expected ${JSON.stringify(v)}`);
      }
    }
  });
  engine.free();
  return failures;
}

test('every scenario agrees with the rulebook, through the WebAssembly binding', () => {
  const names = readdirSync(fixtures)
    .filter((n) => n.endsWith('.json'))
    .sort();
  let bad = 0;
  const lines: string[] = [];
  for (const name of names) {
    let failures: string[];
    try {
      failures = run(join(fixtures, name));
    } catch (error) {
      failures = [`threw: ${error instanceof Error ? error.message : String(error)}`];
    }
    if (failures.length === 0) lines.push(`ok   ${name}`);
    else {
      bad++;
      lines.push(`FAIL ${name}`, ...failures.map((f) => `     ${f}`));
    }
  }
  lines.push(`${names.length - bad} of ${names.length} scenarios pass`);
  console.log(lines.join('\n'));
  assert.equal(bad, 0, `${bad} scenarios failed`);
});
