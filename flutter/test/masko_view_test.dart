// MaskoView and MaskoController against a fake native view: what reaches the
// platform channels and what the app hears. The device tests in
// example/integration_test play real clips with the native players.

import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:masko_player/masko_player.dart';

const methods = StandardMethodCodec();

Map<String, Object?> delivery({String release = 'r1', int sig = 1}) => {
      'deliverySchemaVersion': 1,
      'release': {'id': release},
      'runtimeGraph': {
        'initialNode': 'idle',
        'edges': [
          {
            'id': 'idle',
            'videos': {'hevc': 'https://cdn.example/idle.mov?sig=$sig'},
          },
        ],
      },
    };

/// The native side as the platform channels see it.
class FakeNative {
  FakeNative(this.tester) {
    messenger.setMockMethodCallHandler(SystemChannels.platform_views, (call) async {
      final raw = call.arguments;
      final args = raw is Map ? raw.cast<String, Object?>() : const <String, Object?>{};
      switch (call.method) {
        case 'create':
          final params = const StandardMessageCodec()
              .decodeMessage(ByteData.sublistView(args['params']! as Uint8List)) as Map;
          created.add(params.cast<String, Object?>());
          final name = 'ai.masko.player/view_${params['channel']}';
          messenger.setMockMethodCallHandler(MethodChannel(name), (call) async {
            calls.add(call);
            if (call.method == 'position') return {'clip': 'idle', 'time': 0.5};
            return null;
          });
          // Android's texture id.
          return defaultTargetPlatform == TargetPlatform.android ? 0 : null;
        case 'resize':
          return {'width': args['width'], 'height': args['height']};
        default:
          return null;
      }
    });
  }

  final WidgetTester tester;
  final created = <Map<String, Object?>>[];
  final calls = <MethodCall>[];

  TestDefaultBinaryMessenger get messenger => tester.binding.defaultBinaryMessenger;

  String get channel => 'ai.masko.player/view_${created.last['channel']}';

  List<String> get names => [for (final call in calls) call.method];

  /// Sends what the native view sends, and returns Dart's answer.
  Future<Object?> send(String method, [Object? arguments]) async {
    Object? answer;
    await messenger.handlePlatformMessage(
      channel,
      methods.encodeMethodCall(MethodCall(method, arguments)),
      (reply) => answer = reply == null ? null : methods.decodeEnvelope(reply),
    );
    await tester.pump();
    return answer;
  }
}

Widget sized(Widget child) => Directionality(
      textDirection: TextDirection.ltr,
      child: Center(child: SizedBox(width: 200, height: 200, child: child)),
    );

final ios = TargetPlatformVariant.only(TargetPlatform.iOS);
final android = TargetPlatformVariant.only(TargetPlatform.android);
final macos = TargetPlatformVariant.only(TargetPlatform.macOS);

