// The JS layer against a fake native view: props and ref calls reach the
// right native calls, native events reach the callbacks.

import { act, render } from '@testing-library/react-native';
import { createRef } from 'react';

import { MaskoView, type DeliveryInput, type MaskoHandle } from '../src';
import type { NativeCalls, NativeProps } from '../src/MaskoView';

type Calls = { [K in keyof NativeCalls]: jest.Mock };

const fake: { props: NativeProps | null; calls: Calls; requested: string[] } = {
  props: null,
  calls: {} as Calls,
  requested: []
};

function resetCalls() {
  fake.calls = {
    setBool: jest.fn().mockResolvedValue(undefined),
    setNumber: jest.fn().mockResolvedValue(undefined),
    fire: jest.fn().mockResolvedValue(undefined),
    reset: jest.fn().mockResolvedValue(undefined),
    play: jest.fn().mockResolvedValue(undefined),
    pause: jest.fn().mockResolvedValue(undefined),
    position: jest.fn().mockResolvedValue({ clip: 'idle', time: 0.25 }),
    showPose: jest.fn().mockResolvedValue(undefined),
    playMove: jest.fn().mockResolvedValue(undefined),
    updateDelivery: jest.fn().mockResolvedValue(undefined),
    answerDelivery: jest.fn().mockResolvedValue(undefined)
  };
}

jest.mock('expo', () => {
  const { forwardRef, useImperativeHandle } = jest.requireActual('react');
  return {
    requireNativeModule: () => ({ formats: () => 'av1' }),
    requireNativeView: (name: string) => {
      fake.requested.push(name);
      return forwardRef((props: NativeProps, ref: unknown) => {
        fake.props = props;
        useImperativeHandle(ref, () => fake.calls);
        return null;
      });
    }
  };
});

const delivery = {
  deliverySchemaVersion: 1,
  playerContract: 1,
  requires: [],
  release: { id: 'rel_1' },
  deliveryAccess: { urlMode: 'signed', signedUrlExpiresAt: '2026-10-23T12:00:00Z' },
  runtimeGraph: {
    runtimeGraphSchemaVersion: 1,
    initialNode: 'idle',
    nodes: [{ id: 'idle' }],
    edges: [
      {
        id: 'idle',
        source: 'idle',
        target: 'idle',
        isLoop: true,
        duration: 1,
        videos: { hevc: 'https://cdn.example/idle.mov?sig=a' }
      }
    ]
  }
};

function withLinks(sig: string) {
  const copy = JSON.parse(JSON.stringify(delivery));
  copy.runtimeGraph.edges[0].videos.hevc = `https://cdn.example/idle.mov?sig=${sig}`;
  return copy;
}

function props(): NativeProps {
  if (!fake.props) throw new Error('the native view was not rendered');
  return fake.props;
}

/** Lets pending promise callbacks run. */
async function settle() {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
}

beforeEach(() => {
  fake.props = null;
  resetCalls();
});

