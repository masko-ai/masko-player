#if os(macOS)
import AppKit
import MaskoPlayer

/// A window for a mascot that lives on the Mac's desktop, such as a desktop pet: transparent,
/// above other windows and full-screen apps, on every desktop (Space), and never taking the
/// keyboard. It stays still while you swipe between desktops, like the menu bar and the Dock.
///
/// ```swift
/// let overlay = MaskoOverlay(player: player, size: CGSize(width: 200, height: 200))
/// overlay.show()
/// ```
///
/// Staying still during a swipe uses macOS's private SkyLight framework, as Masko for Mac does.
/// Apps downloaded from the web may use it; the Mac App Store does not accept private
/// frameworks, so leave MaskoOverlay out of an App Store app. When a macOS version lacks the
/// framework, the window still shows on every desktop and only slides with the swipe
/// (`MaskoOverlay.staysStillOnSwipe` is then false).
///
/// What the mascot does (walking, dragging, a menu) stays in the app: it moves the window with
/// `setFrameOrigin`, may change its `level`, or subclasses it. The window may sit partly off
/// screen, and comes back to a corner when its display goes away.
@MainActor open class MaskoOverlay: NSPanel {
    private var observers: [(NotificationCenter, NSObjectProtocol)] = []

    /// Whether this Mac keeps overlays still while you swipe between desktops.
    public static var staysStillOnSwipe: Bool { OverlaySpaces.shared != nil }

    /// Keeps another window of the app still too, such as a menu or a speech bubble beside
    /// the mascot: on every desktop and out of the swipe. Call it once the window shows.
    public static func keepStill(_ window: NSWindow) {
        window.collectionBehavior.formUnion(collection)
        OverlaySpaces.shared?.keepStill(window)
    }

    private static let collection: NSWindow.CollectionBehavior = [
        .canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle,
        .fullScreenDisallowsTiling,
    ]

    /// A transparent window of `size` points, at `origin` (screen coordinates, from the bottom
    /// left) or, without one, in the bottom right corner of the main screen.
    public init(size: CGSize, origin: CGPoint? = nil) {
        super.init(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        isFloatingPanel = true
        level = .screenSaver
        collectionBehavior = Self.collection
        hidesOnDeactivate = false
        animationBehavior = .none
        if let origin { setFrameOrigin(origin) } else { moveToCorner() }
        // macOS can take the window out of its place when desktops or apps change: put it
        // back in front, still on every desktop.
        let workspace = NSWorkspace.shared.notificationCenter
        for name in [
            NSWorkspace.activeSpaceDidChangeNotification,
            NSWorkspace.didActivateApplicationNotification,
        ] {
            observe(workspace, name, object: nil) { overlay in
                if overlay.isVisible { overlay.orderFrontRegardless() }
            }
        }
        let center = NotificationCenter.default
        // Dragged to another display: it keeps still there too.
        observe(center, NSWindow.didChangeScreenNotification, object: self) { $0.pin() }
        // A display went away or changed: back on a screen if it was left off every one.
        observe(center, NSApplication.didChangeScreenParametersNotification, object: nil) {
            $0.bringBackOnScreen()
        }
    }

    /// A MaskoOverlay that shows `player`'s mascot, filling the window.
    public convenience init(
        player: MaskoPlayer, size: CGSize = CGSize(width: 200, height: 200), origin: CGPoint? = nil
    ) {
        self.init(size: size, origin: origin)
        let view = MaskoPlayerView(player: player)
        view.frame = NSRect(origin: .zero, size: size)
        view.autoresizingMask = [.width, .height]
        contentView = view
    }

    /// Shows the window in front of every other, without taking the keyboard from the app in use.
    public func show() {
        bringBackOnScreen()
        orderFrontRegardless()
    }

    /// Hides the window. `show()` brings it back.
    public func hide() { orderOut(nil) }

    /// Moves the window to the bottom right corner of the main screen when less than 48 points
    /// of it show on any screen, for example after a drop off the edge.
    public func bringBackOnScreen() {
        let shows = NSScreen.screens.contains { screen in
            let shared = frame.intersection(screen.frame)
            return shared.width >= 48 && shared.height >= 48
        }
        if !shows { moveToCorner() }
    }

    open override var canBecomeKey: Bool { false }
    open override var canBecomeMain: Bool { false }

    // A mascot may peek from an edge or sit under the menu bar.
    open override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect {
        frameRect
    }

    open override func orderFrontRegardless() {
        super.orderFrontRegardless()
        pin()
    }

    private func pin() {
        if isVisible { OverlaySpaces.shared?.keepStill(self, on: screen) }
    }

    private func moveToCorner() {
        guard let screen = NSScreen.main else { return }
        setFrameOrigin(
            NSPoint(
                x: screen.visibleFrame.maxX - frame.width - 40,
                y: screen.visibleFrame.minY + 20))
    }

    private func observe(
        _ center: NotificationCenter, _ name: Notification.Name, object: AnyObject?,
        _ run: @escaping @MainActor (MaskoOverlay) -> Void
    ) {
        let token = center.addObserver(forName: name, object: object, queue: .main) {
            [weak self] _ in
            MainActor.assumeIsolated {
                if let self { run(self) }
            }
        }
        observers.append((center, token))
    }

    deinit {
        for (center, token) in observers { center.removeObserver(token) }
    }
}
#endif
