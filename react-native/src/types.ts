// What the app gives and hears: the same names as the other players
// (spec/player.md, "What the app sees").

import type { ViewProps } from 'react-native';

/** A release delivery, as the player reads it. Every other field is kept as given. */
export interface Delivery {
  deliverySchemaVersion: number;
  runtimeGraph: { [key: string]: unknown };
  [key: string]: unknown;
}

/**
 * What `delivery` accepts: the delivery, its JSON, or the API response around
 * it (`{ data: { delivery } }` from `GET .../releases/{id}/delivery`).
 */
export type DeliveryInput = Delivery | string | { delivery: Delivery } | { data: { delivery: Delivery } };

/** A value for `set`: a boolean or a number. */
export type Value = boolean | number;

/** Options of `set` and `fire`. */
export interface ValueOptions {
  /**
   * What this value starts happens now: an Any State start goes straight into its target pose,
   * skipping every transition clip and route, and a move plays its own clip (rules.md 5.8). Off
   * by default.
   */
  instant?: boolean;
}

/** What a clip on screen is: a pose's clip or a move. */
export type ClipKind = 'pose' | 'move';

/** What the app hears. */
export interface MaskoEvents {
  /** The mascot entered a pose. */
  pose: { pose: string };
  /** A clip, or a pose's still (`still:<pose>`), is now on screen. */
  clip: { clip: string; kind: ClipKind | 'still'; rate: number };
  /** A fire is done: used by a start, or spent unused. `name` is the full name, as `action::boop`. */
  reactionDone: { name: string; used: boolean };
  /** A move with `movement` is now on screen: the app may move the mascot. */
  movement: { edge: string; movement: unknown; rate: number; seconds: number };
  /** Something went wrong, with a code from rules.md or the player. */
  error: { code: string; detail?: string };
}

/** The clip on screen and its clip time, in seconds of its file. */
export interface Position {
  clip: string | null;
  time: number;
}

/** What the ref gives: the same calls on every platform. */
export interface MaskoHandle {
  /** `{ instant: true }`: what it starts happens now; an Any State start cuts straight in. */
  set(name: string, value: Value, options?: ValueOptions): void;
  fire(name: string, options?: ValueOptions): void;
  reset(): void;
  play(): void;
  pause(): void;
  /** The clip on screen and its clip time in seconds. */
  position(): Promise<Position>;
  /** For editors and tools only (rules.md 13). */
  preview: { showPose(id: string): void; playMove(id: string): void };
}

/** One animation's files, played on its own: HEVC on iOS, the stacked MP4 on Android. */
export interface ClipFiles {
  hevc?: string;
  stacked?: string;
}

/**
 * A canvas played from its URL, with no backend: the player reads the
 * canvas's latest release itself, keeps every file, and moves to a newer
 * release on its own.
 */
export interface CanvasUrl {
  /**
   * `https://api.masko.ai/v1/canvases/{canvasId}/delivery`, with the app key
   * inside it (`?key=masko_app_...`) or given apart in `appKey`.
   */
  url: string;
  /** An app key of the canvas's mascot (`masko_app_...`), when the URL does not carry one. */
  appKey?: string;
}

/**
 * What the view plays: a canvas from its `url`, a release `delivery`, or one
 * animation's `clip` files, looped on its own. Give one of them.
 */
export type MaskoViewProps = ViewProps &
  MaskoViewOptions &
  (
    | (CanvasUrl & { delivery?: undefined; clip?: undefined })
    | { delivery: DeliveryInput; clip?: undefined; url?: undefined; appKey?: undefined }
    | { clip: ClipFiles; delivery?: undefined; url?: undefined; appKey?: undefined }
  );

export interface MaskoViewOptions {
  /**
   * Returns a fresh delivery of the same release, before signed links expire.
   * Giving or taking it away loads the release again, which restarts the mascot.
   * Not needed with `url`, which fetches its own deliveries; it is ignored there.
   */
  getDelivery?: () => DeliveryInput | Promise<DeliveryInput>;
  /** Lets clips with their own voice be heard. On by default; false mutes them. Edge sounds play either way. */
  sound?: boolean;
  /** Starts at once. Defaults to the canvas's `autoPlay`, else true. Read when a release loads. */
  autoPlay?: boolean;
  /**
   * @deprecated Ignored: the player no longer watches the person. The app sets
   * `behavior::pointerHover` and fires `action::click` itself.
   */
  pointer?: boolean;
  onPose?: (event: MaskoEvents['pose']) => void;
  onClip?: (event: MaskoEvents['clip']) => void;
  onReactionDone?: (event: MaskoEvents['reactionDone']) => void;
  onMovement?: (event: MaskoEvents['movement']) => void;
  onError?: (event: MaskoEvents['error']) => void;
}