describe('props', () => {
  it('renders the MaskoPlayer native view with the delivery JSON and defaults', () => {
    render(<MaskoView delivery={delivery} style={{ width: 200, height: 200 }} testID="mascot" />);
    expect(fake.requested).toEqual(['MaskoPlayer']);
    expect(JSON.parse(props().delivery)).toEqual(delivery);
    expect(props().sound).toBe(true);
    expect(props().autoPlay).toBeUndefined();
    expect(props().refreshes).toBe(false);
    expect(props().style).toEqual({ width: 200, height: 200 });
    expect(props().testID).toBe('mascot');
  });

  it('takes the delivery, its JSON, or the API response around it', () => {
    const json = JSON.stringify(delivery);
    for (const input of [delivery, json, { delivery }, { data: { delivery } }, JSON.stringify({ data: { delivery } })]) {
      fake.props = null;
      render(<MaskoView delivery={input} />);
      expect(props().delivery).toBe(json);
    }
  });

  it('passes text that is not JSON as is, for the native player to refuse', () => {
    render(<MaskoView delivery="not json" />);
    expect(props().delivery).toBe('not json');
  });

  it('keeps the loaded delivery for new links of the same release, and gives the player the links', () => {
    const view = render(<MaskoView delivery={delivery} />);
    const first = props().delivery;
    expect(fake.calls.updateDelivery).not.toHaveBeenCalled();
    view.rerender(<MaskoView delivery={withLinks('b')} />);
    expect(props().delivery).toBe(first);
    expect(fake.calls.updateDelivery.mock.calls).toEqual([[JSON.stringify(withLinks('b'))]]);
    // The same links again, in a new object: nothing to swap.
    view.rerender(<MaskoView delivery={{ data: { delivery: withLinks('b') } }} />);
    expect(fake.calls.updateDelivery).toHaveBeenCalledTimes(1);
    // Back to the first links: swapped in too.
    view.rerender(<MaskoView delivery={JSON.parse(JSON.stringify(delivery))} />);
    expect(props().delivery).toBe(first);
    expect(fake.calls.updateDelivery.mock.calls).toEqual([[JSON.stringify(withLinks('b'))], [first]]);
  });

  it('loads a new release', () => {
    const view = render(<MaskoView delivery={delivery} />);
    const next = { ...withLinks('c'), release: { id: 'rel_2' } };
    view.rerender(<MaskoView delivery={next} />);
    expect(JSON.parse(props().delivery)).toEqual(next);
    expect(fake.calls.updateDelivery).not.toHaveBeenCalled();
  });

  it('reloads with the latest links when getDelivery is given or taken away', () => {
    const view = render(<MaskoView delivery={delivery} />);
    view.rerender(<MaskoView delivery={withLinks('b')} />);
    expect(JSON.parse(props().delivery)).toEqual(delivery);
    view.rerender(<MaskoView delivery={withLinks('b')} getDelivery={() => delivery} />);
    expect(props().refreshes).toBe(true);
    expect(props().delivery).toBe(JSON.stringify(withLinks('b')));
    view.rerender(<MaskoView delivery={withLinks('c')} getDelivery={() => delivery} />);
    expect(props().delivery).toBe(JSON.stringify(withLinks('b')));
    view.rerender(<MaskoView delivery={withLinks('c')} />);
    expect(props().refreshes).toBe(false);
    expect(props().delivery).toBe(JSON.stringify(withLinks('c')));
  });

  it('passes sound, autoPlay and whether getDelivery is given', () => {
    const view = render(<MaskoView delivery={delivery} sound autoPlay={false} />);
    expect(props().sound).toBe(true);
    expect(props().autoPlay).toBe(false);
    view.rerender(<MaskoView delivery={delivery} sound={false} autoPlay={false} getDelivery={() => delivery} />);
    expect(props().sound).toBe(false);
    expect(props().refreshes).toBe(true);
  });

  it('drops the deprecated pointer prop: the app handles the person itself', () => {
    render(<MaskoView delivery={delivery} pointer={false} />);
    expect('pointer' in props()).toBe(false);
  });
});

describe('clip', () => {
  const wave = { hevc: 'https://cdn.example/wave.mov', stacked: 'https://cdn.example/wave.mp4' };

  it("plays a lone clip from this platform's file, with no delivery", () => {
    render(<MaskoView clip={wave} />);
    expect(props().clip).toBe(wave.hevc);
    expect(props().delivery).toBe('');
  });

  it('keeps the player for the same file, and loads another file anew', () => {
    const view = render(<MaskoView clip={{ ...wave }} />);
    view.rerender(<MaskoView clip={{ ...wave }} />);
    expect(props().clip).toBe(wave.hevc);
    view.rerender(<MaskoView clip={{ hevc: 'https://cdn.example/hop.mov' }} />);
    expect(props().clip).toBe('https://cdn.example/hop.mov');
    expect(fake.calls.updateDelivery).not.toHaveBeenCalled();
  });

  it("reports clip_failed when the clip has no file for this platform", () => {
    const onError = jest.fn();
    render(<MaskoView clip={{ stacked: wave.stacked }} onError={onError} />);
    expect(props().clip).toBeUndefined();
    expect(onError).toHaveBeenCalledWith({ code: 'clip_failed', detail: 'the clip has no hevc file for ios' });
  });
});