void main() {
  testWidgets('the native view gets the delivery JSON, the props and the calls made before it', (tester) async {
    final native = FakeNative(tester);
    final controller = MaskoController()
      ..set('working', true)
      ..fire('celebrate', instant: true);
    await tester.pumpWidget(sized(MaskoView(
      delivery: {
        'data': {'delivery': delivery()},
      },
      controller: controller,
      autoPlay: false,
      sound: false,
    )));
    final params = native.created.single;
    expect(params['delivery'], jsonEncode(delivery()));
    expect(params['url'], isNull);
    expect(params['appKey'], isNull);
    expect(params['clip'], isNull);
    expect(params['autoPlay'], false);
    expect(params['sound'], false);
    expect(params['refreshes'], false);
    expect(params['calls'], [
      {
        'method': 'setBool',
        'arguments': {'name': 'working', 'value': true, 'instant': false},
      },
      {
        'method': 'fire',
        'arguments': {'name': 'celebrate', 'instant': true},
      },
    ]);
    expect(native.calls, isEmpty);
  }, variant: ios);

  testWidgets('controller calls reach the view in order', (tester) async {
    final native = FakeNative(tester);
    final controller = MaskoController();
    await tester.pumpWidget(sized(MaskoView(delivery: delivery(), controller: controller)));
    controller
      ..set('working', true, instant: true)
      ..set('score', 3)
      ..fire('boop')
      ..pause()
      ..play()
      ..reset()
      ..preview.showPose('working')
      ..preview.playMove('to-idle');
    await tester.pump();
    expect(native.names, ['setBool', 'setNumber', 'fire', 'pause', 'play', 'reset', 'showPose', 'playMove']);
    expect(native.calls[0].arguments, {'name': 'working', 'value': true, 'instant': true});
    expect(native.calls[1].arguments, {'name': 'score', 'value': 3.0, 'instant': false});
    expect(native.calls[6].arguments, {'id': 'working'});
    expect(() => controller.set('working', 'yes'), throwsArgumentError);

    final at = await controller.position();
    expect(at.clip, 'idle');
    expect(at.time, 0.5);
  }, variant: ios);

  testWidgets('the app hears the native events as typed values', (tester) async {
    final native = FakeNative(tester);
    final heard = <Object>[];
    await tester.pumpWidget(sized(MaskoView(
      delivery: delivery(),
      onPose: heard.add,
      onClip: heard.add,
      onReactionDone: heard.add,
      onMovement: heard.add,
      onError: heard.add,
    )));
    await native.send('pose', {'pose': 'idle'});
    await native.send('clip', {'clip': 'to-work', 'kind': 'move', 'rate': 1.0});
    await native.send('reactionDone', {'name': 'action::boop', 'used': false});
    await native.send('movement', {
      'edge': 'to-work',
      'movement': {
        'dx': 0,
        'path': [1, 2],
      },
      'rate': 2.0,
      'seconds': 0.25,
    });
    await native.send('error', {'code': 'clip_failed', 'detail': 'to-work'});
    expect(heard[0], 'idle');
    final clip = heard[1] as MaskoClipEvent;
    expect([clip.clip, clip.kind, clip.rate], ['to-work', MaskoClipKind.move, 1.0]);
    final reaction = heard[2] as MaskoReactionDoneEvent;
    expect([reaction.name, reaction.used], ['action::boop', false]);
    final movement = heard[3] as MaskoMovementEvent;
    expect(movement.movement, {
      'dx': 0,
      'path': [1, 2],
    });
    expect([movement.edge, movement.rate, movement.seconds], ['to-work', 2.0, 0.25]);
    final error = heard[4] as MaskoErrorEvent;
    expect([error.code, error.detail], ['clip_failed', 'to-work']);
  }, variant: ios);

  testWidgets('getDelivery answers the native player with fresh JSON, or null', (tester) async {
    final native = FakeNative(tester);
    await tester.pumpWidget(sized(MaskoView(
      delivery: delivery(),
      getDelivery: () async => {
        'data': {'delivery': delivery(sig: 2)},
      },
    )));
    expect(native.created.single['refreshes'], true);
    expect(await native.send('getDelivery'), jsonEncode(delivery(sig: 2)));

    await tester.pumpWidget(sized(MaskoView(delivery: delivery(), getDelivery: () async => throw StateError('offline'))));
    expect(await native.send('getDelivery'), isNull);

    await tester.pumpWidget(sized(MaskoView(delivery: delivery())));
    expect(await native.send('getDelivery'), isNull);
  }, variant: ios);

  testWidgets('new links keep the player; a new release, sound or getDelivery update the view', (tester) async {
    final native = FakeNative(tester);
    await tester.pumpWidget(sized(MaskoView(delivery: delivery())));

    await tester.pumpWidget(sized(MaskoView(delivery: delivery(sig: 2))));
    expect(native.names, ['updateDelivery']);
    expect(native.calls.last.arguments, {'delivery': jsonEncode(delivery(sig: 2))});

    await tester.pumpWidget(sized(MaskoView(delivery: delivery(sig: 2), sound: false)));
    expect(native.names.last, 'update');
    expect((native.calls.last.arguments as Map)['sound'], false);
    // The player keeps the JSON it was loaded with.
    expect((native.calls.last.arguments as Map)['delivery'], jsonEncode(delivery()));

    await tester.pumpWidget(sized(MaskoView(delivery: delivery(release: 'r2'), sound: false)));
    expect(native.names.last, 'update');
    expect((native.calls.last.arguments as Map)['delivery'], jsonEncode(delivery(release: 'r2')));

    await tester.pumpWidget(sized(MaskoView(delivery: delivery(release: 'r2'), sound: false, getDelivery: () async => null)));
    expect(native.names.last, 'update');
    expect((native.calls.last.arguments as Map)['refreshes'], true);

    final count = native.calls.length;
    await tester.pumpWidget(sized(MaskoView(delivery: delivery(release: 'r2'), sound: false, getDelivery: () async => null)));
    expect(native.calls.length, count, reason: 'nothing changed');
  }, variant: ios);

  testWidgets('a canvas URL with its key inside reaches the native view, with the calls made before it', (tester) async {
    final native = FakeNative(tester);
    final controller = MaskoController()..set('working', true);
    const link = 'https://api.masko.ai/v1/canvases/c1/delivery?key=masko_app_test';
    await tester.pumpWidget(sized(MaskoView(url: link, controller: controller, autoPlay: false)));
    final params = native.created.single;
    expect(params['url'], link);
    expect(params['appKey'], isNull);
    expect(params['delivery'], '');
    expect(params['clip'], isNull);
    expect(params['autoPlay'], false);
    expect(params['sound'], true);
    expect(params['refreshes'], false);
    expect(params['calls'], [
      {
        'method': 'setBool',
        'arguments': {'name': 'working', 'value': true, 'instant': false},
      },
    ]);

    controller.fire('celebrate');
    await tester.pump();
    expect(native.names, ['fire']);
  }, variant: ios);

  testWidgets('a canvas URL with its app key apart reaches the native view on Android', (tester) async {
    final native = FakeNative(tester);
    const link = 'https://api.masko.ai/v1/canvases/c1/delivery';
    await tester.pumpWidget(sized(MaskoView(url: link, appKey: 'masko_app_test', getDelivery: () async => delivery())));
    final params = native.created.single;
    expect(params['url'], link);
    expect(params['appKey'], 'masko_app_test');
    expect(params['delivery'], '');
    expect(params['refreshes'], false, reason: 'a URL renews its own links');
  }, variant: android);

  testWidgets('a new URL or app key updates the view; the same one changes nothing', (tester) async {
    final native = FakeNative(tester);
    const link = 'https://api.masko.ai/v1/canvases/c1/delivery';
    await tester.pumpWidget(sized(const MaskoView(url: link, appKey: 'masko_app_a')));

    await tester.pumpWidget(sized(const MaskoView(url: link, appKey: 'masko_app_a')));
    expect(native.calls, isEmpty);

    await tester.pumpWidget(sized(const MaskoView(url: link, appKey: 'masko_app_b')));
    expect(native.names, ['update']);
    expect((native.calls.last.arguments as Map)['appKey'], 'masko_app_b');

    await tester.pumpWidget(sized(const MaskoView(url: '$link?key=masko_app_c')));
    expect(native.names, ['update', 'update']);
    expect((native.calls.last.arguments as Map)['url'], '$link?key=masko_app_c');
    expect((native.calls.last.arguments as Map)['appKey'], isNull);

    // From a URL to a delivery: the view gets the delivery and no URL.
    await tester.pumpWidget(sized(MaskoView(delivery: delivery())));
    expect(native.names, ['update', 'update', 'update']);
    expect((native.calls.last.arguments as Map)['url'], isNull);
    expect((native.calls.last.arguments as Map)['delivery'], jsonEncode(delivery()));
  }, variant: ios);

  testWidgets('a clip plays the HEVC file on iOS', (tester) async {
    final native = FakeNative(tester);
    await tester.pumpWidget(sized(const MaskoView(clip: MaskoClipFiles(hevc: 'https://cdn.example/a.mov', stacked: 'b.mp4'))));
    expect(native.created.single['clip'], 'https://cdn.example/a.mov');
    expect(native.created.single['delivery'], '');
  }, variant: ios);

  testWidgets('a clip plays the stacked MP4 on Android', (tester) async {
    final native = FakeNative(tester);
    await tester.pumpWidget(sized(const MaskoView(clip: MaskoClipFiles(hevc: 'a.mov', stacked: 'https://cdn.example/b.mp4'))));
    expect(native.created.single['clip'], 'https://cdn.example/b.mp4');
  }, variant: android);

  testWidgets("a clip without this platform's file reports clip_failed", (tester) async {
    final native = FakeNative(tester);
    final errors = <MaskoErrorEvent>[];
    await tester.pumpWidget(sized(MaskoView(clip: const MaskoClipFiles(stacked: 'b.mp4'), onError: errors.add)));
    await tester.pump();
    expect(native.created.single['clip'], isNull);
    expect(errors.single.code, 'clip_failed');
    expect(errors.single.detail, 'the clip has no hevc file for ios');
  }, variant: ios);

  testWidgets('other platforms show nothing and report unsupported_platform', (tester) async {
    final native = FakeNative(tester);
    final errors = <MaskoErrorEvent>[];
    await tester.pumpWidget(sized(MaskoView(delivery: delivery(), onError: errors.add)));
    await tester.pump();
    expect(native.created, isEmpty);
    expect(errors.single.code, 'unsupported_platform');
  }, variant: macos);

  testWidgets('calls made while no view shows wait for the next one', (tester) async {
    final native = FakeNative(tester);
    final controller = MaskoController();
    await tester.pumpWidget(sized(MaskoView(delivery: delivery(), controller: controller)));
    await tester.pumpWidget(const SizedBox());
    controller.set('working', true);
    expect(await controller.position(), isA<MaskoPosition>().having((p) => p.clip, 'clip', isNull));
    await tester.pumpWidget(sized(MaskoView(delivery: delivery(), controller: controller)));
    expect(native.created.last['calls'], [
      {
        'method': 'setBool',
        'arguments': {'name': 'working', 'value': true, 'instant': false},
      },
    ]);
  }, variant: ios);

  testWidgets('maskoFormats asks the plugin', (tester) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('ai.masko.player'),
      (call) async => call.method == 'formats' ? 'av1' : null,
    );
    expect(await maskoFormats(), 'av1');
  }, variant: ios);
}
