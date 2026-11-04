# Masko player rulebook

Player contract 1. Every Masko player follows these rules: the Rust engine,
and the web, Apple and Android packages around it. The scenarios in
`fixtures/` prove them; a change to a rule changes this file, the scenarios
and the engine in one pull request.

## 0. Words

- **Pose**: a node of the graph.
- **Clip**: the video of an edge.
- **Pose clip**: an edge whose source and target are the same pose
  (`isLoop: true`). It plays on the pose and ends on the pose's frame.
- **Move**: an edge from one pose to another (`isLoop: false`). When its clip
  ends, the mascot is on the target pose.
- **Any State edge**: an edge with `source: "*"`. It has no clip. It means
  "go to this pose, from wherever the mascot is, while these conditions
  hold".
- **Candidate**: a move from the current pose, or an Any State edge.
- **Instant**: a value the app set or fired with instant. What it starts
  happens now; when an Any State edge wins, the mascot goes straight into
  its target pose, skipping every transition clip and route (5.8). Nothing
  else cuts a loop.
- **Person inputs**: `behavior::pointerHover`, `action::click`,
  `behavior::heldContact`, `behavior::heldShake`, `behavior::falling`. They
  are the canvas editor's "You" triggers, and the app gives them (3.5).
- **Canvas order**: an edge's position in `runtimeGraph.edges`.
- **Clip time**: seconds into a clip's file, whatever its speed. At speed 2 a
  clip reaches clip time 2.0 after 1 s.
- **Play time**: the engine clock, in whole milliseconds. It stops while the
  player is paused (11.2). "Now" is the current play time.
- **Boundary**: the moment a pose is entered or a pose clip ends. The frame on
  screen at a boundary is the pose's own frame.

## 1. The release

### 1.1 What a player reads

A player reads one object: the release delivery, as
`GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery` returns it in
`delivery`. The rules use these fields and no others:

- `deliverySchemaVersion`, `playerContract`, `requires`.
- `runtimeGraph.runtimeGraphSchemaVersion`, `initialNode`, `autoPlay`.
- `runtimeGraph.nodes[]`: `id`, `transparentThumbnailUrl`.
- `runtimeGraph.edges[]`: `id`, `source`, `target`, `isLoop`, `duration`
  (seconds), `speed`, `speedInput`, `priority`, `reverse`, `conditions`,
  `conditionGroups`, `videos`, `sound`, `hasAudio`.
- `runtimeGraph.inputs[]`: `name`, `type`, `default`.

Every other field (`release`, `deliveryAccess`, `target`, `mode`, `variant`,
`physics`, `presentation`, `anchors`, `defaultAnchors`, `nodeAnchors`,
`capabilities`, `motionProfiles`, `clickEffect`, names, an edge's `movement`
and `anchors`) is host data: the player hands it to the app unchanged and no
rule reads it. A player never reads the legacy `MaskoAnimationConfig` export
(`GET /v1/canvases/{canvasId}/export`).

### 1.2 Versions

A player refuses a delivery when:

1. `deliverySchemaVersion` is not 1: `unsupported_delivery`.
2. `playerContract` is not a version it implements: `unsupported_contract`.
   This rulebook is contract 1. A delivery without `playerContract` (made
   before the field existed) reads as contract 1 with `requires: []`.
3. `requires` lists a feature it does not implement: `missing_feature`.
   Contract 1 has two: `any-state` and `condition-groups`.
4. `runtimeGraph.runtimeGraphSchemaVersion` is not 1: `unsupported_graph`.
5. The graph breaks a rule of 1.3.

The player checks 1 to 4, in that order, before it reads anything else, so a
newer delivery is refused for its version whatever else changed in it. Text
that is not JSON, or a delivery of these versions that cannot be read, is
refused with `unreadable`.

Refusing means playing nothing from the delivery and reporting an `error`
event with the code. A player never plays part of a release. It ignores
fields it does not know, anywhere, with the one exception in 1.3.

### 1.3 A valid graph

A player refuses with `invalid_graph` unless:

