import ExpoModulesCore

/// The `MaskoPlayer` view for React Native. Each call goes to the
/// `MaskoPlayer` of the view, on the main thread, in the order JavaScript made
/// them.
public final class MaskoPlayerModule: Module {
  public func definition() -> ModuleDefinition {
    Name("MaskoPlayer")

    // The formats to ask the release delivery for (formats=...).
    Function("formats") { MaskoPlayer.formats }

    View(MaskoNativeView.self) {
      Events("onPose", "onClip", "onReactionDone", "onMovement", "onError", "onDeliveryRequest")

      Prop("delivery") { (view: MaskoNativeView, json: String?) in
        MainActor.assumeIsolated { view.delivery = json }
      }
      Prop("clip") { (view: MaskoNativeView, url: String?) in
        MainActor.assumeIsolated { view.clip = url }
      }
      Prop("url") { (view: MaskoNativeView, url: String?) in
        MainActor.assumeIsolated { view.url = url }
      }
      Prop("appKey") { (view: MaskoNativeView, key: String?) in
        MainActor.assumeIsolated { view.appKey = key }
      }
      Prop("autoPlay") { (view: MaskoNativeView, on: Bool?) in
        MainActor.assumeIsolated { view.autoPlay = on }
      }
      Prop("sound") { (view: MaskoNativeView, on: Bool?) in
        MainActor.assumeIsolated { view.sound = on ?? true }
      }
      Prop("refreshes") { (view: MaskoNativeView, on: Bool?) in
        MainActor.assumeIsolated { view.refreshes = on ?? false }
      }

      OnViewDidUpdateProps { (view: MaskoNativeView) in
        MainActor.assumeIsolated { view.propsChanged() }
      }

      AsyncFunction("setBool") { (view: MaskoNativeView, name: String, value: Bool, instant: Bool) in
        MainActor.assumeIsolated { view.player?.set(name, value, instant: instant) }
      }.runOnQueue(.main)

      AsyncFunction("setNumber") { (view: MaskoNativeView, name: String, value: Double, instant: Bool) in
        MainActor.assumeIsolated { view.player?.set(name, value, instant: instant) }
      }.runOnQueue(.main)

      AsyncFunction("fire") { (view: MaskoNativeView, name: String, instant: Bool) in
        MainActor.assumeIsolated { view.player?.fire(name, instant: instant) }
      }.runOnQueue(.main)

      AsyncFunction("reset") { (view: MaskoNativeView) in
        MainActor.assumeIsolated { view.player?.reset() }
      }.runOnQueue(.main)

      AsyncFunction("play") { (view: MaskoNativeView) in
        MainActor.assumeIsolated { view.player?.play() }
      }.runOnQueue(.main)

      AsyncFunction("pause") { (view: MaskoNativeView) in
        MainActor.assumeIsolated { view.player?.pause() }
      }.runOnQueue(.main)

      AsyncFunction("position") { (view: MaskoNativeView) -> [String: Any] in
        MainActor.assumeIsolated { view.position() }
      }.runOnQueue(.main)

      AsyncFunction("showPose") { (view: MaskoNativeView, id: String) in
        MainActor.assumeIsolated { view.player?.preview.showPose(id) }
      }.runOnQueue(.main)

      AsyncFunction("playMove") { (view: MaskoNativeView, id: String) in
        MainActor.assumeIsolated { view.player?.preview.playMove(id) }
      }.runOnQueue(.main)

      AsyncFunction("updateDelivery") { (view: MaskoNativeView, json: String) in
        MainActor.assumeIsolated { view.updateDelivery(json) }
      }.runOnQueue(.main)

      AsyncFunction("answerDelivery") { (view: MaskoNativeView, request: Int, json: String?) in
        MainActor.assumeIsolated { view.answerDelivery(request, json: json) }
      }.runOnQueue(.main)
    }
  }
}
