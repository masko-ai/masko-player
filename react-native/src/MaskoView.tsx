// <MaskoView>: the Masko player for iOS (MaskoPlayer, Swift) or Android
// (ai.masko:player, Kotlin) as a React Native view. The handle comes through
// a ref, the events as props, as in @masko/player/react.

import { requireNativeModule, requireNativeView } from 'expo';
import {
  createElement,
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  type ComponentType,
  type ForwardedRef,
  type Ref
} from 'react';
import { Platform, type ViewProps } from 'react-native';

import { deliveryJson, releaseKey } from './delivery';
import type { MaskoEvents, MaskoHandle, MaskoViewProps, Position } from './types';

type NativeEvent<T> = { nativeEvent: T };

/** The native view's own calls, run on the main thread in the order they are made. */
export interface NativeCalls {
  setBool(name: string, value: boolean, instant: boolean): Promise<void>;
  setNumber(name: string, value: number, instant: boolean): Promise<void>;
  fire(name: string, instant: boolean): Promise<void>;
  reset(): Promise<void>;
  play(): Promise<void>;
  pause(): Promise<void>;
  position(): Promise<Position>;
  showPose(id: string): Promise<void>;
  playMove(id: string): Promise<void>;
  /** Swaps in the links of a new delivery of the release that is playing (its JSON). */
  updateDelivery(delivery: string): Promise<void>;
  /** Answers an `onDeliveryRequest`: the fresh delivery's JSON, or null when there is none. */
  answerDelivery(request: number, delivery: string | null): Promise<void>;
}

export interface NativeProps extends ViewProps {
  ref?: Ref<NativeCalls>;
  delivery: string;
  /** A lone clip's file for this platform, played in place of `delivery`. */
  clip?: string;
  /** A canvas's delivery URL, played in place of `delivery`: the native player fetches it. */
  url?: string;
  /** The app key for `url`, when the URL does not carry one. */
  appKey?: string;
  autoPlay?: boolean;
  sound: boolean;
  /** The app passed `getDelivery` with a delivery: the player asks for fresh links through `onDeliveryRequest`. */
  refreshes: boolean;
  onPose: (event: NativeEvent<MaskoEvents['pose']>) => void;
  onClip: (event: NativeEvent<MaskoEvents['clip']>) => void;
  onReactionDone: (event: NativeEvent<MaskoEvents['reactionDone']>) => void;
  onMovement: (event: NativeEvent<MaskoEvents['movement']>) => void;
  onError: (event: NativeEvent<MaskoEvents['error']>) => void;
  onDeliveryRequest: (event: NativeEvent<{ request: number }>) => void;
}

let nativeView: ComponentType<NativeProps> | undefined;

function NativeMaskoView(): ComponentType<NativeProps> {
  nativeView ??= requireNativeView<NativeProps>('MaskoPlayer');
  return nativeView;
}

/**
 * The formats to ask the release delivery for, as `formats=<this>`: `av1` on
 * iPhone and iPad, and on Android phones with an AV1 decoder, else empty.
 * AV1 files are 2 to 3 times lighter than HEVC or stacked H.264.
 */
export function maskoFormats(): string {
  return requireNativeModule<{ formats(): string }>('MaskoPlayer').formats();
}

const NOTHING: Position = { clip: null, time: 0 };