- node ids are unique, and edge ids are unique;
- `initialNode` is a node id;
- every edge's `target` is a node, and its `source` is a node or `"*"`;
- an edge whose source is a node has `isLoop: true` exactly when its source
  and target are the same;
- an Any State edge has `isLoop: false`;
- `duration` is a finite number of at least 0, `speed` (when present) a
  finite number above 0, and `priority` (when present) a finite number,
  fractions allowed;
- every condition `value`, when present, is a boolean or a number (`null`
  and a string are neither);
- `speedInput` (when present) names an input the canvas declares with type
  `number` (9.2).

It refuses with `unknown_operator` a condition whose `op` is not one of
`==`, `!=`, `>`, `<`, `>=`, `<=` (a number or `null` is not one of them;
only an `op` that is not there reads `==`, 2.2), and with
`unknown_condition_key` a condition that has an `operator` field. Some
canvases, the Ko coffee graph among them, wrote `operator` instead of `op`;
a player that reads only `op` treats `operator: ">"` as `==`. The release
compiler rewrites `operator` to `op`; a player that meets it refuses instead
of guessing.

### 1.4 Clips that can play

An edge has a clip when its `videos` names at least one file: `webm`,
`hevc`, `stacked`, `av1`, or one of those in a size of `variants`. A `variants` map
whose sizes name no file, an empty one included, is no clip. The player
picks the file its platform plays; that choice is not a rule of the engine.
A move without a clip plays as a failed clip (9.4). A pose clip without one
is never chosen (7.1).

## 2. Conditions

### 2.1 Groups

- When `conditionGroups` is a non-empty list, it replaces `conditions`. The
  edge holds when at least one group holds, and a group holds when all its
  conditions hold. Empty groups are dropped; when every group is empty, the
  edge has no conditions.
- Otherwise the edge holds when all of `conditions` hold.
- An edge with no conditions at all holds always. A move like that plays at
  the end of the pose's clip (5.4); an Any State edge like that never holds
  (4.1).

### 2.2 One condition

A condition is `input op value`, with `op` `==` and `value` `true` when
missing. Both sides are compared as numbers: `true` is 1 and `false` is 0.

### 2.3 Names without a value

A name that was never set, never fired and has no default reads `false`
(0). So `behavior::heldShake != true` holds while nothing has set
`heldShake`.

### 2.4 Defaults

When the player starts, and on `reset`, every input declared in
`runtimeGraph.inputs` takes its `default`, except:

- an input of type `trigger`: it is never held (3.2), so its default is
  ignored;
- `node::loopCount` and `node::nodeTime`: the player owns them (8);
- `behavior::rest`: the player computes it (3.4).

## 3. Values the app gives: `set` and `fire`

### 3.1 Names

A name that contains `::` is used as given. A bare name resolves to the first
of:

1. the same name, when the canvas declares it in `inputs` (a canvas number
   such as `score`);
2. `behavior::<name>`, when `<name>` is a behavior family or the canvas
   declares `behavior::<name>`. The families are `rest`, `attention`,
   `working`, `focus`, `celebrate`, `error`, `waiting`, `pointerHover`,
   `heldContact`, `heldShake`, `falling`, `ambient`, `rhythm`;
3. `action::<name>`.

So `set('working', true)` sets `behavior::working`, `fire('celebrate')` fires
`behavior::celebrate`, and `fire('boop')` fires `action::boop`.

### 3.2 `set`

`set(name, value)` holds a boolean or a number until the next `set` of that
name. Setting the value a name already holds changes nothing but whether it
is instant (5.8). Several behaviors can be on at once; which one shows is
decided by priority (4), not by the order of the calls. The player never
clears a held value by itself.

- `set` with a number that is not finite (NaN or an infinity) changes
  nothing and reports `error` `bad_value`.
- `set` on `node::loopCount`, `node::nodeTime` or `behavior::rest` changes
  nothing and reports `error` `read_only`.
- `set` on an input the canvas declares as `trigger`: `true` fires it (3.3),
  `false` drops the fire if it is still pending, claimed or not, without a
  `reactionDone`.

### 3.3 `fire`