describe('url', () => {
  const canvas = 'https://api.masko.ai/v1/canvases/can_1/delivery';

  it('passes a canvas URL with its key inside to the native view, with no delivery', () => {
    render(<MaskoView url={`${canvas}?key=masko_app_x`} autoPlay={false} sound={false} />);
    expect(props().url).toBe(`${canvas}?key=masko_app_x`);
    expect(props().appKey).toBeUndefined();
    expect(props().delivery).toBe('');
    expect(props().clip).toBeUndefined();
    expect(props().autoPlay).toBe(false);
    expect(props().sound).toBe(false);
    expect(props().refreshes).toBe(false);
  });

  it('passes the app key given apart', () => {
    render(<MaskoView url={canvas} appKey="masko_app_y" />);
    expect(props().url).toBe(canvas);
    expect(props().appKey).toBe('masko_app_y');
  });

  it('ignores getDelivery: the native player fetches the canvas itself', () => {
    const getDelivery = jest.fn(() => delivery);
    const view = render(<MaskoView url={canvas} appKey="masko_app_y" getDelivery={getDelivery} />);
    expect(props().refreshes).toBe(false);
    view.rerender(<MaskoView url={canvas} appKey="masko_app_y" />);
    expect(props().refreshes).toBe(false);
    expect(getDelivery).not.toHaveBeenCalled();
    expect(fake.calls.updateDelivery).not.toHaveBeenCalled();
  });

  it('moves between a URL and a delivery', () => {
    const view = render(<MaskoView url={canvas} appKey="masko_app_y" />);
    view.rerender(<MaskoView delivery={delivery} />);
    expect(props().url).toBeUndefined();
    expect(props().appKey).toBeUndefined();
    expect(JSON.parse(props().delivery)).toEqual(delivery);
    view.rerender(<MaskoView url={canvas} />);
    expect(props().url).toBe(canvas);
    expect(props().delivery).toBe('');
    expect(fake.calls.updateDelivery).not.toHaveBeenCalled();
  });

  it('drives the player through the ref and hears its events as with a delivery', () => {
    const ref = createRef<MaskoHandle>();
    const onPose = jest.fn();
    render(<MaskoView ref={ref} url={canvas} appKey="masko_app_y" onPose={onPose} />);
    ref.current!.set('working', true);
    ref.current!.fire('celebrate', { instant: true });
    props().onPose({ nativeEvent: { pose: 'idle' } });
    expect(fake.calls.setBool.mock.calls).toEqual([['working', true, false]]);
    expect(fake.calls.fire.mock.calls).toEqual([['celebrate', true]]);
    expect(onPose).toHaveBeenCalledWith({ pose: 'idle' });
  });

  it('leaves url and appKey out for a delivery', () => {
    render(<MaskoView delivery={delivery} />);
    expect(props().url).toBeUndefined();
    expect(props().appKey).toBeUndefined();
  });
});

