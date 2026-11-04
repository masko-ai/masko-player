package ai.masko.player

import ai.masko.player.engine.MaskoEngine
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The player against a fake video layer (spec/player.md, "Tests every
 * platform runs", 2), with the real engine through the Kotlin binding.
 */
/** The latest preload of the move `to-work`: entering idle preloads it, then idle's named clip `boop-idle` (rules 9.7). */
private fun Rig.toWorkPreload() = stage.preloads.last { it.first.contains("/to-work.") }

class MaskoPlayerTest {

    // ------------------------------------------------------------------ start and the swap

    @Test
    fun startsOnTheNextTurnAndShowsTheFirstClipOnlyAfterItsFirstFrame() {
        val rig = Rig()
        assertTrue("nothing happens before the next main-thread turn", rig.log.isEmpty())
        rig.turn()
        assertEquals(listOf("pose:idle"), rig.heard.events)
        assertEquals(listOf("https://cdn.test/m/loop-idle-800.mp4?sig=1"), rig.slot0.loads)
        assertFalse(rig.slot0.shown)
        assertFalse(rig.slot0.playing)

        rig.advance(40)
        val mark = rig.log.size
        rig.slot0.firstFrame()
        assertEquals(listOf("pose:idle", "clip:loop-idle"), rig.heard.events)
        assertEquals(listOf("0 show", "0 play 1.0"), rig.since(mark))
        assertTrue(rig.slot0.playing)
        assertTrue(rig.slot1.loads.isEmpty())
    }

    // ------------------------------------------------------------------ one clip

    @Test
    fun aLoneClipLoopsAsOnePoseFromTheLengthOfItsFile() {
        val url = "https://cdn.test/m/wave.mp4"
        val rig = Rig(clip = url, lengths = mapOf(url to 3.0))
        rig.turn()
        assertEquals(listOf(url), rig.slot0.loads)
        assertEquals(listOf("pose:clip"), rig.heard.events)
        rig.slot0.firstFrame()
        assertEquals(listOf("pose:clip", "clip:clip"), rig.heard.events)
        rig.advance(3_000)
        val mark = rig.log.size
        rig.slot0.end()
        assertEquals("the loop restarts in its slot", listOf("0 rewind", "0 play 1.0"), rig.since(mark))
        assertTrue(rig.slot1.loads.isEmpty())
    }

    @Test
    fun aLoneClipsSafetyNetCountsTheLengthOfItsFile() {
        val url = "https://cdn.test/m/wave.mp4"
        val rig = Rig(clip = url, lengths = mapOf(url to 3.0))
        rig.turn()
        rig.slot0.firstFrame()
        rig.advance(3_999)
        assertFalse(rig.heard.events.contains("error:missing_end"))
        rig.advance(1)
        assertTrue("3 s + 1 s after the first frame", rig.heard.events.contains("error:missing_end"))
    }

    @Test
    fun aLoneClipWhoseLengthCannotBeReadPlaysNothing() {
        val rig = Rig(clip = "https://cdn.test/m/wave.mp4")
        rig.turn()
        assertEquals(listOf("error:clip_failed"), rig.heard.events)
        assertTrue(rig.slot0.loads.isEmpty())
        assertTrue(rig.stage.released)
    }

    @Test
    fun aLoneClipsOwnVoiceIsHeardUntilSoundIsTurnedOff() {
        val url = "https://cdn.test/m/wave.mp4"
        val rig = Rig(clip = url, lengths = mapOf(url to 3.0), voiced = setOf(url))
        rig.turn()
        rig.slot0.firstFrame()
        assertEquals("sound is on by default: the file's voice is heard", 1f, rig.slot0.level, 0f)
        rig.player.sound = false
        assertEquals(0f, rig.slot0.level, 0f)
    }

    @Test
    fun aLoneClipWithoutASoundTrackStaysSilent() {
        val url = "https://cdn.test/m/wave.mp4"
        val rig = Rig(clip = url, lengths = mapOf(url to 3.0))
        rig.turn()
        rig.slot0.firstFrame()
        assertTrue(rig.player.sound)
        assertEquals("no sound track: the clip never asks for the audio", 0f, rig.slot0.level, 0f)
    }

    @Test
    fun aStageThatIsNotReadyDelaysTheStart() {
        val rig = Rig(ready = false)
        rig.advance(5_000)
        assertTrue(rig.slot0.loads.isEmpty())
        assertTrue(rig.heard.events.isEmpty())
        rig.stage.becomeReady()
        rig.turn()
        assertEquals(listOf("pose:idle"), rig.heard.events)
        assertEquals(1, rig.slot0.loads.size)
    }

    @Test
    fun aPoseClipShownAgainRestartsInPlaceWithoutASwap() {
        val rig = Rig().started()
        rig.advance(2_000)
        rig.slot0.time = 2.0
        val mark = rig.log.size
        rig.slot0.end()
        assertEquals(listOf("0 rewind", "0 play 1.0"), rig.since(mark))
        assertEquals(0.0, rig.slot0.time, 0.0)
        assertEquals(1, rig.slot0.loads.size)
        assertTrue(rig.slot1.loads.isEmpty())
        assertTrue(rig.slot0.shown)
        assertEquals(listOf("pose:idle", "clip:loop-idle", "clip:loop-idle"), rig.heard.events)
        // The looping sound goes on: it is not played a second time.
        assertEquals(1, rig.log.count { it.startsWith("sound hum.mp3") })
    }

    // ------------------------------------------------------------------ instant (rules.md 5.8)

    @Test
    fun anInstantMovePlaysItsOwnClipAtOnce() {
        val rig = Rig().started()
        rig.advance(500)
        rig.player.set("working", true, instant = true)
        assertEquals(listOf("https://cdn.test/m/to-work.mp4?sig=1"), rig.slot1.loads)
        rig.slot1.firstFrame()
        // Mid-loop, not at its end: the move shows with its movement and its sound.
        assertTrue(rig.heard.events.contains("clip:to-work"))
        assertTrue(rig.heard.events.any { it.startsWith("movement:to-work") })
        assertFalse(rig.heard.events.contains("pose:working"))
        assertTrue(rig.log.any { it.startsWith("sound whoosh.mp3") })
    }

    @Test
    fun anInstantFirePlaysItsMoveAtOnce() {
        val rig = Rig().started()
        rig.player.set("pace", 2, instant = true)
        rig.player.set("pace", 1.5, instant = true)
        rig.player.fire("working", instant = true)
        rig.slot1.firstFrame()
        assertTrue(rig.heard.events.contains("clip:to-work"))
    }

    @Test
    fun aMoveWaitsForItsFirstFrameThenSwapsInOneTurn() {
        val rig = Rig().started()
        rig.advance(500)
        rig.player.set("working", true)
        // An app's move waits for the end of the loop, and loads now in the hidden slot.
        assertEquals(listOf("https://cdn.test/m/to-work.mp4?sig=1"), rig.slot1.loads)
        assertFalse(rig.slot1.shown)

        rig.advance(1_500)
        rig.slot0.end()
        assertTrue("the old clip stays until the new first frame", rig.slot0.shown)
        assertFalse(rig.slot1.shown)
        assertFalse(rig.heard.events.contains("clip:to-work"))

        rig.advance(30)
        val mark = rig.log.size
        rig.slot1.firstFrame()
        assertEquals(listOf("1 show", "0 hide", "0 pause", "1 play 1.0"), rig.since(mark).take(4))
        assertTrue(rig.heard.events.containsAll(listOf("clip:to-work", "movement:to-work")))
        assertEquals(120, rig.heard.movements.single().second.getInt("x"))
        assertEquals("one load per slot: no reload at the swap", 1, rig.slot1.loads.size)
    }

    @Test
    fun aClipIsReportedShownAtTheSwap() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.advance(2_000)
        rig.slot0.end()
        // to-work is asked for at 2000 and swaps in at 2030, with its first frame.
        rig.advance(30)
        rig.slot1.firstFrame()
        rig.heard.events.clear()
        // Its safety net counts from the swap: 2030 + 1 s + 1 s.
        rig.advance(1_999)
        assertTrue(rig.heard.events.isEmpty())
        rig.advance(1)
        assertEquals(listOf("error:missing_end", "pose:working"), rig.heard.events)
    }

    @Test
    fun aPoseClipRestartedInPlaceIsReportedShownAtOnce() {
        val rig = Rig().started()
        rig.advance(2_000)
        rig.slot0.end()
        rig.heard.events.clear()
        // The restart at 2000 is on screen at once: its safety net is 2 s + 1 s later, not a timeout 10 s later.
        rig.advance(2_999)
        assertTrue(rig.heard.events.isEmpty())
        rig.advance(1)
        assertEquals(listOf("error:missing_end", "clip:loop-idle"), rig.heard.events)
        // Each restart by the safety net is shown at once too.
        rig.advance(10_000)
        assertEquals(4, rig.heard.events.count { it == "error:missing_end" })
        assertFalse(rig.heard.events.contains("error:clip_timeout"))
    }

    @Test
    fun eventsCarryTheSameDataAsOnTheWeb() {
        val fast = TestDelivery.json().replace("\"id\": \"to-work\",", "\"id\": \"to-work\", \"speed\": 2,")
        val rig = Rig(delivery = fast).started()
        rig.player.set("working", true)
        rig.slot1.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        rig.player.set("node::loopCount", 1)
        assertEquals(listOf("loop-idle POSE 1.0", "to-work MOVE 2.0"), rig.heard.clips)
        assertEquals(listOf(2.0 to 0.5), rig.heard.timings)
        assertEquals(listOf("node::loopCount"), rig.heard.details)
        assertEquals("error:read_only", rig.heard.events.last())

        val napping = Rig().started()
        napping.player.set("napping", true)
        napping.advance(2_000)
        napping.slot0.end()
        napping.stage.stillLoaded()
        assertEquals("still:napping STILL 1.0", napping.heard.clips.last())
    }

    @Test
    fun aClipReadyInTheHiddenSlotSwapsAtOnceAndTheMoveEndEntersItsTarget() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.slot1.firstFrame()
        rig.advance(2_000)
        val mark = rig.log.size
        rig.slot0.end()
        val swap = rig.since(mark)
        assertEquals(listOf("sounds stop all", "1 show", "0 hide", "0 pause", "1 play 1.0"), swap.take(5))
        assertTrue(swap.contains("sound whoosh.mp3 1.0 once"))
        // During the move, the target's main clip loads in the slot just hidden.
        assertEquals("https://cdn.test/m/loop-work.mp4?sig=1", rig.slot0.loads.last())

        rig.slot0.firstFrame()
        rig.advance(1_000)
        rig.heard.events.clear()
        val atEnd = rig.log.size
        rig.slot1.end()
        assertEquals(listOf("pose:working", "clip:loop-work"), rig.heard.events)
        assertTrue(rig.since(atEnd).containsAll(listOf("0 show", "1 hide", "1 pause", "0 play 1.0")))
        assertEquals(2, rig.slot0.loads.size)
    }

    @Test
    fun clipsPlayAtTheRateTheEngineGives() {
        val fast = TestDelivery.json().replace("\"id\": \"to-work\",", "\"id\": \"to-work\", \"speed\": 2,")
        val rig = Rig(delivery = fast).started()
        rig.player.set("working", true)
        rig.slot1.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        assertTrue(rig.slot1.playing)
        assertEquals(2.0, rig.slot1.rate, 0.0)
        // At speed 2 the 1 s move should end after 500 ms; its safety net is 1 s later.
        rig.advance(1_499)
        assertFalse(rig.heard.events.contains("error:missing_end"))
        rig.advance(1)
        assertTrue(rig.heard.events.contains("error:missing_end"))
    }

    // ------------------------------------------------------------------ ends and failures

    @Test
    fun endsAreReportedOncePerShowingAndOnlyForTheClipAskedFor() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.advance(2_000)
        rig.slot0.end()
        // to-work is now asked for; a second end of the old clip is not reported.
        rig.slot0.end()
        rig.slot1.firstFrame()
        rig.advance(1_000)
        rig.slot1.end()
        rig.slot1.end()
        assertEquals(1, rig.heard.events.count { it == "pose:working" })
        assertFalse(rig.heard.events.any { it.startsWith("error:") })
    }

    @Test
    fun aMoveThatFailsToLoadReportsClipFailedAndSkipsToItsTarget() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.advance(2_000)
        rig.slot0.end()
        rig.heard.events.clear()
        rig.slot1.fail()
        assertEquals(listOf("error:clip_failed", "pose:working"), rig.heard.events)
        assertEquals("https://cdn.test/m/loop-work.mp4?sig=1", rig.slot1.loads.last())
        assertTrue("the old clip stays on screen until the next first frame", rig.slot0.shown)
        rig.slot1.firstFrame()
        assertTrue(rig.slot1.shown)
        assertFalse(rig.slot0.shown)
    }

    @Test
    fun anEdgeWithoutAStackedFileFailsAndTheEngineSkipsToItsTarget() {
        val rig = Rig().started()
        rig.player.set("napping", true)
        assertTrue("no stacked file: nothing to load in advance", rig.slot1.loads.isEmpty())
        rig.advance(2_000)
        rig.heard.events.clear()
        rig.slot0.end()
        assertEquals(listOf("error:clip_failed", "pose:napping"), rig.heard.events)
        // napping has no clips: its still.
        assertTrue(rig.log.last().startsWith("still load still-napping.png"))
        assertTrue(rig.slot0.shown)
        rig.stage.stillLoaded()
        assertFalse(rig.slot0.shown)
        assertEquals("https://cdn.test/m/still-napping.png?sig=1", rig.stage.still)
        assertEquals("clip:still:napping", rig.heard.events.last())
        assertEquals(MaskoPlayer.Position("still:napping", 0.0), rig.player.position())

        // Waking up: a still is the pose's own picture, so the move starts at once and replaces it after its first frame.
        rig.player.set("napping", false)
        assertEquals("https://cdn.test/m/nap-wake.mp4?sig=1", rig.slot1.loads.last())
        assertEquals("https://cdn.test/m/still-napping.png?sig=1", rig.stage.still)
        val mark = rig.log.size
        rig.slot1.firstFrame()
        assertEquals(listOf("1 show", "still hide", "1 play 1.0"), rig.since(mark).take(3))
        assertNull(rig.stage.still)
    }

    @Test
    fun aPoseClipThatFailsOnScreenShowsTheStill() {
        val rig = Rig().started()
        rig.advance(700)
        rig.slot0.fail()
        assertEquals("error:clip_failed", rig.heard.events.last())
        assertTrue(rig.stage.stillLoading)
        rig.stage.stillLoaded()
        assertEquals("clip:still:idle", rig.heard.events.last())
        assertFalse(rig.slot0.shown)
        assertFalse(rig.slot0.playing)
    }

    @Test
    fun aMissingEndIsCaughtByTheSafetyNet() {
        val rig = Rig().started()
        rig.advance(2_999)
        assertFalse(rig.heard.events.contains("error:missing_end"))
        rig.advance(1)
        assertTrue(rig.heard.events.contains("error:missing_end"))
        assertEquals("the loop goes on in place", listOf("0 rewind", "0 play 1.0"), rig.log.takeLast(2))
    }

    @Test
    fun aSlowFirstFrameDoesNotTripTheSafetyNet() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.advance(2_000)
        rig.slot0.end()
        // to-work takes 2.5 s to show: longer than its 1 s and the safety net's 1 s together.
        rig.advance(2_500)
        assertFalse(rig.heard.events.any { it.startsWith("error:") })
        assertTrue("the old clip stays meanwhile", rig.slot0.shown)
        rig.slot1.firstFrame()
        rig.advance(1_000)
        rig.slot1.end()
        assertFalse(rig.heard.events.any { it.startsWith("error:") })
        assertEquals(listOf("pose:working"), rig.heard.events.filter { it.startsWith("pose:") }.drop(1))
    }

    @Test
    fun aClipThatNeverShowsTimesOutAfterTenSeconds() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.advance(2_000)
        rig.slot0.end()
        rig.heard.events.clear()
        rig.advance(9_999)
        assertTrue(rig.heard.events.isEmpty())
        rig.advance(1)
        assertEquals(listOf("error:clip_timeout", "pose:working"), rig.heard.events)
        // The target's clip takes the slot over; the old clip stays on screen until its first frame.
        assertEquals("https://cdn.test/m/loop-work.mp4?sig=1", rig.slot1.loads.last())
        assertTrue(rig.slot0.shown)
        rig.slot1.firstFrame()
        assertEquals("clip:loop-work", rig.heard.events.last())
        assertFalse(rig.slot0.shown)
    }

    @Test
    fun aClipThatTimedOutPlaysAgainOnceItLoads() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.advance(2_000)
        rig.slot0.end()
        rig.advance(10_000)
        rig.slot1.firstFrame()
        assertTrue(rig.heard.events.contains("error:clip_timeout"))

        // Back to idle, where to-work is preloaded again, then idle's named clip; this time it loads.
        val before = rig.stage.preloads.size
        rig.player.reset()
        rig.slot0.firstFrame()
        assertEquals(
            listOf("https://cdn.test/m/to-work.mp4?sig=1", "https://cdn.test/m/boop-idle.mp4?sig=1"),
            rig.stage.preloads.drop(before).map { it.first },
        )
        rig.toWorkPreload().second(true)
        rig.heard.events.clear()
        rig.player.set("working", true)
        rig.slot1.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        assertEquals(listOf("clip:to-work", "movement:to-work"), rig.heard.events)
    }

    // ------------------------------------------------------------------ preloading

    @Test
    fun enteringAPosePreloadsItsMovesWithAStackedFile() {
        val rig = Rig()
        rig.turn()
        // Its moves first, then its named clips (rules 9.7).
        assertEquals(
            listOf("https://cdn.test/m/to-work.mp4?sig=1", "https://cdn.test/m/boop-idle.mp4?sig=1"),
            rig.stage.preloads.map { it.first },
        )
    }

    @Test
    fun aFailedPreloadMakesTheEngineSkipTheClipUntilItLoads() {
        val rig = Rig().started()
        rig.toWorkPreload().second(false)
        rig.player.set("working", true)
        assertTrue("an unplayable clip is not warmed", rig.slot1.loads.isEmpty())
        rig.advance(2_000)
        rig.heard.events.clear()
        rig.slot0.end()
        assertEquals(listOf("error:clip_failed", "pose:working", "clip:loop-work").take(2), rig.heard.events.take(2))
        assertFalse(rig.slot1.loads.any { it.contains("to-work") })

        // Back to idle; this time the preload works and the move plays again.
        rig.player.set("working", false)
        rig.player.reset()
        rig.toWorkPreload().second(true)
        rig.player.set("working", true)
        assertTrue(rig.stage.slots.any { s -> s.loads.any { it.contains("to-work") } })
    }

    // ------------------------------------------------------------------ sounds and voices

    @Test
    fun edgeSoundsPlayAndStopAsTheEngineSays() {
        val rig = Rig().started()
        assertEquals(listOf("hum.mp3" to true), rig.sounds.playing)
        assertTrue(rig.log.contains("sound hum.mp3 0.5 loop"))
        rig.player.set("working", true)
        rig.slot1.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        // A move stops every sound before its own.
        assertEquals(listOf("whoosh.mp3" to false), rig.sounds.playing)
        val i = rig.log.indexOf("sounds stop all")
        assertTrue(i >= 0 && rig.log.indexOf("sound whoosh.mp3 1.0 once") > i)
    }

    @Test
    fun clipVoicesAreHeardOnlyWithSoundOn() {
        val rig = Rig().started()
        rig.player.set("working", true)
        rig.slot1.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        rig.slot0.firstFrame()
        rig.advance(1_000)
        rig.slot1.end()
        // loop-work has a voice; sound is on by default.
        assertTrue(rig.slot0.shown)
        assertEquals(1f, rig.slot0.level, 0f)
        rig.player.sound = false
        assertEquals(0f, rig.slot0.level, 0f)
        rig.player.sound = true
        assertEquals(1f, rig.slot0.level, 0f)

        val voiced = Rig(options = MaskoPlayer.Options(sound = true)).started()
        assertEquals("loop-idle has no voice", 0f, voiced.slot0.level, 0f)
    }

    // ------------------------------------------------------------------ events

    @Test
    fun firesReportWhetherAStartUsedThem() {
        val rig = Rig().started()
        rig.player.fire("boop")
        rig.advance(2_000)
        rig.slot0.end()
        // The named clip plays at the end of the loop, in the other slot.
        assertEquals("https://cdn.test/m/boop-idle.mp4?sig=1", rig.slot1.loads.last())
        rig.slot1.firstFrame()
        rig.advance(1_000)
        rig.slot1.end()
        assertTrue(rig.heard.events.contains("done:action::boop:used"))

        rig.player.fire("nothing")
        rig.slot0.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        assertTrue(rig.heard.events.contains("done:action::nothing:unused"))
    }

    @Test
    fun previewCallsAndTheirErrorsReachTheApp() {
        val rig = Rig().started()
        rig.player.preview.showPose("nope")
        assertEquals("error:unknown_pose", rig.heard.events.last())
        rig.player.preview.playMove("to-work")
        assertEquals("https://cdn.test/m/to-work.mp4?sig=1", rig.slot1.loads.last())
        rig.slot1.firstFrame()
        assertEquals(listOf("clip:to-work", "movement:to-work"), rig.heard.events.takeLast(2))
    }

    @Test
    fun aRefusedDeliveryReportsItsCodeAndPlaysNothing() {
        val rig = Rig(delivery = TestDelivery.json(contract = 2))
        assertTrue("the decoders are freed at once", rig.stage.released && rig.sounds.released)
        rig.turn()
        assertEquals(listOf("error:unsupported_contract"), rig.heard.events)
        assertEquals(listOf("playerContract 2"), rig.heard.details)
        rig.player.set("working", true)
        rig.player.play()
        rig.advance(10_000)
        assertTrue(rig.slot0.loads.isEmpty() && rig.slot1.loads.isEmpty())
        assertTrue(rig.stage.preloads.isEmpty())
        assertEquals(1, rig.heard.events.size)
    }

    @Test
    fun aDeliveryThatIsNotJsonIsUnreadable() {
        val rig = Rig(delivery = "{not json")
        rig.turn()
        assertEquals(listOf("error:unreadable"), rig.heard.events)
        rig.player.play()
        rig.advance(10_000)
        assertTrue(rig.slot0.loads.isEmpty() && rig.slot1.loads.isEmpty())
        assertEquals(MaskoPlayer.Position(null, 0.0), rig.player.position())
    }

    @Test
    fun everyPlayerRollsItsOwnExtraClips() {
        val delivery = TestDelivery.json(extras = listOf("yawn-idle", "look-idle"))
        fun clips(rig: Rig): List<String> {
            rig.started()
            rig.playClips(40, 2_000)
            assertFalse(rig.heard.events.any { it.startsWith("error:") })
            return rig.heard.clips.map { it.substringBefore(' ') }
        }
        val first = clips(Rig(delivery))
        assertTrue("extras play: $first", first.any { it == "yawn-idle" || it == "look-idle" })
        assertNotEquals("two players roll their own extras", first, clips(Rig(delivery)))
        assertEquals("the same seed rolls the same", clips(Rig(delivery, seed = 7u)), clips(Rig(delivery, seed = 7u)))
    }

    @Test
    fun anEnginePanicInAnyCallIsReportedNotThrown() {
        val triggers = mapOf<String, (Rig) -> Unit>(
            "nextWake" to { it.started() },
            "state" to { it.started() },
            "markUnplayable" to { it.started(); it.toWorkPreload().second(false) },
            "markPlayable" to {
                it.started()
                it.toWorkPreload().second(false)
                it.player.updateDelivery(TestDelivery.json(sig = 2))
                it.toWorkPreload().second(true)
            },
            "setBool" to { it.started(); it.player.set("working", true) },
        )
        for ((call, trigger) in triggers) {
            lateinit var engine: FaultyEngine
            val rig = Rig(makeEngine = { json -> FaultyEngine(MaskoEngine(json)).also { engine = it } })
            engine.failing += call
            trigger(rig)
            assertTrue("$call: ${rig.heard.events}", rig.heard.events.contains("error:engine_failed"))
            assertTrue("$call: ${rig.heard.details}", rig.heard.details.contains("panicked in $call"))
        }
    }

    @Test
    fun callsFromAnotherThreadThrowAClearError() {
        val rig = Rig().started()
        val calls = mapOf<String, () -> Unit>(
            "set" to { rig.player.set("working", true) },
            "fire" to { rig.player.fire("boop") },
            "reset" to { rig.player.reset() },
            "play" to { rig.player.play() },
            "pause" to { rig.player.pause() },
            "position" to { rig.player.position() },
            "sound" to { rig.player.sound = true },
            "updateDelivery" to { rig.player.updateDelivery(TestDelivery.json(sig = 2)) },
            "preview.showPose" to { rig.player.preview.showPose("idle") },
            "destroy" to { rig.player.destroy() },
        )
        for ((name, call) in calls) {
            var error: Throwable? = null
            val thread = Thread({ error = runCatching(call).exceptionOrNull() }, "worker")
            thread.start()
            thread.join()
            assertTrue("$name: $error", error is IllegalStateException)
            assertEquals("MaskoPlayer.$name must be called on the main thread, not on thread \"worker\"", error!!.message)
        }
        assertTrue("nothing happened", rig.slot1.loads.isEmpty() && !rig.player.isPaused && !rig.stage.released)
    }

    @Test
    fun aViewWithoutASurfaceIsReportedAndPlaysOnceItHasOne() {
        val rig = Rig(ready = false)
        rig.stage.fail("the view is 300 x 0 pixels")
        rig.turn()
        assertEquals(listOf("error:surface_unavailable"), rig.heard.events)
        assertEquals(listOf("the view is 300 x 0 pixels"), rig.heard.details)
        rig.stage.becomeReady()
        rig.turn()
        assertEquals("pose:idle", rig.heard.events.last())
    }

    @Test
    fun aDetachedViewKeepsNothingQueuedAndCatchesUpWhenBack() {
        var asked = 0
        val rig = Rig(options = MaskoPlayer.Options(getDelivery = { asked++ })).started()
        rig.player.setHostActive(false)
        rig.player.setHostAttached(false)
        assertTrue("the sounds' decoders are freed", rig.sounds.detached)
        assertTrue("nothing is queued on the main thread", rig.scheduler.dueAt().isEmpty())
        // An hour away, past the refresh due at 55 minutes.
        rig.advance(60 * 60_000L)
        assertEquals(0, asked)
        rig.player.setHostAttached(true)
        rig.player.setHostActive(true)
        rig.turn()
        assertEquals("the refresh that came due runs at once", 1, asked)
        assertTrue(rig.slot0.playing)

        // Detached while getDelivery is asked: nothing waits for the reply, and the player asks again when back.
        val replies = ArrayList<(String?) -> Unit>()
        val asking = Rig(options = MaskoPlayer.Options(getDelivery = { r -> replies += r })).started()
        asking.advance(55 * 60_000L)
        assertEquals(1, replies.size)
        asking.player.setHostActive(false)
        asking.player.setHostAttached(false)
        assertTrue(asking.scheduler.dueAt().isEmpty())
        replies[0](TestDelivery.json(sig = 2, expires = "2026-10-23T14:00:00Z"))
        asking.turn()
        asking.player.setHostAttached(true)
        asking.player.setHostActive(true)
        asking.turn()
        assertEquals(2, replies.size)
        assertFalse(asking.heard.events.any { it.startsWith("error:refresh") })
    }

    // ------------------------------------------------------------------ position, pause and play

    @Test
    fun positionReadsTheClipOnScreen() {
        val rig = Rig()
        rig.turn()
        assertEquals(MaskoPlayer.Position(null, 0.0), rig.player.position())
        rig.slot0.firstFrame()
        rig.slot0.time = 1.25
        assertEquals(MaskoPlayer.Position("loop-idle", 1.25), rig.player.position())
    }

    @Test
    fun pauseStopsPlayTimeAndKeepsCallsUntilPlay() {
        val rig = Rig().started()
        rig.advance(500)
        rig.player.pause()
        assertTrue(rig.player.isPaused)
        assertFalse(rig.slot0.playing)
        assertTrue(rig.sounds.paused)
        rig.player.set("working", true)
        assertTrue("decided on when play resumes", rig.slot1.loads.isEmpty())
        // Ten seconds of wall time: no timer runs, so no safety net fires.
        rig.advance(10_000)
        assertFalse(rig.heard.events.contains("error:missing_end"))

        rig.player.play()
        assertTrue(rig.slot0.playing)
        assertFalse(rig.sounds.paused)
        assertEquals(listOf("https://cdn.test/m/to-work.mp4?sig=1"), rig.slot1.loads)
        // Play time resumed at 500: the safety net is 2 s + 1 s after the clip was shown at 0.
        rig.advance(2_499)
        assertFalse(rig.heard.events.contains("error:missing_end"))
        rig.advance(1)
        assertTrue(rig.heard.events.contains("error:missing_end"))
    }

    @Test
    fun autoPlayFalseShowsTheFirstFrameAndWaitsForPlay() {
        for (rig in listOf(Rig(options = MaskoPlayer.Options(autoPlay = false)), Rig(delivery = TestDelivery.json(autoPlay = false)))) {
            rig.started()
            assertTrue(rig.slot0.shown)
            assertFalse(rig.slot0.playing)
            assertTrue(rig.player.isPaused)
            rig.advance(10_000)
            assertFalse(rig.heard.events.contains("error:missing_end"))
            rig.player.play()
            assertTrue(rig.slot0.playing)
            rig.advance(3_000)
            assertTrue(rig.heard.events.contains("error:missing_end"))
        }
    }

    @Test
    fun theAppsAutoPlayWinsOverTheGraphs() {
        val rig = Rig(delivery = TestDelivery.json(autoPlay = false), options = MaskoPlayer.Options(autoPlay = true)).started()
        assertFalse(rig.player.isPaused)
        assertTrue(rig.slot0.playing)
        val held = Rig(options = MaskoPlayer.Options(autoPlay = false)).started()
        assertTrue(held.player.isPaused)
        assertFalse(held.slot0.playing)
    }

    @Test
    fun valuesSetBeforeTheStartApplyAtTheStart() {
        val rig = Rig()
        rig.player.set("working", true)
        assertTrue(rig.log.isEmpty())
        rig.started()
        // The mascot enters idle, where the move holds: at a boundary it starts at once (rules.md 5.1).
        assertEquals(listOf("pose:idle", "clip:to-work", "movement:to-work"), rig.heard.events)
        assertEquals("https://cdn.test/m/to-work.mp4?sig=1", rig.slot0.loads.single())
    }

    // ------------------------------------------------------------------ signed links

    @Test
    fun signedLinksAreRefreshedBeforeTheyExpireAndOnlyTheLinksChange() {
        var asked = 0
        var reply: ((String?) -> Unit)? = null
        val rig = Rig(options = MaskoPlayer.Options(getDelivery = { r -> asked++; reply = r })).started()
        rig.player.pause()
        // Expires at 13:00, an hour from now: refreshed five minutes before.
        rig.advance(55 * 60_000L - 1)
        assertEquals(0, asked)
        rig.advance(1)
        assertEquals(1, asked)
        reply!!(TestDelivery.json(sig = 2, expires = "2026-10-23T14:00:00Z"))
        rig.turn()
        assertFalse(rig.heard.events.any { it.startsWith("error:") })
        assertEquals("the clip on screen keeps playing", 1, rig.slot0.loads.size)

        rig.player.play()
        rig.player.set("working", true)
        assertEquals("https://cdn.test/m/to-work.mp4?sig=2", rig.slot1.loads.last())
        // The next refresh: five minutes before the new expiry, 14:00.
        assertEquals(listOf(rig.time.now + 60 * 60_000L), rig.scheduler.dueAt().filter { it > rig.time.now + 60_000 })
    }

    @Test
    fun aFailedRefreshIsReportedAndTriedAgain() {
        var asked = 0
        val replies = ArrayList<(String?) -> Unit>()
        val rig = Rig(options = MaskoPlayer.Options(getDelivery = { r -> asked++; replies += r })).started()
        rig.player.pause()
        rig.advance(55 * 60_000L)
        replies.last()(null)
        rig.turn()
        assertEquals("error:refresh_failed", rig.heard.events.last())
        // Tried again 30 s later.
        rig.advance(30_000 - 1)
        assertEquals(1, asked)
        rig.advance(1)
        assertEquals(2, asked)
        // Another release's delivery is refused, and tried again 30 s later.
        replies.last()(TestDelivery.json(sig = 2, releaseId = "rel-2"))
        rig.turn()
        assertEquals("error:refresh_mismatch", rig.heard.events.last())
        rig.advance(30_000)
        assertEquals(3, asked)
        replies.last()("{not json")
        rig.turn()
        assertEquals(2, rig.heard.events.count { it == "error:refresh_failed" })
    }

    @Test
    fun linksThatExpireSoonAreRefreshedHalfwayAndNotInALoop() {
        var asked = 0
        val replies = ArrayList<(String?) -> Unit>()
        val rig = Rig(
            delivery = TestDelivery.json(expires = "2026-10-23T12:08:00Z"),
            options = MaskoPlayer.Options(getDelivery = { r -> asked++; replies += r }),
        ).started()
        rig.advance(4 * 60_000L - 1)
        assertEquals(0, asked)
        rig.advance(1)
        assertEquals(1, asked)
        replies.last()(TestDelivery.json(sig = 2, expires = "2026-10-23T12:08:00Z"))
        rig.turn()
        assertFalse(rig.heard.events.any { it.startsWith("error:refresh") })
        // The links last no longer: asked again 30 s later, not at once.
        rig.advance(30_000 - 1)
        assertEquals(1, asked)
        rig.advance(1)
        assertEquals(2, asked)
    }

    @Test
    fun updateDeliverySwapsTheLinksOfTheSameReleaseAndRetriesFailedClips() {
        val rig = Rig().started()
        rig.toWorkPreload().second(false)
        rig.heard.events.clear()
        assertTrue(rig.player.updateDelivery(TestDelivery.json(sig = 2, expires = "2026-10-23T14:00:00Z")))
        assertTrue("no error, the engine keeps running", rig.heard.events.isEmpty())
        // The clip that failed is tried again, from its new link.
        assertEquals("https://cdn.test/m/to-work.mp4?sig=2", rig.stage.preloads.last().first)
        rig.stage.preloads.last().second(true)
        rig.player.set("working", true)
        assertEquals("https://cdn.test/m/to-work.mp4?sig=2", rig.slot1.loads.last())
        assertEquals("the clip on screen keeps playing", 1, rig.slot0.loads.size)
        assertTrue(rig.slot0.playing)
    }

    @Test
    fun updateDeliveryRefusesAnotherReleaseAndAnUnreadableOne() {
        val rig = Rig().started()
        rig.heard.events.clear()
        // Another graph: the links alone may change.
        val renamed = TestDelivery.json(sig = 2).replace("\"name\": \"Test\"", "\"name\": \"Other\"")
        assertFalse(rig.player.updateDelivery(renamed))
        assertFalse(rig.player.updateDelivery(TestDelivery.json(sig = 2, releaseId = "rel-2")))
        assertFalse(rig.player.updateDelivery("{not json"))
        assertEquals(listOf("error:refresh_mismatch", "error:refresh_mismatch", "error:refresh_failed"), rig.heard.events)
        rig.player.set("working", true)
        assertEquals("the old links stay", "https://cdn.test/m/to-work.mp4?sig=1", rig.slot1.loads.last())
    }

    @Test
    fun aGetDeliveryThatThrowsIsAFailedRefreshNotACrash() {
        var asked = 0
        val rig = Rig(options = MaskoPlayer.Options(getDelivery = { asked++; throw IllegalStateException("offline") })).started()
        rig.player.pause()
        rig.advance(55 * 60_000L)
        assertEquals(1, asked)
        assertEquals("error:refresh_failed", rig.heard.events.last())
        assertEquals("offline", rig.heard.details.last())
        rig.advance(30_000 - 1)
        assertEquals(1, asked)
        rig.advance(1)
        assertEquals(2, asked)
    }

    @Test
    fun aReplyThatNeverComesTimesOutAndALateOneIsIgnored() {
        val replies = ArrayList<(String?) -> Unit>()
        val rig = Rig(options = MaskoPlayer.Options(getDelivery = { r -> replies += r })).started()
        rig.player.pause()
        rig.advance(55 * 60_000L)
        assertEquals(1, replies.size)
        rig.advance(60_000 - 1)
        assertFalse(rig.heard.events.contains("error:refresh_failed"))
        rig.advance(1)
        assertEquals("error:refresh_failed", rig.heard.events.last())
        assertEquals("getDelivery did not reply within 60 seconds", rig.heard.details.last())
        rig.advance(30_000)
        assertEquals("asked again 30 s later", 2, replies.size)
        // The first reply comes now: too late, dropped. The second is taken.
        replies[0](TestDelivery.json(sig = 3, expires = "2026-10-23T15:00:00Z"))
        replies[1](TestDelivery.json(sig = 2, expires = "2026-10-23T14:00:00Z"))
        rig.turn()
        assertEquals(1, rig.heard.events.count { it.startsWith("error:") })
        rig.player.play()
        rig.player.set("working", true)
        assertEquals("https://cdn.test/m/to-work.mp4?sig=2", rig.slot1.loads.last())
    }

    @Test
    fun linksAreCheckedAgainstTheWallClockWhenTheViewIsBack() {
        // Deep sleep past the refresh: the wall clock moves on, the uptime timer does not.
        var asked = 0
        val slept = Rig(options = MaskoPlayer.Options(getDelivery = { asked++ })).started()
        slept.player.setHostActive(false)
        slept.slept = 56 * 60_000L
        slept.advance(1_000)
        assertEquals(0, asked)
        slept.player.setHostActive(true)
        slept.turn()
        assertEquals("due on the wall clock: asked at once", 1, asked)

        // Deep sleep for half an hour: the refresh is still at 12:55 on the wall clock.
        var later = 0
        val rig = Rig(options = MaskoPlayer.Options(getDelivery = { later++ })).started()
        rig.player.setHostActive(false)
        rig.slept = 30 * 60_000L
        rig.advance(1_000)
        rig.player.setHostActive(true)
        rig.advance(25 * 60_000L - rig.time.now - 1)
        assertEquals(0, later)
        rig.advance(1)
        assertEquals(1, later)
    }

    // ------------------------------------------------------------------ the file and the size

    @Test
    fun theFileFollowsTheSizeOnScreen() {
        for ((size, file) in listOf(150 to "loop-idle-200.mp4", 300 to "loop-idle-800.mp4", 1_000 to "loop-idle.mp4")) {
            val rig = Rig()
            rig.stage.size = size
            rig.turn()
            assertEquals(file, short(rig.slot0.loads.single()))
        }
    }

    @Test
    fun destroyFreesEverythingAndLaterCallsDoNothing() {
        val rig = Rig().started()
        rig.player.destroy()
        assertTrue(rig.stage.released && rig.sounds.released)
        assertTrue(rig.slot0.released && rig.slot1.released)
        assertTrue(rig.scheduler.dueAt().isEmpty())
        val before = rig.log.size
        rig.player.set("working", true)
        rig.player.fire("boop")
        rig.player.play()
        rig.advance(60_000)
        assertEquals(before, rig.log.size)
    }
}
