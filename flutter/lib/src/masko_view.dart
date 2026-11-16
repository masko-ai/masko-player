// MaskoView: the Masko player for iOS (MaskoPlayer, Swift) or Android
// (ai.masko:player, Kotlin) as a Flutter platform view. The app drives it
// with a MaskoController and hears its events as callbacks, as in
// @masko/player-react-native.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'delivery.dart';
import 'types.dart';

const _viewType = 'ai.masko.player/view';
const _plugin = MethodChannel('ai.masko.player');

/// The formats to ask the release delivery for, as `formats=<this>`: `av1`
/// on iPhone and iPad, and on Android phones with an AV1 decoder, else
/// empty. AV1 files are 2 to 3 times lighter than HEVC or stacked H.264.
Future<String> maskoFormats() async => await _plugin.invokeMethod<String>('formats') ?? '';

/// A call to the native player.
typedef _Call = ({String method, Map<String, Object?> arguments});

/// Drives a [MaskoView] with the calls every Masko player has.
///
/// Calls made before the view first builds reach the player before it
/// starts, so the values set then choose the first pose (rules.md 11.1).
/// Calls made while no view shows wait for the next one. A controller drives
/// one view at a time.
class MaskoController {
  _MaskoViewState? _view;
  final List<_Call> _waiting = [];

  /// Holds [value], a `bool` or a number, under [name] until the next `set`
  /// of that name. With [instant], what it starts happens now: an Any State
  /// start goes straight into its target pose, skipping every transition
  /// clip and route, and a move plays its own clip (rules.md 5.8).
  void set(String name, Object value, {bool instant = false}) {
    if (value is bool) {
      _call('setBool', {'name': name, 'value': value, 'instant': instant});
    } else if (value is num) {
      _call('setNumber', {'name': name, 'value': value.toDouble(), 'instant': instant});
    } else {
      throw ArgumentError.value(value, 'value', 'must be a bool or a number');
    }
  }

  /// Fires [name] once. With [instant], what it starts happens now.
  void fire(String name, {bool instant = false}) =>
      _call('fire', {'name': name, 'instant': instant});

  /// Values go back to their defaults and the mascot starts again
  /// (rules.md 11.3).
  void reset() => _call('reset', const {});

  void play() => _call('play', const {});

  void pause() => _call('pause', const {});

  /// The clip on screen and its clip time in seconds.
  Future<MaskoPosition> position() async =>
      await _view?._position() ?? MaskoPosition.nothing;

  /// For editors and tools only (rules.md 13).
  late final MaskoPreview preview = MaskoPreview._(this);

  void _call(String method, Map<String, Object?> arguments) {
    final view = _view;
    if (view != null) {
      view._call(method, arguments);
    } else {
      _waiting.add((method: method, arguments: arguments));
    }
  }

  List<_Call> _take() {
    final calls = List.of(_waiting);
    _waiting.clear();
    return calls;
  }
}

/// Calls for editors and tools only (rules.md 13). An app reaches a pose by
/// setting values and plays moves through the canvas.
class MaskoPreview {
  MaskoPreview._(this._controller);

  final MaskoController _controller;

  /// Cuts to the pose [id] at once.
  void showPose(String id) => _controller._call('showPose', {'id': id});

  /// Plays the move [id] now.
  void playMove(String id) => _controller._call('playMove', {'id': id});
}

/// A Masko mascot: a canvas from its [url], a release [delivery], or one
/// animation's [clip] files looped on its own. Give exactly one of them. It
/// fills its parent; size it with a `SizedBox`.
class MaskoView extends StatefulWidget {
  const MaskoView({
    super.key,
    this.url,
    this.appKey,
    this.delivery,
    this.clip,
    this.controller,
    this.getDelivery,
    this.sound = true,
    this.autoPlay,
    this.onPose,
    this.onClip,
    this.onReactionDone,
    this.onMovement,
    this.onError,
  })  : assert(
          (url == null ? 0 : 1) + (delivery == null ? 0 : 1) + (clip == null ? 0 : 1) == 1,
          'MaskoView plays a url, a delivery or a clip: give one of them.',
        ),
        assert(appKey == null || url != null, 'appKey goes with url.');

  /// A canvas's URL, `https://api.masko.ai/v1/canvases/{canvasId}/delivery`,
  /// for an app with no backend: the native player asks it for the canvas's
  /// latest release, keeps every file on the device so the mascot also plays
  /// offline, and switches to a newer release on its own. Read with an app
  /// key of the mascot (`masko_app_...`), in [appKey] or inside the URL
  /// (`...delivery?key=masko_app_...`). In place of [delivery];
  /// [getDelivery] is not used. A new URL or key loads a new player.
  final String? url;

  /// The app key that reads [url], when the URL does not carry it as
  /// `?key=`. It can only read its mascot's released canvases.
  final String? appKey;

