package ai.masko.player

import android.app.Activity
import android.graphics.Bitmap
import android.graphics.Color
import android.os.Bundle
import android.os.SystemClock
import android.widget.FrameLayout
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Collections

/** A white window to put the player in. */
class TestActivity : Activity() {
    lateinit var root: FrameLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        root = FrameLayout(this).apply { setBackgroundColor(Color.WHITE) }
        setContentView(root)
    }
}

/**
 * Real transparent clips on the real decoder (spec/player.md, "Tests every
 * platform runs", 3): stacked MP4s from assets/clips (make-clips.sh) play
 * through Media3, the GL shader and the two TextureView slots.
 */
@RunWith(AndroidJUnit4::class)
class RealVideoTest {
    private val log = Collections.synchronizedList(ArrayList<Pair<Long, String>>())

    private fun record(event: String) {
        log += SystemClock.elapsedRealtime() to event
    }

    private fun events(): List<String> = synchronized(log) { log.map { it.second } }

    private fun timeOf(event: String, nth: Int = 0): Long =
        synchronized(log) { log.filter { it.second == event }[nth].first }

    private fun await(what: String, timeoutMs: Long = 10_000, done: () -> Boolean) {
        val until = SystemClock.elapsedRealtime() + timeoutMs
        while (!done()) {
            if (SystemClock.elapsedRealtime() > until) fail("timed out waiting for $what; log: ${events()}")
            Thread.sleep(20)
        }
    }

    private fun awaitEvent(event: String, count: Int = 1, timeoutMs: Long = 10_000) =
        await("$event x$count", timeoutMs) { events().count { it == event } >= count }

    @Test
    fun stackedClipsPlayAndSwapOnlyAfterTheirFirstFrame() = playsAndSwaps(DELIVERY, "swap")

    /** The same clips in AV1 (`videos.av1`), on the phone's own AV1 decoder, hardware or software. */
    @Test
    fun av1ClipsPlayAndSwapOnlyAfterTheirFirstFrame() = playsAndSwaps(av1(DELIVERY), "swap-av1")

    /** The player asks the delivery for AV1 only when this phone decodes it in hardware. */
    @Test
    fun thisPhoneAsksForAv1OnlyWithAHardwareDecoder() =
        assertEquals(if (MaskoPlayer.av1Decoder == Av1Decoder.HARDWARE) "av1" else "", MaskoPlayer.formats)

    private fun playsAndSwaps(delivery: String, name: String) {
        ActivityScenario.launch(TestActivity::class.java).use { scenario ->
            lateinit var view: MaskoPlayerView
            scenario.onActivity { activity ->
                view = MaskoPlayerView(activity)
                activity.root.addView(view, FrameLayout.LayoutParams(SIZE, SIZE))
                val player = view.load(delivery)
                player.addListener(object : MaskoPlayer.Listener {
                    override fun onPose(pose: String) = record("pose:$pose")
                    override fun onClip(clip: String, kind: MaskoPlayer.ClipKind, rate: Double) = record("clip:$clip")
                    override fun onMovement(edge: String, movement: org.json.JSONObject, rate: Double, seconds: Double) =
                        record("movement:$edge")
                    override fun onError(code: String, detail: String?) = record("error:$code")
                })
                view.stage!!.slots.forEachIndexed { i, slot ->
                    slot.trace = { event ->
                        // At the swap, read what the slot's own view holds: the clip's first frame.
                        val seen = if (event == "visible") ":" + colourAt(view.videos[i].getBitmap(SIZE, SIZE)!!, SIZE / 4, SIZE / 2) else ""
                        record("slot$i:$event$seen")
                    }
                }
            }

            // 1. The first clip appears, then loops: each loop starts from the real end of the clip.
            awaitEvent("clip:loop-red")
            Thread.sleep(250)
            screen(view).let { (left, right) ->
                assertEquals("opaque half", "red", left)
                assertEquals("transparent half shows the window", "white", right)
            }
            awaitEvent("clip:loop-red", count = 3, timeoutMs = 8_000)
            val loop = timeOf("clip:loop-red", 1) - timeOf("clip:loop-red", 0)
            assertTrue("a 1 s loop restarts on its end, not on the 2 s safety net: $loop ms", loop in 700..1_700)

            // 2. A move: it loads in the hidden slot and swaps in after its first frame.
            scenario.onActivity { view.player!!.set("working", true) }
            awaitEvent("clip:to-green")
            Thread.sleep(150)
            assertEquals("green", screen(view).first)

            // 3. The move's end enters the target pose, whose clip swaps in.
            awaitEvent("pose:working")
            awaitEvent("clip:loop-blue")
            Thread.sleep(250)
            screen(view).let { (left, right) ->
                assertEquals("blue", left)
                assertEquals("white", right)
            }
            // The target's loop runs on at its own pace in the reused slot.
            awaitEvent("clip:loop-blue", count = 3, timeoutMs = 8_000)
            scenario.onActivity { view.player!!.destroy() }
        }

        dump(name)
        val blue = timeOf("clip:loop-blue", 2) - timeOf("clip:loop-blue", 1)
        assertTrue("a 1 s loop in a reused slot restarts on its end: $blue ms", blue in 700..1_700)
        val all = events()
        // No safety net, no failure: every end came from Media3's STATE_ENDED.
        assertTrue("errors: $all", all.none { it.startsWith("error:") })
        // Every swap waited for the slot's first frame: Media3's onRenderedFirstFrame and a drawn frame.
        for (slot in 0..1) {
            var loading = false
            var rendered = false
            var drawn = false
            for (e in all.filter { it.startsWith("slot$slot:") }.map { it.substringAfter(':') }) {
                when {
                    e == "load" -> { loading = true; rendered = false; drawn = false }
                    e == "renderedFirstFrame" -> rendered = true
                    e == "drawn" -> drawn = true
                    e == "firstFrame" -> {
                        assertTrue("slot$slot first frame before it rendered and drew: $all", rendered && drawn)
                        loading = false
                    }
                    e.startsWith("visible") -> assertTrue("slot$slot shown while loading: $all", !loading)
                }
            }
        }
        // What each slot's view held when it was shown: the new clip's own first frame.
        val shown = all.filter { it.contains(":visible:") }
        assertEquals(listOf("slot0:visible:red", "slot1:visible:green", "slot0:visible:blue"), shown)
        // Ends drive the engine: the move's end came before its target pose.
        assertTrue(all.indexOf("slot0:ended") in 0 until all.indexOf("clip:to-green"))
        assertTrue(all.lastIndexOf("slot1:ended") in 0 until all.indexOf("pose:working"))
        assertEquals("one load for the move, done before it was asked for", 1, all.count { it == "slot1:load" })
    }