`fire(name)` makes the name read `true` while the fire is pending, whatever
its held value. Firing a name that is already pending changes nothing but
whether it is instant (5.8); fires are not counted. Like every value, what a
fire starts waits for the end of the pose clip (5.3) unless it is instant.

A fire is used or dropped, never kept for later:

1. **Wanted.** Every time the engine decides (5.1), outside a move, a fire
   stays pending while the decided start reads it in one of its holding
   groups, or a named pose clip of the current pose (7.1) reads it in a
   group that holds. Every other fire that no start has claimed is dropped
   at once. A fire made during a move waits for the move's end, where the
   engine decides again (10.3).
2. **Claimed.** When a start happens, it claims the pending fires its winner
   reads in a holding group, unless an earlier start already claimed them,
   and drops the unclaimed fires it does not claim; a named pose clip claims
   the fires it reads when the pose chooses it (7.2). A claimed fire stays
   pending until the first pose clip of the pose the start leads to ends:
   the move's target, the Any State edge's target at the end of its route
   (6.4) or at once for an instant one (5.8), or, for a named pose clip, that
   clip itself, or what plays in its place when it fails (9.5). If the
   mascot leaves that pose before its first pose clip ends, the fire is
   spent when it leaves.
3. **Overtaken.** A fire whose start is cut by an instant one (10.1), or
   whose route ends before reaching its target (6.4), is dropped there. It
   never wins later, whatever its priority.

When a fire is spent, the player reports `reactionDone` with the name and
whether it was used: claimed by a start or a named pose clip. A dropped fire
reports `used: false`.

The claim lasts through the target's first pose clip because a move that
holds when its pose is entered starts at once (5.3): a reaction pose whose
way back is "the action is off" would otherwise leave as soon as it is
entered.

### 3.4 `rest`

`behavior::rest` reads `true` exactly when no other `behavior::` name reads
`true`, held or fired. Its declared default is ignored and `set` cannot
change it.

### 3.5 The person

The player does not watch the person. The app tells it what the person does
to the mascot, with the person inputs' full names (0):

- the pointer over the mascot: `set('behavior::pointerHover', true)`, and
  `false` when it leaves;
- a click or a tap: `fire('action::click')`;
- a grab, a shake or a fall: `behavior::heldContact`, `behavior::heldShake`
  and `behavior::falling`, set on and off.

The app decides what counts as being on the mascot, since it owns the page or
the window, dragging and physics.

## 4. What plays next

### 4.1 Candidates

With the mascot on pose P, and not during a move (10), the candidates are:

- every move from P whose conditions hold;
- every Any State edge whose conditions hold. An Any State edge without
  conditions never holds, since it would pull every pose to its target
  forever.

They are ranked by `priority`, highest first (missing is 0, and so is -0),
then by canvas order. The first is the winner.

### 4.2 What the winner means

- **A move**: it starts, at the moment section 5 gives.
- **An Any State edge whose target is P**: the mascot stays. Nothing starts,
  and a route under way ends.
- **An Any State edge whose target is another pose T**: the mascot goes to T
  (6), or straight into it when the start is instant (5.8). When T cannot be
  reached from P (6.3), this candidate is skipped and the next one is the
  winner, unless the start is instant: an instant start reaches any pose.
- **No candidate**: nothing starts.

Pose clips are not candidates: the pose chooses them at its boundaries when
no move starts (7).

## 5. When a start happens

### 5.1 When the engine decides

The engine decides (4) again after every event that can change the answer:
a `set`, a `fire`, a boundary, a node timer crossing (8.3), and `reset`.
Each decision starts from scratch.

### 5.2 The person: no moment of its own

What the person does reaches the engine as values the app gives (3.5), and
its starts follow 5.3 to 5.8 like any other: a pointer over the mascot, a
grab or a let go waits for the end of the pose clip unless the app gives it
with instant. The player never cuts a loop on its own.

### 5.3 The end of the pose clip

