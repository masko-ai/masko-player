package ai.masko.player

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.IOException
import java.util.UUID

/**
 * A canvas read from its URL with an app key (player.md, "A canvas from its
 * URL"): the request, the kept files, offline play, renewed links, and a
 * newer release taking over when the initial pose's loop starts again.
 */
class CanvasURLTest {
    @get:Rule
    val folder = TemporaryFolder()

    private lateinit var root: File
    private val network = FakeNetwork()
    /** Kept file names to the link's file name, so a clip reads the same kept or not. */
    private val names = HashMap<String, String>()

    @Before
    fun setUp() {
        root = folder.newFolder("kept")
    }

    /** A release whose files are named after it; [work] adds the move to the working pose. */
    private fun release(
        id: String,
        work: Boolean = true,
        sig: Int = 1,
        contract: Int = 1,
        expires: String = "2027-01-15T08:00:00.000Z",
    ): String {
        fun link(name: String): String {
            val link = "https://cdn.test/$id/$name-$id.${if (name.startsWith("still")) "png" else "mp4"}?sig=$sig"
            names[KeptRelease.name(link)] = short(link)
            return link
        }
        val toWork = if (!work) "" else """,
            { "id": "to-work", "source": "idle", "target": "working", "isLoop": false, "duration": 1,
              "conditions": [{ "input": "behavior::working", "op": "==", "value": true }],
              "videos": { "stacked": "${link("to-work")}" } }"""
        return """
        {
          "deliverySchemaVersion": 1, "playerContract": $contract, "requires": [], "target": "android",
          "mode": "selected", "release": { "id": "sha256:$id" },
          "deliveryAccess": { "urlMode": "signed", "signedUrlExpiresAt": "$expires" },
          "runtimeGraph": {
            "runtimeGraphSchemaVersion": 1, "initialNode": "idle", "autoPlay": true,
            "nodes": [
              { "id": "idle", "transparentThumbnailUrl": "${link("still-idle")}" },
              { "id": "working", "transparentThumbnailUrl": "${link("still-working")}" }
            ],
            "inputs": [{ "name": "behavior::working", "type": "boolean", "default": false }],
            "edges": [
              { "id": "idle-loop", "source": "idle", "target": "idle", "isLoop": true, "duration": 2,
                "videos": { "stacked": "${link("idle-loop")}" } },
              { "id": "work-loop", "source": "working", "target": "working", "isLoop": true, "duration": 2,
                "videos": { "stacked": "${link("work-loop")}" } },
              { "id": "to-idle", "source": "working", "target": "idle", "isLoop": false, "duration": 1,
                "conditions": [{ "input": "behavior::working", "op": "==", "value": false }],
                "videos": { "stacked": "${link("to-idle")}" } }$toWork
            ]
          }
        }
        """.trimIndent()
    }

    private fun kept(url: String = URL, appKey: String? = "masko_app_test") =
        KeptRelease(url, appKey, root, "av1", network)

    private fun keep(id: String, work: Boolean = true, expires: String = "2027-01-15T08:00:00.000Z") =
        kept().keep(KeptRelease.Release("rel-$id", release(id, work, expires = expires)))

    /** The canvas URL answers this release. */
    private fun serve(id: String, delivery: String = release(id)) {
        network.latest = delivery
        network.releaseId = "rel-$id"
    }

    private fun rig(ready: Boolean = true) = Rig(delivery = "", ready = ready, kept = kept())

    /** The file a slot loaded, by its link's name, kept or not. */
    private fun clip(slot: FakeSlot): String = short(slot.url).let { names[it] ?: it }

    /** Draws the first frame of every clip loading. */
    private fun Rig.drawn() = stage.slots.filter { it.loading }.forEach { it.firstFrame() }

    // ------------------------------------------------------------------ the request and the kept files

    @Test
    fun theCanvasURLIsAskedWithTheAppKeyThePlatformAndItsFormats() {
        serve("r1")
        kept().latest()
        kept().latest()
        val (url, headers) = network.asked[0]
        assertEquals("$URL?target=android&formats=av1", url)
        assertEquals("Bearer masko_app_test", headers["Authorization"])
        val install = headers["Masko-Install-Id"]!!
        UUID.fromString(install)
        assertEquals("kept by this copy", install, network.asked[1].second["Masko-Install-Id"])
    }