describe('ref', () => {
  it('maps every call to its native call', async () => {
    const ref = createRef<MaskoHandle>();
    render(<MaskoView ref={ref} delivery={delivery} />);
    const handle = ref.current!;
    handle.set('working', true);
    handle.set('score', 3.5);
    handle.set('flag', false);
    handle.set('count', 0);
    handle.set('heldContact', true, { instant: true });
    handle.set('level', 2, { instant: true });
    handle.fire('celebrate');
    handle.fire('boop', { instant: true });
    handle.reset();
    handle.pause();
    handle.play();
    handle.preview.showPose('idle');
    handle.preview.playMove('wave');
    expect(fake.calls.setBool.mock.calls).toEqual([
      ['working', true, false],
      ['flag', false, false],
      ['heldContact', true, true]
    ]);
    expect(fake.calls.setNumber.mock.calls).toEqual([
      ['score', 3.5, false],
      ['count', 0, false],
      ['level', 2, true]
    ]);
    expect(fake.calls.fire.mock.calls).toEqual([
      ['celebrate', false],
      ['boop', true]
    ]);
    expect(fake.calls.reset).toHaveBeenCalledTimes(1);
    expect(fake.calls.pause).toHaveBeenCalledTimes(1);
    expect(fake.calls.play).toHaveBeenCalledTimes(1);
    expect(fake.calls.showPose.mock.calls).toEqual([['idle']]);
    expect(fake.calls.playMove.mock.calls).toEqual([['wave']]);
    await expect(handle.position()).resolves.toEqual({ clip: 'idle', time: 0.25 });
  });

  it('keeps the same handle across renders', () => {
    const ref = createRef<MaskoHandle>();
    const view = render(<MaskoView ref={ref} delivery={delivery} />);
    const first = ref.current;
    view.rerender(<MaskoView ref={ref} delivery={delivery} sound />);
    expect(ref.current).toBe(first);
  });

  it('ignores a native call that fails, as when the view is gone', async () => {
    const ref = createRef<MaskoHandle>();
    render(<MaskoView ref={ref} delivery={delivery} />);
    fake.calls.fire.mockRejectedValue(new Error('View not found'));
    fake.calls.position.mockRejectedValue(new Error('View not found'));
    expect(() => ref.current!.fire('celebrate')).not.toThrow();
    await expect(ref.current!.position()).resolves.toEqual({ clip: null, time: 0 });
    await settle();
  });

  it('does nothing after unmount', async () => {
    const ref = createRef<MaskoHandle>();
    const view = render(<MaskoView ref={ref} delivery={delivery} />);
    const handle = ref.current!;
    view.unmount();
    handle.fire('celebrate');
    expect(fake.calls.fire).not.toHaveBeenCalled();
    await expect(handle.position()).resolves.toEqual({ clip: null, time: 0 });
  });
});

describe('events', () => {
  it('reach the callbacks with the native payload', () => {
    const onPose = jest.fn();
    const onClip = jest.fn();
    const onReactionDone = jest.fn();
    const onMovement = jest.fn();
    const onError = jest.fn();
    render(
      <MaskoView
        delivery={delivery}
        onPose={onPose}
        onClip={onClip}
        onReactionDone={onReactionDone}
        onMovement={onMovement}
        onError={onError}
      />
    );
    props().onPose({ nativeEvent: { pose: 'idle' } });
    props().onClip({ nativeEvent: { clip: 'still:idle', kind: 'still', rate: 1 } });
    props().onReactionDone({ nativeEvent: { name: 'behavior::celebrate', used: true } });
    props().onMovement({ nativeEvent: { edge: 'walk', movement: { dx: 120, unit: 'pt' }, rate: 2, seconds: 0.5 } });
    props().onError({ nativeEvent: { code: 'clip_failed' } });
    props().onError({ nativeEvent: { code: 'refresh_failed', detail: 'offline' } });
    expect(onPose).toHaveBeenCalledWith({ pose: 'idle' });
    expect(onClip).toHaveBeenCalledWith({ clip: 'still:idle', kind: 'still', rate: 1 });
    expect(onReactionDone).toHaveBeenCalledWith({ name: 'behavior::celebrate', used: true });
    expect(onMovement).toHaveBeenCalledWith({ edge: 'walk', movement: { dx: 120, unit: 'pt' }, rate: 2, seconds: 0.5 });
    expect(onError.mock.calls).toEqual([[{ code: 'clip_failed' }], [{ code: 'refresh_failed', detail: 'offline' }]]);
  });

  it('reach the latest callback, and the native handlers stay the same', () => {
    const first = jest.fn();
    const second = jest.fn();
    const view = render(<MaskoView delivery={delivery} onPose={first} />);
    const handler = props().onPose;
    view.rerender(<MaskoView delivery={delivery} onPose={second} />);
    expect(props().onPose).toBe(handler);
    props().onPose({ nativeEvent: { pose: 'working' } });
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith({ pose: 'working' });
  });

  it('are fine without callbacks', () => {
    render(<MaskoView delivery={delivery} />);
    expect(() => props().onError({ nativeEvent: { code: 'unsupported_contract' } })).not.toThrow();
  });
});

