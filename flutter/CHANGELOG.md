## 0.1.22

- No change to the Flutter API. A clip that fails to load or play no longer
  leaves the mascot on its still picture: the player loads it again after
  1 s, 2 s, 4 s and longer waits up to 30 s, even while the mascot stays on
  its pose, and plays it again once its file loads. Meanwhile a pose whose
  main loop cannot play plays another of its loops, and the extra roll
  skips loops that cannot play.

## 0.1.21

- No change to the Flutter API or to how mascots play. The web player no
  longer blinks for one frame when it changes clips in Chrome.

## 0.1.20

- No change to the Flutter API or to how mascots play. The Swift package
  adds `MaskoOverlay`, a window for a mascot on the Mac desktop that stays
  still while desktops swipe.

## 0.1.19

- `MaskoView(url: ...)` plays a canvas from its URL, with no backend: the
  native player asks for the latest release, keeps every file for offline
  play, checks again at launch and every hour, and switches to a newer
  release when the mascot is back on its first pose. The app key may sit
  inside the URL (`...delivery?key=masko_app_...`) or go in `appKey:`.
  On Android this is new in the Kotlin player (`loadCanvas`); on iPhone and
  iPad it uses the Swift player's URL mode.

## 0.1.18

- No change to the Flutter API or to how mascots play. On the web and on
  iPhone, iPad and Mac, a canvas URL may now carry its app key
  (`...delivery?key=masko_app_...`): the player then needs no separate key.

## 0.1.17

- No change to the Flutter API or to how mascots play. The engine inside
  every player can now also be built for ESP32 boards, and the Swift player
  gives iPhone, iPad and Mac apps the release's `capabilities` (what the
  mascot on screen can do).

## 0.1.16

- No change to the Flutter API. On iPhone, iPad and Mac, a talking stacked AV1
  clip now waits for its sound to reach the speakers before its picture
  moves, so the voice stays on the lips, also on Bluetooth headphones.

## 0.1.15

- No change to the Flutter API. The players it carries now preload a pose's
  named clips and extras after its moves, and a value or fire made instant
  shows the named clip it holds at once, cutting the loop on screen, as an
  instant move does.

## 0.1.14

- No change to the Flutter player. When a browser blocks autoplay, as an
  iPhone in Low Power Mode does, the web player now starts on the person's
  next tap anywhere on the page, and the clips after it need no second tap.

## 0.1.13

- No change to the Flutter player. The web player can now play a canvas from
  its URL with an app key, as the Swift player does.

## 0.1.12

- No change to the Flutter API. The iOS player it carries can now play a
  canvas from its URL with an app key (`MaskoPlayer(url:appKey:)` in Swift).

## 0.1.11

- No change to the players. The package is now published from the
  repository's release workflow, with the other Masko players, and carries
  the Masko player 0.1.11 for iOS and Android.

## 0.1.10

- First release: `MaskoView` and `MaskoController` play a Masko release or
  one animation on iOS and Android, with the Masko player 0.1.10 for each.