  /// A release delivery: the delivery `Map`, its JSON `String`, or the API
  /// response around it (`{data: {delivery}}` from
  /// `GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery`). A new
  /// release loads a new player; new links for the same release keep it
  /// playing.
  final Object? delivery;

  /// One animation's files, played on its own in place of a [delivery]: the
  /// HEVC file on iOS, the stacked MP4 on Android.
  final MaskoClipFiles? clip;

  /// Takes the calls the app makes: `set`, `fire` and the others.
  final MaskoController? controller;

  /// Returns a fresh delivery of the same release, before signed links
  /// expire. Giving or taking it away loads the release again, which
  /// restarts the mascot. Only for [delivery]: a [url] renews its own links.
  final Future<Object?> Function()? getDelivery;

  /// Lets clips with their own voice be heard. On by default; false mutes
  /// them. Edge sounds play either way.
  final bool sound;

  /// Starts at once. Defaults to the canvas's `autoPlay`, else true. Read
  /// when a release loads.
  final bool? autoPlay;

  /// The mascot entered a pose.
  final ValueChanged<String>? onPose;

  /// A clip, or a pose's still, is now on screen.
  final ValueChanged<MaskoClipEvent>? onClip;

  /// A fire is done: used by a start, or spent unused.
  final ValueChanged<MaskoReactionDoneEvent>? onReactionDone;

  /// A move with `movement` is now on screen: the app may move the mascot.
  final ValueChanged<MaskoMovementEvent>? onMovement;

  /// Something went wrong, with a code from the rulebook or the player.
  final ValueChanged<MaskoErrorEvent>? onError;

  @override
  State<MaskoView> createState() => _MaskoViewState();
}

class _MaskoViewState extends State<MaskoView> {
  static int _lastChannel = 0;

  /// The view's own channel, named here so its events have a listener
  /// before the native view exists.
  late final int _channelId = ++_lastChannel;
  late final MethodChannel _channel = MethodChannel('ai.masko.player/view_$_channelId');

  bool _created = false;

  /// Calls made after the view was built and before the native view exists.
  final List<_Call> _pending = [];

  late Map<String, Object?> _creationParams;

  /// What the player plays: the release (links left out), `clip:` and the
  /// file, or `url:` with the URL and the key.
  late String _key;
  late bool _refreshes;

  /// The delivery's JSON the player was loaded with.
  late String _json;

  /// The last delivery given, to tell new links of the same release.
  late ({String key, String json}) _given;

  /// The props the native view has.
  late Map<String, Object?> _props;

  static bool get _supported =>
      defaultTargetPlatform == TargetPlatform.iOS ||
      defaultTargetPlatform == TargetPlatform.android;

  /// A lone clip's file for this platform, '' when the clip has none.
  String? get _clipFile {
    final clip = widget.clip;
    if (clip == null) return null;
    return (defaultTargetPlatform == TargetPlatform.iOS ? clip.hevc : clip.stacked) ?? '';
  }

  String get _freshJson => widget.delivery == null ? '' : deliveryJson(widget.delivery!);

  /// What the widget names, as [_key] tells it.
  String _sourceKey(String? clip) {
    final url = widget.url;
    if (url != null) return 'url:$url ${widget.appKey ?? ''}';
    if (clip != null) return 'clip:$clip';
    return releaseKey(widget.delivery!);
  }

  /// A [MaskoView.url] renews its own links, so getDelivery is left out.
  bool get _wantsRefresh => widget.url == null && widget.getDelivery != null;

  Map<String, Object?> _currentProps() {
    final clip = _clipFile;
    return {
      'url': widget.url,
      'appKey': widget.url == null ? null : widget.appKey,
      'delivery': _json,
      'clip': clip == null || clip.isEmpty ? null : clip,
      'autoPlay': widget.autoPlay,
      'sound': widget.sound,
      'refreshes': _refreshes,
    };
  }

  @override
  void initState() {
    super.initState();
    _channel.setMethodCallHandler(_handle);
    final clip = _clipFile;
    _key = _sourceKey(clip);
    _refreshes = _wantsRefresh;
    _json = _freshJson;
    _given = (key: _key, json: _json);
    _props = _currentProps();
    final controller = widget.controller;
    controller?._view = this;
    _creationParams = {
      'channel': _channelId,
      ..._props,
      'calls': [
        for (final call in controller?._take() ?? const <_Call>[])
          {'method': call.method, 'arguments': call.arguments},
      ],
    };
    if (!_supported) {
      _report(MaskoErrorEvent(
        code: 'unsupported_platform',
        detail: 'masko_player plays on iOS and Android, not on ${defaultTargetPlatform.name}',
      ));
    } else if (clip == '') {
      _reportMissingClip();
    }
  }

  @override
  void didUpdateWidget(MaskoView old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      if (old.controller?._view == this) old.controller!._view = null;
      final controller = widget.controller;
      if (controller != null) {
        controller._view = this;
        for (final call in controller._take()) {
          _call(call.method, call.arguments);
        }
      }
    }