function MaskoViewWithRef(props: MaskoViewProps, ref: ForwardedRef<MaskoHandle>) {
  const {
    delivery,
    clip,
    url,
    appKey,
    getDelivery,
    sound,
    autoPlay,
    // Deprecated and ignored: the player no longer watches the person.
    pointer,
    onPose,
    onClip,
    onReactionDone,
    onMovement,
    onError,
    ...viewProps
  } = props;
  const native = useRef<NativeCalls>(null);
  const latest = useRef(props);
  latest.current = props;

  // A new release loads a new player; new links for the same release keep it.
  // getDelivery given or taken away loads the release again (the mascot
  // restarts), with the latest links.
  // A lone clip plays this platform's file: HEVC on iOS, the stacked MP4 on Android.
  // A canvas URL needs no delivery: the native player fetches it and its
  // newer releases itself, so getDelivery plays no part there.
  const clipFile = clip ? ((Platform.OS === 'ios' ? clip.hevc : clip.stacked) ?? '') : undefined;
  const canvasUrl = clipFile === undefined && url !== undefined ? url : undefined;
  const key = useMemo(
    () =>
      clipFile !== undefined
        ? `clip:${clipFile}`
        : canvasUrl !== undefined
          ? `url:${canvasUrl}`
          : releaseKey(delivery!),
    [delivery, clipFile, canvasUrl]
  );
  const refreshes = getDelivery !== undefined && canvasUrl === undefined;
  const json = useMemo(
    () => (latest.current.delivery === undefined ? '' : deliveryJson(latest.current.delivery)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [key, refreshes]
  );

  useEffect(() => {
    if (clipFile === '') {
      const file = Platform.OS === 'ios' ? 'hevc' : 'stacked';
      latest.current.onError?.({ code: 'clip_failed', detail: `the clip has no ${file} file for ${Platform.OS}` });
    }
  }, [clipFile]);

  // New links for the same release: the native player swaps them in and keeps playing.
  const given = useRef({ key, json });
  useEffect(() => {
    if (delivery === undefined || canvasUrl !== undefined) return;
    const fresh = deliveryJson(delivery);
    const last = given.current;
    given.current = { key, json: fresh };
    if (last.key === key && last.json !== fresh) native.current?.updateDelivery(fresh).catch(() => undefined);
  }, [delivery, key, canvasUrl]);

  // getDelivery requests the native view waits on. Unmounting answers them
  // with null (a view already gone ignores it), and their late answers are
  // dropped.
  const waiting = useRef(new Set<number>());
  useLayoutEffect(() => {
    const requests = waiting.current;
    const view = native.current;
    return () => {
      for (const request of requests) view?.answerDelivery(request, null).catch(() => undefined);
      requests.clear();
    };
  }, []);

  const events = useMemo(
    () => ({
      onPose: (e: NativeEvent<MaskoEvents['pose']>) => latest.current.onPose?.(e.nativeEvent),
      onClip: (e: NativeEvent<MaskoEvents['clip']>) => latest.current.onClip?.(e.nativeEvent),
      onReactionDone: (e: NativeEvent<MaskoEvents['reactionDone']>) => latest.current.onReactionDone?.(e.nativeEvent),
      onMovement: (e: NativeEvent<MaskoEvents['movement']>) => latest.current.onMovement?.(e.nativeEvent),
      onError: (e: NativeEvent<MaskoEvents['error']>) => latest.current.onError?.(e.nativeEvent),
      onDeliveryRequest: (e: NativeEvent<{ request: number }>) => {
        const { request } = e.nativeEvent;
        waiting.current.add(request);
        const get = latest.current.getDelivery;
        const fresh = async () => {
          try {
            const answer = get ? await get() : null;
            return answer == null ? null : deliveryJson(answer);
          } catch {
            return null;
          }
        };
        fresh()
          .then((answer) => {
            if (waiting.current.delete(request)) return native.current?.answerDelivery(request, answer);
          })
          .catch(() => undefined);
      }
    }),
    []
  );

  useImperativeHandle(ref, (): MaskoHandle => {
    // The calls return nothing, as on the other platforms; a call to a view
    // that is gone does nothing.
    const call = (f: (n: NativeCalls) => Promise<unknown>) => {
      const n = native.current;
      if (n) f(n).catch(() => undefined);
    };
    return {
      set: (name, value, options) => {
        const instant = options?.instant === true;
        call((n) => (typeof value === 'boolean' ? n.setBool(name, value, instant) : n.setNumber(name, value, instant)));
      },
      fire: (name, options) => call((n) => n.fire(name, options?.instant === true)),
      reset: () => call((n) => n.reset()),
      play: () => call((n) => n.play()),
      pause: () => call((n) => n.pause()),
      position: async () => {
        try {
          return (await native.current?.position()) ?? NOTHING;
        } catch {
          return NOTHING;
        }
      },
      preview: {
        showPose: (id) => call((n) => n.showPose(id)),
        playMove: (id) => call((n) => n.playMove(id))
      }
    };
  }, []);

  return createElement(NativeMaskoView(), {
    ...viewProps,
    ...events,
    ref: native,
    delivery: json,
    clip: clipFile || undefined,
    url: canvasUrl,
    appKey: canvasUrl !== undefined ? appKey : undefined,
    autoPlay,
    sound: sound ?? true,
    refreshes
  });
}

/** A Masko mascot. Size it with `style`; drive it through the ref. */
export const MaskoView = forwardRef<MaskoHandle, MaskoViewProps>(MaskoViewWithRef);
MaskoView.displayName = 'MaskoView';
