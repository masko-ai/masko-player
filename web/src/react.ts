// <MaskoPlayer>: the player as a React component, using the app's React.
// The handle comes through a ref, the events as props.

import {
  createElement,
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  type CSSProperties,
  type ForwardedRef
} from 'react';
import { readDelivery, releaseShape, type DeliveryInput } from './delivery.ts';
import type { ReleasePlayer } from './canvas-url.ts';
import { createPlayer, mountMascot, type MaskoHandle, type MountOptions } from './mount.ts';
import type { MaskoEventName, MaskoEvents, MaskoListener } from './player.ts';

export type { MaskoHandle, MountOptions } from './mount.ts';
export type { MaskoEvents, Position, ValueOptions } from './player.ts';

export type MaskoPlayerProps = MountOptions & {
  className?: string;
  style?: CSSProperties;
  onPose?: (event: MaskoEvents['pose']) => void;
  onClip?: (event: MaskoEvents['clip']) => void;
  onReactionDone?: (event: MaskoEvents['reactionDone']) => void;
  onMovement?: (event: MaskoEvents['movement']) => void;
  onError?: (event: MaskoEvents['error']) => void;
};

const EVENTS: [MaskoEventName, keyof MaskoPlayerProps][] = [
  ['pose', 'onPose'],
  ['clip', 'onClip'],
  ['reactionDone', 'onReactionDone'],
  ['movement', 'onMovement'],
  ['error', 'onError']
];

/** A listener added through the ref, kept across the players of new releases. */
interface Subscription {
  event: MaskoEventName;
  listener: MaskoListener<MaskoEventName>;
  off: (() => void) | null;
}

/** Which release a delivery is: a new release mounts a new player, new links only swap URLs. */
function releaseKey(delivery: DeliveryInput | undefined): string {
  if (delivery === undefined) return '';
  try {
    return releaseShape(readDelivery(delivery).delivery);
  } catch {
    return String(delivery);
  }
}

function MaskoPlayerView(props: MaskoPlayerProps, ref: ForwardedRef<MaskoHandle>) {
  const box = useRef<HTMLDivElement>(null);
  const player = useRef<ReleasePlayer | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const mounted = useRef<DeliveryInput | null>(null);
  const subscriptions = useRef(new Set<Subscription>());
  const { clip, url } = props;
  const appKey = props.url ? props.appKey : undefined;
  const key = useMemo(
    () =>
      url
        ? `url:${url}|${appKey}`
        : clip
          ? `clip:${clip.webm ?? ''}|${clip.hevc ?? ''}`
          : releaseKey(props.delivery),
    [props.delivery, clip?.webm, clip?.hevc, url, appKey]
  );
  const hasRefresh = props.getDelivery !== undefined;

  useEffect(() => {
    const element = box.current;
    if (!element) return;
    const p = latest.current;
    const common = {
      getDelivery: hasRefresh ? () => latest.current.getDelivery!() : undefined,
      sound: p.sound,
      autoPlay: p.autoPlay,
      preload: p.preload
    };
    // A canvas from its URL follows its newest release itself.
    const created: ReleasePlayer = p.url
      ? mountMascot(element, { url: p.url, appKey: p.appKey, sound: p.sound, autoPlay: p.autoPlay, preload: p.preload })
      : createPlayer(element, p.clip ? { ...common, clip: p.clip } : { ...common, delivery: p.delivery! })
          .player;
    for (const [event, prop] of EVENTS) {
      created.on(event, (e) => {
        const handler = latest.current[prop] as ((e: unknown) => void) | undefined;
        handler?.(e);
      });
    }
    // Listeners added with `ref.current.on` hear the new player too.
    for (const s of subscriptions.current) s.off = created.on(s.event, s.listener);
    player.current = created;
    mounted.current = p.delivery ?? null;
    return () => {
      created.destroy();
      player.current = null;
      for (const s of subscriptions.current) s.off = null;
    };
    // A new player only for a new release.
  }, [key, hasRefresh]);

  // New links for the same release: swap URLs, keep playing.
  useEffect(() => {
    if (!player.current || !props.delivery || mounted.current === props.delivery) return;
    mounted.current = props.delivery;
    player.current.updateDelivery(props.delivery);
  }, [props.delivery]);

  useEffect(() => {
    player.current?.setSound(props.sound ?? true);
  }, [props.sound]);

  useImperativeHandle(
    ref,
    (): MaskoHandle => ({
      set: (name, value, options) => player.current?.set(name, value, options),
      fire: (name, options) => player.current?.fire(name, options),
      reset: () => player.current?.reset(),
      play: () => player.current?.play(),
      pause: () => player.current?.pause(),
      destroy: () => player.current?.destroy(),
      position: () => player.current?.position() ?? { clip: null, time: 0 },
      updateDelivery: (delivery) => player.current?.updateDelivery(delivery) ?? false,
      setSound: (on) => player.current?.setSound(on),
      on: (event, handler) => {
        const s: Subscription = { event, listener: handler as MaskoListener<MaskoEventName>, off: null };
        s.off = player.current?.on(event, handler) ?? null;
        subscriptions.current.add(s);
        return () => {
          subscriptions.current.delete(s);
          s.off?.();
        };
      },
      preview: {
        showPose: (id) => player.current?.preview.showPose(id),
        playMove: (id) => player.current?.preview.playMove(id)
      }
    }),
    []
  );

  return createElement('div', { ref: box, className: props.className, style: props.style });
}

/** A Masko mascot. Size it with `className` or `style`; drive it through the ref. */
export const MaskoPlayer = forwardRef<MaskoHandle, MaskoPlayerProps>(MaskoPlayerView);
MaskoPlayer.displayName = 'MaskoPlayer';