    @Test
    fun aViewTakenOffScreenPausesAndDrawsAgainWhenItIsBack() {
        ActivityScenario.launch(TestActivity::class.java).use { scenario ->
            lateinit var view: MaskoPlayerView
            scenario.onActivity { activity ->
                view = MaskoPlayerView(activity)
                activity.root.addView(view, FrameLayout.LayoutParams(SIZE, SIZE))
                view.load(DELIVERY).addListener(object : MaskoPlayer.Listener {
                    override fun onClip(clip: String, kind: MaskoPlayer.ClipKind, rate: Double) = record("clip:$clip")
                    override fun onError(code: String, detail: String?) = record("error:$code")
                })
                view.stage!!.slots.forEachIndexed { i, slot -> slot.trace = { record("slot$i:$it") } }
            }
            awaitEvent("clip:loop-red")
            Thread.sleep(400)
            var paused = false
            var position = 0.0
            var decoders = -1
            scenario.onActivity { activity ->
                activity.root.removeView(view)
                paused = view.player!!.isPaused
                position = view.player!!.position().time
                decoders = view.stage!!.slots.count { it.hasDecoder }
            }
            assertTrue("off screen, the player pauses", paused)
            assertEquals("detached, the decoders are freed", 0, decoders)
            // Off screen for longer than the clip and its safety net: nothing may happen meanwhile.
            Thread.sleep(2_500)
            var later = 0.0
            scenario.onActivity { later = view.player!!.position().time }
            assertEquals("the clip stays where it stopped", position, later, 0.05)
            assertEquals("nothing played off screen", 1, events().count { it == "clip:loop-red" })
            scenario.onActivity { activity -> activity.root.addView(view, FrameLayout.LayoutParams(SIZE, SIZE)) }
            // Back on screen: new surfaces and a new decoder, the same clip drawn again from where it stopped.
            Thread.sleep(600)
            assertEquals("red", screen(view).first)
            assertTrue("the decoder is made again: ${events()}", events().contains("slot0:restored"))
            // And it plays on: the loop ends and starts again.
            val loops = events().count { it == "clip:loop-red" }
            awaitEvent("clip:loop-red", count = loops + 1, timeoutMs = 4_000)
            scenario.onActivity { view.player!!.destroy() }
        }
        dump("offscreen")
        assertTrue("errors: ${events()}", events().none { it.startsWith("error:") })
    }

    @Test
    fun transparencyIsTheLumaOfTheLowerHalf() = tintedTransparency(TINT)

    @Test
    fun av1TransparencyIsTheLumaOfTheLowerHalf() = tintedTransparency(av1(TINT))