A winner with conditions that is not instant (5.8) starts at the end of the
pose clip on screen, so the next clip starts from the pose's own frame. At a boundary that moment is now. A pose shown as
a still (7.4, 9.5) is the pose's own picture, so a winner starts from it at
once, including a start decided for the end of a pose clip that failed on
screen and gave way to the still.

### 5.4 No conditions: the end of the clip

A winner without conditions starts at the end of the pose clip on screen. It
never starts when its pose is entered or from a still, so its pose always
plays one clip first: it is a sequence, not a reaction.

### 5.5 The end of a pose clip

In this order:

1. `node::loopCount` grows by 1.
2. Decide (4) with every pending fire on. A winner starts now.
3. If nothing started: spend the claimed fires whose pose clip this was
   (3.3), and decide again. A winner starts now.
4. If nothing started: choose the pose's next clip (7), which may claim a
   pending fire.
5. Spend every fire that is still unclaimed.
6. If step 5 spent a fire, decide again: a winner starts now, instead of the
   clip from step 4. Otherwise that clip plays.

### 5.6 A decision can change

A start is not final until its moment comes. Every decision before then
starts from scratch: a different winner replaces it, with its own moment,
and no winner cancels it.

### 5.7 No play-once rule

A pose can be left as soon as a candidate wins, at the moment 5.3, 5.4 and
5.8 give. Nothing waits for a pose's first loop except a move without
conditions (5.4). A designer who wants a pose to show first uses "After it
plays once" (`node::loopCount >= 1`).

### 5.8 Instant: now, and straight into an Any State target

The app can make a value instant when it sets or fires it: `set(name, value,
instant)` and `fire(name, instant)`, with instant off unless the app turns it
on (each platform spells it its own way, `player.md`). The start that value
causes then happens now. When an Any State edge wins, the mascot also goes
straight into its target pose, skipping every transition clip and route;
when a move wins, it plays that move's own clip, at once. Without instant,
nothing changes.

- A value set with instant stays instant while it holds that value. A later
  `set` of the name without instant, or setting it off (`false` or 0), ends
  it. Setting the value a name already holds with instant makes it instant.
- Setting a value off with instant (a let go, the pointer leaving) makes the
  start that change decides instant, once: the winner of that decision is
  instant when it reads the name, directly or, for a `behavior::` name,
  through `behavior::rest`, or when it is not the start decided before the
  change. A start already decided that does not read the name keeps its
  moment, and the value does not stay instant.
- A fire with instant stays instant until it is spent (3.3). Firing a
  pending name again with instant makes it instant; firing it again without
  instant leaves it instant.
- `reset` ends every instant value, with the values and fires (11.3).

A start is **instant** when one of the winner's holding groups has a
condition on an instant name that is on. An instant start:

- **happens now**, at any moment: mid pose clip, during a still, and during a
  move, which it interrupts (10.1); it is the only start that does.
- **plays its own clip when the winner is a move.** The move starts at once,
  from whatever frame the pose clip is on, and plays as any move (9).
- **goes straight into the target pose when the winner is an Any State
  edge.** Instead of showing any clip on the way, the mascot enters the
  target: the player reports `poseEntered`, then the target shows the pose
  clip it chooses on entry (7.2), its main loop unless a named clip holds,
  from its first frame, or its still when it has no clip (7.4). No direct
  clip, no route (6), no move clip, no `movement` and no move sound:
  entering stops looping sounds and lets one-shot sounds finish (9.6). It
  reaches any pose, so an Any State edge whose target no route reaches
  (6.3) is not skipped. The entry is like any other: fires claimed by the
  start stay pending through the target's first pose clip (3.3),
  `node::loopCount` and `node::nodeTime` start again from 0 (8), a decided
  start is dropped, and the engine decides again from the target as after
  any arrival (5.1).

An Any State winner whose target is the pose the mascot is on keeps it
there (4.2), with no restart, instant or not. Instant changes nothing else:
the winner is ranked as always (4.1); instant only changes when it starts
and, for an Any State edge, how it reaches its target.

