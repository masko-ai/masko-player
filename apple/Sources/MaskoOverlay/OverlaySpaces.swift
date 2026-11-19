// Keeps windows still while you swipe between desktops (Spaces), like the menu bar and the
// Dock. From Masko for Mac, adapted from SkyLightWindow by Lakr Aream:
// https://github.com/Lakr233/SkyLightWindow
//
// MIT License
//
// Copyright (c) 2025 Lakr Aream
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

#if os(macOS)
import AppKit

/// macOS's private SkyLight framework makes a layer above the desktops, one per display, and
/// a window moved into it takes no part in the swipe. Each display needs its own layer: a
/// window in one display's layer could not be dragged to another display.
@MainActor final class OverlaySpaces {
    /// Nil when this macOS lacks the framework: windows then only slide with the swipe.
    static let shared = OverlaySpaces()

    private typealias MainConnectionID = @convention(c) () -> Int32
    private typealias SpaceCreate = @convention(c) (Int32, Int32, Int32) -> Int32
    private typealias SpaceSetAbsoluteLevel = @convention(c) (Int32, Int32, Int32) -> Int32
    private typealias ShowSpaces = @convention(c) (Int32, CFArray) -> Int32
    private typealias SpaceAddWindowsAndRemoveFromSpaces = @convention(c) (
        Int32, Int32, CFArray, Int32
    ) -> Int32

    private let spaceCreate: SpaceCreate
    private let spaceSetAbsoluteLevel: SpaceSetAbsoluteLevel
    private let showSpaces: ShowSpaces
    private let addWindows: SpaceAddWindowsAndRemoveFromSpaces
    private let connection: Int32
    private var spaces: [CGDirectDisplayID: Int32] = [:]
    private var windows: [ObjectIdentifier: (window: Weak, display: CGDirectDisplayID)] = [:]

    private final class Weak {
        weak var window: NSWindow?
        init(_ window: NSWindow) { self.window = window }
    }

    private init?() {
        guard
            let framework = dlopen(
                "/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight",
                RTLD_NOW),
            let main = dlsym(framework, "SLSMainConnectionID"),
            let create = dlsym(framework, "SLSSpaceCreate"),
            let level = dlsym(framework, "SLSSpaceSetAbsoluteLevel"),
            let show = dlsym(framework, "SLSShowSpaces"),
            let add = dlsym(framework, "SLSSpaceAddWindowsAndRemoveFromSpaces")
        else { return nil }
        spaceCreate = unsafeBitCast(create, to: SpaceCreate.self)
        spaceSetAbsoluteLevel = unsafeBitCast(level, to: SpaceSetAbsoluteLevel.self)
        showSpaces = unsafeBitCast(show, to: ShowSpaces.self)
        addWindows = unsafeBitCast(add, to: SpaceAddWindowsAndRemoveFromSpaces.self)
        connection = unsafeBitCast(main, to: MainConnectionID.self)()
        // A display went away: forget its layer and the windows that were on it.
        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.forgetGoneDisplays() }
        }
    }

    /// Moves `window` into the layer of `screen` (else the window's own screen, else the main
    /// one). A window already there is left alone.
    func keepStill(_ window: NSWindow, on screen: NSScreen? = nil) {
        guard window.windowNumber > 0,
            let target = screen ?? window.screen ?? NSScreen.main,
            let display = Self.display(of: target)
        else { return }
        windows = windows.filter { $0.value.window.window != nil }
        let id = ObjectIdentifier(window)
        if let kept = windows[id], kept.window.window === window, kept.display == display {
            return
        }
        guard let space = space(for: display) else { return }
        _ = addWindows(connection, space, [window.windowNumber] as CFArray, 7)
        windows[id] = (Weak(window), display)
    }

    private func space(for display: CGDirectDisplayID) -> Int32? {
        if let space = spaces[display] { return space }
        let space = spaceCreate(connection, 1, 0)
        guard space > 0 else { return nil }
        _ = spaceSetAbsoluteLevel(connection, space, 100)
        _ = showSpaces(connection, [space] as CFArray)
        spaces[display] = space
        return space
    }

    private static func display(of screen: NSScreen) -> CGDirectDisplayID? {
        screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? CGDirectDisplayID
    }

    private func forgetGoneDisplays() {
        let present = Set(NSScreen.screens.compactMap(Self.display))
        spaces = spaces.filter { present.contains($0.key) }
        windows = windows.filter { $0.value.window.window != nil && present.contains($0.value.display) }
    }
}
#endif