    private fun tintedTransparency(delivery: String) {
        ActivityScenario.launch(TestActivity::class.java).use { scenario ->
            lateinit var view: MaskoPlayerView
            scenario.onActivity { activity ->
                view = MaskoPlayerView(activity)
                activity.root.addView(view, FrameLayout.LayoutParams(SIZE, SIZE))
                view.load(delivery).addListener(object : MaskoPlayer.Listener {
                    override fun onClip(clip: String, kind: MaskoPlayer.ClipKind, rate: Double) = record("clip:$clip")
                    override fun onError(code: String, detail: String?) = record("error:$code")
                })
            }
            awaitEvent("clip:loop-tint")
            Thread.sleep(250)
            // The left half's transparency is pure red: luma about 0.2, so the blue is faint over white.
            val shot = InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
                .copy(Bitmap.Config.ARGB_8888, false)
            val at = IntArray(2)
            InstrumentationRegistry.getInstrumentation().runOnMainSync { view.getLocationOnScreen(at) }
            val c = shot.getPixel(at[0] + SIZE / 4, at[1] + SIZE / 2)
            val seen = "#%06x".format(c and 0xffffff)
            assertTrue("a faint blue, not opaque blue: $seen", Color.red(c) in 150..235 && Color.green(c) in 150..235 && Color.blue(c) > 230)
            assertEquals("white", colourAt(shot, at[0] + 3 * SIZE / 4, at[1] + SIZE / 2))
            scenario.onActivity { view.player!!.destroy() }
        }
        assertFalse("errors: ${events()}", events().any { it.startsWith("error:") })
    }

    private fun dump(name: String) {
        val start = synchronized(log) { log.firstOrNull()?.first ?: 0L }
        val lines = synchronized(log) { log.map { (t, e) -> "${t - start} $e" } }
        android.util.Log.i("RealVideoTest", "$name: " + lines.joinToString(" | "))
    }

    /** The colours at the mascot's opaque (left) and transparent (right) halves, on the real screen. */
    private fun screen(view: MaskoPlayerView): Pair<String, String> {
        val shot = InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
            .copy(Bitmap.Config.ARGB_8888, false)
        val at = IntArray(2)
        InstrumentationRegistry.getInstrumentation().runOnMainSync { view.getLocationOnScreen(at) }
        return colourAt(shot, at[0] + SIZE / 4, at[1] + SIZE / 2) to colourAt(shot, at[0] + 3 * SIZE / 4, at[1] + SIZE / 2)
    }

    internal companion object {
        const val SIZE = 400

        /** The same delivery with each clip's AV1 file in `videos.av1` (make-clips.sh). */
        fun av1(delivery: String) = delivery.replace(Regex("\"stacked\": \"(asset:///clips/[a-z]+)\\.mp4\"")) {
            "\"av1\": \"${it.groupValues[1]}-av1.mp4\""
        }

        fun colourAt(bitmap: Bitmap, x: Int, y: Int): String {
            val c = bitmap.getPixel(x, y)
            val (a, r, g, b) = listOf(Color.alpha(c), Color.red(c), Color.green(c), Color.blue(c))
            return when {
                a < 40 -> "transparent"
                r > 200 && g > 200 && b > 200 -> "white"
                r > 170 && g < 90 && b < 90 -> "red"
                g > 170 && r < 90 && b < 90 -> "green"
                b > 170 && r < 90 && g < 90 -> "blue"
                else -> "#%02x%02x%02x%02x".format(a, r, g, b)
            }
        }

        val DELIVERY = """
        {
          "deliverySchemaVersion": 1,
          "playerContract": 1,
          "requires": [],
          "runtimeGraph": {
            "runtimeGraphSchemaVersion": 1,
            "initialNode": "idle",
            "autoPlay": true,
            "nodes": [{ "id": "idle", "name": "Idle" }, { "id": "working", "name": "Working" }],
            "inputs": [{ "name": "behavior::working", "type": "boolean", "default": false }],
            "edges": [
              { "id": "loop-red", "source": "idle", "target": "idle", "isLoop": true, "duration": 1.0,
                "videos": { "stacked": "asset:///clips/red.mp4" } },
              { "id": "to-green", "source": "idle", "target": "working", "isLoop": false, "duration": 0.6,
                "conditions": [{ "input": "behavior::working", "op": "==", "value": true }],
                "videos": { "stacked": "asset:///clips/green.mp4" }, "movement": { "x": 1 } },
              { "id": "loop-blue", "source": "working", "target": "working", "isLoop": true, "duration": 1.0,
                "videos": { "stacked": "asset:///clips/blue.mp4" } }
            ]
          }
        }
        """.trimIndent()

        val TINT = """
        {
          "deliverySchemaVersion": 1,
          "playerContract": 1,
          "requires": [],
          "runtimeGraph": {
            "runtimeGraphSchemaVersion": 1,
            "initialNode": "idle",
            "nodes": [{ "id": "idle", "name": "Idle" }],
            "edges": [
              { "id": "loop-tint", "source": "idle", "target": "idle", "isLoop": true, "duration": 1.0,
                "videos": { "stacked": "asset:///clips/tint.mp4" } }
            ]
          }
        }
        """.trimIndent()
    }
}
