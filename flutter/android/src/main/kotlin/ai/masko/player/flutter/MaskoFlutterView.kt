package ai.masko.player.flutter

import ai.masko.player.MaskoPlayer
import ai.masko.player.MaskoPlayerView
import android.content.Context
import android.view.View
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.platform.PlatformView
import org.json.JSONArray
import org.json.JSONObject

/**
 * Shows one [MaskoPlayer] in a [MaskoPlayerView] for a Flutter `MaskoView`. A
 * new canvas URL or app key, delivery or clip, or `getDelivery` given or taken
 * away, loads a new player; the view going away destroys it. Calls come on the view's own
 * channel, on the main thread, in the order Dart made them; events go back on
 * it.
 *
 * [args] are the widget's props, its channel's number and the calls the app
 * made before the view existed, which reach the player before it starts.
 */
internal class MaskoFlutterView(
  context: Context,
  messenger: BinaryMessenger,
  args: Map<String, Any?>,
) : PlatformView, MethodChannel.MethodCallHandler {
  private val channel = MethodChannel(messenger, "ai.masko.player/view_${(args["channel"] as? Number)?.toInt() ?: 0}")
  private val mascot = MaskoPlayerView(context)

  private var player: MaskoPlayer? = null
  /**
   * A canvas's URL, played in place of [delivery]: the player asks it for the
   * latest release and keeps the files.
   */
  private var url: String? = null
  /** The app key that reads [url], unless the URL carries it as `?key=`. */
  private var appKey: String? = null
  private var delivery: String? = null
  /** A lone clip's stacked file, played in place of [delivery]. */
  private var clip: String? = null
  private var autoPlay: Boolean? = null
  private var sound = true
  private var refreshes = false

  /** What the player was loaded with: the delivery's JSON, `clip:` and the file, or `url:` with the URL and the key. */
  private var loaded: Pair<String, Boolean>? = null

  /** `getDelivery` calls waiting for Dart's answer. */
  private val requests = HashMap<Int, (String?) -> Unit>()
  private var lastRequest = 0

  private val listener = object : MaskoPlayer.Listener {
    override fun onPose(pose: String) = channel.invokeMethod("pose", mapOf("pose" to pose))

    override fun onClip(clip: String, kind: MaskoPlayer.ClipKind, rate: Double) =
      channel.invokeMethod("clip", mapOf("clip" to clip, "kind" to kind.name.lowercase(), "rate" to rate))

    override fun onReactionDone(name: String, used: Boolean) =
      channel.invokeMethod("reactionDone", mapOf("name" to name, "used" to used))

    override fun onMovement(edge: String, movement: JSONObject, rate: Double, seconds: Double) =
      channel.invokeMethod(
        "movement",
        mapOf("edge" to edge, "movement" to plain(movement), "rate" to rate, "seconds" to seconds),
      )

    override fun onError(code: String, detail: String?) =
      channel.invokeMethod("error", if (detail == null) mapOf("code" to code) else mapOf("code" to code, "detail" to detail))
  }

  init {
    channel.setMethodCallHandler(this)
    apply(args)
    @Suppress("UNCHECKED_CAST")
    for (call in args["calls"] as? List<Map<String, Any?>> ?: emptyList()) {
      run(call["method"] as? String ?: "", call["arguments"] as? Map<String, Any?> ?: emptyMap())
    }
  }

  override fun getView(): View = mascot

  override fun dispose() {
    channel.setMethodCallHandler(null)
    unload()
  }

  override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
    if (call.method == "position") {
      val position = player?.position()
      result.success(mapOf("clip" to position?.clip, "time" to (position?.time ?: 0.0)))
      return
    }
    @Suppress("UNCHECKED_CAST")
    if (run(call.method, call.arguments as? Map<String, Any?> ?: emptyMap())) result.success(null) else result.notImplemented()
  }

  /** One call of the app's. False for a call this view does not know. */
  private fun run(method: String, args: Map<String, Any?>): Boolean {
    val name = args["name"] as? String ?: ""
    val instant = args["instant"] as? Boolean ?: false
    when (method) {
      "update" -> apply(args)
      "setBool" -> player?.set(name, args["value"] as? Boolean ?: false, instant)
      "setNumber" -> player?.set(name, (args["value"] as? Number)?.toDouble() ?: 0.0, instant)
      "fire" -> player?.fire(name, instant)
      "reset" -> player?.reset()
      "play" -> player?.play()
      "pause" -> player?.pause()
      "showPose" -> player?.preview?.showPose(args["id"] as? String ?: "")
      "playMove" -> player?.preview?.playMove(args["id"] as? String ?: "")
      // New links for the release that is playing: the player swaps them in.
      "updateDelivery" -> player?.updateDelivery(args["delivery"] as? String ?: "")
      else -> return false
    }
    return true
  }

  /** The widget's props: loads what they name when it changed. */
  private fun apply(props: Map<String, Any?>) {
    url = props["url"] as? String
    appKey = props["appKey"] as? String
    delivery = props["delivery"] as? String
    clip = props["clip"] as? String
    autoPlay = props["autoPlay"] as? Boolean
    sound = props["sound"] as? Boolean ?: true
    refreshes = props["refreshes"] as? Boolean ?: false
    val link = url?.takeIf { it.isNotEmpty() }
    val file = clip?.takeIf { it.isNotEmpty() }
    if (link != null) {
      val source = "url:$link ${appKey ?: ""}"
      if (loaded != source to refreshes) loadCanvas(link, source)
    } else if (file != null) {
      if (loaded != "clip:$file" to refreshes) loadClip(file)
    } else {
      val json = delivery?.takeIf { it.isNotEmpty() } ?: return unload()
      if (loaded != json to refreshes) load(json)
    }
    player?.sound = sound
  }

  private fun load(json: String) {
    unload()
    val source = if (refreshes) MaskoPlayer.DeliverySource(::requestDelivery) else null
    val next = mascot.load(json, MaskoPlayer.Options(autoPlay = autoPlay, sound = sound, getDelivery = source))
    next.addListener(listener)
    player = next
    loaded = json to refreshes
  }

  /** Plays a canvas from its URL; the player asks for its links itself, so `getDelivery` is not used. */
  private fun loadCanvas(link: String, source: String) {
    unload()
    val key = appKey?.takeIf { it.isNotEmpty() }
    val next = mascot.loadCanvas(link, key, MaskoPlayer.Options(autoPlay = autoPlay, sound = sound))
    next.addListener(listener)
    player = next
    loaded = source to refreshes
  }

  private fun loadClip(file: String) {
    unload()
    val next = mascot.loadClip(file, MaskoPlayer.Options(autoPlay = autoPlay, sound = sound))
    next.addListener(listener)
    player = next
    loaded = "clip:$file" to refreshes
  }

  private fun unload() {
    answerAll()
    player?.destroy()
    player = null
    loaded = null
  }

  /** Asks Dart's `getDelivery` (the player asks on the main thread); its answer replies. */
  private fun requestDelivery(reply: (String?) -> Unit) {
    val request = ++lastRequest
    requests[request] = reply
    channel.invokeMethod(
      "getDelivery",
      null,
      object : MethodChannel.Result {
        override fun success(result: Any?) = answer(request, result as? String)

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) = answer(request, null)

        override fun notImplemented() = answer(request, null)
      },
    )
  }

  private fun answer(request: Int, json: String?) {
    requests.remove(request)?.invoke(json)
  }

  /** Ends every `getDelivery` call still waiting for Dart. */
  private fun answerAll() {
    val waiting = requests.values.toList()
    requests.clear()
    waiting.forEach { it(null) }
  }
}

/** A JSON value as maps, lists and plain values, for Dart. */
private fun plain(value: Any?): Any? = when (value) {
  is JSONObject -> value.keys().asSequence().associateWith { plain(value.opt(it)) }
  is JSONArray -> (0 until value.length()).map { plain(value.opt(it)) }
  JSONObject.NULL -> null
  else -> value
}