**An instant named clip.** When no start happens now, and the named clip
the pose would choose at its next boundary (7.2) has a holding group with a
condition on an instant name that is on, that named clip shows now: it cuts
the pose clip or still on screen, from whatever frame it is on, like an
instant move, and claims its fires as at a boundary (3.3). The fires the cut
clip had claimed are spent there. A named clip already on screen plays on
and does not start again. During a move nothing changes: the named clip
waits for the move's end, as every fire made during a move does (10.3). A
start the engine has decided keeps its moment, and comes after the named
clip when that moment is the end of the clip on screen.

## 6. Going to a pose

The winner is an Any State edge with target T, the start is not instant
(5.8), and the mascot is on P.

### 6.1 A direct clip

When moves from P to T have a clip, the first hop is the best of them by
`priority` (highest first), then clip length (`duration / speed`, shortest
first, with `speed` alone and never `speedInput`), then `id`. Their own
conditions do not matter.

### 6.2 An exit that leads there

Otherwise the exits are the moves from P that have a clip and whose target
can reach T through moves with clips (conditions ignored, never through Any
State edges, never back through P). The first hop is the best exit among the
exits whose conditions hold, or among all of them when none holds: by
`priority` (highest first), then the fewest moves from the exit's target to
T, then clip length as in 6.1, then `id`. The fewest moves come before clip
length so a route never takes a detour through an unrelated pose because its
first clip is shorter.

### 6.3 No way

With no direct clip and no exit, T cannot be reached from P (4.2).

### 6.4 The route

The route remembers T and the Any State edge that chose it. At every pose
entered on the way, the engine decides again from scratch (5.1). When an Any
State edge to T still wins, the next hop starts at once, since it is a
boundary; when something else wins, the route ends and the new winner plays.
The route ends when T is entered.

## 7. Pose clips

### 7.1 Named, main and extra clips

Among a pose's pose clips that have a clip, in canvas order:

- a **named** clip has conditions. The first named clip whose conditions hold
  is chosen at every boundary, before anything else.
- the **main** loop is the first clip without conditions, else the first
  clip.
- the **extras** are the other clips without conditions.

### 7.2 When a pose chooses

- When the pose is entered and no move starts: the named clip that holds,
  else the main loop. Never an extra.
- Outside a move, when an instant name holds the named clip the pose would
  choose next and no start happens now: that named clip, at once (5.8).
- When a pose clip ends and no move starts (5.5): the named clip that holds;
  else, if the clip that ended was the main loop and the pose has extras, the
  extra roll (7.3); else the main loop.

A pose clip that is chosen again simply plays again.

### 7.3 The extra roll

The engine draws one number r in [0, 1) from the host's random source. When
r < 1/3, it plays `pool[floor(r * 3 * pool.length)]`, where the pool is the
pose's extras without the one this pose played last (all its extras if that
leaves none). Otherwise it plays the main loop. The extra plays right after
the main loop whose end drew the number. The engine remembers the last extra
of each pose for as long as it runs. This is the only rule that uses
randomness; scenarios inject the numbers, and when those run out the engine
draws from its own seeded source.

### 7.4 A pose without a clip

A pose with no pose clip that has a clip shows its still image
(`transparentThumbnailUrl`) as a pose clip of 1 s, so loop counts, fires
and every other rule work unchanged.

### 7.5 Switching clips

Choosing another pose clip keeps `node::loopCount` and `node::nodeTime`; only
entering a pose resets them.

## 8. Node timers

### 8.1 `node::loopCount`

0 when a pose is entered, and 1 more at the end of each pose clip on it
(5.5), main, named, extra or still.

### 8.2 `node::nodeTime`

Milliseconds of play time since the pose was entered. The canvas editor
writes seconds times 1000: "After 30 s" is `>= 30000`.

### 8.3 Waking up

The engine decides again (5.1) at every play time where a condition on
`node::nodeTime` of a candidate or pose clip of the current pose changes
value: for a threshold v, at `ceil(v)` and at `floor(v) + 1` milliseconds
after the pose was entered.

## 9. Moves

### 9.1 A move ends when its clip ends

While a move's clip plays, the mascot is still on the source pose. When the
player reports the clip's end, the target pose is entered and the player
reports `poseEntered`.