    @Test
    fun aURLThatCarriesItsKeyIsAskedWithItAndNoHeader() {
        serve("r1")
        kept("$URL?key=masko_app_test", appKey = null).latest()
        val (url, headers) = network.asked[0]
        assertEquals("$URL?key=masko_app_test&target=android&formats=av1", url)
        assertNull(headers["Authorization"])
        assertNotNull(headers["Masko-Install-Id"])
    }

    @Test
    fun aURLThatNamesItsTargetAndFormatsIsKeptAsItIs() {
        assertEquals("$URL?formats=&target=android", KeptRelease.request("$URL?formats=&target=android", "av1"))
        assertEquals("$URL?target=android&formats=", KeptRelease.request(URL, ""))
    }

    @Test
    fun theReleaseIsAskedAgainFromItsOwnURL() {
        assertEquals(
            "https://api.masko.test/v1/canvases/c1/releases/rel-1/delivery?key=k&target=android&formats=av1",
            KeptRelease.releaseRequest("$URL?key=k&target=android&formats=av1", "rel-1"),
        )
    }

    @Test
    fun aRefusalIsAFailureWithItsMessage() {
        network.status = 401
        try {
            kept().latest()
            fail("a 401 is a failure")
        } catch (e: KeptRelease.Failure) {
            assertEquals("the canvas URL answered 401: Invalid API key", e.message)
        }
    }

    @Test
    fun everyFileIsDownloadedOnceAndTheReleaseKeptOnlyWhenComplete() {
        val release = kept()
        assertNull(release.kept())
        keep("r1")
        assertEquals("four clips and two stills", 6, network.downloaded.size)
        assertEquals("rel-r1", release.kept()!!.id)
        keep("r1")
        assertEquals("a file kept is never downloaded again", 6, network.downloaded.size)
        assertTrue(release.local("https://cdn.test/r1/idle-loop-r1.mp4?sig=9")!!.startsWith("file://"))

        // A missing file: the release is not complete any more.
        assertTrue(release.file("https://cdn.test/r1/to-work-r1.mp4").delete())
        assertNull(release.kept())
        assertNull(release.local("https://cdn.test/r1/to-work-r1.mp4"))
    }

    @Test
    fun aFileNamedAfterItsPathOutlivesItsSignature() {
        val signed = KeptRelease.name("https://cdn.test/r1/idle-loop-r1.mp4?Expires=1&Signature=a")
        assertEquals(signed, KeptRelease.name("https://cdn.test/r1/idle-loop-r1.mp4?Expires=2&Signature=b"))
        assertTrue(signed.endsWith(".mp4"))
    }

    @Test
    fun aFileThatArrivesIncompleteIsNotKept() {
        network.shortBy = 1
        try {
            keep("r1")
            fail("an incomplete file is a failure")
        } catch (_: KeptRelease.Failure) {
        }
        assertNull(kept().kept())
        assertEquals("nothing half there", 0, kept().files.listFiles()?.size ?: 0)
    }

    @Test
    fun filesOfOlderReleasesAreDeletedBeforeAnythingPlays() {
        keep("r1")
        keep("r2")
        assertEquals(12, kept().files.listFiles()!!.size)
        kept().removeUnused()
        assertEquals(6, kept().files.listFiles()!!.size)
        assertEquals("the kept release keeps every file", "rel-r2", kept().kept()!!.id)
    }

    // ------------------------------------------------------------------ playing

    @Test
    fun theFirstDeliveryPlaysAtOnceAndEveryFileIsKept() {
        serve("r1")
        val rig = rig()
        rig.player.set("behavior::working", true)
        rig.player.fire("action::dropped")
        rig.turn()
        assertEquals("the value set before chose the first pose", "to-work-r1.mp4", clip(rig.slot0))
        rig.slot0.firstFrame()
        assertEquals(listOf("pose:idle", "clip:to-work"), rig.heard.events)
        assertEquals("every file kept", "rel-r1", kept().kept()!!.id)
        assertEquals("asked again in an hour", TestDelivery.WALL_START + HOUR, rig.player.checkAt)
    }

    @Test
    fun aKeptReleasePlaysOffline() {
        keep("r1")
        network.offline = true
        val rig = rig()
        rig.turn()
        assertTrue("from its kept file", rig.slot0.url!!.startsWith("file://"))
        assertEquals("idle-loop-r1.mp4", clip(rig.slot0))
        rig.slot0.firstFrame()
        assertEquals(listOf("pose:idle", "error:refresh_failed", "clip:idle-loop"), rig.heard.events)
        assertEquals("it asks in an hour", TestDelivery.WALL_START + HOUR, rig.player.checkAt)
    }

