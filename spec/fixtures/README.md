# Scenarios

Each `*.json` file here is one scenario: a canvas, a list of steps, and what
a player must show after each step. Every Masko player runs all of them: the
engine with a fake clock and fixed random numbers, and each platform again
through its own wrapper. `../rules.md` is the rulebook they prove; each
scenario names the rules it covers.

## A scenario

```json
{
  "name": "fire-reaction-pose-plays-one-loop",
  "about": "One sentence: what it proves.",
  "rules": ["3.3", "5.5"],
  "canvas": "ko-coffee",
  "patch": { "edges": { "loop-overboop-fluster": { "speed": 2 } } },
  "files": { "idle-to-watching": 1.5 },
  "random": [0.5, 0.2],
  "steps": [{ "fire": "boop", "expect": { "pose": "idle", "next": null } }]
}
```

- `name`, `about`, `rules`: the file's name, one sentence on what the
  scenario proves, and the rules it covers.
- `canvas`: a release delivery, inline, or the name of a file in
  `canvases/`:
  - `ko-coffee`: the Ko coffee canvas, with `operator` rewritten to `op`
    (the scenario `load-refuses-operator-key` puts it back) and `null`
    condition lists removed, so it is valid against
    `../release.schema.json`;
  - `otto`: an Otto test canvas with a 4 s loop on each pose and a 1 s clip
    on each move, since the original has no videos;
  - `otto-noclips`: the same Otto canvas with no videos.

  Both Otto files say `version: "2.1"` instead of the original `"1.0"`,
  which the release schema does not allow.
  Video URLs are placeholders; players under test never load them.
- `patch` (optional), applied before loading:
  - `delivery`: merged into the delivery; `null` removes a field;
  - `edges`: merged into the edge with that id; `null` removes the edge;
  - `addEdges`, `addInputs`: appended;
  - `defaults`: a new `default` for declared inputs.
- `files` (optional): the real length in seconds of some clips' files, when
  it differs from the edge's `duration`.
- `loadMs` (optional): how long some clips' first frames take to show, in
  milliseconds, by edge.
- `random` (optional): the numbers the engine draws for the extra roll
  (rules 7.3), in order. When they run out the engine draws from its own
  seeded source, so a scenario gives every number its trace depends on.
- `expectLoad` (optional): `{ "refused": "<code>" }` for a delivery the
  player must refuse (rules 1.2, 1.3). Such a scenario has no steps.

## Steps

Each step is one action, plus `expect`:

| Step                         | Meaning                                                            |
| ---------------------------- | ------------------------------------------------------------------ |
| `"start": true`              | Load and enter the initial pose at play time 0.                    |
| `"set": [name, value]`       | The app calls `set`.                                               |
| `"fire": name`               | The app calls `fire`.                                              |
| `"instant": true`            | On a `set` or `fire` step: the `instant` option (rules 5.8).       |
| `"advance": ms`              | Play time moves on.                                                |
| `"loopEnded": true`          | Play time moves to the end of the pose clip on screen.             |
| `"clipEnded": true`          | Play time moves to the end of the move clip on screen.             |
| `"failClip": edgeId`         | From now on that clip fails to load.                               |
| `"noEndSignal": edgeId`      | From now on the player never reports that clip's end.              |
| `"reset": true`              | The app calls `reset`.                                             |
| `"showPose": poseId`         | A tool calls `preview.showPose` (rules 13).                        |
| `"playMove": edgeId`         | A tool calls `preview.playMove` (rules 13).                        |

Steps before `start` set or fire values the start may use (rules 11.1).

The player in a scenario is an ideal one: a clip loads and shows at once,
reported with `clipShown`, unless `failClip` or `loadMs` says otherwise; it
plays for its file's length divided by its `speed`, rounded to whole
milliseconds, and ends there unless `noEndSignal` says otherwise.
`advance`, `loopEnded` and `clipEnded` handle, in order, everything that
happens in the time they cover: clips shown, clip ends, node timers and the
safety net. Two things at the same millisecond go in that order.

## What `expect` checks

After a step, only the fields present are checked:

- `clock`: play time in milliseconds.
- `pose`: the pose the mascot is on. During a move, that is still its
  source.
- `clip`: the edge whose clip is on screen, or `still:<pose>` for a pose's
  still image (rules 7.4, 9.5).
- `next`: the start the engine has decided but not yet shown, as
  `{ "clip": <first clip to show>, "at": <play time, or "end"> }`, or
  `null`. A start whose moment was now has already happened and shows in
  `clip`.