### 9.2 Speed and direction

A clip always plays forwards, at its rate. `reverse` only says the edge's
clip was made as the return of another move; its own file already runs in
the right direction, so no player plays it backwards. Its rate is
`speed` (1 when missing). When the edge has `speedInput`, the rate is
`speed` times the number that input holds, kept between 0.2 and 4; when the
input holds no number, the factor is 1. The rate is fixed when the clip
starts: a new value applies the next time that clip starts, which for a pose
clip is its next loop. Nothing stretches a clip to its `duration`.

### 9.3 Showing a clip, and the safety net

A clip is shown when its first frame is on screen, which the player reports
(`clipShown`), not when the engine asks for it: loading and decoding take
time, and the frame before stays on screen meanwhile. A clip that is not on
screen 10 s after the engine asked for it counts as a clip that cannot play
(9.4, 9.5), reported as `error` `clip_timeout`.

When the player has not reported the end of a clip, a move or a pose clip,
`duration` divided by its rate (9.2), plus 1 s, after it was shown, the
engine ends it itself and reports `error` `missing_end`.

### 9.4 A move that fails

A move whose clip has no file, or fails to load or play, ends at once: the
target pose is entered without its clip, and the player reports `error`
`clip_failed`. The mascot never waits on a failed clip.

Moves whose clips all fail can send the mascot from pose to pose without
showing anything, and so can instant Any State starts (5.8), which count as
moves here. When more than 32 poses are entered within one second of play
time, the engine starts no move from the next ones until that second is
over, and reports `error` `clipless_cycle`; the pose shows its clip or still
meanwhile. No event starts a move during the rest of that second: not a
`set`, a `fire`, a node timer, a boundary or `preview.playMove` (13). When
the second is over, the engine decides again (5.1).

### 9.5 A pose clip that fails

When a chosen pose clip fails, before it shows or while it is on screen,
the pose plays again the pose clip that ended last on it since it was
entered, if there is one and it has not failed; otherwise its still (7.4)
for the failed clip's length, and never less than the 1 s of a still of its
own, so a pose whose clips fail cannot flicker through stills. Either way
the failure is reported as `error` `clip_failed` (or `clip_timeout`, 9.3)
and the pose keeps counting loops. A fire the failed clip claimed (3.3) is
spent when the clip or still that plays in its place ends.

### 9.6 Sound

When a clip with a `sound` is shown, the sound plays at its `volume`, looping
when `loop` is true. Entering a pose stops looping sounds and lets one-shot
sounds finish; a move stops every sound before its own. Nothing else stops a
sound, and a looping sound that is playing is never started again: a pose
clip that loops, or comes back after another clip of the same pose, leaves
its looping sound running, so each looping sound plays once at most. A
clip's own voice (`hasAudio`) is heard unless the app turns sound off; sound
is on by default.

### 9.7 Preloading

When a pose is entered, the engine asks the player to preload, in this order
and once each: the first hop (6) of each Any State edge whose target is
another pose it can reach, in rank order (4.1), then the pose's other moves
that have a clip, in rank order, then the pose's named clips that have a
clip, then its extras that have one, both in canvas order (7.1). Every first
hop is one of the pose's moves, so this is every clip that can play next
from this pose, moves first and the most important first, without the main
loop on screen. When a move starts, the engine asks the player to preload
the target pose's main loop (7.1), so the pose can show as soon as the move
ends. No app call preloads; a web page can ask the player to load every
clip of the release as well (`player.md`, `preload`).

## 10. A change during a move

1. **Only an instant start interrupts a move.** During a move, the engine
   decides (4) from the move's source pose, where the mascot still is
   (9.1). When the winner is an instant move (5.8) whose clip is not the
   move in progress, or an instant Any State edge (5.8), even toward that
   move's own target, that move's clip stops and the start begins at once,
   accepting one visible cut: an instant move shows its clip, an instant Any
   State start enters its target. The interrupted move never enters its
   target, and the fires it claimed for that target are dropped (3.3),
   unless the new start leads there too. A winner that only keeps the mascot
   where it is (an Any State edge to the source pose) does not interrupt.