describe('getDelivery', () => {
  it('answers a native request with the fresh delivery JSON', async () => {
    const fresh = withLinks('fresh');
    const getDelivery = jest.fn().mockResolvedValue({ data: { delivery: fresh } });
    render(<MaskoView delivery={delivery} getDelivery={getDelivery} />);
    props().onDeliveryRequest({ nativeEvent: { request: 7 } });
    await settle();
    expect(getDelivery).toHaveBeenCalledTimes(1);
    expect(fake.calls.answerDelivery.mock.calls).toEqual([[7, JSON.stringify(fresh)]]);
  });

  it('takes a delivery returned without a promise, or as JSON', async () => {
    const view = render(<MaskoView delivery={delivery} getDelivery={() => withLinks('sync')} />);
    props().onDeliveryRequest({ nativeEvent: { request: 1 } });
    await settle();
    view.rerender(<MaskoView delivery={delivery} getDelivery={() => JSON.stringify(withLinks('text'))} />);
    props().onDeliveryRequest({ nativeEvent: { request: 2 } });
    await settle();
    expect(fake.calls.answerDelivery.mock.calls).toEqual([
      [1, JSON.stringify(withLinks('sync'))],
      [2, JSON.stringify(withLinks('text'))]
    ]);
  });

  it('answers null when getDelivery fails, so the player reports refresh_failed', async () => {
    const getDelivery = jest.fn().mockRejectedValue(new Error('offline'));
    render(<MaskoView delivery={delivery} getDelivery={getDelivery} />);
    props().onDeliveryRequest({ nativeEvent: { request: 3 } });
    await settle();
    expect(fake.calls.answerDelivery.mock.calls).toEqual([[3, null]]);
  });

  it('answers waiting requests with null on unmount, and drops their late answers', async () => {
    let answer: (value: DeliveryInput) => void = () => undefined;
    const getDelivery = jest.fn(() => new Promise<DeliveryInput>((resolve) => (answer = resolve)));
    const view = render(<MaskoView delivery={delivery} getDelivery={getDelivery} />);
    props().onDeliveryRequest({ nativeEvent: { request: 5 } });
    await settle();
    expect(fake.calls.answerDelivery).not.toHaveBeenCalled();
    view.unmount();
    expect(fake.calls.answerDelivery.mock.calls).toEqual([[5, null]]);
    answer(withLinks('late'));
    await settle();
    expect(fake.calls.answerDelivery).toHaveBeenCalledTimes(1);
  });

  it('answers null when getDelivery is gone', async () => {
    const view = render(<MaskoView delivery={delivery} getDelivery={() => delivery} />);
    view.rerender(<MaskoView delivery={delivery} />);
    expect(props().refreshes).toBe(false);
    props().onDeliveryRequest({ nativeEvent: { request: 4 } });
    await settle();
    expect(fake.calls.answerDelivery.mock.calls).toEqual([[4, null]]);
  });
});

test('maskoFormats gives what the native player asks the delivery for', () => {
  const { maskoFormats } = jest.requireActual('../src') as typeof import('../src');
  expect(maskoFormats()).toBe('av1');
});
