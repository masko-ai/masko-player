package ai.masko.player.reactnative

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

/**
 * The `MaskoPlayer` view for React Native. Each call goes to the
 * [ai.masko.player.MaskoPlayer] of the view, on the main thread, in the order
 * JavaScript made them.
 */
class MaskoPlayerModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("MaskoPlayer")

    // The formats to ask the release delivery for (formats=...).
    Function("formats") { ai.masko.player.MaskoPlayer.formats }

    View(MaskoNativeView::class) {
      Events("onPose", "onClip", "onReactionDone", "onMovement", "onError", "onDeliveryRequest")

      Prop("delivery") { view: MaskoNativeView, json: String? -> view.delivery = json }
      Prop("clip") { view: MaskoNativeView, url: String? -> view.clip = url }
      Prop("url") { view: MaskoNativeView, url: String? -> view.url = url }
      Prop("appKey") { view: MaskoNativeView, key: String? -> view.appKey = key }
      Prop("autoPlay") { view: MaskoNativeView, on: Boolean? -> view.autoPlay = on }
      Prop("sound") { view: MaskoNativeView, on: Boolean? -> view.sound = on ?: true }
      Prop("refreshes") { view: MaskoNativeView, on: Boolean? -> view.refreshes = on ?: false }

      OnViewDidUpdateProps { view: MaskoNativeView -> view.propsChanged() }
      OnViewDestroys { view: MaskoNativeView -> view.unload() }

      AsyncFunction("setBool") { view: MaskoNativeView, name: String, value: Boolean, instant: Boolean ->
        view.player?.set(name, value, instant)
      }
      AsyncFunction("setNumber") { view: MaskoNativeView, name: String, value: Double, instant: Boolean ->
        view.player?.set(name, value, instant)
      }
      AsyncFunction("fire") { view: MaskoNativeView, name: String, instant: Boolean -> view.player?.fire(name, instant) }
      AsyncFunction("reset") { view: MaskoNativeView -> view.player?.reset() }
      AsyncFunction("play") { view: MaskoNativeView -> view.player?.play() }
      AsyncFunction("pause") { view: MaskoNativeView -> view.player?.pause() }
      AsyncFunction("position") { view: MaskoNativeView -> view.position() }
      AsyncFunction("showPose") { view: MaskoNativeView, id: String -> view.player?.preview?.showPose(id) }
      AsyncFunction("playMove") { view: MaskoNativeView, id: String -> view.player?.preview?.playMove(id) }
      AsyncFunction("updateDelivery") { view: MaskoNativeView, json: String -> view.updateDelivery(json) }
      AsyncFunction("answerDelivery") { view: MaskoNativeView, request: Int, json: String? ->
        view.answerDelivery(request, json)
      }
    }
  }
}