2. Every other start waits for the move in progress. `set` and `fire` change
   values at once, and the engine decides again when the move ends, with
   the latest values (6.4 and 5.1); entering the target is a boundary, so a
   start that holds there starts at once.
3. A fire during a move stays pending until the move ends. There the target
   pose's decision takes it or drops it (3.3).
4. The engine accepts every event in every state, and after every event the
   mascot either plays a clip or waits for one to end. Nothing leaves it
   frozen.

## 11. Start, pause and reset

1. **Start.** The app may `set` and `fire` before the player starts. At play
   time 0 the engine decides among the Any State edges whose conditions hold,
   ranked as in 4.1, and enters the winner's target directly, without a clip,
   since nothing is on screen yet. With none, it enters `initialNode`. A fire
   that chose the first pose is claimed for it (3.3). The pose is then
   entered as any other (4, 7). This is how an app starts on the pose it
   wants (Masko Life starts each evolution form this way); no call sends the
   mascot to a pose. The app's `autoPlay` wins over the graph's when the app
   gives one; with neither, the mascot starts at once. With `autoPlay`
   false, the player shows the first pose's first frame and starts only on
   `play()`. A `start` while the mascot already plays starts again the same
   way with the values and fires it has: the mascot leaves its pose (3.3),
   and the fires a start claimed for another pose are no longer claimed.
2. **Pause.** Play time stops, and so do clips and timers. `set` and `fire`
   are recorded and decided on when play resumes. `play()` resumes exactly
   where it stopped. The player also pauses itself while the mascot cannot
   be seen (a hidden page, a view off screen, an app in the background) and
   resumes when it can, unless the app paused it.
3. **Reset.** Values go back to their defaults (2.4), none of them instant
   (5.8), pending fires and the route are dropped, and the player starts
   again as in 1, with the defaults.
4. **Cutting a move.** `reset`, a `start` while the mascot plays and
   `preview.showPose` (13) cut a move in progress: the player is told
   (`interrupted`, as in 10.1), the move's target is never entered, and the
   pose they enter stops looping sounds as any pose entered (9.6).

## 12. What the engine says

The engine returns commands after every call: show a clip at a rate, show a
still, preload a clip, play or stop sounds, and report a pose entered, a move
interrupted, a fire spent or an error. It also says when it next has work
(`nextWake`). The player tells it when a clip ends or fails. `player.md`
gives the exact calls and commands every platform implements. The app hears
`pose`, `clip`, `reactionDone`, `movement` and `error`.

The app can also ask the player `position()`: the clip on screen and its
clip time, read only. It is a player call that reads what is on screen, not
an engine rule.

## 13. Preview calls, for editors and tools only

These two calls are outside the app contract: the canvas editor, the
preview and test tools use them, and an app never needs them. An app reaches
a pose by setting values (11.1) and plays moves through the canvas.

- **`preview.showPose(poseId)`** cuts to that pose at once, and cuts a move
  in progress (11.4): pending fires (dropped without `reactionDone`), the
  route and any decided start are dropped, the pose is entered with its
  timers at 0, and its main loop (7.1) is shown, or its still. The rules
  then take over as after any event (5.1): an instant winner starts at once,
  any other at the end of the loop. An unknown pose reports `error`
  `unknown_pose`.
- **`preview.playMove(edgeId)`** plays one move whose source is the current
  pose, at once, from whatever frame the pose clip is on. Any decided start
  is dropped and the pose's claimed fires are spent, as when the mascot
  leaves a pose (3.3). The move ends as any move (9), and the rules take
  over at its target. It is refused with `error` `preview_refused` when the
  edge is not a move from the current pose or a move is in progress, and
  with `error` `clipless_cycle` while no move may start (9.4).

## 14. The web player, beyond the rules

Behaviour of `@masko/player` on the web that no rule governs:

- **Tap to play.** When the browser blocks autoplay (for example iPhone Low
  Power Mode), the player shows the first frame with a tap-to-play overlay
  and starts on the first tap.
