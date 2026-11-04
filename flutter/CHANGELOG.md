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