    @Test
    fun aFirstDeliveryThatDoesNotComeIsAskedForLessAndLessOften() {
        network.offline = true
        val rig = rig()
        rig.turn()
        assertEquals(listOf("error:refresh_failed"), rig.heard.events)
        assertTrue(rig.slot0.loads.isEmpty())
        assertEquals(TestDelivery.WALL_START + 30_000, rig.player.checkAt)
        rig.advance(30_000)
        assertEquals(TestDelivery.WALL_START + 30_000 + 60_000, rig.player.checkAt)
        network.offline = false
        serve("r1")
        rig.advance(60_000)
        assertEquals("plays once it comes", "idle-loop-r1.mp4", clip(rig.slot0))
        assertEquals(TestDelivery.WALL_START + 90_000 + HOUR, rig.player.checkAt)
    }

    @Test
    fun theSameReleaseOnlyRenewsItsLinks() {
        keep("r1")
        serve("r1", release("r1", sig = 2))
        val rig = rig()
        rig.turn()
        rig.slot0.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        assertEquals("restarted in place, no switch", 1, rig.slot0.loads.size)
        assertEquals("nothing more to download", 6, network.downloaded.size)
        assertFalse(rig.heard.events.any { it.startsWith("error:") })
    }

    @Test
    fun linksAreRenewedFromTheReleasesOwnURL() {
        val soon = "2026-10-23T13:00:00Z" // an hour after the test starts
        keep("r1", expires = soon)
        serve("r1", release("r1", sig = 2, expires = soon))
        val rig = rig()
        rig.turn()
        rig.slot0.firstFrame()
        val asks = network.asked.size
        rig.advance(55 * 60_000L)
        assertEquals(asks + 1, network.asked.size)
        assertEquals(
            "https://api.masko.test/v1/canvases/c1/releases/rel-r1/delivery?target=android&formats=av1",
            network.asked.last().first,
        )
        assertEquals("Bearer masko_app_test", network.asked.last().second["Authorization"])
        assertEquals(emptyList<String>(), rig.heard.events.filter { it.startsWith("error:refresh") })
    }

    @Test
    fun aNewerReleaseBeforeTheStartPlaysFromTheStart() {
        keep("r1")
        serve("r2")
        val rig = rig(ready = false)
        rig.turn()
        rig.stage.becomeReady()
        rig.turn()
        assertEquals(listOf("idle-loop-r2.mp4"), rig.slot0.loads.map { short(it).let { n -> names[n] ?: n } })
        rig.slot0.firstFrame()
        assertEquals(listOf("pose:idle", "clip:idle-loop"), rig.heard.events)
    }

    @Test
    fun aNewerReleaseTakesOverWhenTheInitialLoopStartsAgain() {
        keep("r1")
        serve("r2")
        val rig = rig()
        rig.turn()
        rig.slot0.firstFrame()
        assertEquals("every file of r2 is kept before it plays", 12, network.downloaded.size)
        assertEquals("rel-r2", kept().kept()!!.id)
        rig.heard.events.clear()
        val mark = rig.log.size
        rig.advance(2_000)
        rig.slot0.end()
        // The old loop's last frame stays on screen until the new loop's first
        // frame is drawn in the other slot.
        assertFalse("the old loop is not started again", rig.since(mark).contains("0 rewind"))
        assertEquals("idle-loop-r2.mp4", clip(rig.slot1))
        assertTrue(rig.slot0.shown)
        assertFalse(rig.slot1.shown)
        rig.slot1.firstFrame()
        assertTrue(rig.slot1.shown)
        assertFalse(rig.slot0.shown)
        assertEquals(listOf("pose:idle", "clip:idle-loop"), rig.heard.events)
        assertEquals(Pair("idle-loop", 0.0), rig.player.position().let { it.clip to it.time })
        // It plays on from r2's files.
        rig.player.set("behavior::working", true)
        rig.advance(2_000)
        rig.slot1.end()
        rig.drawn()
        assertEquals("to-work-r2.mp4", clip(rig.slot0))
        assertTrue(rig.slot0.shown)
    }

