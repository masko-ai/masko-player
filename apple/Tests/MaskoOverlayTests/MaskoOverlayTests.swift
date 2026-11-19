#if os(macOS)
import AppKit
import MaskoPlayer
import XCTest

@testable import MaskoOverlay

/// The overlay is the desktop mascot's window: see-through, above everything, on every
/// desktop, never the keyboard's, and never lost off screen.
@MainActor
final class MaskoOverlayTests: XCTestCase {
    func testIsATransparentWindowAboveEverythingOnEveryDesktop() {
        let overlay = MaskoOverlay(size: CGSize(width: 180, height: 160))
        XCTAssertEqual(overlay.frame.size, CGSize(width: 180, height: 160))
        XCTAssertFalse(overlay.isOpaque)
        XCTAssertEqual(overlay.backgroundColor, .clear)
        XCTAssertFalse(overlay.hasShadow)
        XCTAssertEqual(overlay.level, .screenSaver)
        XCTAssertFalse(overlay.hidesOnDeactivate)
        XCTAssertTrue(overlay.styleMask.contains(.nonactivatingPanel))
        for behavior: NSWindow.CollectionBehavior in [
            .canJoinAllSpaces, .stationary, .fullScreenAuxiliary, .ignoresCycle,
        ] {
            XCTAssertTrue(overlay.collectionBehavior.contains(behavior))
        }
    }

    func testNeverTakesTheKeyboard() {
        let overlay = MaskoOverlay(size: CGSize(width: 100, height: 100))
        XCTAssertFalse(overlay.canBecomeKey)
        XCTAssertFalse(overlay.canBecomeMain)
    }

    func testStartsWhereAskedAndMayPeekFromAnEdge() {
        let overlay = MaskoOverlay(
            size: CGSize(width: 100, height: 100), origin: CGPoint(x: 30, y: 40))
        XCTAssertEqual(overlay.frame.origin, CGPoint(x: 30, y: 40))
        let peeking = NSRect(x: -60, y: 10, width: 100, height: 100)
        XCTAssertEqual(overlay.constrainFrameRect(peeking, to: NSScreen.main), peeking)
    }

    func testComesBackToACornerWhenOffEveryScreen() throws {
        guard let screen = NSScreen.main else { throw XCTSkip("needs a display") }
        let overlay = MaskoOverlay(
            size: CGSize(width: 100, height: 100), origin: CGPoint(x: -100_000, y: -100_000))
        overlay.bringBackOnScreen()
        XCTAssertTrue(screen.visibleFrame.contains(overlay.frame))

        let corner = overlay.frame.origin
        overlay.setFrameOrigin(NSPoint(x: corner.x - 10, y: corner.y))
        overlay.bringBackOnScreen()
        XCTAssertEqual(overlay.frame.origin.x, corner.x - 10, "left alone while it shows")
    }

    func testShowsThePlayersMascot() {
        let player = MaskoPlayer(clip: URL(fileURLWithPath: "/nonexistent/clip.mp4"), autoPlay: false)
        let overlay = MaskoOverlay(player: player, size: CGSize(width: 120, height: 120))
        let view = overlay.contentView as? MaskoPlayerView
        XCTAssertNotNil(view)
        XCTAssertEqual(view?.frame.size, CGSize(width: 120, height: 120))
    }

    func testCanBeSubclassed() {
        final class Pet: MaskoOverlay {
            var shown = 0
            override func orderFrontRegardless() {
                shown += 1
                super.orderFrontRegardless()
            }
        }
        let pet = Pet(size: CGSize(width: 80, height: 80))
        pet.show()
        XCTAssertEqual(pet.shown, 1)
        XCTAssertTrue(pet.isVisible)
        pet.hide()
        XCTAssertFalse(pet.isVisible)
    }

    func testKeepsAnotherWindowOnEveryDesktop() {
        let bubble = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 50, height: 50), styleMask: [.borderless],
            backing: .buffered, defer: true)
        MaskoOverlay.keepStill(bubble)
        XCTAssertTrue(bubble.collectionBehavior.contains(.canJoinAllSpaces))
        XCTAssertTrue(bubble.collectionBehavior.contains(.stationary))
    }

    func testThisMacCanKeepItStill() {
        XCTAssertTrue(MaskoOverlay.staysStillOnSwipe, "SkyLight is missing on this macOS")
    }
}
#endif