- `events`: everything that happened during the step, in order:
  `pose:<id>` (entered), `shown:<edge>` or `shown:still:<pose>`,
  `interrupted:<edge>` (an instant start or a call cut that move, rules
  10.1, 11.4),
  `spent:<name>` (a fire claimed by a start was spent),
  `spent:<name>:unused`, and `error:<code>`.
- `values`: names as `set` would resolve them, and the value a condition
  reads now.
- `fired`: the fires still pending, sorted.
- `preloaded`: the clips the engine asked the player to preload during the
  step, in order (rules 9.7).

## Checking the scenarios

The engine runs every scenario through an ideal player:

```bash
source scripts/env.sh
cargo test -p masko-player-engine --test scenarios -- --nocapture
```

Each platform runs them again through its own wrapper (web, Apple, Android).
Work a new scenario's expected trace out by hand from the rulebook first;
the engine's output is checked against it, never copied into it.

## The scenarios

The 91 scenarios, by group:

| Group             | Scenarios                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Loading           | `load-refuses-unknown-contract`, `load-refuses-missing-feature`, `load-refuses-unsupported-graph`, `load-refuses-newer-delivery-first`, `load-refuses-newer-contract-first`, `load-refuses-unknown-feature-first`, `load-refuses-newer-graph-first`, `load-without-player-contract`, `load-refuses-unknown-operator`, `load-refuses-operator-key`, `load-refuses-isloop-mismatch`, `load-accepts-fractional-priority`, `load-refuses-speed-input-not-a-number`, `load-refuses-string-condition-value`, `load-refuses-null-condition-value`, `variants-without-files-are-no-clip`, `av1-files-are-clips`, `mkpk-files-are-clips`, `load-refuses-number-operator`, `load-refuses-null-operator` |
| Conditions        | `defaults-apply-at-start`, `unset-name-reads-false`, `condition-groups-or`, `empty-group-dropped`, `any-state-without-conditions-never-holds`, `rest-is-computed`, `bare-names`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Priority          | `behaviors-priority-not-call-order`, `otto-attention-over-working`, `priority-then-canvas-order`, `any-state-target-here-stays`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `fire`            | `fire-waits-for-its-clip`, `fire-reaction-pose-plays-one-loop`, `fire-carried-through-route`, `fire-unwanted-is-dropped-at-once`, `fire-cut-off-is-dropped`, `fire-during-move-used-at-arrival`, `fire-named-pose-clip`, `set-on-trigger-fires`, `fire-named-clip-fails-is-spent`                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Timing            | `hover-waits-for-the-loop-end`, `let-go-waits-for-the-loop-end-unless-instant`, `app-move-waits-for-loop-end`, `no-condition-move-at-loop-end`, `no-play-once`, `after-it-plays-once`, `decision-replaced-before-its-moment`, `decision-cancelled-when-value-clears`, `ko-nodetime-in-milliseconds`                                                                                                                                                                                     |
| Instant           | `instant-grab-skips-the-way-home`, `instant-off-plays-transitions`, `instant-app-value-skips-the-route`, `instant-move-plays-its-clip-at-once`, `instant-cuts-a-move`, `instant-cuts-the-move-to-its-own-target`, `instant-from-a-still`, `instant-target-here-stays`, `instant-fire-claimed`, `instant-value-stays-until-set-again`, `instant-fire-named-clip-cuts-the-loop`                                                                                                                                                                                                                                                                                                                                  |
| Routes            | `route-via-only-exit`, `route-exit-tie-break`, `route-unreachable-falls-back`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Pose clips        | `extra-clip-roll`, `named-clip-while-held`, `preload-named-and-extra-clips`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| Moves and failure | `move-ends-when-its-clip-ends`, `move-shown-late`, `move-safety-net`, `pose-clip-safety-net`, `clip-never-shown-times-out`, `move-clip-fails-skips-to-target`, `pose-clip-fails-keeps-counting`, `pose-clip-fails-replays-previous`, `pose-clip-with-no-length-fails`, `clipless-poses`, `change-during-move`, `request-at-the-swap`, `grab-during-a-move-waits-for-its-end`, `hover-during-a-move-waits-for-its-end`, `preload-on-entry`, `speed-from-canvas-number`, `pose-clip-fails-on-screen-replays-previous`, `pose-clip-fails-short-still-lasts-a-second`, `pose-clip-fails-on-screen-start-leaves-still`, `clipless-cycle-waits-for-its-second`              |
| Start and reset   | `start-chooses-first-pose`, `reset-restores-defaults`, `calls-cut-a-move`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Preview calls     | `preview-show-pose`, `preview-play-move`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