    @Test
    fun aNewerReleaseWaitsUntilTheMascotIsBackOnItsInitialPose() {
        keep("r1")
        serve("r2")
        val rig = rig()
        rig.player.set("behavior::working", true)
        rig.turn()
        assertEquals("to-work-r1.mp4", clip(rig.slot0))
        rig.slot0.firstFrame()
        rig.advance(1_000)
        rig.slot0.end()
        rig.drawn()
        assertEquals("work-loop-r1.mp4", clip(rig.slot1))
        rig.advance(2_000)
        rig.slot1.end()
        assertEquals("the working loop goes on in r1", "work-loop-r1.mp4", clip(rig.slot1))
        rig.player.set("behavior::working", false)
        rig.advance(2_000)
        rig.slot1.end()
        rig.drawn()
        assertEquals("to-idle-r1.mp4", clip(rig.slot0))
        rig.advance(1_000)
        rig.slot0.end()
        rig.drawn()
        assertEquals("back on the initial pose, still r1", "idle-loop-r1.mp4", clip(rig.slot1))
        rig.advance(2_000)
        rig.slot1.end()
        assertEquals("its loop starting again is the switch", "idle-loop-r2.mp4", clip(rig.slot0))
    }

    @Test
    fun valuesTheAppSetCarryOverToTheNewerRelease() {
        // r1 has no move to the working pose: working changes nothing there.
        keep("r1", work = false)
        serve("r2")
        val rig = rig()
        rig.turn()
        rig.slot0.firstFrame()
        rig.player.set("behavior::working", true)
        rig.advance(2_000)
        rig.slot0.end()
        assertEquals("r1 stayed on its initial pose: the switch", "idle-loop-r2.mp4", clip(rig.slot1))
        rig.slot1.firstFrame()
        rig.advance(2_000)
        rig.slot1.end()
        rig.drawn()
        assertEquals("working is still set, and r2 moves for it", "to-work-r2.mp4", clip(rig.slot0))
    }

    @Test
    fun aReleaseTheEngineRefusesIsReportedAndTheOnePlayingGoesOn() {
        keep("r1")
        serve("r2", release("r2", contract = 99))
        val rig = rig()
        rig.turn()
        assertTrue("reported when it comes", rig.heard.events.last().startsWith("error:"))
        assertEquals("and never downloaded", 6, network.downloaded.size)
        rig.slot0.firstFrame()
        rig.advance(2_000)
        rig.slot0.end()
        assertEquals("r1 goes on", listOf("idle-loop-r1.mp4"), rig.slot0.loads.map { names[short(it)] })
        assertEquals("idle-loop", rig.player.position().clip)
    }

    @Test
    fun destroyStopsAskingAndKeepsTheFiles() {
        serve("r1")
        val rig = rig()
        rig.turn()
        rig.player.destroy()
        val asks = network.asked.size
        rig.advance(2 * HOUR)
        assertEquals(asks, network.asked.size)
        assertNotNull(kept().kept())
    }

    private companion object {
        const val URL = "https://api.masko.test/v1/canvases/c1/delivery"
        const val HOUR = 60 * 60_000L
    }
}

/** The canvas URL and the file links, answered in memory. */
internal class FakeNetwork : CanvasNetwork {
    /** The delivery the canvas URL answers. */
    var latest: String? = null
    var releaseId: String? = null
    var status = 200
    var offline = false
    /** Files arrive this many bytes short of their Content-Length. */
    var shortBy = 0
    val asked = ArrayList<Pair<String, Map<String, String>>>()
    val downloaded = ArrayList<String>()

    override fun get(url: String, headers: Map<String, String>): CanvasNetwork.Answer {
        asked += url to headers.toMap()
        if (offline) throw IOException("offline")
        if (status != 200) return CanvasNetwork.Answer(status, """{"error":{"message":"Invalid API key"}}""")
        val data = JSONObject()
            .put("release_id", releaseId ?: JSONObject.NULL)
            .put("number", 1)
            .put("delivery", latest?.let(::JSONObject) ?: JSONObject.NULL)
        return CanvasNetwork.Answer(200, JSONObject().put("data", data).toString())
    }

    override fun download(url: String, into: File): CanvasNetwork.Download {
        if (offline) throw IOException("offline")
        downloaded += url
        val bytes = url.toByteArray()
        into.writeBytes(bytes)
        return CanvasNetwork.Download(200, bytes.size.toLong() + shortBy)
    }
}