    // A new release, URL or key loads a new player; new links for the same
    // release keep it. getDelivery given or taken away loads the release
    // again (the mascot restarts), with the latest links.
    final clip = _clipFile;
    final key = _sourceKey(clip);
    final refreshes = _wantsRefresh;
    final fresh = _freshJson;
    if (key != _key || refreshes != _refreshes) {
      _key = key;
      _refreshes = refreshes;
      _json = fresh;
      if (clip == '') _reportMissingClip();
    }

    // New links for the same release: the native player swaps them in and
    // keeps playing.
    final last = _given;
    _given = (key: key, json: fresh);
    if (widget.delivery != null && last.key == key && last.json != fresh) {
      _call('updateDelivery', {'delivery': fresh});
    }

    final props = _currentProps();
    if (!mapEquals(props, _props)) {
      _props = props;
      _call('update', props);
    }
  }

  @override
  void dispose() {
    if (widget.controller?._view == this) widget.controller!._view = null;
    _channel.setMethodCallHandler(null);
    super.dispose();
  }

  void _call(String method, Map<String, Object?> arguments) {
    if (!_created) {
      _pending.add((method: method, arguments: arguments));
      return;
    }
    // The calls return nothing, as on the other platforms; a call to a view
    // that is gone does nothing.
    unawaited(_channel.invokeMethod<void>(method, arguments).catchError((Object _) {}));
  }

  void _onCreated(int _) {
    if (!mounted) return;
    _created = true;
    final calls = List.of(_pending);
    _pending.clear();
    for (final call in calls) {
      _call(call.method, call.arguments);
    }
  }

  Future<MaskoPosition> _position() async {
    if (!_created) return MaskoPosition.nothing;
    try {
      final at = await _channel.invokeMapMethod<String, Object?>('position');
      return MaskoPosition(
        clip: at?['clip'] as String?,
        time: (at?['time'] as num?)?.toDouble() ?? 0,
      );
    } catch (_) {
      return MaskoPosition.nothing;
    }
  }

  /// Reports an error after this frame, so the app may set state in it.
  void _report(MaskoErrorEvent error) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) widget.onError?.call(error);
    });
  }

  void _reportMissingClip() {
    final ios = defaultTargetPlatform == TargetPlatform.iOS;
    _report(MaskoErrorEvent(
      code: 'clip_failed',
      detail: 'the clip has no ${ios ? 'hevc' : 'stacked'} file for ${ios ? 'ios' : 'android'}',
    ));
  }

  Future<Object?> _handle(MethodCall call) async {
    final raw = call.arguments;
    final args = raw is Map ? raw.cast<String, Object?>() : const <String, Object?>{};
    switch (call.method) {
      case 'pose':
        widget.onPose?.call(args['pose']! as String);
      case 'clip':
        widget.onClip?.call(MaskoClipEvent(
          clip: args['clip']! as String,
          kind: MaskoClipKind.values.byName(args['kind']! as String),
          rate: (args['rate']! as num).toDouble(),
        ));
      case 'reactionDone':
        widget.onReactionDone?.call(MaskoReactionDoneEvent(
          name: args['name']! as String,
          used: args['used']! as bool,
        ));
      case 'movement':
        widget.onMovement?.call(MaskoMovementEvent(
          edge: args['edge']! as String,
          movement: _plain(args['movement']),
          rate: (args['rate']! as num).toDouble(),
          seconds: (args['seconds']! as num).toDouble(),
        ));
      case 'error':
        widget.onError?.call(MaskoErrorEvent(
          code: args['code']! as String,
          detail: args['detail'] as String?,
        ));
      case 'getDelivery':
        // The native player asks before its links expire; null when there is
        // no fresh delivery, and it reports refresh_failed.
        final get = widget.getDelivery;
        if (get == null) return null;
        try {
          final answer = await get();
          return answer == null ? null : deliveryJson(answer);
        } catch (_) {
          return null;
        }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    // The player watches no touch (rules.md 3.5): the mascot takes hits like
    // a picture, so the app's own GestureDetector around it gets them, and
    // the native view never joins the gesture arena.
    switch (defaultTargetPlatform) {
      case TargetPlatform.iOS:
        return AbsorbPointer(
          child: UiKitView(
            viewType: _viewType,
            creationParams: _creationParams,
            creationParamsCodec: const StandardMessageCodec(),
            onPlatformViewCreated: _onCreated,
          ),
        );
      case TargetPlatform.android:
        return AbsorbPointer(
          child: AndroidView(
            viewType: _viewType,
            creationParams: _creationParams,
            creationParamsCodec: const StandardMessageCodec(),
            onPlatformViewCreated: _onCreated,
          ),
        );
      default:
        return const SizedBox.expand();
    }
  }
}

/// A decoded JSON value from the platform channel, with string keys.
Object? _plain(Object? value) {
  if (value is Map) return value.map((key, v) => MapEntry('$key', _plain(v)));
  if (value is List) return value.map(_plain).toList();
  return value;
}
